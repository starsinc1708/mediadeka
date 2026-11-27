use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::{
    playlist::{Media, PlaylistKey, PlaylistTrack},
    providers::{Credentials, ProviderId, catalog, collections::Collection, likes},
    storage::{Account, Store},
};

use super::{
    MusicPage, ObjectKind, ObjectRef, page_cache::PageQuery, region, service::cached_links,
    spotify_access,
};

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, Hash, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct LikeIdentity {
    pub provider: Option<ProviderId>,
    pub profile: Option<String>,
    pub id: String,
}

impl LikeIdentity {
    pub(crate) fn from_track(key: &PlaylistKey, track: &PlaylistTrack) -> Option<Self> {
        if key.provider().is_none()
            && let Some(encoded) = track.id.strip_prefix("liked:")
        {
            return serde_json::from_str(encoded).ok();
        }
        let source = key.connected_source();
        let (provider, id) = match &track.media {
            Some(Media::Provider { provider, id }) => (Some(*provider), id.clone()),
            Some(Media::File(_)) => (None, track.id.clone()),
            _ => (source.map(|(provider, _)| provider), track.id.clone()),
        };
        let profile = source
            .filter(|(owner, _)| Some(*owner) == provider)
            .and_then(|(_, profile)| profile.map(str::to_owned));
        (!id.is_empty() && !id.starts_with("missing:")).then_some(Self {
            provider,
            profile,
            id,
        })
    }

    pub(super) fn reference(&self) -> Option<ObjectRef> {
        let reference = ObjectRef {
            provider: self.provider?,
            kind: ObjectKind::Track,
            id: self.id.clone(),
        };
        reference.is_valid().then_some(reference)
    }
}

#[derive(Clone)]
pub(crate) struct Subject {
    pub identity: LikeIdentity,
    pub track: PlaylistTrack,
}

impl Subject {
    pub(crate) fn new(key: &PlaylistKey, track: &PlaylistTrack) -> Option<Self> {
        Some(Self {
            identity: LikeIdentity::from_track(key, track)?,
            track: track.clone(),
        })
    }
}

#[derive(Clone)]
pub(crate) struct Match {
    reference: ObjectRef,
    pub track: PlaylistTrack,
}

#[derive(Clone)]
pub(crate) enum State {
    Local {
        liked: bool,
    },
    Ready {
        recording: Arc<Match>,
        liked: Option<bool>,
    },
    Unmatched,
    Error(String),
}

#[derive(Clone)]
pub(crate) struct Destination {
    pub provider: Option<ProviderId>,
    pub profile: Option<String>,
    pub state: State,
}

