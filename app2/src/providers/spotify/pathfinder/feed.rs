use std::collections::HashSet;

use serde_json::Value;

use crate::providers::{
    Home, HomeItem, HomeSection,
    transport::{artwork_from_images, artwork_url, field},
};

use super::super::PROVIDER;

const SHORTS: &str = "HomeShortsSectionData";
const GENERIC: &str = "HomeGenericSectionData";
const SHELF_LIMIT: usize = 32;

pub(super) fn compose(home: Option<&Value>, made: Option<&Value>) -> Option<Home> {
    let mut result = Home {
        title: "Рекомендации Spotify".into(),
        items: Vec::new(),
        sections: Vec::new(),
    };
    if let Some(home) = home {
        let shelves = entries(&home["home"]["sectionContainer"]["sections"]);
        if let Some((title, items)) = shelves
            .clone()
            .filter(|shelf| kind(shelf) == Some(SHORTS))
            .find_map(|shelf| section(shelf, true))
        {
            append(&mut result, title, items, true, false);
        }
        for (title, items) in shelves
            .filter(|shelf| kind(shelf) == Some(GENERIC))
            .filter_map(|shelf| section(shelf, false))
            .take(SHELF_LIMIT)
        {
            append(&mut result, title, items, false, false);
        }
    }
    if let Some(made) = made {
        append_made(&mut result, &made["browse"]["sections"]);
    }
    (!result.items.is_empty()).then_some(result)
}

pub(super) fn collection(data: &Value) -> Option<Home> {
    let data = &data["browse"];
    let title = label(&data["header"]["title"]).or_else(|| label(&data["title"]))?;
    let mut home = Home {
        title,
        items: Vec::new(),
        sections: Vec::new(),
    };
    append_made(&mut home, &data["sections"]);
    Some(home)
}

fn append_made(result: &mut Home, shelves: &Value) {
    let mut seen = result
        .sections
        .iter()
        .filter(|section| !section.quick)
        .flat_map(|section| &result.items[section.start..section.start + section.len])
        .map(|item| item.id.clone())
        .collect::<HashSet<_>>();
    for (title, mut items) in entries(shelves)
        .filter_map(|shelf| section(shelf, false))
        .take(SHELF_LIMIT)
    {
        if let Some(section) = result
            .sections
            .iter_mut()
            .find(|section| section.title == title)
        {
            section.personal = true;
            continue;
        }
        items.retain(|item| seen.insert(item.id.clone()));
        if !items.is_empty() {
            append(result, title, items, false, true);
        }
    }
}

fn append(result: &mut Home, title: String, items: Vec<HomeItem>, quick: bool, personal: bool) {
    result.sections.push(HomeSection {
        title,
        start: result.items.len(),
        len: items.len(),
        quick,
        personal,
        ..Default::default()
    });
    result.items.extend(items);
}

fn kind(shelf: &Value) -> Option<&str> {
    shelf["data"]["__typename"].as_str()
}

fn entries(value: &Value) -> impl Iterator<Item = &Value> + Clone {
    value["items"].as_array().into_iter().flatten()
}

fn section(shelf: &Value, quick: bool) -> Option<(String, Vec<HomeItem>)> {
    let title = if quick {
        "Быстрый выбор".into()
    } else {
        label(&shelf["data"]["title"]).unwrap_or_else(|| "Рекомендации Spotify".into())
    };
    let mut seen = HashSet::new();
    let items = entries(&shelf["sectionItems"])
        .filter_map(item)
        .filter(|item| seen.insert(item.id.clone()))
        .take(super::ITEMS_PER_SHELF)
        .collect::<Vec<_>>();
    (!items.is_empty()).then_some((title, items))
}

fn item(entry: &Value) -> Option<HomeItem> {
    let data = &entry["content"]["data"];
    let uri = [entry["uri"].as_str(), data["uri"].as_str()]
        .into_iter()
        .flatten()
        .find(|uri| spotify_entity(uri).is_some())?;
    let (kind, id) = spotify_entity(uri)?;
    let (title, subtitle, artwork, destination) = match kind {
        "playlist" => (
            field(data, "name")?,
            field(&data["ownerV2"]["data"], "name").unwrap_or_else(|| "Плейлист".into()),
            entries(&data["images"]).find_map(art),
            format!("playlist/{id}"),
        ),
        "album" => (
            field(data, "name")?,
            artist_names(&data["artists"], "Альбом"),
            art(&data["coverArt"]),
            format!("album/{id}"),
        ),
        "artist" => (
            field(&data["profile"], "name")?,
            "Исполнитель".into(),
            art(&data["visuals"]["avatarImage"]),
            format!("artist/{id}"),
        ),
        "track" => (
            field(data, "name")?,
            artist_names(&data["artists"], "Трек"),
            art(&data["albumOfTrack"]["coverArt"]),
            format!("track/{id}"),
        ),
        "page" => {
            let card = &data["data"]["cardRepresentation"];
            let artwork = art(&card["artwork"])?;
            if artwork
                .rsplit('/')
                .next()
                .is_some_and(|file| file.starts_with("ab676563"))
            {
                return None;
            }
            (
                label(&card["title"])?,
                "Подборка Spotify".into(),
                Some(artwork),
                format!("genre/{id}"),
            )
        }
        "collection" => (
            field(data, "name").unwrap_or_else(|| "Любимые треки".into()),
            "Медиатека".into(),
            entries(&data["images"]).find_map(art),
            "collection/tracks".into(),
        ),
        _ => return None,
    };
    Some(HomeItem {
        id: uri.into(),
        title,
        subtitle,
        url: Some(format!("https://open.spotify.com/{destination}")),
        artwork,
    })
}

