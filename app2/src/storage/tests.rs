use super::*;
use crate::providers::{Home, HomeItem};

fn snapshot(title: &str, received_at_ms: i64) -> Snapshot {
    Snapshot {
        home: Home {
            title: title.into(),
            items: Vec::new(),
            sections: Vec::new(),
        },
        received_at_ms,
    }
}

#[test]
fn concurrent_partial_cache_updates_merge_before_commit() {
    use std::{sync::Barrier, thread};

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let scope = CacheScope::Guest;
    let other_scope = CacheScope::Account(ProviderId::SoundCloud, "other-profile");
    let original = snapshot("Guest", 1);
    let other = snapshot("Another profile", 1);
    store.save_cache(scope, &original, || Some(())).unwrap();
    store.save_cache(other_scope, &other, || Some(())).unwrap();

    let start = Barrier::new(2);
    thread::scope(|threads| {
        for id in ["one", "two"] {
            let store = &store;
            let start = &start;
            threads.spawn(move || {
                start.wait();
                store
                    .merge_session_cache(
                        scope,
                        |previous| {
                            let mut home = previous.unwrap().clone();
                            home.items.push(HomeItem {
                                id: id.into(),
                                title: id.into(),
                                subtitle: String::new(),
                                url: None,
                                artwork: None,
                            });
                            (home, ())
                        },
                        || Some(()),
                    )
                    .unwrap();
            });
        }
    });
    let reopened = Store::open(directory.path()).unwrap();
    let merged = reopened.cache(scope).unwrap().unwrap();
    let mut ids: Vec<_> = merged
        .home
        .items
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, ["one", "two"]);
    assert_eq!(reopened.cache(other_scope).unwrap(), Some(other));

    store
        .merge_session_cache(scope, |_| (original.home, ()), || None::<()>)
        .unwrap();
    assert_eq!(reopened.cache(scope).unwrap(), Some(merged));
}

#[test]
fn migrates_populated_v1_profile_and_keeps_route() {
    let directory = tempfile::tempdir().unwrap();
    let legacy = Connection::open(directory.path().join(DATABASE)).unwrap();
    for migration in MIGRATIONS[0] {
        legacy.execute_batch(migration).unwrap();
    }
    legacy.pragma_update(None, "user_version", 1).unwrap();
    legacy
        .execute("INSERT INTO startup (id, kind) VALUES (1, 'guest')", [])
        .unwrap();
    drop(legacy);

    let store = Store::open(directory.path()).unwrap();
    assert!(matches!(store.startup().unwrap(), Startup::Guest));
    let version: i64 = store
        .connect()
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, MIGRATIONS.len() as i64);
    store
        .save_cache(CacheScope::Guest, &snapshot("Guest", 1), || Some(()))
        .unwrap();
    assert_eq!(
        store.cache(CacheScope::Guest).unwrap(),
        Some(snapshot("Guest", 1))
    );
}

#[test]
fn migrates_a_populated_current_profile_into_the_new_tables() {
    use crate::playlist::{Media, PlaylistKey, PlaylistTrack};

    let directory = tempfile::tempdir().unwrap();
    let previous = 8;
    let legacy = Connection::open(directory.path().join(DATABASE)).unwrap();
    for migration in MIGRATIONS[..previous].iter().flat_map(|version| *version) {
        legacy.execute_batch(migration).unwrap();
    }
    legacy
        .pragma_update(None, "user_version", previous as i64)
        .unwrap();
    legacy
        .execute(
            "INSERT INTO local_playlist (id, name) VALUES (7, 'Старый')",
            [],
        )
        .unwrap();
    legacy
        .execute(
            "INSERT INTO local_playlist_track
                (playlist_id, ordinal, path, title, artist, added)
             VALUES (7, 0, 'C:/audio.mp3', 'Трек', 'Артист', '2026-01-01')",
            [],
        )
        .unwrap();
    drop(legacy);

    let store = Store::open(directory.path()).unwrap();
    let version: i64 = store
        .connect()
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap();
    assert_eq!(version, MIGRATIONS.len() as i64);
    let playlist = store.local_playlist(7).unwrap().expect("local playlist");
    assert_eq!(playlist.name, "Старый");
    assert_eq!(playlist.tracks.len(), 1);
    assert_eq!(playlist.tracks[0].artist, "Артист");
    assert!(playlist.tracks[0].artists.is_empty());
    let track = PlaylistTrack {
        id: "track".into(),
        title: "Новый".into(),
        artist: "Артист".into(),
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album_ref: None,
        album: None,
        artwork: None,
        added: None,
        duration: Some(90),
        media: Some(Media::File("audio.mp3".into())),
    };
    store
        .record_history(
            &PlaylistKey::Local(7),
            &track,
            crate::playback::SoundSource::LocalFile,
        )
        .unwrap();
    assert_eq!(store.listening_history().unwrap().len(), 1);
    assert!(
        store
            .pin_object(&NewPin {
                id: PinId {
                    kind: PinnedKind::Playlist,
                    provider: None,
                    profile: String::new(),
                    object_id: "7".into(),
                },
                title: None,
                artwork: None,
                link: None,
            })
            .unwrap()
    );
    assert_eq!(store.pinned_objects().unwrap().len(), 1);
    assert_eq!(store.panel_state().unwrap(), PanelState::default());
}

