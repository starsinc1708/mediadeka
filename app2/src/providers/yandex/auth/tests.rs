use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    process::Command,
    sync::atomic::Ordering,
    thread,
};

use serde_json::json;

use super::*;
use crate::providers::{
    LoginInput, renew as renew_account, restore as restore_account, sign_in as sign_in_device,
};
use crate::{state::ConnectionMaintenance, storage::Store};

const CHILD: &str = "MEDIADEKA_YANDEX_RECOVERY_FIXTURE_CHILD";
const TEST: &str = "providers::yandex::auth::tests::device_grant_recovery_through_http";
const FIRST_ACCESS: &str = "fixture-access-1";
const FIRST_REFRESH: &str = "fixture-refresh-1";
const SECOND_ACCESS: &str = "fixture-access-2";
const SECOND_REFRESH: &str = "fixture-refresh-2";
const THIRD_ACCESS: &str = "fixture-access-3";
const FIRST_LIFETIME: u64 = 7200;
const SECOND_LIFETIME: u64 = 3600;

#[test]
fn device_grant_recovery_through_http() {
    if std::env::var_os(CHILD).is_none() {
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD, "1")
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    crate::providers::yandex::test_server::use_loopback(listener.local_addr().unwrap());
    let server = thread::spawn(move || {
        use Request::{Account, Device, Exchange};

        let profile = json!({"result":{"account":{"uid":74368,"displayName":"Fixture"}}});
        let steps = [
            (
                Device,
                200,
                json!({
                    "device_code":"fixture-device",
                    "user_code":"abc123",
                    "interval":1,
                    "expires_in":30,
                }),
            ),
            (
                Exchange("device_code", "code", "fixture-device"),
                200,
                json!({
                    "access_token":FIRST_ACCESS,
                    "refresh_token":FIRST_REFRESH,
                    "expires_in":FIRST_LIFETIME,
                }),
            ),
            (Account(FIRST_ACCESS), 200, profile.clone()),
            (Account(FIRST_ACCESS), 200, profile.clone()),
            (
                Exchange("refresh_token", "refresh_token", FIRST_REFRESH),
                200,
                json!({
                    "access_token":SECOND_ACCESS,
                    "refresh_token":SECOND_REFRESH,
                    "expires_in":SECOND_LIFETIME,
                }),
            ),
            (Account(SECOND_ACCESS), 200, profile.clone()),
            (Account(SECOND_ACCESS), 401, json!({})),
            (
                Exchange("refresh_token", "refresh_token", SECOND_REFRESH),
                200,
                json!({"access_token":THIRD_ACCESS}),
            ),
            (Account(THIRD_ACCESS), 200, profile.clone()),
            (Account(THIRD_ACCESS), 401, json!({})),
            (
                Exchange("refresh_token", "refresh_token", SECOND_REFRESH),
                503,
                json!({"error":"expired_token"}),
            ),
            (Account(THIRD_ACCESS), 403, json!({})),
            (Account(THIRD_ACCESS), 200, json!({})),
            (Account(THIRD_ACCESS), 401, json!({})),
            (
                Exchange("refresh_token", "refresh_token", SECOND_REFRESH),
                400,
                json!({"error":"invalid_grant"}),
            ),
            (Account(THIRD_ACCESS), 200, profile),
            (Account(THIRD_ACCESS), 401, json!({})),
        ];
        for (request, status, response) in steps {
            respond(listener.accept().unwrap().0, request, status, response);
        }
    });
    let cancelled = AtomicBool::new(false);
    let first_started = epoch_seconds().unwrap();
    let first = sign_in_device(PROVIDER, LoginInput::YandexDevice, &cancelled, |prompt| {
        assert_eq!(
            prompt,
            LoginPrompt::Code {
                code: "abc123".into(),
                url: VERIFICATION_URL.into(),
            }
        );
    })
    .unwrap();
    assert_eq!(first.profile.id, "74368");
    assert_grant(&first, FIRST_ACCESS, Some(FIRST_REFRESH));
    assert_expiration(&first, first_started, FIRST_LIFETIME);
    assert_eq!(first.credentials.renewal_deadline(), expire_at(&first));

    let restore = |credentials| restore_account(PROVIDER, &first.profile, credentials, &cancelled);
    let second_started = epoch_seconds().unwrap();
    let mut due = first.clone();
    let Credentials::YandexMusic {
        access_expires_at, ..
    } = &mut due.credentials
    else {
        unreachable!()
    };
    *access_expires_at = Some(second_started + 60);
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    assert!(store.use_account(&due, || Some(()), || {}).unwrap());
    let checks = ConnectionMaintenance::default()
        .poll(&store, &cancelled)
        .unwrap();
    assert_eq!(checks.len(), 1);
    assert!(checks[0].problem.is_none());
    let reopened = Store::open(directory.path()).unwrap();
    let account = reopened.preferred_account(PROVIDER).unwrap().unwrap();
    let second = Session::new(
        account.profile.clone(),
        reopened.credentials(&account).unwrap().unwrap(),
    );
    assert_eq!(second.profile, first.profile);
    assert_grant(&second, SECOND_ACCESS, Some(SECOND_REFRESH));
    assert_expiration(&second, second_started, SECOND_LIFETIME);
    reopened
        .disconnect_account(PROVIDER, &account.profile.id, || Some(()))
        .unwrap();
    let third = restore(second.credentials).unwrap();
    assert_grant(&third, THIRD_ACCESS, Some(SECOND_REFRESH));
    assert_eq!(expire_at(&third), None);
    assert_eq!(third.credentials.renewal_deadline(), None);

    for kind in [
        ErrorKind::Network,
        ErrorKind::Unavailable,
        ErrorKind::Unavailable,
        ErrorKind::Expired,
    ] {
        assert!(matches!(restore(third.credentials.clone()), Err(error) if error.kind == kind));
    }

    let legacy: Credentials =
        serde_json::from_value(json!({"provider":"yandex-music","access_token":THIRD_ACCESS}))
            .unwrap();
    let restored = restore(legacy.clone()).unwrap();
    assert_eq!(restored.profile, first.profile);
    assert_grant(&restored, THIRD_ACCESS, None);
    assert_eq!(expire_at(&restored), None);
    assert_eq!(restored.credentials.renewal_deadline(), None);
    assert!(matches!(
        renew_account(restored, &cancelled),
        Err(error) if error.kind == ErrorKind::Expired
    ));
    assert!(matches!(restore(legacy.clone()), Err(error) if error.kind == ErrorKind::Expired));
    cancelled.store(true, Ordering::Relaxed);
    assert!(matches!(restore(legacy), Err(error) if error.kind == ErrorKind::Cancelled));
    assert!(matches!(
        renew_account(third, &cancelled),
        Err(error) if error.kind == ErrorKind::Cancelled
    ));
    server.join().unwrap();
}