fn spotify_entity(uri: &str) -> Option<(&str, &str)> {
    let (kind, id) = uri.strip_prefix("spotify:")?.split_once(':')?;
    if kind == "collection" && id == "tracks" {
        return Some((kind, id));
    }
    if matches!(kind, "playlist" | "album" | "artist" | "track" | "page")
        && id.len() == 22
        && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
    {
        Some((kind, id))
    } else {
        None
    }
}

fn label(value: &Value) -> Option<String> {
    field(value, "transformedLabel").or_else(|| {
        value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    })
}

fn artist_names(value: &Value, fallback: &str) -> String {
    let names = entries(value)
        .filter_map(|artist| field(&artist["profile"], "name").or_else(|| field(artist, "name")))
        .collect::<Vec<_>>();
    if names.is_empty() {
        fallback.into()
    } else {
        names.join(", ")
    }
}

fn art(value: &Value) -> Option<String> {
    let sources = &value["sources"];
    artwork_from_images(PROVIDER, sources).or_else(|| {
        let images = sources.as_array()?.iter();
        images
            .filter_map(|source| source["url"].as_str())
            .find_map(spotify_image_uri)
    })
}

fn spotify_image_uri(uri: &str) -> Option<String> {
    let digest = uri.strip_prefix("spotify:image:")?;
    if digest.len() == 40 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        artwork_url(PROVIDER, &format!("https://i.scdn.co/image/{digest}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn keeps_quick_picks_and_recommendations_with_safe_destinations() {
        let track = json!({
            "uri": "spotify:track:1234567890123456789012",
            "content": { "data": {
                "name": "Recently played",
                "artists": { "items": [{ "profile": { "name": "Artist" } }] },
                "albumOfTrack": { "coverArt": { "sources": [
                    { "url": "https://i.scdn.co/image/track" }
                ] } }
            } }
        });
        let playlist = json!({
            "uri": "spotify:playlist:1234567890123456789012",
            "content": { "data": {
                "name": "Daily mix",
                "ownerV2": { "data": { "name": "Spotify" } },
                "images": { "items": [{ "sources": [
                    { "url": "https://dailymix-images.scdn.co/v2/img/mix?temporary=data" }
                ] }] }
            } }
        });
        let failed = json!({
            "uri": "spotify:album:../../browser-command",
            "content": { "data": { "name": "Invalid" } }
        });
        let primary = json!({ "home": { "sectionContainer": { "sections": { "items": [
            { "data": { "__typename": GENERIC, "title": { "transformedLabel": "Made for you" } },
              "sectionItems": { "items": [playlist.clone()] } },
            { "data": { "__typename": SHORTS },
              "sectionItems": { "items": [track, failed] } }
        ] } } } });
        let more = json!({ "browse": { "sections": { "items": [
            { "data": { "title": { "transformedLabel": "Only duplicates" } },
              "sectionItems": { "items": [playlist] } }
        ] } } });
        let home = compose(Some(&primary), Some(&more)).unwrap();
        assert_eq!(home.sections.len(), 2);
        assert!(home.sections[0].quick);
        assert_eq!(home.sections[1].start, 1);
        assert_eq!(home.sections[1].len, 1);
        assert_eq!(home.items[0].subtitle, "Artist");
        assert_eq!(
            home.items[0].url.as_deref(),
            Some("https://open.spotify.com/track/1234567890123456789012")
        );
        assert_eq!(
            home.items[1].artwork.as_deref(),
            Some("https://dailymix-images.scdn.co/v2/img/mix")
        );
    }

    #[test]
    fn can_show_made_for_you_when_primary_feed_is_unavailable() {
        let made = json!({ "browse": { "sections": { "items": [
            { "data": { "title": { "transformedLabel": "Your mixes" } },
              "sectionItems": { "items": [
                { "uri": "spotify:playlist:1234567890123456789012",
                  "content": { "data": { "name": "Daily Mix" } } }
              ] } }
        ] } } });
        let home = compose(None, Some(&made)).unwrap();
        assert_eq!(home.sections[0].title, "Your mixes");
        assert!(!home.sections[0].quick);
        assert_eq!(home.items[0].title, "Daily Mix");
    }
}
