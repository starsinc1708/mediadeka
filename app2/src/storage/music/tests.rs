use super::*;
use crate::{
    music::{ConfirmedLink, MusicPage, ObjectKind},
    playlist::{ArtistCredit, Media, Playlist, PlaylistTrack},
    providers::{Capabilities, HomeItem, collections::Collection},
};

const PROVIDER: ProviderId = ProviderId::YandexMusic;
const REGION: &str = "RU";

fn connect(store: &Store, profile: &str) -> Account {
    let capabilities = serde_json::to_string(&Capabilities {
        home: true,
        profile_selection: false,
        device_code: true,
    })
    .unwrap();
    let mut connection = store.connect().unwrap();
    let transaction = connection.transaction().unwrap();
    // Represent an already verified profile without writing a test secret to the vault.
    transaction
        .execute(
            "INSERT INTO account VALUES (?1, ?2, ?2, ?3, hex(randomblob(16)))
             ON CONFLICT(provider,profile_id) DO UPDATE SET credential_ref=excluded.credential_ref",
            params![PROVIDER.as_str(), profile, capabilities],
        )
        .unwrap();
    transaction
        .execute(
            "INSERT INTO account_source VALUES (?1,?2)
             ON CONFLICT(provider) DO UPDATE SET profile_id=excluded.profile_id",
            params![PROVIDER.as_str(), profile],
        )
        .unwrap();
    transaction.commit().unwrap();
    store.account(PROVIDER, profile).unwrap().unwrap()
}

fn recording() -> ObjectRef {
    ObjectRef {
        provider: PROVIDER,
        kind: ObjectKind::Track,
        id: "42:71".into(),
    }
}

#[test]
fn previous_snapshot_survives_the_freshness_migration() {
    let directory = tempfile::tempdir().unwrap();
    let previous = super::super::MIGRATIONS
        .iter()
        .position(|migration| migration.contains(&SNAPSHOT_TIMESTAMP_MIGRATION))
        .unwrap();
    let old_store = Store::versioned_fixture(directory.path(), previous).unwrap();
    let page = MusicPage::new(recording(), "Existing".into());
    old_store
        .connect()
        .unwrap()
        .execute(
            "INSERT INTO music_snapshot (provider, profile_id, region, slot, payload)
             VALUES (?1, '', ?2, 'track:42:71', ?3)",
            params![
                PROVIDER.as_str(),
                REGION,
                serde_json::to_string(&page).unwrap()
            ],
        )
        .unwrap();
    let store = Store::open(directory.path()).unwrap();
    assert!(matches!(
        store.versioned_music_snapshot::<MusicPage>(PROVIDER, None, REGION, "track:42:71", None)
            .unwrap(),
        Some(MusicSnapshot::Changed { value, saved_at_ms: 0, .. }) if value == page
    ));
    let generation = store.catalog_generation().unwrap();
    let CatalogSave::Stored {
        saved_at_ms: new_stamp,
        ..
    } = store
        .save_catalog_snapshot(PROVIDER, None, REGION, "track:42:71", &page, generation)
        .unwrap()
    else {
        panic!("expected a new snapshot");
    };
    assert!(new_stamp > 0);
    assert!(matches!(
        store
            .versioned_music_snapshot::<MusicPage>(
                PROVIDER,
                None,
                REGION,
                "track:42:71",
                Some((new_stamp, generation))
            )
            .unwrap(),
        Some(MusicSnapshot::Unchanged)
    ));
}

