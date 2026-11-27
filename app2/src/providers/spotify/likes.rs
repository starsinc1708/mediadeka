use std::sync::atomic::AtomicBool;

use crate::providers::{
    Credentials, ErrorKind, Profile, ProviderError,
    likes::status,
    transport::{agent, check_cancelled, decode, invalid_response},
};

use super::{
    PROVIDER, library_access_token,
    rate_limit::{LIBRARY, Step},
};

const ENDPOINT: &str = "https://api.spotify.com/v1/me/library";
const CONTAINS: &str = "https://api.spotify.com/v1/me/library/contains";

pub(in crate::providers) fn contains(
    credentials: &Credentials,
    profile: &Profile,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<bool, ProviderError> {
    let token = library_access_token(credentials, profile, cancelled)?;
    let agent = agent();
    let uri = format!("spotify:track:{id}");
    let authorization = format!("Bearer {token}");
    let response = LIBRARY.call(Step::Library, None, || {
        agent
            .get(CONTAINS)
            .query("uris", &uri)
            .header("Authorization", &authorization)
            .call()
    })?;
    check_cancelled(cancelled)?;
    let marks: Vec<bool> = decode(PROVIDER, Ok(response))?;
    marks
        .first()
        .copied()
        .ok_or_else(|| invalid_response(PROVIDER))
}

pub(in crate::providers) fn set(
    credentials: &Credentials,
    profile: &Profile,
    id: &str,
    liked: bool,
    cancelled: &AtomicBool,
) -> Result<(), ProviderError> {
    let token = library_access_token(credentials, profile, cancelled)?;
    let agent = agent();
    let uri = format!("spotify:track:{id}");
    let authorization = format!("Bearer {token}");
    let response = LIBRARY.call(Step::Library, None, || {
        if liked {
            agent
                .put(ENDPOINT)
                .query("uris", &uri)
                .header("Authorization", &authorization)
                .send_empty()
        } else {
            agent
                .delete(ENDPOINT)
                .query("uris", &uri)
                .header("Authorization", &authorization)
                .call()
        }
    })?;
    // Once the server has replied to a write, report that result even if the
    // originating UI closed. The next read must see what happened remotely.
    status(PROVIDER, Ok(response))
    .map_err(|error| {
        if error.kind == ErrorKind::Rejected {
            ProviderError::new(
                error.kind,
                "Spotify отклонил запись (HTTP 403). Проверьте доступ к медиатеке Spotify в подключении.",
            )
        } else {
            error
        }
    })
}
