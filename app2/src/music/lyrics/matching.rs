mod credits;

pub(crate) use credits::{ArtistCredits, displayed_parts};
use credits::{credit_parts, names};

use super::{
    Recording,
    spelling::{identity as normalized, search_spelling},
};

const DURATION_TOLERANCE_MS: u64 = 3_000;

pub(crate) fn matches_recording(
    request: &Recording,
    title: &str,
    artist: ArtistCredits<'_>,
    duration_ms: Option<u64>,
) -> bool {
    request
        .duration_ms
        .zip(duration_ms)
        .is_some_and(|(a, b)| a > 0 && b > 0 && a.abs_diff(b) <= DURATION_TOLERANCE_MS)
        && matches_identity(request, title, artist)
}

pub(crate) fn matches_identity(
    request: &Recording,
    title: &str,
    artist: ArtistCredits<'_>,
) -> bool {
    let wanted_title = search_spelling(&request.title);
    let wanted_display = search_spelling(&request.artist);
    let found_title = search_spelling(title);
    let found_display = search_spelling(artist.display);
    let wanted_title = strip_channel_title(&wanted_title, &wanted_display);
    let found_title = strip_channel_title(&found_title, &found_display);
    let (wanted_base, wanted_guests) = featured_title(wanted_title);
    let (found_base, found_guests) = featured_title(found_title);
    let wanted_base = normalized(wanted_base);
    if wanted_base.is_empty() || wanted_base != normalized(found_base) {
        return false;
    }
    let wanted_names: Vec<_> = request.artists.iter().map(String::as_str).collect();
    let known: Vec<_> = wanted_names.iter().chain(artist.names).copied().collect();
    let wanted = names(
        ArtistCredits {
            display: &wanted_display,
            names: &wanted_names,
        },
        wanted_guests,
        &known,
    );
    let found = names(
        ArtistCredits {
            display: &found_display,
            ..artist
        },
        found_guests,
        &known,
    );
    !wanted.is_empty() && wanted == found
}

pub(crate) fn search_terms(recording: &Recording) -> Option<(String, String)> {
    let title = search_spelling(&recording.title);
    let artist = search_spelling(&recording.artist);
    let base = base_title(&title, &artist);
    let lead = recording
        .artists
        .iter()
        .map(String::as_str)
        .find(|name| !name.trim().is_empty())
        .or_else(|| credit_parts(&artist).next())?;
    let lead = search_spelling(lead);
    (base != title || lead != artist).then(|| (base.to_owned(), lead))
}

pub(crate) fn credit_names(recording: &Recording) -> Vec<String> {
    let title = search_spelling(&recording.title);
    let artist = search_spelling(&recording.artist);
    let listed: Vec<_> = recording.artists.iter().map(String::as_str).collect();
    let (_, guests) = featured_title(strip_channel_title(&title, &artist));
    names(
        ArtistCredits {
            display: &artist,
            names: &listed,
        },
        guests,
        &listed,
    )
}

fn base_title<'a>(title: &'a str, artist: &str) -> &'a str {
    featured_title(strip_channel_title(title, artist)).0
}

pub(super) fn youtube_song_title<'a>(title: &'a str, artist: &str) -> Option<&'a str> {
    let (prefix, song) = split_prefixed_title(title)?;
    (prefix_matches(prefix, artist)
        && !normalized(song).is_empty()
        && strip_channel_title(title, artist) == title)
        .then_some(song)
}

fn split_prefixed_title(title: &str) -> Option<(&str, &str)> {
    title.char_indices().find_map(|(offset, separator)| {
        if !matches!(separator, '-' | '–' | '—') {
            return None;
        }
        let prefix = &title[..offset];
        let song = &title[offset + separator.len_utf8()..];
        (prefix.ends_with(char::is_whitespace) && song.starts_with(char::is_whitespace))
            .then(|| (prefix.trim(), song.trim()))
    })
}

fn prefix_matches(prefix: &str, artist: &str) -> bool {
    let prefix = normalized(prefix);
    !prefix.is_empty() && prefix == normalized(strip_producer(artist))
}

fn strip_channel_title<'a>(title: &'a str, artist: &str) -> &'a str {
    let Some((base, label)) = bracket_suffix(title) else {
        return title;
    };
    let label = normalized(label);
    let upload_label = matches!(
        label.as_str(),
        "official music video"
            | "official video"
            | "official audio"
            | "lyric video"
            | "official lyric video"
            | "official lyrics video"
            | "official visualizer"
            | "official visualiser"
            | "премьера клипа"
    ) || label
        .strip_prefix("премьера клипа ")
        .is_some_and(|year| year.len() == 4 && year.bytes().all(|byte| byte.is_ascii_digit()));
    if !upload_label {
        return title;
    }
    if let Some((channel, song)) = split_prefixed_title(base) {
        if prefix_matches(channel, artist) && !song.is_empty() {
            return song;
        }
        return title;
    }
    base
}

fn strip_producer(artist: &str) -> &str {
    bracket_suffix(artist)
        .filter(|(_, label)| {
            label
                .split_once(char::is_whitespace)
                .is_some_and(|(marker, name)| {
                    marker.trim_end_matches('.').eq_ignore_ascii_case("prod")
                        && !name.trim().is_empty()
                })
        })
        .map_or(artist, |(artist, _)| artist)
}

fn featured_title(value: &str) -> (&str, Option<&str>) {
    let value = value.trim();
    let Some((base, label)) = bracket_suffix(value) else {
        return (value, None);
    };
    feature_credit(label).map_or((value, None), |guests| (base, Some(guests)))
}