#[test]
fn migrates_legacy_bindings_to_one_profile_per_provider() {
    let directory = tempfile::tempdir().unwrap();
    let legacy = Connection::open(directory.path().join(DATABASE)).unwrap();
    for migration in MIGRATIONS[..2].iter().flat_map(|version| *version) {
        legacy.execute_batch(migration).unwrap();
    }
    legacy.pragma_update(None, "user_version", 2).unwrap();
    let capabilities = serde_json::to_string(&Capabilities {
        home: true,
        profile_selection: false,
        device_code: false,
    })
    .unwrap();
    for (provider, id, name) in [
        ("spotify", "active", "Active Spotify"),
        ("spotify", "newer-row", "Newer Spotify"),
        ("soundcloud", "old", "Old SoundCloud"),
        ("soundcloud", "new", "New SoundCloud"),
    ] {
        legacy
            .execute(
                "INSERT INTO account VALUES (?1, ?2, ?3, ?4, '')",
                params![provider, id, name, capabilities],
            )
            .unwrap();
    }
    legacy.execute(
        "INSERT INTO startup (id, kind, provider, profile_id) VALUES (1, 'account', 'spotify', 'active')",
        [],
    ).unwrap();
    drop(legacy);

    let store = Store::open(directory.path()).unwrap();
    let account_ids = store
        .accounts()
        .unwrap()
        .into_iter()
        .map(|account| (account.provider, account.profile.id))
        .collect::<Vec<_>>();
    assert_eq!(
        account_ids,
        [
            (ProviderId::SoundCloud, "new".into()),
            (ProviderId::Spotify, "active".into()),
        ]
    );
    assert!(
        store
            .account(ProviderId::Spotify, "newer-row")
            .unwrap()
            .is_none()
    );
    assert!(
        !store
            .select_account(ProviderId::SoundCloud, "old", || Some(()))
            .unwrap()
    );
    assert!(matches!(
        store.startup().unwrap(),
        Startup::Account(account) if account.profile.id == "active"
    ));
}

#[test]
fn migrates_connected_profile_without_a_startup_route() {
    let directory = tempfile::tempdir().unwrap();
    let legacy = Connection::open(directory.path().join(DATABASE)).unwrap();
    for migration in MIGRATIONS[..3].iter().flat_map(|version| *version) {
        legacy.execute_batch(migration).unwrap();
    }
    legacy.pragma_update(None, "user_version", 3).unwrap();
    legacy
        .execute_batch(
            r#"INSERT INTO account VALUES ('spotify', 'saved', 'Saved Spotify',
            '{"home":true,"profile_selection":false,"device_code":false}', '');
         INSERT INTO account_source VALUES ('spotify', 'saved');"#,
        )
        .unwrap();
    drop(legacy);

    let store = Store::open(directory.path()).unwrap();
    assert!(matches!(
        store.startup().unwrap(),
        Startup::Account(account) if account.profile.id == "saved"
    ));
    let selected = store
        .connect()
        .unwrap()
        .query_row("SELECT profile_id FROM startup WHERE id = 1", [], |row| {
            row.get::<_, String>(0)
        })
        .unwrap();
    assert_eq!(selected, "saved");
}

