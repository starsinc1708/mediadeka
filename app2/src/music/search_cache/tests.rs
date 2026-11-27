use super::*;

#[test]
fn query_normalization_preserves_musical_punctuation() {
    assert_eq!(normalize("  AC/DC\t LIVE  "), "ac/dc live");
    assert_ne!(normalize("AC/DC Live"), normalize("AC DC Live"));
    assert_ne!(normalize("Wham!"), normalize("Wham"));
}

#[test]
fn cancelling_during_debounce_stops_before_dispatch() {
    let cancelled = AtomicBool::new(true);
    assert_eq!(
        wait(Instant::now() + DEBOUNCE, &cancelled)
            .unwrap_err()
            .kind,
        ErrorKind::Cancelled,
    );
}

#[cfg(windows)]
#[test]
fn catalog_search_coalesces_warm_spelling_and_preserves_stale_offline() {
    use crate::{
        providers::{Capabilities, Credentials, Profile, Session, yandex::test_server},
        telemetry,
    };
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        process::Command,
    };

    const CHILD: &str = "MEDIADEKA_SEARCH_FIXTURE_CHILD";
    const TEST: &str = "music::search_cache::tests::catalog_search_coalesces_warm_spelling_and_preserves_stale_offline";
    if std::env::var_os(CHILD).is_none() {
        let directory = tempfile::tempdir().unwrap();
        assert!(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", TEST, "--nocapture"])
                .env(CHILD, "1")
                .env("MEDIADEKA_PROFILE_DIR", directory.path())
                .status()
                .unwrap()
                .success()
        );
        return;
    }

    let store = Store::open_default().unwrap();
    let session = Session {
        provider: ProviderId::YandexMusic,
        profile: Profile {
            id: "83160".into(),
            name: "Search fixture".into(),
        },
        credentials: Credentials::YandexMusic {
            access_token: "fixture-search".into(),
            refresh_token: None,
            access_expires_at: None,
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: true,
        },
    };
    struct Disconnect<'a>(&'a Store, &'a Session);
    impl Drop for Disconnect<'_> {
        fn drop(&mut self) {
            let _ = self
                .0
                .disconnect_account(self.1.provider, &self.1.profile.id, || Some(()));
        }
    }
    store
        .connect_account(&session, || Some(()), || {})
        .unwrap()
        .unwrap();
    let _disconnect = Disconnect(&store, &session);

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    listener.set_nonblocking(true).unwrap();
    test_server::use_loopback(listener.local_addr().unwrap());
    let requests_before = telemetry::received().api_requests;
    let cold_started = Instant::now();
    let response = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("catalog request was not received: {error}"),
            }
        };
        assert!(cold_started.elapsed() >= DEBOUNCE);
        stream.set_nonblocking(false).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let mut request = BufReader::new(&stream);
        let mut line = String::new();
        request.read_line(&mut line).unwrap();
        assert!(
            line.starts_with("GET /search?") && line.contains("type=track"),
            "{line}"
        );
        loop {
            line.clear();
            request.read_line(&mut line).unwrap();
            if line.trim().is_empty() {
                break;
            }
        }
        let body = r#"{"result":{"tracks":{"results":[{"id":42,"title":"Fixture Song","artists":[{"id":71,"name":"Fixture Artist"}]}]}}}"#;
        write!(stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len(),
        ).unwrap();
    });

    let read = |kind, query, started, cancelled| {
        let events = Mutex::new(Vec::new());
        search_catalogs(
            kind,
            query,
            Dispatch::Submitted(started),
            cancelled,
            |event| {
                events.lock().unwrap().push(event);
            },
        )
        .unwrap();
        events.into_inner().unwrap()
    };
    let flag = AtomicBool::new(false);
    let cold = read(ObjectKind::Track, "Fixture Song", cold_started, &flag);
    let cold_elapsed = cold_started.elapsed();
    response.join().unwrap();
    assert_eq!(telemetry::received().api_requests - requests_before, 1);
    let [Ok(cold)] = cold.as_slice() else {
        panic!("expected one successful provider batch")
    };
    assert!(cold.complete);
    assert_eq!(cold.pages[0].page.title, "Fixture Song");

    let warm_started = Instant::now();
    let warm = read(ObjectKind::Track, "  FIXTURE\tSONG ", warm_started, &flag);
    let warm_elapsed = warm_started.elapsed();
    assert_eq!(telemetry::received().api_requests - requests_before, 1);
    let [Ok(warm)] = warm.as_slice() else {
        panic!("expected one warm provider batch")
    };
    assert!(warm.complete && Arc::ptr_eq(&cold.pages, &warm.pages));

    let mut cache = lock_cache();
    let entry = cache.entries.front_mut().unwrap();
    let cold_key = entry.key.clone();
    entry.loaded = Instant::now() - FRESH;
    drop(cache);
    let stale = read(ObjectKind::Track, "Fixture Song", Instant::now(), &flag);
    assert!(
        matches!(stale.as_slice(), [Ok(old), Err(_)] if !old.complete
        && Arc::ptr_eq(&old.pages, &cold.pages))
    );

    let stale_other_kind = read(ObjectKind::Album, "Fixture Song", Instant::now(), &flag);
    assert!(matches!(stale_other_kind.as_slice(), [Err(_)]));

    // Reconnecting this profile rotates its credential reference without
    // changing the provider, profile id or query.
    let reconnected = Session {
        credentials: Credentials::YandexMusic {
            access_token: "fixture-search-reconnected".into(),
            refresh_token: None,
            access_expires_at: None,
        },
        ..session.clone()
    };
    assert!(
        store
            .update_account(&reconnected, &cold_key.account, || Some(()))
            .unwrap()
    );
    let previous_profile = read(ObjectKind::Track, "Fixture Song", Instant::now(), &flag);
    assert!(matches!(previous_profile.as_slice(), [Err(_)]));

    let mut cache = lock_cache();
    let now = Instant::now();
    assert!(cache.read(&cold_key, now + STALE).is_none());
    assert!(cache.entries.is_empty());
    cache.insert(cold_key.clone(), Arc::from([]), 1, now);
    assert!(cache.read(&cold_key, now + EMPTY / 2).is_some());
    assert!(cache.read(&cold_key, now + EMPTY).is_none());

    let next = Key {
        query: "other query".into(),
        ..cold_key.clone()
    };
    cache.insert(cold_key.clone(), cold.pages.clone(), MAX_BYTES / 2 + 1, now);
    cache.insert(next.clone(), cold.pages.clone(), MAX_BYTES / 2 + 1, now);
    assert!(cache.read(&cold_key, now).is_none());
    assert!(cache.read(&next, now).is_some());

    println!(
        "search cold with debounce {:.2} ms, warm {:.2} ms, one successful catalog request",
        cold_elapsed.as_secs_f64() * 1000.,
        warm_elapsed.as_secs_f64() * 1000.
    );
}
