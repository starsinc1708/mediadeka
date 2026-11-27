mod account;
#[cfg(windows)]
mod browser;
pub(super) mod catalog;
mod collection;
mod download;
mod feed;
mod home;
mod item;
pub(super) mod likes;
mod playlist;
mod radio;
mod stream;
pub(crate) use account::clear_browser_session;
#[cfg(all(test, windows))]
pub(super) use account::sign_in;
pub(super) use account::{browser_sign_in, restore, restore_in_profile};
pub(super) use collection::read_collection;
pub(super) use download::original;

pub use home::SoundCloudShelf;
pub(crate) use home::SoundCloudUpdate;
#[cfg(test)]
pub(crate) use home::load as load_soundcloud_home_for_test;
pub(crate) use playlist::{
    artist_station_from_slug, artist_station_id, artist_station_url, personal_playlist_id,
    system_playlist_id,
};
pub(super) use playlist::{playlist, playlist_with_progress, valid_playlist_id};
pub(crate) use radio::track_radio;
pub(super) use stream::direct_stream;

use std::sync::atomic::AtomicBool;

use serde_json::Value;
use url::Url;

use super::{ErrorKind, ProviderError, ProviderId, transport::*};

const PROVIDER: ProviderId = ProviderId::SoundCloud;
const API: &str = "https://api-v2.soundcloud.com";
const TOKEN_LIMIT: usize = 2048;

#[cfg(all(test, windows, feature = "capture"))]
static TEST_API: std::sync::OnceLock<std::net::SocketAddr> = std::sync::OnceLock::new();

#[cfg(all(test, windows, feature = "capture"))]
pub(crate) fn use_loopback_for_test(address: std::net::SocketAddr) {
    assert!(address.ip().is_loopback());
    TEST_API.set(address).expect("one SoundCloud test process");
}

fn valid_track_id(value: &str) -> bool {
    value.parse::<u64>().is_ok_and(|id| id != 0) && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn resolve(
    agent: &ureq::Agent,
    url: &Url,
    token: &str,
    client_id: &str,
) -> Result<Value, ProviderError> {
    let query = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("url", url.as_str())
        .finish();
    get_with(agent, &format!("/resolve?{query}"), token, client_id)
}

fn get_with(
    agent: &ureq::Agent,
    path: &str,
    token: &str,
    client_id: &str,
) -> Result<Value, ProviderError> {
    if !valid_token(token, TOKEN_LIMIT) || !valid_client_id(client_id) {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Подключение SoundCloud недействительно.",
        ));
    }
    let url = format!("{API}{path}");
    #[cfg(all(test, windows, feature = "capture"))]
    let url = TEST_API
        .get()
        .map_or(url, |address| format!("http://{address}{path}"));
    decode(
        PROVIDER,
        agent
            .get(&url)
            .query("client_id", client_id)
            .header("Authorization", format!("OAuth {token}"))
            .header("Accept", "application/json")
            .call(),
    )
    .map_err(|error| {
        if error.kind == ErrorKind::Expired {
            ProviderError::new(
                ErrorKind::Expired,
                "Сеанс SoundCloud истёк (HTTP 401). Войдите снова.",
            )
        } else {
            error
        }
    })
}

pub(super) fn home(
    token: &str,
    client_id: &str,
    profile_id: &str,
    cancelled: &AtomicBool,
) -> Result<SoundCloudUpdate, ProviderError> {
    let agent = agent();
    home::load(profile_id, cancelled, |path| {
        get_with(&agent, path, token, client_id)
    })
}

fn valid_client_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}
