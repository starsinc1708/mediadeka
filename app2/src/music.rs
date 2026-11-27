pub(crate) mod entity_relations;
pub(crate) mod library;
pub(crate) mod likes;
pub(crate) mod links;
pub(crate) mod lyrics;
mod matching;
mod page_cache;
pub(crate) mod recommendations;
pub(crate) mod recording;
mod reference;
pub(crate) mod search_cache;
mod service;
pub(crate) mod shared_reads;
pub(crate) mod sound;
pub(crate) mod spotify_access;
pub(crate) mod station;
#[cfg(test)]
mod tests;

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::{
    playlist::{ArtistCredit, PlaylistTrack},
    providers::HomeItem,
};

pub(crate) use page_cache::{PageReceipt, clear_catalog_cache};
pub use reference::{ObjectKind, ObjectRef};
pub(crate) use reference::{SearchInput, search_url};
pub(crate) use service::{
    PageUpdate, PublicInput, ScopedPage, is_soundcloud_set, load_page, prefetch_page,
    resolve_public_input,
};

pub(crate) fn region() -> String {
    std::env::var("MEDIADEKA_MUSIC_REGION")
        .ok()
        .map(|country| country.trim().to_ascii_uppercase())
        .filter(|country| country.len() == 2 && country.bytes().all(|c| c.is_ascii_uppercase()))
        .or_else(system_region)
        .unwrap_or_else(|| "ZZ".into())
}

#[cfg(windows)]
fn system_region() -> Option<String> {
    let mut name = [0u16; 3];
    // Windows writes at most the number of UTF-16 slots supplied, including the terminator.
    let length =
        unsafe { windows_sys::Win32::Globalization::GetUserDefaultGeoName(name.as_mut_ptr(), 3) };
    (length == 3 && name[..2].iter().all(|&unit| (65..=90).contains(&unit)))
        .then(|| String::from_utf16_lossy(&name[..2]))
}

