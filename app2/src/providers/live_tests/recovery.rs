use std::{
    path::Path,
    process::Command,
    sync::atomic::AtomicBool,
    thread,
    time::{Duration, Instant},
};

use crate::{
    providers::{Credentials, ErrorKind, ProviderId, Session},
    state::{
        ActiveSession, ConnectionMaintenance, HomeContent, HomeView, Intent, Issue, Screen, State,
    },
    storage::{CacheScope, Store},
};

use super::profile_snapshot;

const PROFILE: &str = "MEDIADEKA_RECOVERY_SOURCE_PROFILE";
const PROVIDER: &str = "MEDIADEKA_RECOVERY_PROVIDER";
const MODE: &str = "MEDIADEKA_RECOVERY_MODE";
const COPY: &str = "MEDIADEKA_RECOVERY_COPY";
const TEST: &str = "providers::live_tests::recovery::live_saved_music_connection_recovery";

#[test]
#[ignore = "requires a saved app2 test profile and connects to the selected music provider"]
fn live_saved_music_connection_recovery() {
    let name = std::env::var(PROVIDER).expect("test provider");
    let provider = ProviderId::ALL
        .into_iter()
        .find(|provider| provider.as_str() == name)
        .unwrap();
    if let Ok(mode) = std::env::var(MODE) {
        let directory = std::env::var_os(COPY).expect("isolated test profile");
        check_route(&Store::open(directory).unwrap(), provider, &mode);
        return;
    }

    let source_directory = profile_snapshot(PROFILE);
    let source = Store::open(source_directory.path()).unwrap();
    let account = source
        .preferred_account(provider)
        .unwrap()
        .expect("saved provider");
    let mut credentials = source.credentials(&account).unwrap().expect("saved access");
    match &credentials {
        Credentials::Spotify {
            refresh_token,
            access_expires_at,
            ..
        } => println!(
            "saved Spotify has_refresh={} has_expiry={}",
            !refresh_token.is_empty(),
            access_expires_at.is_some()
        ),
        Credentials::YandexMusic {
            refresh_token,
            access_expires_at,
            ..
        } => println!(
            "saved Yandex has_refresh={} has_expiry={}",
            refresh_token.is_some(),
            access_expires_at.is_some()
        ),
        _ => {}
    }
    // A copied OAuth refresh token may still rotate the authorization held by the original profile.
    match &mut credentials {
        Credentials::Spotify { refresh_token, .. } => refresh_token.clear(),
        Credentials::YandexMusic { refresh_token, .. } => *refresh_token = None,
        _ => {}
    }
    let session = Session {
        provider,
        profile: account.profile.clone(),
        capabilities: account.capabilities,
        credentials,
    };
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    assert!(store.use_account(&session, || Some(()), || {}).unwrap());
    let scope = CacheScope::Account(provider, &account.profile.id);
    if let Some(snapshot) = source.cache(scope).unwrap() {
        store
            .save_session_cache(scope, &snapshot, || Some(()))
            .unwrap();
    }

    let results = ["startup", "offline", "retry"].map(|mode| run_process(directory.path(), mode));
    store
        .disconnect_account(provider, &account.profile.id, || Some(()))
        .unwrap();
    assert!(results.into_iter().all(|success| success));
}

fn run_process(directory: &Path, mode: &str) -> bool {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", TEST, "--ignored", "--nocapture"])
        .env(COPY, directory)
        .env(MODE, mode);
    if mode == "offline" {
        // The loopback deny proxy fails immediately and leaves the real network accessible to the parent.
        command
            .env("HTTP_PROXY", "http://127.0.0.1:1")
            .env("HTTPS_PROXY", "http://127.0.0.1:1")
            .env("ALL_PROXY", "http://127.0.0.1:1")
            .env("MEDIADEKA_TEST_SPOTIFY_PROXY", "http://127.0.0.1:1")
            .env("NO_PROXY", "");
    }
    command
        .status()
        .expect("start separate Mediadeka test process")
        .success()
}

fn check_route(store: &Store, provider: ProviderId, mode: &str) {
    let account = store.preferred_account(provider).unwrap().unwrap();
    let scope = CacheScope::Account(provider, &account.profile.id);
    let before = store.cache(scope).unwrap();
    if mode == "retry" && provider == ProviderId::SoundCloud {
        let mut maintenance = ConnectionMaintenance::default();
        let cancelled = AtomicBool::new(false);
        let deadline = Instant::now() + Duration::from_secs(85);
        loop {
            let checks = maintenance.poll(store, &cancelled).unwrap();
            if let Some(check) = checks
                .iter()
                .find(|check| check.account.profile.id == account.profile.id)
            {
                assert!(
                    check.problem.is_none(),
                    "saved SoundCloud profile requires sign-in"
                );
                println!("SoundCloud background check restored the saved profile");
                return;
            }
            assert!(
                Instant::now() < deadline,
                "SoundCloud did not recover within the background retry window"
            );
            thread::sleep(Duration::from_secs(2));
        }
    }
    let intent = if mode == "retry" {
        Intent::Refresh
    } else {
        Intent::Startup
    };
    let result = State::default().begin(intent).run(store);
    let Screen::Home(HomeView {
        session:
            ActiveSession::Account {
                provider: restored,
                profile,
                ..
            },
        content,
        ..
    }) = result
    else {
        panic!("saved route must open a music home: {result:?}");
    };
    assert_eq!(restored, provider);
    assert_eq!(profile.id, account.profile.id);
    let (status, items) = match content {
        HomeContent::Fresh(snapshot) => ("fresh", snapshot.home.items.len()),
        HomeContent::Cached { snapshot, issue } => {
            assert_eq!(Some(&snapshot), before.as_ref());
            (observed_issue(issue), snapshot.home.items.len())
        }
        HomeContent::Partial { snapshot, issue } => {
            (observed_issue(issue), snapshot.home.items.len())
        }
        HomeContent::Empty(issue) => (observed_issue(issue), 0),
        HomeContent::Loading => panic!("restore did not finish"),
    };
    println!(
        "provider={} mode={mode} state={status} items={items}",
        provider.as_str()
    );
    if mode == "offline" {
        assert_eq!(status, "temporary");
        assert_eq!(store.cache(scope).unwrap(), before);
    } else if mode == "startup" && status == "temporary" {
        assert!(
            before.is_some() && items > 0,
            "an initial outage must preserve the saved feed"
        );
    } else {
        assert!(
            status == "fresh"
                || (provider == ProviderId::YouTubeMusic && status == "needs-sign-in")
        );
    }
}

fn observed_issue(issue: Issue) -> &'static str {
    println!("recovery observation: {issue}");
    match issue {
        Issue::Provider(error) if error.needs_sign_in() => "needs-sign-in",
        Issue::Provider(error)
            if matches!(error.kind, ErrorKind::Network | ErrorKind::Unavailable) =>
        {
            "temporary"
        }
        _ => panic!("unexpected restoration issue: {issue}"),
    }
}
