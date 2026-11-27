use std::sync::atomic::AtomicBool;

use serde_json::{Value, json};

use crate::{
    music::lyrics::{Lookup, Lyrics, MAX_CACHE_AGE, MAX_LINE_BYTES, MAX_TEXT_BYTES, Source},
    providers::{ProviderError, catalog, transport::invalid_response},
};

use super::{PROVIDER, catalog::Client, feed::safe_id, named, text};

const LYRICS_PAGE: &str = "MUSIC_PAGE_TYPE_TRACK_LYRICS";

pub(in crate::providers) fn fetch(
    id: &str,
    cancelled: &AtomicBool,
) -> Result<Lookup, ProviderError> {
    if id.len() != 11 || !safe_id(id) {
        return Err(catalog::invalid_id(PROVIDER));
    }
    // The lyrics tab is public. Keep lookup out of the connected account's history.
    let client = Client::new(None);
    let next = client.request(
        "next",
        json!({
            "videoId": id,
            "playlistId": format!("RDAMVM{id}"),
            "enablePersistentPlaylistPanel": true,
        }),
        None,
        cancelled,
    )?;
    let tab = match lyrics_tab(&next, id) {
        Tab::Public(id) => id,
        Tab::Disabled => return Ok(Lookup::Missing(Some(MAX_CACHE_AGE))),
        Tab::Unknown => return Ok(Lookup::Indeterminate),
    };
    let visitor = next["responseContext"]["visitorData"].as_str();
    let response = client.request("browse", json!({"browseId": tab}), visitor, cancelled)?;
    let lyrics = plain(&response).ok_or_else(|| invalid_response(PROVIDER))?;
    Ok(Lookup::Found(lyrics.into(), Some(MAX_CACHE_AGE)))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab<'a> {
    Public(&'a str),
    Disabled,
    Unknown,
}

fn lyrics_tab<'a>(value: &'a Value, id: &str) -> Tab<'a> {
    if value["currentVideoEndpoint"]["watchEndpoint"]["videoId"].as_str() != Some(id) {
        return Tab::Unknown;
    }
    let mut tabs = Vec::new();
    named(value, "tabRenderer", &mut tabs);
    let mut lyrics = tabs.into_iter().filter_map(|tab| {
        let endpoint = &tab["endpoint"]["browseEndpoint"];
        let kind = &endpoint["browseEndpointContextSupportedConfigs"]
            ["browseEndpointContextMusicConfig"]["pageType"];
        let browse = endpoint["browseId"].as_str().filter(|id| safe_id(id));
        let typed = kind == LYRICS_PAGE || browse.is_some_and(|id| id.starts_with("MPLYt"));
        let disabled = tab["unselectable"].as_bool() == Some(true);
        if disabled && (typed || matches!(tab["title"].as_str(), Some("Текст" | "Lyrics"))) {
            return Some(Tab::Disabled);
        }
        typed.then(|| browse.map_or(Tab::Unknown, Tab::Public))
    });
    let result = lyrics.next().unwrap_or(Tab::Unknown);
    if lyrics.all(|other| other == result) {
        result
    } else {
        Tab::Unknown
    }
}

fn plain(value: &Value) -> Option<Lyrics> {
    let mut shelves = Vec::new();
    named(value, "musicDescriptionShelfRenderer", &mut shelves);
    shelves.into_iter().find_map(|shelf| {
        let description = text(&shelf["description"])?;
        if description.len() > MAX_TEXT_BYTES
            || description.lines().any(|line| line.len() > MAX_LINE_BYTES)
        {
            return None;
        }
        let mut lyrics = Lyrics::plain(Source::YouTubeMusic, &description);
        lyrics.attribution = text(&shelf["footer"]);
        lyrics.usable().then_some(lyrics)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_lyrics_require_the_exact_video_and_an_unambiguous_tab() {
        const ID: &str = "DyDfgMOUjCI";
        let route = |tabs| {
            json!({
                "currentVideoEndpoint": {"watchEndpoint": {"videoId": ID}},
                "contents": tabs,
            })
        };
        let tab = |browse| {
            json!({"tabRenderer": {
                "endpoint": {"browseEndpoint": {"browseId": browse,
                    "browseEndpointContextSupportedConfigs": {
                        "browseEndpointContextMusicConfig": {"pageType": LYRICS_PAGE}
                    }
                }}
            }})
        };
        let found = route(json!([tab("MPLYtabc"), tab("MPLYtabc")]));
        assert_eq!(lyrics_tab(&found, ID), Tab::Public("MPLYtabc"));
        assert_eq!(lyrics_tab(&found, "wrong-video"), Tab::Unknown);
        let ambiguous = route(json!([tab("MPLYtabc"), tab("MPLYtxyz")]));
        assert_eq!(lyrics_tab(&ambiguous, ID), Tab::Unknown);
        assert_eq!(lyrics_tab(&route(json!([])), ID), Tab::Unknown);
        let disabled = route(json!([{"tabRenderer": {"title": "Текст", "unselectable": true}}]));
        assert_eq!(lyrics_tab(&disabled, ID), Tab::Disabled);
    }
}