#[test]
fn catalog_allowance_evicts_old_details_and_retains_collection_snapshots() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    for (slot, value) in [
        ("album:older", "Old"),
        ("album:newer", "New"),
        ("artist:popular", "Artist"),
        ("album:oversized", "Too large for this allowance"),
        ("liked", "Collection"),
        ("playlist:private", "Playlist"),
    ] {
        assert!(
            store
                .save_music_snapshot(PROVIDER, None, REGION, slot, &value)
                .unwrap()
        );
    }
    let connection = store.connect().unwrap();
    connection
        .execute(
            "UPDATE music_snapshot SET saved_at_ms=CASE slot WHEN 'album:older' THEN 1
             WHEN 'album:newer' THEN 2 ELSE 3 END",
            [],
        )
        .unwrap();
    trim_catalog_snapshots(&connection, CATALOG_DETAIL_SLOTS, 10, 8).unwrap();
    for (slot, expected) in [
        ("album:older", None),
        ("album:newer", None),
        ("artist:popular", Some("Artist")),
        ("album:oversized", None),
        ("liked", Some("Collection")),
        ("playlist:private", Some("Playlist")),
    ] {
        let actual = store
            .music_snapshot::<String>(PROVIDER, None, REGION, slot)
            .unwrap();
        assert_eq!(actual.as_deref(), expected);
    }
}

#[test]
fn remote_playlist_budget_migrates_large_cache_and_enforces_new_writes() {
    let directory = tempfile::tempdir().unwrap();
    let previous = super::super::MIGRATIONS
        .iter()
        .position(|migration| migration.contains(&PLAYLIST_SNAPSHOT_BUDGET_MIGRATION))
        .unwrap();
    let old = Store::versioned_fixture(directory.path(), previous).unwrap();
    let local = old.create_local_playlist("Authored list", &[]).unwrap();
    old.connect().unwrap().execute(
        "WITH RECURSIVE ids(id) AS (SELECT 1 UNION ALL SELECT id+1 FROM ids WHERE id <= ?3)
         INSERT INTO music_snapshot (provider,profile_id,region,slot,payload,saved_at_ms,encoded_bytes)
         SELECT ?1,'',?2,'playlist:'||id,'true',id,4 FROM ids",
        params![PROVIDER.as_str(), REGION, CATALOG_PLAYLIST_LIMIT],
    ).unwrap();
    for outside in ["liked", "album:inside", "collections:user"] {
        old.save_music_snapshot(PROVIDER, None, REGION, outside, &true)
            .unwrap();
    }
    let store = Store::open(directory.path()).unwrap();
    let contains = |slot| {
        store
            .music_snapshot::<bool>(PROVIDER, None, REGION, slot)
            .unwrap()
            .is_some()
    };
    assert!(!contains("playlist:1"));
    assert!(contains("playlist:2"));
    let generation = store.catalog_generation().unwrap();
    assert!(matches!(
        store
            .save_catalog_snapshot(PROVIDER, None, REGION, "playlist:fresh", &true, generation)
            .unwrap(),
        CatalogSave::Stored { .. }
    ));
    assert!(!contains("playlist:2"));
    assert!(contains("playlist:3"));
    assert!(contains("playlist:fresh"));
    trim_catalog_snapshots(&store.connect().unwrap(), CATALOG_PLAYLIST_SLOTS, 50, 5).unwrap();
    assert!(contains("playlist:fresh"));
    assert!(!contains("playlist:257"));
    assert!(contains("liked"));
    assert!(contains("album:inside"));
    assert!(contains("collections:user"));
    assert!(store.local_playlist_exists(local).unwrap());
}

