use std::{env, net::TcpListener, process::Command};

use crate::{music::lyrics::Lyrics, providers::Capabilities, telemetry};

use super::*;

const PUBLIC: &str = "DyDfgMOUjCI";
const DISABLED: &str = "CvFH_6DNRCY";
const LEGACY: &str = "legacy_1234";
const PROVIDER: ProviderId = ProviderId::YouTubeMusic;
const SOURCE: Source = Source::YouTubeMusic;

#[test]
#[ignore = "reads public YouTube Music lyrics, then checks a separate offline process"]
fn public_youtube_lyrics_reopen_offline_and_keep_old_profiles_isolated() {
    const CHILD: &str = "MEDIADEKA_TEST_OFFLINE_YOUTUBE_LYRICS";
    const TEST: &str = "music::lyrics::request::acceptance::youtube::public_youtube_lyrics_reopen_offline_and_keep_old_profiles_isolated";
    if let Some(path) = env::var_os(CHILD) {
        offline(&Store::open(path).unwrap());
        return;
    }

    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();
    assert!(store.preferred_account(PROVIDER).unwrap().is_none());
    let start = std::time::Instant::now();
    let before = telemetry::received().api_requests;
    let public = entry(PUBLIC, None);
    let versions = resolve(&store, &public).unwrap().unwrap();
    let requests = telemetry::received().api_requests - before;
    assert_eq!(requests, 2);
    println!(
        "Cold guest resolver: {} plain lines, source attribution, {requests} HTTP, {} ms",
        plain_count(&versions),
        start.elapsed().as_millis()
    );

    let mut differently_labelled = public;
    differently_labelled.track.title = "Different catalogue title".into();
    assert!(resolve(&store, &differently_labelled).unwrap().as_ref() == Some(&versions));
    assert_eq!(telemetry::received().api_requests - before, 2);

    let disabled = entry(DISABLED, None);
    let before_missing = telemetry::received().api_requests;
    assert!(resolve(&store, &disabled).unwrap().is_none());
    assert!(resolve(&store, &disabled).unwrap().is_none());
    assert_eq!(telemetry::received().api_requests - before_missing, 1);
    println!("Exact disabled lyrics tab: one HTTP for two opens");
    drop(store);

    let port = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = format!("http://{}", port.local_addr().unwrap());
    drop(port);
    assert!(
        Command::new(env::current_exe().unwrap())
            .args(["--exact", TEST, "--ignored", "--nocapture"])
            .env(CHILD, profile.path())
            .env("HTTPS_PROXY", &proxy)
            .env("HTTP_PROXY", &proxy)
            .env("ALL_PROXY", &proxy)
            .env("NO_PROXY", "")
            .status()
            .unwrap()
            .success()
    );
}

fn offline(store: &Store) {
    let before = telemetry::received().api_requests;
    let versions = resolve(store, &entry(PUBLIC, None)).unwrap().unwrap();
    assert_eq!(telemetry::received().api_requests, before);
    println!(
        "Offline process: {} cached plain lines, zero HTTP",
        plain_count(&versions)
    );

    let cold = entry(LEGACY, None);
    for _ in 0..2 {
        assert!(resolve(store, &cold).is_err());
    }
    let attempts = telemetry::received().api_requests - before;
    assert_eq!(attempts, 2);
    println!("Network refused: two errors and {attempts} attempts, no cached absence");

    let database = rusqlite::Connection::open(store.directory().join("mediadeka.sqlite3")).unwrap();
    database
        .execute(
            "UPDATE lyrics_snapshot SET refresh_at=0 WHERE key=?1 AND provider=''",
            [native_key(SOURCE, PUBLIC).unwrap()],
        )
        .unwrap();
    assert!(resolve(store, &entry(PUBLIC, None)).unwrap().as_ref() == Some(&versions));
    assert_eq!(telemetry::received().api_requests - before, 3);
    println!("Expired refresh window: one failed revalidation kept the earlier public text");

    let scoped_profile = "lyrics-test-profile";
    let credential_ref = "lyrics-test-reference";
    let capabilities = Capabilities {
        home: true,
        profile_selection: true,
        device_code: false,
    };
    database
        .execute(
            "INSERT INTO account VALUES (?1, ?2, 'Test scope', ?3, ?4)",
            rusqlite::params![
                PROVIDER.as_str(),
                scoped_profile,
                serde_json::to_string(&capabilities).unwrap(),
                credential_ref
            ],
        )
        .unwrap();
    database
        .execute(
            "INSERT INTO account_source VALUES (?1, ?2)",
            [PROVIDER.as_str(), scoped_profile],
        )
        .unwrap();
    let account = store.account(PROVIDER, scoped_profile).unwrap().unwrap();
    let old = LyricsVersions::from_source(Lyrics::plain(SOURCE, "Historical fixture"), None);
    let key = native_key(SOURCE, LEGACY).unwrap();
    assert!(
        store
            .save_lyrics_snapshot(&key, Some(&account), http_cache::now(), 600, &old)
            .unwrap()
    );
    let scoped = entry(LEGACY, Some(scoped_profile));
    assert_eq!(resolve(store, &scoped).unwrap().unwrap(), old);
    assert!(resolve(store, &cold).is_err());
    assert!(resolve(store, &entry(LEGACY, Some("other-profile"))).is_err());
    assert!(
        store
            .lyrics_snapshot(&key, None, http_cache::now())
            .unwrap()
            .is_none()
    );
    database
        .execute(
            "INSERT INTO music_revoked VALUES (?1, ?2, ?3)",
            [PROVIDER.as_str(), scoped_profile, credential_ref],
        )
        .unwrap();
    assert!(resolve(store, &scoped).is_err());
    println!(
        "Historical snapshot: visible only in its original active scope, never copied to the public key"
    );
}

fn plain_count(versions: &LyricsVersions) -> usize {
    let sheet = versions.preferred();
    assert_eq!(sheet.source, SOURCE);
    assert!(sheet.attribution.is_some());
    assert_eq!(sheet.timing(), Timing::Plain);
    assert!(versions.alternate_plain().is_none() && sheet.lines.len() > 20);
    sheet.lines.len()
}

fn resolve(store: &Store, entry: &QueueEntry) -> Result<Option<LyricsVersions>, String> {
    let mut found = None;
    load(
        store,
        entry,
        None,
        Some(SOURCE),
        &AtomicBool::new(false),
        |versions| found = Some(versions),
    )?;
    Ok(found)
}

fn entry(id: &str, profile: Option<&str>) -> QueueEntry {
    QueueEntry {
        playlist: PlaylistKey::Source {
            provider: PROVIDER,
            profile: profile.map(str::to_owned),
            id: format!("track:{id}"),
        },
        playlist_revision: None,
        recommendations: Vec::new(),
        index: 0,
        track: PlaylistTrack {
            id: id.into(),
            title: "Exact YouTube Music recording".into(),
            artist: "Public artist".into(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: None,
            media: None,
        },
    }
}