#[cfg(not(windows))]
fn system_region() -> Option<String> {
    None
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct MusicPage {
    pub reference: ObjectRef,
    pub title: String,
    pub artists: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artist_credits: Vec<ArtistCredit>,
    pub artwork: Option<String>,
    pub url: Option<String>,
    pub album: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub album_ref: Option<ObjectRef>,
    pub year: Option<u32>,
    pub duration: Option<u32>,
    pub version: Option<String>,
    pub isrc: Option<String>,
    pub barcode: Option<String>,
    pub tracks: Vec<PlaylistTrack>,
    pub releases: Vec<HomeItem>,
    pub single: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artist_details: Option<ArtistDetails>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ArtistDetails {
    pub banner: Option<String>,
    pub monthly_listeners: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_listeners_caption: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub biography: Option<String>,
    pub related: Vec<HomeItem>,
    pub release_is_single: std::collections::BTreeMap<String, bool>,
}

impl ArtistDetails {
    const RELATED_PREVIEW_LIMIT: usize = 4;

    pub(crate) fn related_preview(
        origin: &ObjectRef,
        candidates: impl IntoIterator<Item = HomeItem>,
    ) -> Vec<HomeItem> {
        let mut seen = HashSet::new();
        candidates
            .into_iter()
            .filter(|item| {
                !item.title.trim().is_empty()
                    && ObjectRef::from_item(origin.provider, item).is_some_and(|reference| {
                        reference.kind == ObjectKind::Artist
                            && &reference != origin
                            && seen.insert(reference.id)
                    })
            })
            .take(Self::RELATED_PREVIEW_LIMIT)
            .collect()
    }
}

impl MusicPage {
    pub(crate) fn new(reference: ObjectRef, title: String) -> Self {
        Self {
            url: reference.url(),
            reference,
            title,
            artists: Vec::new(),
            artist_credits: Vec::new(),
            artwork: None,
            album: None,
            album_ref: None,
            year: None,
            duration: None,
            version: None,
            isrc: None,
            barcode: None,
            tracks: Vec::new(),
            releases: Vec::new(),
            single: false,
            artist_details: None,
        }
    }

    pub(crate) fn fill_missing_track_albums(&mut self) {
        debug_assert_eq!(self.reference.kind, ObjectKind::Album);
        for track in &mut self.tracks {
            if track.album_ref.is_none() {
                track.album_ref = Some(self.reference.clone());
                track.album = Some(self.title.clone());
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(crate) struct ConfirmedLink {
    pub platform: String,
    pub id: String,
    pub url: String,
    pub evidence: String,
    pub checked_at: u64,
}

pub(crate) fn normalized(value: &str) -> String {
    value
        .to_lowercase()
        .replace('ё', "е")
        .split(|character: char| !character.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn artist_names(names: &[String]) -> Vec<String> {
    let mut names: Vec<_> = names.iter().map(|name| normalized(name)).collect();
    names.retain(|name| !name.is_empty());
    names.sort_unstable();
    names.dedup();
    names
}

pub(crate) fn qualified_track_title(title: &str, version: Option<&str>) -> String {
    if let Some(version) = version.map(str::trim).filter(|version| !version.is_empty()) {
        let qualified = normalized(version);
        if !qualified.is_empty()
            && !normalized(title)
                .strip_suffix(&qualified)
                .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with(' '))
        {
            return format!("{title} ({version})");
        }
    }
    title.to_owned()
}

fn recording_title(page: &MusicPage) -> String {
    normalized(&qualified_track_title(&page.title, page.version.as_deref()))
}

fn isrc(value: Option<&str>) -> Option<String> {
    let value = value?.replace('-', "").to_ascii_uppercase();
    let bytes = value.as_bytes();
    (bytes.len() == 12
        && bytes[..2].iter().all(u8::is_ascii_uppercase)
        && bytes[2..5].iter().all(u8::is_ascii_alphanumeric)
        && bytes[5..].iter().all(u8::is_ascii_digit))
    .then_some(value)
}

fn barcode(value: Option<&str>) -> Option<&str> {
    let value = value?.trim();
    (matches!(value.len(), 12..=14) && value.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| value.trim_start_matches('0'))
}

fn same_track_list(wanted: &MusicPage, found: &MusicPage) -> bool {
    !wanted.tracks.is_empty()
        && wanted.tracks.len() == found.tracks.len()
        && wanted.tracks.iter().zip(&found.tracks).all(|(a, b)| {
            normalized(&a.title) == normalized(&b.title)
                && !a.artist.is_empty()
                && normalized(&a.artist) == normalized(&b.artist)
                && a.duration
                    .zip(b.duration)
                    .is_some_and(|(a, b)| a.abs_diff(b) <= 3)
        })
}

pub(crate) fn same_object(wanted: &MusicPage, found: &MusicPage) -> Option<&'static str> {
    if wanted.reference.kind != found.reference.kind
        || recording_title(wanted) != recording_title(found)
        || wanted.title.trim().is_empty()
    {
        return None;
    }
    let artists = artist_names(&wanted.artists);
    let matching_artists = !artists.is_empty() && artists == artist_names(&found.artists);
    match wanted.reference.kind {
        ObjectKind::Track if matching_artists => {
            let durations = wanted.duration.zip(found.duration);
            if durations.is_some_and(|(a, b)| a.abs_diff(b) > 3_000) {
                return None;
            }
            match isrc(wanted.isrc.as_deref()).zip(isrc(found.isrc.as_deref())) {
                Some((a, b)) => (a == b).then_some("ISRC, исполнители и версия"),
                None => durations.map(|_| "Название, исполнители, версия и длительность"),
            }
        }
        ObjectKind::Album if matching_artists => {
            let codes = barcode(wanted.barcode.as_deref()).zip(barcode(found.barcode.as_deref()));
            if codes.is_some_and(|(a, b)| a != b)
                || wanted.year.zip(found.year).is_some_and(|(a, b)| a != b)
                || (!wanted.tracks.is_empty()
                    && !found.tracks.is_empty()
                    && wanted.tracks.len() != found.tracks.len())
            {
                return None;
            }
            if same_track_list(wanted, found)
                && (codes.is_some() || wanted.year.zip(found.year).is_some())
            {
                Some(if codes.is_some() {
                    "UPC/EAN и треклист"
                } else {
                    "Издание и треклист"
                })
            } else {
                None
            }
        }
        ObjectKind::Artist => {
            let shared = wanted.releases.iter().filter(|release| {
                !release.title.is_empty()
                    && found
                        .releases
                        .iter()
                        .any(|other| normalized(&release.title) == normalized(&other.title))
            });
            let titles: std::collections::HashSet<_> =
                shared.map(|item| normalized(&item.title)).collect();
            (titles.len() >= 2).then_some("Имя и два издания в дискографии")
        }
        _ => None,
    }
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
