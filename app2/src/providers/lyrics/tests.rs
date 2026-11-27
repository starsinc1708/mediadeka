use std::time::Instant;

use crate::{music::lyrics::Source, providers::live_tests::profile_snapshot, storage::Store};

use super::*;

fn native_lyrics(provider: ProviderId, source: Source, id: &str, expected_timing: bool) {
    let isolated = profile_snapshot("MEDIADEKA_LYRICS_TEST_PROFILE");
    let store = Store::open(isolated.path()).expect("isolated state");
    let account = store
        .preferred_account(provider)
        .expect("preferred account")
        .expect("connected account in the supplied profile");
    let credentials = store.credentials(&account).unwrap().unwrap();
    assert!(supports(&credentials));
    let start = Instant::now();
    let versions = fetch(provider, &credentials, id, &AtomicBool::new(false))
        .unwrap_or_else(|error| panic!("{provider:?} {id}: {error}"))
        .unwrap_or_else(|| panic!("{provider:?} {id} returned no lyrics"));
    let sheet = versions.preferred();
    let timed = sheet.lines.iter().filter_map(|line| line.time_ms).count();
    let words = sheet
        .lines
        .iter()
        .map(|line| line.words.len())
        .sum::<usize>();
    assert_eq!(sheet.source, source);
    assert!(sheet.lines.len() >= 5);
    assert_eq!(timed > 0, expected_timing);
    println!(
        "{provider:?} {id}: {} lines, {timed} timed, {words} timed words, obtained in {} ms",
        sheet.lines.len(),
        start.elapsed().as_millis(),
    );
}

#[test]
#[ignore = "requires a connected Yandex Music profile in MEDIADEKA_LYRICS_TEST_PROFILE"]
fn live_stage6_native_yandex_lyrics() {
    let id = std::env::var("MEDIADEKA_LYRICS_YANDEX_TRACK")
        .unwrap_or_else(|_| "10741527:1612097".into());
    native_lyrics(ProviderId::YandexMusic, Source::YandexMusic, &id, true);
}

#[test]
#[ignore = "requires a connected Spotify profile in MEDIADEKA_LYRICS_TEST_PROFILE"]
fn live_stage6_native_spotify_lyrics() {
    let id = std::env::var("MEDIADEKA_LYRICS_SPOTIFY_TRACK")
        .unwrap_or_else(|_| "2Fxmhks0bxGSBdJ92vM42m".into());
    native_lyrics(ProviderId::Spotify, Source::Spotify, &id, true);
}

#[test]
#[ignore = "reads public lyrics directly from YouTube Music without an account"]
fn live_stage6_native_youtube_lyrics() {
    let cancelled = AtomicBool::new(false);
    let before = crate::telemetry::received().api_requests;
    let Lookup::Found(versions, _) = fetch_public_youtube("DyDfgMOUjCI", &cancelled).unwrap()
    else {
        panic!("public YouTube Music lyrics missing");
    };
    let lyrics = versions.preferred();
    assert_eq!(lyrics.source, Source::YouTubeMusic);
    assert!(lyrics.lines.len() > 20 && lyrics.lines.iter().all(|line| line.time_ms.is_none()));
    assert!(lyrics.attribution.is_some());
    let requests = crate::telemetry::received().api_requests - before;
    assert_eq!(requests, 2);
    println!(
        "Public YouTube Music: {} plain lines and attribution, {requests} HTTP requests",
        lyrics.lines.len()
    );
}