pub(crate) fn timestamp_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(crate) fn destinations(store: &Store, subject: &Subject) -> Result<Vec<Destination>, String> {
    let local = store.local_like(&subject.identity).map_err(message)?;
    let mut destinations = vec![Destination {
        provider: None,
        profile: None,
        state: State::Local { liked: local },
    }];
    let links = store.confirmed_track_pairs().map_err(message)?;
    let region = region();
    let known = match subject.identity.reference() {
        // Jellyfin IDs are meaningful only within the connected server.
        Some(reference) if reference.provider != ProviderId::Jellyfin => {
            cached_links(store, &region, &reference).map_err(message)?
        }
        _ => Vec::new(),
    };
    for summary in store.accounts().map_err(message)? {
        let provider = summary.provider;
        let profile = summary.profile.id;
        let state = (|| {
            let Some(account) = store.account(provider, &profile).map_err(message)? else {
                return Err("Подключение изменилось.".into());
            };
            let mut references = links.iter().filter_map(|(a, b)| {
                let other = if a == &subject.identity {
                    b
                } else if b == &subject.identity {
                    a
                } else {
                    return None;
                };
                (other.provider == Some(provider) && other.profile.as_deref() == Some(&profile))
                    .then(|| other.reference())
                    .flatten()
            });
            let native = subject.identity.reference().filter(|reference| {
                reference.provider == provider
                    && (provider != ProviderId::Jellyfin
                        || subject.identity.profile.as_deref() == Some(&profile))
            });
            let confirmed = || {
                known.iter().find_map(|link| {
                    if provider.as_str() != link.platform {
                        return None;
                    }
                    ObjectRef::from_url(&link.url).filter(|reference| {
                        reference.provider == provider && reference.kind == ObjectKind::Track
                    })
                })
            };
            let Some(reference) = native.or_else(|| references.next()).or_else(confirmed) else {
                return Ok(State::Unmatched);
            };
            let snapshot = liked_snapshot(store, &account, &region).map_err(message)?;
            let native_saved = snapshot
                .as_ref()
                .and_then(|saved| saved.iter().find(|track| track.id == reference.id).cloned());
            let liked = store
                .remote_like_override(&account, &reference.id)
                .map_err(message)?
                .or_else(|| snapshot.as_ref().map(|_| native_saved.is_some()));
            let recording = Arc::new(Match {
                track: native_saved.unwrap_or_else(|| {
                    let mut track = subject.track.clone();
                    track.id = reference.id.clone();
                    track.media = Some(Media::Provider {
                        provider,
                        id: reference.id.clone(),
                    });
                    track
                }),
                reference,
            });
            Ok(State::Ready { recording, liked })
        })()
        .unwrap_or_else(State::Error);
        destinations.push(Destination {
            provider: Some(provider),
            profile: Some(profile),
            state,
        });
    }
    Ok(destinations)
}

pub(crate) fn remote_liked_tracks(
    store: &Store,
    account: &Account,
    region: &str,
) -> Result<(Vec<PlaylistTrack>, bool), crate::storage::StoreError> {
    let snapshot = liked_snapshot(store, account, region)?;
    let has_snapshot = snapshot.is_some();
    let mut tracks = snapshot.unwrap_or_default();
    store.apply_liked_changes(account.provider, &account.profile.id, &mut tracks)?;
    Ok((tracks, has_snapshot))
}

fn liked_snapshot(
    store: &Store,
    account: &Account,
    region: &str,
) -> Result<Option<Vec<PlaylistTrack>>, crate::storage::StoreError> {
    Ok(
        match store.music_snapshot(account.provider, Some(account), region, "liked")? {
            Some(Collection::Liked(tracks)) => Some(tracks),
            _ => None,
        },
    )
}

