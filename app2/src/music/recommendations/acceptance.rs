use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use rusqlite::{Connection, OpenFlags, params};
use serde::{Deserialize, Serialize};

use crate::{
    music::{library::PlaylistScope, likes::LikeIdentity, region},
    playback::can_resolve,
    playlist::{Playlist, PlaylistKey},
    providers::ProviderId,
    storage::{CacheScope, Store},
};

use super::{
    Mix, Mode, PLAYLISTS_PER_PROVIDER, Ranking, Reaction, Selection, assemble, assemble_cached,
    assemble_cached_candidates, personal_playlists, set_reaction,
};

const PROFILE: &str = "MEDIADEKA_MIX_SOURCE_PROFILE";
const SNAPSHOTS: &str = "MEDIADEKA_MIX_SNAPSHOT_FILE";
const CAPTURE_PROFILE: &str = "MEDIADEKA_MIX_CAPTURE_PROFILE";

#[derive(Serialize, Deserialize)]
struct RecordedPlaylist {
    provider: ProviderId,
    id: String,
    title: String,
    playlist: Playlist,
}

#[test]
#[ignore = "reads personal playlists using connected profiles without refreshing their access"]
fn record_live_recommendation_snapshots() {
    let (_copy, store) = copy_profile();
    let mut input = Vec::new();
    for provider in ProviderId::ALL {
        let Some(account) = store.preferred_account(provider).unwrap() else {
            continue;
        };
        let Some(snapshot) = store
            .cache(CacheScope::Account(provider, &account.profile.id))
            .unwrap()
        else {
            continue;
        };
        let playlists: Vec<_> = personal_playlists(&snapshot.home, provider, &account.profile.id)
            .take(PLAYLISTS_PER_PROVIDER)
            .collect();
        println!(
            "{}: {} candidate playlists",
            provider.as_str(),
            playlists.len()
        );
        input.extend(playlists);
    }

    // Existing slots on the copy must not mask a failed request.
    let region = region();
    clear_snapshots(&store, &region, input.iter());
    let mix = assemble(&store, input.clone(), &AtomicBool::new(false)).unwrap();
    let snapshots: Vec<_> = input
        .iter()
        .filter_map(|selection| record_snapshot(&store, selection, &region))
        .collect();
    let path = snapshot_path();
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(&snapshots).unwrap()).unwrap();
    let sources: BTreeSet<_> = snapshots.iter().map(|snapshot| snapshot.provider).collect();
    assert!(
        sources.len() >= 2,
        "at least two providers with saved personal playlists"
    );
    assert_eq!(report("live", &mix, &input), sources);
}

fn record_snapshot(store: &Store, selection: &Selection, region: &str) -> Option<RecordedPlaylist> {
    let provider = selection.provider;
    let account = store.account(provider, &selection.profile).unwrap()?;
    let scope =
        PlaylistScope::for_source(store, provider, Some(account), &selection.id, region).unwrap();
    let mut playlist = scope.cached().unwrap()?;
    let key = PlaylistKey::Source {
        provider,
        profile: Some(selection.profile.clone()),
        id: selection.id.clone(),
    };
    let playable = playlist
        .tracks
        .iter()
        .filter(|track| can_resolve(&key, track))
        .count();
    println!(
        "{} playlist: {} rows, {playable} playable",
        provider.as_str(),
        playlist.tracks.len(),
    );
    playlist.owner = None;
    playlist.link = None;
    (playable > 0).then(|| RecordedPlaylist {
        provider,
        id: selection.id.clone(),
        title: selection.title.clone(),
        playlist,
    })
}

#[test]
#[ignore = "replays previously recorded personal playlists from real providers with no service calls"]
fn recommendation_mix_from_saved_provider_snapshots() {
    let (copy, store) = copy_profile();
    let input = seed_saved_snapshots(&store);
    let region = region();
    let sources: BTreeSet<_> = input.iter().map(|selection| selection.provider).collect();
    assert!(sources.len() >= 2);
    let cancelled = AtomicBool::new(false);
    let complete = assemble_cached(&store, input.clone(), &cancelled).unwrap();
    assert_eq!(report("complete", &complete, &input), sources);
    assert!(complete.unavailable.is_empty());

    if let Some(directory) = std::env::var_os(CAPTURE_PROFILE) {
        let directory = PathBuf::from(directory);
        fs::create_dir_all(&directory).unwrap();
        copy_database(copy.path(), &directory);
    }

    let removed = *sources.first().unwrap();
    let unavailable = input
        .iter()
        .filter(|selection| selection.provider == removed);
    assert_eq!(
        clear_snapshots(&store, &region, unavailable.clone()),
        unavailable.count()
    );
    let partial = assemble_cached(&store, input.clone(), &cancelled).unwrap();
    assert_eq!(partial.unavailable, [removed]);
    assert_eq!(
        report("partial", &partial, &input),
        sources
            .into_iter()
            .filter(|provider| *provider != removed)
            .collect()
    );
}

pub(crate) fn seed_saved_snapshots(store: &Store) -> Vec<Selection> {
    let snapshots: Vec<RecordedPlaylist> =
        serde_json::from_slice(&fs::read(snapshot_path()).unwrap()).unwrap();
    let mut input = Vec::new();
    let region = region();
    for snapshot in snapshots {
        let provider = snapshot.provider;
        let account = store.preferred_account(provider).unwrap().unwrap();
        let selection = Selection {
            provider,
            profile: account.profile.id.clone(),
            id: snapshot.id,
            title: snapshot.title,
        };
        assert!(
            store
                .save_music_snapshot(
                    provider,
                    Some(&account),
                    &region,
                    &format!("playlist:{}", selection.id),
                    &snapshot.playlist,
                )
                .unwrap()
        );
        input.push(selection);
    }
    input
}