#[test]
fn cache_isolated_between_guests_profiles_and_providers() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let scopes = [
        (CacheScope::Guest, snapshot("Guest", 12)),
        (
            CacheScope::Account(ProviderId::YouTubeMusic, "one"),
            snapshot("YouTube one", 12),
        ),
        (
            CacheScope::Account(ProviderId::YouTubeMusic, "two"),
            snapshot("YouTube two", 12),
        ),
        (
            CacheScope::Account(ProviderId::Spotify, "one"),
            snapshot("Spotify one", 12),
        ),
    ];
    for (scope, snapshot) in &scopes {
        store.save_cache(*scope, snapshot, || Some(())).unwrap();
    }
    store
        .save_cache(CacheScope::Guest, &snapshot("Old", 11), || Some(()))
        .unwrap();

    let reopened = Store::open(directory.path()).unwrap();
    for (scope, snapshot) in scopes {
        assert_eq!(reopened.cache(scope).unwrap(), Some(snapshot));
    }
    assert!(
        reopened
            .cache(CacheScope::Account(ProviderId::SoundCloud, "one"))
            .unwrap()
            .is_none()
    );
}

#[cfg(windows)]
#[test]
fn a_saved_youtube_profile_replaces_legacy_guest_without_sharing_its_cache() {
    use crate::state::{ActiveSession, HomeContent, HomeView, Intent, Screen, State};

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let provider = ProviderId::YouTubeMusic;
    let session = Session {
        provider,
        profile: Profile {
            id: "saved-user".into(),
            name: "Saved YouTube Music".into(),
        },
        credentials: Credentials::YouTubeMusic {
            cookies: String::new(),
            index: 0,
            page_id: None,
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: true,
            device_code: false,
        },
    };
    assert!(store.use_account(&session, || Some(()), || {}).unwrap());
    let account = store.preferred_account(provider).unwrap().unwrap();
    let _credential = TestCredential(target(provider, &account.credential_ref).unwrap());
    let guest_snapshot = snapshot("Guest snapshot", 1);
    store
        .save_cache(CacheScope::Guest, &guest_snapshot, || Some(()))
        .unwrap();
    assert!(!store.use_guest(|| Some(())).unwrap());
    store
        .connect()
        .unwrap()
        .execute(
            "UPDATE startup SET kind = 'guest', provider = NULL, profile_id = NULL WHERE id = 1",
            [],
        )
        .unwrap();

    let mut state = State::default();
    let startup = state.begin(Intent::Startup);
    let preview = startup.home_preview(&store).unwrap();
    assert!(matches!(
        preview.session,
        ActiveSession::Account {
            provider: ProviderId::YouTubeMusic,
            ..
        }
    ));
    assert!(matches!(preview.content, HomeContent::Loading));
    assert!(state.set_home_preview(&startup, preview));
    assert!(state.has_youtube_account());

    let account_snapshot = snapshot("Account snapshot", 1);
    store
        .save_cache(
            CacheScope::Account(provider, &session.profile.id),
            &account_snapshot,
            || Some(()),
        )
        .unwrap();
    let preview = startup.home_preview(&store).unwrap();
    assert!(matches!(
        preview.content,
        HomeContent::Fresh(cached) if cached == account_snapshot
    ));
    // The empty cookie is rejected locally, so neither launch uses the network.
    for intent in [Intent::Startup, Intent::Guest] {
        let request = state.begin(intent);
        assert!(matches!(
            request.run(&store),
            Screen::Home(HomeView {
                session: ActiveSession::Account {
                    provider: ProviderId::YouTubeMusic,
                    ..
                },
                content: HomeContent::Cached { snapshot: cached, .. },
                ..
            }) if cached == account_snapshot
        ));
        assert!(matches!(
            store.startup().unwrap(),
            Startup::Account(account) if account.profile == session.profile
        ));
    }
    assert_eq!(
        store.cache(CacheScope::Guest).unwrap(),
        Some(guest_snapshot)
    );
}

