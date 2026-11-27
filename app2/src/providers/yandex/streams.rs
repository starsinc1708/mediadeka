use std::{
    sync::atomic::AtomicBool,
    time::{SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::STANDARD_NO_PAD};
use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use serde_json::Value;
use sha2::Sha256;
use url::Url;

use crate::providers::{
    ErrorKind, ProviderError,
    streams::{DirectStream, StreamOffer},
    transport::*,
};

use super::{PROVIDER, api_endpoint, check_access, get, playlists::valid_track_id};

const DESKTOP_CLIENT: &str = "YandexMusicDesktopAppWindows/5.60.2";
// Protocol signing constants from the playback reference. Account authorization still
// uses the existing profile and the server chooses which source files are available.
const FILE_INFO_KEY: &[u8] = b"kzqU4XhfCaY6B6JTHODeq5";
const DOWNLOAD_SALT: &str = "XGRlBW9FXlekgbPrRHuSiA";
const RESPONSE_LIMIT: u64 = 16 * 1024;
const RAW_TRANSPORT: &str = "raw";

pub(crate) fn direct_stream(
    token: &str,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<StreamOffer, ProviderError> {
    if !valid_track_id(id) {
        return Err(ProviderError::new(
            ErrorKind::InvalidInput,
            "Некорректный идентификатор трека Яндекс Музыки.",
        ));
    }
    check_access(token)?;
    let agent = agent();
    let status = get(&agent, "/account/status", token)?;
    check_cancelled(cancelled)?;
    require_subscription(&status)?;
    let canonical_id = id.split_once(':').map_or(id, |(track, _)| track);
    let preview = match raw_stream(&agent, token, canonical_id) {
        Ok(StreamOffer::Full(stream)) => return Ok(StreamOffer::Full(stream)),
        Ok(StreamOffer::Preview) => true,
        Err(error) if error.kind == ErrorKind::Expired => return Err(error),
        Err(_) => false,
    };
    check_cancelled(cancelled)?;
    match download_stream(&agent, token, id, cancelled) {
        Err(error) if preview && error.kind == ErrorKind::Unavailable => Ok(StreamOffer::Preview),
        result => result,
    }
}

fn require_subscription(status: &Value) -> Result<(), ProviderError> {
    match status["result"]["plus"]["hasPlus"].as_bool() {
        Some(true) => Ok(()),
        Some(false) => Err(ProviderError::new(
            ErrorKind::Rejected,
            "Для полного потока Яндекс Музыки нужна активная подписка.",
        )),
        None => Err(invalid_response(PROVIDER)),
    }
}

fn raw_stream(agent: &ureq::Agent, token: &str, id: &str) -> Result<StreamOffer, ProviderError> {
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| invalid_response(PROVIDER))?
        .as_secs();
    let quality = "lossless";
    let codecs = "flac-mp4,flac";
    let transport = RAW_TRANSPORT;
    let message = format!("{epoch}{id}{quality}{}{transport}", codecs.replace(',', ""));
    let mut signer =
        Hmac::<Sha256>::new_from_slice(FILE_INFO_KEY).expect("HMAC accepts any key length");
    signer.update(message.as_bytes());
    let signature = STANDARD_NO_PAD.encode(signer.finalize().into_bytes());
    let data: Value = decode(
        PROVIDER,
        agent
            .get(&api_endpoint("/get-file-info"))
            .header("Authorization", format!("OAuth {token}"))
            .header("X-Yandex-Music-Client", DESKTOP_CLIENT)
            .query("ts", epoch.to_string())
            .query("trackId", id)
            .query("quality", quality)
            .query("codecs", codecs)
            .query("transports", transport)
            .query("sign", signature)
            .call(),
    )?;
    raw_offer(&data).ok_or_else(no_stream)
}

fn raw_offer(data: &Value) -> Option<StreamOffer> {
    let result = &data["result"];
    let info = result
        .get("downloadInfo")
        .or_else(|| result.get("download_info"))?;
    if info["preview"] == true {
        return Some(StreamOffer::Preview);
    }
    let codec = info["codec"].as_str()?;
    // Yandex can return a full lossy stream even for a lossless request.
    if !playable_codec(codec)
        || info["transport"]
            .as_str()
            .is_some_and(|mode| mode != RAW_TRANSPORT)
        || !info["key"].is_null()
        || !info["keyId"].is_null()
    {
        return None;
    }
    let url = std::iter::once(&info["url"])
        .chain(info["urls"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .find_map(stream_url)?;
    Some(StreamOffer::Full(DirectStream {
        url: url.into(),
        codec: Some(codec.into()),
        bitrate_kbps: positive_bitrate(info.get("bitrate").or_else(|| info.get("bitrateInKbps"))),
    }))
}

fn positive_bitrate(value: Option<&Value>) -> Option<u32> {
    u32::try_from(value?.as_u64()?)
        .ok()
        .filter(|bitrate| *bitrate > 0)
}

fn playable_codec(codec: &str) -> bool {
    matches!(
        codec,
        "flac" | "flac-mp4" | "aac" | "he-aac" | "opus" | "mp3"
    )
}

fn download_stream(
    agent: &ureq::Agent,
    token: &str,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<StreamOffer, ProviderError> {
    let data = get(
        agent,
        &format!("/tracks/{id}/download-info?can_use_streaming=true"),
        token,
    )?;
    let rows = data["result"]
        .as_array()
        .ok_or_else(|| invalid_response(PROVIDER))?;
    let mut sources = rows
        .iter()
        .filter_map(|row| {
            if row["preview"].as_bool() != Some(false) {
                return None;
            }
            let codec = row["codec"].as_str()?;
            if !playable_codec(codec) {
                return None;
            }
            let source = stream_url(row["downloadInfoUrl"].as_str()?)?;
            let bitrate = positive_bitrate(row.get("bitrateInKbps"));
            Some((codec, bitrate, source))
        })
        .collect::<Vec<_>>();
    sources.sort_by_key(|(codec, bitrate, _)| {
        std::cmp::Reverse((codec.starts_with("flac"), *bitrate))
    });
    if sources.is_empty() && rows.iter().any(|row| row["preview"] == true) {
        return Ok(StreamOffer::Preview);
    }
    let mut last_error = no_stream();
    for (codec, bitrate_kbps, url) in sources {
        check_cancelled(cancelled)?;
        match download_descriptor(agent, url, token) {
            Ok(url) => {
                return Ok(StreamOffer::Full(DirectStream {
                    url,
                    codec: Some(codec.into()),
                    bitrate_kbps,
                }));
            }
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn download_descriptor(
    agent: &ureq::Agent,
    url: Url,
    token: &str,
) -> Result<String, ProviderError> {
    let mut response = agent
        .get(url.as_str())
        .header("Authorization", format!("OAuth {token}"))
        .call()
        .map_err(|error| network(PROVIDER, error))?;
    if !response.status().is_success() {
        return Err(response_error(PROVIDER, response.status().as_u16()));
    }
    let xml = response
        .body_mut()
        .with_config()
        .limit(RESPONSE_LIMIT)
        .read_to_string()
        .map_err(|_| invalid_response(PROVIDER))?;
    descriptor_url(&xml)
        .map(Into::into)
        .ok_or_else(|| invalid_response(PROVIDER))
}

fn descriptor_url(xml: &str) -> Option<Url> {
    let tag = |name| {
        xml.split_once(&format!("<{name}>"))
            .and_then(|(_, xml)| xml.split_once(&format!("</{name}>")))
            .map(|(value, _)| value.trim())
            .filter(|value| !value.is_empty())
    };
    let (Some(host), Some(path), Some(ts), Some(salt)) =
        (tag("host"), tag("path"), tag("ts"), tag("s"))
    else {
        return None;
    };
    let path = path
        .strip_prefix('/')
        .filter(|path| !path.starts_with('/'))?;
    if !host
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b".-".contains(&byte))
        // The CDN uses this field as a signed path segment and can send it in
        // URL-safe encoding instead of decimal digits.
        || !ts.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"-_".contains(&byte))
    {
        return None;
    }
    let signature = Md5::digest(format!("{DOWNLOAD_SALT}{path}{salt}").as_bytes());
    stream_url(&format!("https://{host}/get-mp3/{signature:x}/{ts}/{path}"))
}

fn stream_url(value: &str) -> Option<Url> {
    let url = match value.strip_prefix("//") {
        Some(_) => Url::parse(&format!("https:{value}")),
        None => Url::parse(value),
    }
    .ok()?;
    #[cfg(all(test, windows))]
    if super::test_server::accepts_stream(&url) {
        return Some(url);
    }
    let host = url.host_str()?;
    (url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.fragment().is_none()
        && ["yandex.net", "yandex.ru", "yandex.com"]
            .iter()
            .any(|domain| {
                host.strip_suffix(domain)
                    .is_some_and(|prefix| prefix.ends_with('.'))
            }))
    .then_some(url)
}

fn no_stream() -> ProviderError {
    ProviderError::new(
        ErrorKind::Unavailable,
        "Яндекс Музыка не предоставила полный аудиопоток для этого трека.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn accepts_full_mp3_returned_instead_of_requested_flac() {
        for codec in ["mp3", "flac-mp4"] {
            let info = json!({"result": {"downloadInfo": {
                "codec": codec, "transport": "raw", "bitrate": 320,
                "urls": ["https://strm.yandex.net/audio"]
            }}});
            let Some(StreamOffer::Full(stream)) = raw_offer(&info) else {
                panic!("expected full audio");
            };
            assert_eq!(stream.codec.as_deref(), Some(codec));
            assert_eq!(stream.bitrate_kbps, Some(320));
            assert_eq!(
                Url::parse(&stream.url).unwrap().host_str(),
                Some("strm.yandex.net")
            );
        }
    }

    #[test]
    fn raw_offer_still_requires_full_unencrypted_audio_on_a_supported_host() {
        let offer =
            json!({"codec": "mp3", "transport": "raw", "url": "https://strm.yandex.net/audio"});
        for (field, value) in [
            ("key", json!("encrypted")),
            ("codec", json!("unknown")),
            ("transport", json!("encraw")),
            ("url", json!("https://example.com/audio")),
        ] {
            let mut rejected = offer.clone();
            rejected[field] = value;
            assert!(raw_offer(&json!({"result": {"downloadInfo": rejected}})).is_none());
        }
    }

    #[test]
    fn distinguishes_preview_from_refusal() {
        assert!(matches!(
            raw_offer(&json!({"result": {"downloadInfo": {"preview": true, "codec": "mp3"}}})),
            Some(StreamOffer::Preview)
        ));
        assert!(raw_offer(&json!({})).is_none());
    }

    #[test]
    fn requires_a_confirmed_profile_subscription() {
        let status = |has_plus| json!({"result":{"plus":{"hasPlus":has_plus}}});
        assert!(require_subscription(&status(true)).is_ok());
        assert_eq!(
            require_subscription(&status(false)).unwrap_err().kind,
            ErrorKind::Rejected
        );
        assert_eq!(
            require_subscription(&json!({})).unwrap_err().kind,
            ErrorKind::InvalidResponse
        );
    }

    #[test]
    fn descriptor_preserves_safe_opaque_segments_and_rejects_path_injection() {
        let xml = |ts| {
            format!(
                "<download-info><host>strm.yandex.net</host><path>/audio</path><ts>{ts}</ts><s>salt</s></download-info>"
            )
        };
        for ts in ["123456789", "AbCD_ef-01234567"] {
            let url = descriptor_url(&xml(ts)).unwrap();
            assert!(url.path().ends_with(&format!("/{ts}/audio")));
        }
        for ts in ["../item", "..", "ab?x=1", "ab%2fitem", "ab#item"] {
            assert!(descriptor_url(&xml(ts)).is_none());
        }
    }
}
