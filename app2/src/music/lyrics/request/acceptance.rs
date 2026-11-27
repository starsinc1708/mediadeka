use crate::playlist::{PlaylistKey, PlaylistTrack};

use super::*;

#[cfg(windows)]
mod youtube;

#[test]
fn local_sidecar_revisions_preserve_distinct_plain_and_upgrade_timing() {
    use std::fs;

    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();
    let audio = profile.path().join("song.wav");
    fs::write(&audio, []).unwrap();
    let ttml = audio.with_extension("ttml");
    fs::write(&ttml, r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div><p begin="0s">TTML line</p></div></body></tt>"#).unwrap();
    let lrc = audio.with_extension("lrc");
    fs::write(&lrc, "[00:00.00]LRC line").unwrap();
    let txt = audio.with_extension("txt");
    fs::write(&txt, "First plain verse\nSecond plain verse").unwrap();
    let entry = QueueEntry {
        playlist: PlaylistKey::Local(1),
        playlist_revision: None,
        recommendations: Vec::new(),
        index: 0,
        track: PlaylistTrack {
            id: "local-sidecar-revision".into(),
            title: "Song".into(),
            artist: "Fixture".into(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: None,
            media: Some(Media::File(audio)),
        },
    };
    let check = || {
        let mut loaded = None;
        load(
            &store,
            &entry,
            None,
            Some(Source::Local),
            &AtomicBool::new(false),
            |versions| {
                loaded = Some(versions);
            },
        )
        .unwrap();
        loaded.unwrap()
    };

    let first = check();
    assert_eq!(first.preferred().timing(), Timing::Lines);
    assert_eq!(first.preferred().lines[0].text, "TTML line");
    assert_eq!(first.alternate_plain().unwrap().lines.len(), 2);

    fs::write(lrc, "[00:00.00]<00:00.00>Enhanced <00:00.50>line").unwrap();
    fs::write(&txt, "New first verse\nNext verse\nFinal verse").unwrap();
    let edited = check();
    assert_eq!(edited.preferred().timing(), Timing::Words);
    assert_eq!(edited.preferred().lines[0].text, "Enhanced line");
    let plain = edited.alternate_plain().unwrap();
    assert_eq!(plain.lines.len(), 3);
    assert_eq!(plain.lines[0].text, "New first verse");
    assert!(
        plain
            .lines
            .iter()
            .all(|line| line.time_ms.is_none() && line.words.is_empty())
    );
    assert_eq!(check(), edited);

    fs::remove_file(txt).unwrap();
    assert!(check().alternate_plain().is_none());
}

#[test]
#[ignore = "requires live LRCLIB to create a snapshot before an offline restart"]
fn live_stage6_lyrics_cache_reopens_offline() {
    const CHILD_PROFILE: &str = "MEDIADEKA_TEST_OFFLINE_LYRICS_PROFILE";
    const TEST: &str =
        "music::lyrics::request::acceptance::live_stage6_lyrics_cache_reopens_offline";

    let item = QueueEntry {
        playlist: PlaylistKey::Local(1),
        playlist_revision: None,
        recommendations: Vec::new(),
        index: 0,
        track: PlaylistTrack {
            id: "lrclib-cache-test".into(),
            title: "I Want to Live".into(),
            artist: "Borislav Slavov".into(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: Some("Baldur's Gate 3 (Original Game Soundtrack)".into()),
            artwork: None,
            added: None,
            duration: Some(233),
            media: None,
        },
    };
    let cancelled = AtomicBool::new(false);
    let check = |store: &Store| {
        let mut count = None;
        load(
            store,
            &item,
            None,
            Some(Source::Lrclib),
            &cancelled,
            |versions| {
                let plain = versions.alternate_plain().expect("LRCLIB plain version");
                assert!(plain.lines.iter().all(|line| line.time_ms.is_none()));
                count = Some((
                    versions
                        .preferred()
                        .lines
                        .iter()
                        .filter(|line| line.time_ms.is_some())
                        .count(),
                    plain.lines.len(),
                ));
            },
        )
        .unwrap();
        let count = count.expect("both lyric forms");
        assert!(count.0 > 20 && count.1 > 20);
        count
    };
    if let Some(directory) = std::env::var_os(CHILD_PROFILE) {
        assert!(
            lrclib::fetch(&Recording::from_entry(&item), None, &cancelled).is_err(),
            "verification proxy did not block a direct fetch"
        );
        let store = Store::open(directory).unwrap();
        let (timed, plain) = check(&store);
        println!("Offline restart: {timed} timed and {plain} plain lines from SQLite");
        return;
    }

    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();
    let (timed, plain) = check(&store);
    println!("Online request: {timed} timed and {plain} plain lines");
    drop(store);

    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = format!("http://{}", probe.local_addr().unwrap());
    drop(probe);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", TEST, "--ignored", "--nocapture"])
        .env(CHILD_PROFILE, profile.path())
        .env("HTTPS_PROXY", &proxy)
        .env("HTTP_PROXY", &proxy)
        .env("ALL_PROXY", &proxy)
        .env("NO_PROXY", "")
        .status()
        .unwrap();
    assert!(status.success());
}
