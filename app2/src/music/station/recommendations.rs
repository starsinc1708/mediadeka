use std::{
    collections::{BTreeMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crate::{
    music::{ObjectKind, ObjectRef, library::PlaylistScope, region},
    playback::{QueueEntry, RecommendationOrigin, can_resolve},
    playlist::{
        Playlist, PlaylistKey, PlaylistTrack,
        library::{AccessError, ReadError},
    },
    providers::{
        self, ErrorKind, Home, ProviderError, ProviderId, yandex::station::MAX_RECENT_TRACKS,
    },
    storage::{Account, Store},
};

use super::{MY_WAVE, refill_provider, start_provider};

#[cfg(test)]
mod tests;

const STATIONS_PER_PROVIDER: usize = 2;
pub(crate) const TRACK_RADIO_PREFIX: &str = "track:";
pub(crate) const ARTIST_RADIO_PREFIX: &str = "artist:";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StationSource {
    pub key: PlaylistKey,
    pub title: String,
}

#[derive(Clone, Default)]
pub(crate) struct StationSessions(Vec<SourceSession>);

#[derive(Clone)]
struct SourceSession {
    key: PlaylistKey,
    account: Account,
    feed: StationFeed,
}

#[derive(Clone)]
enum StationFeed {
    Rotor {
        session: String,
        recent: Vec<String>,
    },
    Playlist(Arc<[QueueEntry]>),
}

pub(crate) struct StationCandidates {
    pub entries: Vec<QueueEntry>,
    pub unavailable: Vec<ProviderId>,
    pub sessions: StationSessions,
}

pub(crate) fn selections(
    home: Option<&Home>,
    provider: ProviderId,
    profile: &str,
) -> Vec<StationSource> {
    let source = |id: String, title: String| StationSource {
        key: PlaylistKey::Station {
            provider,
            profile: Some(profile.to_owned()),
            id,
        },
        title,
    };
    let mut stations = Vec::new();
    let mut seen = HashSet::new();
    if provider == ProviderId::YandexMusic {
        seen.insert(MY_WAVE.to_owned());
        stations.push(source(MY_WAVE.into(), "Моя волна".into()));
    }
    let available = home
        .into_iter()
        .flat_map(|home| &home.items)
        .filter_map(|item| {
            let object = ObjectRef::from_item(provider, item)?;
            (object.kind == ObjectKind::Station
                && !item.title.trim().is_empty()
                && seen.insert(object.id.clone()))
            .then(|| source(object.id, item.title.clone()))
        });
    stations.extend(available.take(STATIONS_PER_PROVIDER - stations.len()));
    stations
}

pub(crate) fn load(
    store: &Store,
    sources: &[StationSource],
    sessions: StationSessions,
    cancelled: &AtomicBool,
) -> StationCandidates {
    load_with(store, sources, sessions, cancelled, true)
}

#[cfg(any(test, feature = "capture"))]
pub(crate) fn load_cached(
    store: &Store,
    sources: &[StationSource],
    sessions: StationSessions,
    cancelled: &AtomicBool,
) -> StationCandidates {
    load_with(store, sources, sessions, cancelled, false)
}

fn load_with(
    store: &Store,
    sources: &[StationSource],
    previous: StationSessions,
    cancelled: &AtomicBool,
    live: bool,
) -> StationCandidates {
    let mut providers = BTreeMap::<_, Vec<_>>::new();
    for source in sources {
        if let Some(provider) = source.key.provider() {
            providers.entry(provider).or_default().push(source);
        }
    }
    let region = region();
    let mut result = StationCandidates {
        entries: Vec::new(),
        unavailable: Vec::new(),
        sessions: StationSessions::default(),
    };
    thread::scope(|scope| {
        let jobs = providers
            .into_iter()
            .map(|(provider, sources)| {
                let region = &region;
                let previous = &previous.0;
                let job = scope.spawn(move || {
                    let mut entries = Vec::new();
                    let mut sessions = Vec::new();
                    for source in sources {
                        if cancelled.load(Ordering::Relaxed) {
                            break;
                        }
                        let prior = previous.iter().find(|session| session.key == source.key);
                        match load_source(store, region, source, prior, cancelled, live) {
                            Ok(Some((tracks, session))) => {
                                entries.extend(tracks);
                                sessions.push(session);
                            }
                            Ok(None) => {}
                            Err(error) if !cancelled.load(Ordering::Relaxed) => {
                                eprintln!(
                                    "Рекомендации · {} · {}: {error}",
                                    provider.display_name(),
                                    source.title
                                );
                                if let Some(prior) = prior.filter(|_| transient(&error)) {
                                    sessions.push(prior.clone());
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    (entries, sessions)
                });
                (provider, job)
            })
            .collect::<Vec<_>>();
        for (provider, job) in jobs {
            let (entries, sessions) = job.join().expect("station provider worker");
            if entries.is_empty() {
                result.unavailable.push(provider);
            }
            result.entries.extend(entries);
            result.sessions.0.extend(sessions);
        }
    });
    result
}

fn load_source(
    store: &Store,
    region: &str,
    source: &StationSource,
    previous: Option<&SourceSession>,
    cancelled: &AtomicBool,
    live: bool,
) -> Result<Option<(Vec<QueueEntry>, SourceSession)>, ReadError> {
    let PlaylistKey::Station {
        provider,
        profile: Some(profile),
        id,
    } = &source.key
    else {
        return Ok(None);
    };
    let Some(account) = store
        .account(*provider, profile)
        .map_err(ReadError::Store)?
    else {
        return Ok(None);
    };
    if store.account_revoked(&account).map_err(ReadError::Store)? {
        return Ok(None);
    }
    let previous = previous.filter(|previous| previous.account == account);
    let (entries, feed) = match provider {
        ProviderId::YandexMusic if live => {
            let credentials = store.credentials(&account).map_err(ReadError::Store)?;
            let (feed, mut recent) = match previous.map(|previous| &previous.feed) {
                Some(StationFeed::Rotor { session, recent }) => (
                    refill_provider(credentials.as_ref(), session, recent, cancelled),
                    recent.clone(),
                ),
                _ => (
                    start_provider(*provider, id, credentials.as_ref(), cancelled),
                    Vec::new(),
                ),
            };
            let feed = feed.map_err(ReadError::Provider)?;
            let Some(rotor) = feed.rotor else {
                return Ok(None);
            };
            recent.extend(feed.tracks.iter().map(|track| track.id.clone()));
            recent.drain(..recent.len().saturating_sub(MAX_RECENT_TRACKS));
            (
                queue_entries(source, *provider, id, &source.title, feed.tracks),
                StationFeed::Rotor {
                    session: rotor.session_id,
                    recent,
                },
            )
        }
        ProviderId::SoundCloud | ProviderId::Spotify => {
            let entries = match previous.map(|previous| &previous.feed) {
                Some(StationFeed::Playlist(entries)) => entries.clone(),
                _ => {
                    let scope = PlaylistScope::for_source(
                        store,
                        *provider,
                        Some(account.clone()),
                        id,
                        region,
                    )
                    .map_err(ReadError::Store)?;
                    let Some(playlist) =
                        station_playlist(store, &account, &scope, id, cancelled, live)?
                    else {
                        return Ok(None);
                    };
                    let title = if !playlist.name.trim().is_empty() {
                        &playlist.name
                    } else {
                        &source.title
                    };
                    let origin_id = if id.starts_with(TRACK_RADIO_PREFIX)
                        || id.starts_with(ARTIST_RADIO_PREFIX)
                    {
                        playlist
                            .link
                            .as_deref()
                            .and_then(ObjectRef::from_url)
                            .filter(|object| {
                                object.provider == *provider
                                    && matches!(
                                        object.kind,
                                        ObjectKind::Playlist | ObjectKind::Station
                                    )
                            })
                            .map(|object| object.id)
                    } else {
                        None
                    };
                    Arc::from(queue_entries(
                        source,
                        *provider,
                        origin_id.as_deref().unwrap_or(id),
                        title,
                        playlist.tracks,
                    ))
                }
            };
            (entries.to_vec(), StationFeed::Playlist(entries))
        }
        _ => return Ok(None),
    };
    if cancelled.load(Ordering::Relaxed)
        || store
            .account(*provider, profile)
            .map_err(ReadError::Store)?
            .as_ref()
            != Some(&account)
        || store.account_revoked(&account).map_err(ReadError::Store)?
    {
        return Ok(None);
    }
    Ok(Some((
        entries,
        SourceSession {
            key: source.key.clone(),
            account,
            feed,
        },
    )))
}

fn station_playlist(
    store: &Store,
    account: &Account,
    scope: &PlaylistScope,
    id: &str,
    cancelled: &AtomicBool,
    live: bool,
) -> Result<Option<Playlist>, ReadError> {
    let issue = if live {
        let result = store
            .credentials(account)
            .map_err(ReadError::Store)?
            .ok_or(ReadError::Access(AccessError::MissingCredentials))
            .and_then(|credentials| {
                let result = match (account.provider, id.split_once(':')) {
                    (ProviderId::SoundCloud, Some(("track", track))) => {
                        providers::soundcloud_track_radio(&credentials, track, cancelled)
                    }
                    (ProviderId::Spotify, Some(("track", track))) => {
                        providers::spotify_track_radio(&credentials, track, cancelled)
                    }
                    (ProviderId::Spotify, Some(("artist", artist))) => {
                        providers::spotify_artist_radio(&credentials, artist, cancelled)
                    }
                    (provider, _) => {
                        providers::read_playlist(provider, Some(&credentials), id, cancelled)
                    }
                };
                result.map_err(ReadError::Provider)
            });
        match result {
            Ok(playlist) if !cancelled.load(Ordering::Relaxed) => {
                return Ok(scope
                    .save(&playlist)
                    .map_err(ReadError::Store)?
                    .then_some(playlist));
            }
            Ok(_) => return Ok(None),
            Err(error) if !error.can_show_cached() => return Err(error),
            Err(error) => Some(error),
        }
    } else {
        None
    };
    match (scope.cached().map_err(ReadError::Store)?, issue) {
        (Some(playlist), _) => Ok(Some(playlist)),
        (None, Some(error)) => Err(error),
        (None, None) => Ok(None),
    }
}

fn transient(error: &ReadError) -> bool {
    matches!(
        error,
        ReadError::Provider(ProviderError {
            kind: ErrorKind::Network,
            ..
        })
    )
}

fn queue_entries(
    source: &StationSource,
    provider: ProviderId,
    id: &str,
    title: &str,
    tracks: Vec<PlaylistTrack>,
) -> Vec<QueueEntry> {
    let origin = RecommendationOrigin {
        provider,
        playlist: title.to_owned(),
        playlist_id: id.to_owned(),
    };
    tracks
        .into_iter()
        .enumerate()
        .filter(|(_, track)| can_resolve(&source.key, track))
        .map(|(index, track)| QueueEntry {
            playlist: source.key.clone(),
            playlist_revision: None,
            index,
            track,
            recommendations: vec![origin.clone()],
        })
        .collect()
}
