use std::{fs, path::Path};

use super::*;
use crate::storage::{CacheScope, DATABASE, MIGRATIONS, Snapshot, Startup};

#[test]
fn removing_local_track_checks_duplicate_occurrence_and_persists_revision() {
    let directory = tempfile::tempdir().unwrap();
    let version = MIGRATIONS
        .iter()
        .position(|migration| migration.contains(&LOCAL_PLAYLIST_REVISION_MIGRATION))
        .unwrap();
    let old = Store::versioned_fixture(directory.path(), version).unwrap();
    let connection = old.connect().unwrap();
    connection
        .execute(
            "INSERT INTO local_playlist (id, name) VALUES (42, 'Playlist')",
            [],
        )
        .unwrap();
    for (ordinal, path, title) in [
        (4, "first.wav", "First"),
        (6, "second.wav", "Second"),
        (10, "first.wav", "First reprise"),
    ] {
        connection.execute(
            "INSERT INTO local_playlist_track (playlist_id, ordinal, path, title, artist, added)
             VALUES (42, ?1, ?2, ?3, '', '2026-11-26')",
            params![ordinal, path, title],
        ).unwrap();
    }
    drop(connection);
    let current = Store::open(directory.path()).unwrap();
    assert_eq!(
        current
            .local_playlist_snapshot(42)
            .unwrap()
            .unwrap()
            .revision,
        None
    );
    assert!(
        current
            .remove_local_playlist_track(42, 1, "first.wav", None)
            .unwrap()
            .is_none()
    );
    let removed = current
        .remove_local_playlist_track(42, 2, "first.wav", None)
        .unwrap()
        .unwrap();
    assert_eq!(removed.revision, Some(1));
    assert_eq!(
        removed
            .playlist
            .tracks
            .iter()
            .map(|track| track.title.as_str())
            .collect::<Vec<_>>(),
        ["First", "Second"]
    );
    assert!(
        current
            .remove_local_playlist_track(42, 0, "first.wav", None)
            .unwrap()
            .is_none()
    );

    let reopened = Store::open(directory.path()).unwrap();
    let before = reopened.local_playlist_snapshot(42).unwrap().unwrap();
    assert_eq!(before.revision, Some(1));
    assert_eq!(before.playlist, removed.playlist);
    let second = reopened
        .remove_local_playlist_track(42, 0, "first.wav", Some(1))
        .unwrap()
        .unwrap();
    assert_eq!(second.revision, Some(2));
    assert_eq!(second.playlist.tracks[0].title, "Second");
}

fn write_wave(path: &Path) {
    // One real second of 8 kHz, 16-bit PCM with an INFO title and artist.
    let sample_bytes = 16_000u32;
    let list = b"INFOINAM\x06\0\0\0Tones\0IART\x06\0\0\0Synth\0";
    let mut wave = b"RIFF".to_vec();
    wave.extend((36 + sample_bytes + 8 + list.len() as u32).to_le_bytes());
    wave.extend(b"WAVEfmt ");
    wave.extend(16u32.to_le_bytes());
    wave.extend(1u16.to_le_bytes());
    wave.extend(1u16.to_le_bytes());
    wave.extend(8000u32.to_le_bytes());
    wave.extend(16000u32.to_le_bytes());
    wave.extend(2u16.to_le_bytes());
    wave.extend(16u16.to_le_bytes());
    wave.extend(b"data");
    wave.extend(sample_bytes.to_le_bytes());
    wave.extend((0..8000).flat_map(|sample| {
        let frame: i16 = if sample % 20 < 10 { 1000 } else { -1000 };
        frame.to_le_bytes()
    }));
    wave.extend(b"LIST");
    wave.extend((list.len() as u32).to_le_bytes());
    wave.extend(list);
    fs::write(path, wave).unwrap();
}

#[test]
fn current_profile_migrates_with_home_cache_and_guest_route_intact() {
    let directory = tempfile::tempdir().unwrap();
    let legacy = Connection::open(directory.path().join(DATABASE)).unwrap();
    for migration in MIGRATIONS[..4].iter().flat_map(|version| *version) {
        legacy.execute_batch(migration).unwrap();
    }
    legacy
        .execute("INSERT INTO startup (id, kind) VALUES (1, 'guest')", [])
        .unwrap();
    legacy
        .execute(
            r#"INSERT INTO home_cache VALUES ('guest', 'youtube-music', '', 1, '{"title":"Guest","items":[]}')"#,
            [],
        )
        .unwrap();
    legacy.pragma_update(None, "user_version", 4).unwrap();
    drop(legacy);

    let store = Store::open(directory.path()).unwrap();
    assert!(matches!(store.startup().unwrap(), Startup::Guest));
    let Snapshot {
        home,
        received_at_ms,
    } = store.cache(CacheScope::Guest).unwrap().unwrap();
    assert_eq!(home.title, "Guest");
    assert_eq!(received_at_ms, 1);
    let id = store.create_local_playlist("Empty", &[]).unwrap();
    assert!(store.local_playlist(id).unwrap().unwrap().tracks.is_empty());
}

