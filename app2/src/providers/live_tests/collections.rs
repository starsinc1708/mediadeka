use super::*;
use crate::providers::collections::{Collection, CollectionKind, read};

#[test]
#[ignore = "reads real favorites and playlists from an isolated Jellyfin test server"]
fn live_jellyfin_account_collections() {
    let cancelled = AtomicBool::new(false);
    let server = std::env::var("MEDIADEKA_TEST_JELLYFIN_URL").expect("fixture URL");
    let username = std::env::var("MEDIADEKA_TEST_JELLYFIN_USER").expect("fixture user");
    let password = std::env::var("MEDIADEKA_TEST_JELLYFIN_PASSWORD").expect("fixture password");
    let expected_playlist =
        std::env::var("MEDIADEKA_TEST_JELLYFIN_PLAYLIST").expect("fixture playlist");
    let session = sign_in(
        ProviderId::Jellyfin,
        LoginInput::JellyfinPassword {
            server,
            username,
            password,
        },
        &cancelled,
        |_| {},
    )
    .expect("Jellyfin sign-in");
    let Collection::Liked(tracks) = read(
        &session.credentials,
        &session.profile,
        CollectionKind::Liked,
        &cancelled,
    )
    .expect("Jellyfin favorite audio") else {
        panic!("expected liked tracks");
    };
    assert_eq!(tracks.len(), 1);
    assert_eq!(tracks[0].title, "Mediadeka Jellyfin Acceptance");
    assert!(tracks[0].media.is_some());
    let Collection::Playlists(playlists) = read(
        &session.credentials,
        &session.profile,
        CollectionKind::Playlists,
        &cancelled,
    )
    .expect("Jellyfin connected playlists") else {
        panic!("expected playlists");
    };
    let item = playlists
        .iter()
        .find(|item| playlist_id(session.provider, item).as_deref() == Some(&expected_playlist))
        .expect("fixture playlist card");
    let opened = read_playlist(
        session.provider,
        Some(&session.credentials),
        &expected_playlist,
        &cancelled,
    )
    .expect("open fixture playlist");
    assert_eq!(opened.name, item.title);
    assert_eq!(opened.tracks.len(), 3);
    let matches = crate::providers::catalog::search(
        session.provider,
        &session.credentials,
        crate::music::ObjectKind::Track,
        &tracks[0].title,
        &cancelled,
    )
    .expect("search the connected Jellyfin catalog");
    let found = matches
        .iter()
        .find(|page| page.reference.id == tracks[0].id)
        .expect("favorite recording in Jellyfin search results");
    let page =
        crate::providers::catalog::read(&found.reference, Some(&session.credentials), &cancelled)
            .expect("read the Jellyfin recording from search");
    assert_eq!(page.title, tracks[0].title);
    assert_eq!(page.artists, ["Mediadeka"]);
    assert_eq!(page.tracks[0].media, tracks[0].media);
    println!(
        "Jellyfin: one playable favorite, playlist with three additions, recording searched and opened"
    );
}

#[cfg(windows)]
#[test]
#[ignore = "reads the connected Yandex Music test account's favorites and playlists"]
fn live_yandex_account_collections() {
    assert_collections(&existing_yandex_session());
}

#[cfg(windows)]
#[test]
#[ignore = "reads favorites and playlists using the dedicated SoundCloud test profile"]
fn live_soundcloud_account_collections() {
    assert_collections(&saved_session(
        ProviderId::SoundCloud,
        "MEDIADEKA_SOUNDCLOUD_TEST_PROFILE",
    ));
}

#[cfg(windows)]
#[test]
#[ignore = "reads favorites and playlists using the selected Spotify test profile"]
fn live_spotify_account_collections() {
    assert_collections(&saved_session(
        ProviderId::Spotify,
        "MEDIADEKA_SPOTIFY_TEST_PROFILE",
    ));
}

#[cfg(windows)]
#[test]
#[ignore = "reads favorites and playlists using the selected YouTube Music test profile"]
fn live_youtube_account_collections() {
    assert_collections(&saved_session(
        ProviderId::YouTubeMusic,
        "MEDIADEKA_YOUTUBE_TEST_PROFILE",
    ));
}

#[cfg(windows)]
fn saved_session(provider: ProviderId, variable: &str) -> Session {
    use crate::storage::Store;
    let directory = std::env::var_os(variable).expect("test profile");
    let store = Store::open(directory).expect("open existing test profile");
    let account = store
        .preferred_account(provider)
        .unwrap()
        .expect("connected account");
    let credentials = store
        .credentials(&account)
        .unwrap()
        .expect("account access");
    restore(
        account.provider,
        &account.profile,
        credentials,
        &AtomicBool::new(false),
    )
    .expect("restore profile")
}

#[cfg(windows)]
fn assert_collections(session: &Session) {
    let cancelled = AtomicBool::new(false);
    let Collection::Liked(tracks) = read(
        &session.credentials,
        &session.profile,
        CollectionKind::Liked,
        &cancelled,
    )
    .expect("liked tracks") else {
        panic!("expected liked tracks");
    };
    assert!(tracks.iter().any(|track| !track.artist.is_empty()));
    let Collection::Playlists(playlists) = read(
        &session.credentials,
        &session.profile,
        CollectionKind::Playlists,
        &cancelled,
    )
    .expect("saved playlists") else {
        panic!("expected saved playlists");
    };
    assert!(
        playlists
            .iter()
            .all(|item| playlist_id(session.provider, item).is_some())
    );
    let Collection::Albums(albums) = read(
        &session.credentials,
        &session.profile,
        CollectionKind::Albums,
        &cancelled,
    )
    .expect("saved albums") else {
        panic!("expected album collection");
    };
    let Collection::Artists(artists) = read(
        &session.credentials,
        &session.profile,
        CollectionKind::Artists,
        &cancelled,
    )
    .expect("followed artists") else {
        panic!("expected artist collection");
    };
    if session.provider == ProviderId::Spotify {
        assert!(
            albums.iter().any(|album| {
                album.item.artwork.is_some()
                    && album.added.is_some()
                    && !album.item.subtitle.is_empty()
                    && album.item.id.starts_with("spotify:album:")
            }),
            "the Spotify test profile must contain a saved album with metadata"
        );
        assert!(
            artists.iter().any(|artist| {
                !artist.item.title.is_empty() && artist.item.id.starts_with("spotify:artist:")
            }),
            "the Spotify test profile must follow an artist"
        );
    }
    println!(
        "{}: {} favorites, {} saved albums, {} followed artists, {} playlist cards",
        session.provider.display_name(),
        tracks.len(),
        albums.len(),
        artists.len(),
        playlists.len()
    );
}
