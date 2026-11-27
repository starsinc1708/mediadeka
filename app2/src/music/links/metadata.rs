use std::sync::atomic::AtomicBool;

use serde_json::Value;
use ureq::Agent;
use url::Url;

use crate::{
    music::{
        MusicPage,
        ObjectKind::{self, Album, Artist, Collection, Playlist, Station, Track},
        ObjectRef,
    },
    playlist::PlaylistTrack,
    providers::HomeItem,
};

use super::{
    PublicLink,
    http::{self, LookupError, Result},
    public_link,
};

#[derive(Clone, Default)]
pub(crate) struct PublicMetadata {
    pub(crate) title: String,
    pub(crate) artists: Vec<String>,
    pub(crate) album: Option<String>,
    pub(crate) year: Option<u32>,
    pub(crate) duration: Option<u32>,
    pub(crate) isrc: Option<String>,
    pub(crate) barcode: Option<String>,
    pub(crate) tracks: Vec<PlaylistTrack>,
    pub(crate) version: Option<String>,
    pub(crate) artwork: Option<String>,
    pub(crate) releases: Vec<HomeItem>,
    pub(crate) single: bool,
}

impl PublicMetadata {
    pub(crate) fn into_page(self, reference: ObjectRef) -> MusicPage {
        let mut page = MusicPage::new(reference, self.title);
        page.artists = self.artists;
        page.album = self.album;
        page.year = self.year;
        page.duration = self.duration;
        page.isrc = self.isrc;
        page.barcode = self.barcode;
        page.tracks = self.tracks;
        page.version = self.version;
        page.artwork = self.artwork;
        page.releases = self.releases;
        page.single = self.single;
        page
    }
}

pub(crate) fn public_metadata(
    link: &PublicLink,
    region: &str,
    cancelled: &AtomicBool,
) -> Option<PublicMetadata> {
    let checked = in_region(link.clone(), region);
    read(&http::client(), &checked, region, cancelled).ok()
}

pub(super) fn covers(platform: &str, kind: ObjectKind) -> bool {
    matches!(
        (platform, kind),
        ("apple-music" | "deezer", Track | Album | Artist) | ("tidal", Track) | ("qobuz", Album)
    )
}

pub(super) fn evidence_source(platform: &str) -> &str {
    match platform {
        "apple-music" => "Apple Music",
        "deezer" => "Deezer",
        "tidal" => "TIDAL",
        "qobuz" => "MusicBrainz",
        _ => platform,
    }
}

pub(super) fn in_region(mut link: PublicLink, region: &str) -> PublicLink {
    if link.platform == "apple-music"
        && valid_region(region)
        && let Ok(mut url) = Url::parse(&link.url)
        && let Some((_, path)) = url.path().trim_start_matches('/').split_once('/')
    {
        url.set_path(&format!("/{}/{path}", region.to_ascii_lowercase()));
        link.url = url.into();
    }
    link
}

pub(super) fn read(
    agent: &Agent,
    link: &PublicLink,
    region: &str,
    cancelled: &AtomicBool,
) -> Result<PublicMetadata> {
    match (link.platform.as_str(), link.kind) {
        ("deezer", Track | Album | Artist) => deezer(agent, link, region, cancelled),
        ("apple-music", Album | Artist) => apple_collection(agent, link, cancelled),
        ("qobuz", Album) => super::musicbrainz::release(agent, link, region, cancelled),
        ("apple-music", Track) | ("tidal", Track | Album | Artist) => {
            let body = http::html(agent, link, cancelled)?;
            structured_music(&body, link).ok_or(LookupError::Missing)
        }
        _ => Err(LookupError::Unsupported),
    }
}

pub(super) fn valid_region(region: &str) -> bool {
    region != "ZZ" && region.len() == 2 && region.bytes().all(|byte| byte.is_ascii_alphabetic())
}