#[test]
fn clearing_catalog_snapshots_rejects_old_writes_and_keeps_profile_records() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let account = connect(&store, "keep");
    let local = store.create_local_playlist("Keep playlist", &[]).unwrap();
    let object = recording();
    let generation = store.catalog_generation().unwrap();
    for slot in [
        "track:42:71",
        "album:42",
        "artist:14",
        "genre:rock",
        "playlist:owner:42",
    ] {
        assert!(matches!(
            store
                .save_catalog_snapshot(PROVIDER, Some(&account), REGION, slot, &"old", generation)
                .unwrap(),
            CatalogSave::Stored { .. }
        ));
    }
    assert!(
        store
            .save_music_snapshot(PROVIDER, Some(&account), REGION, "liked", &"keep")
            .unwrap()
    );
    assert!(store.claim_music_match(REGION, &object, 1, 60).unwrap());
    store
        .save_music_match(REGION, std::slice::from_ref(&object), 60, 100, &"verified")
        .unwrap();

    let (count, next) = store.clear_catalog_snapshots().unwrap();
    assert_eq!(count, 5);
    assert_eq!(next, generation + 1);
    for slot in ["track:42:71", "playlist:owner:42"] {
        assert!(
            store
                .music_snapshot::<String>(PROVIDER, Some(&account), REGION, slot)
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            store
                .save_catalog_snapshot(PROVIDER, Some(&account), REGION, slot, &"late", generation)
                .unwrap(),
            CatalogSave::CacheCleared
        ));
    }
    assert_eq!(
        store
            .music_snapshot::<String>(PROVIDER, Some(&account), REGION, "liked")
            .unwrap()
            .as_deref(),
        Some("keep")
    );
    assert_eq!(
        store
            .music_match::<String>(REGION, &object)
            .unwrap()
            .as_deref(),
        Some("verified")
    );
    assert_eq!(
        store.account(PROVIDER, "keep").unwrap(),
        Some(account.clone())
    );
    assert!(store.local_playlist(local).unwrap().is_some());
    assert!(matches!(
        store
            .save_catalog_snapshot(
                PROVIDER,
                Some(&account),
                REGION,
                "playlist:owner:42",
                &"new",
                next
            )
            .unwrap(),
        CatalogSave::Stored { .. }
    ));
}

#[test]
fn playback_uses_confirmed_recordings_for_the_current_connected_profile() {
    use crate::{
        music::sound::{AlternateStream, confirmed_streams},
        playlist::PlaylistKey,
    };

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let region = crate::music::region();
    let source = ObjectRef {
        provider: ProviderId::Spotify,
        kind: ObjectKind::Track,
        id: "69kOkLUCkxIZYexIgSG8rq".into(),
    };
    let track = PlaylistTrack {
        id: source.id.clone(),
        title: "Track".into(),
        artist: "Artist".into(),
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album_ref: None,
        album: None,
        artwork: None,
        added: None,
        duration: Some(180),
        media: Some(Media::Provider {
            provider: source.provider,
            id: source.id.clone(),
        }),
    };
    let playlist = PlaylistKey::Source {
        provider: source.provider,
        profile: Some("spotify-listener".into()),
        id: "liked".into(),
    };
    let target = recording();
    let confirmed = ConfirmedLink {
        platform: target.provider.as_str().into(),
        id: target.id.clone(),
        url: target.url().unwrap(),
        evidence: "ISRC".into(),
        checked_at: 1,
    };
    assert!(store.claim_music_match(&region, &source, 0, 1).unwrap());
    store
        .save_music_match(
            &region,
            &[source, target.clone()],
            1,
            2,
            &serde_json::json!({ "links": [confirmed] }),
        )
        .unwrap();
    assert!(
        confirmed_streams(&store, &playlist, &track)
            .unwrap()
            .is_empty()
    );
    for profile in ["first", "second"] {
        connect(&store, profile);
        assert_eq!(
            confirmed_streams(&store, &playlist, &track).unwrap(),
            vec![AlternateStream {
                provider: PROVIDER,
                profile: profile.into(),
                id: target.id.clone(),
                evidence: "Подтверждённая музыкальная ссылка",
            }]
        );
    }
}

