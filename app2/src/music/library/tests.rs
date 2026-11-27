use std::sync::Mutex;

use crate::{
    music::ObjectRef,
    playlist::{Media, PlaylistTrack},
    providers::{LoginInput, sign_in},
};

use super::*;

#[test]
fn confirmed_recordings_share_one_row_and_keep_the_earliest_known_date() {
    let spotify = ProviderId::Spotify;
    let yandex = ProviderId::YandexMusic;
    let soundcloud = ProviderId::SoundCloud;
    let spotify_id = "12AbCdEfgHiJkL0mNOpQRs";
    let spotify_rows = source(spotify, &[("Spotify Name", spotify_id, "2026-11-22")]);
    let yandex_rows = source(
        yandex,
        &[("Yandex Name", "100:200", "2026-10-15T09:00:00+04:00")],
    );
    let soundcloud_rows = source(soundcloud, &[("Spotify Name", "123", "2026-11-01")]);
    let source_ref = ObjectRef {
        provider: spotify,
        kind: ObjectKind::Track,
        id: spotify_id.into(),
    };
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let region = region();
    assert!(
        store
            .claim_music_match(&region, &source_ref, 100, 150)
            .unwrap()
    );
    store
        .save_music_match(
            &region,
            &[source_ref],
            150,
            999,
            &serde_json::json!({
                "links": [{
                    "platform": "yandex-music",
                    "id": "100:201",
                    "url": "https://music.yandex.ru/album/201/track/100",
                    "evidence": "Название, исполнители, версия и длительность",
                    "checked_at": 100
                }]
            }),
        )
        .unwrap();
    drop(store);
    let reopened = Store::open(directory.path()).unwrap();
    let links = MatchLinks::load(&reopened, &region, ObjectKind::Track).unwrap();
    let index = LibraryIndex::build([spotify_rows, yandex_rows, soundcloud_rows], &links);
    let summary = index.project(None, None, "", false, LibrarySort::Recent);
    assert_eq!(summary.visible.len(), 2);
    assert_eq!(summary.visible_sources, 3);
    assert_eq!(summary.counts[&Some(spotify)], 1);
    assert_eq!(summary.counts[&Some(yandex)], 1);
    assert_eq!(summary.counts[&Some(soundcloud)], 1);
    let merged = &index.rows[summary.visible[1]];
    assert_eq!(merged.added, summary.first_added);
    assert_eq!(merged.scopes().count(), 2);
    assert_eq!(merged.added, Some(1792040400));

    let source = Scope::Provider {
        provider: yandex,
        profile: "sample".into(),
    };
    let filtered = index.project(
        Some(&source),
        None,
        "Spotify Name",
        false,
        LibrarySort::Title,
    );
    assert_eq!(filtered.visible.len(), 1);
    assert_eq!(
        index.rows[filtered.visible[0]].entry(Some(&source)).title,
        "Yandex Name"
    );
    assert_eq!(filtered.unfiltered, 2);
    let other_profile = Scope::Provider {
        provider: yandex,
        profile: "other".into(),
    };
    assert!(
        index
            .project(Some(&other_profile), None, "", false, LibrarySort::Title)
            .visible
            .is_empty()
    );
}

#[test]
fn confirmed_public_permalink_joins_its_native_catalog_recording() {
    let spotify = ProviderId::Spotify;
    let soundcloud = ProviderId::SoundCloud;
    let spotify_id = "12AbCdEfgHiJkL0mNOpQRs";
    let source_ref = ObjectRef {
        provider: spotify,
        kind: ObjectKind::Track,
        id: spotify_id.into(),
    };
    let soundcloud_ref = |id: &str| ObjectRef {
        provider: soundcloud,
        kind: ObjectKind::Track,
        id: id.to_owned(),
    };
    let link = crate::music::ConfirmedLink {
        platform: soundcloud.as_str().into(),
        id: "artist/recording".into(),
        url: "https://soundcloud.com/artist/recording".into(),
        evidence: "ISRC".into(),
        checked_at: 100,
    };
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    assert!(
        store
            .claim_music_match("FR", &source_ref, 100, 150)
            .unwrap()
    );
    store
        .save_music_match(
            "FR",
            &[source_ref],
            150,
            999,
            &serde_json::json!({"links": [&link]}),
        )
        .unwrap();
    store
        .remember_music_aliases("FR", &soundcloud_ref("123"), std::slice::from_ref(&link))
        .unwrap();
    store
        .remember_music_aliases("DE", &soundcloud_ref("456"), &[link])
        .unwrap();
    drop(store);

    let reopened = Store::open(directory.path()).unwrap();
    let links = MatchLinks::load(&reopened, "FR", ObjectKind::Track).unwrap();
    let index = LibraryIndex::build(
        [
            source(spotify, &[("Recording", spotify_id, "2026-11-22")]),
            source(
                soundcloud,
                &[
                    ("Recording", "123", "2026-10-15"),
                    ("Recording", "456", "2026-11-01"),
                ],
            ),
        ],
        &links,
    );
    assert_eq!(index.rows.len(), 2);
    let merged = index
        .rows
        .iter()
        .find(|row| row.scopes().count() == 2)
        .unwrap();
    assert!(
        merged
            .entries()
            .any(|entry| entry.key.1 == EntryId::Source("123".into()))
    );
}

