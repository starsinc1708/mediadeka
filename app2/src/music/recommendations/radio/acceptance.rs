use std::{collections::BTreeSet, env, fs, path::PathBuf};

use crate::{
    music::{
        library::PlaylistScope,
        recommendations::acceptance::{copy_database, copy_profile},
    },
    playlist::Playlist,
    providers::catalog,
};

use super::*;

const TRACKS: [&str; 2] = ["7GhIk7Il098yCjg4BQjzvb", "3AJwUDP919kvQ9QcozQPxg"];
const ARTIST: &str = "4gzpq5DPGxSnKTe4SA8HAU";

#[test]
#[ignore = "uses real Spotify snapshots and a copy of the connected Windows profile"]
fn contextual_radio_saved_tracks_artist_and_capture_profile() {
    let (copy, store) = copy_profile();
    let directory =
        PathBuf::from(env::var_os("MEDIADEKA_RADIO_SNAPSHOT_DIR").expect("radio snapshots"));
    let spotify = ProviderId::Spotify;
    let account = store
        .preferred_account(spotify)
        .unwrap()
        .expect("Spotify account");
    let credentials = store
        .credentials(&account)
        .unwrap()
        .expect("Spotify credentials");
    let profile = Some(account.profile.id.clone());
    let region = region();
    let cancelled = AtomicBool::new(false);
    let mut sets = Vec::new();
    for (id, kind) in TRACKS
        .into_iter()
        .map(|id| (id, ObjectKind::Track))
        .chain([(ARTIST, ObjectKind::Artist)])
    {
        let reference = ObjectRef {
            provider: spotify,
            kind,
            id: id.into(),
        };
        let slot = format!("{}:{id}", kind.path());
        let playlist: Playlist = serde_json::from_slice(
            &fs::read(directory.join(format!("spotify-{id}.json"))).unwrap(),
        )
        .unwrap();
        let page = match store
            .music_snapshot(spotify, Some(&account), &region, &slot)
            .unwrap()
        {
            Some(page) => page,
            None => {
                let page = catalog::read(&reference, Some(&credentials), &cancelled).unwrap();
                assert!(
                    store
                        .save_music_snapshot(spotify, Some(&account), &region, &slot, &page)
                        .unwrap()
                );
                page
            }
        };
        assert_eq!(page.reference, reference);
        let scope =
            PlaylistScope::for_source(&store, spotify, Some(account.clone()), &slot, &region)
                .unwrap();
        assert!(scope.save(&playlist).unwrap());
        let (seed, excluded) = match kind {
            ObjectKind::Track => {
                let entry = QueueEntry {
                    playlist: PlaylistKey::Source {
                        provider: spotify,
                        profile: profile.clone(),
                        id: id.into(),
                    },
                    playlist_revision: None,
                    index: 0,
                    track: page.tracks[0].clone(),
                    recommendations: Vec::new(),
                };
                (Seed::Track(Box::new(entry.clone())), vec![entry])
            }
            _ => (
                Seed::Artist(ScopedPage {
                    page: std::sync::Arc::new(page),
                    profile: profile.clone(),
                    receipt: None,
                }),
                Vec::new(),
            ),
        };
        let batch = start(
            &store,
            &seed,
            Mode::Familiar,
            wave::Access::Saved,
            &cancelled,
        )
        .unwrap();
        let entries = batch.entries(&excluded);
        assert_eq!(
            entries.len(),
            20,
            "{slot}, unavailable={:?}",
            batch.unavailable
        );
        let ids = entries
            .iter()
            .map(|entry| entry.track.id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), entries.len());
        assert!(!ids.contains(id));
        assert!(entries.iter().all(|entry| entry.recommendations.iter().any(
            |origin| origin.provider == spotify
                && origin.playlist == playlist.name
                && origin.playlist_id != slot
        )));
        println!(
            "{slot}: {} recommendations, original station {:?}, first {}",
            entries.len(),
            playlist.name,
            entries[0].track.id
        );
        sets.push(ids);
    }
    assert_ne!(sets[0], sets[1]);
    assert_ne!(sets[1], sets[2]);

    if let Some(destination) = env::var_os("MEDIADEKA_RADIO_CAPTURE_PROFILE") {
        let destination = PathBuf::from(destination);
        fs::create_dir_all(&destination).unwrap();
        copy_database(copy.path(), &destination);
        println!(
            "isolation and provider snapshots prepared at {}",
            destination.display()
        );
    }
}

// No catalog substitutions are allowed for records with similar-looking titles.
#[test]
fn radio_only_expands_to_confirmed_identities() {
    let spotify = ObjectRef {
        provider: ProviderId::Spotify,
        kind: ObjectKind::Track,
        id: TRACKS[0].into(),
    };
    let yandex = |id: &str| ObjectRef {
        provider: ProviderId::YandexMusic,
        kind: ObjectKind::Track,
        id: id.into(),
    };
    let confirmed = yandex("11:21");
    let transitive = yandex("11:51");
    let unrelated = yandex("12:21");
    let links = MatchLinks {
        native: vec![
            (spotify.clone(), confirmed.clone()),
            (transitive.clone(), confirmed),
        ],
        ..Default::default()
    };
    let matches = matched_objects(&links, [(Some(spotify), None)]);
    assert!(matches.contains(&transitive));
    assert!(!matches.contains(&unrelated));
    assert!(station_id(&transitive).is_some());
}
