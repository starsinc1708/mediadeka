use std::{sync::atomic::AtomicBool, time::Duration};

use librespot_core::{Error, Session, SpotifyId, SpotifyUri, error::ErrorKind as SpotifyError};
use serde::Deserialize;
use tokio::time::timeout;

use crate::{
    playlist::Playlist,
    providers::{Credentials, ErrorKind, ProviderError, transport::invalid_response},
};

use super::{PROVIDER, librespot, playlist, rate_limit};

const RADIO_TIMEOUT: Duration = Duration::from_secs(20);

pub(crate) fn track_radio(
    credentials: &Credentials,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<Playlist, ProviderError> {
    let id = seed_id(id, "spotify:track:")?;
    radio(credentials, SpotifyUri::Track { id }, cancelled)
}

pub(crate) fn artist_radio(
    credentials: &Credentials,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<Playlist, ProviderError> {
    let id = seed_id(id, "spotify:artist:")?;
    radio(credentials, SpotifyUri::Artist { id }, cancelled)
}

fn seed_id(id: &str, prefix: &str) -> Result<SpotifyId, ProviderError> {
    SpotifyId::from_base62(id.strip_prefix(prefix).unwrap_or(id))
        .map_err(|_| invalid_response(PROVIDER))
}

fn radio(
    credentials: &Credentials,
    seed: SpotifyUri,
    cancelled: &AtomicBool,
) -> Result<Playlist, ProviderError> {
    let Credentials::Spotify {
        client_id,
        librespot: Some(saved),
        ..
    } = credentials
    else {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Подключите Spotify для загрузки радио.",
        ));
    };
    rate_limit::FEED.check()?;
    librespot::with_session(client_id, saved, cancelled, async |session| {
        timeout(RADIO_TIMEOUT, load(session, seed))
            .await
            .map_err(|_| {
                ProviderError::new(ErrorKind::Network, "Spotify не ответил при загрузке радио.")
            })?
    })
}

async fn load(session: &Session, seed: SpotifyUri) -> Result<Playlist, ProviderError> {
    // The pinned client names this method for tracks. Spotify accepts artist URIs too.
    let body = session
        .spclient()
        .get_radio_for_track(&seed)
        .await
        .map_err(radio_error)?;
    let id = radio_playlist(&body)?;
    playlist::load(session, &id).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RadioResponse {
    media_items: Vec<MediaItem>,
}

#[derive(Deserialize)]
struct MediaItem {
    uri: String,
}

fn radio_playlist(body: &[u8]) -> Result<String, ProviderError> {
    let response: RadioResponse =
        serde_json::from_slice(body).map_err(|_| invalid_response(PROVIDER))?;
    response
        .media_items
        .iter()
        .find_map(|item| match SpotifyUri::from_uri(&item.uri).ok()? {
            SpotifyUri::Playlist { id, .. } => Some(id.to_base62()),
            _ => None,
        })
        .ok_or_else(|| invalid_response(PROVIDER))
}

fn radio_error(error: Error) -> ProviderError {
    let (kind, message) = match error.kind {
        SpotifyError::Unauthenticated => (ErrorKind::Expired, "Подключите Spotify заново."),
        SpotifyError::PermissionDenied | SpotifyError::NotFound => (
            ErrorKind::Rejected,
            "Spotify не предоставил радио для этого объекта.",
        ),
        SpotifyError::ResourceExhausted => (
            ErrorKind::Network,
            "Spotify временно ограничил загрузку радио.",
        ),
        _ => (ErrorKind::Network, "Не удалось загрузить радио Spotify."),
    };
    ProviderError::new(kind, message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotify_radio_uses_the_returned_playlist() {
        let body = br#"{"mediaItems":[
            {"uri":"spotify:track:7GhIk7Il098yCjg4BQjzvb"},
            {"uri":"spotify:playlist:37i9dQZF1E8JUqpoPdwFSV"}
        ]}"#;
        assert_eq!(radio_playlist(body).unwrap(), "37i9dQZF1E8JUqpoPdwFSV");
        assert_eq!(
            radio_playlist(br#"{"mediaItems":[]}"#).unwrap_err().kind,
            ErrorKind::InvalidResponse
        );
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "reads real Spotify radio from a copy of a connected profile"]
    fn spotify_track_radio_live() {
        verify_live(
            ["7GhIk7Il098yCjg4BQjzvb", "3AJwUDP919kvQ9QcozQPxg"],
            track_radio,
        );
    }

    #[test]
    #[cfg(windows)]
    #[ignore = "reads real Spotify artist radio from a copy of a connected profile"]
    fn spotify_artist_radio_live() {
        verify_live(
            ["4gzpq5DPGxSnKTe4SA8HAU", "0TnOYISbd1XYRBk9myaseg"],
            artist_radio,
        );
    }

    #[cfg(windows)]
    fn verify_live(
        seeds: [&str; 2],
        read: impl Fn(&Credentials, &str, &AtomicBool) -> Result<Playlist, ProviderError>,
    ) {
        use std::{collections::HashSet, env, fs, path::PathBuf};

        use crate::storage::Store;

        let source = PathBuf::from(env::var_os("MEDIADEKA_RADIO_SOURCE_PROFILE").unwrap());
        let profile = tempfile::tempdir().unwrap();
        let database = rusqlite::Connection::open_with_flags(
            source.join("mediadeka.sqlite3"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap();
        let copy = profile.path().join("mediadeka.sqlite3");
        database
            .execute("VACUUM INTO ?1", [copy.to_str().unwrap()])
            .unwrap();
        let store = Store::open(profile.path()).unwrap();
        let account = store.preferred_account(PROVIDER).unwrap().unwrap();
        let credentials = store.credentials(&account).unwrap().unwrap();
        let snapshots = env::var_os("MEDIADEKA_RADIO_SNAPSHOT_DIR").map(PathBuf::from);
        if let Some(path) = &snapshots {
            fs::create_dir_all(path).unwrap();
        }
        let rows = seeds.map(|id| {
            let playlist = read(&credentials, id, &AtomicBool::new(false));
            let playlist = playlist.unwrap_or_else(|error| panic!("seed {id}: {error:?}"));
            let playable = playlist
                .tracks
                .iter()
                .filter(|track| track.media.is_some() && track.id != id)
                .map(|track| track.id.clone())
                .collect::<HashSet<_>>();
            println!(
                "seed={id} playlist={} title={:?} tracks={} playable={}",
                playlist.link.as_deref().unwrap_or("—"),
                playlist.name,
                playlist.tracks.len(),
                playable.len()
            );
            assert!(!playable.is_empty());
            if let Some(path) = &snapshots {
                let file = fs::File::create(path.join(format!("spotify-{id}.json"))).unwrap();
                serde_json::to_writer_pretty(
                    file,
                    &Playlist {
                        owner: None,
                        ..playlist
                    },
                )
                .unwrap();
            }
            playable
        });
        assert_ne!(rows[0], rows[1]);
    }
}
