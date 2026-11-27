use std::sync::atomic::AtomicBool;

use serde_json::Value;
use url::Url;

use crate::music::ObjectKind;
use crate::providers::{
    ErrorKind, ProviderError, catalog,
    streams::{DirectStream, StreamOffer},
    transport::*,
};

use super::{API, PROVIDER, get_with, item, resolve, valid_track_id};

pub(super) fn track_snapshot(
    agent: &ureq::Agent,
    id: &str,
    token: &str,
    client_id: &str,
) -> Result<Value, ProviderError> {
    let track = if valid_track_id(id) {
        get_with(
            agent,
            &format!("/tracks/{id}?representation=full"),
            token,
            client_id,
        )?
    } else {
        let url =
            item::permalink(id, ObjectKind::Track).ok_or_else(|| catalog::invalid_id(PROVIDER))?;
        resolve(agent, &url, token, client_id)?
    };
    if track["kind"] != "track"
        || (valid_track_id(id)
            && track["id"]
                .as_u64()
                .is_none_or(|actual| actual.to_string() != id))
    {
        return Err(invalid_response(PROVIDER));
    }
    Ok(track)
}

pub(crate) fn direct_stream(
    token: &str,
    client_id: &str,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<StreamOffer, ProviderError> {
    let agent = agent();
    let track = track_snapshot(&agent, id, token, client_id)?;
    check_cancelled(cancelled)?;
    match stream_access(&track) {
        Access::Full => {}
        Access::Preview => return Ok(StreamOffer::Preview),
        Access::Unavailable => return Err(no_stream()),
    }
    let proof = track["track_authorization"]
        .as_str()
        .filter(|auth| valid_token(auth, 4096));
    let mut variants = stream_rows(&track).filter_map(variant).collect::<Vec<_>>();
    variants.sort_by_key(|(rank, _, _)| std::cmp::Reverse(*rank));
    let mut last_error = no_stream();
    let mut preview = false;
    for ((_, bitrate, _), url, codec) in variants {
        check_cancelled(cancelled)?;
        let mut request = agent
            .get(url.as_str())
            .query("client_id", client_id)
            .header("Authorization", format!("OAuth {token}"))
            .header("Accept", "application/json");
        if let Some(proof) = proof {
            request = request.query("track_authorization", proof);
        }
        match decode::<Value>(PROVIDER, request.call()) {
            Ok(data) => {
                let Some(url) = data["url"].as_str() else {
                    last_error = invalid_response(PROVIDER);
                    continue;
                };
                if let Some(url) = cdn_url(url) {
                    if url.path().contains("/preview/") {
                        preview = true;
                        continue;
                    }
                    return Ok(StreamOffer::Full(DirectStream {
                        url: url.into(),
                        codec: codec.map(str::to_owned),
                        bitrate_kbps: (bitrate != 0).then_some(u32::from(bitrate)),
                    }));
                }
                last_error = invalid_response(PROVIDER);
            }
            Err(error) => last_error = error,
        }
    }
    if preview && last_error.kind == ErrorKind::Unavailable {
        Ok(StreamOffer::Preview)
    } else {
        Err(last_error)
    }
}

pub(super) fn full_stream_allowed(track: &Value) -> bool {
    stream_access(track) == Access::Full
}

#[derive(Debug, Eq, PartialEq)]
enum Access {
    Full,
    Preview,
    Unavailable,
}

fn stream_access(track: &Value) -> Access {
    match track["policy"].as_str() {
        Some("BLOCK") => return Access::Unavailable,
        Some("SNIP") => return Access::Preview,
        _ => {}
    }
    if track["streamable"] != false && stream_rows(track).any(|row| variant(row).is_some()) {
        Access::Full
    } else if stream_rows(track).any(is_preview) {
        Access::Preview
    } else {
        Access::Unavailable
    }
}

fn stream_rows(track: &Value) -> impl Iterator<Item = &Value> {
    track["media"]["transcodings"]
        .as_array()
        .into_iter()
        .flatten()
}

type StreamRank = (bool, u16, bool);

fn is_preview(value: &Value) -> bool {
    value["snipped"] == true
        || value["preset"]
            .as_str()
            .is_some_and(|preset| preset.starts_with("abr_"))
}

fn variant(value: &Value) -> Option<(StreamRank, Url, Option<&str>)> {
    let preset = value["preset"].as_str()?;
    if is_preview(value) {
        return None;
    }
    let format = &value["format"];
    if !matches!(format["protocol"].as_str(), Some("hls" | "progressive")) {
        return None;
    }
    let mime = format["mime_type"].as_str()?;
    if !mime.starts_with("audio/") {
        return None;
    }
    let url = canonical_https(value["url"].as_str()?)?;
    if !url
        .as_str()
        .strip_prefix(API)
        .is_some_and(|path| path.starts_with("/media/"))
        || url.query().is_some()
    {
        return None;
    }
    let bitrate = preset
        .rsplit_once('_')
        .and_then(|(_, bitrate)| bitrate.strip_suffix('k'))
        .and_then(|bitrate| bitrate.parse().ok())
        .unwrap_or_default();
    let rank = (
        value["quality"] == "hq",
        bitrate,
        format["protocol"] == "progressive",
    );
    let codec = match (mime.split(';').next(), preset.split('_').next()) {
        (Some("audio/mpeg"), _) => Some("mp3"),
        (_, Some("aac")) => Some("aac"),
        (_, Some("opus")) => Some("opus"),
        _ => None,
    };
    Some((rank, url, codec))
}

fn canonical_https(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    (url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.fragment().is_none())
    .then_some(url)
}

fn cdn_url(value: &str) -> Option<Url> {
    let url = canonical_https(value)?;
    url.host_str()
        .is_some_and(|host| {
            host.ends_with(".sndcdn.com") || host == "playback.media-streaming.soundcloud.cloud"
        })
        .then_some(url)
}

fn no_stream() -> ProviderError {
    ProviderError::new(
        ErrorKind::Unavailable,
        "SoundCloud не предоставил полный аудиопоток для этого трека.",
    )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn accepts_the_aac_playback_cdn_and_ranks_full_variants() {
        let source = |preset, format, snipped| {
            json!({"url":"https://api-v2.soundcloud.com/media/soundcloud:tracks:1/example/stream/hls",
                "format":{"protocol":format,"mime_type":"audio/mp4"},"preset":preset,"quality":"sq","snipped":snipped})
        };
        let high_source = source("aac_160k", "hls", false);
        let low_source = source("aac_96k", "hls", false);
        let high = variant(&high_source).unwrap();
        let low = variant(&low_source).unwrap();
        assert!(high.0 > low.0);
        assert!(variant(&source("aac_160k", "hls", true)).is_none());
        assert!(variant(&source("aac_160k", "cbc-encrypted-hls", false)).is_none());
        assert!(cdn_url("https://playback.media-streaming.soundcloud.cloud/1/aac_160k/2/playlist.m3u8?signature=x").is_some());
        assert!(
            cdn_url("https://playback.media-streaming.soundcloud.cloud.evil.test/playlist.m3u8")
                .is_none()
        );
    }

    #[test]
    fn distinguishes_preview_from_blocked_or_missing_streams() {
        assert_eq!(stream_access(&json!({"policy": "SNIP"})), Access::Preview);
        assert_eq!(
            stream_access(&json!({"media":{"transcodings":[{"snipped": true}]}})),
            Access::Preview
        );
        assert_eq!(
            stream_access(
                &json!({"policy": "BLOCK", "media":{"transcodings":[{"snipped": true}]}})
            ),
            Access::Unavailable
        );
        assert_eq!(stream_access(&json!({})), Access::Unavailable);
    }
}