#[test]
fn local_playlists_read_committed_rows_while_another_connection_writes() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let id = store.create_local_playlist("Saved", &[]).unwrap();
    let mut connection = store.connect().unwrap();
    let writer = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    writer
        .execute(
            "UPDATE local_playlist SET name = 'Uncommitted' WHERE id = ?1",
            [id],
        )
        .unwrap();

    let reader = Store::open(directory.path()).unwrap();
    assert_eq!(reader.local_playlists().unwrap()[0].name, "Saved");
    assert_eq!(reader.local_playlist(id).unwrap().unwrap().name, "Saved");

    writer.commit().unwrap();
    assert_eq!(
        reader.local_playlist(id).unwrap().unwrap().name,
        "Uncommitted"
    );
}

#[test]
fn large_local_playlist_survives_restart_with_order_and_duplicate_occurrences() {
    let directory = tempfile::tempdir().unwrap();
    let track = directory.path().join("Музыка.wav");
    write_wave(&track);
    let missing = directory.path().join("missing.wav");
    let list = directory.path().join("Mix.m3u8");
    fs::write(
        &list,
        format!(
            "\u{feff}#EXTM3U\nМузыка.wav\n#EXTINF:12,Off-line track\nmissing.wav\n{}",
            "Музыка.wav\n".repeat(9998)
        ),
    )
    .unwrap();
    let store = Store::open(directory.path()).unwrap();
    let id = store.create_local_playlist(" Mix ", &[list]).unwrap();
    assert!(store.rename_local_playlist(id, "Reopened").unwrap());
    drop(store);

    let store = Store::open(directory.path()).unwrap();
    assert_eq!(
        store.local_playlists().unwrap(),
        [LocalPlaylistSummary {
            id,
            name: "Reopened".into(),
            track_count: 10_000,
            added: Some(chrono::Utc::now().format("%Y-%m-%d").to_string()),
            has_files: true,
        }]
    );
    let playlist = store.local_playlist(id).unwrap().unwrap();
    let a = &playlist.tracks[0];
    let b = &playlist.tracks[1];
    assert_eq!(a.title, "Tones");
    assert_eq!(a.artist, "Synth");
    assert_eq!(a.duration, Some(1));
    assert_eq!(a, &playlist.tracks[9999]);
    assert_eq!(b.title, "Off-line track");
    assert_eq!(b.duration, Some(12));
    assert_eq!(b.media, Some(Media::File(missing)));
    store
        .set_track_favorite(FavoriteScope::Local, &a.id, true, &AtomicBool::new(false))
        .unwrap();
    let second = store.create_local_playlist("Another", &[track]).unwrap();
    assert_eq!(
        store.local_playlist(second).unwrap().unwrap().tracks[0].id,
        a.id
    );
    assert!(store.delete_local_playlist(id).unwrap());
    assert!(!store.delete_local_playlist(id).unwrap());
    assert!(store.local_playlist(id).unwrap().is_none());
    assert!(
        store
            .favorite_tracks(FavoriteScope::Local)
            .unwrap()
            .contains(&a.id)
    );
    let replacement = store.create_local_playlist("Third", &[]).unwrap();
    assert!(replacement > id);
}

#[test]
fn invalid_import_leaves_existing_playlists_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let list = directory.path().join("Remote.m3u8");
    fs::write(&list, "#EXTM3U\nhttps://example.com/track.mp3\n").unwrap();
    let store = Store::open(directory.path()).unwrap();
    let id = store.create_local_playlist("Original", &[]).unwrap();
    assert!(matches!(
        store.create_local_playlist("Invalid", &[list]),
        Err(StoreError::PlaylistInput(_))
    ));
    assert!(matches!(
        store.rename_local_playlist(id, "\t"),
        Err(StoreError::PlaylistInput(_))
    ));
    assert_eq!(store.local_playlists().unwrap().len(), 1);
    assert_eq!(store.local_playlist(id).unwrap().unwrap().name, "Original");
}