#[cfg(windows)]
#[test]
fn settings_connections_preserve_the_collection_and_disconnect_atomically() {
    use crate::{
        providers::{HomeSection, SpotifySessionCredentials},
        state::{ActiveSession, HomeContent, Intent, Screen, State},
    };

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let capabilities = Capabilities {
        home: true,
        profile_selection: false,
        device_code: false,
    };
    let spotify = Session {
        provider: ProviderId::Spotify,
        profile: Profile {
            id: "primary".into(),
            name: "Primary".into(),
        },
        credentials: Credentials::Spotify {
            client_id: "test-client".into(),
            access_token: "test-access".into(),
            refresh_token: "test-refresh".into(),
            access_expires_at: None,
            librespot: Some(SpotifySessionCredentials {
                username: "primary".into(),
                auth_data: vec![1],
            }),
        },
        capabilities,
    };
    let youtube = Session {
        provider: ProviderId::YouTubeMusic,
        profile: Profile {
            id: "secondary".into(),
            name: "Secondary".into(),
        },
        credentials: Credentials::YouTubeMusic {
            cookies: String::new(),
            index: 0,
            page_id: None,
        },
        capabilities,
    };
    assert!(store.use_account(&spotify, || Some(()), || {}).unwrap());
    let saved_spotify = store.preferred_account(spotify.provider).unwrap().unwrap();
    let spotify_target = target(spotify.provider, &saved_spotify.credential_ref).unwrap();
    let _spotify_cleanup = TestCredential(spotify_target.clone());
    let spotify_scope = CacheScope::Account(spotify.provider, &spotify.profile.id);
    let youtube_scope = CacheScope::Account(youtube.provider, &youtube.profile.id);
    let collection = Snapshot::received(Home {
        title: "Primary collection".into(),
        items: Vec::new(),
        sections: vec![HomeSection {
            title: "Library".into(),
            start: 0,
            len: 0,
            quick: false,
            ..Default::default()
        }],
    });
    store
        .save_session_cache(spotify_scope, &collection, || Some(()))
        .unwrap();

    let mut state = State::default();
    let startup = state.begin(Intent::Startup);
    let preview = startup.home_preview(&store).unwrap();
    assert!(state.finish(&startup, Screen::Home(preview)));
    let connect = state.begin(Intent::Connect(youtube.provider));
    let connected = connect.finish_login(&store, Ok(youtube.clone()));
    let saved_youtube = store.preferred_account(youtube.provider).unwrap().unwrap();
    let first_target = target(youtube.provider, &saved_youtube.credential_ref).unwrap();
    let _youtube_cleanup = TestCredential(first_target.clone());
    assert!(!state.dismiss_connection());
    assert!(state.finish(&connect, connected));
    let (home, busy) = state.home_view().unwrap();
    assert!(!busy);
    assert!(matches!(
        home.session,
        ActiveSession::Account {
            provider: ProviderId::Spotify,
            ..
        }
    ));
    assert!(matches!(&home.content, HomeContent::Fresh(snapshot) if snapshot == &collection));
    assert_eq!(home.accounts.len(), 2);
    assert!(
        matches!(store.startup().unwrap(), Startup::Account(account) if account == saved_spotify)
    );

    store
        .save_session_cache(youtube_scope, &collection, || Some(()))
        .unwrap();
    assert!(store.cache(youtube_scope).unwrap().is_some());
    assert!(
        store
            .disconnect_account(youtube.provider, &youtube.profile.id, || None::<()>)
            .unwrap()
            .is_none()
    );
    assert!(vault::read(&first_target).unwrap().is_some());
    assert!(store.cache(youtube_scope).unwrap().is_some());

    let logout = state.begin(Intent::Disconnect(youtube.provider));
    assert!(state.finish(&logout, logout.run(&store)));
    let (home, _) = state.home_view().unwrap();
    assert_eq!(home.accounts.len(), 1);
    assert!(matches!(&home.content, HomeContent::Fresh(snapshot) if snapshot == &collection));
    assert!(store.preferred_account(youtube.provider).unwrap().is_none());
    assert!(vault::read(&first_target).unwrap().is_none());
    store
        .save_session_cache(youtube_scope, &collection, || Some(()))
        .unwrap();
    assert!(store.cache(youtube_scope).unwrap().is_none());

    let connect = state.begin(Intent::Connect(youtube.provider));
    let reply = connect.finish_login(&store, Ok(youtube.clone()));
    let saved_youtube = store.preferred_account(youtube.provider).unwrap().unwrap();
    let second_target = target(youtube.provider, &saved_youtube.credential_ref).unwrap();
    let _youtube_cleanup = TestCredential(second_target.clone());
    assert!(state.finish(&connect, reply));
    let logout = state.begin(Intent::Disconnect(spotify.provider));
    assert!(state.finish(&logout, logout.run(&store)));
    let (home, _) = state.home_view().unwrap();
    assert!(matches!(
        home.session,
        ActiveSession::Account {
            provider: ProviderId::YouTubeMusic,
            ..
        }
    ));
    assert!(
        matches!(store.startup().unwrap(), Startup::Account(account) if account == saved_youtube)
    );
    assert!(store.cache(spotify_scope).unwrap().is_none());
    assert!(vault::read(&spotify_target).unwrap().is_none());

    assert!(matches!(
        store
            .disconnect_account(youtube.provider, &youtube.profile.id, || Some(()))
            .unwrap(),
        Some(Startup::Guest)
    ));
    assert!(vault::read(&second_target).unwrap().is_none());
    assert!(matches!(
        Store::open(directory.path()).unwrap().startup().unwrap(),
        Startup::Guest
    ));
}

