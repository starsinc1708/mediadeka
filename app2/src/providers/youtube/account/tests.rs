use super::*;

fn identity(name: &str, handle: &str, page_id: Option<&str>) -> Identity {
    Identity::read(&json!({
        "accountName": {"simpleText":name},
        "channelHandle": {"simpleText":handle},
        "email": {"simpleText":"shared@example.invalid"},
        "serviceEndpoint": {"pageIdToken":{"pageId":page_id}},
    }))
    .unwrap()
}

#[test]
fn rejects_unsigned_or_multiline_session_headers() {
    assert!(normalize_cookies("PREF=abc").is_err());
    assert!(normalize_cookies("SAPISID=abc\r\nInjection: foo").is_err());
    assert_eq!(
        normalize_cookies("cookie:  __Secure-3PAPISID=abc; PREF=1").unwrap(),
        "__Secure-3PAPISID=abc; PREF=1"
    );
}

#[test]
fn distinguishes_channels_that_have_the_same_name_and_google_email() {
    let selected = identity("Music", "@channel", Some("brand1"));
    let current = identity("Music", "@another", None);
    assert!(!current.matches_identity(&selected));
    assert!(!current.matches_saved("shared@example.invalid", None));
    let correct = identity("Updated name", "@channel", None);
    assert!(correct.matches_identity(&selected));
}

#[test]
fn keeps_the_same_brand_channel_when_its_handle_changes() {
    let moved = identity("Music", "@new", Some("brand1"));
    assert!(moved.matches_saved("brand1", Some("brand1")));
    assert!(!moved.matches_saved("@new", Some("another-brand")));
    assert!(!moved.matches_saved("@old", None));
    assert!(!moved.matches_saved("@old", Some("brand1")));
}

#[test]
fn logged_out_marker_is_distinct_from_an_unfamiliar_menu() {
    assert!(!signed_out(&json!({"actions": []})));
    assert!(!signed_out(
        &json!({"responseContext":{"mainAppWebResponseContext":{"loggedOut": false}}})
    ));
    assert!(signed_out(
        &json!({"responseContext":{"mainAppWebResponseContext":{"loggedOut": true}}})
    ));
    assert!(signed_out(
        &json!({"responseContext":{"serviceTrackingParams":[
            {"service":"GFEEDBACK","params":[{"key":"logged_in","value":"0"}]}
        ]}})
    ));
    assert_eq!(profile_unavailable().kind, ErrorKind::Unavailable);
    assert_eq!(cookie_expired().kind, ErrorKind::Expired);
}

#[test]
fn a_missing_menu_can_use_the_explicitly_selected_switcher_identity() {
    let viewed = identity("Music", "@same", Some("brand1"));
    assert!(viewed.verify(Err(profile_unavailable())).is_err());
    let selected = Identity {
        selected: true,
        ..viewed
    };
    let restored = selected.verify(Err(profile_unavailable())).unwrap();
    assert_eq!(restored.id, "brand1");
    assert!(selected.verify(Err(cookie_expired())).is_err());
    assert!(
        selected
            .verify(Ok(identity("Music", "@another", None)))
            .is_err()
    );
}

#[cfg(windows)]
#[test]
#[ignore = "calls real YouTube endpoints with a disposable non-account cookie"]
fn live_disposable_youtube_session_is_rejected() {
    use crate::state::{ActiveSession, HomeContent, HomeView, Intent, Issue, Screen, State};
    use crate::storage::{CacheScope, Store};
    use std::process::Command;

    let profile_dir = tempfile::tempdir().unwrap();
    let store = Store::open(profile_dir.path()).unwrap();
    let session = Session::new(
        Profile {
            id: "@mediadeka-test-invalid".to_owned(),
            name: "Fixture".to_owned(),
        },
        Credentials::YouTubeMusic {
            cookies: "SAPISID=mediadeka-test-expired-session".to_owned(),
            index: 0,
            page_id: None,
        },
    );
    assert!(store.use_account(&session, || Some(()), || {}).unwrap());
    let _cleanup = ConnectedFixture {
        store: &store,
        id: &session.profile.id,
    };
    let scope = CacheScope::Account(session.provider, &session.profile.id);
    let personal = snapshot("Personal fixture");
    store.save_cache(scope, &personal, || Some(())).unwrap();
    store
        .save_cache(CacheScope::Guest, &snapshot("Guest fixture"), || Some(()))
        .unwrap();

    let screen = State::default().begin(Intent::Startup).run(&store);
    assert!(matches!(
        screen,
        Screen::Home(HomeView {
            session: ActiveSession::Account { profile, .. },
            content: HomeContent::Cached {
                snapshot,
                issue: Issue::Provider(ProviderError { kind: ErrorKind::Expired, .. }),
            },
            ..
        }) if profile == session.profile && snapshot == personal
    ));
    assert_eq!(store.cache(scope).unwrap(), Some(personal));
    println!(
        "YouTube Music confirmed sign-out in the real startup path and retained the personal snapshot."
    );

    let closed_proxy = "http://127.0.0.1:1";
    let offline = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "providers::youtube::account::tests::disposable_youtube_restore_uses_personal_snapshot_after_network_failure",
            "--nocapture",
        ])
        .env("MEDIADEKA_TEST_YOUTUBE_RESTORE_PROFILE", profile_dir.path())
        .env("HTTPS_PROXY", closed_proxy)
        .env("HTTP_PROXY", closed_proxy)
        .env("ALL_PROXY", closed_proxy)
        .env("NO_PROXY", "")
        .output()
        .unwrap();
    let output = String::from_utf8_lossy(&offline.stdout);
    print!("{output}");
    assert!(offline.status.success() && output.contains("1 passed"));
}

#[cfg(windows)]
struct ConnectedFixture<'a> {
    store: &'a crate::storage::Store,
    id: &'a str,
}

#[cfg(windows)]
impl Drop for ConnectedFixture<'_> {
    fn drop(&mut self) {
        let _ = self
            .store
            .disconnect_account(super::super::PROVIDER, self.id, || Some(()));
    }
}

#[cfg(windows)]
fn snapshot(title: &str) -> crate::storage::Snapshot {
    crate::storage::Snapshot::received(crate::providers::Home {
        title: title.to_owned(),
        items: Vec::new(),
        sections: Vec::new(),
    })
}

#[cfg(windows)]
#[test]
#[ignore = "spawned with an isolated profile and deliberately closed network proxy"]
fn disposable_youtube_restore_uses_personal_snapshot_after_network_failure() {
    use crate::state::{ActiveSession, HomeContent, HomeView, Intent, Issue, Screen, State};
    use crate::storage::{CacheScope, Store};
    let path = std::env::var_os("MEDIADEKA_TEST_YOUTUBE_RESTORE_PROFILE").unwrap();
    let store = Store::open(path).unwrap();
    let provider = super::super::PROVIDER;
    let saved = store.preferred_account(provider).unwrap().unwrap();
    let scope = CacheScope::Account(provider, &saved.profile.id);
    let expected = store.cache(scope).unwrap().unwrap();
    let screen = State::default().begin(Intent::Startup).run(&store);
    assert!(matches!(
        screen,
        Screen::Home(HomeView {
            session: ActiveSession::Account { profile, .. },
            content: HomeContent::Cached {
                snapshot,
                issue: Issue::Provider(ProviderError { kind: ErrorKind::Network, .. }),
            },
            ..
        }) if profile == saved.profile && snapshot == expected
    ));
    println!("The same profile and its own SQLite snapshot survived the network refusal.");
}