pub(crate) fn candidates(
    store: &Store,
    subject: &Subject,
    provider: ProviderId,
    profile: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<Match>, String> {
    let (_, credentials) = account_access(store, provider, profile)?;
    let title = subject.track.title.trim();
    let query = format!("{title} {}", subject.track.artist);
    let initial = if provider == ProviderId::Jellyfin {
        title
    } else {
        &query
    };
    let search = |query: &str| {
        catalog::search(provider, &credentials, ObjectKind::Track, query, cancelled)
            .map(|pages| pages.into_iter().filter_map(candidate).collect::<Vec<_>>())
            .map_err(message)
    };
    let matches = search(initial)?;
    if matches.is_empty() && initial != title {
        return search(title);
    }
    Ok(matches)
}

pub(crate) fn check_remote(
    store: &Store,
    destination: &Destination,
    recording: &Match,
    cancelled: &AtomicBool,
) -> Result<bool, String> {
    let provider = destination.provider.ok_or("Не выбрана площадка.")?;
    let profile = destination.profile.as_deref().ok_or("Не выбран профиль.")?;
    let (account, credentials) = remote_account_access(store, provider, profile, cancelled)?;
    let recording = native_match(store, &account, recording, cancelled)?;
    likes::contains(
        &credentials,
        &account.profile,
        &recording.reference,
        cancelled,
    )
    .map_err(message)
}

fn candidate(mut page: MusicPage) -> Option<Match> {
    let provider = page.reference.provider;
    let track = page.tracks.pop()?;
    let id = match &track.media {
        Some(Media::Provider {
            provider: owner,
            id,
        }) if *owner == provider => id.clone(),
        _ => track.id.clone(),
    };
    let reference = ObjectRef {
        id,
        ..page.reference
    };
    (reference.is_valid() && reference.kind == ObjectKind::Track)
        .then_some(Match { reference, track })
}

pub(crate) fn set_local(
    store: &Store,
    subject: &Subject,
    liked: bool,
    cancelled: &AtomicBool,
) -> Result<super::library::LikeChange, String> {
    store
        .set_local_like(&subject.identity, &subject.track, liked, cancelled)
        .map_err(message)?;
    super::library::local_like_change(store, &subject.identity).map_err(message)
}

pub(crate) fn set_remote(
    store: &Store,
    subject: &Subject,
    destination: &Destination,
    recording: &Match,
    liked: bool,
    explicit_match: bool,
    cancelled: &AtomicBool,
) -> Result<super::library::LikeChange, String> {
    let provider = destination.provider.ok_or("Не выбрана площадка.")?;
    let profile = destination.profile.as_deref().ok_or("Не выбран профиль.")?;
    if recording.reference.provider != provider {
        return Err("Запись принадлежит другой площадке.".into());
    }
    let (account, credentials) = remote_account_access(store, provider, profile, cancelled)?;
    let recording = native_match(store, &account, recording, cancelled)?;
    if cancelled.load(Ordering::Relaxed) {
        return Err("Сохранение отменено.".into());
    }
    let target = LikeIdentity {
        provider: Some(provider),
        profile: Some(profile.to_owned()),
        id: recording.reference.id.clone(),
    };
    if explicit_match {
        store
            .remember_track_confirmation(&subject.identity, &target, &account)
            .map_err(message)?;
    }
    likes::set(
        &credentials,
        &account.profile,
        &recording.reference,
        liked,
        cancelled,
    )
    .map_err(message)?;
    if !explicit_match && target != subject.identity {
        store
            .remember_track_confirmation(&subject.identity, &target, &account)
            .map_err(message)?;
    }
    let track = store
        .record_remote_like(&account, &recording.track, liked)
        .map_err(|error| {
            format!("Площадка сохранила изменение, но снимок не обновился: {error}")
        })?;
    Ok(super::library::remote_like_change(
        target,
        &subject.identity,
        provider,
        profile,
        track,
        liked,
    ))
}

fn native_match(
    store: &Store,
    account: &Account,
    recording: &Match,
    cancelled: &AtomicBool,
) -> Result<Match, String> {
    if recording.reference.provider == ProviderId::SoundCloud
        && !recording.reference.id.bytes().all(|b| b.is_ascii_digit())
    {
        let page = PageQuery::for_account(
            store,
            &recording.reference,
            Some(account.clone()),
            &region(),
        )
        .refresh(cancelled)
        .map_err(message)?;
        return candidate((*page).clone()).ok_or("SoundCloud не вернул запись.".into());
    }
    Ok(recording.clone())
}

fn account_access(
    store: &Store,
    provider: ProviderId,
    id: &str,
) -> Result<(Account, Credentials), String> {
    let account = store
        .account(provider, id)
        .map_err(message)?
        .ok_or("Подключение изменилось.")?;
    let credentials = store
        .credentials(&account)
        .map_err(message)?
        .ok_or("Подтвердите вход заново.")?;
    Ok((account, credentials))
}

fn remote_account_access(
    store: &Store,
    provider: ProviderId,
    profile: &str,
    cancelled: &AtomicBool,
) -> Result<(Account, Credentials), String> {
    if provider == ProviderId::Spotify {
        spotify_access::prepare(store, profile, cancelled).map_err(message)
    } else {
        account_access(store, provider, profile)
    }
}

fn message(error: impl std::fmt::Display) -> String {
    error.to_string()
}
