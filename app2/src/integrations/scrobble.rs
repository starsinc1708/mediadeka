mod audioscrobbler;
mod listenbrainz;

#[cfg(test)]
pub(super) mod tests;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use ureq::{Body, http::Response};

pub(super) use audioscrobbler::{LastFmAccount, last_fm_connect};
pub(super) use listenbrainz::{ListenBrainzAccount, listenbrainz_connect};

const MIN_LENGTH: Duration = Duration::from_secs(30);
const FULL_PLAY: Duration = Duration::from_secs(240);
const RESPONSE_LIMIT: u64 = 64 * 1024;

/// One playback, identified by when listening began.
#[derive(Clone, Debug)]
pub(super) struct Play {
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub duration: Duration,
    pub at: SystemTime,
}

impl Play {
    pub(super) fn earned(&self, played: Duration) -> bool {
        self.duration >= MIN_LENGTH
            && (played >= FULL_PLAY || played.as_secs_f64() * 2. >= self.duration.as_secs_f64())
    }

    fn timestamp(&self) -> i64 {
        let seconds = self
            .at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        i64::try_from(seconds).unwrap_or(i64::MAX)
    }

    fn release(&self) -> Option<&str> {
        self.album.as_deref().filter(|album| !album.is_empty())
    }
}

fn read_json<T: DeserializeOwned>(response: &mut Response<Body>) -> Option<T> {
    let reader = crate::telemetry::api_body(
        response
            .body_mut()
            .with_config()
            .limit(RESPONSE_LIMIT)
            .reader(),
    );
    serde_json::from_reader(reader).ok()
}
