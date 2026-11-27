use super::*;
use crate::providers::{Credentials, Home, HomeItem, HomeSection, SpotifySessionCredentials};

const LIVE_GUEST_PROFILE: &str = "MEDIADEKA_TEST_LIVE_GUEST_PROFILE";

#[test]
fn superseded_reply_cannot_replace_another_pending_provider() {
    let mut state = State::default();
    let spotify = state.begin(Intent::SignIn(ProviderId::Spotify));
    let jellyfin = state.begin(Intent::SignIn(ProviderId::Jellyfin));
    assert!(spotify.is_cancelled());
    assert!(!state.accepts(&spotify));
    assert!(!state.finish(&spotify, Screen::Welcome));
    assert!(matches!(
        state.screen(),
        Screen::Pending(Intent::SignIn(ProviderId::Jellyfin))
    ));
    assert!(state.finish(&jellyfin, Screen::Welcome));
    assert!(!state.finish(&jellyfin, Screen::Pending(Intent::Guest)));
    assert!(matches!(state.screen(), Screen::Welcome));
}

#[test]
fn cancelled_attempt_cannot_update_screen_or_startup_route() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let mut state = State::default();
    let request = state.begin(Intent::Guest);
    state.cancel();
    let reply = request.run(&store);
    assert!(matches!(reply, Screen::Cancelled(Intent::Guest)));
    assert!(!state.finish(&request, reply));
    assert!(matches!(state.screen(), Screen::Cancelled(Intent::Guest)));
    assert!(matches!(store.startup().unwrap(), Startup::Welcome));
}

#[test]
fn startup_preview_presents_a_saved_guest_snapshot_before_any_fetch() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let mut state = State::default();
    let startup = state.begin(Intent::Startup);
    assert!(startup.home_preview(&store).is_none());
    assert!(store.use_guest(|| Some(())).unwrap());
    let loading = startup.home_preview(&store).unwrap();
    assert!(matches!(loading.session, ActiveSession::Guest));
    assert!(matches!(loading.content, HomeContent::Loading));
    assert!(state.set_home_preview(&startup, loading));
    assert!(state.home_view().is_some_and(|(_, busy)| busy));
    state.cancel();
    assert!(state.home_view().is_none());

    let snapshot = Snapshot {
        home: Home {
            title: "Previously loaded".into(),
            items: Vec::new(),
            sections: Vec::new(),
        },
        received_at_ms: 1,
    };
    store
        .save_cache(CacheScope::Guest, &snapshot, || Some(()))
        .unwrap();
    let startup = state.begin(Intent::Startup);
    let preview = startup.home_preview(&store).unwrap();
    assert!(state.set_home_preview(&startup, preview));
    assert!(matches!(
        state.home_view(),
        Some((HomeView { content: HomeContent::Fresh(cached), .. }, true)) if cached == &snapshot
    ));

    let late = startup.home_preview(&store).unwrap();
    state.cancel();
    assert!(!state.set_home_preview(&startup, late));
    assert!(matches!(
        state.home_view(),
        Some((HomeView { content: HomeContent::Fresh(cached), .. }, false)) if cached == &snapshot
    ));
}

#[test]
fn adding_or_switching_accounts_preserves_the_current_route_until_success() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    store.use_guest(|| Some(())).unwrap();
    let mut state = State::default();
    let guest = state.begin(Intent::Guest);
    let home = guest.guest_home(
        &store,
        Ok(Home {
            title: "Existing guest collection".into(),
            items: Vec::new(),
            sections: Vec::new(),
        }),
    );
    assert!(state.finish(&guest, home));

    let intent = Intent::Connect(ProviderId::Spotify);
    let add = state.begin(intent);
    assert!(matches!(state.screen(), Screen::Pending(candidate) if *candidate == intent));
    assert!(matches!(state.home_view(), Some((_, false))));
    let failed = add.finish_login(
        &store,
        Err(ProviderError {
            kind: ErrorKind::Network,
            message: "Test connection failure".into(),
        }),
    );
    assert!(state.finish(&add, failed));
    assert!(matches!(
        state.screen(),
        Screen::Failed(Failure { intent: candidate, .. }) if *candidate == intent
    ));
    assert!(matches!(state.home_view(), Some((_, false))));

    let add = state.begin(intent);
    assert!(state.dismiss_connection());
    assert!(add.is_cancelled());
    assert!(!state.finish(&add, Screen::Welcome));
    assert!(matches!(state.screen(), Screen::Home(_)));

    let switch = state.begin_account(ProviderId::Spotify, "missing".into());
    assert!(state.home_view().is_some_and(|(_, pending)| pending));
    let failed = switch.run(&store);
    assert!(state.finish(&switch, failed));
    let (home, pending) = state.home_view().unwrap();
    assert!(!pending);
    assert!(matches!(home.session, ActiveSession::Guest));
    assert!(home.switch_issue.is_some());
    assert!(matches!(
        &home.content,
        HomeContent::Fresh(snapshot) if snapshot.home.title == "Existing guest collection"
    ));
    assert!(matches!(store.startup().unwrap(), Startup::Guest));
}

