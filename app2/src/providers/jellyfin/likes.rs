use std::sync::atomic::AtomicBool;

use serde_json::Value;

use crate::providers::{
    ErrorKind, ProviderError,
    transport::{agent, check_cancelled, decode, invalid_response},
};

use super::{PROVIDER, at, auth, me, profile_user, server, valid_item_id};

pub(in crate::providers) fn contains(
    address: &str,
    device: &str,
    token: &str,
    profile: &str,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<bool, ProviderError> {
    if !valid_item_id(id) {
        return Err(invalid_response(PROVIDER));
    }
    let server = server(address)?;
    let user = profile_user(&server, profile)?;
    check_cancelled(cancelled)?;
    let item: Value = decode(
        PROVIDER,
        agent()
            .get(at(&server, &format!("Items/{id}")).as_str())
            .query("userId", user)
            .header("Authorization", auth(device, Some(token))?)
            .call(),
    )?;
    item["UserData"]["IsFavorite"]
        .as_bool()
        .ok_or_else(|| invalid_response(PROVIDER))
}

pub(in crate::providers) fn set(
    address: &str,
    device: &str,
    token: &str,
    profile: &str,
    id: &str,
    liked: bool,
    cancelled: &AtomicBool,
) -> Result<(), ProviderError> {
    if !valid_item_id(id) {
        return Err(invalid_response(PROVIDER));
    }
    let server = server(address)?;
    let user = profile_user(&server, profile)?;
    if !me(&server, device, token)?.0.eq_ignore_ascii_case(user) {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Подключите нужный профиль Jellyfin.",
        ));
    }
    check_cancelled(cancelled)?;
    let url = at(&server, &format!("UserFavoriteItems/{id}"));
    let authorization = auth(device, Some(token))?;
    let agent = agent();
    let response: Value = decode(
        PROVIDER,
        if liked {
            agent
                .post(url.as_str())
                .query("userId", user)
                .header("Authorization", &authorization)
                .send_empty()
        } else {
            agent
                .delete(url.as_str())
                .query("userId", user)
                .header("Authorization", &authorization)
                .call()
        },
    )?;
    if response["IsFavorite"].as_bool() != Some(liked) {
        return Err(invalid_response(PROVIDER));
    }
    Ok(())
}
