use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use ureq::Agent;
use url::Url;

use crate::{
    music::lyrics::{Lyrics, LyricsVersions, Source, parse_lrc},
    providers::{ErrorKind, ProviderError, catalog, transport::*},
};

use super::{PROVIDER, api_endpoint, check_access, playlists::valid_track_id};

#[cfg(all(test, windows))]
mod acceptance;
#[cfg(all(test, windows))]
mod tests;

// MarshalX and aeviww sign the lyrics request with this Android client key.
const SIGNING_KEY: &[u8] = b"p93jhgh689SBReK6ghtw62";
const MUSIC_CLIENT: &str = "YandexMusicAndroid/24023621";
const TEXT_LIMIT: u64 = 256 * 1024;
const FORMAT_BUDGET: Duration = Duration::from_secs(6);
const ALTERNATE_BUDGET: Duration = Duration::from_secs(3);

pub(in crate::providers) fn supports(token: &str) -> bool {
    check_access(token).is_ok()
}

pub(in crate::providers) fn fetch(
    token: &str,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<Option<LyricsVersions>, ProviderError> {
    check_access(token)?;
    if !valid_track_id(id) {
        return Err(catalog::invalid_id(PROVIDER));
    }
    let client = agent();
    let synchronized = read_format(&client, token, id, Format::Lrc, FORMAT_BUDGET, cancelled);
    let text_budget = if matches!(synchronized, Ok(Some(_))) {
        ALTERNATE_BUDGET
    } else {
        FORMAT_BUDGET
    };
    let plain = read_format(&client, token, id, Format::Text, text_budget, cancelled);
    check_cancelled(cancelled)?;
    match (synchronized, plain) {
        (Ok(Some(timed)), Ok(plain)) => Ok(Some(LyricsVersions::from_source(timed, plain))),
        (Ok(None), Ok(plain)) => Ok(plain.map(|plain| LyricsVersions::from_source(plain, None))),
        (Ok(Some(lyrics)), Err(_)) | (Err(_), Ok(Some(lyrics))) => Ok(Some(lyrics.into())),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

#[derive(Clone, Copy)]
enum Format {
    Lrc,
    Text,
}

impl Format {
    const fn preference(self) -> &'static str {
        match self {
            Self::Lrc => "LRC",
            Self::Text => "TEXT",
        }
    }
}

fn read_format(
    client: &Agent,
    token: &str,
    id: &str,
    format: Format,
    budget: Duration,
    cancelled: &AtomicBool,
) -> Result<Option<Lyrics>, ProviderError> {
    check_cancelled(cancelled)?;
    let deadline = Instant::now() + budget;
    let Some(result) = descriptor(client, token, id, format, deadline)? else {
        return Ok(None);
    };
    check_cancelled(cancelled)?;
    let Some(text) = text(client, &result, deadline)? else {
        return Ok(None);
    };
    if text.len() as u64 > TEXT_LIMIT {
        return Err(invalid_response(PROVIDER));
    }
    let timed = parse_lrc(Source::YandexMusic, &text);
    let mut lyrics = match format {
        Format::Lrc => timed,
        Format::Text if timed.is_none() => Some(Lyrics::plain(Source::YandexMusic, &text)),
        Format::Text => None,
    }
    .filter(Lyrics::usable)
    .ok_or_else(|| invalid_response(PROVIDER))?;
    let major = &result["major"];
    lyrics.attribution = field(major, "prettyName").or_else(|| field(major, "name"));
    Ok(Some(lyrics))
}

fn descriptor(
    agent: &Agent,
    token: &str,
    id: &str,
    format: Format,
    deadline: Instant,
) -> Result<Option<Value>, ProviderError> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid_response(PROVIDER))?
        .as_secs();
    let track_id = id.split_once(':').map_or(id, |(track, _)| track);
    let mut mac = Hmac::<Sha256>::new_from_slice(SIGNING_KEY).expect("HMAC accepts any key length");
    mac.update(track_id.as_bytes());
    mac.update(timestamp.to_string().as_bytes());
    let signature = STANDARD.encode(mac.finalize().into_bytes());
    let mut response = agent
        .get(&api_endpoint(&format!("/tracks/{id}/lyrics")))
        .query("format", format.preference())
        .query("timeStamp", timestamp.to_string())
        .query("sign", signature)
        .header("Authorization", format!("OAuth {token}"))
        .header("X-Yandex-Music-Client", MUSIC_CLIENT)
        .header("Accept", "application/json")
        .config()
        .timeout_global(Some(remaining(deadline)?))
        .build()
        .call()
        .map_err(|error| network(PROVIDER, error))?;
    match response.status().as_u16() {
        404 | 204 => Ok(None),
        200 => {
            let value: Value = decode_body(PROVIDER, &mut response)?;
            value
                .get("result")
                .filter(|node| node.is_object())
                .map(|result| Some(result.clone()))
                .ok_or_else(|| invalid_response(PROVIDER))
        }
        status => Err(response_error(PROVIDER, status)),
    }
}

fn text(agent: &Agent, result: &Value, deadline: Instant) -> Result<Option<String>, ProviderError> {
    if let Some(value) = result["downloadUrl"].as_str() {
        let url = lyrics_url(value).ok_or_else(|| invalid_response(PROVIDER))?;
        let mut response = agent
            .get(url.as_str())
            .config()
            .timeout_global(Some(remaining(deadline)?))
            .build()
            .call()
            .map_err(|error| network(PROVIDER, error))?;
        match response.status().as_u16() {
            404 | 204 => return Ok(None),
            200 => {}
            status => return Err(response_error(PROVIDER, status)),
        }
        let text = response
            .body_mut()
            .with_config()
            .limit(TEXT_LIMIT)
            .read_to_vec()
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or_else(|| invalid_response(PROVIDER))?;
        return Ok(Some(text.trim_start_matches('\u{feff}').to_owned()));
    }
    let inline = &result["lyrics"];
    inline
        .as_str()
        .map(str::to_owned)
        .or_else(|| {
            ["fullLyrics", "lyrics", "text"]
                .into_iter()
                .find_map(|key| field(inline, key))
        })
        .map(Some)
        .ok_or_else(|| invalid_response(PROVIDER))
}

fn remaining(deadline: Instant) -> Result<Duration, ProviderError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(ProviderError::new(
            ErrorKind::Network,
            "Истекло время ожидания текста Яндекс Музыки.",
        ));
    }
    Ok(remaining)
}

fn lyrics_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    let host = url.host_str()?;
    (url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.fragment().is_none()
        && (host.ends_with(".yandex.net") || host.ends_with(".yandexcloud.net")))
    .then_some(url)
}