fn bracket_suffix(value: &str) -> Option<(&str, &str)> {
    let value = value.trim();
    let (base, label) = value
        .strip_suffix(')')
        .and_then(|text| text.rsplit_once(" ("))
        .or_else(|| value.strip_suffix(']')?.rsplit_once(" ["))?;
    Some((base.trim_end(), label.trim()))
}

fn feature_credit(value: &str) -> Option<&str> {
    let (marker, names) = value.trim_start().split_once(char::is_whitespace)?;
    let names = names.trim();
    (matches!(
        marker.trim_end_matches('.').to_lowercase().as_str(),
        "feat" | "ft" | "featuring" | "п.у"
    ) && !names.is_empty())
    .then_some(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_full_credits_across_metadata_fields() {
        let recording = Recording {
            title: "Kingdom Hearts Key (ft. redveil)".into(),
            artist: "JPEGMAFIA, Danny Brown".into(),
            artists: Vec::new(),
            album: None,
            duration_ms: Some(195_000),
        };
        let artists = "Danny Brown & JPEGMAFIA (Ft. redveil)";
        assert!(matches_recording(
            &recording,
            "Kingdom Hearts Key",
            ArtistCredits::displayed(artists),
            Some(196_000)
        ));
        assert!(!matches_identity(
            &recording,
            "Kingdom Hearts Key",
            ArtistCredits::displayed("JPEGMAFIA, Danny Brown")
        ));
        assert!(!matches_identity(
            &recording,
            "Kingdom Hearts Key (Live)",
            ArtistCredits::displayed(artists)
        ));
        assert!(!matches_recording(
            &recording,
            "Kingdom Hearts Key",
            ArtistCredits::displayed(artists),
            Some(199_000)
        ));
        assert_eq!(
            search_terms(&recording),
            Some(("Kingdom Hearts Key".into(), "JPEGMAFIA".into()))
        );
    }

    #[test]
    fn removes_upload_and_producer_labels_and_keeps_track_versions() {
        let recording = Recording {
            title: "Валя Вальчинский - СПГС (Премьера клипа 2025)".into(),
            artist: "Валя Вальчинский (prod. Example)".into(),
            artists: Vec::new(),
            album: None,
            duration_ms: None,
        };
        let artist = ArtistCredits::displayed("Валя Вальчинский");
        assert!(matches_identity(&recording, "СПГС", artist));
        assert!(!matches_identity(&recording, "СПГС (Live)", artist));
        assert!(!matches_identity(
            &recording,
            "Другой канал - СПГС (Official Video)",
            artist
        ));
    }

    #[test]
    fn youtube_lookup_keeps_the_whole_title_after_an_exact_artist_prefix() {
        assert_eq!(
            youtube_song_title(
                "edik_kingsta — Track - Subtitle (Live Remix)",
                "edik kingsta"
            ),
            Some("Track - Subtitle (Live Remix)")
        );
        assert_eq!(
            youtube_song_title("edik_kingsta - Track", "edik kingsta feat. Guest"),
            None
        );
        assert_eq!(youtube_song_title("Artist Cover - Song", "Artist"), None);
        assert_eq!(
            youtube_song_title("Artist - Song (Official Video)", "Artist"),
            None
        );
    }

    #[test]
    fn structured_group_is_atomic_in_both_structured_and_display_credits() {
        let recording = Recording {
            title: "ＷＡＳＳＵＰ　（ｆｅａｔ． JPEGMAFIA）".into(),
            artist: "Joey Valence & Brae, JPEGMAFIA".into(),
            artists: vec!["Joey Valence & Brae".into(), "JPEGMAFIA".into()],
            album: None,
            duration_ms: Some(180_000),
        };
        assert_eq!(
            search_terms(&recording),
            Some(("WASSUP".into(), "Joey Valence & Brae".into()))
        );
        assert!(matches_identity(
            &recording,
            "WASSUP (feat. JPEGMAFIA)",
            ArtistCredits::displayed("Joey Valence & Brae"),
        ));
        assert!(!matches_identity(
            &recording,
            "WASSUP (feat. JPEGMAFIA)",
            ArtistCredits::displayed("Joey Valence, Brae"),
        ));
        let split = ArtistCredits {
            display: &recording.artist,
            names: &["Joey Valence", "Brae", "JPEGMAFIA"],
        };
        assert!(!matches_identity(&recording, "WASSUP", split));
        assert!(!matches_identity(
            &recording,
            "WASSUP",
            ArtistCredits::displayed("Joey Valence & Brae")
        ));
        assert!(!matches_identity(
            &recording,
            "WASSUP (Live) [feat. JPEGMAFIA]",
            ArtistCredits::displayed("Joey Valence & Brae"),
        ));
    }

    #[test]
    fn conjunction_in_the_title_uses_confirmed_guests() {
        let track = Recording {
            title: "Get Lucky (feat. Pharrell Williams and Nile Rodgers)".into(),
            artist: "Daft Punk, Pharrell Williams, Nile Rodgers".into(),
            artists: ["Daft Punk", "Pharrell Williams", "Nile Rodgers"]
                .map(str::to_owned)
                .into(),
            album: None,
            duration_ms: None,
        };
        let complete = ArtistCredits {
            display: &track.artist,
            names: &["Daft Punk", "Pharrell Williams", "Nile Rodgers"],
        };
        assert!(matches_identity(&track, "Get Lucky", complete));
        let missing_guest = ArtistCredits {
            display: &track.artist,
            names: &["Daft Punk", "Pharrell Williams"],
        };
        assert!(!matches_identity(&track, "Get Lucky", missing_guest));
    }
}