#[test]
fn local_folder_files_supply_the_downloaded_and_saved_groups() {
    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("Imported");
    std::fs::create_dir_all(&folder).unwrap();
    let track = folder.join("Blue.wav");
    let mut wave = b"RIFF".to_vec();
    wave.extend_from_slice(&[40, 0, 0, 0]);
    wave.extend_from_slice(b"WAVEfmt ");
    wave.extend_from_slice(&[
        16, 0, 0, 0, 1, 0, 1, 0, 64, 31, 0, 0, 128, 62, 0, 0, 2, 0, 16, 0,
    ]);
    wave.extend_from_slice(b"data");
    wave.extend_from_slice(&[4, 0, 0, 0, 0, 0, 1, 0]);
    std::fs::write(&track, wave).unwrap();
    std::fs::write(folder.join("notes.txt"), "notes").unwrap();
    let store = Store::open(directory.path()).unwrap();
    let id = store
        .create_local_playlist("First", std::slice::from_ref(&folder))
        .unwrap();
    store.create_local_playlist("Second", &[track]).unwrap();
    let database = rusqlite::Connection::open(directory.path().join("mediadeka.sqlite3")).unwrap();
    database.execute("UPDATE local_playlist_track SET artist = 'Artist', album = 'Album' WHERE playlist_id = ?1", [id]).unwrap();
    let albums = local::read(&store, CollectionKind::Albums).unwrap();
    assert_eq!(albums.len(), 1);
    let EntryAction::LocalGroup(group) = &albums[0].action else {
        panic!("local album");
    };
    let artists = local::read(&store, CollectionKind::Artists).unwrap();
    assert_eq!(artists[0].title, "Artist");
    let local = local::read(&store, CollectionKind::Liked).unwrap();
    let index = LibraryIndex::build([local.into()], &MatchLinks::default());
    let result = index.project(
        Some(&Scope::Local),
        Some(group),
        "blue",
        true,
        LibrarySort::Title,
    );
    assert_eq!(result.visible.len(), 1);
    assert_eq!(result.downloaded, 1);
    assert_eq!(result.counts[&None], 1);
    let playlists = store.local_playlists().unwrap();
    assert_eq!(playlists.len(), 2);
    assert!(
        playlists
            .iter()
            .all(|playlist| playlist.has_files && playlist.track_count == 1)
    );
}

fn source(provider: ProviderId, tracks: &[(&str, &str, &str)]) -> Arc<[Entry]> {
    index::source_entries(
        provider,
        "sample",
        Collection::Liked(
            tracks
                .iter()
                .map(|&(title, id, added)| PlaylistTrack {
                    title: title.into(),
                    id: id.into(),
                    artist: "Artist".into(),
                    artists: Vec::new(),
                    artist_credits: Vec::new(),
                    album_ref: None,
                    album: None,
                    artwork: None,
                    added: Some(added.into()),
                    duration: Some(197),
                    media: Some(Media::Provider {
                        provider,
                        id: id.into(),
                    }),
                })
                .collect(),
        ),
    )
    .into()
}

#[test]
#[ignore = "reads the isolated Jellyfin stage 4 profile using actual connected collection loading"]
fn live_jellyfin_unified_library() {
    let cancelled = AtomicBool::new(false);
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let env = |field| {
        std::env::var(format!("MEDIADEKA_TEST_JELLYFIN_{field}")).expect("fixture environment")
    };
    let session = sign_in(
        ProviderId::Jellyfin,
        LoginInput::JellyfinPassword {
            server: env("URL"),
            username: env("USER"),
            password: env("PASSWORD"),
        },
        &cancelled,
        |_| {},
    )
    .unwrap();
    store.connect_account(&session, || Some(()), || {}).unwrap();
    let result = probe_jellyfin(&store, &cancelled, &env("PLAYLIST"));
    store
        .disconnect_account(ProviderId::Jellyfin, &session.profile.id, || Some(()))
        .unwrap();
    result.unwrap();
}

