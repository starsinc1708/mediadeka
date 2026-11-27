use std::sync::atomic::AtomicBool;

use crate::music::lyrics::{Lookup, LyricsVersions};

use super::{Credentials, ProviderId, transport::check_cancelled};

#[cfg(all(test, windows))]
mod tests;

pub(crate) fn supports(credentials: &Credentials) -> bool {
    match credentials {
        Credentials::YandexMusic { access_token, .. } => {
            super::yandex::lyrics::supports(access_token)
        }
        Credentials::Spotify { librespot, .. } => librespot.is_some(),
        _ => false,
    }
}

pub(crate) fn fetch(
    provider: ProviderId,
    credentials: &Credentials,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<Option<LyricsVersions>, String> {
    if provider != credentials.provider() || !supports(credentials) {
        return Ok(None);
    }
    check_cancelled(cancelled).map_err(|error| error.message)?;
    let result = match credentials {
        Credentials::YandexMusic { access_token, .. } => {
            super::yandex::lyrics::fetch(access_token, id, cancelled)
        }
        Credentials::Spotify {
            client_id,
            librespot: Some(saved),
            ..
        } => super::spotify::lyrics::fetch(client_id, saved, id, cancelled)
            .map(|result| result.map(Into::into)),
        _ => return Ok(None),
    };
    check_cancelled(cancelled).map_err(|error| error.message)?;
    result.map_err(|error| error.message)
}

pub(crate) fn fetch_public_youtube(id: &str, cancelled: &AtomicBool) -> Result<Lookup, String> {
    super::youtube::lyrics::fetch(id, cancelled).map_err(|error| error.message)
}
