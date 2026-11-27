use std::sync::atomic::AtomicBool;

use serde_json::Value;

use crate::providers::{
    Credentials, ErrorKind, Profile, ProviderError,
    transport::{agent, check_cancelled, decode, invalid_response},
};

use super::{PROVIDER, api_endpoint, auth::confirm_profile, get, numeric_id};

pub(in crate::providers) fn contains(
    credentials: &Credentials,
    profile: &Profile,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<bool, ProviderError> {
    let user = &profile.id;
    if user.is_empty() || !user.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_response(PROVIDER));
    }
    let Credentials::YandexMusic { access_token, .. } = credentials else {
        return Err(invalid_response(PROVIDER));
    };
    check_cancelled(cancelled)?;
    let data = get(
        &agent(),
        &format!("/users/{user}/likes/tracks"),
        access_token,
    )?;
    let tracks = data["result"]["library"]["tracks"]
        .as_array()
        .ok_or_else(|| invalid_response(PROVIDER))?;
    let id = id.split_once(':').map_or(id, |(track, _)| track);
    Ok(tracks
        .iter()
        .any(|track| numeric_id(&track["id"]).as_deref() == Some(id)))
}

pub(in crate::providers) fn set(
    credentials: &Credentials,
    profile: &Profile,
    id: &str,
    liked: bool,
    cancelled: &AtomicBool,
) -> Result<(), ProviderError> {
    let user = &profile.id;
    if user.is_empty() || !user.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_response(PROVIDER));
    }
    let Credentials::YandexMusic { access_token, .. } = credentials else {
        return Err(invalid_response(PROVIDER));
    };
    check_cancelled(cancelled)?;
    let agent = agent();
    if confirm_profile(&agent, access_token)?.id != *user {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Подключите нужный профиль Яндекс Музыки.",
        ));
    }
    check_cancelled(cancelled)?;
    let action = if liked { "add-multiple" } else { "remove" };
    let response: Value = decode(
        PROVIDER,
        agent
            .post(&api_endpoint(&format!(
                "/users/{user}/likes/tracks/{action}"
            )))
            .header("Authorization", format!("OAuth {access_token}"))
            .send_form([("track-ids", id)]),
    )?;
    if response["result"]
        .get("revision")
        .is_none_or(Value::is_null)
    {
        return Err(invalid_response(PROVIDER));
    }
    Ok(())
}