fn deezer(
    agent: &Agent,
    link: &PublicLink,
    region: &str,
    cancelled: &AtomicBool,
) -> Result<PublicMetadata> {
    let path = match link.kind {
        Track => "track",
        Album => "album",
        Artist => "artist",
        Playlist | Collection | Station => return Err(LookupError::Unsupported),
    };
    let url = format!("https://api.deezer.com/{path}/{}", link.id);
    let response = http::json(agent, &url, cancelled)?;
    if number_string(&response["id"]).as_ref() != Some(&link.id)
        || response["readable"].as_bool() == Some(false)
        || (valid_region(region)
            && response["available_countries"]
                .as_array()
                .is_some_and(|countries| {
                    !countries.iter().any(|value| {
                        value
                            .as_str()
                            .is_some_and(|value| value.eq_ignore_ascii_case(region))
                    })
                }))
    {
        return Err(LookupError::Missing);
    }
    let mut result = deezer_metadata(&response, link.kind).ok_or(LookupError::Missing)?;
    if link.kind == Artist {
        let albums = http::json(agent, &format!("{url}/albums?limit=20"), cancelled)?;
        result.releases = items(&albums["data"])
            .filter_map(|row| {
                let id = number_string(&row["id"])?;
                let url = format!("https://www.deezer.com/album/{id}");
                Some(release(
                    "deezer",
                    id,
                    url,
                    text(&row["title"])?,
                    text(&row["cover_medium"]),
                ))
            })
            .collect();
    }
    Ok(result)
}

fn deezer_metadata(response: &Value, kind: ObjectKind) -> Option<PublicMetadata> {
    let title = text(&response[if kind == Artist { "name" } else { "title" }])?;
    let mut artists = names(&response["contributors"]);
    if artists.is_empty()
        && let Some(artist) = text(&response["artist"]["name"])
    {
        artists.push(artist);
    }
    let rows: Vec<_> = items(&response["tracks"]["data"])
        .filter_map(|row| {
            let title = text(&row["title"])?;
            let id = number_string(&row["id"])?;
            let artist = text(&row["artist"]["name"])?;
            let duration = number(&row["duration"]);
            Some(track_row(
                id,
                title,
                artist,
                names(&row["contributors"]),
                duration,
            ))
        })
        .collect();
    let tracks = if number(&response["nb_tracks"]).is_some_and(|count| count as usize == rows.len())
    {
        rows
    } else {
        Vec::new()
    };
    Some(PublicMetadata {
        title,
        artists,
        album: text(&response["album"]["title"]),
        year: year(&response["release_date"]),
        duration: number(&response["duration"]).and_then(|value| value.checked_mul(1_000)),
        isrc: text(&response["isrc"]),
        barcode: text(&response["upc"]),
        tracks,
        version: text(&response["title_version"])
            .map(|value| value.trim_matches(['(', ')']).to_owned()),
        artwork: ["cover_big", "picture_big"]
            .into_iter()
            .find_map(|field| text(&response[field]))
            .or_else(|| text(&response["album"]["cover_big"])),
        single: response["record_type"] == "single",
        ..PublicMetadata::default()
    })
}

fn apple_collection(
    agent: &Agent,
    link: &PublicLink,
    cancelled: &AtomicBool,
) -> Result<PublicMetadata> {
    let url = Url::parse(&link.url).map_err(|_| LookupError::Unsupported)?;
    let region = url
        .path_segments()
        .and_then(|mut path| path.next())
        .ok_or(LookupError::Unsupported)?;
    let entity = if link.kind == Album { "song" } else { "album" };
    let lookup = Url::parse_with_params(
        "https://itunes.apple.com/lookup",
        [
            ("id", link.id.as_str()),
            ("country", region),
            ("entity", entity),
            ("limit", "200"),
        ],
    )
    .map_err(|_| LookupError::Unsupported)?;
    let data = http::json(agent, lookup.as_str(), cancelled)?;
    apple_result(&data, link, region).ok_or(LookupError::Missing)
}

