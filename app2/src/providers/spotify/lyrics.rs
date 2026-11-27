use std::{sync::atomic::AtomicBool, time::Duration};

use librespot_core::{SpotifyId, error::ErrorKind as SpotifyError};
use serde_json::Value;
use tokio::time::timeout;

use crate::{
    music::lyrics::{Line, Lyrics, Source, Word},
    providers::{SpotifySessionCredentials, catalog, transport::*},
};

use super::{ErrorKind, PROVIDER, ProviderError, librespot::with_session};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const RESPONSE_LIMIT: usize = 512 * 1024;

pub(in crate::providers) fn fetch(
    client_id: &str,
    saved: &SpotifySessionCredentials,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<Option<Lyrics>, ProviderError> {
    if id.len() != 22 {
        return Err(catalog::invalid_id(PROVIDER));
    }
    let id = SpotifyId::from_base62(id).map_err(|_| catalog::invalid_id(PROVIDER))?;
    with_session(client_id, saved, cancelled, async |session| {
        let response = timeout(REQUEST_TIMEOUT, session.spclient().get_lyrics(&id))
            .await
            .map_err(|_| {
                ProviderError::new(ErrorKind::Network, "Spotify не ответил на запрос текста.")
            })?;
        let body = match response {
            Ok(body) => body,
            Err(error) if error.kind == SpotifyError::NotFound => return Ok(None),
            Err(error) => {
                let kind = match error.kind {
                    SpotifyError::Unauthenticated => ErrorKind::Expired,
                    SpotifyError::PermissionDenied => ErrorKind::Rejected,
                    _ => ErrorKind::Network,
                };
                return Err(ProviderError::new(
                    kind,
                    "Spotify не предоставил текст песни.",
                ));
            }
        };
        if body.len() > RESPONSE_LIMIT {
            return Err(invalid_response(PROVIDER));
        }
        let value: Value = serde_json::from_slice(&body).map_err(|_| invalid_response(PROVIDER))?;
        parse(&value["lyrics"])
            .map(Some)
            .ok_or_else(|| invalid_response(PROVIDER))
    })
}

fn parse(sheet: &Value) -> Option<Lyrics> {
    let verses = sheet["lines"].as_array()?;
    let synced = matches!(
        sheet["syncType"].as_str(),
        Some("LINE_SYNCED" | "SYLLABLE_SYNCED")
    );
    let mut lines = verses
        .iter()
        .filter_map(|verse| line(verse, synced))
        .collect::<Vec<_>>();
    lines.sort_by_key(|line| line.time_ms);
    (!lines.is_empty()).then(|| Lyrics {
        source: Source::Spotify,
        lines,
        instrumental: false,
        attribution: field(sheet, "providerDisplayName"),
    })
}

fn line(verse: &Value, synced: bool) -> Option<Line> {
    let text = field(verse, "words").filter(|text| text != "♪")?;
    let time_ms = if synced {
        Some(millis(&verse["startTimeMs"])?)
    } else {
        None
    };
    let words = time_ms.map_or_else(Vec::new, |start| {
        syllables(&text, &verse["syllables"], start)
    });
    Some(Line {
        time_ms,
        text,
        words,
    })
}

fn syllables(line: &str, syllables: &Value, line_start: u64) -> Vec<Word> {
    let mut end_byte = 0;
    let mut previous = line_start;
    syllables
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|syllable| {
            let time_ms = millis(&syllable["startTimeMs"]).filter(|time| *time >= previous)?;
            let text = syllable["text"]
                .as_str()
                .filter(|text| !text.trim().is_empty())?;
            let start_byte = end_byte + line.get(end_byte..)?.find(text)?;
            end_byte = start_byte + text.len();
            previous = time_ms;
            Word::in_line(
                line,
                start_byte..end_byte,
                time_ms,
                millis(&syllable["endTimeMs"]).filter(|end| *end > time_ms),
            )
        })
        .collect()
}

fn millis(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parses_sorted_lyrics_with_utf16_syllable_ranges() {
        let lyrics = parse(&json!({
            "syncType": "SYLLABLE_SYNCED",
            "lines": [
                {"startTimeMs": "5000", "words": "Later"},
                {"startTimeMs": "1500", "words": "🙂 hello", "syllables": [
                    {"startTimeMs": "1500", "endTimeMs": "1900", "text": "🙂"},
                    {"startTimeMs": "1900", "endTimeMs": "2500", "text": "hello"}
                ]},
                {"startTimeMs": "0", "words": "♪"}
            ]
        }))
        .unwrap();
        assert_eq!(lyrics.lines.len(), 2);
        let first = &lyrics.lines[0];
        assert_eq!(first.time_ms, Some(1500));
        assert_eq!(
            (first.words[0].start_utf16, first.words[0].end_utf16),
            (0, 2)
        );
        assert_eq!(
            (first.words[1].start_utf16, first.words[1].end_utf16),
            (3, 8)
        );
        assert_eq!(first.words[1].end_ms, Some(2500));
    }
}