#[cfg(windows)]
#[test]
fn source_switch_previews_the_matching_snapshot_and_retires_it_on_a_new_choice() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let profile = Profile {
        id: "shared-id".into(),
        name: "Test profile".into(),
    };
    let session = |credentials: Credentials| Session {
        provider: credentials.provider(),
        profile: profile.clone(),
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: false,
        },
        credentials,
    };
    let jellyfin = session(Credentials::Jellyfin {
        server: "http://127.0.0.1:1".into(),
        device: "preview-test".into(),
        access_token: "fixture".into(),
    });
    let soundcloud = session(Credentials::SoundCloud {
        token: "fixture".into(),
        client_id: "fixture".into(),
    });
    for session in [&jellyfin, &soundcloud] {
        assert!(store.use_account(session, || Some(()), || {}).unwrap());
    }
    assert!(store.use_guest(|| Some(())).unwrap());
    let saved = Snapshot {
        home: Home {
            title: "Saved Jellyfin".into(),
            items: Vec::new(),
            sections: Vec::new(),
        },
        received_at_ms: 1,
    };
    store
        .save_cache(
            CacheScope::Account(jellyfin.provider, &profile.id),
            &saved,
            || Some(()),
        )
        .unwrap();
    store
        .save_cache(
            CacheScope::Account(soundcloud.provider, "different-profile"),
            &saved,
            || Some(()),
        )
        .unwrap();

    let mut state = State::default();
    let startup = state.begin(Intent::Startup);
    let home = startup.home_preview(&store).unwrap();
    assert!(state.finish(&startup, Screen::Home(home)));

    let first = state.begin_account(jellyfin.provider, profile.id.clone());
    let preview = first.home_preview(&store).unwrap();
    assert!(state.set_home_preview(&first, preview));
    assert!(matches!(state.home_view(), Some((HomeView {
        session: ActiveSession::Account { provider: ProviderId::Jellyfin, .. },
        content: HomeContent::Fresh(snapshot), ..
    }, true)) if snapshot == &saved));
    assert!(matches!(store.startup().unwrap(), Startup::Guest));
    let late_preview = first.home_preview(&store).unwrap();

    let second = state.begin_account(soundcloud.provider, profile.id.clone());
    let preview = second.home_preview(&store).unwrap();
    assert!(state.set_home_preview(&second, preview));
    assert!(!state.set_home_preview(&first, late_preview));
    assert!(!state.finish(&first, Screen::Welcome));
    assert!(matches!(
        state.home_view(),
        Some((
            HomeView {
                session: ActiveSession::Account {
                    provider: ProviderId::SoundCloud,
                    ..
                },
                content: HomeContent::Loading,
                ..
            },
            true
        ))
    ));

    let rejected = state.begin_account(jellyfin.provider, profile.id.clone());
    let preview = rejected.home_preview(&store).unwrap();
    assert!(matches!(preview.content, HomeContent::Fresh(_)));
    assert!(state.set_home_preview(&rejected, preview));
    let failure = rejected
        .restore_error(
            &store,
            ActiveSession::from(&rejected.requested_account(&store).unwrap()),
            ProviderError {
                kind: ErrorKind::InvalidResponse,
                message: "Invalid fixture response".into(),
            },
        )
        .unwrap_err();
    let failed = rejected.failed(failure);
    assert!(!state.keeps_cached_preview(&failed));
    assert!(state.finish(&rejected, failed));
    assert!(matches!(
        state.home_view(),
        Some((
            HomeView {
                session: ActiveSession::Guest,
                switch_issue: Some(_),
                ..
            },
            false
        ))
    ));
    assert!(matches!(store.startup().unwrap(), Startup::Guest));
    for session in [&jellyfin, &soundcloud] {
        store
            .disconnect_account(session.provider, &profile.id, || Some(()))
            .unwrap();
    }
}

