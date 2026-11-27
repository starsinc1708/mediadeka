use std::sync::atomic::AtomicBool;

use serde_json::Value;
use ureq::http::{HeaderValue, header::AUTHORIZATION};

use crate::providers::{
    ProviderError,
    downloads::{DownloadCandidate, format_hint},
    transport::*,
};

use super::{PROVIDER, at, auth, catalog::same_item_id, profile_user, server, valid_item_id};

pub(in crate::providers) fn original(
    address: &str,
    device: &str,
    token: &str,
    profile: &str,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<DownloadCandidate, ProviderError> {
    check_cancelled(cancelled)?;
    if !valid_item_id(id) {
        return Err(crate::providers::catalog::invalid_id(PROVIDER));
    }
    let server = server(address)?;
    let user = profile_user(&server, profile)?;
    let header = auth(device, Some(token))?;
    let mut header = HeaderValue::from_str(&header).map_err(|_| invalid_response(PROVIDER))?;
    header.set_sensitive(true);
    let row: Value = decode(
        PROVIDER,
        agent()
            .get(at(&server, &format!("Items/{id}")).as_str())
            .header(AUTHORIZATION, &header)
            .query("UserId", user)
            .query("Fields", "MediaSources,MediaStreams")
            .call(),
    )?;
    check_cancelled(cancelled)?;
    if row["Type"] != "Audio"
        || !row["Id"]
            .as_str()
            .is_some_and(|actual| same_item_id(actual, id))
    {
        return Err(invalid_response(PROVIDER));
    }
    let mut candidate = DownloadCandidate::new(at(&server, &format!("Items/{id}/Download")));
    candidate.headers.insert(AUTHORIZATION, header);

    // The download endpoint serves this item's file. An alternate media version may
    // have a different size or format and cannot describe the requested file.
    let media = row["MediaSources"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|media| {
            media["Id"]
                .as_str()
                .is_some_and(|actual| same_item_id(actual, id))
        });
    if let Some(media) = media {
        candidate.bytes = media["Size"].as_u64().filter(|&size| size > 0);
        candidate.format = media["Container"]
            .as_str()
            .and_then(|containers| containers.split(',').find_map(format_hint));
    }
    let audio = media
        .and_then(|media| audio_stream(&media["MediaStreams"]))
        .or_else(|| audio_stream(&row["MediaStreams"]));
    if let Some(audio) = audio {
        candidate.bitrate_bps = positive_u32(&audio["BitRate"]);
        candidate.sample_rate_hz = positive_u32(&audio["SampleRate"]);
        if candidate.format.is_none() {
            candidate.format = audio["Codec"].as_str().and_then(format_hint);
        }
    }
    Ok(candidate)
}

fn audio_stream(streams: &Value) -> Option<&Value> {
    streams
        .as_array()?
        .iter()
        .find(|stream| stream["Type"] == "Audio")
}

fn positive_u32(value: &Value) -> Option<u32> {
    u32::try_from(value.as_u64()?)
        .ok()
        .filter(|&number| number > 0)
}

#[cfg(test)]
mod tests;