#[test]
fn folder_import_reuses_tags_across_restarts_and_checks_both_file_attributes() {
    use std::time::{Duration, Instant, UNIX_EPOCH};

    let directory = tempfile::tempdir().unwrap();
    let folder = directory.path().join("music");
    fs::create_dir(&folder).unwrap();
    for index in 0..65 {
        write_wave(&folder.join(format!("{index:02}.wav")));
    }
    let profile = directory.path().join("profile");
    let files = [folder.clone()];
    let cancelled = AtomicBool::new(false);
    let import = |store: &Store, name| {
        let mut batches = Vec::new();
        let started = Instant::now();
        let id = store
            .import_local_playlist(name, &files, &cancelled, |event| {
                if let ImportProgress::Reading {
                    processed,
                    reused,
                    unreadable,
                    titles,
                } = event
                {
                    assert_eq!(unreadable, 0);
                    assert_eq!(titles.last().unwrap().len(), 5);
                    batches.push((processed, reused));
                }
            })
            .unwrap();
        eprintln!("{name}: {:?}, batches {batches:?}", started.elapsed());
        (id, batches)
    };

    let store = Store::open(&profile).unwrap();
    let (first, batches) = import(&store, "Cold");
    assert_eq!(batches, [(64, 0), (65, 0)]);
    assert_eq!(
        store.local_playlist(first).unwrap().unwrap().tracks.len(),
        65
    );
    drop(store);

    let store = Store::open(&profile).unwrap();
    let (_, batches) = import(&store, "Warm");
    assert_eq!(batches, [(64, 64), (65, 65)]);

    let changed = folder.join("64.wav");
    let mut bytes = fs::read(&changed).unwrap();
    let tag = bytes
        .windows(5)
        .position(|value| value == b"Tones")
        .unwrap();
    let modified = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    bytes[tag..tag + 5].copy_from_slice(b"Steps");
    fs::write(&changed, &bytes).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&changed)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let (third, batches) = import(&store, "Changed timestamp");
    assert_eq!(batches, [(64, 64), (65, 64)]);
    assert_eq!(
        store.local_playlist(third).unwrap().unwrap().tracks[64].title,
        "Steps"
    );

    bytes[tag..tag + 5].copy_from_slice(b"Other");
    bytes.extend_from_slice(b"JUNK\0\0\0\0");
    let riff_size = (bytes.len() as u32 - 8).to_le_bytes();
    bytes[4..8].copy_from_slice(&riff_size);
    fs::write(&changed, bytes).unwrap();
    fs::OpenOptions::new()
        .write(true)
        .open(&changed)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let (fourth, batches) = import(&store, "Changed size");
    assert_eq!(batches, [(64, 64), (65, 64)]);
    assert_eq!(
        store.local_playlist(fourth).unwrap().unwrap().tracks[64].title,
        "Other"
    );
}

#[test]
fn cancelling_a_folder_import_between_batches_leaves_no_partial_playlist() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    for index in 0..65 {
        write_wave(&directory.path().join(format!("{index:02}.wav")));
    }
    let files = [directory.path().to_owned()];
    let cancelled = AtomicBool::new(false);
    let result = store.import_local_playlist("Cancelled", &files, &cancelled, |event| {
        if matches!(event, ImportProgress::Reading { processed: 64, .. }) {
            cancelled.store(true, Ordering::Relaxed);
        }
    });
    assert!(matches!(result, Err(StoreError::ImportCancelled)));
    assert!(store.local_playlists().unwrap().is_empty());

    let mut last = None;
    let id = store
        .import_local_playlist("Retry", &files, &AtomicBool::new(false), |event| {
            if let ImportProgress::Reading {
                processed, reused, ..
            } = event
            {
                last = Some((processed, reused));
            }
        })
        .unwrap();
    assert_eq!(last, Some((65, 64)));
    assert_eq!(store.local_playlist(id).unwrap().unwrap().tracks.len(), 65);
}