fn assert_grant(session: &Session, access: &str, refresh: Option<&str>) {
    let Credentials::YandexMusic {
        access_token,
        refresh_token,
        ..
    } = &session.credentials
    else {
        panic!("expected Yandex credentials")
    };
    assert_eq!(access_token, access);
    assert_eq!(refresh_token.as_deref(), refresh);
}

fn expire_at(session: &Session) -> Option<u64> {
    let Credentials::YandexMusic {
        access_expires_at, ..
    } = &session.credentials
    else {
        panic!("expected Yandex credentials")
    };
    *access_expires_at
}

fn assert_expiration(session: &Session, started: u64, lifetime: u64) {
    let finished = epoch_seconds().unwrap();
    assert!((started + lifetime..=finished + lifetime).contains(&expire_at(session).unwrap()));
}

enum Request<'a> {
    Account(&'a str),
    Device,
    Exchange(&'a str, &'a str, &'a str),
}

fn respond(mut stream: TcpStream, expected: Request<'_>, status: u16, response: Value) {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = BufReader::new(&mut stream);
    let mut line = String::new();
    request.read_line(&mut line).unwrap();
    let route = line
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    let mut length = 0;
    let mut authorization = None;
    loop {
        line.clear();
        assert_ne!(request.read_line(&mut line).unwrap(), 0);
        let Some((key, value)) = line.trim_end().split_once(':') else {
            break;
        };
        if key.eq_ignore_ascii_case("content-length") {
            length = value.trim().parse().unwrap();
        }
        if key.eq_ignore_ascii_case("authorization") {
            authorization = Some(value.trim().to_owned());
        }
    }
    let mut body = vec![0; length];
    request.read_exact(&mut body).unwrap();
    let form: std::collections::HashMap<_, _> = url::form_urlencoded::parse(&body).collect();
    match expected {
        Request::Account(access) => {
            assert_eq!(route, "GET /account/status");
            assert_eq!(authorization, Some(format!("OAuth {access}")));
        }
        Request::Device => {
            assert_eq!(route, "POST /device/code");
            assert!(form.get("device_id").is_some_and(|value| !value.is_empty()));
            assert_eq!(form["device_name"], "Mediadeka");
        }
        Request::Exchange(grant, parameter, value) => {
            assert_eq!(route, "POST /token");
            assert_eq!(form["grant_type"], grant);
            assert_eq!(form[parameter], value);
        }
    }
    let response = response.to_string();
    write!(
        stream,
        "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
        response.len()
    )
    .unwrap();
}
