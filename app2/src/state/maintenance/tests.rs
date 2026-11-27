use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

use crate::providers::{Capabilities, Credentials, ErrorKind, Profile, Session};

use super::*;

const ID: &str = "0d66c253b6a14f2c931c74bc69125982";
const PROVIDER: ProviderId = ProviderId::Jellyfin;

struct Fixture {
    _directory: tempfile::TempDir,
    store: Store,
    session: Session,
}

impl Fixture {
    fn with_address(address: std::net::SocketAddr) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let session = Session {
            provider: PROVIDER,
            profile: Profile {
                id: format!("http://{address}/#{ID}"),
                name: "Test server".into(),
            },
            capabilities: Capabilities {
                home: true,
                profile_selection: false,
                device_code: true,
            },
            credentials: Credentials::Jellyfin {
                server: format!("http://{address}/"),
                device: "connection-maintenance".into(),
                access_token: "fixture".into(),
            },
        };
        assert!(store.use_account(&session, || Some(()), || {}).unwrap());
        Self {
            _directory: directory,
            store,
            session,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self
            .store
            .disconnect_account(PROVIDER, &self.session.profile.id, || Some(()));
    }
}

#[test]
fn renews_before_the_expiry_and_limits_retry_frequency() {
    assert_eq!(
        normal_delay(Some(1_350), Some(1_000)),
        Duration::from_secs(50)
    );
    assert_eq!(normal_delay(Some(1_001), Some(1_000)), RETRY_INTERVAL);
    assert_eq!(
        normal_delay(Some(1_200), Some(1_000)),
        Duration::from_secs(100)
    );
    assert_eq!(normal_delay(None, Some(1_000)), HEALTH_INTERVAL);
    assert_eq!(retry_delay(0), RETRY_INTERVAL);
    assert_eq!(retry_delay(2), RETRY_INTERVAL * 4);
    assert_eq!(retry_delay(100), MAX_RETRY);
}

#[test]
fn maintenance_rechecks_an_unavailable_session_without_a_home_request() {
    let reserving = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = reserving.local_addr().unwrap();
    drop(reserving);
    let fixture = Fixture::with_address(addr);
    let mut maintenance = ConnectionMaintenance::default();
    let cancelled = AtomicBool::new(false);
    assert!(
        maintenance
            .poll(&fixture.store, &cancelled)
            .unwrap()
            .is_empty()
    );
    assert_eq!(maintenance.accounts[&PROVIDER].failures, 1);
    let server = TcpListener::bind(addr).unwrap();
    let handle = thread::spawn(move || {
        let (mut client, _) = server.accept().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = [0; 2048];
        let len = client.read(&mut request).unwrap();
        assert!(String::from_utf8_lossy(&request[..len]).starts_with("GET /Users/Me "));
        let body = format!(r#"{{"Id":"{ID}","Name":"Test server"}}"#);
        write!(
            client,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    });
    maintenance.accounts.get_mut(&PROVIDER).unwrap().next = Some(Instant::now());
    let updates = maintenance.poll(&fixture.store, &cancelled).unwrap();
    handle.join().unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].account.profile, fixture.session.profile);
    assert!(updates[0].problem.is_none());
    assert_eq!(maintenance.accounts[&PROVIDER].failures, 0);
}

#[test]
fn an_expired_binding_waits_for_new_credentials_and_disconnected_accounts_are_removed() {
    let fixture = Fixture::with_address("127.0.0.1:1".parse().unwrap());
    let mut maintenance = ConnectionMaintenance::default();
    let cancelled = AtomicBool::new(false);
    let result = maintenance
        .poll_with(&fixture.store, &cancelled, |_, _, _| {
            Err(ProviderError {
                kind: ErrorKind::Expired,
                message: "expired fixture".into(),
            }
            .into())
        })
        .unwrap();
    assert!(result[0].problem.as_ref().unwrap().needs_sign_in());
    assert!(maintenance.accounts[&PROVIDER].next.is_none());
    let held = maintenance
        .poll_with(&fixture.store, &cancelled, |_, _, _| {
            panic!("terminal grant polled again")
        })
        .unwrap();
    assert!(held[0].problem.as_ref().unwrap().needs_sign_in());

    let previous = fixture.store.preferred_account(PROVIDER).unwrap().unwrap();
    assert!(
        fixture
            .store
            .update_account(&fixture.session, &previous, || Some(()))
            .unwrap()
    );
    let updated = maintenance
        .poll_with(&fixture.store, &cancelled, |_, _, _| Ok(None))
        .unwrap();
    assert_eq!(updated.len(), 1);
    assert_ne!(updated[0].account, previous);
    fixture
        .store
        .disconnect_account(PROVIDER, &fixture.session.profile.id, || Some(()))
        .unwrap();
    assert!(
        maintenance
            .poll_with(&fixture.store, &cancelled, |_, _, _| panic!(
                "disconnected grant polled"
            ))
            .unwrap()
            .is_empty()
    );
    assert!(maintenance.accounts.is_empty());
}

#[test]
fn an_unavailable_source_does_not_block_other_connections_or_restore_a_logout() {
    let fixture = Fixture::with_address("127.0.0.1:1".parse().unwrap());
    let store = &fixture.store;
    let soundcloud = Session {
        provider: ProviderId::SoundCloud,
        profile: Profile {
            id: "secondary".into(),
            name: "Other connection".into(),
        },
        capabilities: fixture.session.capabilities,
        credentials: Credentials::SoundCloud {
            token: "fixture".into(),
            client_id: "fixture".into(),
        },
    };
    assert!(
        store
            .connect_account(&soundcloud, || Some(()), || {})
            .unwrap()
            .is_some()
    );
    let mut maintenance = ConnectionMaintenance::default();
    let cancelled = AtomicBool::new(false);
    let mut calls = Vec::new();
    let updates = maintenance
        .poll_with(store, &cancelled, |store, account, _| {
            calls.push(account.provider);
            if account.provider == ProviderId::SoundCloud {
                return Err(ProviderError {
                    kind: ErrorKind::Network,
                    message: "fixture offline".into(),
                }
                .into());
            }
            store
                .disconnect_account(account.provider, &account.profile.id, || Some(()))
                .unwrap();
            Ok(None)
        })
        .unwrap();
    assert_eq!(calls, [ProviderId::SoundCloud, PROVIDER]);
    assert!(updates.is_empty());
    assert!(store.preferred_account(PROVIDER).unwrap().is_none());
    assert_eq!(maintenance.accounts[&ProviderId::SoundCloud].failures, 1);
    store
        .disconnect_account(soundcloud.provider, &soundcloud.profile.id, || Some(()))
        .unwrap();
}
