use std::{
    io::{BufReader, Read},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::de::DeserializeOwned;
use serde_json::Value;
use ureq::{Agent, Body, http::Response};
use url::Url;

use crate::telemetry;

use super::{ErrorKind, ProviderError, ProviderId};

pub const HOME_LIMIT: usize = 24;
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
const BODY_LIMIT: u64 = 8 * 1024 * 1024;
const JSON_BUFFER_LIMIT: u64 = BODY_LIMIT / 3;
const ARTWORK_WIDTH: u64 = 320;
const YOUTUBE_MIX_ART_QUERY_LIMIT: usize = 4096;
const SOUNDCLOUD_MIX_ART_QUERY_LIMIT: usize = 4096;
const SOUNDCLOUD_MIX_ART_HOST: &str = "al.sndcdn.com";

pub(crate) fn epoch_seconds() -> Option<u64> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs())
}

pub(crate) fn expiration_after(lifetime: Option<u64>) -> Option<u64> {
    lifetime
        .zip(epoch_seconds())
        .map(|(seconds, now)| now.saturating_add(seconds))
}

pub fn agent() -> Agent {
    static AGENT: OnceLock<Agent> = OnceLock::new();
    AGENT
        .get_or_init(|| {
            Agent::config_builder()
                .timeout_connect(Some(Duration::from_secs(6)))
                .timeout_global(Some(Duration::from_secs(12)))
                .max_redirects(0)
                .http_status_as_error(false)
                .middleware(telemetry::api_request)
                .build()
                .new_agent()
        })
        .clone()
}

pub fn response_error(provider: ProviderId, status: u16) -> ProviderError {
    let kind = match status {
        401 => ErrorKind::Expired,
        403 => ErrorKind::Rejected,
        408 | 425 | 429 | 500..=599 => ErrorKind::Network,
        _ => ErrorKind::Unavailable,
    };
    ProviderError::new(
        kind,
        format!("{} вернул HTTP {status}.", provider.display_name()),
    )
}

pub fn network(provider: ProviderId, _: ureq::Error) -> ProviderError {
    ProviderError::new(
        ErrorKind::Network,
        format!(
            "Нет ответа от {}. Проверьте соединение.",
            provider.display_name()
        ),
    )
}

pub fn decode<T: DeserializeOwned>(
    provider: ProviderId,
    response: Result<Response<Body>, ureq::Error>,
) -> Result<T, ProviderError> {
    let mut response = response.map_err(|error| network(provider, error))?;
    if !response.status().is_success() {
        return Err(response_error(provider, response.status().as_u16()));
    }
    decode_body(provider, &mut response)
}

pub fn decode_body<T: DeserializeOwned>(
    provider: ProviderId,
    response: &mut Response<Body>,
) -> Result<T, ProviderError> {
    // Like ureq's JSON reader, parse small bodies from memory and stream larger ones.
    let fits_buffer = response
        .body()
        .content_length()
        .is_some_and(|size| size <= JSON_BUFFER_LIMIT);
    let mut reader =
        telemetry::api_body(response.body_mut().with_config().limit(BODY_LIMIT).reader());
    let parsed = if fits_buffer {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .map_err(|_| invalid_response(provider))?;
        serde_json::from_slice(&bytes)
    } else {
        serde_json::from_reader(BufReader::new(reader))
    };
    parsed.map_err(|_| invalid_response(provider))
}

pub fn invalid_response(provider: ProviderId) -> ProviderError {
    ProviderError::new(
        ErrorKind::InvalidResponse,
        format!("{} вернул неожиданный ответ.", provider.display_name()),
    )
}

pub fn check_cancelled(cancelled: &AtomicBool) -> Result<(), ProviderError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(ProviderError::new(ErrorKind::Cancelled, "Вход отменён."));
    }
    Ok(())
}

pub fn wait(
    cancelled: &AtomicBool,
    duration: Duration,
    deadline: Instant,
) -> Result<(), ProviderError> {
    let end = deadline.min(Instant::now() + duration);
    loop {
        check_cancelled(cancelled)?;
        let now = Instant::now();
        if now >= deadline {
            return Err(ProviderError::new(
                ErrorKind::Expired,
                "Время ожидания входа истекло. Начните подключение снова.",
            ));
        }
        if now >= end {
            return Ok(());
        }
        thread::sleep((end - now).min(Duration::from_millis(100)));
    }
}

