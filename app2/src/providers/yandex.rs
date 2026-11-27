mod auth;
pub(super) mod catalog;
mod collection;
mod editorial;
pub(super) mod likes;
pub(super) use collection::read_collection;
mod landing;
pub(super) mod lyrics;
mod playlists;
pub(crate) mod station;
mod streams;
#[cfg(all(test, windows))]
pub(crate) mod test_server;

pub(super) use auth::{renew, restore, sign_in};
pub(crate) use catalog::{tag_slug, tag_url};
pub(super) use playlists::playlist;
pub(crate) use playlists::{playlist_url, valid_playlist_id};
pub(super) use streams::direct_stream;

use std::sync::atomic::AtomicBool;

use serde_json::Value;

use super::{ErrorKind, Home, ProviderError, ProviderId, transport::*};

const PROVIDER: ProviderId = ProviderId::YandexMusic;
const API: &str = "https://api.music.yandex.net";
const TOKEN_LIMIT: usize = 4096;

fn api_endpoint(path: &str) -> String {
    endpoint(API, path)
}

fn endpoint(origin: &str, path: &str) -> String {
    #[cfg(all(test, windows))]
    if let Some(endpoint) = test_server::endpoint(path) {
        return endpoint;
    }
    format!("{origin}{path}")
}

fn get(agent: &ureq::Agent, path: &str, token: &str) -> Result<Value, ProviderError> {
    check_access(token)?;
    decode(
        PROVIDER,
        agent
            .get(&api_endpoint(path))
            .header("Authorization", format!("OAuth {token}"))
            .call(),
    )
}

fn check_access(token: &str) -> Result<(), ProviderError> {
    if !valid_token(token, TOKEN_LIMIT) {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Подключение Яндекс Музыки недействительно.",
        ));
    }
    Ok(())
}

pub fn home(token: &str, user: &str, cancelled: &AtomicBool) -> Result<Home, ProviderError> {
    if user.is_empty() || !user.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_response(PROVIDER));
    }
    check_cancelled(cancelled)?;
    check_access(token)?;
    let agent = agent();
    let (playlists, wheel, editorial) = std::thread::scope(|scope| {
        let playlists =
            scope.spawn(|| get(&agent, &format!("/users/{user}/playlists/list"), token));
        let wheel = scope.spawn(|| {
            decode(
                PROVIDER,
                agent
                    .post(&format!("{API}/wheel/new"))
                    .header("Authorization", format!("OAuth {token}"))
                    .header("Content-Type", "application/json")
                    .send("{}"),
            )
        });
        let editorial = editorial::Editorial::fetch(&agent, token, cancelled);
        (
            playlists.join().expect("Yandex playlists request"),
            wheel.join().expect("Yandex wheel request"),
            editorial,
        )
    });
    check_cancelled(cancelled)?;
    let legacy = landing::parse(
        user,
        playlists.as_ref().unwrap_or(&Value::Null),
        wheel.as_ref().unwrap_or(&Value::Null),
        !editorial.has_waves(),
    );
    let legacy_available = legacy.is_some();
    let mut home = legacy.unwrap_or_else(|| Home {
        title: PROVIDER.display_name().into(),
        items: Vec::new(),
        sections: Vec::new(),
    });
    editorial.append(&mut home);
    if !legacy_available && home.items.is_empty() {
        return Err(playlists
            .err()
            .or_else(|| wheel.err())
            .unwrap_or_else(|| invalid_response(PROVIDER)));
    }
    Ok(home)
}

fn numeric_id(value: &Value) -> Option<String> {
    value.as_u64().map(|id| id.to_string()).or_else(|| {
        value
            .as_str()
            .filter(|id| !id.is_empty() && id.bytes().all(|byte| byte.is_ascii_digit()))
            .map(str::to_owned)
    })
}