fn apple_result(response: &Value, link: &PublicLink, region: &str) -> Option<PublicMetadata> {
    let results = &response["results"];
    if link.kind == Artist {
        let artist = items(results).find(|row| {
            row["wrapperType"] == "artist"
                && number_string(&row["artistId"]).as_ref() == Some(&link.id)
        })?;
        let releases = items(results)
            .filter(|row| row["wrapperType"] == "collection" && row["collectionType"] == "Album")
            .filter_map(|row| {
                let id = number_string(&row["collectionId"])?;
                let url = format!("https://music.apple.com/{region}/album/{id}");
                Some(release(
                    "apple-music",
                    id,
                    url,
                    text(&row["collectionName"])?,
                    text(&row["artworkUrl100"]),
                ))
            })
            .collect();
        return Some(PublicMetadata {
            title: text(&artist["artistName"])?,
            releases,
            ..PublicMetadata::default()
        });
    }
    let album = items(results).find(|row| {
        row["wrapperType"] == "collection"
            && row["collectionType"] == "Album"
            && number_string(&row["collectionId"]).as_ref() == Some(&link.id)
    })?;
    let title = text(&album["collectionName"])?;
    let mut tracks: Vec<_> = items(results)
        .filter(|row| {
            row["kind"] == "song" && number_string(&row["collectionId"]).as_ref() == Some(&link.id)
        })
        .filter_map(|row| {
            let order = (number(&row["discNumber"])?, number(&row["trackNumber"])?);
            let track = track_row(
                number_string(&row["trackId"])?,
                text(&row["trackName"])?,
                text(&row["artistName"])?,
                Vec::new(),
                number(&row["trackTimeMillis"]).map(|value| value / 1_000),
            );
            Some((order, track))
        })
        .collect();
    tracks.sort_by_key(|(order, _)| *order);
    let tracks = if number(&album["trackCount"]).is_some_and(|count| count as usize == tracks.len())
    {
        tracks.into_iter().map(|(_, track)| track).collect()
    } else {
        Vec::new()
    };
    Some(PublicMetadata {
        single: title.ends_with(" - Single"),
        title,
        artists: text(&album["artistName"]).into_iter().collect(),
        year: year(&album["releaseDate"]),
        barcode: text(&album["upc"]),
        artwork: text(&album["artworkUrl100"]),
        tracks,
        ..PublicMetadata::default()
    })
}

fn structured_music(html: &str, link: &PublicLink) -> Option<PublicMetadata> {
    html.split("<script")
        .skip(1)
        .filter_map(|tag| {
            let (attributes, body) = tag.split_once('>')?;
            if !attributes.contains("type=\"application/ld+json\"")
                && !attributes.contains("type='application/ld+json'")
            {
                return None;
            }
            let json = serde_json::from_str::<Value>(body.split_once("</script>")?.0).ok()?;
            let data = if json["@type"] == "MusicComposition"
                && json["audio"]["@type"] == "MusicRecording"
            {
                &json["audio"]
            } else {
                &json
            };
            let kind = match data["@type"].as_str()? {
                "MusicRecording" => Track,
                "MusicAlbum" => Album,
                "MusicGroup" | "Person" => Artist,
                _ => return None,
            };
            let actual = public_link(data["url"].as_str()?)?;
            if kind != link.kind || !link.same_address(&actual) {
                return None;
            }
            Some(PublicMetadata {
                title: text(&data["name"])?,
                artists: names(&data["byArtist"]),
                album: text(&data["inAlbum"]["name"]),
                year: year(&data["datePublished"]),
                duration: data["duration"].as_str().and_then(iso_duration),
                isrc: text(&data["isrcCode"]),
                barcode: text(&data["gtin13"]).or_else(|| text(&data["gtin12"])),
                artwork: text(&data["image"]),
                ..PublicMetadata::default()
            })
        })
        .next()
}

fn iso_duration(value: &str) -> Option<u32> {
    let mut remainder = value.strip_prefix("PT")?;
    if remainder.is_empty() {
        return None;
    }
    let mut milliseconds = 0u32;
    for (unit, scale) in [('H', 3_600_000), ('M', 60_000), ('S', 1_000)] {
        if let Some((value, rest)) = remainder.split_once(unit) {
            let value = value.parse::<f64>().ok()?;
            if !value.is_finite() || value < 0. || value > (u32::MAX / scale) as f64 {
                return None;
            }
            milliseconds = milliseconds.checked_add((value * scale as f64) as u32)?;
            remainder = rest;
        }
    }
    remainder.is_empty().then_some(milliseconds)
}

