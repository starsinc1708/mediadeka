use std::sync::atomic::AtomicBool;

use ureq::http::HeaderMap;
use url::Url;

use crate::{playlist::library::connected_credentials, storage::Store};

use super::{Credentials, ProviderId, jellyfin, soundcloud, transport::check_cancelled};

pub(crate) const PROVIDERS: [ProviderId; 2] = [ProviderId::Jellyfin, ProviderId::SoundCloud];

// The signed URL and account headers stay in memory and must not be logged or persisted.
pub(crate) struct DownloadCandidate {
    pub url: Url,
    pub headers: HeaderMap,
    pub bytes: Option<u64>,
    pub sha1: Option<[u8; 20]>,
    pub format: Option<String>,
    pub bitrate_bps: Option<u32>,
    pub sample_rate_hz: Option<u32>,
}

impl DownloadCandidate {
    pub(crate) fn new(url: Url) -> Self {
        Self {
            url,
            headers: HeaderMap::new(),
            bytes: None,
            sha1: None,
            format: None,
            bitrate_bps: None,
            sample_rate_hz: None,
        }
    }
}

pub(crate) fn original(
    store: &Store,
    provider: ProviderId,
    profile: &str,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<DownloadCandidate, String> {
    check_cancelled(cancelled).map_err(|error| error.to_string())?;
    let credentials =
        connected_credentials(store, provider, profile).map_err(|error| error.to_string())?;
    match credentials {
        Credentials::Jellyfin {
            server,
            device,
            access_token,
        } => jellyfin::original(&server, &device, &access_token, profile, id, cancelled),
        Credentials::SoundCloud { token, client_id } => {
            soundcloud::original(&token, &client_id, id, cancelled)
        }
        _ => {
            return Err(format!(
                "{} не предоставляет оригинал файла.",
                provider.display_name()
            ));
        }
    }
    .map_err(|error| error.to_string())
}

pub(super) fn format_hint(value: &str) -> Option<String> {
    let format = value.trim().to_ascii_lowercase();
    matches!(
        format.as_str(),
        "aac"
            | "aif"
            | "aiff"
            | "alac"
            | "ape"
            | "caf"
            | "flac"
            | "m4a"
            | "mka"
            | "mp3"
            | "mp4"
            | "ogg"
            | "opus"
            | "tta"
            | "wav"
            | "webm"
            | "wma"
            | "wv"
    )
    .then_some(format)
}
