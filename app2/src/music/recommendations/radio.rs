use std::{collections::HashSet, sync::atomic::AtomicBool};

use crate::{
    music::{
        ObjectKind, ObjectRef, ScopedPage,
        likes::LikeIdentity,
        matching::MatchLinks,
        region,
        station::recommendations::{ARTIST_RADIO_PREFIX, StationSource, TRACK_RADIO_PREFIX},
    },
    playback::QueueEntry,
    playlist::PlaylistKey,
    providers::{self, ProviderId},
    storage::{Store, StoreError},
};

use super::{Mode, wave};

#[cfg(all(test, windows))]
mod acceptance;

pub(crate) enum Seed {
    Track(Box<QueueEntry>),
    Artist(ScopedPage),
}

pub(crate) fn has_track_radio(provider: ProviderId) -> bool {
    matches!(
        provider,
        ProviderId::YandexMusic | ProviderId::SoundCloud | ProviderId::Spotify
    )
}

pub(crate) fn start(
    store: &Store,
    seed: &Seed,
    mode: Mode,
    access: wave::Access,
    cancelled: &AtomicBool,
) -> Result<wave::Batch, StoreError> {
    let sources = sources(store, seed)?;
    wave::start(store, Vec::new(), sources, mode, access, cancelled)
}

fn sources(store: &Store, seed: &Seed) -> Result<Vec<StationSource>, StoreError> {
    let region = region();
    let mut sources = Vec::new();
    match seed {
        Seed::Track(entry) => {
            sources.extend(track_sources(
                store,
                &region,
                &entry.playlist,
                &entry.track,
            )?);
        }
        Seed::Artist(data) => {
            let page = &data.page;
            let artist = &page.reference;
            let mut native = vec![artist.clone()];
            native.extend(matched_objects(
                &MatchLinks::load(store, &region, ObjectKind::Artist)?,
                [(Some(artist.clone()), None)],
            ));
            sources.extend(for_accounts(
                store,
                native,
                data.profile.as_deref(),
                &page.title,
            )?);
            let key = PlaylistKey::Source {
                provider: artist.provider,
                profile: data.profile.clone(),
                id: artist.id.clone(),
            };
            // The artist's own catalog supplies the representative recording where
            // a provider only supports track radio. Never pick a search result by name.
            if let Some(track) = page
                .tracks
                .iter()
                .find(|track| crate::playback::can_resolve(&key, track))
            {
                let mut providers: HashSet<_> = sources
                    .iter()
                    .filter_map(|source| source.key.provider())
                    .collect();
                sources.extend(
                    track_sources(store, &region, &key, track)?
                        .into_iter()
                        .filter(|source| {
                            source
                                .key
                                .provider()
                                .is_some_and(|provider| providers.insert(provider))
                        }),
                );
            }
        }
    }
    Ok(sources)
}

fn track_sources(
    store: &Store,
    region: &str,
    key: &PlaylistKey,
    track: &crate::playlist::PlaylistTrack,
) -> Result<Vec<StationSource>, StoreError> {
    let Some(identity) = LikeIdentity::from_track(key, track) else {
        return Ok(Vec::new());
    };
    let reference = identity.reference();
    let catalog_reference = key
        .provider()
        .filter(|provider| Some(*provider) == identity.provider)
        .map(|provider| ObjectRef {
            provider,
            kind: ObjectKind::Track,
            id: track.id.clone(),
        })
        .filter(ObjectRef::is_valid);
    // A provider can relink audio for the listener's market. Radio starts from
    // the catalog row the listener chose, with the playback identity for matches.
    let mut native: Vec<_> = catalog_reference
        .iter()
        .chain(&reference)
        .cloned()
        .collect();
    native.extend(matched_objects(
        &MatchLinks::load(store, region, ObjectKind::Track)?,
        [
            (catalog_reference, None),
            (reference, Some(identity.clone())),
        ],
    ));
    for_accounts(store, native, identity.profile.as_deref(), &track.title)
}

fn matched_objects<const N: usize>(
    links: &MatchLinks,
    anchors: [(Option<ObjectRef>, Option<LikeIdentity>); N],
) -> Vec<ObjectRef> {
    let mut candidates = links
        .native
        .iter()
        .flat_map(|(a, b)| [a, b])
        .cloned()
        .map(|object| (Some(object), None))
        .chain(
            links
                .choices
                .iter()
                .flat_map(|(a, b)| [a, b])
                .map(|identity| (identity.reference(), Some(identity.clone()))),
        )
        .collect::<Vec<_>>();
    // Prefer SoundCloud catalog IDs to the permalink aliases of those same objects.
    candidates.sort_by_key(|(object, _)| {
        object.as_ref().is_some_and(|object| {
            object.provider == ProviderId::SoundCloud && object.id.parse::<u64>().is_err()
        })
    });
    let groups = links.groups(anchors.into_iter().chain(candidates.iter().cloned()));
    let (anchors, matches) = groups.split_at(N);
    candidates
        .into_iter()
        .zip(matches)
        .filter_map(|((object, _), group)| anchors.contains(group).then_some(object).flatten())
        .collect()
}

fn for_accounts(
    store: &Store,
    objects: Vec<ObjectRef>,
    profile: Option<&str>,
    title: &str,
) -> Result<Vec<StationSource>, StoreError> {
    let origin = objects.first().map(|object| object.provider);
    let mut selected = HashSet::new();
    let mut sources = Vec::new();
    for object in objects {
        let provider = object.provider;
        let Some(id) = station_id(&object) else {
            continue;
        };
        if !selected.insert(provider) {
            continue;
        }
        let account = match profile.filter(|_| origin == Some(provider)) {
            Some(profile) => store.account(provider, profile)?,
            None => store.preferred_account(provider)?,
        };
        if let Some(account) = account {
            sources.push(StationSource {
                key: PlaylistKey::Station {
                    provider,
                    profile: Some(account.profile.id),
                    id,
                },
                title: format!("Радио · {title}"),
            });
        }
    }
    Ok(sources)
}

fn station_id(object: &ObjectRef) -> Option<String> {
    use ObjectKind::{Artist, Track};
    use ProviderId::{SoundCloud, Spotify, YandexMusic};
    if !object.is_valid() {
        return None;
    }
    match (object.provider, object.kind) {
        (YandexMusic, Track | Artist) => {
            let id = format!("{}:{}", object.kind.path(), object.id);
            providers::yandex::station::valid_seed(&id).then_some(id)
        }
        (Spotify | SoundCloud, Track) => Some(format!("{TRACK_RADIO_PREFIX}{}", object.id)),
        (Spotify, Artist) => Some(format!("{ARTIST_RADIO_PREFIX}{}", object.id)),
        (SoundCloud, Artist) => {
            let id = format!("soundcloud:system-playlists:artist-stations:{}", object.id);
            providers::soundcloud_station_id(&id).then_some(id)
        }
        _ => None,
    }
}
