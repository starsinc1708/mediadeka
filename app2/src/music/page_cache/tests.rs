use super::*;
use crate::providers::ProviderId;

const REGION: &str = "ZZ";

fn album() -> ObjectRef {
    ObjectRef {
        provider: ProviderId::YouTubeMusic,
        kind: ObjectKind::Album,
        id: "MPRE-offline-catalog-fixture".into(),
    }
}

#[test]
fn loading_fresh_album_again_reuses_one_resident_model() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let object = album();
    let page = MusicPage::new(object.clone(), "Первый альбом".into());
    let flag = AtomicBool::new(false);
    let query = PageQuery::resolve(&store, &object, None, REGION).unwrap();
    let first = query
        .refresh_with(&flag, |credentials| {
            assert!(credentials.is_none());
            Ok(page)
        })
        .unwrap();
    let reopened = Store::open(directory.path()).unwrap();
    let query = PageQuery::resolve(&reopened, &object, None, REGION).unwrap();
    let cached = query.cached().unwrap().unwrap();
    assert!(cached.fresh(ObjectKind::Album));
    assert!(Arc::ptr_eq(&cached.data, &first.data));
    assert_eq!(query.receipt(&cached), query.receipt(&first));
    let second = query
        .refresh_with(&flag, |_| panic!("a fresh album started another request"))
        .unwrap();
    assert!(Arc::ptr_eq(&first.data, &second.data));
}

#[test]
fn stale_snapshot_remains_available_when_revalidation_fails() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let object = album();
    let previous = MusicPage::new(object.clone(), "Сохранённый альбом".into());
    store
        .save_music_snapshot(
            object.provider,
            None,
            REGION,
            "album:MPRE-offline-catalog-fixture",
            &previous,
        )
        .unwrap();
    rusqlite::Connection::open(directory.path().join("mediadeka.sqlite3"))
        .unwrap()
        .execute("UPDATE music_snapshot SET saved_at_ms = 0", [])
        .unwrap();
    let query = PageQuery::resolve(&store, &object, None, REGION).unwrap();
    let cached = query.cached().unwrap().unwrap();
    assert!(!cached.fresh(ObjectKind::Album));
    assert_eq!(cached.data.as_ref(), &previous);
    let error = query
        .refresh_with(&AtomicBool::new(false), |_| {
            Err(access_error(ErrorKind::Network, "Нет соединения."))
        })
        .unwrap_err();
    assert_eq!(error.kind, ErrorKind::Network);
    assert!(Arc::ptr_eq(
        &query.cached().unwrap().unwrap().data,
        &cached.data
    ));
}

#[test]
fn memory_budget_evicts_old_pages_without_affecting_durable_snapshots() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let query = PageQuery::resolve(&store, &album(), None, REGION).unwrap();
    let make = |title: &str| CachedPage {
        data: Arc::new(MusicPage::new(album(), title.into())),
        modified_ms: 1,
        generation: 0,
    };
    let mut cache = ResidentCache {
        entries: VecDeque::new(),
        bytes: 0,
        generations: BTreeMap::new(),
    };
    let first = query.key;
    let second = Key {
        region: "US".into(),
        ..first.clone()
    };
    cache.insert(first.clone(), make("old"), MAX_RESIDENT_BYTES / 2 + 1);
    cache.insert(second.clone(), make("new"), MAX_RESIDENT_BYTES / 2 + 1);
    assert_eq!(cache.entries.len(), 1);
    assert!(cache.entries[0].key == second);
    cache.insert(first, make("oversized"), MAX_RESIDENT_BYTES + 1);
    assert_eq!(cache.entries.len(), 1);
    assert!(cache.entries[0].key == second);
}

#[test]
fn retired_detail_cannot_repopulate_resident_memory_after_clear() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let query = PageQuery::resolve(&store, &album(), None, REGION).unwrap();
    let mut cache = ResidentCache {
        entries: VecDeque::new(),
        bytes: 0,
        generations: BTreeMap::new(),
    };
    let old = CachedPage {
        data: Arc::new(MusicPage::new(album(), "Old".into())),
        modified_ms: 1,
        generation: 0,
    };
    cache.insert(query.key.clone(), old.clone(), 1);
    cache.retire(store.directory(), 1);
    assert!(cache.entries.is_empty());
    cache.insert(query.key, old, 1);
    assert!(cache.entries.is_empty());
}

