use std::{
    collections::{BTreeMap, HashSet},
    sync::atomic::{AtomicBool, Ordering},
    thread,
};

use crate::{
    playback::{QueueEntry, RecommendationOrigin, can_resolve},
    playlist::{
        Playlist, PlaylistKey,
        library::{AccessError, ReadError},
    },
    providers::{Home, ProviderId, playlist_id, read_playlist},
    storage::{Account, Store, StoreError},
};

use super::{library::PlaylistScope, region};

#[cfg(all(test, windows))]
pub(crate) mod acceptance;
pub(crate) mod radio;
mod ranking;
#[cfg(test)]
mod tests;
pub(crate) mod wave;

pub(crate) use ranking::{Mode, Ranking, Reaction, set_reaction};

pub(crate) const PLAYLISTS_PER_PROVIDER: usize = 2;

#[derive(Clone)]
pub(crate) struct Selection {
    pub provider: ProviderId,
    pub profile: String,
    pub id: String,
    pub title: String,
}

pub(crate) fn personal_playlists<'a>(
    home: &'a Home,
    provider: ProviderId,
    profile: &'a str,
) -> impl Iterator<Item = Selection> + 'a {
    let mut seen = HashSet::new();
    home.items
        .iter()
        .enumerate()
        .filter_map(move |(index, item)| {
            if !home.personal_mix(provider, index) || item.title.trim().is_empty() {
                return None;
            }
            let id = playlist_id(provider, item)?;
            seen.insert(id.clone()).then(|| Selection {
                provider,
                profile: profile.to_owned(),
                id,
                title: item.title.clone(),
            })
        })
}

#[derive(Default)]
pub(crate) struct Mix {
    pub entries: Vec<QueueEntry>,
    pub unavailable: Vec<ProviderId>,
}

type ReadResult = Result<Playlist, ReadError>;

#[cfg(all(test, windows))]
pub(crate) fn assemble(
    store: &Store,
    selections: Vec<Selection>,
    cancelled: &AtomicBool,
) -> Result<Mix, StoreError> {
    let mut mix = assemble_candidates(store, selections, cancelled)?;
    mix.entries = Ranking::unranked(store)?.entries(&mix.entries, &[]);
    Ok(mix)
}

pub(crate) fn assemble_candidates(
    store: &Store,
    selections: Vec<Selection>,
    cancelled: &AtomicBool,
) -> Result<Mix, StoreError> {
    assemble_with(store, selections, cancelled, |account, selection| {
        let credentials = store
            .credentials(account)
            .map_err(ReadError::Store)?
            .ok_or(ReadError::Access(AccessError::MissingCredentials))?;
        read_playlist(
            selection.provider,
            Some(&credentials),
            &selection.id,
            cancelled,
        )
        .map_err(ReadError::Provider)
    })
}

#[cfg(all(test, windows))]
pub(crate) fn assemble_cached(
    store: &Store,
    selections: Vec<Selection>,
    cancelled: &AtomicBool,
) -> Result<Mix, StoreError> {
    let mut mix = assemble_cached_candidates(store, selections, cancelled)?;
    mix.entries = Ranking::unranked(store)?.entries(&mix.entries, &[]);
    Ok(mix)
}

#[cfg(any(test, feature = "capture"))]
pub(crate) fn assemble_cached_candidates(
    store: &Store,
    selections: Vec<Selection>,
    cancelled: &AtomicBool,
) -> Result<Mix, StoreError> {
    use crate::providers::{ErrorKind, ProviderError};

    assemble_with(store, selections, cancelled, |_, _| {
        Err(ReadError::Provider(ProviderError {
            kind: ErrorKind::Network,
            message: "Сеть недоступна.".into(),
        }))
    })
}

fn assemble_with(
    store: &Store,
    selections: Vec<Selection>,
    cancelled: &AtomicBool,
    read: impl Fn(&Account, &Selection) -> ReadResult + Sync,
) -> Result<Mix, StoreError> {
    let region = region();
    let mut sources = BTreeMap::<_, Vec<_>>::new();
    for selection in selections {
        sources
            .entry(selection.provider)
            .or_default()
            .push(selection);
    }
    let loaded = thread::scope(|scope| {
        let jobs = sources
            .into_iter()
            .map(|(provider, selections)| {
                let region = &region;
                let read = &read;
                scope.spawn(move || {
                    (
                        provider,
                        load_source(store, region, selections, cancelled, read),
                    )
                })
            })
            .collect::<Vec<_>>();
        jobs.into_iter()
            .map(|job| job.join().expect("recommendation provider worker"))
            .collect::<Vec<_>>()
    });
    let mut playlists = Vec::new();
    let mut unavailable = Vec::new();
    for (provider, loaded) in loaded {
        let loaded = loaded.unwrap_or_else(|error| {
            eprintln!("Микс Mediadeka · {}: {error}", provider.display_name());
            Vec::new()
        });
        if loaded.is_empty() {
            unavailable.push(provider);
        }
        playlists.extend(loaded.into_iter().enumerate());
    }
    // First playlist of each provider, then the second playlist of each provider.
    playlists.sort_by_key(|(rank, _)| *rank);
    Ok(Mix {
        entries: build_candidates(&playlists),
        unavailable,
    })
}

struct LoadedPlaylist {
    key: PlaylistKey,
    origin: RecommendationOrigin,
    data: Playlist,
}

fn load_source(
    store: &Store,
    region: &str,
    selections: Vec<Selection>,
    cancelled: &AtomicBool,
    read: &impl Fn(&Account, &Selection) -> ReadResult,
) -> Result<Vec<LoadedPlaylist>, StoreError> {
    let mut loaded = Vec::new();
    for selection in selections {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        let Some(account) = store.account(selection.provider, &selection.profile)? else {
            return Ok(Vec::new());
        };
        let scope = PlaylistScope::for_source(
            store,
            selection.provider,
            Some(account.clone()),
            &selection.id,
            region,
        )?;
        let result = read(&account, &selection);
        if cancelled.load(Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        let data = match result {
            Ok(data) => {
                if !scope.save(&data)? {
                    return Ok(Vec::new());
                }
                data
            }
            Err(error) if error.revokes_account() => return Ok(Vec::new()),
            Err(error) => {
                if !error.can_show_cached() {
                    continue;
                }
                let Some(data) = scope.cached()? else {
                    continue;
                };
                data
            }
        };
        let key = PlaylistKey::Source {
            provider: selection.provider,
            profile: Some(selection.profile),
            id: selection.id.clone(),
        };
        if data.tracks.iter().any(|track| can_resolve(&key, track)) {
            loaded.push(LoadedPlaylist {
                key,
                origin: RecommendationOrigin {
                    provider: selection.provider,
                    playlist: selection.title,
                    playlist_id: selection.id,
                },
                data,
            });
        }
    }
    Ok(loaded)
}

fn build_candidates(playlists: &[(usize, LoadedPlaylist)]) -> Vec<QueueEntry> {
    (0..playlists
        .iter()
        .map(|(_, list)| list.data.tracks.len())
        .max()
        .unwrap_or(0))
        .flat_map(|index| {
            playlists.iter().filter_map(move |(_, list)| {
                let track = list.data.tracks.get(index)?;
                can_resolve(&list.key, track).then(|| QueueEntry {
                    playlist: list.key.clone(),
                    playlist_revision: None,
                    index,
                    track: track.clone(),
                    recommendations: vec![list.origin.clone()],
                })
            })
        })
        .collect()
}
