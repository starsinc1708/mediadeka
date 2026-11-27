use serde::Serialize;

use crate::{playlist::PlaylistTrack, providers::is_public_artwork_url};

#[cfg(windows)]
mod ipc;
#[cfg(test)]
mod tests;

const MAX_TEXT_UTF16_UNITS: usize = 128;
const MAX_IMAGE_UTF16_UNITS: usize = 300;
const LISTENING: u8 = 2;
const ANONYMOUS_DETAILS: &str = "Слушает музыку";

#[derive(Clone, PartialEq, Serialize)]
pub(super) struct Activity {
    #[serde(skip)]
    application_id: String,
    #[serde(rename = "type")]
    activity_type: u8,
    details: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    assets: Option<Assets>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timestamps: Option<Timestamps>,
}

#[derive(Clone, PartialEq, Serialize)]
struct Assets {
    large_image: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    large_text: Option<String>,
}

#[derive(Clone, PartialEq, Serialize)]
struct Timestamps {
    // Discord's native SET_ACTIVITY RPC takes Unix seconds.
    start: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    end: Option<i64>,
}

impl Activity {
    pub(super) fn track(
        application_id: &str,
        track: &PlaylistTrack,
        timing: Option<(i64, Option<i64>)>,
    ) -> Self {
        let assets = track
            .artwork
            .as_deref()
            .filter(|cover| {
                cover.encode_utf16().count() <= MAX_IMAGE_UTF16_UNITS
                    && is_public_artwork_url(cover)
            })
            .map(|cover| Assets {
                large_image: cover.to_owned(),
                large_text: track.album.as_deref().and_then(fit_text),
            });
        Self {
            application_id: application_id.to_owned(),
            activity_type: LISTENING,
            details: fit_text(&track.title).unwrap_or_else(|| ANONYMOUS_DETAILS.to_owned()),
            state: fit_text(&track.artist),
            assets,
            timestamps: timing.map(|(start, end)| Timestamps { start, end }),
        }
    }

    pub(super) fn anonymous(application_id: &str, started_at: Option<i64>) -> Self {
        Self {
            application_id: application_id.to_owned(),
            activity_type: LISTENING,
            details: ANONYMOUS_DETAILS.to_owned(),
            state: None,
            assets: None,
            timestamps: started_at.map(|start| Timestamps { start, end: None }),
        }
    }
}

/// Fits Discord's text fields without splitting UTF-16 surrogate pairs. A lone character is
/// padded with a non-breaking space, which Discord draws as empty.
fn fit_text(value: &str) -> Option<String> {
    let mut units = 0;
    let mut fitted = String::new();
    for character in value.trim().chars() {
        units += character.len_utf16();
        if units > MAX_TEXT_UTF16_UNITS {
            break;
        }
        fitted.push(character);
    }

    let mut fitted = fitted.trim_end().to_owned();
    match fitted.chars().count() {
        0 => None,
        1 => {
            fitted.push('\u{a0}');
            Some(fitted)
        }
        _ => Some(fitted),
    }
}

#[derive(Default)]
pub(super) struct Discord {
    #[cfg(windows)]
    worker: Option<tokio::sync::watch::Sender<Option<Activity>>>,
    #[cfg(all(test, windows))]
    pipe: Option<String>,
}

impl Discord {
    pub(super) fn new() -> Self {
        Self::default()
    }

    #[cfg(all(test, windows))]
    pub(super) fn for_pipe(path: String) -> Self {
        Self {
            pipe: Some(path),
            ..Self::default()
        }
    }

    pub(super) fn publish(&mut self, wanted: Option<Activity>) {
        #[cfg(windows)]
        if let Some(worker) = &self.worker {
            worker.send_if_modified(|current| {
                if *current == wanted {
                    return false;
                }
                *current = wanted;
                true
            });
        } else if wanted.is_some() {
            #[cfg(test)]
            if let Some(path) = self.pipe.as_ref() {
                self.worker = ipc::spawn_on(wanted, vec![path.clone()]);
                return;
            }
            self.worker = ipc::spawn(wanted);
        }
        #[cfg(not(windows))]
        let _ = wanted;
    }
}
