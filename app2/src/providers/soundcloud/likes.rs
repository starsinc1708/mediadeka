use std::sync::atomic::AtomicBool;

use crate::providers::{
    ErrorKind, ProviderError,
    likes::status,
    transport::{agent, check_cancelled, invalid_response},
};

use super::{API, PROVIDER, get_with, valid_track_id};

pub(in crate::providers) fn set(
    token: &str,
    client_id: &str,
    user: &str,
    id: &str,
    liked: bool,
    cancelled: &AtomicBool,
) -> Result<(), ProviderError> {
    if !valid_track_id(user) || !valid_track_id(id) {
        return Err(invalid_response(PROVIDER));
    }
    let agent = agent();
    if get_with(&agent, "/me", token, client_id)?["id"].as_u64() != user.parse().ok() {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Подключите нужный профиль SoundCloud.",
        ));
    }
    check_cancelled(cancelled)?;
    let url = format!("{API}/users/{user}/track_likes/{id}");
    let authorization = format!("OAuth {token}");
    status(
        PROVIDER,
        if liked {
            agent
                .put(&url)
                .query("client_id", client_id)
                .header("Authorization", &authorization)
                .send_empty()
        } else {
            agent
                .delete(&url)
                .query("client_id", client_id)
                .header("Authorization", &authorization)
                .call()
        },
    )
}