fn names(value: &Value) -> Vec<String> {
    if let Some(values) = value.as_array() {
        return values
            .iter()
            .filter_map(|item| text(&item["name"]))
            .collect();
    }
    text(&value["name"]).into_iter().collect()
}

pub(super) fn items(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

pub(super) fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

pub(super) fn number(value: &Value) -> Option<u32> {
    u32::try_from(value.as_u64()?).ok()
}

fn number_string(value: &Value) -> Option<String> {
    value.as_u64().map(|value| value.to_string())
}

pub(super) fn year(value: &Value) -> Option<u32> {
    value.as_str()?.get(..4)?.parse().ok()
}

fn release(
    platform: &str,
    id: String,
    url: String,
    title: String,
    artwork: Option<String>,
) -> HomeItem {
    HomeItem {
        id: format!("{platform}:album:{id}"),
        title,
        subtitle: String::new(),
        url: Some(url),
        artwork,
    }
}

pub(super) fn track_row(
    id: String,
    title: String,
    artist: String,
    artists: Vec<String>,
    duration: Option<u32>,
) -> PlaylistTrack {
    PlaylistTrack {
        id,
        title,
        artist,
        artists,
        artist_credits: Vec::new(),
        album_ref: None,
        duration,
        album: None,
        artwork: None,
        added: None,
        media: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn metadata_must_identify_the_requested_recording() {
        let url = public_link("https://tidal.com/browse/track/548933173").unwrap();
        let body = r#"<script type="application/ld+json">{"@type":"MusicRecording","name":"ПЕРВЫЙ РАЗ","byArtist":[{"name":"GATASKI"}],"url":"https://tidal.com/track/548933173","duration":"PT2M56S","isrcCode":"FRX452603197"}</script>"#;
        let page = structured_music(body, &url).unwrap();
        assert_eq!(page.title, "ПЕРВЫЙ РАЗ");
        assert_eq!(page.artists, ["GATASKI"]);
        assert_eq!(page.duration, Some(176_000));
        assert_eq!(page.isrc.as_deref(), Some("FRX452603197"));
        assert!(structured_music(&body.replace("548933173", "111"), &url).is_none());
    }

    #[test]
    fn incomplete_disc_cannot_be_used_as_edition_evidence() {
        let link = public_link("https://music.apple.com/fr/album/1234").unwrap();
        let mut response = json!({"results": [
            {"wrapperType":"collection", "collectionType":"Album", "collectionId":1234,
                "collectionName":"Album", "artistName":"Artist", "trackCount":1},
            {"wrapperType":"track", "kind":"song", "collectionId":1234, "trackId":3,
                "discNumber":1, "trackNumber":1, "trackName":"Track", "artistName":"Artist",
                "trackTimeMillis":207900}
        ]});
        assert_eq!(
            apple_result(&response, &link, "fr").unwrap().tracks[0].duration,
            Some(207)
        );
        response["results"][0]["trackCount"] = json!(2);
        assert!(
            apple_result(&response, &link, "fr")
                .unwrap()
                .tracks
                .is_empty()
        );
    }

    #[test]
    fn apple_validation_checks_the_requested_storefront() {
        let link = public_link("https://music.apple.com/us/album/12?i=34").unwrap();
        assert_eq!(
            in_region(link.clone(), "FR").url,
            "https://music.apple.com/fr/album/12?i=34"
        );
        assert_eq!(in_region(link.clone(), "ZZ"), link);
    }

    #[test]
    #[ignore = "requires the public Apple Music page"]
    fn live_apple_geo_song_reads_the_same_recording() {
        let url = "https://geo.music.apple.com/us/song/617154366";
        let link = public_link(url).unwrap();
        let meta = public_metadata(&link, "US", &AtomicBool::new(false)).unwrap();
        assert_eq!(meta.title, "Get Lucky");
        assert!(meta.artists.iter().any(|artist| artist == "Daft Punk"));
    }
}
