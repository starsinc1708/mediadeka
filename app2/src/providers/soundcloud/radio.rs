use std::sync::atomic::AtomicBool;

use crate::{
    playlist::Playlist,
    providers::{
        Credentials, ProviderError, catalog,
        transport::{agent, check_cancelled, invalid_response},
    },
};

use super::{PROVIDER, playlist, stream::track_snapshot, valid_track_id};

pub(crate) fn track_radio(
    credentials: &Credentials,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<Playlist, ProviderError> {
    check_cancelled(cancelled)?;
    let Credentials::SoundCloud { token, client_id } = credentials else {
        return Err(catalog::missing_access(PROVIDER));
    };
    let native_id = if valid_track_id(id) {
        id.to_owned()
    } else {
        track_snapshot(&agent(), id, token, client_id)?["id"]
            .as_u64()
            .filter(|id| *id != 0)
            .ok_or_else(|| invalid_response(PROVIDER))?
            .to_string()
    };
    check_cancelled(cancelled)?;
    let id = format!("soundcloud:system-playlists:track-stations:{native_id}");
    let station = playlist(token, client_id, &id, cancelled)?;
    check_cancelled(cancelled)?;
    Ok(station)
}

#[cfg(all(test, windows))]
#[test]
#[ignore = "requires a connected SoundCloud profile"]
fn live_soundcloud_track_radio() {
    use std::collections::BTreeSet;

    use crate::music::recommendations::acceptance::copy_profile;

    let (_copy, store) = copy_profile();
    let account = store.preferred_account(PROVIDER).unwrap().unwrap();
    let credentials = store.credentials(&account).unwrap().unwrap();
    let cancelled = AtomicBool::new(false);
    let seeds = ["2287580618", "1170714061"];
    let [first, second] = seeds.map(|seed| {
        let playlist = track_radio(&credentials, seed, &cancelled).unwrap();
        let playable = playlist
            .tracks
            .iter()
            .filter(|row| row.media.is_some())
            .count();
        println!(
            "SoundCloud station {seed}: title={:?} rows={} full_stream_metadata={playable}",
            playlist.name,
            playlist.tracks.len(),
        );
        save_station_snapshot(&format!("soundcloud-track-{seed}.json"), &playlist);
        assert_ne!(playable, 0);
        playlist
            .tracks
            .into_iter()
            .map(|track| track.id)
            .collect::<BTreeSet<_>>()
    });
    assert_ne!(first, second);
}

#[cfg(all(test, windows))]
#[test]
#[ignore = "requires a connected SoundCloud profile"]
fn live_soundcloud_artist_station() {
    use crate::{music::recommendations::acceptance::copy_profile, providers::read_playlist};

    let (_copy, store) = copy_profile();
    let account = store.preferred_account(PROVIDER).unwrap().unwrap();
    assert!(!store.account_revoked(&account).unwrap());
    let credentials = store.credentials(&account).unwrap().unwrap();
    let Credentials::SoundCloud { token, client_id } = &credentials else {
        panic!("SoundCloud profile returned credentials for another provider");
    };
    let cancelled = AtomicBool::new(false);
    let agent = agent();
    let mut usable = 0;
    for seed in ["2287580618", "1170714061"] {
        let track = track_snapshot(&agent, seed, token, client_id).unwrap();
        let artist = track["user"]["id"].as_u64().unwrap();
        let station_id = format!("soundcloud:system-playlists:artist-stations:{artist}");
        match read_playlist(PROVIDER, Some(&credentials), &station_id, &cancelled) {
            Ok(playlist) => {
                let playable = playlist
                    .tracks
                    .iter()
                    .filter(|row| row.media.is_some())
                    .count();
                println!(
                    "SoundCloud artist station {artist} from track {seed}: title={:?} rows={} full_stream_metadata={playable} link={:?}",
                    playlist.name,
                    playlist.tracks.len(),
                    playlist.link,
                );
                save_station_snapshot(&format!("soundcloud-artist-{artist}.json"), &playlist);
                usable += usize::from(playable > 0);
            }
            Err(error) => println!("SoundCloud artist station {artist} from track {seed}: {error}"),
        }
    }
    assert!(
        usable > 0,
        "a native artist station should provide playable entries"
    );
}

#[cfg(all(test, windows))]
fn save_station_snapshot(name: &str, playlist: &Playlist) {
    if let Some(path) = std::env::var_os("MEDIADEKA_SOUNDCLOUD_RADIO_SNAPSHOT_DIR") {
        let path = std::path::PathBuf::from(path);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(
            path.join(name),
            serde_json::to_vec_pretty(playlist).unwrap(),
        )
        .unwrap();
    }
}