#[cfg(windows)]
#[test]
fn production_catalog_prefetch_remains_available_offline() {
    use std::{
        cell::RefCell,
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        process::Command,
        thread,
        time::{Duration, Instant},
    };

    use crate::{
        music::{PageUpdate, load_page, now, prefetch_page, region},
        providers::{Capabilities, Profile, Session, yandex::test_server},
    };

    const CHILD: &str = "MEDIADEKA_CATALOG_CACHE_FIXTURE_CHILD";
    const TEST: &str =
        "music::page_cache::tests::production_catalog_prefetch_remains_available_offline";
    if std::env::var_os(CHILD).is_none() {
        let directory = tempfile::tempdir().unwrap();
        let status = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--nocapture"])
            .env(CHILD, "1")
            .env("MEDIADEKA_PROFILE_DIR", directory.path())
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }

    let store = Store::open_default().unwrap();
    let object = ObjectRef {
        provider: ProviderId::YandexMusic,
        kind: ObjectKind::Album,
        id: "42".into(),
    };
    let session = Session {
        provider: object.provider,
        profile: Profile {
            id: "74368".into(),
            name: "Catalog fixture".into(),
        },
        credentials: Credentials::YandexMusic {
            access_token: "fixture-catalog-access".into(),
            refresh_token: None,
            access_expires_at: None,
        },
        capabilities: Capabilities {
            home: true,
            profile_selection: false,
            device_code: true,
        },
    };
    store
        .connect_account(&session, || Some(()), || {})
        .unwrap()
        .unwrap();
    struct Disconnect<'a>(&'a Store, &'a Session);
    impl Drop for Disconnect<'_> {
        fn drop(&mut self) {
            let _ = self
                .0
                .disconnect_account(self.1.provider, &self.1.profile.id, || Some(()));
        }
    }
    let _disconnect = Disconnect(&store, &session);
    // Keep the fixture on the exact source provider and out of external link matching.
    let now = now();
    assert!(
        store
            .claim_music_match(&region(), &object, now, now + 3_600)
            .unwrap()
    );

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    test_server::use_loopback(listener.local_addr().unwrap());
    let body = r#"{"result":{"id":42,"title":"Cache fixture","artists":[{"id":71,"name":"One"},{"id":72,"name":"Two"}]}}"#;
    let response = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(4)))
            .unwrap();
        let mut request = BufReader::new(&stream);
        let mut line = String::new();
        request.read_line(&mut line).unwrap();
        assert!(
            line.starts_with("GET /albums/42/with-tracks HTTP/"),
            "{line}"
        );
        loop {
            line.clear();
            request.read_line(&mut line).unwrap();
            if line.trim().is_empty() {
                break;
            }
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len(),
        ).unwrap();
    });

    let active_profile = Some(session.profile.id.as_str());
    let requests = telemetry::received().api_requests;
    let flag = AtomicBool::new(false);
    let cold_started = Instant::now();
    let (prefetched, bytes) = telemetry::prefetch_body_bytes(|| {
        prefetch_page(&object, active_profile, &flag)
            .unwrap()
            .unwrap()
    });
    let cold_elapsed = cold_started.elapsed();
    response.join().unwrap();
    assert_eq!(bytes, body.len() as u64);
    let (already_cached, bytes) = telemetry::prefetch_body_bytes(|| {
        prefetch_page(&object, active_profile, &flag)
            .unwrap()
            .unwrap()
    });
    assert_eq!(bytes, 0);
    assert_eq!(already_cached.receipt, prefetched.receipt);
    assert_eq!(prefetched.page.title, "Cache fixture");
    assert_eq!(prefetched.page.artists, ["One", "Two"]);
    assert_eq!(telemetry::received().api_requests - requests, 1);

    let updates = RefCell::new(Vec::new());
    let warm_started = Instant::now();
    let first_data = RefCell::new(None);
    load_page(&object, active_profile, &flag, |update| {
        if matches!(&update, PageUpdate::Data(_)) {
            first_data.replace(Some(warm_started.elapsed()));
        }
        updates.borrow_mut().push(update);
    });
    assert_eq!(telemetry::received().api_requests - requests, 1);
    assert!(matches!(
        &updates.borrow()[0],
        PageUpdate::Data(data) if data.profile == prefetched.profile
            && data.receipt == prefetched.receipt
            && Arc::ptr_eq(&data.page, &prefetched.page)
    ));
    assert!(
        !updates
            .borrow()
            .iter()
            .any(|update| matches!(update, PageUpdate::Error { .. }))
    );

    // The listener is now closed. A stale read still emits the persisted page
    // before attempting the real provider request and reporting its failure.
    rusqlite::Connection::open(store.directory().join("mediadeka.sqlite3"))
        .unwrap()
        .execute("UPDATE music_snapshot SET saved_at_ms=0", [])
        .unwrap();
    updates.borrow_mut().clear();
    load_page(&object, active_profile, &flag, |update| {
        updates.borrow_mut().push(update)
    });
    assert!(matches!(
        &updates.borrow()[0],
        PageUpdate::Data(data) if data.page.as_ref() == prefetched.page.as_ref()
            && data.receipt != prefetched.receipt
    ));
    assert!(
        updates
            .borrow()
            .iter()
            .any(|update| matches!(update, PageUpdate::Error { hide: false, .. }))
    );
    let metrics = telemetry::catalog();
    assert_eq!((metrics.fresh, metrics.stale, metrics.miss), (1, 1, 0));
    assert_eq!(
        (metrics.active, metrics.failures, metrics.cancelled),
        (0, 1, 0)
    );
    assert_eq!(metrics.resident_pages, 1);
    assert!(metrics.resident_bytes > 0);
    let latency = metrics.source_latency.unwrap();
    assert!(latency.p95 >= latency.mean);
    println!(
        "catalog cold prefetch {:.2} ms, warm to model {:.2} ms, successful source requests 1",
        cold_elapsed.as_secs_f64() * 1_000.,
        first_data.into_inner().unwrap().as_secs_f64() * 1_000.,
    );
}
