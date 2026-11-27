use std::sync::atomic::AtomicBool;

use crate::{
    music::{ObjectKind, ObjectRef},
    providers::{Credentials, ProviderId, catalog},
    storage::Store,
};

use super::playlists::{profile_snapshot, test_profile};

fn saved_catalog_connection(provider: ProviderId) -> Credentials {
    let snapshot = profile_snapshot("MEDIADEKA_CATALOG_TEST_PROFILE");
    let store = Store::open(snapshot.path()).expect("open isolated profile");
    let account = store
        .preferred_account(provider)
        .expect("read preferred account")
        .expect("provider account in the supplied profile");
    store
        .credentials(&account)
        .expect("read stored connection")
        .expect("credentials for the selected account")
}

#[test]
#[ignore = "uses the saved Spotify account through the catalog read path in an isolated profile"]
fn live_catalog_spotify_saved_connection() {
    let provider = ProviderId::Spotify;
    let credentials = saved_catalog_connection(provider);
    let cancelled = AtomicBool::new(false);
    let track = catalog::read(
        &ObjectRef {
            provider,
            kind: ObjectKind::Track,
            id: "2Fxmhks0bxGSBdJ92vM42m".into(),
        },
        Some(&credentials),
        &cancelled,
    )
    .expect("open recording from the saved connection");
    let album = catalog::read(
        track
            .album_ref
            .as_ref()
            .expect("recording release reference"),
        Some(&credentials),
        &cancelled,
    )
    .expect("open linked release");
    let artist = catalog::read(
        track.artist_credits[0]
            .reference
            .as_ref()
            .expect("artist ID"),
        Some(&credentials),
        &cancelled,
    )
    .expect("open linked artist");
    assert_eq!(Some(album.title.as_str()), track.album.as_deref());
    assert_eq!(artist.title, track.artist_credits[0].name);
    assert!(!album.tracks.is_empty());
    assert!(!artist.releases.is_empty());
    println!("Spotify: recording, linked album and artist opened from the saved connection");
}

#[test]
#[ignore = "requires read-only access to a saved test profile in MEDIADEKA_CATALOG_TEST_PROFILE"]
fn live_catalog_spotify() {
    let cancelled = AtomicBool::new(false);
    let provider = ProviderId::Spotify;
    let session = test_profile(provider, "MEDIADEKA_CATALOG_TEST_PROFILE", &cancelled);
    let search = catalog::search(
        provider,
        &session.credentials,
        ObjectKind::Album,
        "Daft Punk Random Access Memories",
        &cancelled,
    )
    .expect("search Spotify albums");
    assert!(
        search
            .iter()
            .any(|album| album.title == "Random Access Memories")
    );
    let reference = ObjectRef {
        provider,
        kind: ObjectKind::Album,
        id: "4m2880jivSbbyEGAKfITCa".into(),
    };
    let page = catalog::read(&reference, Some(&session.credentials), &cancelled)
        .expect("read Spotify album");
    assert_eq!(page.title, "Random Access Memories");
    assert!(
        page.tracks
            .iter()
            .any(|track| track.title.contains("Get Lucky"))
    );
    assert_eq!(page.year, Some(2013));
    let first = page.tracks.first().expect("ordered tracks");
    let track = catalog::read(
        &ObjectRef {
            provider,
            kind: ObjectKind::Track,
            id: first.id.clone(),
        },
        Some(&session.credentials),
        &cancelled,
    )
    .expect("Spotify recording");
    assert_eq!(track.title, first.title);
    assert!(track.isrc.is_some());
    let artist = catalog::read(
        &ObjectRef {
            provider,
            kind: ObjectKind::Artist,
            id: "4tZwfgrHOc3mvqYlEYSvVi".into(),
        },
        Some(&session.credentials),
        &cancelled,
    )
    .expect("Spotify artist");
    assert_eq!(artist.title, "Daft Punk");
    assert!(!artist.tracks.is_empty());
    assert!(
        artist
            .releases
            .iter()
            .any(|release| release.title == "Random Access Memories")
    );
    let collection = catalog::read(
        &ObjectRef {
            provider,
            kind: ObjectKind::Collection,
            id: "0JQ5DAt0tbjZptfcdMSKl3".into(),
        },
        Some(&session.credentials),
        &cancelled,
    )
    .expect("Spotify browse page");
    assert!(!collection.title.is_empty());
    assert!(!collection.releases.is_empty());
    let recordings = catalog::search(
        provider,
        &session.credentials,
        ObjectKind::Track,
        "Daft Punk Get Lucky",
        &cancelled,
    )
    .expect("search Spotify recordings");
    assert!(
        recordings
            .iter()
            .any(|track| track.title.contains("Get Lucky") && track.duration.is_some())
    );
    println!(
        "Spotify: {} album results, album {}, {} tracks, artist {} with {} releases, {} browse cards, {} recording results",
        search.len(),
        page.title,
        page.tracks.len(),
        artist.title,
        artist.releases.len(),
        collection.releases.len(),
        recordings.len()
    );
}