#[test]
fn collections_playlists_and_music_pages_survive_restart_within_their_scope() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let account = connect(&store, "listener");
    let track = PlaylistTrack {
        id: "42:71".into(),
        title: "Track".into(),
        artist: "Joey Valence & Brae, JPEGMAFIA".into(),
        artists: vec!["Joey Valence & Brae".into(), "JPEGMAFIA".into()],
        artist_credits: vec![
            ArtistCredit::from_catalog("Joey Valence & Brae".into(), PROVIDER, Some("7".into())),
            ArtistCredit::from_catalog("JPEGMAFIA".into(), PROVIDER, None),
        ],
        album_ref: ObjectRef::native(PROVIDER, ObjectKind::Album, "71".into()),
        album: Some("Album".into()),
        artwork: None,
        added: None,
        duration: Some(180),
        media: Some(Media::Provider {
            provider: PROVIDER,
            id: "42:71".into(),
        }),
    };
    let liked = Collection::Liked(vec![track.clone()]);
    let playlists = Collection::Playlists(vec![HomeItem {
        id: "playlist:121:3".into(),
        title: "Saved selection".into(),
        subtitle: "Playlist".into(),
        url: None,
        artwork: None,
    }]);
    let playlist = Playlist {
        name: "Saved selection".into(),
        owner: None,
        artwork: None,
        link: None,
        tracks: vec![track.clone()],
    };
    let mut page = MusicPage::new(recording(), track.title.clone());
    page.artist_credits.clone_from(&track.artist_credits);
    page.album_ref.clone_from(&track.album_ref);
    page.tracks.push(track);
    let playlist_slot = "playlist:121:3";
    let track_slot = "track:42:71";
    for (slot, value) in [("liked", &liked), ("playlists", &playlists)] {
        assert!(
            store
                .save_music_snapshot(PROVIDER, Some(&account), REGION, slot, value)
                .unwrap()
        );
    }
    assert!(
        store
            .save_music_snapshot(PROVIDER, Some(&account), REGION, playlist_slot, &playlist)
            .unwrap()
    );
    assert!(
        store
            .save_music_snapshot(PROVIDER, Some(&account), REGION, track_slot, &page)
            .unwrap()
    );
    drop(store);

    let store = Store::open(directory.path()).unwrap();
    let restored = store.account(PROVIDER, "listener").unwrap().unwrap();
    assert_eq!(restored, account);
    for (slot, value) in [("liked", liked), ("playlists", playlists)] {
        assert_eq!(
            store
                .music_snapshot(PROVIDER, Some(&restored), REGION, slot)
                .unwrap(),
            Some(value)
        );
    }
    assert_eq!(
        store
            .music_snapshot(PROVIDER, Some(&restored), REGION, playlist_slot)
            .unwrap(),
        Some(playlist)
    );
    assert_eq!(
        store
            .music_snapshot(PROVIDER, Some(&restored), REGION, track_slot)
            .unwrap(),
        Some(page)
    );
    assert!(
        store
            .music_snapshot::<MusicPage>(PROVIDER, Some(&restored), "US", track_slot)
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .music_snapshot::<MusicPage>(PROVIDER, None, REGION, track_slot)
            .unwrap()
            .is_none()
    );
}

#[test]
fn disconnected_account_loses_all_regions_and_stale_access_cannot_write_or_purge() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let old = connect(&store, "listener");
    let first = Collection::Liked(Vec::new());
    for region in [REGION, "US"] {
        assert!(
            store
                .save_music_snapshot(PROVIDER, Some(&old), region, "liked", &first)
                .unwrap()
        );
    }

    let other = connect(&store, "other");
    assert!(
        store
            .music_snapshot::<Collection>(PROVIDER, Some(&other), REGION, "liked")
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .music_snapshot::<Collection>(PROVIDER, Some(&old), REGION, "liked")
            .unwrap()
            .is_none()
    );
    assert!(
        !store
            .save_music_snapshot(PROVIDER, Some(&old), REGION, "liked", &first)
            .unwrap()
    );
    assert!(
        store
            .save_music_snapshot(PROVIDER, Some(&other), REGION, "liked", &first)
            .unwrap()
    );

    let current = connect(&store, "listener");
    assert!(!store.forget_music_account(&old).unwrap());
    assert!(
        store
            .music_snapshot::<Collection>(PROVIDER, Some(&old), REGION, "liked")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .music_snapshot(PROVIDER, Some(&current), REGION, "liked")
            .unwrap(),
        Some(first.clone())
    );
    assert!(
        store
            .disconnect_account(PROVIDER, &current.profile.id, || Some(()))
            .unwrap()
            .is_some()
    );
    let reconnected = connect(&store, "listener");
    for region in [REGION, "US"] {
        assert!(
            store
                .music_snapshot::<Collection>(PROVIDER, Some(&reconnected), region, "liked")
                .unwrap()
                .is_none()
        );
    }
    assert!(
        !store
            .save_music_snapshot(PROVIDER, Some(&current), REGION, "liked", &first)
            .unwrap()
    );
    let other = connect(&store, "other");
    assert_eq!(
        store
            .music_snapshot(PROVIDER, Some(&other), REGION, "liked")
            .unwrap(),
        Some(first)
    );
}

