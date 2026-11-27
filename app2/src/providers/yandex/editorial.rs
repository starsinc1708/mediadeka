use std::{collections::HashMap, sync::atomic::AtomicBool, thread};

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate};
use serde_json::Value;
use url::Url;

use super::{
    get,
    landing::{cover, cover_uri, valid_uuid},
    numeric_id,
    station::valid_seed,
};
use crate::providers::{
    Home, HomeItem, HomeSection, HomeSectionStyle, HomeTab,
    transport::{HOME_LIMIT, check_cancelled, field},
};

#[cfg(test)]
mod tests;

const SITE: &str = "https://music.yandex.ru";
const TAB_LIMIT: usize = 8;
const RELEASE_DATE_OFFSET_SECONDS: i32 = 3 * 60 * 60;
const SECTIONS: [&str; 7] = [
    "/landing-blocks/in-style",
    "/landing-blocks/waves",
    "/landing-blocks/new-releases",
    "/landing/block/editorial/new-releases/ALL_albums_of_the_month",
    "/landing/block/mixes/mixes_music?count=5",
    "/landing/block/editorial/compilation/foreign",
    "/landing-blocks/new-playlists",
];
const MONTHS: [&str; 12] = [
    "января",
    "февраля",
    "марта",
    "апреля",
    "мая",
    "июня",
    "июля",
    "августа",
    "сентября",
    "октября",
    "ноября",
    "декабря",
];

pub(super) struct Editorial {
    in_style: Vec<Group>,
    waves: Vec<Group>,
    releases: Vec<Group>,
    collections: Vec<HomeItem>,
    missed: Vec<HomeItem>,
    new_music: Vec<HomeItem>,
}

struct Group {
    title: String,
    artwork: Option<String>,
    items: Vec<HomeItem>,
}

impl Editorial {
    pub(super) fn fetch(agent: &ureq::Agent, token: &str, cancelled: &AtomicBool) -> Self {
        let answers = thread::scope(|scope| {
            SECTIONS
                .map(|path| {
                    scope.spawn(move || {
                        check_cancelled(cancelled).and_then(|()| get(agent, path, token))
                    })
                })
                .map(|worker| {
                    worker
                        .join()
                        .expect("Yandex home request")
                        .unwrap_or(Value::Null)
                })
        });
        Self::parse(&answers)
    }

    fn parse(answers: &[Value; 7]) -> Self {
        let [
            style,
            waves,
            releases,
            trends,
            collections,
            missed,
            new_music,
        ] = answers;
        Self {
            in_style: array(style, "inStyleTabs")
                .take(TAB_LIMIT)
                .filter_map(|tab| {
                    Some(Group {
                        title: field(tab, "title")?,
                        artwork: cover(tab),
                        items: array(tab, "items")
                            .filter_map(|item| album(item, false))
                            .take(HOME_LIMIT)
                            .collect(),
                    })
                })
                .collect(),
            waves: array(waves, "waves")
                .take(TAB_LIMIT)
                .filter_map(|tab| {
                    Some(Group {
                        title: field(tab, "title")?,
                        artwork: None,
                        items: array(tab, "items")
                            .filter_map(wave)
                            .take(HOME_LIMIT)
                            .collect(),
                    })
                })
                .collect(),
            releases: [("Для вас", releases), ("Тренды", trends)]
                .map(|(title, answer)| Group {
                    title: title.into(),
                    artwork: None,
                    items: array(answer, "newReleases")
                        .filter_map(|item| album(item, true))
                        .take(HOME_LIMIT)
                        .collect(),
                })
                .into(),
            collections: array(collections, "items")
                .filter_map(collection)
                .take(HOME_LIMIT)
                .collect(),
            missed: array(missed, "items")
                .filter_map(playlist)
                .take(HOME_LIMIT)
                .collect(),
            new_music: array(new_music, "items")
                .filter_map(playlist)
                .take(HOME_LIMIT)
                .collect(),
        }
    }

    pub(super) fn has_waves(&self) -> bool {
        self.waves.iter().any(|group| !group.items.is_empty())
    }

    pub(super) fn append(self, home: &mut Home) {
        use HomeSectionStyle::{Cards, Collections, Releases};
        add_groups(home, "В стиле", Cards, self.in_style);
        add_groups(home, "Волна открытий", Cards, self.waves);
        add_groups(home, "Новые релизы", Releases, self.releases);
        add_shelf(home, "Подборки музыки", Collections, self.collections);
        add_shelf(home, "Вы могли пропустить", Cards, self.missed);
        add_shelf(home, "Больше новой музыки", Cards, self.new_music);
    }
}

fn array<'a>(value: &'a Value, key: &str) -> impl Iterator<Item = &'a Value> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

fn add_shelf(home: &mut Home, title: &str, style: HomeSectionStyle, items: Vec<HomeItem>) {
    add_groups(
        home,
        title,
        style,
        vec![Group {
            title: String::new(),
            artwork: None,
            items,
        }],
    );
}

