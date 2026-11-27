use std::{
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;
use ureq::Agent;
use url::Url;

use super::{
    PublicLink,
    http::{self, LookupError, Result},
    metadata::{PublicMetadata, items, number, text, track_row, valid_region, year},
    public_link,
};

const REQUEST_INTERVAL: Duration = Duration::from_millis(1_250);
const MAX_SLOT_WAIT: Duration = Duration::from_secs(3);
static NEXT_REQUEST: Mutex<Option<Instant>> = Mutex::new(None);

/// Check a Qobuz album against a curated link to a specific release and its edition metadata.
pub(super) fn release(
    agent: &Agent,
    link: &PublicLink,
    region: &str,
    cancelled: &AtomicBool,
) -> Result<PublicMetadata> {
    let url = url(
        "https://musicbrainz.org/ws/2/url",
        [
            ("resource", link.url.as_str()),
            ("inc", "release-rels"),
            ("fmt", "json"),
        ],
    )?;
    let address = request(agent, &url, cancelled)?;
    if !address["resource"]
        .as_str()
        .and_then(public_link)
        .is_some_and(|address| link.same_address(&address))
    {
        return Err(LookupError::Missing);
    }
    let mut releases = items(&address["relations"]).filter_map(|relation| {
        relation_active(relation)
            .then(|| relation["release"]["id"].as_str())
            .flatten()
            .filter(|id| valid_uuid(id))
    });
    let id = releases.next().ok_or(LookupError::Missing)?;
    if releases.any(|other| other != id) {
        return Err(LookupError::Missing);
    }
    let query = Url::parse_with_params(
        &format!("https://musicbrainz.org/ws/2/release/{id}"),
        [
            ("inc", "recordings+artist-credits+url-rels"),
            ("fmt", "json"),
        ],
    )
    .map_err(|_| LookupError::Unsupported)?;
    let release = request(agent, &query, cancelled)?;
    if release["id"] != id {
        return Err(LookupError::Missing);
    }
    edition(&release, link, region).ok_or(LookupError::Missing)
}

fn relation_active(relation: &Value) -> bool {
    matches!(
        relation["type"].as_str(),
        Some("streaming" | "purchase for download")
    ) && relation["ended"] == false
}

fn edition(release: &Value, link: &PublicLink, region: &str) -> Option<PublicMetadata> {
    if valid_region(region)
        && let Some(country) = release["country"].as_str()
        && country != "XW"
        && !country.eq_ignore_ascii_case(region)
    {
        return None;
    }
    if !items(&release["relations"])
        .filter(|row| relation_active(row))
        .any(|relation| {
            relation["url"]["resource"]
                .as_str()
                .and_then(public_link)
                .is_some_and(|address| link.same_address(&address))
        })
    {
        return None;
    }
    let barcode = text(&release["barcode"])?;
    if !valid_barcode(&barcode) {
        return None;
    }
    let mut media: Vec<_> = items(&release["media"]).collect();
    media.sort_by_key(|medium| number(&medium["position"]));
    let mut tracks = Vec::new();
    for medium in media {
        let mut disc: Vec<_> = items(&medium["tracks"]).collect();
        if number(&medium["track-count"])? as usize != disc.len() {
            return None;
        }
        disc.sort_by_key(|track| number(&track["position"]));
        for row in disc {
            let duration = number(&row["length"]).map(|ms| ms / 1_000);
            let artists = credits(&row["artist-credit"]);
            tracks.push(track_row(
                text(&row["id"])?,
                text(&row["title"])?,
                artists.join(", "),
                artists,
                duration,
            ));
        }
    }
    if tracks.is_empty() {
        return None;
    }
    Some(PublicMetadata {
        title: text(&release["title"])?,
        artists: credits(&release["artist-credit"]),
        year: year(&release["date"]),
        barcode: Some(barcode),
        tracks,
        ..PublicMetadata::default()
    })
}

fn credits(value: &Value) -> Vec<String> {
    items(value).filter_map(|row| text(&row["name"])).collect()
}

fn valid_uuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(index, digit)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                digit == b'-'
            } else {
                digit.is_ascii_hexdigit()
            }
        })
}

fn valid_barcode(value: &str) -> bool {
    matches!(value.len(), 12..=14) && value.bytes().all(|digit| digit.is_ascii_digit())
}

fn url<const N: usize>(base: &str, parameters: [(&str, &str); N]) -> Result<Url> {
    Url::parse_with_params(base, parameters).map_err(|_| LookupError::Unsupported)
}

fn request(agent: &Agent, url: &Url, cancelled: &AtomicBool) -> Result<Value> {
    let deadline = Instant::now() + MAX_SLOT_WAIT;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(LookupError::Cancelled);
        }
        let now = Instant::now();
        let delay = {
            let mut next = NEXT_REQUEST
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            match next.and_then(|next| next.checked_duration_since(now)) {
                Some(delay) if !delay.is_zero() => delay,
                _ => {
                    *next = Some(now + REQUEST_INTERVAL);
                    break;
                }
            }
        };
        if now + delay > deadline {
            return Err(LookupError::Temporary(delay));
        }
        thread::sleep(delay.min(Duration::from_millis(100)));
    }
    let result = http::json(agent, url.as_str(), cancelled);
    if let Err(LookupError::Temporary(delay)) = result {
        let mut next = NEXT_REQUEST
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let until = Instant::now() + delay;
        *next = Some(next.map_or(until, |previous| until.max(previous)));
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn curated_release_needs_the_exact_active_link_and_complete_media() {
        let link = public_link("https://open.qobuz.com/album/0886443927087").unwrap();
        let mut release = json!({
            "title": "Album",
            "country": "US",
            "barcode": "886443927087",
            "artist-credit": [{"name": "Artist"}],
            "relations": [{"type": "streaming", "ended": false, "url": {"resource": &link.url}}],
            "media": [{"position": 1, "track-count": 1, "tracks": [
                {"id": "track-id", "title": "Song", "position": 1, "length": 170000,
                    "artist-credit": [{"name": "Artist"}]}
            ]}]
        });
        assert_eq!(
            edition(&release, &link, "US").unwrap().tracks[0].duration,
            Some(170)
        );
        assert!(edition(&release, &link, "DE").is_none());
        release["relations"][0]["ended"] = json!(true);
        assert!(edition(&release, &link, "US").is_none());
    }
}
