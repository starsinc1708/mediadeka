use std::sync::atomic::AtomicBool;

use serde_json::Value;
use ureq::http::{
    HeaderMap,
    header::{ACCEPT_ENCODING, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
};
use url::Url;

use crate::providers::{
    ErrorKind, ProviderError,
    downloads::{DownloadCandidate, format_hint},
    transport::*,
};

use super::{PROVIDER, get_with, stream::track_snapshot};

pub(in crate::providers) fn original(
    token: &str,
    client_id: &str,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<DownloadCandidate, ProviderError> {
    check_cancelled(cancelled)?;
    let agent = agent();
    let track = track_snapshot(&agent, id, token, client_id)?;
    check_cancelled(cancelled)?;
    let track_id = downloadable_id(&track).ok_or_else(|| {
        ProviderError::new(
            ErrorKind::Unavailable,
            "SoundCloud не разрешил скачивание оригинала.",
        )
    })?;
    let response = get_with(
        &agent,
        &format!("/tracks/{track_id}/download"),
        token,
        client_id,
    )?;
    check_cancelled(cancelled)?;
    let url = response["redirectUri"]
        .as_str()
        .and_then(original_cdn)
        .ok_or_else(|| invalid_response(PROVIDER))?;

    // The CDN receives the signed URL only. Account authorization stays on the metadata API.
    let mut candidate = DownloadCandidate::new(url);
    candidate.bytes = track["original_content_size"]
        .as_u64()
        .filter(|&size| size > 0);
    candidate.format = track["original_format"].as_str().and_then(format_hint);
    if (candidate.bytes.is_none() || candidate.format.is_none())
        && let Ok(response) = agent
            .head(candidate.url.as_str())
            .header(ACCEPT_ENCODING, "identity")
            .call()
        && response.status().is_success()
    {
        let headers = response.headers();
        candidate.bytes = headers
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|&size| size > 0)
            .or(candidate.bytes);
        candidate.format = header_format(headers).or(candidate.format);
    }
    check_cancelled(cancelled)?;
    candidate.bitrate_bps = candidate
        .bytes
        .zip(track["duration"].as_u64().filter(|&duration| duration > 0))
        .and_then(|(bytes, duration)| u32::try_from(bytes.checked_mul(8000)? / duration).ok())
        .filter(|&bitrate| bitrate > 0);
    Ok(candidate)
}

fn downloadable_id(track: &Value) -> Option<u64> {
    (track["sharing"] == "public"
        && track["downloadable"] == true
        && track["has_downloads_left"] == true)
        .then(|| track["id"].as_u64())
        .flatten()
        .filter(|&id| id > 0)
}

fn original_cdn(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    (url.scheme() == "https"
        && url.host_str().is_some_and(|host| {
            host.ends_with(".sndcdn.com") || host.ends_with(".soundcloud.cloud")
        })
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn header_format(headers: &HeaderMap) -> Option<String> {
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .unwrap_or_default();
    let known = match content_type {
        "audio/mpeg" => Some("mp3"),
        "audio/flac" | "audio/x-flac" => Some("flac"),
        "audio/opus" => Some("opus"),
        "audio/wav" | "audio/x-wav" | "audio/wave" => Some("wav"),
        "audio/aiff" | "audio/x-aiff" => Some("aiff"),
        "audio/mp4" | "audio/x-m4a" => Some("m4a"),
        "audio/ogg" | "application/ogg" => Some("ogg"),
        "audio/aac" | "audio/aacp" => Some("aac"),
        _ => None,
    };
    known.map(str::to_owned).or_else(|| {
        let name = headers
            .get(CONTENT_DISPOSITION)?
            .to_str()
            .ok()?
            .split(';')
            .find_map(|part| part.trim().strip_prefix("filename="))?;
        format_hint(name.trim_matches('"').rsplit_once('.')?.1)
    })
}

#[cfg(all(test, windows))]
mod live;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requires_public_original_permission_and_available_downloads() {
        let mut track = json!({"id": 17, "sharing": "public", "downloadable": true, "has_downloads_left": true});
        assert_eq!(downloadable_id(&track), Some(17));
        track["has_downloads_left"] = Value::Null;
        assert_eq!(downloadable_id(&track), None);
        track["has_downloads_left"] = true.into();
        track["downloadable"] = false.into();
        assert_eq!(downloadable_id(&track), None);
        track["downloadable"] = true.into();
        track["sharing"] = "private".into();
        assert_eq!(downloadable_id(&track), None);
    }

    #[test]
    fn restricts_signed_original_to_soundcloud_cdn_and_sanitizes_format() {
        assert!(original_cdn("https://download-media.sndcdn.com/file?Key=1").is_some());
        assert!(original_cdn("https://download-media.soundcloud.cloud/file?Key=1").is_some());
        for url in [
            "http://download-media.sndcdn.com/file",
            "https://nothing.sndcdn.com.example.test/file",
            "https://example.test/nothing.sndcdn.com/file",
            "https://user:secret@download-media.sndcdn.com/file",
        ] {
            assert!(original_cdn(url).is_none());
        }
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, "application/octet-stream".parse().unwrap());
        headers.insert(
            CONTENT_DISPOSITION,
            "attachment; filename=\"Original.Flac\"".parse().unwrap(),
        );
        assert_eq!(header_format(&headers).as_deref(), Some("flac"));
        assert!(format_hint("../../x.exe").is_none());
    }
}
