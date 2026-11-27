use super::*;

fn track() -> Recording {
    Recording {
        title: "Creep".into(),
        artist: "Radiohead".into(),
        artists: Vec::new(),
        album: Some("Pablo Honey".into()),
        duration_ms: Some(238_000),
    }
}

fn hit(id: u64, title: &str, artist: &str, duration: Option<f64>) -> Hit {
    serde_json::from_value(serde_json::json!({
        "type": "song",
        "result": {
            "id": id,
            "title": title,
            "artist_names": artist,
            "duration": duration,
        }
    }))
    .unwrap()
}

#[test]
fn requires_exact_version_artist_and_unique_genius_song() {
    let version = hit(1, "Creep (Live)", "Radiohead", None);
    let artist = hit(2, "Creep", "Radiohead Tribute", None);
    let length = hit(3, "Creep", "Radiohead", Some(190.));
    let correct = hit(4, "Creep", "Radiohead", Some(238.));
    assert_eq!(
        unique_hit(&track(), vec![version, artist, length, correct]),
        matching::SearchMatch::Unique(4)
    );
    assert_eq!(
        unique_hit(
            &track(),
            vec![
                hit(4, "Creep", "Radiohead", None),
                hit(5, "Creep", "Radiohead", None)
            ],
        ),
        matching::SearchMatch::Ambiguous
    );
}

#[test]
fn matches_original_with_latin_aliases_without_accepting_other_versions() {
    let track = Recording {
        title: "ты че обиделась".into(),
        artist: "madk1d, тёмный принц".into(),
        artists: Vec::new(),
        album: None,
        duration_ms: Some(90_000),
    };
    let original = || {
        hit(
            11224347,
            "ты че обиделась (are you offended)",
            "madk1d & тёмный принц (darkprinceee)",
            None,
        )
    };
    let artist = "madk1d & тёмный принц (darkprinceee)";
    assert_eq!(
        unique_hit(
            &track,
            vec![
                hit(1, "ты че обиделась (Live at Home)", artist, None),
                hit(2, "ты че обиделась (Bootleg Remix)", artist, None),
                hit(3, "ты че обиделась (are you offended)", "madk1d", None),
                hit(4, "ты че обиделась (are you offended)", artist, Some(150.)),
                hit(
                    5,
                    "ты че обиделась (English Translation)",
                    "Genius English Translations",
                    None
                ),
                original(),
            ]
        ),
        matching::SearchMatch::Unique(11224347)
    );
    assert_eq!(
        unique_hit(
            &track,
            vec![
                original(),
                hit(6, "ты че обиделась (are you offended)", artist, None),
            ]
        ),
        matching::SearchMatch::Ambiguous
    );
}

#[test]
fn matches_structured_featured_credits_and_single_word_translation() {
    let track = Recording {
        title: "ДЭФО".into(),
        artist: "The Pak".into(),
        artists: Vec::new(),
        duration_ms: Some(125_000),
        album: None,
    };
    let song = |id, title, features| Hit {
        kind: "song".into(),
        result: serde_json::json!({
            "id": id,
            "title": title,
            "artist_names": "unstructured credits",
            "primary_artists": [{"name":"the pak (RUS)"}],
            "featured_artists": features,
        }),
    };
    let no_feature = serde_json::json!([]);
    assert_eq!(
        unique_hit(
            &track,
            vec![
                song(1, "ДЭФО (Remix)", no_feature.clone()),
                song(2, "ДЭФО (DAFOE)", serde_json::json!([{"name":"ЛАУД"}])),
                song(13430149, "ДЭФО (DAFOE)", no_feature),
            ]
        ),
        matching::SearchMatch::Unique(13430149)
    );
}

#[test]
fn genius_applies_complete_listed_credits_to_wassup() {
    let mut recording = Recording {
        title: "WASSUP (feat. JPEGMAFIA)".into(),
        artist: "Joey Valence & Brae, JPEGMAFIA".into(),
        artists: ["Joey Valence & Brae", "JPEGMAFIA"]
            .map(str::to_owned)
            .into(),
        album: Some("WASSUP (feat. JPEGMAFIA)".into()),
        duration_ms: Some(180_000),
    };
    // Credits from the real Genius search and song responses for 11998459.
    let song = |primary: &[&str], featured: &[&str]| Hit {
        kind: "song".into(),
        result: serde_json::json!({
            "id":11998459, "title":"WASSUP",
            "artist_names":"Joey Valence & Brae (Ft. JPEGMAFIA)",
            "primary_artists":primary.iter().map(|name| serde_json::json!({"name":name})).collect::<Vec<_>>(),
            "featured_artists":featured.iter().map(|name| serde_json::json!({"name":name})).collect::<Vec<_>>(),
            "album":{"name":"HYPERYOUTH"},
        }),
    };
    assert_eq!(
        unique_hit(
            &recording,
            vec![song(&["Joey Valence & Brae"], &["JPEGMAFIA"])]
        ),
        SearchMatch::Unique(11998459)
    );
    assert_eq!(
        unique_hit(
            &recording,
            vec![song(&["Joey Valence", "Brae"], &["JPEGMAFIA"])]
        ),
        SearchMatch::Missing
    );
    assert_eq!(
        unique_hit(&recording, vec![song(&["Joey Valence & Brae"], &[])]),
        SearchMatch::Missing
    );
    recording.artists.clear();
    assert_eq!(
        unique_hit(
            &recording,
            vec![song(&["Joey Valence & Brae"], &["JPEGMAFIA"])]
        ),
        SearchMatch::Unique(11998459)
    );
}