pub fn random() -> Result<String, ProviderError> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes).map_err(|_| {
        ProviderError::new(
            ErrorKind::Unavailable,
            "Не удалось подготовить безопасный вход.",
        )
    })?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub fn field(value: &Value, name: &str) -> Option<String> {
    value[name]
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub fn https_url(value: &str, host: &str) -> Option<String> {
    let url = Url::parse(value).ok()?;
    (url.scheme() == "https"
        && url.host_str() == Some(host)
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none())
    .then(|| url.into())
}

pub fn artwork_url(provider: ProviderId, value: &str) -> Option<String> {
    let mut url = parse_artwork_url(value)?;
    url.set_fragment(None);
    if provider == ProviderId::YouTubeMusic && is_youtube_mix_art(&url) {
        return Some(url.into());
    }
    if provider == ProviderId::SoundCloud && url.host_str() == Some(SOUNDCLOUD_MIX_ART_HOST) {
        return is_soundcloud_mix_art(&url).then(|| url.into());
    }
    // The mix image uses a query to identify its artwork. CDN images use their path.
    url.set_query(None);
    artwork_source(provider, &url).then(|| url.into())
}

pub(crate) fn is_public_artwork_url(value: &str) -> bool {
    artwork_provider(value).is_some()
}

pub(crate) fn artwork_provider(value: &str) -> Option<ProviderId> {
    let url = parse_artwork_url(value)?;
    ProviderId::ALL
        .into_iter()
        .find(|&provider| artwork_source(provider, &url))
}

pub(crate) fn soundcloud_card_artwork(value: &str) -> Option<String> {
    const CDN: &str = "https://i1.sndcdn.com/";
    let (name, extension) = value.strip_prefix(CDN)?.rsplit_once("-large.")?;
    if !matches!(extension, "jpg" | "png")
        || name.is_empty()
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return None;
    }
    Some(format!("{CDN}{name}-t500x500.{extension}"))
}

fn artwork_source(provider: ProviderId, url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    if url.fragment().is_some() {
        return false;
    }
    if provider == ProviderId::SoundCloud && host == SOUNDCLOUD_MIX_ART_HOST {
        return is_soundcloud_mix_art(url);
    }
    if url.query().is_some() {
        return provider == ProviderId::YouTubeMusic && is_youtube_mix_art(url);
    }
    match provider {
        ProviderId::Spotify => host.ends_with(".scdn.co") || host.ends_with(".spotifycdn.com"),
        ProviderId::YouTubeMusic => {
            matches!(
                host,
                "i.ytimg.com"
                    | "lh3.googleusercontent.com"
                    | "yt3.googleusercontent.com"
                    | "yt3.ggpht.com"
            ) || (host == "www.gstatic.com"
                && (url.path().starts_with("/youtube/media/ytm/images/pbg/")
                    || url.path().starts_with("/music/listening_review/")))
        }
        ProviderId::YandexMusic => matches!(host, "avatars.yandex.net" | "avatars.mds.yandex.net"),
        ProviderId::SoundCloud => host.ends_with(".sndcdn.com"),
        ProviderId::Jellyfin => false,
    }
}