#[test]
fn reads_home_cache_written_before_artwork_support() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store
        .connect()
        .unwrap()
        .execute(
            "INSERT INTO home_cache (kind, provider, profile_id, received_at_ms, payload)
             VALUES ('guest', 'youtube-music', '', 1, ?1)",
            [r#"{"title":"Guest","items":[{"id":"1","title":"A title","subtitle":"An artist","url":null}]}"#],
        )
        .unwrap();
    let saved = store.cache(CacheScope::Guest).unwrap().unwrap();
    assert_eq!(saved.home.items[0].title, "A title");
    assert!(saved.home.items[0].artwork.is_none());
}

#[test]
fn rejected_route_transition_keeps_last_selection() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    assert!(matches!(store.startup().unwrap(), Startup::Welcome));
    store.use_guest(|| None::<()>).unwrap();
    assert!(matches!(store.startup().unwrap(), Startup::Welcome));
    store.use_guest(|| Some(())).unwrap();
    store.welcome(|| None::<()>).unwrap();
    assert!(matches!(store.startup().unwrap(), Startup::Guest));
    store.welcome(|| Some(())).unwrap();
    assert!(matches!(store.startup().unwrap(), Startup::Welcome));
}

#[test]
fn unknown_database_version_is_left_intact() {
    let directory = tempfile::tempdir().unwrap();
    let connection = Connection::open(directory.path().join(DATABASE)).unwrap();
    let new_version = MIGRATIONS.len() as i64 + 10;
    connection
        .pragma_update(None, "user_version", new_version)
        .unwrap();
    assert!(
        matches!(Store::open(directory.path()), Err(StoreError::NewerVersion(found)) if found == new_version)
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))
            .unwrap(),
        new_version
    );
}

#[cfg(windows)]
#[test]
fn verified_account_and_vault_survive_restart_and_abort_an_uncommitted_rotation() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let provider = ProviderId::SoundCloud;
    let fake_token = format!("storage-test-token-{}", std::process::id());
    let mut session = Session {
        provider,
        profile: Profile {
            id: "profile-42".into(),
            name: "Profile".into(),
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: false,
        },
        credentials: Credentials::SoundCloud {
            token: fake_token.clone(),
            client_id: "storage-test-client".into(),
        },
    };
    assert!(store.use_account(&session, || Some(()), || {}).unwrap());
    let reopened = Store::open(directory.path()).unwrap();
    let Startup::Account(account) = reopened.startup().unwrap() else {
        panic!("Saved account was lost")
    };
    let original = session.credentials.clone();
    let key = target(provider, &account.credential_ref).unwrap();
    let _guard = TestCredential(key);
    assert_eq!(account.profile, session.profile);
    assert_eq!(account.capabilities, session.capabilities);
    assert_saved_credentials(&reopened, &account, &original);
    let sqlite_bytes = fs::read(&store.database).unwrap();
    assert!(
        !sqlite_bytes
            .windows(fake_token.len())
            .any(|window| window == fake_token.as_bytes())
    );

    let Credentials::SoundCloud { token, .. } = &mut session.credentials else {
        unreachable!()
    };
    *token = "storage-test-uncommitted-credential".into();
    assert!(
        !reopened
            .use_account(&session, || None::<()>, || {})
            .unwrap()
    );
    assert_saved_credentials(&reopened, &account, &original);
    let Startup::Account(found) = reopened.startup().unwrap() else {
        panic!("Saved account was lost")
    };
    assert_eq!(found, account);
}