fn add_groups(home: &mut Home, title: &str, style: HomeSectionStyle, groups: Vec<Group>) {
    let start = home.items.len();
    let mut keys = HashMap::new();
    let tabs = groups
        .into_iter()
        .filter_map(|group| {
            let mut indices = Vec::with_capacity(group.items.len());
            for item in group.items {
                let index = *keys.entry(item.id.clone()).or_insert_with(|| {
                    let index = home.items.len();
                    home.items.push(item);
                    index
                });
                if !indices.contains(&index) {
                    indices.push(index);
                }
            }
            (!indices.is_empty() && !group.title.is_empty()).then_some(HomeTab {
                title: group.title,
                artwork: group.artwork,
                indices,
            })
        })
        .collect();
    let len = home.items.len() - start;
    if len > 0 {
        home.sections.push(HomeSection {
            title: title.into(),
            start,
            len,
            tabs,
            style,
            ..Default::default()
        });
    }
}

fn album(entry: &Value, release: bool) -> Option<HomeItem> {
    let album = entry.get("album")?;
    let id = numeric_id(&album["id"])?;
    let artist_list = entry["artists"]
        .as_array()
        .or_else(|| album["artists"].as_array());
    let artists = artist_list
        .into_iter()
        .flatten()
        .filter_map(|artist| field(artist, "name"))
        .take(4)
        .collect::<Vec<_>>()
        .join(", ");
    let format = match album["albumType"].as_str() {
        Some("single") => Some("Сингл"),
        Some("ep") => Some("EP"),
        _ if release => Some("Альбом"),
        _ => None,
    };
    let date = release
        .then(|| entry["releaseDate"].as_str().and_then(release_date))
        .flatten();
    let subtitle = [
        Some(artists.as_str()).filter(|name| !name.is_empty()),
        format,
        date.as_deref(),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" · ");
    let artwork = if release {
        cover(entry)
            .or_else(|| artist_list.and_then(|artists| artists.iter().find_map(cover)))
            .or_else(|| cover(album))
    } else {
        cover(album)
    };
    Some(HomeItem {
        id: format!("album:{id}"),
        title: field(album, "title")?,
        subtitle,
        url: Some(format!("{SITE}/album/{id}")),
        artwork,
    })
}

fn release_date(date: &str) -> Option<String> {
    let offset = FixedOffset::east_opt(RELEASE_DATE_OFFSET_SECONDS)?;
    let date = DateTime::parse_from_rfc3339(date)
        .map(|date| date.with_timezone(&offset).date_naive())
        .or_else(|_| NaiveDate::parse_from_str(date, "%Y-%m-%d"))
        .ok()?;
    let month = MONTHS[date.month0() as usize];
    Some(format!("{} {month}", date.day()))
}

fn wave(entry: &Value) -> Option<HomeItem> {
    let id = field(entry, "stationId")
        .filter(|id| valid_seed(id))
        .or_else(|| {
            entry["seeds"][0]
                .as_str()
                .filter(|seed| valid_seed(seed))
                .map(str::to_owned)
        })?;
    Some(HomeItem {
        id: format!("wave:{id}"),
        title: field(entry, "title")?,
        subtitle: field(entry, "header").unwrap_or_default(),
        // Starting a seed is part of the service's player, not a public destination.
        url: None,
        artwork: cover(&entry["agent"])
            .or_else(|| entry["compactImageUrl"].as_str().and_then(cover_uri))
            .or_else(|| entry["backgroundImageUrl"].as_str().and_then(cover_uri)),
    })
}

fn collection(entry: &Value) -> Option<HomeItem> {
    let data = entry.get("data")?;
    let id = field(data, "id")?;
    let url = Url::parse(SITE)
        .ok()?
        .join(data["action"]["weblink"].as_str()?)
        .ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("music.yandex.ru")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !(url.path().starts_with("/tag/") || url.path().starts_with("/landing/"))
    {
        return None;
    }
    Some(HomeItem {
        id: format!("collection:{id}"),
        title: field(data, "title")?,
        subtitle: String::new(),
        url: Some(url.into()),
        artwork: data["covers"][0]["uri"].as_str().and_then(cover_uri),
    })
}

fn playlist(entry: &Value) -> Option<HomeItem> {
    let data = entry.get("data")?;
    let playlist = data.get("playlist")?;
    let id = field(playlist, "playlistUuid").filter(|id| valid_uuid(id))?;
    Some(HomeItem {
        id: format!("playlist:{id}"),
        title: field(playlist, "title")?,
        subtitle: data["likesCount"]
            .as_u64()
            .map(|count| format!("♡ {}", thousands(count)))
            .unwrap_or_default(),
        url: Some(format!("{SITE}/playlists/{id}")),
        artwork: cover(playlist),
    })
}

fn thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut result = String::with_capacity(digits.len() * 2);
    for (index, digit) in digits.bytes().enumerate() {
        if index != 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(' ');
        }
        result.push(char::from(digit));
    }
    result
}
