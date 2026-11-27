use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::{
    music::{
        ObjectKind,
        likes::{LikeIdentity, timestamp_ms},
    },
    playlist::PlaylistTrack,
    providers::{
        ErrorKind, ProviderError, ProviderId,
        collections::{self, Collection, CollectionKind},
    },
    storage::{Account, Store, StoreError},
};

pub(crate) use super::matching::MatchLinks;
use super::region;

mod index;
mod local;
mod playlist_source;
#[cfg(test)]
mod tests;
#[cfg(feature = "capture")]
pub(crate) use index::source_entries;
pub(crate) use index::{
    Entry, EntryAction, EntryId, EntryKey, LibraryGroup, LibraryIndex, LibraryResults, LibraryRow,
    LibrarySort, Scope,
};
pub(super) use local::like_change as local_like_change;
pub(crate) use local::playlist_entry;
pub(crate) use playlist_source::{
    PlaylistCache, PlaylistReceipt, PlaylistScope, SavedPlaylist, playlist_snapshot_fresh,
};

#[derive(Debug)]
pub(crate) struct LikeChange {
    pub key: EntryKey,
    pub entry: Option<Entry>,
    pub link: Option<(LikeIdentity, LikeIdentity)>,
}

pub(super) fn remote_like_change(
    target: LikeIdentity,
    source: &LikeIdentity,
    provider: ProviderId,
    profile: &str,
    track: PlaylistTrack,
    liked: bool,
) -> LikeChange {
    let scope = Scope::Provider {
        provider,
        profile: profile.to_owned(),
    };
    let key = EntryKey(scope.clone(), EntryId::Source(track.id.clone()));
    let entry = liked.then(|| index::remote_liked_track(scope, provider, profile, track));
    let link = (source != &target).then(|| (source.clone(), target));
    LikeChange { key, entry, link }
}

pub(crate) fn search_saved(
    kind: ObjectKind,
    query: &str,
    cancelled: &AtomicBool,
    emit: impl Fn(Vec<Arc<Entry>>),
) -> Result<(), StoreError> {
    const LOCAL_LIMIT: usize = 30;
    const SAVED_SOURCE_LIMIT: usize = 12;

    let kind = match kind {
        ObjectKind::Track => CollectionKind::Liked,
        ObjectKind::Album => CollectionKind::Albums,
        ObjectKind::Artist => CollectionKind::Artists,
        ObjectKind::Playlist => CollectionKind::Playlists,
        ObjectKind::Station | ObjectKind::Collection => return Ok(()),
    };
    let query = super::normalized(query);
    let words: Vec<_> = query.split_whitespace().collect();
    if words.is_empty() || cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    let store = Store::open_default()?;
    let matches = |entries: Vec<Entry>, limit| {
        entries
            .into_iter()
            .filter(|entry| words.iter().all(|word| entry.matches_word(word)))
            .take(limit)
            .map(Arc::new)
            .collect::<Vec<_>>()
    };
    let local = if kind == CollectionKind::Playlists {
        store
            .local_playlists()?
            .iter()
            .map(playlist_entry)
            .collect()
    } else {
        local::read(&store, kind)?
    };
    emit(matches(local, LOCAL_LIMIT));
    let region = region();
    for summary in store.accounts()? {
        if cancelled.load(Ordering::Relaxed) {
            break;
        }
        if let Some(account) = store.account(summary.provider, &summary.profile.id)?
            && let Some(collection) = cached_data(&store, &account, &region, kind)?
        {
            emit(matches(
                index::source_entries(account.provider, &account.profile.id, collection),
                SAVED_SOURCE_LIMIT,
            ));
        }
    }
    Ok(())
}

impl CollectionKind {
    const fn slot(self) -> &'static str {
        match self {
            Self::Liked => "liked",
            Self::Albums => "saved-albums",
            Self::Artists => "followed-artists",
            Self::Playlists => "playlists",
        }
    }

    fn object_kind(self) -> ObjectKind {
        match self {
            Self::Liked => ObjectKind::Track,
            Self::Albums => ObjectKind::Album,
            Self::Artists => ObjectKind::Artist,
            Self::Playlists => ObjectKind::Playlist,
        }
    }
}

pub(crate) enum CollectionUpdate {
    Data {
        provider: ProviderId,
        profile: String,
        entries: Arc<[Entry]>,
    },
    Local(Vec<Entry>),
    Downloads(std::collections::HashSet<LikeIdentity>),
    Links(MatchLinks),
    Issue(String),
    Error {
        provider: ProviderId,
        profile: String,
        message: String,
        hide: bool,
    },
}

pub(crate) fn load_collections(
    kind: CollectionKind,
    cancelled: &AtomicBool,
    emit: impl Fn(CollectionUpdate) + Sync,
) -> Result<(), StoreError> {
    let store = Store::open_default()?;
    load_from_store(&store, kind, cancelled, emit)
}