#[cfg(windows)]
#[test]
fn selects_each_saved_source_and_previews_a_recent_spotify_feed() {
    use crate::{
        providers::{HomeItem, HomeSection, SpotifySessionCredentials},
        state::{ActiveSession, HomeContent, HomeView, Intent, State},
    };

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let capabilities = Capabilities {
        home: true,
        profile_selection: false,
        device_code: false,
    };
    let spotify = |profile_id: &str| Session {
        provider: ProviderId::Spotify,
        profile: Profile {
            id: profile_id.into(),
            name: format!("Spotify {profile_id}"),
        },
        credentials: Credentials::Spotify {
            client_id: "storage-test-client".into(),
            access_token: "storage-test-spotify".into(),
            refresh_token: "storage-test-refresh".into(),
            access_expires_at: None,
            librespot: Some(SpotifySessionCredentials {
                username: profile_id.into(),
                auth_data: vec![1],
            }),
        },
        capabilities,
    };
    let soundcloud = Session {
        provider: ProviderId::SoundCloud,
        profile: Profile {
            id: "soundcloud".into(),
            name: "Saved SoundCloud".into(),
        },
        credentials: Credentials::SoundCloud {
            token: "storage-test-token".into(),
            client_id: "storage-test-client".into(),
        },
        capabilities,
    };
    let mut test_credentials = Vec::new();
    for session in [spotify("old"), spotify("active"), soundcloud] {
        assert!(store.use_account(&session, || Some(()), || {}).unwrap());
        let account = store
            .account(session.provider, &session.profile.id)
            .unwrap()
            .unwrap();
        test_credentials.push(TestCredential(
            target(session.provider, &account.credential_ref).unwrap(),
        ));
        assert_saved_credentials(&store, &account, &session.credentials);
    }
    assert!(store.account(ProviderId::Spotify, "old").unwrap().is_none());
    assert_eq!(store.accounts().unwrap().len(), 2);

    let playlist = HomeItem {
        id: "spotify-active".into(),
        title: "Mix for active Spotify".into(),
        subtitle: String::new(),
        url: None,
        artwork: None,
    };
    let spotify_home = Snapshot::received(Home {
        title: "Spotify".into(),
        items: vec![playlist],
        sections: vec![HomeSection {
            title: "Made for You".into(),
            start: 0,
            len: 1,
            quick: false,
            ..Default::default()
        }],
    });
    store
        .save_cache(
            CacheScope::Account(ProviderId::Spotify, "active"),
            &spotify_home,
            || Some(()),
        )
        .unwrap();
    let soundcloud_home = snapshot("SoundCloud", 2);
    store
        .save_cache(
            CacheScope::Account(ProviderId::SoundCloud, "soundcloud"),
            &soundcloud_home,
            || Some(()),
        )
        .unwrap();
    let guest_home = snapshot("Guest", 3);
    store
        .save_cache(CacheScope::Guest, &guest_home, || Some(()))
        .unwrap();
    store.use_guest(|| Some(())).unwrap();
    assert!(matches!(store.startup().unwrap(), Startup::Guest));
    assert!(
        !store
            .select_account(ProviderId::Spotify, "old", || Some(()))
            .unwrap()
    );
    assert!(
        !store
            .select_account(ProviderId::Spotify, "active", || None::<()>)
            .unwrap()
    );
    assert!(matches!(store.startup().unwrap(), Startup::Guest));

    let mut state = State::default();
    let choose = state.begin_account(ProviderId::Spotify, "active".into());
    assert!(choose.home_preview(&store).is_some());
    assert!(
        store
            .select_account(ProviderId::Spotify, "active", || Some(()))
            .unwrap()
    );
    assert_eq!(
        store
            .cache(CacheScope::Account(ProviderId::SoundCloud, "soundcloud"))
            .unwrap(),
        Some(soundcloud_home)
    );
    assert_eq!(store.cache(CacheScope::Guest).unwrap(), Some(guest_home));
    assert!(matches!(
        store.startup().unwrap(),
        Startup::Account(account) if account.provider == ProviderId::Spotify && account.profile.id == "active"
    ));
    let reopened = Store::open(directory.path()).unwrap();
    let restart = state.begin(Intent::Startup);
    assert!(matches!(
        restart.home_preview(&reopened),
        Some(HomeView {
            session: ActiveSession::Account { provider: ProviderId::Spotify, .. },
            content: HomeContent::Fresh(snapshot),
            accounts,
            ..
        }) if snapshot == spotify_home && accounts.len() == 2
    ));
    assert!(
        reopened
            .select_account(ProviderId::SoundCloud, "soundcloud", || Some(()))
            .unwrap()
    );
    assert!(matches!(
        reopened.startup().unwrap(),
        Startup::Account(account) if account.provider == ProviderId::SoundCloud
    ));
}

#[cfg(windows)]
fn assert_saved_credentials(store: &Store, account: &Account, expected: &Credentials) {
    let credential = store
        .credentials(account)
        .unwrap()
        .expect("Persisted credential is missing");
    assert!(&credential == expected, "Persisted credential changed");
}

