use rusqlite::Connection;

use super::*;
use crate::{
    playlist::{Media, Playlist},
    providers::HomeItem,
};

const SOUNDCLOUD_STATION: &str = "soundcloud:system-playlists:artist-stations:12:23";

fn home(ids: &[&str]) -> Home {
    Home {
        title: "Главная".into(),
        items: ids
            .iter()
            .map(|id| HomeItem {
                id: (*id).into(),
                title: (*id).into(),
                subtitle: String::new(),
                url: None,
                artwork: None,
            })
            .collect(),
        sections: Vec::new(),
    }
}

fn keys(sources: &[StationSource], provider: ProviderId) -> Vec<&str> {
    sources
        .iter()
        .map(|source| match &source.key {
            PlaylistKey::Station {
                provider: owner,
                profile: Some(profile),
                id,
            } if *owner == provider && profile == "connected" => id.as_str(),
            _ => panic!("station candidate did not keep the connected source"),
        })
        .collect()
}

#[test]
fn chooses_only_supported_stations_and_keeps_the_personal_yandex_seed() {
    let provider = ProviderId::YandexMusic;
    let personal = selections(None, provider, "connected");
    assert_eq!(keys(&personal, provider), [MY_WAVE]);
    assert_eq!(personal[0].title, "Моя волна");

    let yandex = home(&[
        "playlist:12:23",
        "wave:user:onyourwave",
        "wave:invalid/seed",
        "wave:mood:calm",
        "wave:mood:study",
    ]);
    let stations = selections(Some(&yandex), provider, "connected");
    assert_eq!(keys(&stations, provider), [MY_WAVE, "mood:calm"]);

    let cloud = home(&[
        "soundcloud:system-playlists:weekly:1",
        "station:invalid",
        "station:soundcloud:system-playlists:genre:1",
        "station:soundcloud:system-playlists:genre:1",
        SOUNDCLOUD_STATION,
    ]);
    let provider = ProviderId::SoundCloud;
    let stations = selections(Some(&cloud), provider, "connected");
    assert_eq!(
        keys(&stations, provider),
        ["soundcloud:system-playlists:genre:1", SOUNDCLOUD_STATION]
    );
    for provider in [
        ProviderId::Spotify,
        ProviderId::YouTubeMusic,
        ProviderId::Jellyfin,
    ] {
        assert!(selections(Some(&cloud), provider, "connected").is_empty());
    }
}

#[test]
fn cached_station_keeps_provenance_and_stops_after_revocation_or_cancellation() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let connection = Connection::open(directory.path().join("mediadeka.sqlite3")).unwrap();
    connection
        .execute_batch(
            r#"INSERT INTO account VALUES ('soundcloud', 'connected', 'Listener',
            '{"home":true,"profile_selection":false,"device_code":false}', hex(randomblob(16)));
            INSERT INTO account_source VALUES ('soundcloud', 'connected');"#,
        )
        .unwrap();
    let provider = ProviderId::SoundCloud;
    let account = store.account(provider, "connected").unwrap().unwrap();
    let scope = PlaylistScope::for_source(
        &store,
        provider,
        Some(account.clone()),
        SOUNDCLOUD_STATION,
        &region(),
    )
    .unwrap();
    let playlist = Playlist {
        name: "Станция артиста".into(),
        owner: None,
        artwork: None,
        link: None,
        tracks: vec![PlaylistTrack {
            id: "456".into(),
            title: "Трек".into(),
            artist: "Артист".into(),
            artists: Vec::new(),
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: Some(150),
            media: Some(Media::Provider {
                provider,
                id: "456".into(),
            }),
        }],
    };
    assert!(scope.save(&playlist).unwrap());
    let mut home = home(&[SOUNDCLOUD_STATION]);
    home.items[0].title = playlist.name.clone();
    let mut sources = selections(Some(&home), provider, "connected");
    sources.extend(selections(None, ProviderId::YandexMusic, "connected"));

    let result = load_cached(
        &store,
        &sources,
        StationSessions::default(),
        &AtomicBool::new(false),
    );
    assert_eq!(result.unavailable, [ProviderId::YandexMusic]);
    let [entry] = result.entries.as_slice() else {
        panic!("the saved SoundCloud station should remain available");
    };
    assert_eq!(entry.playlist, sources[0].key);
    assert_eq!(entry.track, playlist.tracks[0]);
    assert_eq!(entry.playlist_revision, None);
    assert_eq!(
        entry.recommendations,
        [RecommendationOrigin {
            provider,
            playlist: playlist.name,
            playlist_id: SOUNDCLOUD_STATION.into(),
        }]
    );

    let cancelled = load_cached(
        &store,
        &sources,
        result.sessions.clone(),
        &AtomicBool::new(true),
    );
    assert!(cancelled.entries.is_empty());
    assert!(cancelled.sessions.0.is_empty());

    assert!(store.forget_music_account(&account).unwrap());
    let revoked = load_cached(&store, &sources, result.sessions, &AtomicBool::new(false));
    assert!(revoked.entries.is_empty());
    assert!(revoked.sessions.0.is_empty());
    assert_eq!(revoked.unavailable, [ProviderId::YandexMusic, provider]);
}