fn load_from_store(
    store: &Store,
    kind: CollectionKind,
    cancelled: &AtomicBool,
    emit: impl Fn(CollectionUpdate) + Sync,
) -> Result<(), StoreError> {
    let region = region();
    if kind == CollectionKind::Liked {
        match store.cached_recordings() {
            Ok(downloaded) => emit(CollectionUpdate::Downloads(downloaded)),
            Err(error) => emit(CollectionUpdate::Issue(format!("Аудиокэш · {error}"))),
        }
    }
    match local::read(store, kind) {
        Ok(entries) => emit(CollectionUpdate::Local(entries)),
        Err(error) => emit(CollectionUpdate::Issue(format!(
            "Локальная медиатека · {error}"
        ))),
    }
    match MatchLinks::load(store, &region, kind.object_kind()) {
        Ok(links) => emit(CollectionUpdate::Links(links)),
        Err(error) => emit(CollectionUpdate::Issue(format!("Связи записей · {error}"))),
    }
    let mut accounts = Vec::new();
    for summary in store.accounts()? {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        let provider = summary.provider;
        let Some(account) = store.account(provider, &summary.profile.id)? else {
            continue;
        };
        match cached_data(store, &account, &region, kind) {
            Ok(Some(collection)) => emit(data(&account, collection)),
            Ok(None) => {}
            Err(error) => emit(CollectionUpdate::Error {
                provider,
                profile: account.profile.id.clone(),
                message: error.to_string(),
                hide: false,
            }),
        }
        accounts.push(account);
    }
    std::thread::scope(|scope| {
        for account in accounts {
            let region = &region;
            let emit = &emit;
            scope.spawn(move || {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let update =
                    refresh(store, &account, region, kind, cancelled).unwrap_or_else(|error| {
                        CollectionUpdate::Error {
                            provider: account.provider,
                            profile: account.profile.id.clone(),
                            message: error.to_string(),
                            hide: false,
                        }
                    });
                if !cancelled.load(Ordering::Relaxed) {
                    emit(update);
                }
            });
        }
    });
    Ok(())
}

fn data(account: &Account, collection: Collection) -> CollectionUpdate {
    CollectionUpdate::Data {
        provider: account.provider,
        profile: account.profile.id.clone(),
        entries: index::source_entries(account.provider, &account.profile.id, collection).into(),
    }
}

fn cached_data(
    store: &Store,
    account: &Account,
    region: &str,
    kind: CollectionKind,
) -> Result<Option<Collection>, StoreError> {
    let mut collection =
        store.music_snapshot(account.provider, Some(account), region, kind.slot())?;
    if let Some(Collection::Liked(tracks)) = collection.as_mut() {
        store.apply_liked_changes(account.provider, &account.profile.id, tracks)?;
    } else if collection.is_none() && kind == CollectionKind::Liked {
        let mut tracks = Vec::new();
        store.apply_liked_changes(account.provider, &account.profile.id, &mut tracks)?;
        if !tracks.is_empty() {
            collection = Some(Collection::Liked(tracks));
        }
    }
    Ok(collection)
}

#[cfg(feature = "capture")]
pub(crate) fn spotify_album_snapshot(
    directory: &std::path::Path,
) -> Result<Option<Collection>, StoreError> {
    let store = Store::open(directory)?;
    let provider = ProviderId::Spotify;
    let Some(profile) = store.preferred_account(provider)? else {
        return Ok(None);
    };
    let Some(account) = store.account(provider, &profile.profile.id)? else {
        return Ok(None);
    };
    cached_data(&store, &account, &region(), CollectionKind::Albums)
}

fn refresh(
    store: &Store,
    account: &Account,
    region: &str,
    kind: CollectionKind,
    cancelled: &AtomicBool,
) -> Result<CollectionUpdate, StoreError> {
    let provider = account.provider;
    let Some(credentials) = store.credentials(account)? else {
        return Ok(CollectionUpdate::Error {
            provider,
            profile: account.profile.id.clone(),
            message: "Подтвердите вход заново.".into(),
            hide: false,
        });
    };
    let read_started = timestamp_ms();
    match collections::read(&credentials, &account.profile, kind, cancelled) {
        Ok(mut collection) if !cancelled.load(Ordering::Relaxed) => {
            let saved = if matches!(&collection, Collection::Liked(_)) {
                store.save_liked_snapshot(
                    account,
                    region,
                    kind.slot(),
                    read_started,
                    &mut collection,
                )?
            } else {
                store.save_music_snapshot(
                    provider,
                    Some(account),
                    region,
                    kind.slot(),
                    &collection,
                )?
            };
            if saved {
                return Ok(data(account, collection));
            }
            Ok(CollectionUpdate::Error {
                provider,
                profile: account.profile.id.clone(),
                message: "Подключение изменилось.".into(),
                hide: true,
            })
        }
        Ok(collection) => Ok(data(account, collection)),
        Err(error) => collection_failure(store, account, error, cancelled),
    }
}

fn collection_failure(
    store: &Store,
    account: &Account,
    error: ProviderError,
    cancelled: &AtomicBool,
) -> Result<CollectionUpdate, StoreError> {
    let hide = error.kind == ErrorKind::Revoked;
    if hide && !cancelled.load(Ordering::Relaxed) {
        store.forget_music_account(account)?;
    }
    Ok(CollectionUpdate::Error {
        provider: account.provider,
        profile: account.profile.id.clone(),
        message: error.to_string(),
        hide,
    })
}
