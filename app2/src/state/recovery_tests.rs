use super::*;
use crate::providers::{Credentials, Home};

struct Fixture {
    directory: tempfile::TempDir,
    store: Store,
    session: Session,
}

impl Fixture {
    fn new(session: Session) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        assert!(store.use_account(&session, || Some(()), || {}).unwrap());
        Self {
            directory,
            store,
            session,
        }
    }

    fn with_schema(session: Session, version: usize) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::versioned_fixture(directory.path(), version).unwrap();
        assert!(store.use_account(&session, || Some(()), || {}).unwrap());
        Self {
            directory,
            store,
            session,
        }
    }

    fn account(&self) -> Account {
        self.store
            .preferred_account(self.session.provider)
            .unwrap()
            .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ =
            self.store
                .disconnect_account(self.session.provider, &self.session.profile.id, || Some(()));
    }
}

fn jellyfin(profile: &str, access_token: &str) -> Session {
    Session {
        provider: ProviderId::Jellyfin,
        profile: Profile {
            id: profile.into(),
            name: profile.into(),
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: true,
        },
        credentials: Credentials::Jellyfin {
            server: "http://127.0.0.1:1/".into(),
            device: "isolated-test".into(),
            access_token: access_token.into(),
        },
    }
}

#[test]
fn reconnect_replaces_only_the_expected_profile_and_keeps_the_route() {
    let fixture = Fixture::new(jellyfin("original", "before"));
    let old = fixture.account();
    let store = &fixture.store;
    assert!(store.use_guest(|| Some(())).unwrap());
    let mut state = State::default();
    let startup = state.begin(Intent::Startup);
    assert!(state.finish(&startup, Screen::Home(startup.home_preview(store).unwrap())));

    let request = state.begin(Intent::Connect(old.provider));
    assert!(request.login_preparation_failure(store).is_none());
    let rejected = request.finish_login(store, Ok(jellyfin("different", "other")));
    assert!(matches!(rejected, Screen::Failed(_)));
    assert!(state.finish(&request, rejected));
    assert_eq!(fixture.account(), old);

    let request = state.begin(Intent::Connect(old.provider));
    assert!(request.login_preparation_failure(store).is_none());
    let updated = jellyfin("original", "after");
    let result = request.finish_login(store, Ok(updated.clone()));
    assert!(matches!(result, Screen::Home(_)));
    assert_eq!(request.reconnected_provider(), Some(old.provider));
    assert!(state.finish(&request, result));
    let reopened = Store::open(fixture.directory.path()).unwrap();
    assert!(matches!(reopened.startup().unwrap(), Startup::Guest));
    let new = reopened.preferred_account(old.provider).unwrap().unwrap();
    assert_ne!(new, old);
    assert!(reopened.credentials(&new).unwrap().unwrap() == updated.credentials);
}

#[test]
fn expiry_keeps_account_snapshots_but_revocation_invalidates_current_binding() {
    let fixture = Fixture::new(jellyfin("saved", "saved-token"));
    let account = fixture.account();
    let store = &fixture.store;
    let active = ActiveSession::from(&account);
    let scope = CacheScope::Account(account.provider, &account.profile.id);
    let snapshot = Snapshot::received(Home {
        title: "Saved personal home".into(),
        items: Vec::new(),
        sections: Vec::new(),
    });
    store
        .save_session_cache(scope, &snapshot, || Some(()))
        .unwrap();
    assert!(
        store
            .save_music_snapshot(account.provider, Some(&account), "test", "liked", &vec![1])
            .unwrap()
    );

    let request = Request::new(Intent::Refresh, None, None);
    let expired = ProviderError {
        kind: ErrorKind::Expired,
        message: "Session expired".into(),
    };
    let (_, content) = request.restore_error(store, active, expired).unwrap();
    assert!(matches!(content, HomeContent::Cached { .. }));
    assert_eq!(
        store
            .music_snapshot::<Vec<u8>>(account.provider, Some(&account), "test", "liked")
            .unwrap(),
        Some(vec![1])
    );

    assert!(store.forget_music_account(&account).unwrap());
    assert!(store.account_revoked(&account).unwrap());
    store
        .save_session_cache(scope, &snapshot, || Some(()))
        .unwrap();
    assert!(store.cache(scope).unwrap().is_none());
    assert!(
        store
            .music_snapshot::<Vec<u8>>(account.provider, Some(&account), "test", "liked")
            .unwrap()
            .is_none()
    );
    assert!(matches!(
        request.run(store),
        Screen::Home(HomeView {
            content: HomeContent::Empty(Issue::Provider(ProviderError {
                kind: ErrorKind::Revoked,
                ..
            })),
            ..
        })
    ));
    assert_eq!(fixture.account(), account);
    assert!(
        store
            .update_account(&jellyfin("saved", "new-token"), &account, || Some(()))
            .unwrap()
    );
    assert!(!store.account_revoked(&fixture.account()).unwrap());
}

#[test]
fn migration_releases_legacy_expiry_markers_without_dropping_the_profile() {
    let fixture = Fixture::with_schema(jellyfin("legacy", "stored-token"), 17);
    let account = fixture.account();
    let database =
        rusqlite::Connection::open(fixture.directory.path().join("mediadeka.sqlite3")).unwrap();
    database
        .execute(
            "INSERT INTO music_revoked SELECT provider, profile_id, credential_ref FROM account",
            [],
        )
        .unwrap();
    drop(database);
    assert!(fixture.store.account_revoked(&account).unwrap());
    let reopened = Store::open(fixture.directory.path()).unwrap();
    assert!(!reopened.account_revoked(&account).unwrap());
    assert!(matches!(reopened.startup().unwrap(), Startup::Account(current) if current == account));
}
