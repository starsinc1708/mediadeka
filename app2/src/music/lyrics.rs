pub(crate) mod genius;
mod http_cache;
mod local;
mod lrc;
mod matching;
mod request;
mod spelling;
mod ttml;

pub(crate) use http_cache::{MAX_AGE as MAX_CACHE_AGE, response_max_age};
pub(crate) use lrc::parse_lrc;
pub(crate) use matching::{
    ArtistCredits, credit_names, displayed_parts, matches_identity, matches_recording, search_terms,
};
#[cfg(feature = "capture")]
pub(crate) use request::seed_lrclib_versions_for_capture;
pub(crate) use request::{load, prefetch, reload};
pub(crate) use spelling::{identity as normalized_spelling, search_spelling};

use serde::{Deserialize, Serialize};

use crate::{playback::QueueEntry, providers::ProviderId};

pub(crate) const MAX_TEXT_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_LINE_BYTES: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Source {
    Local,
    YandexMusic,
    Lrclib,
    Spotify,
    YouTubeMusic,
    Genius,
}

impl Source {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Local => "Локальный файл",
            Self::YandexMusic => ProviderId::YandexMusic.display_name(),
            Self::Lrclib => "LRCLIB",
            Self::Spotify => ProviderId::Spotify.display_name(),
            Self::YouTubeMusic => ProviderId::YouTubeMusic.display_name(),
            Self::Genius => "Genius",
        }
    }

    const fn provider(self) -> Option<ProviderId> {
        match self {
            Self::YandexMusic => Some(ProviderId::YandexMusic),
            Self::Spotify => Some(ProviderId::Spotify),
            Self::YouTubeMusic => Some(ProviderId::YouTubeMusic),
            Self::Local | Self::Lrclib | Self::Genius => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Lyrics {
    pub source: Source,
    pub lines: Vec<Line>,
    pub instrumental: bool,
    pub attribution: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct LyricsVersions {
    // The original fields stay at the top level for existing snapshots and readers.
    #[serde(flatten)]
    preferred: Lyrics,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    alternate_plain: Option<Lyrics>,
    #[serde(default)]
    forms_checked: bool,
}

impl From<Lyrics> for LyricsVersions {
    fn from(preferred: Lyrics) -> Self {
        Self {
            preferred,
            alternate_plain: None,
            forms_checked: false,
        }
    }
}

impl LyricsVersions {
    pub(crate) fn from_source(preferred: Lyrics, plain: Option<Lyrics>) -> Self {
        let alternate_plain = plain.filter(|plain| {
            !preferred.instrumental
                && preferred.timing() >= Timing::Lines
                && plain.source == preferred.source
                && plain.timing() == Timing::Plain
                && plain.usable()
        });
        Self {
            preferred,
            alternate_plain,
            forms_checked: true,
        }
    }

    pub(crate) fn preferred(&self) -> &Lyrics {
        &self.preferred
    }

    pub(crate) fn alternate_plain(&self) -> Option<&Lyrics> {
        self.alternate_plain.as_ref()
    }

    pub(crate) fn into_preferred(self) -> Lyrics {
        self.preferred
    }

    fn needs_format_revalidation(&self) -> bool {
        matches!(self.preferred.source, Source::Lrclib | Source::YandexMusic) && !self.forms_checked
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Line {
    pub time_ms: Option<u64>,
    pub text: String,
    pub words: Vec<Word>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct Word {
    pub time_ms: u64,
    pub end_ms: Option<u64>,
    pub start_utf16: usize,
    pub end_utf16: usize,
}

impl Word {
    pub(crate) fn in_line(
        text: &str,
        bytes: std::ops::Range<usize>,
        time_ms: u64,
        end_ms: Option<u64>,
    ) -> Option<Self> {
        let fragment = text.get(bytes.clone())?;
        let word = fragment.trim();
        if word.is_empty() || end_ms.is_some_and(|end| end <= time_ms) {
            return None;
        }
        let start_utf16 = text.get(..bytes.start)?.encode_utf16().count()
            + fragment[..fragment.len() - fragment.trim_start().len()]
                .encode_utf16()
                .count();
        Some(Self {
            time_ms,
            end_ms,
            start_utf16,
            end_utf16: start_utf16 + word.encode_utf16().count(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum Timing {
    Instrumental,
    Plain,
    Lines,
    Words,
}

pub(crate) enum Lookup {
    Found(LyricsVersions, Option<i64>),
    Missing(Option<i64>),
    Indeterminate,
}

impl Lyrics {
    pub(crate) fn plain(source: Source, text: &str) -> Self {
        let lines = text
            .lines()
            .map(str::trim)
            .map(|text| Line {
                time_ms: None,
                text: text.to_owned(),
                words: Vec::new(),
            })
            .collect();
        Self {
            source,
            lines,
            instrumental: false,
            attribution: None,
        }
    }

    pub(crate) fn active_line(&self, position_ms: u64) -> Option<usize> {
        self.lines
            .partition_point(|line| line.time_ms.is_some_and(|time| time <= position_ms))
            .checked_sub(1)
    }

    pub(crate) fn usable(&self) -> bool {
        self.instrumental || self.lines.iter().any(|line| !line.text.trim().is_empty())
    }

    pub(crate) fn timing(&self) -> Timing {
        if self.lines.iter().any(|line| !line.words.is_empty()) {
            Timing::Words
        } else if self.lines.iter().any(|line| line.time_ms.is_some()) {
            Timing::Lines
        } else if self.lines.is_empty() && self.instrumental {
            Timing::Instrumental
        } else {
            Timing::Plain
        }
    }

    pub(crate) fn prepare(&mut self) {
        if self.timing() >= Timing::Lines {
            self.lines
                .sort_by_key(|line| (line.time_ms.is_none(), line.time_ms));
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct Recording {
    pub title: String,
    pub artist: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<u64>,
}

impl Recording {
    pub(crate) fn from_entry(entry: &QueueEntry) -> Self {
        Self::from_track(&entry.track)
    }

    pub(crate) fn from_track(track: &crate::playlist::PlaylistTrack) -> Self {
        Self {
            title: track.title.clone(),
            artist: track.artist.clone(),
            artists: track.artists.clone(),
            album: track.album.clone(),
            duration_ms: track.duration.map(|seconds| u64::from(seconds) * 1000),
        }
    }
}
