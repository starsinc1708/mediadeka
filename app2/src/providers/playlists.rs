use std::sync::atomic::AtomicBool;

use crate::playlist::{Playlist, PlaylistTrack};

use super::{
    Credentials, ErrorKind, HomeItem, ProviderError, ProviderId, jellyfin, soundcloud, spotify,
    transport::check_cancelled, yandex, youtube,
};

pub(crate) struct PlaylistPage<'a> {
    pub playlist: &'a Playlist,
    pub tracks: &'a [PlaylistTrack],
    pub offset: usize,
    pub total: Option<usize>,
}

pub(crate) type PlaylistProgress<'a> =
    dyn FnMut(PlaylistPage<'_>) -> Result<(), ProviderError> + 'a;

pub(crate) fn playlist_id(provider: ProviderId, item: &HomeItem) -> Option<String> {
    let id = match provider {
        ProviderId::Spotify => item.id.strip_prefix("spotify:playlist:")?,
        ProviderId::YouTubeMusic => item.id.strip_prefix("browse:VL")?,
        ProviderId::SoundCloud if soundcloud::system_playlist_id(&item.id) => &item.id,
        _ => item.id.strip_prefix("playlist:")?,
    };
    valid_id(provider, id).then(|| id.to_owned())
}

fn valid_id(provider: ProviderId, id: &str) -> bool {
    match provider {
        ProviderId::Spotify => {
            id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }
        ProviderId::YouTubeMusic => youtube::valid_playlist_id(id),
        ProviderId::YandexMusic => yandex::valid_playlist_id(id),
        ProviderId::SoundCloud => soundcloud::valid_playlist_id(id),
        ProviderId::Jellyfin => jellyfin::valid_item_id(id),
    }
}

pub(crate) fn read_playlist(
    provider: ProviderId,
    credentials: Option<&Credentials>,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<Playlist, ProviderError> {
    read_playlist_with_progress(provider, credentials, id, cancelled, &mut |_| Ok(()))
}

pub(crate) fn read_playlist_with_progress(
    provider: ProviderId,
    credentials: Option<&Credentials>,
    id: &str,
    cancelled: &AtomicBool,
    progress: &mut PlaylistProgress<'_>,
) -> Result<Playlist, ProviderError> {
    check_cancelled(cancelled)?;
    if !valid_id(provider, id) {
        return Err(ProviderError::new(
            ErrorKind::InvalidInput,
            format!("Некорректный адрес плейлиста {}.", provider.display_name()),
        ));
    }
    let mut result = match (provider, credentials) {
        (
            ProviderId::Spotify,
            Some(Credentials::Spotify {
                client_id,
                librespot,
                ..
            }),
        ) => spotify::playlist(client_id, librespot.as_ref(), id, cancelled, progress),
        (ProviderId::YouTubeMusic, None | Some(Credentials::YouTubeMusic { .. })) => {
            youtube::playlist_with_progress(credentials, id, cancelled, progress)
        }
        (ProviderId::YandexMusic, Some(Credentials::YandexMusic { access_token, .. })) => {
            yandex::playlist(Some(access_token), id, cancelled)
        }
        (ProviderId::YandexMusic, None) => yandex::playlist(None, id, cancelled),
        (ProviderId::SoundCloud, Some(Credentials::SoundCloud { token, client_id })) => {
            soundcloud::playlist_with_progress(token, client_id, id, cancelled, progress)
        }
        (
            ProviderId::Jellyfin,
            Some(Credentials::Jellyfin {
                server,
                device,
                access_token,
            }),
        ) => jellyfin::playlist(server, device, access_token, id, cancelled, progress),
        _ => Err(no_connection(provider)),
    };
    if credentials.is_none()
        && let Ok(playlist) = &mut result
    {
        for track in &mut playlist.tracks {
            track.media = None;
        }
    }
    check_cancelled(cancelled)?;
    result
}

fn no_connection(provider: ProviderId) -> ProviderError {
    ProviderError::new(
        ErrorKind::Expired,
        format!("Откройте нужное подключение {}.", provider.display_name()),
    )
}

/// Keep one row per playlist addition even if a catalog no longer knows the recording.
pub(super) fn unavailable_track(id: String) -> crate::playlist::PlaylistTrack {
    crate::playlist::PlaylistTrack {
        id,
        title: "Трек недоступен".into(),
        artist: String::new(),
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album_ref: None,
        album: None,
        artwork: None,
        added: None,
        duration: None,
        media: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_cards_route_to_typed_playlist_ids() {
        for (provider, card, expected) in [
            (
                ProviderId::Spotify,
                "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M",
                Some("37i9dQZF1DXcBWIGoYBM5M"),
            ),
            (
                ProviderId::Spotify,
                "spotify:artist:37i9dQZF1DXcBWIGoYBM5M",
                None,
            ),
            (
                ProviderId::YouTubeMusic,
                "browse:VLPLexample",
                Some("PLexample"),
            ),
            (ProviderId::YouTubeMusic, "browse:MPREalbum", None),
            (
                ProviderId::YandexMusic,
                "playlist:556:1034",
                Some("556:1034"),
            ),
            (ProviderId::SoundCloud, "playlist:123", Some("123")),
            (
                ProviderId::SoundCloud,
                "soundcloud:system-playlists:your-mix:1",
                Some("soundcloud:system-playlists:your-mix:1"),
            ),
            (ProviderId::SoundCloud, "playlist:12?secret=x", None),
            (
                ProviderId::Jellyfin,
                "playlist:00000000000000000000000000000001",
                Some("00000000000000000000000000000001"),
            ),
        ] {
            let card = HomeItem {
                id: card.into(),
                title: String::new(),
                subtitle: String::new(),
                url: None,
                artwork: None,
            };
            assert_eq!(playlist_id(provider, &card).as_deref(), expected);
        }
    }

    #[test]
    fn unavailable_account_is_distinct_from_invalid_address_and_cancellation() {
        let id = "37i9dQZF1DXcBWIGoYBM5M";
        let cancelled = AtomicBool::new(false);
        assert_eq!(
            read_playlist(ProviderId::Spotify, None, id, &cancelled)
                .unwrap_err()
                .kind,
            ErrorKind::Expired
        );
        assert_eq!(
            read_playlist(ProviderId::Spotify, None, "123", &cancelled)
                .unwrap_err()
                .kind,
            ErrorKind::InvalidInput
        );
        assert_eq!(
            read_playlist(ProviderId::Spotify, None, id, &AtomicBool::new(true))
                .unwrap_err()
                .kind,
            ErrorKind::Cancelled
        );
    }
}
