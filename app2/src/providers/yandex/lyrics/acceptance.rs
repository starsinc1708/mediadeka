use std::{env, fs, net::TcpListener, process::Command, time::Instant};

use crate::{
    music::{ObjectKind, ObjectRef, lyrics},
    playback::{QueueEntry, SoundSource},
    playlist::PlaylistKey,
    providers::{Credentials, catalog, live_tests::profile_snapshot},
    storage::Store,
    telemetry,
};

use super::*;

const PROFILE: &str = "MEDIADEKA_LYRICS_TEST_PROFILE";

fn recording_id() -> String {
    env::var("MEDIADEKA_LYRICS_YANDEX_TRACK").unwrap_or_else(|_| "10741527:1612097".into())
}

#[test]
#[ignore = "reads both live lyric formats through a copy of a connected profile"]
fn live_yandex_returns_independent_lrc_and_text() {
    let profile = profile_snapshot(PROFILE);
    let store = Store::open(profile.path()).expect("isolated state");
    let account = store
        .preferred_account(PROVIDER)
        .expect("preferred account")
        .expect("connected Yandex account in supplied profile");
    let Credentials::YandexMusic { access_token, .. } = store
        .credentials(&account)
        .expect("stored credentials")
        .expect("active credentials")
    else {
        panic!("expected Yandex credentials");
    };
    let id = recording_id();
    assert!(valid_track_id(&id));
    let client = agent();
    let read = |format: Format| {
        let start = Instant::now();
        let deadline = start + FORMAT_BUDGET;
        let preference = format.preference();
        let result = descriptor(&client, &access_token, &id, format, deadline)
            .unwrap_or_else(|error| panic!("{preference} descriptor: {}", error.message))
            .unwrap_or_else(|| panic!("{preference} supplied no descriptor"));
        let payload = text(&client, &result, deadline)
            .unwrap_or_else(|error| panic!("{preference} payload: {}", error.message))
            .unwrap_or_else(|| panic!("{preference} supplied no payload"));
        println!(
            "Yandex {id} {preference}: successful descriptor, {} payload, {} bytes in {} ms",
            if result["downloadUrl"].is_string() {
                "downloaded"
            } else {
                "inline"
            },
            payload.len(),
            start.elapsed().as_millis(),
        );
        payload
    };
    let lrc = read(Format::Lrc);
    let plain = read(Format::Text);
    let timed = parse_lrc(Source::YandexMusic, &lrc).expect("timed lyric payload");
    let plain_lines = Lyrics::plain(Source::YandexMusic, &plain);
    println!(
        "Yandex {id}: {} timed lines, {} plain lines",
        timed
            .lines
            .iter()
            .filter(|line| line.time_ms.is_some())
            .count(),
        plain_lines.lines.len(),
    );
    assert_ne!(lrc, plain);
    assert!(timed.lines.iter().any(|line| line.time_ms.is_some()));
    assert!(parse_lrc(Source::YandexMusic, &plain).is_none());
    assert!(plain_lines.usable());
}

#[test]
#[ignore = "requires a live Yandex account, then reopens the cached formats with network blocked"]
fn live_yandex_dual_snapshot_reopens_offline() {
    const CHILD: &str = "MEDIADEKA_TEST_YANDEX_LYRICS_RESTART";
    const TEST: &str =
        "providers::yandex::lyrics::acceptance::live_yandex_dual_snapshot_reopens_offline";
    const ENTRY: &str = "entry.json";
    let cancelled = AtomicBool::new(false);
    let id = recording_id();

    if let Some(path) = env::var_os(CHILD) {
        assert!(
            agent()
                .get(&super::super::api_endpoint("/tracks"))
                .call()
                .is_err(),
            "network block did not apply to the native adapter"
        );
        let store = Store::open(&path).unwrap();
        let entry =
            serde_json::from_slice(&fs::read(store.directory().join(ENTRY)).unwrap()).unwrap();
        let before = telemetry::received().api_requests;
        let versions = resolved(&store, &entry, &cancelled);
        report(&id, "Offline restart", &versions);
        assert_eq!(telemetry::received().api_requests, before);
        return;
    }

    let isolated = profile_snapshot(PROFILE);
    let store = Store::open(isolated.path()).unwrap();
    let account = store.preferred_account(PROVIDER).unwrap().unwrap();
    let credentials = store.credentials(&account).unwrap().unwrap();
    let reference = ObjectRef::native(PROVIDER, ObjectKind::Track, id.clone()).unwrap();
    let track = catalog::read(&reference, Some(&credentials), &cancelled)
        .unwrap()
        .tracks
        .into_iter()
        .next()
        .unwrap();
    let entry = QueueEntry {
        playlist: PlaylistKey::Source {
            provider: PROVIDER,
            profile: Some(account.profile.id),
            id: format!("track:{id}"),
        },
        playlist_revision: None,
        index: 0,
        track,
        recommendations: Vec::new(),
    };
    rusqlite::Connection::open(isolated.path().join("mediadeka.sqlite3"))
        .unwrap()
        .execute(
            "DELETE FROM lyrics_snapshot WHERE provider=?1",
            [PROVIDER.as_str()],
        )
        .unwrap();
    let before = telemetry::received().api_requests;
    let start = Instant::now();
    let versions = resolved(&store, &entry, &cancelled);
    assert_eq!(telemetry::received().api_requests - before, 4);
    report(&id, "Online resolver, 4 requests", &versions);
    println!("Both versions ready in {} ms", start.elapsed().as_millis());
    assert_eq!(resolved(&store, &entry, &cancelled), versions);
    assert_eq!(telemetry::received().api_requests - before, 4);
    fs::write(
        isolated.path().join(ENTRY),
        serde_json::to_vec(&entry).unwrap(),
    )
    .unwrap();
    drop(store);

    let port = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = format!("http://{}", port.local_addr().unwrap());
    drop(port);
    assert!(
        Command::new(env::current_exe().unwrap())
            .args(["--exact", TEST, "--ignored", "--nocapture"])
            .env(CHILD, isolated.path())
            .env("HTTPS_PROXY", &proxy)
            .env("HTTP_PROXY", &proxy)
            .env("ALL_PROXY", &proxy)
            .env("NO_PROXY", "")
            .status()
            .unwrap()
            .success()
    );
}

fn resolved(store: &Store, entry: &QueueEntry, cancelled: &AtomicBool) -> LyricsVersions {
    let mut latest = None;
    lyrics::load(
        store,
        entry,
        Some(&SoundSource::Stream(PROVIDER)),
        Some(Source::YandexMusic),
        cancelled,
        |versions| latest = Some(versions),
    )
    .unwrap();
    latest.expect("native Yandex variants")
}

fn report(id: &str, operation: &str, versions: &LyricsVersions) {
    let timed = versions.preferred();
    let plain = versions
        .alternate_plain()
        .expect("separately supplied TEXT");
    assert_eq!(timed.source, Source::YandexMusic);
    assert_eq!(plain.source, Source::YandexMusic);
    let timed_rows = timed
        .lines
        .iter()
        .filter(|line| line.time_ms.is_some())
        .count();
    assert!(timed_rows > 20);
    assert!(plain.lines.len() > 20);
    assert!(
        plain
            .lines
            .iter()
            .all(|line| line.time_ms.is_none() && line.words.is_empty())
    );
    println!(
        "{operation}: Yandex {id}, {timed_rows} timed lines, {} plain lines",
        plain.lines.len()
    );
}