#[test]
fn match_lease_survives_restart_and_positive_aliases_stay_in_one_region() {
    let directory = tempfile::tempdir().unwrap();
    let source = recording();
    let alias = ObjectRef {
        provider: ProviderId::Spotify,
        kind: ObjectKind::Track,
        id: "75UGzVK6Obcgg4TToPEJHk".into(),
    };
    let result = vec![ConfirmedLink {
        platform: alias.provider.as_str().into(),
        id: alias.id.clone(),
        url: alias.url().unwrap(),
        evidence: "ISRC".into(),
        checked_at: 100,
    }];
    let objects = [source, alias];
    let [source, alias] = &objects;
    let store = Store::open(directory.path()).unwrap();
    assert!(store.claim_music_match(REGION, source, 10, 20).unwrap());
    drop(store);

    let store = Store::open(directory.path()).unwrap();
    assert!(!store.claim_music_match(REGION, source, 19, 30).unwrap());
    assert!(store.claim_music_match(REGION, source, 20, 30).unwrap());
    assert!(store.claim_music_match("US", source, 20, 30).unwrap());
    store
        .save_music_match(REGION, &objects, 20, u64::MAX, &result)
        .unwrap();
    for object in &objects {
        assert!(
            store
                .music_match::<Vec<ConfirmedLink>>(REGION, object)
                .unwrap()
                .is_none()
        );
    }

    store
        .save_music_match(REGION, &objects, 30, u64::MAX, &result)
        .unwrap();
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    for object in &objects {
        assert_eq!(
            store.music_match(REGION, object).unwrap(),
            Some(result.clone())
        );
        assert!(!store.claim_music_match(REGION, object, 1000, 2000).unwrap());
        assert!(
            store
                .music_match::<Vec<ConfirmedLink>>("US", object)
                .unwrap()
                .is_none()
        );
    }
    assert!(store.claim_music_match("US", alias, 1000, 2000).unwrap());
    assert!(
        store
            .reopen_completed_music_match(REGION, source, ProviderId::Spotify, 2000)
            .unwrap()
    );
    assert!(
        !store
            .reopen_completed_music_match(REGION, source, ProviderId::Spotify, 2001)
            .unwrap()
    );
    assert_eq!(store.music_match(REGION, source).unwrap(), Some(result));
    store
        .save_music_match(
            REGION,
            &objects,
            2000,
            u64::MAX,
            &serde_json::json!({"links": [{"platform": "spotify"}]}),
        )
        .unwrap();
    assert!(
        !store
            .reopen_completed_music_match(REGION, source, ProviderId::Spotify, 2001)
            .unwrap()
    );
}

#[test]
fn confirmed_revocation_blocks_an_in_flight_success_until_credentials_change() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let previous = connect(&store, "listener");
    let data = Collection::Liked(Vec::new());
    assert!(
        store
            .save_music_snapshot(PROVIDER, Some(&previous), REGION, "liked", &data)
            .unwrap()
    );
    assert!(store.forget_music_account(&previous).unwrap());
    assert!(!store.forget_music_account(&previous).unwrap());
    assert!(
        !store
            .save_music_snapshot(PROVIDER, Some(&previous), REGION, "liked", &data)
            .unwrap()
    );
    assert!(
        store
            .music_snapshot::<Collection>(PROVIDER, Some(&previous), REGION, "liked")
            .unwrap()
            .is_none()
    );
    let current = connect(&store, "listener");
    assert!(
        store
            .save_music_snapshot(PROVIDER, Some(&current), REGION, "liked", &data)
            .unwrap()
    );
    assert!(!store.forget_music_account(&previous).unwrap());
    assert_eq!(
        store
            .music_snapshot(PROVIDER, Some(&current), REGION, "liked")
            .unwrap(),
        Some(data)
    );
}