#[test]
#[ignore = "compares modes on recorded provider playlists and listening history from an isolated profile copy"]
fn recommendation_modes_from_saved_provider_snapshots() {
    let (copy, store) = copy_profile();
    let input = seed_saved_snapshots(&store);
    let mix = assemble_cached_candidates(&store, input.clone(), &AtomicBool::new(false)).unwrap();
    assert!(mix.unavailable.is_empty());
    let candidates = mix.entries;
    let ranked = |store: &Store, mode| {
        Ranking::load(store, mode)
            .unwrap()
            .entries(&candidates, &[])
    };
    let familiar = ranked(&store, Mode::Familiar);
    let discoveries = ranked(&store, Mode::Discoveries);
    let history = store
        .listening_history()
        .unwrap()
        .iter()
        .filter_map(|row| LikeIdentity::from_track(&row.key, &row.track))
        .collect::<std::collections::HashSet<_>>();
    let previously_played = |entries: &[crate::playback::QueueEntry]| {
        entries
            .iter()
            .filter_map(|entry| LikeIdentity::from_track(&entry.playlist, &entry.track))
            .filter(|identity| history.contains(identity))
            .count()
    };
    let played_familiar = previously_played(&familiar);
    let played_discoveries = previously_played(&discoveries);
    assert!(played_familiar > played_discoveries);
    println!(
        "same candidates: {}; exact identities from history in Familiar: {played_familiar}; in Discoveries: {played_discoveries}",
        candidates.len()
    );
    let hidden = discoveries.first().unwrap().clone();
    let promoted = discoveries.last().unwrap().clone();
    for (label, entries) in [("Familiar", familiar), ("Discoveries", discoveries)] {
        report(
            label,
            &Mix {
                entries,
                unavailable: Vec::new(),
            },
            &input,
        );
    }

    set_reaction(&store, &hidden, Reaction::Hide).unwrap();
    set_reaction(&store, &promoted, Reaction::MoreLikeThis).unwrap();
    drop(store);
    let reopened = Store::open(copy.path()).unwrap();
    let discoveries = ranked(&reopened, Mode::Discoveries);
    assert!(discoveries.first() == Some(&promoted));
    assert!(!discoveries.contains(&hidden));
    assert!(!ranked(&reopened, Mode::Familiar).contains(&hidden));
    report(
        "Discoveries after reopening",
        &Mix {
            entries: discoveries,
            unavailable: Vec::new(),
        },
        &input,
    );
    println!(
        "More like this moved a recording from #20 to #1; Hide removed a recording in both modes after reopening"
    );
}

pub(crate) fn clear_snapshots<'a>(
    store: &Store,
    region: &str,
    selections: impl Iterator<Item = &'a Selection>,
) -> usize {
    let connection = Connection::open(store.directory().join("mediadeka.sqlite3")).unwrap();
    selections
        .map(|selection| connection.execute(
            "DELETE FROM music_snapshot WHERE provider = ?1 AND profile_id = ?2 AND region = ?3 AND slot = ?4",
            params![selection.provider.as_str(), selection.profile, region, format!("playlist:{}", selection.id)],
        ).unwrap())
        .sum()
}

fn report(label: &str, mix: &Mix, input: &[Selection]) -> BTreeSet<ProviderId> {
    assert_eq!(mix.entries.len(), 20);
    let mut contributions = BTreeMap::new();
    let mut native = BTreeSet::new();
    let mut shared = 0;
    for entry in &mix.entries {
        assert!(entry.is_playable());
        assert!(!entry.recommendations.is_empty());
        let PlaylistKey::Source { provider, .. } = &entry.playlist else {
            panic!("the mix keeps the ordinary source playlist key");
        };
        assert!(native.insert((*provider, &entry.track.id)));
        let mut providers = BTreeSet::new();
        for origin in &entry.recommendations {
            assert!(!origin.playlist.trim().is_empty());
            assert!(input.iter().any(|selection| {
                selection.provider == origin.provider
                    && selection.id == origin.playlist_id
                    && selection.title == origin.playlist
            }));
            providers.insert(origin.provider);
            *contributions.entry(origin.provider).or_insert(0) += 1;
        }
        shared += usize::from(providers.len() > 1);
    }
    println!(
        "{label}: rows={} native_identities={} rows_with_provenance={} cross_provider_rows={} contributions={:?} unavailable={:?}",
        mix.entries.len(),
        native.len(),
        mix.entries.len(),
        shared,
        contributions,
        mix.unavailable,
    );
    contributions.into_keys().collect()
}

pub(crate) fn copy_profile() -> (tempfile::TempDir, Store) {
    let source = PathBuf::from(std::env::var_os(PROFILE).expect("saved profile directory"));
    let copy = tempfile::tempdir().unwrap();
    copy_database(&source, copy.path());
    let store = Store::open(copy.path()).unwrap();
    (copy, store)
}

pub(crate) fn copy_database(source: &Path, destination: &Path) {
    let destination = destination.join("mediadeka.sqlite3");
    let original = Connection::open_with_flags(
        source.join("mediadeka.sqlite3"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    original
        .execute("VACUUM INTO ?1", [destination.to_str().unwrap()])
        .unwrap();
}

fn snapshot_path() -> PathBuf {
    PathBuf::from(std::env::var_os(SNAPSHOTS).expect("path to the real playlist snapshots"))
}