#[test]
#[ignore = "requires read-only access to a saved test profile in MEDIADEKA_CATALOG_TEST_PROFILE"]
fn live_catalog_soundcloud() {
    let cancelled = AtomicBool::new(false);
    let provider = ProviderId::SoundCloud;
    let credentials = saved_catalog_connection(provider);
    let hits = catalog::search(
        provider,
        &credentials,
        ObjectKind::Track,
        "Daft Punk Get Lucky",
        &cancelled,
    )
    .expect("search SoundCloud recordings");
    let hit = hits
        .iter()
        .find(|hit| hit.url.is_some())
        .expect("search hit with public address");
    let reference =
        ObjectRef::from_url(hit.url.as_deref().unwrap()).expect("parse SoundCloud public URL");
    let page = catalog::read(&reference, Some(&credentials), &cancelled)
        .expect("resolve SoundCloud permalink");
    assert_eq!(page.reference, hit.reference);
    assert_eq!(page.title, hit.title);
    assert!(page.duration.is_some());
    let artists = catalog::search(
        provider,
        &credentials,
        ObjectKind::Artist,
        "Daft Punk",
        &cancelled,
    )
    .expect("search SoundCloud artists");
    let artist = artists
        .iter()
        .find(|hit| hit.url.as_deref() == Some("https://soundcloud.com/daftpunkofficialmusic"))
        .expect("Daft Punk artist link");
    let artist = catalog::read(&artist.reference, Some(&credentials), &cancelled)
        .expect("read SoundCloud artist");
    assert!(!artist.tracks.is_empty());
    let album = artist.releases.first().expect("SoundCloud artist release");
    let reference = ObjectRef::from_item(provider, album).expect("SoundCloud native album route");
    assert_eq!(reference.kind, ObjectKind::Album);
    let public = ObjectRef::from_url(album.url.as_deref().expect("public SoundCloud release"))
        .expect("public SoundCloud set");
    let resolved = catalog::resolve_soundcloud_set(&public, &credentials, &cancelled)
        .expect("classify public SoundCloud set");
    assert_eq!(resolved, reference);
    let album =
        catalog::read(&reference, Some(&credentials), &cancelled).expect("read SoundCloud album");
    assert!(!album.tracks.is_empty());
    println!(
        "SoundCloud: {} recordings, permalink resolved to its numeric track ID, artist with {} releases and album with {} tracks",
        hits.len(),
        artist.releases.len(),
        album.tracks.len()
    );
}

#[test]
#[ignore = "requires read-only access to a saved test profile in MEDIADEKA_CATALOG_TEST_PROFILE"]
fn live_catalog_yandex() {
    let cancelled = AtomicBool::new(false);
    let provider = ProviderId::YandexMusic;
    let session = test_profile(provider, "MEDIADEKA_CATALOG_TEST_PROFILE", &cancelled);
    let hits = catalog::search(
        provider,
        &session.credentials,
        ObjectKind::Artist,
        "Daft Punk",
        &cancelled,
    )
    .expect("search Yandex Music artists");
    let hit = hits
        .iter()
        .find(|hit| hit.title == "Daft Punk")
        .expect("Daft Punk search hit");
    let page =
        catalog::read(&hit.reference, Some(&session.credentials), &cancelled).expect("read artist");
    assert_eq!(page.title, hit.title);
    assert!(
        page.releases
            .iter()
            .any(|release| release.title == "Discovery")
    );
    assert!(!page.tracks.is_empty());
    println!(
        "Yandex Music: {} artist results, {} albums, {} popular tracks",
        hits.len(),
        page.releases.len(),
        page.tracks.len()
    );
}

#[test]
#[ignore = "requires read-only access to a saved test profile in MEDIADEKA_CATALOG_TEST_PROFILE"]
fn live_catalog_youtube() {
    let cancelled = AtomicBool::new(false);
    let provider = ProviderId::YouTubeMusic;
    let session = test_profile(provider, "MEDIADEKA_CATALOG_TEST_PROFILE", &cancelled);
    let hits = catalog::search(
        provider,
        &session.credentials,
        ObjectKind::Artist,
        "Daft Punk",
        &cancelled,
    )
    .expect("search YouTube Music artists");
    let hit = hits
        .iter()
        .find(|hit| hit.title == "Daft Punk")
        .expect("Daft Punk search hit");
    let page =
        catalog::read(&hit.reference, Some(&session.credentials), &cancelled).expect("read artist");
    assert_eq!(page.title, hit.title);
    assert!(!page.releases.is_empty());
    assert!(!page.tracks.is_empty());
    println!(
        "YouTube Music: {} artist results, {} releases, {} popular tracks",
        hits.len(),
        page.releases.len(),
        page.tracks.len()
    );
}

#[test]
#[ignore = "reads public artist profiles and their exact related profiles without an account"]
fn live_public_artist_relationships() {
    let cancelled = AtomicBool::new(false);
    for (provider, artist) in [
        (ProviderId::YouTubeMusic, "UCc4K7bAqpdBP8jh1j9XZAww"),
        (ProviderId::YandexMusic, "36800"),
    ] {
        let reference = ObjectRef::native(provider, ObjectKind::Artist, artist.into()).unwrap();
        let before = crate::telemetry::received().api_requests;
        let page = catalog::read(&reference, None, &cancelled).expect("read public artist page");
        let requests = crate::telemetry::received().api_requests - before;
        assert_eq!(requests, 1, "artist metadata uses the existing request");
        let details = page.artist_details.expect("artist metadata");
        let biography = details
            .biography
            .filter(|body| !body.is_empty())
            .expect("public biography");
        let audience = details
            .monthly_listeners_caption
            .or_else(|| details.monthly_listeners.map(|value| value.to_string()));
        let related = details.related.first().expect("real related item");
        let exact = ObjectRef::from_item(provider, related).expect("related profile identity");
        assert_eq!(exact.kind, ObjectKind::Artist);
        assert_ne!(exact, reference);
        let destination =
            catalog::read(&exact, None, &cancelled).expect("read exact related profile");
        assert_eq!(destination.reference, exact);
        assert_eq!(destination.title, related.title);
        println!(
            "{}: {} → {} ({}); {} related, {} biography characters, audience {}, {requests} request for the origin",
            provider.display_name(),
            page.title,
            destination.title,
            exact.id,
            details.related.len(),
            biography.chars().count(),
            audience.expect("public audience")
        );
    }
}
