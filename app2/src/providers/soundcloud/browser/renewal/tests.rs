use std::{
    io::{BufRead, BufReader, Write},
    net::{Ipv4Addr, TcpListener},
};

use wry::cookie::{Cookie, SameSite, time};

use super::*;
use crate::providers::soundcloud::browser::REFRESH_COOKIE;

const STRICT_COOKIE: &str = "strict-credential";

#[test]
fn separates_temporary_or_ambiguous_renewal_from_rejected_access() {
    assert_eq!(refresh_status(204), Ok(true));
    assert_eq!(refresh_status(401), Ok(false));
    assert_eq!(
        refresh_status(403).unwrap_err().kind,
        ErrorKind::Unavailable
    );
    for status in [408, 429, 503] {
        assert_eq!(refresh_status(status).unwrap_err().kind, ErrorKind::Network);
    }
}

#[test]
fn missing_webview_profile_does_not_create_state() {
    let profile = tempfile::tempdir().unwrap();
    let cancelled = AtomicBool::new(false);
    assert!(
        renew(profile.path(), "expired", &cancelled)
            .unwrap()
            .is_none()
    );
    clear_browser_session(profile.path(), &cancelled).unwrap();
    assert!(!profile.path().join(SIGN_IN_DIRECTORY).exists());
}

#[test]
#[ignore = "requires Windows WebView2 and sends one request to a loopback fixture"]
fn live_soundcloud_native_post_uses_the_webview_cookie_jar() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!(
        "http://127.0.0.1:{}/renewal",
        listener.local_addr().unwrap().port()
    );
    let server = thread::spawn(move || serve_renewal(listener));
    let directory = tempfile::tempdir().unwrap();
    let result = with_browser(&directory.path().join("fixture"), Some(&url), |view| {
        for (name, policy) in [
            (REFRESH_COOKIE, SameSite::Lax),
            (STRICT_COOKIE, SameSite::Strict),
        ] {
            let cookie = Cookie::build((name, "fixture-grant"))
                .domain("127.0.0.1")
                .path("/")
                .http_only(true)
                .secure(true)
                .same_site(policy)
                .expires(time::OffsetDateTime::now_utc() + time::Duration::days(1))
                .build();
            view.set_cookie(&cookie).unwrap();
        }
        assert!(view.cookies_for_url(&url).unwrap().iter().any(|cookie| {
            cookie.name() == REFRESH_COOKIE && cookie.value() == "fixture-grant"
        }));
        let status = post_refresh(view, &url, &AtomicBool::new(false))?;
        assert_eq!(status, 204);
        assert!(view.cookies_for_url(&url).unwrap().iter().any(|cookie| {
            cookie.name() == "oauth_token" && cookie.value() == "fixture-new-token"
        }));
        Ok(())
    });
    let sent_cookie = server.join();
    result.expect("execute native POST and store the replacement cookie");
    assert_eq!(
        sent_cookie.unwrap(),
        (true, true),
        "WebView2 must send the Lax and Strict cookies"
    );
    directory.close().unwrap();
}

fn serve_renewal(listener: TcpListener) -> (bool, bool) {
    let deadline = Instant::now() + REFRESH_TIMEOUT;
    let mut line = String::new();
    let mut stream = loop {
        assert!(Instant::now() < deadline, "WebView2 POST did not arrive");
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(PUMP_INTERVAL);
                continue;
            }
            Err(error) => panic!("Could not accept loopback POST: {error}"),
        };
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut stream = BufReader::new(stream);
        line.clear();
        // Chromium can preconnect without issuing the actual request on that socket.
        if stream.read_line(&mut line).is_ok_and(|size| size > 0) {
            break stream;
        }
    };
    assert!(line.starts_with("POST /renewal HTTP/"));
    let mut lax = false;
    let mut strict = false;
    loop {
        line.clear();
        let read = stream.read_line(&mut line).unwrap();
        if read == 0 || line == "\r\n" {
            break;
        }
        if line.to_ascii_lowercase().starts_with("cookie:") {
            lax |= line.contains("oauth_refresh_token=fixture-grant");
            strict |= line.contains("strict-credential=fixture-grant");
        }
    }
    stream
        .get_mut()
        .write_all(
            b"HTTP/1.1 204 No Content\r\nSet-Cookie: oauth_token=fixture-new-token; Path=/; Secure; SameSite=Lax\r\nConnection: close\r\n\r\n",
        )
        .unwrap();
    (lax, strict)
}
