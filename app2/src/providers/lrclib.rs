use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, de::DeserializeOwned};
use url::Url;

use crate::music::{
    lyrics::{
        ArtistCredits, Lookup, Lyrics, LyricsVersions, MAX_TEXT_BYTES, Recording, Source, Timing,
        matches_recording, parse_lrc, response_max_age, search_spelling, search_terms,
    },
    normalized,
};

use super::transport;

mod file;

const AGENT: &str = concat!("Mediadeka/", env!("CARGO_PKG_VERSION"));

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    track_name: String,
    artist_name: String,
    album_name: Option<String>,
    duration: Option<f64>,
    #[serde(default)]
    instrumental: bool,
    lyricsfile: Option<String>,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}

impl Record {
    fn lyrics(&self, recording: &Recording) -> Option<LyricsVersions> {
        let duration = self
            .duration
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .map(|seconds| (seconds * 1000.0).round() as u64);
        if !matches_recording(
            recording,
            &self.track_name,
            ArtistCredits::displayed(&self.artist_name),
            duration,
        ) {
            return None;
        }
        let synchronized = self
            .lyricsfile
            .as_deref()
            .and_then(file::parse)
            .or_else(|| {
                self.synced_lyrics
                    .as_deref()
                    .and_then(|text| parse_lrc(Source::Lrclib, text))
            })
            .filter(Lyrics::usable);
        let plain = self
            .plain_lyrics
            .as_deref()
            .filter(|text| text.len() <= MAX_TEXT_BYTES)
            .map(|text| Lyrics::plain(Source::Lrclib, text))
            .filter(Lyrics::usable);
        let (mut preferred, plain) = match synchronized {
            Some(lyrics) => (lyrics, plain),
            None => (
                plain.unwrap_or_else(|| Lyrics::plain(Source::Lrclib, "")),
                None,
            ),
        };
        preferred.instrumental = self.instrumental;
        preferred
            .usable()
            .then(|| LyricsVersions::from_source(preferred, plain))
    }
}

