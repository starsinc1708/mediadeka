use std::sync::atomic::AtomicBool;

use ureq::{Body, http::Response};

use crate::music::{ObjectKind, ObjectRef};

use super::{
    Credentials, Profile, ProviderError, ProviderId, catalog, collections, jellyfin, soundcloud,
    spotify,
    transport::{check_cancelled, network, response_error},
    yandex, youtube,
};

pub(crate) fn contains(
    credentials: &Credentials,
    profile: &Profile,
    reference: &ObjectRef,
    cancelled: &AtomicBool,
) -> Result<bool, ProviderError> {
    check_cancelled(cancelled)?;
    let provider = reference.provider;
    if credentials.provider() != provider
        || reference.kind != ObjectKind::Track
        || !reference.is_valid()
    {
        return Err(catalog::invalid_id(provider));
    }
    let id = &reference.id;
    match credentials {
        Credentials::Spotify { .. } => {
            spotify::likes::contains(credentials, profile, id, cancelled)
        }
        Credentials::YandexMusic { .. } => {
            yandex::likes::contains(credentials, profile, id, cancelled)
        }
        Credentials::Jellyfin {
            server,
            device,
            access_token,
        } => jellyfin::likes::contains(server, device, access_token, &profile.id, id, cancelled),
        _ => {
            let collection = collections::read(
                credentials,
                profile,
                collections::CollectionKind::Liked,
                cancelled,
            )?;
            let collections::Collection::Liked(tracks) = collection else {
                return Err(super::transport::invalid_response(provider));
            };
            Ok(tracks.iter().any(|track| &track.id == id))
        }
    }
}

pub(crate) fn set(
    credentials: &Credentials,
    profile: &Profile,
    reference: &ObjectRef,
    liked: bool,
    cancelled: &AtomicBool,
) -> Result<(), ProviderError> {
    check_cancelled(cancelled)?;
    let provider = reference.provider;
    if credentials.provider() != provider
        || reference.kind != ObjectKind::Track
        || !reference.is_valid()
    {
        return Err(catalog::invalid_id(provider));
    }
    let id = &reference.id;
    match credentials {
        Credentials::Spotify { .. } => {
            spotify::likes::set(credentials, profile, id, liked, cancelled)
        }
        Credentials::YouTubeMusic { .. } => {
            youtube::likes::set(credentials, profile, id, liked, cancelled)
        }
        Credentials::YandexMusic { .. } => {
            yandex::likes::set(credentials, profile, id, liked, cancelled)
        }
        Credentials::SoundCloud { token, client_id } => {
            soundcloud::likes::set(token, client_id, &profile.id, id, liked, cancelled)
        }
        Credentials::Jellyfin {
            server,
            device,
            access_token,
        } => jellyfin::likes::set(
            server,
            device,
            access_token,
            &profile.id,
            id,
            liked,
            cancelled,
        ),
    }
}

pub(super) fn status(
    provider: ProviderId,
    response: Result<Response<Body>, ureq::Error>,
) -> Result<(), ProviderError> {
    let status = response.map_err(|error| network(provider, error))?.status();
    if status.is_success() {
        Ok(())
    } else {
        Err(response_error(provider, status.as_u16()))
    }
}