#[test]
fn superseding_a_prepared_write_rolls_back_the_transaction() {
    use std::{sync::mpsc::sync_channel, thread, time::Duration};

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let mut state = State::default();
    let request = state.begin(Intent::Guest);
    let (ready, started) = sync_channel(1);
    let (commit, resume) = sync_channel(1);
    thread::scope(|tasks| {
        let store_for_writer = &store;
        let request_for_writer = &request;
        let writer = tasks.spawn(move || {
            store_for_writer
                .use_guest(|| {
                    ready.send(()).unwrap();
                    resume.recv_timeout(Duration::from_secs(3)).unwrap();
                    request_for_writer.commit_permit()
                })
                .unwrap();
        });
        started.recv_timeout(Duration::from_secs(3)).unwrap();
        let replacement = state.begin(Intent::SignIn(ProviderId::Jellyfin));
        commit.send(()).unwrap();
        writer.join().unwrap();
        assert!(state.accepts(&replacement));
    });
    assert!(matches!(store.startup().unwrap(), Startup::Welcome));
}

#[test]
fn temporary_failures_retain_list_without_crossing_a_scope_or_overwriting_it() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let mut state = State::default();
    let request = state.begin(Intent::Guest);
    let home = Home {
        title: "Guest".into(),
        items: Vec::new(),
        sections: Vec::new(),
    };
    let first = request.guest_home(&store, Ok(home.clone()));
    let Screen::Home(HomeView {
        content: HomeContent::Fresh(snapshot),
        ..
    }) = first
    else {
        panic!("Expected live guest content: {first:?}");
    };
    assert_eq!(snapshot.home, home);
    assert!(snapshot.received_at_ms > 0);

    let offline = || ProviderError {
        kind: ErrorKind::Network,
        message: "Offline".into(),
    };
    let cached = request.guest_home(&store, Err(offline()));
    let Screen::Home(HomeView {
        content: HomeContent::Cached {
            snapshot: saved, ..
        },
        ..
    }) = cached
    else {
        panic!("Expected cache fallback: {cached:?}");
    };
    assert_eq!(snapshot, saved);
    let empty = request.guest_home(
        &store,
        Err(ProviderError {
            kind: ErrorKind::Expired,
            message: "Expired".into(),
        }),
    );
    assert!(matches!(
        empty,
        Screen::Home(HomeView {
            content: HomeContent::Empty(_),
            ..
        })
    ));
    assert!(
        store
            .cache(CacheScope::Account(ProviderId::YouTubeMusic, "profile"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn confirmed_spotify_session_opens_home_and_uses_only_its_own_cache_after_429() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let mut state = State::default();
    let provider = ProviderId::Spotify;
    let profile = Profile {
        id: "confirmed-spotify".into(),
        name: "Confirmed profile".into(),
    };
    let session = Session {
        provider,
        profile: profile.clone(),
        credentials: Credentials::Spotify {
            client_id: "test-client".into(),
            access_token: "test-access".into(),
            refresh_token: "test-refresh".into(),
            access_expires_at: None,
            librespot: Some(SpotifySessionCredentials {
                username: profile.id.clone(),
                auth_data: vec![1, 2, 3],
            }),
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: false,
        },
    };
    let make_snapshot = |title: &str| {
        Snapshot::received(Home {
            title: title.into(),
            items: vec![HomeItem {
                id: title.into(),
                title: title.into(),
                subtitle: "Test artist".into(),
                url: None,
                artwork: None,
            }],
            sections: vec![HomeSection {
                title: "Made for You".into(),
                start: 0,
                len: 1,
                quick: false,
                ..Default::default()
            }],
        })
    };
    let unrelated = [
        (
            CacheScope::Account(provider, "other-spotify"),
            make_snapshot("Other account"),
        ),
        (
            CacheScope::Account(ProviderId::Jellyfin, &profile.id),
            make_snapshot("Other provider"),
        ),
    ];
    for (scope, snapshot) in &unrelated {
        store.save_cache(*scope, snapshot, || Some(())).unwrap();
    }
    let rate_limit = ProviderError {
        kind: ErrorKind::Network,
        message: "Spotify вернул HTTP 429.".into(),
    };

    let request = state.begin(Intent::SignIn(provider));
    let screen = request.account_home(&store, session.clone(), Err(rate_limit.clone()));
    assert!(state.finish(&request, screen));
    assert!(matches!(
        state.screen(),
        Screen::Home(HomeView {
            session: ActiveSession::Account { provider: ProviderId::Spotify, profile: active, capabilities },
            content: HomeContent::Empty(Issue::Provider(issue)),
            ..
        }) if active == &profile && capabilities.home && issue == &rate_limit
    ));
    let own_scope = CacheScope::Account(provider, &profile.id);
    assert!(store.cache(own_scope).unwrap().is_none());

    let mut legacy = make_snapshot("Legacy saved tracks");
    legacy.home.sections.clear();
    store.save_cache(own_scope, &legacy, || Some(())).unwrap();
    let request = state.begin(Intent::Refresh);
    let screen = request.account_home(&store, session.clone(), Err(rate_limit.clone()));
    assert!(state.finish(&request, screen));
    assert!(matches!(
        state.screen(),
        Screen::Home(HomeView {
            content: HomeContent::Empty(_),
            ..
        })
    ));

    let saved = make_snapshot("Matching Spotify profile");
    store.save_cache(own_scope, &saved, || Some(())).unwrap();
    let request = state.begin(Intent::Refresh);
    let screen = request.account_home(&store, session, Err(rate_limit.clone()));
    assert!(state.finish(&request, screen));
    assert!(matches!(
        state.screen(),
        Screen::Home(HomeView {
            session: ActiveSession::Account { provider: ProviderId::Spotify, profile: active, .. },
            content: HomeContent::Cached { snapshot, issue: Issue::Provider(issue) },
            ..
        }) if active == &profile && snapshot == &saved && issue == &rate_limit
    ));
    assert_eq!(store.cache(own_scope).unwrap(), Some(saved));
    for (scope, snapshot) in unrelated {
        assert_eq!(store.cache(scope).unwrap(), Some(snapshot));
    }
}

#[cfg(windows)]
#[test]
fn soundcloud_partial_refresh_preserves_activity_until_successful_empty_response() {
    use crate::providers::load_soundcloud_home_for_test;
    use serde_json::json;

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let provider = ProviderId::SoundCloud;
    let id = "123";
    let session = Session {
        provider,
        profile: Profile {
            id: id.into(),
            name: "Test user".into(),
        },
        credentials: Credentials::SoundCloud {
            token: "test-token".into(),
            client_id: "test-client".into(),
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: false,
        },
    };
    assert!(store.use_account(&session, || Some(()), || {}).unwrap());
    struct Disconnect<'a>(&'a Store, ProviderId, &'a str);
    impl Drop for Disconnect<'_> {
        fn drop(&mut self) {
            let _ = self.0.disconnect_account(self.1, self.2, || Some(()));
        }
    }
    let _disconnect = Disconnect(&store, provider, id);
    let scope = CacheScope::Account(provider, id);
    let mut state = State::default();
    let mut refresh = |discover: &str, activity: Result<Option<&str>, ErrorKind>| {
        let request = state.begin(Intent::Refresh);
        let track = |title: &str| json!({"kind":"track", "id":1, "title":title});
        let update = load_soundcloud_home_for_test(id, request.cancel_flag(), |path| {
            match path.split('?').next().unwrap() {
                "/stream" => match activity {
                    Ok(Some(title)) => {
                        Ok(json!({"collection":[{"type":"track", "track":track(title)}]}))
                    }
                    Ok(None) => Ok(json!({"collection":[]})),
                    Err(kind) => Err(ProviderError {
                        kind,
                        message: "Activity fixture".into(),
                    }),
                },
                "/mixed-selections" => Ok(json!({"collection":[
                    {"title":"Discover", "items":{"collection":[track(discover)]}}
                ]})),
                _ => Ok(json!({"collection":[]})),
            }
        })
        .unwrap();
        let reply = request.account_home(
            &store,
            session.clone(),
            Ok(providers::HomeUpdate::SoundCloud(Box::new(update))),
        );
        assert!(state.finish(&request, reply));
        let content = &state.home_view().unwrap().0.content;
        let (snapshot, kind) = match content {
            HomeContent::Fresh(snapshot) => (snapshot, None),
            HomeContent::Partial {
                snapshot,
                issue: Issue::Provider(issue),
            } => (snapshot, Some(issue.kind)),
            _ => panic!("Unexpected home state: {content:?}"),
        };
        let titles: Vec<_> = snapshot
            .home
            .items
            .iter()
            .map(|item| item.title.clone())
            .collect();
        (titles, kind)
    };

    assert_eq!(
        refresh("Initial", Ok(Some("Saved activity"))),
        (
            vec!["Initial".to_owned(), "Saved activity".to_owned()],
            None
        )
    );
    assert_eq!(
        refresh("Fresh", Err(ErrorKind::Network)),
        (
            vec!["Fresh".to_owned(), "Saved activity".to_owned()],
            Some(ErrorKind::Network)
        )
    );
    let merged = Store::open(directory.path())
        .unwrap()
        .cache(scope)
        .unwrap()
        .unwrap();
    assert_eq!(merged.home.items[0].title, "Fresh");
    assert_eq!(merged.home.items[1].title, "Saved activity");

    assert_eq!(
        refresh("Allowed", Err(ErrorKind::Rejected)),
        (vec!["Allowed".to_owned()], Some(ErrorKind::Rejected))
    );
    assert_eq!(store.cache(scope).unwrap(), Some(merged));
    assert_eq!(
        refresh("Recovered", Ok(None)),
        (vec!["Recovered".to_owned()], None)
    );
    assert_eq!(store.cache(scope).unwrap().unwrap().home.items.len(), 1);
}

#[test]
#[ignore = "requires live YouTube Music and creates a disposable local profile"]
fn live_guest_home_has_public_items_and_restores_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let mut state = State::default();
    for intent in [Intent::Guest, Intent::Startup] {
        let request = state.begin(intent);
        let result = request.run(&Store::open(directory.path()).unwrap());
        let Screen::Home(HomeView {
            session: ActiveSession::Guest,
            content: HomeContent::Fresh(snapshot),
            ..
        }) = &result
        else {
            panic!("Live guest result for {intent:?}: {result:?}");
        };
        assert!(!snapshot.home.items.is_empty());
        let stored = store.cache(CacheScope::Guest).unwrap().unwrap();
        assert_eq!(&stored, snapshot);
        println!(
            "{intent:?}: {} public items · cache timestamp {}",
            snapshot.home.items.len(),
            snapshot.received_at_ms
        );
        assert!(state.finish(&request, result));
    }
    assert!(matches!(store.startup().unwrap(), Startup::Guest));

    let closed_proxy = "http://127.0.0.1:1";
    let offline = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "state::tests::guest_startup_uses_saved_snapshot_on_real_network_failure",
            "--nocapture",
        ])
        .env(LIVE_GUEST_PROFILE, directory.path())
        .env("HTTPS_PROXY", closed_proxy)
        .env("HTTP_PROXY", closed_proxy)
        .env("ALL_PROXY", closed_proxy)
        .env("NO_PROXY", "")
        .output()
        .unwrap();
    print!("{}", String::from_utf8_lossy(&offline.stdout));
    assert!(
        offline.status.success(),
        "{}",
        String::from_utf8_lossy(&offline.stderr)
    );
    assert!(String::from_utf8_lossy(&offline.stdout).contains("1 passed"));
}

