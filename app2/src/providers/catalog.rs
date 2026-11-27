use std::sync::atomic::AtomicBool;

use url::Url;

use crate::music::{MusicPage, ObjectKind, ObjectRef};

use super::{
    Credentials, ErrorKind, ProviderError, ProviderId, jellyfin, soundcloud, spotify,
    transport::check_cancelled, yandex, youtube,
};

pub(super) const SEARCH_LIMIT: usize = 20;

pub(crate) fn valid_jellyfin_item_id(id: &str) -> bool {
    jellyfin::valid_item_id(id)
}

pub(crate) fn resolve_jellyfin_link(
    credentials: &Credentials,
    web_root: &Url,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<ObjectRef, ProviderError> {
    jellyfin::catalog::resolve_link(credentials, web_root, id, cancelled)
}

pub(crate) fn resolve_soundcloud_set(
    reference: &ObjectRef,
    credentials: &Credentials,
    cancelled: &AtomicBool,
) -> Result<ObjectRef, ProviderError> {
    if reference.provider != ProviderId::SoundCloud
        || !matches!(reference.kind, ObjectKind::Album | ObjectKind::Playlist)
    {
        return Err(invalid_id(reference.provider));
    }
    check_cancelled(cancelled)?;
    soundcloud::catalog::resolve_set(reference, credentials, cancelled)
}

pub(crate) fn read(
    reference: &ObjectRef,
    credentials: Option<&Credentials>,
    cancelled: &AtomicBool,
) -> Result<MusicPage, ProviderError> {
    check_cancelled(cancelled)?;
    let provider = reference.provider;
    if credentials.is_some_and(|value| value.provider() != provider) {
        return Err(missing_access(provider));
    }
    if reference.kind == ObjectKind::Playlist {
        let playlist = super::read_playlist(provider, credentials, &reference.id, cancelled)?;
        let mut page = MusicPage::new(reference.clone(), playlist.name);
        if provider != ProviderId::Spotify {
            page.artists = playlist
                .owner
                .into_iter()
                .map(|owner| owner.name().to_owned())
                .collect();
        }
        page.artwork = playlist.artwork;
        page.url = playlist.link;
        page.tracks = playlist.tracks;
        return Ok(page);
    }
    let mut result = match (provider, credentials) {
        (ProviderId::YandexMusic, credential) => {
            yandex::catalog::read(reference, credential, cancelled)
        }
        (ProviderId::YouTubeMusic, credential) => {
            youtube::catalog::read(reference, credential, cancelled)
        }
        (ProviderId::Spotify, Some(credential)) => {
            spotify::catalog::read(reference, credential, cancelled)
        }
        (ProviderId::SoundCloud, Some(credential)) => {
            soundcloud::catalog::read(reference, credential, cancelled)
        }
        (ProviderId::Jellyfin, Some(credential)) => {
            jellyfin::catalog::read(reference, credential, cancelled)
        }
        _ => Err(missing_access(provider)),
    };
    if credentials.is_none()
        && let Ok(page) = &mut result
    {
        for track in &mut page.tracks {
            track.media = None;
        }
    }
    check_cancelled(cancelled)?;
    result
}

pub(crate) fn search(
    provider: ProviderId,
    credentials: &Credentials,
    kind: ObjectKind,
    query: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<MusicPage>, ProviderError> {
    check_cancelled(cancelled)?;
    if credentials.provider() != provider {
        return Err(missing_access(provider));
    }
    let query = query.trim();
    if query.is_empty() || query.len() > 512 {
        return Err(ProviderError::new(
            ErrorKind::InvalidInput,
            "Некорректный поисковый запрос.",
        ));
    }
    let result = match provider {
        ProviderId::YandexMusic => yandex::catalog::search(credentials, kind, query, cancelled),
        ProviderId::YouTubeMusic => youtube::catalog::search(credentials, kind, query, cancelled),
        ProviderId::Spotify => spotify::catalog::search(credentials, kind, query, cancelled),
        ProviderId::SoundCloud => soundcloud::catalog::search(credentials, kind, query, cancelled),
        ProviderId::Jellyfin => jellyfin::catalog::search(credentials, kind, query, cancelled),
    };
    check_cancelled(cancelled)?;
    result
}

pub(super) fn missing_access(provider: ProviderId) -> ProviderError {
    ProviderError::new(
        ErrorKind::Expired,
        format!("Откройте подключение {}.", provider.display_name()),
    )
}

pub(super) fn invalid_id(provider: ProviderId) -> ProviderError {
    ProviderError::new(
        ErrorKind::InvalidInput,
        format!(
            "Некорректный идентификатор объекта {}.",
            provider.display_name()
        ),
    )
}