#[test]
fn matches_genius_album_alias_and_same_title_single() {
    let mut track = Recording {
        title: "Рэп".into(),
        artist: "MC Senechka".into(),
        artists: Vec::new(),
        album: Some("Хип-хоп будни".into()),
        duration_ms: Some(141_000),
    };
    let response = |album: &str| {
        let mut result = hit(4474584, "Рэп (Rap)", "МС Сенечка (MC Senechka)", None);
        result.result["album"] = serde_json::json!({"name":album});
        result
    };
    assert_eq!(
        unique_hit(&track, vec![response("Хип-хоп будни (Days Of Hip-Hop)")]),
        matching::SearchMatch::Unique(4474584)
    );
    assert_eq!(
        unique_hit(&track, vec![response("Different Record")]),
        matching::SearchMatch::Missing
    );
    track.album = Some("Рэп".into());
    assert_eq!(
        unique_hit(&track, vec![response("Хип-хоп будни (Days Of Hip-Hop)")]),
        matching::SearchMatch::Unique(4474584)
    );
}

#[test]
fn access_key_canonicalizes_the_song_page_and_rejects_other_hosts() {
    assert_eq!(
        song_url("https://www.genius.com/Radiohead-creep-lyrics?ref=track#part")
            .unwrap()
            .as_str(),
        "https://genius.com/Radiohead-creep-lyrics"
    );
    assert!(song_url("https://genius.com@example.net/Radiohead-creep-lyrics").is_none());
    assert!(song_url("https://genius.com:8080/Radiohead-creep-lyrics").is_none());
}

#[cfg(windows)]
#[test]
#[ignore = "requires an existing Genius connection token in MEDIADEKA_GENIUS_TEST_TOKEN"]
fn live_genius_connection_open_credits_and_annotations() {
    use crate::{music::lyrics::genius, storage::Store};

    let token = std::env::var("MEDIADEKA_GENIUS_TEST_TOKEN").expect("Genius token environment");
    let profile = tempfile::tempdir().unwrap();
    let store = Store::open(profile.path()).unwrap();

    struct Connection<'a>(&'a Store);
    impl Drop for Connection<'_> {
        fn drop(&mut self) {
            let _ = self.0.set_genius_token(None);
        }
    }
    let cleanup = Connection(&store);
    genius::connect(&store, &token).expect("validate and store active Genius connection");
    let cancelled = AtomicBool::new(false);
    let song = genius::load(&store, &track(), &cancelled, |_| {})
        .expect("Genius API details")
        .expect("same Genius song");
    assert_eq!(song.title, "Creep");
    assert_eq!(song.artist, "Radiohead");
    assert!(song.lyrics.is_none());
    assert!(!song.writers.is_empty());
    assert!(
        song.annotations
            .iter()
            .any(|note| !note.fragment.is_empty() && !note.text.is_empty())
    );
    eprintln!(
        "Genius · {} · {} · {} writer credits · {} notes",
        song.artist,
        song.title,
        song.writers.len(),
        song.annotations.len()
    );
    if let Some(capture) = std::env::var_os("MEDIADEKA_GENIUS_CAPTURE_EXECUTABLE") {
        let output = std::env::var_os("MEDIADEKA_GENIUS_CAPTURE_DIRECTORY").unwrap();
        let audio = std::env::var_os("MEDIADEKA_GENIUS_CAPTURE_AUDIO").unwrap();
        let fragment = &song
            .annotations
            .iter()
            .min_by_key(|note| note.fragment.len())
            .unwrap()
            .fragment;
        let mut command = std::process::Command::new(capture);
        command
            .arg("--lyrics-genius-stage6")
            .arg(output)
            .arg(audio)
            .env("MEDIADEKA_PROFILE_DIR", profile.path())
            .env("MEDIADEKA_GENIUS_CAPTURE_FRAGMENT", fragment)
            .env_remove("MEDIADEKA_GENIUS_TEST_TOKEN");
        let status = command.status().expect("start the Genius capture window");
        assert!(status.success(), "capture exited with {status}");

        let offline_profile = tempfile::tempdir().unwrap();
        let offline = Store::open(offline_profile.path()).unwrap();
        let _offline_cleanup = Connection(&offline);
        offline.set_genius_token(Some(&token)).unwrap();
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("http://{}", socket.local_addr().unwrap());
        drop(socket);
        let status = command
            .env("MEDIADEKA_PROFILE_DIR", offline_profile.path())
            .env("MEDIADEKA_GENIUS_CAPTURE_OFFLINE", "1")
            .env("HTTPS_PROXY", &proxy)
            .env("HTTP_PROXY", &proxy)
            .env("ALL_PROXY", &proxy)
            .env("NO_PROXY", "")
            .status()
            .expect("start Genius capture with unavailable network");
        assert!(status.success(), "offline capture exited with {status}");
    }
    genius::set_access(&store, &song, genius::Access::Link).unwrap();
    assert!(!song.is_current(&store));
    let revoked = genius::load(&store, &track(), &cancelled, |_| {})
        .unwrap()
        .unwrap();
    assert!(revoked.revoked && revoked.lyrics.is_none() && revoked.annotations.is_empty());
    assert_eq!(revoked.writers, song.writers);
    drop(cleanup);
    assert!(!store.genius_connected().unwrap());
    assert!(!revoked.is_current(&store));
}