fn is_youtube_mix_art(url: &Url) -> bool {
    if url.host_str() != Some("music.youtube.com") || url.path() != "/image/mixart" {
        return false;
    }
    url.query()
        .and_then(|query| query.strip_prefix("r="))
        .is_some_and(|key| {
            !key.is_empty()
                && key.len() <= YOUTUBE_MIX_ART_QUERY_LIMIT
                && key
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

fn is_soundcloud_mix_art(url: &Url) -> bool {
    let Some(name) = url.path().strip_prefix("/labs-") else {
        return false;
    };
    let Some(stem) = name.strip_suffix(".jpg") else {
        return false;
    };
    !stem.is_empty()
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && url
            .query()
            .and_then(|query| query.strip_prefix("q="))
            .is_some_and(|key| {
                !key.is_empty()
                    && key.len() <= SOUNDCLOUD_MIX_ART_QUERY_LIMIT
                    && key
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            })
}

fn parse_artwork_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    (url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() != "/")
        .then_some(url)
}

pub fn artwork_from_images(provider: ProviderId, images: &Value) -> Option<String> {
    images
        .as_array()?
        .iter()
        .filter_map(|image| {
            let url = artwork_url(provider, image["url"].as_str()?)?;
            let distance = image["width"]
                .as_u64()
                .map_or(u64::MAX, |width| width.abs_diff(ARTWORK_WIDTH));
            Some((url, distance))
        })
        .min_by_key(|(_, distance)| *distance)
        .map(|(url, _)| url)
}

pub fn valid_token(token: &str, max: usize) -> bool {
    !token.is_empty()
        && token.len() <= max
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.~+/=".contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn provider_agent_reuses_a_connection_across_calls() {
        use std::{
            io::{BufRead, Write},
            net::TcpListener,
        };

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let url = format!("http://{}/catalog", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            // Fail a second connection immediately instead of waiting for the client timeout.
            drop(listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut connection = BufReader::new(stream);
            for _ in 0..2 {
                let mut headers = String::new();
                while !headers.ends_with("\r\n\r\n") {
                    assert_ne!(connection.read_line(&mut headers).unwrap(), 0);
                }
                assert!(headers.starts_with("GET /catalog HTTP/1.1"));
                connection
                    .get_mut()
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                    .unwrap();
            }
        });
        for _ in 0..2 {
            let value: Value = decode(ProviderId::Jellyfin, agent().get(&url).call()).unwrap();
            assert_eq!(value, json!({}));
        }
        server.join().unwrap();
    }

    #[test]
    fn soundcloud_enlarges_static_card_thumbnails_only() {
        for extension in ["jpg", "png"] {
            let thumbnail = format!("https://i1.sndcdn.com/artworks-abc-large.{extension}");
            let larger = format!("https://i1.sndcdn.com/artworks-abc-t500x500.{extension}");
            assert_eq!(
                soundcloud_card_artwork(&thumbnail).as_deref(),
                Some(larger.as_str())
            );
            assert!(is_public_artwork_url(&larger));
            assert!(soundcloud_card_artwork(&larger).is_none());
        }
        for url in [
            "https://al.sndcdn.com/labs-abcd1234-0-t500x500.jpg?q=YWJj",
            "https://i1.sndcdn.com/artworks-abc-original.jpg",
            "https://i1.sndcdn.com/artworks-abc-large.jpg?size=10",
            "https://i1.sndcdn.com/unknown/path-large.jpg",
            "https://i1.sndcdn.com.example.org/artworks-abc-large.jpg",
        ] {
            assert!(soundcloud_card_artwork(url).is_none(), "{url}");
        }
    }

    #[test]
    fn soundcloud_mix_art_keeps_only_the_cdn_image_directive() {
        const MIX: &str = "https://al.sndcdn.com/labs-abcd1234-0-t500x500.jpg?q=YWJj";
        let provider = ProviderId::SoundCloud;
        assert_eq!(
            artwork_url(provider, &format!("{MIX}#section")).as_deref(),
            Some(MIX)
        );
        assert!(is_public_artwork_url(MIX));
        for invalid in [
            "https://al.sndcdn.com/labs-abcd1234-0-t500x500.jpg",
            "https://al.sndcdn.com/labs-abcd1234-0-t500x500.jpg?q=YWJj&other=value",
            "https://al.sndcdn.com/profile.jpg?q=YWJj",
            "https://al.sndcdn.com/labs-test/image.jpg?q=YWJj",
            "https://al.sndcdn.com.example.org/labs-abcd1234-0-t500x500.jpg?q=YWJj",
        ] {
            assert!(artwork_url(provider, invalid).is_none(), "{invalid}");
            assert!(!is_public_artwork_url(invalid), "{invalid}");
        }
    }

    #[test]
    fn artwork_keeps_only_public_cdn_addresses() {
        const COVER: &str = "https://i.scdn.co/image/cover";
        let provider = ProviderId::Spotify;
        let images = json!([
            {"url": "https://i.scdn.co/image/big", "width": 640},
            {"url": "https://i.scdn.co/image/cover?access_token=test#private", "width": 300},
            {"url": "https://i.scdn.co.example.com/image/wrong-host", "width": 320},
        ]);
        assert_eq!(
            artwork_from_images(provider, &images).as_deref(),
            Some(COVER)
        );
        assert!(is_public_artwork_url(COVER));
        assert!(!is_public_artwork_url(
            "https://i.scdn.co/image/cover?access_token=test"
        ));
        assert!(!is_public_artwork_url(
            "https://user:pass@i.scdn.co/image/cover"
        ));
        assert!(!is_public_artwork_url("http://i.scdn.co/image/cover"));
        assert!(artwork_url(ProviderId::Jellyfin, COVER).is_none());
    }

    #[test]
    fn youtube_mix_and_playlist_images_keep_only_their_required_parts() {
        const MIX: &str = "https://music.youtube.com/image/mixart?r=cover-_01";
        const LIKED: &str =
            "https://www.gstatic.com/youtube/media/ytm/images/pbg/liked-songs-delhi-192.png";
        const RECAP: &str = "https://www.gstatic.com/music/listening_review/recap.png";
        let provider = ProviderId::YouTubeMusic;
        for cover in [MIX, LIKED, RECAP] {
            assert_eq!(artwork_url(provider, cover).as_deref(), Some(cover));
            assert!(is_public_artwork_url(cover));
            assert_eq!(artwork_provider(cover), Some(provider));
        }
        assert_eq!(
            artwork_url(provider, &format!("{LIKED}?tracking=sample#example")).as_deref(),
            Some(LIKED),
        );
        for unsupported in [
            "https://music.youtube.com/image/mixart",
            "https://music.youtube.com/image/mixart?r=",
            "https://music.youtube.com/image/mixart?r=cover&r=another",
            "https://music.youtube.com/image/mixart?r=cover&session=example",
            "https://music.youtube.com/image/mixart?r=https%3A%2F%2Fexample.com",
            "https://music.youtube.com/image/mixart/other?r=cover",
            "https://music.youtube.com/api/mixart?r=cover",
            "https://music.youtube.com.example.com/image/mixart?r=cover",
            "https://www.gstatic.com/not-music/cover.png",
        ] {
            assert!(
                artwork_url(provider, unsupported).is_none(),
                "{unsupported}"
            );
            assert!(!is_public_artwork_url(unsupported), "{unsupported}");
        }
    }
}
