use std::{
    collections::HashSet,
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use url::Url;

use super::{
    Auth, MUSIC_ORIGIN, PROVIDER, named, request_with_agent, signed_out, text, valid_browse_token,
};
use crate::providers::{
    Credentials, ErrorKind, Home, HomeItem, HomeSection, ProviderError,
    transport::{HOME_LIMIT, agent, artwork_from_images, check_cancelled},
};

const HOME: &str = "FEmusic_home";
pub(super) const LIBRARY_PLAYLISTS: &str = "FEmusic_liked_playlists";
const MAX_FOLLOW_UP_PAGES: usize = 2;
const FOLLOW_UP_BUDGET: Duration = Duration::from_secs(3);
const SHELF_LIMIT: usize = 16;
const RECENT_LIMIT: usize = 10;

pub(in crate::providers) fn is_mix_shelf(title: &str) -> bool {
    matches!(
        title.trim().to_lowercase().as_str(),
        "mixed for you" | "mixes for you" | "your mixes" | "миксы для вас" | "ваши миксы"
    )
}

/// The shelf order and signed-in library fallback follow Sonora's YouTube Music
/// feed at 8f01fd7fd59103bf0103906712955ddea392d135.
pub fn home(
    credentials: Option<&Credentials>,
    cancelled: &AtomicBool,
) -> Result<Home, ProviderError> {
    check_cancelled(cancelled)?;
    let auth = credentials.map(Auth::from_credentials);
    let agent = agent();
    let mut answer = request_with_agent(
        &agent,
        false,
        "browse",
        json!({"browseId": HOME}),
        auth,
        None,
        None,
    )
    .map_err(|error| {
        if auth.is_some()
            && matches!(
                error.kind,
                ErrorKind::Expired | ErrorKind::Rejected | ErrorKind::InvalidResponse
            )
        {
            personal_feed_unavailable()
        } else {
            error
        }
    })?;
    if auth.is_some() && signed_out(&answer) {
        return Err(personal_feed_unavailable());
    }
    let visitor = answer["responseContext"]["visitorData"]
        .as_str()
        .map(str::to_owned);
    let deadline = Instant::now() + FOLLOW_UP_BUDGET;
    let mut feed = Feed::default();
    let mut seen = HashSet::new();
    for page in 0..=MAX_FOLLOW_UP_PAGES {
        check_cancelled(cancelled)?;
        feed.collect(&answer);
        let remaining = deadline.saturating_duration_since(Instant::now());
        if page == MAX_FOLLOW_UP_PAGES || remaining.is_zero() {
            break;
        }
        let Some(token) = continuation(&answer).filter(|token| seen.insert((*token).to_owned()))
        else {
            break;
        };
        let next = request_with_agent(
            &agent,
            false,
            "browse",
            json!({"continuation": token}),
            auth,
            // Guest continuations belong to the visitor in the initial response.
            visitor.as_deref(),
            Some(remaining),
        );
        let Ok(next) = next else { break };
        if auth.is_some() && signed_out(&next) {
            break;
        }
        answer = next;
    }

    let remaining = deadline.saturating_duration_since(Instant::now());
    if auth.is_some() && !feed.has_library() && !remaining.is_zero() {
        check_cancelled(cancelled)?;
        if let Ok(library) = request_with_agent(
            &agent,
            false,
            "browse",
            json!({"browseId": LIBRARY_PLAYLISTS}),
            auth,
            None,
            Some(remaining),
        ) && !signed_out(&library)
        {
            feed.add_library(&library);
        }
    }
    check_cancelled(cancelled)?;
    let home = feed.into_home();
    if home.items.is_empty() {
        return Err(ProviderError::new(
            ErrorKind::Unavailable,
            "YouTube Music сейчас не показывает музыкальные подборки. Попробуйте ещё раз.",
        ));
    }
    Ok(home)
}

fn personal_feed_unavailable() -> ProviderError {
    ProviderError::new(
        ErrorKind::Unavailable,
        "YouTube Music не подтвердил персональную подборку. Попробуйте ещё раз.",
    )
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ShelfKind {
    Recent,
    Picks,
    Library,
    Recaps,
    Other,
    Skip,
}

impl ShelfKind {
    fn from_title(title: &str) -> Self {
        match title.to_lowercase().as_str() {
            "listen again"
            | "послушать ещё раз"
            | "послушать еще раз"
            | "послушайте ещё раз"
            | "послушайте еще раз"
            | "слушать снова"
            | "послушайте снова"
            | "послушать снова" => Self::Recent,
            "quick picks" | "быстрый выбор" | "быстрые рекомендации" | "быстрый подбор" => {
                Self::Picks
            }
            "from your library" | "из вашей медиатеки" | "из медиатеки" => {
                Self::Library
            }
            "recaps" | "ваши итоги" | "итоги" | "итоги прослушивания" => {
                Self::Recaps
            }
            "music videos for you"
            | "музыкальные видео для вас"
            | "видеоклипы для вас"
            | "подкасты"
            | "подкасты для вас"
            | "podcasts"
            | "podcasts for you"
            | "рекомендуемые подкасты"
            | "recommended podcasts" => Self::Skip,
            _ => Self::Other,
        }
    }

    fn order(self) -> usize {
        match self {
            Self::Library => 0,
            Self::Recaps => 1,
            _ => 2,
        }
    }
}

struct Shelf {
    kind: ShelfKind,
    title: String,
    items: Vec<HomeItem>,
}

#[derive(Default)]
struct Feed {
    shelves: Vec<Shelf>,
}

impl Feed {
    fn collect(&mut self, answer: &Value) {
        let Some(list) = sections(answer) else { return };
        for shelf in list["contents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row.get("musicCarouselShelfRenderer"))
        {
            let Some(title) =
                text(&shelf["header"]["musicCarouselShelfBasicHeaderRenderer"]["title"])
            else {
                continue;
            };
            let kind = ShelfKind::from_title(&title);
            if kind == ShelfKind::Skip {
                continue;
            }
            let mut seen = HashSet::new();
            let items = shelf["contents"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| home_item(row, &title))
                .filter(|item| kind != ShelfKind::Picks || item.id.starts_with("video:"))
                .filter(|item| seen.insert(item.id.clone()))
                .take(HOME_LIMIT)
                .collect();
            self.add(Shelf { kind, title, items });
        }
    }

    fn add(&mut self, shelf: Shelf) {
        if shelf.items.is_empty() {
            return;
        }
        if let Some(existing) = self.shelves.iter_mut().find(|existing| {
            existing.title == shelf.title
                || (shelf.kind != ShelfKind::Other && existing.kind == shelf.kind)
        }) {
            *existing = shelf;
        } else {
            self.shelves.push(shelf);
        }
    }

    fn has_library(&self) -> bool {
        self.shelves
            .iter()
            .any(|shelf| shelf.kind == ShelfKind::Library)
    }

    fn add_library(&mut self, answer: &Value) {
        let title = "Из вашей медиатеки";
        let mut grids = Vec::new();
        named(answer, "gridRenderer", &mut grids);
        let mut seen = HashSet::new();
        let items = grids
            .into_iter()
            .flat_map(|grid| grid["items"].as_array().into_iter().flatten())
            .filter_map(|row| home_item(row, title))
            .filter(|item| item.id.starts_with("browse:VL") && item.id != "browse:VLLM")
            .filter(|item| seen.insert(item.id.clone()))
            .take(RECENT_LIMIT)
            .collect();
        self.add(Shelf {
            kind: ShelfKind::Library,
            title: title.into(),
            items,
        });
    }

    fn into_home(mut self) -> Home {
        let mut home = Home {
            title: "Подборки YouTube Music".into(),
            items: Vec::new(),
            sections: Vec::new(),
        };
        let recent = take_shelf(&mut self.shelves, ShelfKind::Recent);
        let picks = take_shelf(&mut self.shelves, ShelfKind::Picks);
        let mut seen = HashSet::new();
        let quick = recent
            .into_iter()
            .take(RECENT_LIMIT)
            .chain(picks)
            .filter(|item| seen.insert(item.id.clone()))
            .take(HOME_LIMIT)
            .collect();
        append(&mut home, "Быстрый выбор".into(), quick, true);

        self.shelves.sort_by_key(|shelf| shelf.kind.order());
        for shelf in self.shelves.into_iter().take(SHELF_LIMIT) {
            append(&mut home, shelf.title, shelf.items, false);
        }
        home
    }
}

fn take_shelf(shelves: &mut Vec<Shelf>, kind: ShelfKind) -> Vec<HomeItem> {
    shelves
        .iter()
        .position(|shelf| shelf.kind == kind)
        .map_or_else(Vec::new, |index| shelves.remove(index).items)
}

fn append(home: &mut Home, title: String, items: Vec<HomeItem>, quick: bool) {
    if !items.is_empty() {
        home.sections.push(HomeSection {
            title,
            start: home.items.len(),
            len: items.len(),
            quick,
            ..Default::default()
        });
        home.items.extend(items);
    }
}

fn sections(answer: &Value) -> Option<&Value> {
    if let Some(continued) = answer.pointer("/continuationContents/sectionListContinuation") {
        return Some(continued);
    }
    let mut lists = Vec::new();
    named(&answer["contents"], "sectionListRenderer", &mut lists);
    lists.into_iter().find(|list| list["contents"].is_array())
}

fn continuation(answer: &Value) -> Option<&str> {
    let list = sections(answer)?;
    list["continuations"]
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|entry| entry["nextContinuationData"]["continuation"].as_str())
        .or_else(|| {
            list["contents"]
                .as_array()
                .into_iter()
                .flatten()
                .find_map(|entry| {
                    entry["continuationItemRenderer"]["continuationEndpoint"]["continuationCommand"]
                        ["token"]
                        .as_str()
                })
        })
        .filter(|token| valid_browse_token(token))
}

pub(super) fn home_item(row: &Value, label: &str) -> Option<HomeItem> {
    let (renderer, title, subtitle) = if let Some(tile) = row.get("musicTwoRowItemRenderer") {
        (tile, text(&tile["title"])?, text(&tile["subtitle"]))
    } else {
        let detail = row.get("musicResponsiveListItemRenderer")?;
        let column = |index: usize| {
            &detail["flexColumns"][index]["musicResponsiveListItemFlexColumnRenderer"]["text"]
        };
        (detail, text(column(0))?, text(column(1)))
    };
    let (id, url) = destination(renderer)?;
    let artwork = [&renderer["thumbnailRenderer"], &renderer["thumbnail"]]
        .into_iter()
        .find_map(|thumbnail| {
            artwork_from_images(
                PROVIDER,
                &thumbnail["musicThumbnailRenderer"]["thumbnail"]["thumbnails"],
            )
        });
    Some(HomeItem {
        id,
        title,
        subtitle: subtitle.unwrap_or_else(|| label.to_owned()),
        url: Some(url.into()),
        artwork,
    })
}

fn destination(renderer: &Value) -> Option<(String, Url)> {
    let mut url = Url::parse(MUSIC_ORIGIN).expect("fixed URL");
    let navigation = &renderer["navigationEndpoint"];
    if let Some(id) = navigation["browseEndpoint"]["browseId"]
        .as_str()
        .or_else(|| {
            renderer["title"]["runs"][0]["navigationEndpoint"]["browseEndpoint"]["browseId"]
                .as_str()
        })
    {
        if !safe_id(id) {
            return None;
        }
        if let Some(playlist) = id
            .strip_prefix("VL")
            .filter(|playlist| !playlist.is_empty())
        {
            url.set_path("/playlist");
            url.query_pairs_mut().append_pair("list", playlist);
        } else if id.starts_with("MPRE") || id.starts_with("UC") {
            url.path_segments_mut().ok()?.clear().extend(["browse", id]);
        } else {
            return None;
        }
        return Some((format!("browse:{id}"), url));
    }
    let overlay = &renderer["thumbnailOverlay"]["musicItemThumbnailOverlayRenderer"]["content"]["musicPlayButtonRenderer"]
        ["playNavigationEndpoint"]["watchEndpoint"];
    let watch = navigation.get("watchEndpoint").unwrap_or(overlay);
    if matches!(
        watch["watchEndpointMusicSupportedConfigs"]["watchEndpointMusicConfig"]["musicVideoType"]
            .as_str(),
        Some("MUSIC_VIDEO_TYPE_PODCAST_EPISODE" | "MUSIC_VIDEO_TYPE_NON_MUSIC_AUDIO")
    ) {
        return None;
    }
    let video = watch["videoId"]
        .as_str()
        .or_else(|| renderer["playlistItemData"]["videoId"].as_str())
        .filter(|id| id.len() == 11 && safe_id(id))?;
    url.set_path("/watch");
    url.query_pairs_mut().append_pair("v", video);
    Some((format!("video:{video}"), url))
}

pub(super) fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(name: &str, browse: &str) -> Value {
        json!({"musicTwoRowItemRenderer": {
            "title":{"runs":[{"text":name}]},
            "navigationEndpoint":{"browseEndpoint":{"browseId":browse}},
            "thumbnailRenderer":{"musicThumbnailRenderer":{"thumbnail":{"thumbnails":[
                {"url":"https://lh3.googleusercontent.com/cover", "width":160}
            ]}}}
        }})
    }

    fn track(id: &str, kind: &str) -> Value {
        json!({"musicTwoRowItemRenderer": {
            "title":{"runs":[{"text":id}]},
            "thumbnailOverlay":{"musicItemThumbnailOverlayRenderer":{"content":{
                "musicPlayButtonRenderer":{"playNavigationEndpoint":{"watchEndpoint":{
                    "videoId":id,
                    "watchEndpointMusicSupportedConfigs":{"watchEndpointMusicConfig":{"musicVideoType":kind}}
                }}}
            }}}
        }})
    }

    fn shelf(title: &str, cards: Vec<Value>) -> Value {
        json!({"musicCarouselShelfRenderer": {
            "header":{"musicCarouselShelfBasicHeaderRenderer":{"title":{"runs":[{"text":title}]}}},
            "contents":cards
        }})
    }

    #[test]
    fn maps_youtube_shelves_into_ordered_sections_with_music_only_picks() {
        let recent = track("a12b34c56d7", "MUSIC_VIDEO_TYPE_ATV");
        let recommended = track("ABCDEFGHIJK", "MUSIC_VIDEO_TYPE_ATV");
        let episode = track("abdefghijkl", "MUSIC_VIDEO_TYPE_PODCAST_EPISODE");
        let curated = card("Плейлист от сервиса", "VLPLexample");
        let mix = shelf(
            "Подборки",
            vec![
                curated.clone(),
                curated.clone(),
                card("Подкаст", "MPSPexample"),
            ],
        );
        let first = json!({"contents":{"sectionListRenderer":{"contents":[
            mix, shelf("Послушайте ещё раз", vec![recent.clone()]),
            shelf("Музыкальные видео для вас", vec![recommended.clone()])
        ], "continuations":[{"nextContinuationData":{"continuation":"page2%3D"}}]}}});
        let next = json!({"continuationContents":{"sectionListContinuation":{"contents":[
            shelf("Быстрый выбор", vec![recent, recommended, curated.clone(), episode]),
            shelf("Итоги", vec![curated.clone()]),
            shelf("Из вашей медиатеки", vec![curated])
        ]}}});
        assert_eq!(continuation(&first), Some("page2%3D"));
        assert_eq!(continuation(&next), None);
        let mut feed = Feed::default();
        feed.collect(&first);
        feed.collect(&next);
        let home = feed.into_home();
        let headings: Vec<_> = home
            .sections
            .iter()
            .map(|entry| (entry.title.as_str(), entry.quick, entry.start, entry.len))
            .collect();
        assert_eq!(
            headings,
            [
                ("Быстрый выбор", true, 0, 2),
                ("Из вашей медиатеки", false, 2, 1),
                ("Итоги", false, 3, 1),
                ("Подборки", false, 4, 1),
            ]
        );
        assert_eq!(
            home.items[0].url.as_deref(),
            Some("https://music.youtube.com/watch?v=a12b34c56d7")
        );
        assert_eq!(
            home.items[2].url.as_deref(),
            Some("https://music.youtube.com/playlist?list=PLexample")
        );
        assert_eq!(
            home.items[2].artwork.as_deref(),
            Some("https://lh3.googleusercontent.com/cover")
        );
    }
}