#[cfg(windows)]
#[test]
#[ignore = "reads the connected Spotify profile and saves its stage 4 snapshots"]
fn live_spotify_unified_library() {
    let directory = std::env::var_os("MEDIADEKA_SPOTIFY_TEST_PROFILE").expect("test profile");
    let store = Store::open(&directory).expect("open Spotify profile");
    let account = store
        .preferred_account(ProviderId::Spotify)
        .unwrap()
        .expect("connected Spotify profile");
    let account = store
        .account(ProviderId::Spotify, &account.profile.id)
        .unwrap()
        .unwrap();
    let region = region();
    let cancelled = AtomicBool::new(false);
    let credentials = store
        .credentials(&account)
        .unwrap()
        .expect("Spotify account access");
    for (kind, object_kind) in [
        (CollectionKind::Albums, ObjectKind::Album),
        (CollectionKind::Artists, ObjectKind::Artist),
    ] {
        let entries = match refresh(&store, &account, &region, kind, &cancelled).unwrap() {
            CollectionUpdate::Data { entries, .. } => entries,
            CollectionUpdate::Error { message, .. } => panic!("{kind:?}: {message}"),
            _ => panic!("expected Spotify {kind:?} collection"),
        };
        let entry = entries
            .iter()
            .find(|entry| {
                !entry.title.is_empty()
                    && entry.artwork.is_some()
                    && entry.added.is_some()
                    && (kind != CollectionKind::Albums
                        || (entry.year.is_some() && !entry.subtitle.is_empty()))
            })
            .expect("Spotify profile must contain a collection with metadata");
        let EntryAction::Object { object, .. } = &entry.action else {
            panic!("expected navigable Spotify {kind:?} entry");
        };
        assert_eq!(object.kind, object_kind);
        if kind == CollectionKind::Albums {
            let page = crate::providers::catalog::read(object, Some(&credentials), &cancelled)
                .expect("open saved Spotify album");
            assert_eq!(page.title, entry.title);
            assert_eq!(page.year, entry.year);
            assert!(!page.tracks.is_empty());
        }
        let reopened = Store::open(&directory).unwrap();
        let snapshot = cached_data(&reopened, &account, &region, kind)
            .unwrap()
            .expect("persisted Spotify snapshot");
        let CollectionUpdate::Data {
            entries: cached, ..
        } = data(&account, snapshot)
        else {
            unreachable!()
        };
        assert_eq!(cached.len(), entries.len());
        assert!(
            cached
                .iter()
                .any(|cached| cached.key == entry.key && cached.year == entry.year)
        );
        let index = LibraryIndex::build([entries], &MatchLinks::default());
        let projection = index.project(None, None, "", false, LibrarySort::Recent);
        assert_eq!(projection.counts[&Some(ProviderId::Spotify)], cached.len());
        println!(
            "{kind:?}: {} Spotify library entries, persisted and projected",
            cached.len()
        );
    }
}

fn probe_jellyfin(store: &Store, cancelled: &AtomicBool, playlist: &str) -> Result<(), String> {
    for (kind, expected) in [
        (CollectionKind::Liked, "Mediadeka Jellyfin Acceptance"),
        (CollectionKind::Albums, "Mediadeka Acceptance Collection"),
        (CollectionKind::Artists, "Mediadeka"),
        (CollectionKind::Playlists, playlist),
    ] {
        let remote = Mutex::new(None);
        let errors = Mutex::new(Vec::new());
        load_from_store(store, kind, cancelled, |update| match update {
            CollectionUpdate::Data {
                provider: ProviderId::Jellyfin,
                entries,
                ..
            } => {
                *remote.lock().unwrap() = Some(entries);
            }
            CollectionUpdate::Error { message, .. } | CollectionUpdate::Issue(message) => {
                errors.lock().unwrap().push(message);
            }
            _ => {}
        })
        .map_err(|error| error.to_string())?;
        if !errors.lock().unwrap().is_empty() {
            return Err(format!("{kind:?}: {:?}", errors.into_inner().unwrap()));
        }
        let rows = remote
            .into_inner()
            .unwrap()
            .ok_or_else(|| format!("{kind:?}: no Jellyfin rows"))?;
        if !rows.iter().any(|row| {
            row.title == expected
                || matches!(
                    &row.action, EntryAction::Object { object, .. } if object.id == expected
                )
        }) {
            return Err(format!(
                "{kind:?}: {expected} absent from {:?}",
                rows.iter().map(|row| &row.title).collect::<Vec<_>>()
            ));
        }
        let index = LibraryIndex::build([rows], &MatchLinks::default());
        let projection = index.project(None, None, "", false, LibrarySort::Recent);
        assert_eq!(
            projection.counts[&Some(ProviderId::Jellyfin)],
            projection.visible.len()
        );
        println!(
            "{kind:?}: {} library rows via Jellyfin",
            projection.visible.len()
        );
    }
    Ok(())
}