pub(crate) fn fetch(
    recording: &Recording,
    youtube_title: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<Lookup, String> {
    let Some(duration_ms) = recording.duration_ms.filter(|ms| *ms != 0) else {
        return Ok(Lookup::Indeterminate);
    };
    let title = search_spelling(&recording.title);
    let artist = search_spelling(&recording.artist);
    if title.is_empty() || artist.is_empty() || cancelled.load(Ordering::Relaxed) {
        return Ok(Lookup::Indeterminate);
    }
    let agent = transport::agent();
    let signature = [
        ("track_name", title.as_str()),
        ("artist_name", artist.as_str()),
    ];
    let album = recording
        .album
        .as_deref()
        .map(str::trim)
        .filter(|album| !album.is_empty());
    let mut plain = None;
    let mut exact_error = None;
    if let Some(album) = album {
        let mut url = Url::parse("https://lrclib.net/api/get").expect("fixed LRCLIB URL");
        url.query_pairs_mut()
            .extend_pairs(signature)
            .append_pair("album_name", album)
            .append_pair("duration", &(duration_ms as f64 / 1000.0).to_string());
        match get::<Record>(&agent, &url) {
            Ok(Some((record, max_age))) => {
                if let Some(lyrics) = record.lyrics(recording)
                    && album_matches(Some(album), record.album_name.as_deref())
                {
                    if lyrics.preferred().timing() >= Timing::Lines
                        || lyrics.preferred().instrumental
                    {
                        return Ok(Lookup::Found(lyrics, max_age));
                    }
                    plain = Some((lyrics, max_age));
                }
            }
            Err(error) => exact_error = Some(error),
            Ok(None) => {}
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        return Ok(Lookup::Indeterminate);
    }
    let results = search(&agent, &title, &artist).and_then(|(mut rows, age)| {
        let initial = choose(recording, &rows);
        let use_short_title = match initial {
            Match::Found(lyrics) => return Ok((Match::Found(lyrics), age)),
            Match::Missing => youtube_title,
            Match::Ambiguous => return Ok((Match::Ambiguous, age)),
        };
        if cancelled.load(Ordering::Relaxed) {
            return Ok((Match::Ambiguous, None));
        }
        let alternate = use_short_title.map(|title| Recording {
            title: title.into(),
            ..recording.clone()
        });
        let lookup = alternate.as_ref().unwrap_or(recording);
        let terms = search_terms(lookup).or_else(|| {
            alternate
                .as_ref()
                .map(|item| (search_spelling(&item.title), search_spelling(&item.artist)))
        });
        let Some((title, artist)) = terms else {
            return Ok((Match::Missing, age));
        };
        let (other, other_age) = search(&agent, &title, &artist)?;
        rows.extend(other);
        let age = match (age, other_age) {
            (Some(a), Some(b)) => Some(a.min(b)),
            _ => None,
        };
        Ok((
            choose_after_fallback(recording, alternate.as_ref(), &rows),
            age,
        ))
    });
    match (results, plain, exact_error) {
        (Ok((Match::Found(versions), age)), None, _) => Ok(Lookup::Found(versions, age)),
        (Ok((Match::Found(versions), age)), Some((exact, exact_age)), _)
            if versions.preferred().timing() >= Timing::Lines
                && same_text(versions.preferred(), exact.preferred()) =>
        {
            if versions.alternate_plain().is_some() {
                Ok(Lookup::Found(versions, age))
            } else {
                let versions = LyricsVersions::from_source(
                    versions.into_preferred(),
                    Some(exact.into_preferred()),
                );
                Ok(Lookup::Found(
                    versions,
                    age.zip(exact_age).map(|(a, b)| a.min(b)),
                ))
            }
        }
        (_, Some((versions, age)), _) => Ok(Lookup::Found(versions, age)),
        (Ok(_), None, Some(error)) | (Err(error), None, _) => Err(error),
        (Ok((Match::Missing, age)), None, None) => Ok(Lookup::Missing(age)),
        (Ok(_), None, None) => Ok(Lookup::Indeterminate),
    }
}

fn search(
    agent: &ureq::Agent,
    title: &str,
    artist: &str,
) -> Result<(Vec<Record>, Option<i64>), String> {
    let mut url = Url::parse("https://lrclib.net/api/search").expect("fixed LRCLIB URL");
    url.query_pairs_mut()
        .append_pair("track_name", title)
        .append_pair("artist_name", artist);
    Ok(get::<Vec<Record>>(agent, &url)?.unwrap_or_default())
}

enum Match {
    Missing,
    Found(LyricsVersions),
    Ambiguous,
}

#[cfg(test)]
impl Match {
    fn lyrics(self) -> Option<Lyrics> {
        match self {
            Self::Found(versions) => Some(versions.into_preferred()),
            Self::Missing | Self::Ambiguous => None,
        }
    }
}

fn choose_after_fallback(
    recording: &Recording,
    alternate: Option<&Recording>,
    rows: &[Record],
) -> Match {
    match (choose(recording, rows), alternate) {
        (Match::Missing, Some(alternate)) => {
            let compatible = rows.iter().filter(|row| {
                recording
                    .album
                    .as_deref()
                    .zip(row.album_name.as_deref())
                    .is_none_or(|(wanted, found)| {
                        wanted.trim().is_empty()
                            || found.trim().is_empty()
                            || album_matches(Some(wanted), Some(found))
                    })
            });
            choose(alternate, compatible)
        }
        (result, _) => result,
    }
}

fn choose<'a>(recording: &Recording, rows: impl IntoIterator<Item = &'a Record>) -> Match {
    let mut best: Option<(bool, LyricsVersions)> = None;
    let mut text_conflict = false;
    let mut timing_conflict = false;
    for (album, lyrics) in rows.into_iter().filter_map(|row| {
        let album = album_matches(recording.album.as_deref(), row.album_name.as_deref());
        row.lyrics(recording).map(|lyrics| (album, lyrics))
    }) {
        match &best {
            Some((true, _)) if !album => {}
            Some((matched, found)) if *matched == album => {
                text_conflict |= !same_text(lyrics.preferred(), found.preferred());
                match lyrics.preferred().timing().cmp(&found.preferred().timing()) {
                    std::cmp::Ordering::Greater => {
                        timing_conflict = false;
                        best = Some((album, lyrics));
                    }
                    std::cmp::Ordering::Equal => {
                        timing_conflict |= lyrics.preferred().lines != found.preferred().lines;
                        if found.alternate_plain().is_none() && lyrics.alternate_plain().is_some() {
                            best = Some((album, lyrics));
                        }
                    }
                    std::cmp::Ordering::Less => {}
                }
            }
            _ => {
                best = Some((album, lyrics));
                text_conflict = false;
                timing_conflict = false;
            }
        }
    }
    if text_conflict {
        return Match::Ambiguous;
    }
    let Some((_, mut lyrics)) = best else {
        return Match::Missing;
    };
    if timing_conflict {
        let untimed = lyrics.alternate_plain().cloned().unwrap_or_else(|| {
            // Keep the existing ambiguity fallback as the only displayable form.
            let mut preferred = lyrics.into_preferred();
            for line in &mut preferred.lines {
                line.time_ms = None;
                line.words.clear();
            }
            preferred
        });
        lyrics = LyricsVersions::from_source(untimed, None);
    }
    Match::Found(lyrics)
}

fn album_matches(wanted: Option<&str>, found: Option<&str>) -> bool {
    wanted
        .zip(found)
        .is_some_and(|(a, b)| !a.trim().is_empty() && normalized(a) == normalized(b))
}

fn same_text(a: &Lyrics, b: &Lyrics) -> bool {
    let words = |lyrics: &Lyrics| {
        normalized(
            &lyrics
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        )
    };
    a.instrumental == b.instrumental && words(a) == words(b)
}

fn get<T: DeserializeOwned>(
    agent: &ureq::Agent,
    url: &Url,
) -> Result<Option<(T, Option<i64>)>, String> {
    let mut response = agent
        .get(url.as_str())
        .header("User-Agent", AGENT)
        .call()
        .map_err(|_| "LRCLIB недоступен.".to_owned())?;
    if response.status().as_u16() == 404 {
        return Ok(None);
    }
    if !response.status().is_success() {
        return Err(format!(
            "LRCLIB вернул HTTP {}.",
            response.status().as_u16()
        ));
    }
    let max_age = response_max_age(response.headers());
    let reader = crate::telemetry::api_body(
        response
            .body_mut()
            .with_config()
            .limit((MAX_TEXT_BYTES * 4) as u64)
            .reader(),
    );
    serde_json::from_reader(std::io::BufReader::new(reader))
        .map(|payload| Some((payload, max_age)))
        .map_err(|_| "LRCLIB вернул неожиданный ответ.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(
        title: &str,
        artist: &str,
        seconds: f64,
        album: Option<&str>,
        lyrics: &str,
    ) -> Record {
        Record {
            track_name: title.into(),
            artist_name: artist.into(),
            album_name: album.map(str::to_owned),
            duration: Some(seconds),
            instrumental: false,
            lyricsfile: None,
            synced_lyrics: None,
            plain_lyrics: Some(lyrics.into()),
        }
    }

    #[test]
    fn keeps_the_supplied_plain_version_with_the_synchronized_default() {
        let recording = Recording {
            title: "Example".into(),
            artist: "Example Artist".into(),
            artists: Vec::new(),
            album: None,
            duration_ms: Some(231_000),
        };
        let mut row: Record = serde_json::from_value(serde_json::json!({
            "trackName": "Example",
            "artistName": "Example Artist",
            "duration": 231,
            "syncedLyrics": "[00:01.00]Synchronized arrangement",
            "plainLyrics": "Ordinary arrangement\nProvider's second line",
        }))
        .unwrap();
        let versions = row.lyrics(&recording).unwrap();
        assert_eq!(versions.preferred().timing(), Timing::Lines);
        let plain = versions.alternate_plain().unwrap();
        assert_eq!(plain.timing(), Timing::Plain);
        assert_eq!(plain.lines[0].text, "Ordinary arrangement");
        assert_eq!(plain.lines[1].text, "Provider's second line");

        row.plain_lyrics = None;
        assert!(row.lyrics(&recording).unwrap().alternate_plain().is_none());
        row.synced_lyrics = None;
        row.plain_lyrics = Some("Only the plain form".into());
        let only = row.lyrics(&recording).unwrap();
        assert_eq!(only.preferred().timing(), Timing::Plain);
        assert!(only.alternate_plain().is_none());
    }

    #[test]
    fn youtube_fallback_requires_the_same_artist_and_every_guest() {
        let original = Recording {
            title: "BOOKER - LIDA DISS (ft. OG BUDA)".into(),
            artist: "BOOKER".into(),
            artists: Vec::new(),
            album: None,
            duration_ms: Some(204_000),
        };
        let short = Recording {
            title: "LIDA DISS (ft. OG BUDA)".into(),
            ..original.clone()
        };
        let rows = [
            candidate(
                "LIDA DISS",
                "Lida feat. OG BUDA",
                204.,
                None,
                "Other artist",
            ),
            candidate("LIDA DISS", "BOOKER", 204., None, "Missing guest"),
            candidate(
                "LIDA DISS",
                "BOOKER feat. OG BUDA",
                204.,
                None,
                "Complete credits",
            ),
        ];
        assert!(
            choose_after_fallback(&original, Some(&short), &rows[..2])
                .lyrics()
                .is_none()
        );
        let lyrics = choose_after_fallback(&original, Some(&short), &rows)
            .lyrics()
            .unwrap();
        assert_eq!(lyrics.lines[0].text, "Complete credits");
    }

    #[test]
    fn youtube_fallback_keeps_live_remix_and_cover_versions() {
        for variant in ["Live", "Remix", "Cover"] {
            let title = format!("тёмный ({variant})");
            let original = Recording {
                title: format!("тима ищет свет - {title}"),
                artist: "тима ищет свет".into(),
                artists: Vec::new(),
                album: None,
                duration_ms: Some(221_000),
            };
            let short = Recording {
                title: title.clone(),
                ..original.clone()
            };
            let rows = [
                candidate("тёмный", "тима ищет свет", 221., None, "Studio"),
                candidate(&title, "тима ищет свет", 221., None, "Requested version"),
            ];
            assert!(
                choose_after_fallback(&original, Some(&short), &rows[..1])
                    .lyrics()
                    .is_none()
            );
            assert!(
                choose_after_fallback(&original, Some(&short), &rows)
                    .lyrics()
                    .is_some()
            );
            let studio = Recording {
                title: "тёмный".into(),
                ..short
            };
            assert!(
                choose_after_fallback(&original, Some(&studio), &rows[1..])
                    .lyrics()
                    .is_none()
            );
        }
    }

    #[test]
    fn youtube_fallback_rejects_another_duration_or_release() {
        let original = Recording {
            title: "источник — NONONO".into(),
            artist: "источник".into(),
            artists: Vec::new(),
            album: Some("Requested release".into()),
            duration_ms: Some(172_000),
        };
        let short = Recording {
            title: "NONONO".into(),
            ..original.clone()
        };
        let rows = [
            candidate(
                "NONONO",
                "источник",
                258.,
                Some("Requested release"),
                "Long version",
            ),
            candidate(
                "NONONO",
                "источник",
                172.,
                Some("Another release"),
                "Other release",
            ),
            candidate(
                "NONONO",
                "источник",
                175.,
                Some("Requested release"),
                "Verified",
            ),
        ];
        assert!(
            choose_after_fallback(&original, Some(&short), &rows[..2])
                .lyrics()
                .is_none()
        );
        let lyrics = choose_after_fallback(&original, Some(&short), &rows)
            .lyrics()
            .unwrap();
        assert_eq!(lyrics.lines[0].text, "Verified");
    }

    #[test]
    fn youtube_fallback_does_not_replace_a_literal_match_or_hide_its_conflict() {
        let original = Recording {
            title: "Artist - Track".into(),
            artist: "Artist".into(),
            artists: Vec::new(),
            album: None,
            duration_ms: Some(172_000),
        };
        let short = Recording {
            title: "Track".into(),
            ..original.clone()
        };
        let rows = [
            candidate("Track", "Artist", 172., None, "Short title"),
            candidate("Artist - Track", "Artist", 172., None, "Exact title"),
            candidate("Artist - Track", "Artist", 172., None, "Conflict"),
        ];
        let lyrics = choose_after_fallback(&original, Some(&short), &rows[..2])
            .lyrics()
            .unwrap();
        assert_eq!(lyrics.lines[0].text, "Exact title");
        assert!(
            choose_after_fallback(&original, Some(&short), &rows)
                .lyrics()
                .is_none()
        );
    }

    #[test]
    fn rejects_a_different_version_and_ambiguous_lyrics() {
        let recording = Recording {
            title: "Example (Live)".into(),
            artist: "Example Artist".into(),
            artists: Vec::new(),
            album: Some("Concert".into()),
            duration_ms: Some(231_000),
        };
        let rows: Vec<Record> = serde_json::from_value(serde_json::json!([
            {"trackName":"Example","artistName":"Example Artist","duration":231,"plainLyrics":"Studio"},
            {"trackName":"Example (Live)","artistName":"Example Artist","duration":231,"plainLyrics":"First"},
            {"trackName":"Example (Live)","artistName":"Example Artist","duration":231,"plainLyrics":"Second"},
            {"trackName":"Example (Live)","artistName":"Example Artist","duration":260,"plainLyrics":"Long"}
        ])).unwrap();
        assert!(matches!(choose(&recording, &rows), Match::Ambiguous));
        assert_eq!(
            choose(&recording, &rows[..2]).lyrics().unwrap().lines[0].text,
            "First"
        );
    }

    #[test]
    fn keeps_words_when_equally_precise_timestamps_conflict() {
        let recording = Recording {
            title: "Example".into(),
            artist: "Artist A, Artist B".into(),
            artists: Vec::new(),
            album: None,
            duration_ms: Some(231_000),
        };
        let rows: Vec<Record> = serde_json::from_value(serde_json::json!([
            {"trackName":"Example","artistName":"Artist B feat. Artist A","duration":231,
                "albumName":"Release One","syncedLyrics":"[00:01.00]Same words"},
            {"trackName":"Example","artistName":"Artist A & Artist B","duration":231,
                "albumName":"Release Two","syncedLyrics":"[00:02.00]Same words"}
        ]))
        .unwrap();
        let lyrics = choose(&recording, &rows).lyrics().unwrap();
        assert_eq!(lyrics.timing(), Timing::Plain);
        assert_eq!(lyrics.lines[0].text, "Same words");
        let recording = Recording {
            album: Some("Release Two".into()),
            ..recording
        };
        let lyrics = choose(&recording, &rows).lyrics().unwrap();
        assert_eq!(lyrics.timing(), Timing::Lines);
        assert_eq!(lyrics.lines[0].time_ms, Some(2_000));
    }

    #[test]
    #[ignore = "requires live LRCLIB"]
    fn live_stage6_public_lyrics() {
        let recording = Recording {
            title: "I Want to Live".into(),
            artist: "Borislav Slavov".into(),
            artists: Vec::new(),
            album: Some("Baldur's Gate 3 (Original Game Soundtrack)".into()),
            duration_ms: Some(233_000),
        };
        let Lookup::Found(versions, max_age) =
            fetch(&recording, None, &AtomicBool::new(false)).unwrap()
        else {
            panic!("expected matching timed lyrics");
        };
        let lyrics = versions.preferred();
        assert!(lyrics.lines.len() > 20);
        assert!(
            lyrics
                .lines
                .iter()
                .filter(|line| line.time_ms.is_some())
                .count()
                > 20
        );
        println!(
            "LRCLIB: {} timed rows, cache {:?} seconds",
            lyrics.lines.len(),
            max_age
        );
    }
}