#[test]
#[ignore = "configure the process HTTP proxy to a closed endpoint to exercise a real network refusal"]
fn guest_startup_uses_saved_snapshot_on_real_network_failure() {
    let directory = tempfile::tempdir().unwrap();
    let shared_profile = std::env::var_os(LIVE_GUEST_PROFILE);
    let store = if let Some(path) = shared_profile {
        let store = Store::open(path).unwrap();
        assert!(
            !store
                .cache(CacheScope::Guest)
                .unwrap()
                .unwrap()
                .home
                .items
                .is_empty()
        );
        store
    } else {
        let store = Store::open(directory.path().join("seeded")).unwrap();
        let seeded = Snapshot::received(Home {
            title: "Saved guest snapshot".into(),
            items: Vec::new(),
            sections: Vec::new(),
        });
        store.use_guest(|| Some(())).unwrap();
        store
            .save_cache(CacheScope::Guest, &seeded, || Some(()))
            .unwrap();
        store
    };
    let expected = store.cache(CacheScope::Guest).unwrap().unwrap();
    let mut state = State::default();
    let request = state.begin(Intent::Startup);
    let result = request.run(&store);
    let Screen::Home(HomeView {
        session: ActiveSession::Guest,
        content: HomeContent::Cached { snapshot, issue },
        ..
    }) = &result
    else {
        panic!("Guest result with a closed network proxy: {result:?}");
    };
    assert_eq!(snapshot, &expected);
    println!(
        "Offline process restored {} public items from cache timestamp {}; provider issue {issue}",
        snapshot.home.items.len(),
        snapshot.received_at_ms
    );
    assert!(state.finish(&request, result));

    let cold_store = Store::open(directory.path().join("clean")).unwrap();
    let request = state.begin(Intent::Guest);
    let result = request.run(&cold_store);
    assert!(matches!(
        result,
        Screen::Home(HomeView {
            session: ActiveSession::Guest,
            content: HomeContent::Empty(Issue::Provider(ProviderError {
                kind: ErrorKind::Network,
                ..
            })),
            ..
        })
    ));
    assert!(cold_store.cache(CacheScope::Guest).unwrap().is_none());
    assert!(state.finish(&request, result));
}
