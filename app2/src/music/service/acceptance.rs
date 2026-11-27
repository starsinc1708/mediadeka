use std::{
    cell::{Cell, RefCell},
    time::Instant,
};

use super::*;

#[test]
#[ignore = "uses connected Yandex Music and Spotify from an isolated profile"]
fn live_odyssee_prefers_the_spotify_ep() {
    assert!(std::env::var_os("MEDIADEKA_PROFILE_DIR").is_some());
    let store = Store::open_default().unwrap();
    let cancelled = AtomicBool::new(false);
    let source =
        ObjectRef::from_url("https://music.yandex.ru/album/16293786/track/81439810").unwrap();
    let expected = "https://open.spotify.com/track/2ed3BmK8XmEIiNF0836KVR";
    let account = store
        .preferred_account(ProviderId::Spotify)
        .unwrap()
        .unwrap();
    let credentials = store.credentials(&account).unwrap().unwrap();
    let hits = catalog::search(
        ProviderId::Spotify,
        &credentials,
        ObjectKind::Track,
        "Odyssee FIBEL",
        &cancelled,
    )
    .unwrap();
    for hit in hits
        .iter()
        .filter(|page| page.title == "Odyssee" && page.artists.iter().any(|name| name == "FIBEL"))
    {
        println!(
            "Spotify hit: {} · {:?} · {:?} · {:?}",
            hit.reference.id,
            hit.album,
            hit.album_ref.as_ref().map(|album| &album.id),
            hit.duration
        );
    }

    let open_started = Instant::now();
    let spotify_ready = Cell::new(None);
    let links = RefCell::new(Vec::new());
    load_page(&source, None, &cancelled, |update| match update {
        PageUpdate::Links(found) => {
            if spotify_ready.get().is_none() && found.iter().any(|link| link.url == expected) {
                spotify_ready.set(Some(open_started.elapsed()));
            }
            *links.borrow_mut() = found;
        }
        PageUpdate::Error { message, .. } => panic!("could not open source: {message}"),
        PageUpdate::Data(_) => {}
    });
    let links = links.into_inner();
    let spotify = links.iter().find(|link| link.platform == "spotify");
    println!(
        "Confirmed platforms: {:?}",
        links
            .iter()
            .map(|link| link.platform.as_str())
            .collect::<Vec<_>>()
    );
    println!(
        "Confirmed Spotify URL: {:?}",
        spotify.map(|link| link.url.as_str())
    );
    assert_eq!(spotify.map(|link| link.url.as_str()), Some(expected));
    println!(
        "Spotify link ready after {:?}",
        spotify_ready.get().unwrap()
    );

    let before = telemetry::received().api_requests;
    let replayed = Cell::new(false);
    load_page(&source, None, &cancelled, |update| {
        if let PageUpdate::Links(found) = update {
            replayed.set(found.iter().any(|link| link.url == expected));
        }
    });
    let requests = telemetry::received().api_requests - before;
    assert!(replayed.get());
    assert_eq!(requests, 0);
    println!("Warm open: {requests} API requests, same Spotify URL");
}
