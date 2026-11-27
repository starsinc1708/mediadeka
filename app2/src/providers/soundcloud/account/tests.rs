use std::sync::atomic::Ordering;

use serde_json::json;

use super::*;

const TOKEN: &str = "test-soundcloud-token";
const OLD_CLIENT: &str = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const CURRENT_CLIENT: &str = "BBBBBBBBBBBBBBBBBBBBBBBBBBBBBBBB";
const RENEWED_TOKEN: &str = "renewed-test-token";

fn restore_without_renewal(
    credentials: Credentials,
    cancelled: &AtomicBool,
    verify: impl FnMut(&str, &str) -> Result<Value, ProviderError>,
    discover: impl FnOnce() -> Result<String, ProviderError>,
) -> Result<Session, ProviderError> {
    restore_with(credentials, cancelled, verify, discover, |_| Ok(None))
}

fn saved_credentials() -> Credentials {
    Credentials::SoundCloud {
        token: TOKEN.into(),
        client_id: OLD_CLIENT.into(),
    }
}

fn confirmed() -> Value {
    json!({"id": 42, "username": "Confirmed profile"})
}

#[test]
fn retains_working_credentials_without_discovery() {
    let session = restore_without_renewal(
        saved_credentials(),
        &AtomicBool::new(false),
        |token, client| {
            assert_eq!((token, client), (TOKEN, OLD_CLIENT));
            Ok(confirmed())
        },
        || panic!("discovery for a working connection"),
    )
    .unwrap();
    assert_eq!(session.profile.id, "42");
    assert!(session.credentials == saved_credentials());
}

#[test]
fn verifies_the_current_public_client_once_before_returning_it() {
    for status in [401, 403] {
        let mut clients_checked = Vec::new();
        let session = restore_without_renewal(
            saved_credentials(),
            &AtomicBool::new(false),
            |token, client| {
                assert_eq!(token, TOKEN);
                clients_checked.push(client.to_owned());
                if client == OLD_CLIENT {
                    Err(response_error(PROVIDER, status))
                } else {
                    Ok(confirmed())
                }
            },
            || Ok(CURRENT_CLIENT.into()),
        )
        .unwrap();
        assert_eq!(clients_checked, [OLD_CLIENT, CURRENT_CLIENT]);
        assert_eq!(session.profile.id, "42");
        assert!(matches!(
            session.credentials,
            Credentials::SoundCloud { token, client_id }
                if token == TOKEN && client_id == CURRENT_CLIENT
        ));
    }
}

#[test]
fn reports_discovery_outage_instead_of_the_initial_unauthorized_response() {
    for kind in [ErrorKind::Network, ErrorKind::Unavailable] {
        let issue = ProviderError::new(kind, "Public configuration unavailable");
        let error = restore_without_renewal(
            saved_credentials(),
            &AtomicBool::new(false),
            |_, _| Err(response_error(PROVIDER, 401)),
            || Err(issue.clone()),
        )
        .err()
        .unwrap();
        assert_eq!(error, issue);
    }
}

#[test]
fn distinguishes_expired_access_from_an_ambiguous_forbidden_response() {
    for client in [OLD_CLIENT, CURRENT_CLIENT] {
        for (status, kind) in [(401, ErrorKind::Expired), (403, ErrorKind::Unavailable)] {
            let mut probes = 0;
            let error = restore_without_renewal(
                saved_credentials(),
                &AtomicBool::new(false),
                |_, _| {
                    probes += 1;
                    Err(response_error(PROVIDER, status))
                },
                || Ok(client.into()),
            )
            .err()
            .unwrap();
            assert_eq!(error.kind, kind);
            assert_eq!(probes, if client == OLD_CLIENT { 1 } else { 2 });
        }
    }
}

#[test]
fn keeps_the_retry_network_failure_temporary() {
    let issue = ProviderError::new(ErrorKind::Network, "Current client probe is offline");
    let error = restore_without_renewal(
        saved_credentials(),
        &AtomicBool::new(false),
        |_, client| {
            if client == OLD_CLIENT {
                Err(response_error(PROVIDER, 401))
            } else {
                Err(issue.clone())
            }
        },
        || Ok(CURRENT_CLIENT.into()),
    )
    .err()
    .unwrap();
    assert_eq!(error, issue);
}

#[test]
fn cancellation_prevents_retry_after_discovery() {
    let cancelled = AtomicBool::new(false);
    let error = restore_without_renewal(
        saved_credentials(),
        &cancelled,
        |_, client| {
            assert_eq!(client, OLD_CLIENT);
            Err(response_error(PROVIDER, 401))
        },
        || {
            cancelled.store(true, Ordering::Relaxed);
            Ok(CURRENT_CLIENT.into())
        },
    )
    .err()
    .unwrap();
    assert_eq!(error.kind, ErrorKind::Cancelled);
}

#[test]
fn verifies_renewed_token_with_the_current_client_and_preserves_profile() {
    let mut checked = Vec::new();
    let session = restore_with(
        saved_credentials(),
        &AtomicBool::new(false),
        |token, client| {
            checked.push((token.to_owned(), client.to_owned()));
            if token == RENEWED_TOKEN {
                Ok(confirmed())
            } else {
                Err(response_error(PROVIDER, 401))
            }
        },
        || Ok(CURRENT_CLIENT.into()),
        |expired| {
            assert_eq!(expired, TOKEN);
            Ok(Some(RENEWED_TOKEN.into()))
        },
    )
    .unwrap();
    assert_eq!(
        checked
            .iter()
            .map(|(token, client)| (token.as_str(), client.as_str()))
            .collect::<Vec<_>>(),
        [
            (TOKEN, OLD_CLIENT),
            (TOKEN, CURRENT_CLIENT),
            (RENEWED_TOKEN, CURRENT_CLIENT)
        ]
    );
    let wrong_profile = Profile {
        id: "7".into(),
        name: session.profile.name.clone(),
    };
    let error = confirm_restored_profile(session, &wrong_profile)
        .err()
        .unwrap();
    assert_eq!(error.kind, ErrorKind::Expired);
}

#[test]
fn leaves_missing_or_unchanged_renewal_expired() {
    for token in [None, Some(TOKEN.to_owned())] {
        let mut checks = 0;
        let error = restore_with(
            saved_credentials(),
            &AtomicBool::new(false),
            |_, _| {
                checks += 1;
                Err(response_error(PROVIDER, 401))
            },
            || Ok(OLD_CLIENT.into()),
            |_| Ok(token),
        )
        .err()
        .unwrap();
        assert_eq!(error.kind, ErrorKind::Expired);
        assert_eq!(checks, 1);
    }
}

#[test]
fn preserves_temporary_renewal_failure_and_skips_renewal_on_forbidden() {
    let outage = ProviderError::new(ErrorKind::Network, "Renewal unavailable");
    let attempt = |status| {
        restore_with(
            saved_credentials(),
            &AtomicBool::new(false),
            |_, _| Err(response_error(PROVIDER, status)),
            || Ok(OLD_CLIENT.into()),
            |_| {
                assert_eq!(status, 401);
                Err(outage.clone())
            },
        )
        .err()
        .unwrap()
    };
    assert_eq!(attempt(401), outage);
    assert_eq!(attempt(403).kind, ErrorKind::Unavailable);
}