#[test]
fn appending_preserves_occurrence_order_without_creating_a_playlist() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let files = ["first.wav", "second.wav", "third.wav"].map(|name| directory.path().join(name));
    for file in &files {
        write_wave(file);
    }
    let id = store
        .create_local_playlist("Keep this playlist", &files[..1])
        .unwrap();
    let cancelled = AtomicBool::new(false);
    assert!(
        store
            .append_local_playlist(id, &files[1..], &cancelled, |_| {})
            .unwrap()
            .is_some()
    );
    let summaries = store.local_playlists().unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].id, id);
    assert_eq!(summaries[0].track_count, 3);
    let playlist = store.local_playlist(id).unwrap().unwrap();
    for (track, path) in playlist.tracks.iter().zip(&files) {
        assert_eq!(Path::new(&track.id).file_name(), path.file_name());
    }

    let result = store.append_local_playlist(id, &files, &cancelled, |event| {
        if let ImportProgress::Appending { count, titles } = event {
            assert_eq!(count, 3);
            assert_eq!(titles, ["Tones", "Tones", "Tones"]);
            cancelled.store(true, Ordering::Relaxed);
        }
    });
    assert!(matches!(result, Err(StoreError::ImportCancelled)));
    assert_eq!(store.local_playlist(id).unwrap().unwrap(), playlist);

    let removed = store.append_local_playlist(id, &files, &AtomicBool::new(false), |event| {
        if matches!(event, ImportProgress::Reading { .. }) {
            assert!(store.delete_local_playlist(id).unwrap());
        }
    });
    assert!(removed.unwrap().is_none());
    assert!(store.local_playlists().unwrap().is_empty());
}

#[test]
fn append_offsets_are_based_on_the_committed_playlist() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let files = ["first.wav", "second.wav"].map(|name| directory.path().join(name));
    for file in &files {
        write_wave(file);
    }
    let id = store.create_local_playlist("Shared", &[]).unwrap();
    let cancelled = AtomicBool::new(false);
    let mut inserted = false;
    assert!(
        store
            .append_local_playlist(id, &files[..1], &cancelled, |progress| {
                if matches!(progress, ImportProgress::Reading { .. }) {
                    inserted = store
                        .append_local_playlist(id, &files[1..], &cancelled, |_| {})
                        .unwrap()
                        .is_some();
                }
            })
            .unwrap()
            .is_some()
    );
    assert!(inserted);
    let playlist = store.local_playlist(id).unwrap().unwrap();
    assert_eq!(playlist.tracks.len(), 2);
    assert_eq!(
        Path::new(&playlist.tracks[0].id).file_name(),
        files[1].file_name()
    );
    assert_eq!(
        Path::new(&playlist.tracks[1].id).file_name(),
        files[0].file_name()
    );
}

#[test]
fn favorites_and_local_playlists_are_profile_scoped() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let store = Store::open(first.path()).unwrap();
    let other_store = Store::open(second.path()).unwrap();
    let id = store.create_local_playlist("Local", &[]).unwrap();
    let guest = FavoriteScope::Guest(ProviderId::YouTubeMusic);
    let account = FavoriteScope::Account(ProviderId::YouTubeMusic, "one");
    let local = FavoriteScope::Local;
    let track = "shared-id";
    let active = AtomicBool::new(false);
    assert!(
        store
            .set_track_favorite(local, track, true, &active)
            .unwrap()
    );
    store
        .set_track_favorite(local, track, true, &active)
        .unwrap();
    store
        .set_track_favorite(guest, track, true, &active)
        .unwrap();
    assert!(other_store.local_playlist(id).unwrap().is_none());
    assert!(other_store.favorite_tracks(guest).unwrap().is_empty());
    assert_eq!(store.favorite_tracks(guest).unwrap().len(), 1);
    assert_eq!(store.favorite_tracks(local).unwrap().len(), 1);
    assert!(
        store
            .set_track_favorite(account, track, true, &active)
            .is_err()
    );
    let mut db = store.connect().unwrap();
    let transaction = db.transaction().unwrap();
    transaction
        .execute(
            "INSERT INTO account VALUES ('youtube-music', 'one', 'Profile 1', '{}', 'ref')",
            [],
        )
        .unwrap();
    transaction
        .execute(
            "INSERT INTO account_source VALUES ('youtube-music', 'one')",
            [],
        )
        .unwrap();
    transaction.commit().unwrap();
    store
        .set_track_favorite(account, track, true, &active)
        .unwrap();
    assert!(store.favorite_tracks(account).unwrap().contains(track));
    assert!(
        store
            .favorite_tracks(FavoriteScope::Account(ProviderId::YouTubeMusic, "two"))
            .unwrap()
            .is_empty()
    );
    delete_account_favorites(&db, ProviderId::YouTubeMusic, "one").unwrap();
    let cancelled = AtomicBool::new(true);
    assert!(
        !store
            .set_track_favorite(account, track, true, &cancelled)
            .unwrap()
    );
    assert!(store.favorite_tracks(account).unwrap().is_empty());
    store
        .set_track_favorite(guest, track, false, &active)
        .unwrap();
    assert!(store.favorite_tracks(guest).unwrap().is_empty());
    assert_eq!(store.favorite_tracks(local).unwrap().len(), 1);
}
