use crate::music::lyrics::{
    ArtistCredits, Recording, credit_names, displayed_parts, matches_identity, matches_recording,
    normalized_spelling as normalized, search_spelling,
};

use super::{Hit, Song};

// These English qualifiers describe editions or different recordings.
const VERSION_WORDS: &[&str] = &[
    "acoustic",
    "alternate",
    "alternative",
    "bonus",
    "clean",
    "cover",
    "cut",
    "deluxe",
    "demo",
    "edit",
    "explicit",
    "extended",
    "feat",
    "featuring",
    "instrumental",
    "interlude",
    "intro",
    "karaoke",
    "live",
    "mix",
    "mono",
    "outro",
    "part",
    "radio",
    "remaster",
    "remastered",
    "remix",
    "reprise",
    "recording",
    "rework",
    "romanized",
    "session",
    "slowed",
    "sped",
    "spedup",
    "speed",
    "stereo",
    "studio",
    "take",
    "translation",
    "unplugged",
    "version",
    "video",
    "vip",
    "volume",
];

impl Song {
    pub(super) fn matches(&self, track: &Recording) -> bool {
        let duration = self.duration_ms.or_else(|| {
            self.duration
                .filter(|seconds| seconds.is_finite() && *seconds > 0.)
                .map(|seconds| (seconds * 1_000.).round() as u64)
        });
        let wanted = credit_names(track);
        let known: Vec<_> = track.artists.iter().map(String::as_str).collect();
        let display = self
            .artist_names
            .as_deref()
            .or_else(|| {
                self.primary_artist
                    .as_ref()
                    .map(|person| person.name.as_str())
            })
            .unwrap_or_default();
        let display = search_spelling(display);
        let names = self.artists(&display, &wanted, &known);
        let artist = ArtistCredits {
            display: &display,
            names: &names,
        };
        let title = search_spelling(&self.title);
        let matches = |title| {
            if duration.is_some() {
                matches_recording(track, title, artist, duration)
            } else {
                matches_identity(track, title, artist)
            }
        };
        let identity = matches(&title)
            || latin_suffix(&title)
                .is_some_and(|(base, suffix)| translation(suffix) && matches(base));
        identity && album_matches(track, self.album.as_ref().map(|album| album.name.as_str()))
    }

    fn artists<'a>(&'a self, display: &'a str, wanted: &[String], known: &[&str]) -> Vec<&'a str> {
        if !self.primary_artists.is_empty() {
            return self
                .primary_artists
                .iter()
                .chain(&self.featured_artists)
                .map(|artist| resolve_alias(&artist.name, wanted))
                .collect();
        }
        displayed_parts(display, known)
            .into_iter()
            .map(|artist| resolve_alias(artist, wanted))
            .collect()
    }
}

fn album_matches(track: &Recording, found: Option<&str>) -> bool {
    let Some((wanted, found)) = track.album.as_deref().zip(found) else {
        return true;
    };
    let wanted = normalized(wanted);
    if wanted.is_empty() || found.trim().is_empty() || wanted == normalized(found) {
        return true;
    }
    let original = latin_suffix(found)
        .filter(|(_, alias)| translation(alias))
        .map_or(found, |(title, _)| title);
    let original = original.strip_suffix(" (CD Edition)").unwrap_or(original);
    let original = normalized(original);
    // Genius groups a separately released single under the album containing it.
    let track_title = normalized(&track.title);
    let single =
        wanted == track_title || wanted.strip_suffix(" single") == Some(track_title.as_str());
    wanted == original
        || original.strip_suffix(" ep") == Some(wanted.as_str())
        || wanted.strip_suffix(" ep") == Some(original.as_str())
        || single
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SearchMatch {
    Missing,
    Unique(u64),
    Ambiguous,
}

pub(super) fn unique_hit(track: &Recording, hits: Vec<Hit>) -> SearchMatch {
    let mut matching = hits
        .into_iter()
        .filter(|hit| hit.kind == "song")
        .filter_map(|hit| serde_json::from_value::<Song>(hit.result).ok())
        .filter(|song| song.id != 0 && song.matches(track));
    let Some(first) = matching.next() else {
        return SearchMatch::Missing;
    };
    if matching.all(|song| song.id == first.id) {
        SearchMatch::Unique(first.id)
    } else {
        SearchMatch::Ambiguous
    }
}

fn latin_suffix(value: &str) -> Option<(&str, &str)> {
    let (base, suffix) = value.trim().strip_suffix(')')?.rsplit_once(" (")?;
    let suffix = suffix.trim();
    (base
        .chars()
        .any(|letter| ('\u{0400}'..='\u{04ff}').contains(&letter))
        && suffix.bytes().any(|byte| byte.is_ascii_alphabetic())
        && suffix.chars().all(|letter| {
            letter.is_ascii_alphanumeric()
                || matches!(
                    letter,
                    ' ' | '.' | ',' | '-' | '_' | '\'' | '’' | '*' | '!' | '?'
                )
        }))
    .then_some((base.trim_end(), suffix))
}

fn translation(suffix: &str) -> bool {
    let words = normalized(suffix);
    !words.is_empty()
        && suffix.bytes().all(|byte| !byte.is_ascii_digit())
        && !words
            .split_whitespace()
            .any(|word| VERSION_WORDS.contains(&word))
}

fn resolve_alias<'a>(artist: &'a str, wanted: &[String]) -> &'a str {
    let contains = |name: &str| wanted.contains(&normalized(name));
    if contains(artist) {
        return artist;
    }
    if let Some((original, alias)) = latin_suffix(artist) {
        if contains(original) {
            return original;
        }
        if contains(alias) {
            return alias;
        }
    }
    artist
        .strip_suffix(" (RUS)")
        .filter(|base| contains(base))
        .unwrap_or(artist)
}
