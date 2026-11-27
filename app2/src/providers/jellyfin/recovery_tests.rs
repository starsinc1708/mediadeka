use std::{env, path::PathBuf};

use crate::{
    providers::{self, LoginInput},
    state::{ActiveSession, ConnectionMaintenance, HomeContent, Intent, Issue, Screen, State},
    storage::{Startup, Store},
};

use super::*;

#[test]
#[ignore = "uses the isolated loopback Jellyfin fixture in scripts/check-jellyfin.ps1"]
fn live_jellyfin_connections_recover_after_restart() {
    let address = env::var("MEDIADEKA_TEST_JELLYFIN_URL").expect("fixture URL");
    let url = server(&address).expect("valid fixture URL");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    assert!(url.port().is_some(), "dedicated fixture port");
    let profile_dir =
        PathBuf::from(env::var_os("MEDIADEKA_PROFILE_DIR").expect("isolated profile"));
    let phase = env::var("MEDIADEKA_TEST_JELLYFIN_RECOVERY_PHASE").expect("recovery phase");
    let user = env::var("MEDIADEKA_TEST_JELLYFIN_USER_ID").expect("fixture user ID");
    let expected_id = format!("{url}#{user}");

    for method in ["password", "quick-connect"] {
        let store = Store::open(profile_dir.join(method)).expect("open isolated profile");
        let mut state = State::default();
        if phase == "cleanup" {
            if let Startup::Account(account) = store.startup().expect("read saved route") {
                assert_eq!(account.profile.id, expected_id);
                store
                    .disconnect_account(PROVIDER, &expected_id, || Some(()))
                    .expect("delete isolated account credentials");
            }
            continue;
        }
        if matches!(phase.as_str(), "sign-in" | "sign-in-again") {
            let request = state.begin(Intent::SignIn(PROVIDER));
            let input = if method == "password" {
                LoginInput::JellyfinPassword {
                    server: address.clone(),
                    username: env::var("MEDIADEKA_TEST_JELLYFIN_USER").expect("fixture user"),
                    password: env::var("MEDIADEKA_TEST_JELLYFIN_PASSWORD")
                        .expect("fixture password"),
                }
            } else {
                LoginInput::JellyfinQuickConnect {
                    server: address.clone(),
                }
            };
            let session = providers::sign_in(PROVIDER, input, request.cancel_flag(), |prompt| {
                let LoginPrompt::Code { code, .. } = prompt else {
                    panic!("expected Quick Connect code");
                };
                let authorization = env::var("MEDIADEKA_TEST_JELLYFIN_AUTHORIZATION")
                    .expect("fixture approver authorization");
                let response = agent()
                    .post(at(&url, "QuickConnect/Authorize").as_str())
                    .header("Authorization", authorization)
                    .query("code", code)
                    .send_empty()
                    .expect("authorize Quick Connect on the fixture");
                assert!(response.status().is_success(), "HTTP {}", response.status());
            });
            let session = session.unwrap_or_else(|error| panic!("{method} login: {error}"));
            assert_eq!(session.profile.id, expected_id);
            check_home(
                request.finish_login(&store, Ok(session)),
                &expected_id,
                None,
            );
        } else {
            let Startup::Account(account) = store.startup().expect("read saved route") else {
                panic!("{method}: saved profile missing at {phase}");
            };
            assert_eq!(account.profile.id, expected_id);
            let credentials = store
                .credentials(&account)
                .unwrap()
                .expect("saved credentials");
            let (intent, expected_error) = match phase.as_str() {
                "online" => (Intent::Startup, None),
                "offline" => (Intent::Startup, Some(ErrorKind::Network)),
                "recovered" => (Intent::Refresh, None),
                "invalid" => {
                    logout(&url, &credentials);
                    (Intent::Startup, Some(ErrorKind::Expired))
                }
                _ => panic!("unknown recovery phase"),
            };
            let checks = ConnectionMaintenance::default()
                .poll(&store, &AtomicBool::new(false))
                .expect("run background connection check");
            if expected_error == Some(ErrorKind::Network) {
                assert!(checks.is_empty(), "a temporary outage schedules a retry");
            } else {
                assert_eq!(checks.len(), 1);
                assert_eq!(checks[0].account.profile.id, expected_id);
                assert_eq!(
                    checks[0].problem.as_ref().map(|problem| problem.kind),
                    expected_error
                );
            }
            check_home(
                state.begin(intent).run(&store),
                &expected_id,
                expected_error,
            );
            assert!(store.credentials(&account).unwrap() == Some(credentials));
        }
        println!("Jellyfin {method}: {phase} verified");
    }
}

fn logout(url: &Url, credentials: &Credentials) {
    let Credentials::Jellyfin {
        device,
        access_token,
        ..
    } = credentials
    else {
        panic!("expected Jellyfin credentials");
    };
    let response = agent()
        .post(at(url, "Sessions/Logout").as_str())
        .header("Authorization", auth(device, Some(access_token)).unwrap())
        .send_empty()
        .expect("log isolated device out on the server");
    assert!(response.status().is_success(), "HTTP {}", response.status());
}

fn check_home(screen: Screen, expected_id: &str, expected_error: Option<ErrorKind>) {
    let Screen::Home(view) = screen else {
        panic!("expected musical shell, got {screen:?}");
    };
    let ActiveSession::Account {
        provider, profile, ..
    } = view.session
    else {
        panic!("saved Jellyfin profile was replaced by the guest route");
    };
    assert_eq!(provider, PROVIDER);
    assert_eq!(profile.id, expected_id);
    let snapshot = match (view.content, expected_error) {
        (HomeContent::Fresh(snapshot), None) => snapshot,
        (
            HomeContent::Cached {
                snapshot,
                issue: Issue::Provider(error),
            },
            Some(kind),
        ) => {
            assert_eq!(error.kind, kind);
            snapshot
        }
        (content, expected) => panic!("unexpected home content for {expected:?}: {content:?}"),
    };
    assert!(
        snapshot
            .home
            .items
            .iter()
            .any(|item| item.title == "Mediadeka Jellyfin Acceptance")
    );
}