#[cfg(windows)]
#[test]
fn concurrent_apps_keep_separate_credentials() {
    use std::process::{Command, Stdio};

    let executable = env::current_exe().unwrap();
    let children: Vec<_> = (0..6).map(|_| {
        Command::new(&executable)
            .args(["--exact", "storage::tests::verified_account_and_vault_survive_restart_and_abort_an_uncommitted_rotation"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn().unwrap()
    }).collect();
    for child in children {
        let output = child.wait_with_output().unwrap();
        let summary = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && summary.contains("1 passed; 0 failed"),
            "Concurrent credential verification failed"
        );
    }
}

#[cfg(windows)]
struct TestCredential(String);

#[cfg(windows)]
#[test]
#[ignore = "runs the built Windows application twice against an existing v1 Yandex Music test session"]
fn live_yandex_restores_in_windows_application() {
    use std::{path::Path, process::Command};

    use crate::providers::live_tests::existing_yandex_session;

    let directory = tempfile::tempdir().unwrap();
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let executable = std::env::var_os("MEDIADEKA_TEST_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("target/debug/mediadeka.exe"));
    assert!(
        executable.is_file(),
        "build Mediadeka with cargo build --locked"
    );
    let store = Store::open(directory.path()).unwrap();
    let provider = ProviderId::YandexMusic;
    let session = existing_yandex_session();
    assert!(store.use_account(&session, || Some(()), || {}).unwrap());
    let Startup::Account(account) = store.startup().unwrap() else {
        panic!("Verified Yandex Music account was not saved");
    };
    let _credential = TestCredential(target(provider, &account.credential_ref).unwrap());
    assert_eq!(account.provider, provider);
    let scope = CacheScope::Account(provider, &account.profile.id);
    assert!(store.cache(scope).unwrap().is_none());

    let mut online_snapshot = None;
    for offline in [false, true] {
        let script = manifest.join("scripts/check-welcome.ps1");
        let mut command = Command::new("pwsh.exe");
        command
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(script)
            .arg("-Executable")
            .arg(&executable)
            .arg("-Evidence")
            .arg(directory.path())
            .arg("-RestoredProfile")
            .arg(directory.path())
            .arg("-RestoredAccountSource")
            .arg(provider.display_name());
        if offline {
            let closed_proxy = "http://127.0.0.1:1";
            command
                .arg("-ExpectOfflineCache")
                .env("HTTPS_PROXY", closed_proxy)
                .env("HTTP_PROXY", closed_proxy)
                .env("ALL_PROXY", closed_proxy)
                .env("NO_PROXY", "");
        }
        let output = command.output().expect("launch Windows UI Automation");
        print!("{}", String::from_utf8_lossy(&output.stdout));
        assert!(
            output.status.success(),
            "Windows UIA: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let snapshot = store
            .cache(scope)
            .unwrap()
            .expect("The online launch saved the Yandex Music feed to SQLite");
        if offline {
            assert!(
                online_snapshot.as_ref() == Some(&snapshot),
                "Offline startup changed the saved Yandex Music feed"
            );
        } else {
            assert_yandex_editorial_home(&snapshot.home);
            online_snapshot = Some(snapshot);
        }
    }

    capture_connected_home(
        directory.path(),
        &manifest.join("target/acceptance/yandex-expanded/frames"),
        "yandex",
    );
}

#[cfg(windows)]
fn assert_yandex_editorial_home(home: &Home) {
    use std::collections::HashSet;
    use url::Url;

    let shelves = [
        ("В стиле", true, &["/album/"][..]),
        ("Волна открытий", true, &[][..]),
        ("Новые релизы", true, &["/album/"][..]),
        ("Подборки музыки", false, &["/landing/", "/tag/"][..]),
        ("Вы могли пропустить", false, &["/playlists/"][..]),
        ("Больше новой музыки", false, &["/playlists/"][..]),
    ];
    let artwork = |item: &crate::providers::HomeItem| {
        item.artwork
            .as_deref()
            .and_then(|address| Url::parse(address).ok())
            .is_some_and(|url| {
                url.scheme() == "https"
                    && matches!(
                        url.host_str(),
                        Some("avatars.yandex.net" | "avatars.mds.yandex.net")
                    )
            })
    };
    for (title, filtered, paths) in shelves {
        let shelf = home
            .sections
            .iter()
            .find(|shelf| shelf.title == title)
            .unwrap_or_else(|| panic!("Missing Yandex Music section «{title}»"));
        let end = shelf.start.checked_add(shelf.len).expect("shelf range");
        let items = &home.items[shelf.start..end];
        assert!(!items.is_empty(), "Empty Yandex Music section «{title}»");
        if filtered {
            assert!(shelf.tabs.len() >= 2, "Missing filters in «{title}»");
            for tab in &shelf.tabs {
                assert!(
                    !tab.title.trim().is_empty()
                        && !tab.indices.is_empty()
                        && tab
                            .indices
                            .iter()
                            .all(|index| (shelf.start..end).contains(index)),
                    "Invalid filter in «{title}»"
                );
            }
            assert_ne!(
                shelf.tabs[0].indices, shelf.tabs[1].indices,
                "Filters in «{title}» show the same items"
            );
            if title == "Новые релизы" {
                assert_eq!(shelf.tabs[0].title, "Для вас");
                assert_eq!(shelf.tabs[1].title, "Тренды");
            }
        }
        if paths.is_empty() {
            let top_artwork = shelf.tabs[0]
                .indices
                .iter()
                .take(3)
                .map(|&index| &home.items[index])
                .filter(|item| artwork(item))
                .filter_map(|item| item.artwork.as_ref())
                .collect::<HashSet<_>>();
            assert!(
                top_artwork.len() == 3 && items.iter().all(|item| item.url.is_none()),
                "The top waves need distinct service artwork and no fabricated web destinations"
            );
        } else {
            assert!(
                items.iter().any(|item| {
                    artwork(item)
                        && item
                            .url
                            .as_deref()
                            .and_then(|address| Url::parse(address).ok())
                            .is_some_and(|url| {
                                url.scheme() == "https"
                                    && url.host_str() == Some("music.yandex.ru")
                                    && paths.iter().any(|prefix| url.path().starts_with(*prefix))
                            })
                }),
                "No card with service artwork and a public link in «{title}»"
            );
        }
        println!(
            "Yandex Music «{title}»: {} entries, {} filters",
            shelf.len,
            shelf.tabs.len()
        );
    }
}

#[cfg(windows)]
#[test]
#[ignore = "uses an existing app2 YouTube Music connection to read its live home feed"]
fn live_saved_youtube_home() {
    use std::sync::atomic::AtomicBool;

    let store = Store::open_default().expect("open Mediadeka profile");
    let provider = ProviderId::YouTubeMusic;
    let account = store
        .preferred_account(provider)
        .expect("read saved YouTube Music profile")
        .expect("a YouTube Music profile is connected");
    let credentials = store
        .credentials(&account)
        .expect("read the connection through Mediadeka's credential store")
        .expect("YouTube Music credentials are present");
    let cancelled = AtomicBool::new(false);
    let session = crate::providers::restore(provider, &account.profile, credentials, &cancelled)
        .unwrap_or_else(|issue| panic!("YouTube Music connection check: {issue}"));
    let home = crate::providers::home(&session, &cancelled)
        .unwrap_or_else(|issue| panic!("YouTube Music account feed: {issue}"));
    assert!(home.sections.len() > 1);
    assert!(home.items.iter().any(|item| item.artwork.is_some()));
    println!(
        "YouTube Music signed-in home: {} sections, {} items, quick picks: {}",
        home.sections.len(),
        home.items.len(),
        home.sections.iter().any(|section| section.quick),
    );
    for section in &home.sections {
        let items = &home.items[section.start..section.start + section.len];
        let with_artwork = items.iter().filter(|item| item.artwork.is_some()).count();
        println!("{}: {with_artwork}/{} covers", section.title, items.len());
        if matches!(
            section.title.as_str(),
            "Миксы для вас" | "Новинки и любимые треки" | "Плейлисты Recap"
        ) {
            assert_eq!(with_artwork, items.len(), "{}", section.title);
        }
    }
    if std::env::var_os("MEDIADEKA_TEST_CAPTURE_WINDOW").is_some() {
        let directory = tempfile::tempdir().unwrap();
        let isolated = Store::open(directory.path()).unwrap();
        assert!(isolated.use_account(&session, || Some(()), || {}).unwrap());
        let Startup::Account(account) = isolated.startup().unwrap() else {
            panic!("The isolated YouTube Music connection was not saved");
        };
        let _credential = TestCredential(target(provider, &account.credential_ref).unwrap());
        capture_connected_home(
            directory.path(),
            &Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("target/acceptance/youtube-mix-artwork/frames"),
            "youtube-mixes",
        );
    }
}

#[cfg(windows)]
fn capture_connected_home(profile: &Path, evidence: &Path, mode: &str) {
    let Some(capture) = std::env::var_os("MEDIADEKA_TEST_CAPTURE_WINDOW") else {
        return;
    };
    let status = std::process::Command::new(capture)
        .arg("--home")
        .arg(evidence)
        .arg(mode)
        .arg(profile)
        .status()
        .expect("launch provider home GPUI capture");
    assert!(status.success(), "Provider home GPUI capture failed");
}

#[cfg(windows)]
impl Drop for TestCredential {
    fn drop(&mut self) {
        remove_credential(&self.0);
    }
}
