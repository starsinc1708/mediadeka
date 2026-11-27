use std::io::Read as _;

use crate::{
    music::{
        ObjectKind, ObjectRef, PageUpdate, load_page,
        sound::{alternate_streams, confirmed_streams, recording_isrc},
    },
    playback::QueueEntry,
    playlist::{Media, PlaylistKey},
    providers::catalog,
    storage::Store,
};

use super::{playlists::profile_snapshot, *};

pub(crate) fn stage5_profile() -> tempfile::TempDir {
    profile_snapshot("MEDIADEKA_SOUND_TEST_PROFILE")
}

pub(crate) fn decode_fixture_spotify_track(
    reference: &ObjectRef,
    title: &str,
    artist: &str,
    isrc: &str,
    duration_ms: i32,
) -> crate::music::MusicPage {
    use librespot_core::SpotifyUri;
    use librespot_protocol::metadata::{Artist, AudioFile, ExternalId, Track};
    use protobuf::Message as _;

    let track = Track {
        name: Some(title.into()),
        duration: Some(duration_ms),
        artist: vec![Artist {
            name: Some(artist.into()),
            ..Default::default()
        }],
        external_id: vec![ExternalId {
            type_: Some("isrc".into()),
            id: Some(isrc.into()),
            ..Default::default()
        }],
        file: vec![AudioFile::new()],
        ..Default::default()
    };
    let uri = SpotifyUri::from_uri(&format!("spotify:track:{}", reference.id)).unwrap();
    spotify::catalog::decode_track_metadata(reference, &uri, &track.write_to_bytes().unwrap())
        .expect("Spotify track fixture parses through the catalog adapter")
}

pub(crate) struct Stage5Recordings {
    pub spotify: QueueEntry,
    pub yandex: QueueEntry,
    pub isrc: String,
}

#[derive(Clone, Copy)]
pub(crate) enum Stage5Evidence {
    Isrc,
    ConfirmedLink,
}

pub(crate) fn stage5_recordings(
    store: &Store,
    evidence: Stage5Evidence,
    cancelled: &AtomicBool,
) -> Stage5Recordings {
    let spotify = store
        .preferred_account(ProviderId::Spotify)
        .unwrap()
        .expect("Spotify account");
    let credentials = store.credentials(&spotify).unwrap().unwrap();
    let id = match evidence {
        Stage5Evidence::Isrc => std::env::var("MEDIADEKA_SOUND_TEST_SPOTIFY_ISRC_TRACK")
            .unwrap_or_else(|_| "1E96rC1JYgZoPPw8kvxd1I".into()),
        Stage5Evidence::ConfirmedLink => "1E96rC1JYgZoPPw8kvxd1I".into(),
    };
    let object = ObjectRef {
        provider: spotify.provider,
        kind: ObjectKind::Track,
        id,
    };
    let page = catalog::read(&object, Some(&credentials), cancelled).expect("Spotify recording");
    let track = page.tracks.into_iter().next().expect("Spotify track");
    let playlist = PlaylistKey::Source {
        provider: spotify.provider,
        profile: Some(spotify.profile.id.clone()),
        id: format!("track:{}", object.id),
    };
    let isrc = recording_isrc(store, &playlist, &track, cancelled)
        .expect("read Spotify recording")
        .expect("Spotify ISRC");
    println!(
        "Spotify catalog: {} · {} · {isrc}",
        track.artist, track.title
    );
    let matches = match evidence {
        Stage5Evidence::Isrc => alternate_streams(store, &playlist, &track, Some(&isrc), cancelled)
            .expect("find ISRC in connected catalogs"),
        Stage5Evidence::ConfirmedLink => {
            load_page(&object, Some(&spotify.profile.id), cancelled, |update| {
                if let PageUpdate::Error { message, .. } = update {
                    println!("Music links: {message}");
                }
            });
            confirmed_streams(store, &playlist, &track).expect("confirmed music links")
        }
    };
    let alternate = matches
        .into_iter()
        .find(|candidate| candidate.provider == ProviderId::YandexMusic)
        .expect("same recording in the Yandex Music catalog");
    let account = store
        .account(alternate.provider, &alternate.profile)
        .unwrap()
        .expect("Yandex Music account");
    let credentials = store.credentials(&account).unwrap().unwrap();
    let yandex = catalog::read(
        &ObjectRef {
            provider: alternate.provider,
            kind: ObjectKind::Track,
            id: alternate.id.clone(),
        },
        Some(&credentials),
        cancelled,
    )
    .expect("matching Yandex Music recording");
    let yandex_track = yandex
        .tracks
        .into_iter()
        .next()
        .expect("Yandex Music track");
    let yandex_playlist = PlaylistKey::Source {
        provider: alternate.provider,
        profile: Some(alternate.profile),
        id: format!("track:{}", yandex.reference.id),
    };
    Stage5Recordings {
        spotify: QueueEntry {
            playlist_revision: None,
            recommendations: Vec::new(),
            playlist,
            index: 0,
            track,
        },
        yandex: QueueEntry {
            playlist_revision: None,
            recommendations: Vec::new(),
            playlist: yandex_playlist,
            index: 0,
            track: yandex_track,
        },
        isrc,
    }
}

#[test]
#[ignore = "requires connected Spotify and subscribed Yandex Music in MEDIADEKA_SOUND_TEST_PROFILE"]
fn live_stage5_spotify_yandex_isrc() {
    let snapshot = stage5_profile();
    let store = Store::open(snapshot.path()).expect("isolated profile");
    let cancelled = AtomicBool::new(false);
    let Stage5Recordings {
        spotify,
        yandex,
        isrc,
    } = stage5_recordings(&store, Stage5Evidence::Isrc, &cancelled);
    let (provider, profile) = yandex.playlist.connected_source().unwrap();
    let account = store.account(provider, profile.unwrap()).unwrap().unwrap();
    let credentials = store.credentials(&account).unwrap().unwrap();
    let Some(Media::Provider { id, .. }) = &yandex.track.media else {
        panic!("expected an available provider-backed track");
    };
    let direct = direct_stream(provider, Some(&credentials), id, &cancelled)
        .expect("authenticated Yandex Music stream");
    let StreamOffer::Full(stream) = direct else {
        panic!("Yandex Music returned only a preview");
    };
    let mut response = transport::agent()
        .get(&stream.url)
        .header("Range", "bytes=0-1023")
        .call()
        .expect("Yandex Music CDN");
    assert!(response.status().is_success());
    let mut prefix = [0; 32];
    response
        .body_mut()
        .as_reader()
        .read_exact(&mut prefix)
        .expect("nonempty audio response");
    println!(
        "{} ({isrc}) → {} ({}) · {} · {:?} kbps · HTTP {}",
        spotify.track.title,
        provider.display_name(),
        id,
        stream.codec.as_deref().unwrap_or("unknown format"),
        stream.bitrate_kbps,
        response.status()
    );
}
