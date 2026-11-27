use std::collections::HashSet;

use serde_json::Value;

use super::{PROVIDER, numeric_id, station::valid_seed};
use crate::providers::{
    Home, HomeItem, HomeSection,
    transport::{HOME_LIMIT, artwork_url, field},
};

const SITE: &str = "https://music.yandex.ru";

pub(super) fn parse(
    user: &str,
    playlists: &Value,
    wheel: &Value,
    include_legacy_waves: bool,
) -> Option<Home> {
    // The desktop client reads playlists and distributes the wheel into picks and mood stations.
    // Reference: aeviww/yandex-music-client at 08d514a, HomePage.vue and yandex/{library,radio}.rs.
    let playlists = playlists["result"].as_array();
    let wheel = wheel
        .get("result")
        .filter(|value| !value.is_null())
        .unwrap_or(wheel);
    let wheel = wheel["items"].as_array();
    if playlists.is_none() && wheel.is_none() {
        return None;
    }
    let mut home = Home {
        title: "Яндекс Музыка".into(),
        items: Vec::new(),
        sections: Vec::new(),
    };
    add_shelf(
        &mut home,
        "Твои плейлисты",
        playlists
            .into_iter()
            .flatten()
            .filter_map(|playlist| playlist_item(user, playlist)),
    );
    add_shelf(
        &mut home,
        "Новое для тебя",
        wheel.into_iter().flatten().filter_map(pick_item),
    );
    if include_legacy_waves {
        add_shelf(
            &mut home,
            "Волны по настроению",
            wheel.into_iter().flatten().filter_map(wave_item),
        );
    }
    Some(home)
}

fn add_shelf(home: &mut Home, title: &str, items: impl Iterator<Item = HomeItem>) {
    let mut seen = HashSet::new();
    let start = home.items.len();
    home.items.extend(
        items
            .filter(|item| seen.insert(item.id.clone()))
            .take(HOME_LIMIT),
    );
    let len = home.items.len() - start;
    if len > 0 {
        home.sections.push(HomeSection {
            title: title.into(),
            start,
            len,
            quick: false,
            ..Default::default()
        });
    }
}

pub(super) fn playlist_item(user: &str, value: &Value) -> Option<HomeItem> {
    let kind = numeric_id(&value["kind"])?;
    let title = field(value, "title")?;
    let owner_id = numeric_id(&value["owner"]["uid"])
        .or_else(|| numeric_id(&value["uid"]))
        .unwrap_or_else(|| user.into());
    let url = value["owner"]["login"]
        .as_str()
        .filter(|owner| valid_login(owner))
        .map(|owner| format!("{SITE}/users/{owner}/playlists/{kind}"))
        .or_else(|| {
            value["playlistUuid"]
                .as_str()
                .filter(|id| valid_uuid(id))
                .map(|id| format!("{SITE}/playlists/{id}"))
        });
    Some(HomeItem {
        id: format!("playlist:{owner_id}:{kind}"),
        title,
        subtitle: value["trackCount"]
            .as_u64()
            .map(track_count)
            .unwrap_or_else(|| "Плейлист".into()),
        url,
        artwork: cover(value),
    })
}

fn pick_item(value: &Value) -> Option<HomeItem> {
    let data = &value["data"];
    let is_album =
        matches!(value["type"].as_str(), Some(kind) if kind.eq_ignore_ascii_case("album"));
    let (kind, entity, title_field, label) = if is_album {
        ("album", &data["album"], "title", "Альбом")
    } else if matches!(value["type"].as_str(), Some(kind) if kind.eq_ignore_ascii_case("artist")) {
        ("artist", &data["artist"], "name", "Артист")
    } else {
        return None;
    };
    let id = numeric_id(&entity["id"]).or_else(|| numeric_id(&value["id"]))?;
    let artists = data["artists"]
        .as_array()
        .or_else(|| entity["artists"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|artist| field(artist, "name"))
        .take(6)
        .collect::<Vec<_>>()
        .join(", ");
    let subtitle = if is_album && !artists.is_empty() {
        artists
    } else {
        field(value, "description").unwrap_or_else(|| label.into())
    };
    Some(HomeItem {
        id: format!("{kind}:{id}"),
        title: field(entity, title_field)?,
        subtitle,
        url: Some(format!("{SITE}/{kind}/{id}")),
        artwork: cover(entity),
    })
}

fn wave_item(value: &Value) -> Option<HomeItem> {
    if !value["type"].as_str()?.eq_ignore_ascii_case("wave") {
        return None;
    }
    let data = &value["data"];
    let wave = &data["wave"];
    let id = wave["seeds"][0]
        .as_str()
        .filter(|seed| valid_seed(seed))
        .map(str::to_owned)
        .or_else(|| field(value, "id").filter(|id| valid_seed(id)))?;
    Some(HomeItem {
        id: format!("wave:{id}"),
        title: field(wave, "name")?,
        subtitle: field(wave, "description")
            .or_else(|| field(value, "description"))
            .unwrap_or_default(),
        // A station seed is a playback API identifier, not a destination on the web site.
        url: None,
        artwork: cover(&data["agent"]).or_else(|| cover(wave)),
    })
}

pub(super) fn cover(value: &Value) -> Option<String> {
    let template = field(&value["cover"], "uri")
        .or_else(|| field(value, "coverUri"))
        .or_else(|| field(value, "ogImage"))?;
    cover_uri(&template)
}

pub(super) fn cover_uri(template: &str) -> Option<String> {
    let address = if template.starts_with("//") {
        format!("https:{template}")
    } else if template.contains("://") {
        template.to_owned()
    } else {
        format!("https://{template}")
    };
    artwork_url(PROVIDER, &address.replace("%%", "400x400"))
}

pub(super) fn valid_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 128
        && login.bytes().any(|byte| byte.is_ascii_alphanumeric())
        && login
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

pub(super) fn valid_uuid(id: &str) -> bool {
    let id = id.strip_prefix("ar.").unwrap_or(id);
    id.len() == 36
        && id.bytes().enumerate().all(|(i, byte)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn track_count(count: u64) -> String {
    let suffix = match (count % 100, count % 10) {
        (11..=14, _) => "треков",
        (_, 1) => "трек",
        (_, 2..=4) => "трека",
        _ => "треков",
    };
    format!("{count} {suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_yandex_shelf_order_and_public_destinations() {
        let playlists = json!({"result": [{
            "kind": 1003, "title": "Личный плейлист", "trackCount": 22,
            "owner": {"uid": 11, "login": "test.user"},
            "cover": {"uri": "avatars.yandex.net/get-music-content/23/playlist/%%"}
        }]});
        let wheel = json!({"result": {"items": [
            {"type":"WAVE","id":"calm","data":{"wave":{"name":"Тихая волна","seeds":["mood:calm"]},
                "agent":{"cover":{"uri":"avatars.mds.yandex.net/get-music-content/1/wave/%%"}}}},
            {"type":"WAVE","id":"unknown","data":{"wave":{"name":"Без идентификатора станции"}}},
            {"type":"ALBUM","id":"12","data":{"album":{"title":"Альбом","cover":{"uri":"https://outside.invalid/cover"}},
                "artists":[{"name":"Группа"}]}},
            {"type":"ARTIST","id":40,"data":{"artist":{"id":40,"name":"Артист"}}},
            {"type":"ARTIST","id":40,"data":{"artist":{"id":40,"name":"Повтор"}}},
            {"type":"PODCAST","id":50,"data":{"artist":{"id":50,"name":"Подкаст"}}}
        ]}});
        let home = parse("11", &playlists, &wheel, true).expect("valid feed");
        assert_eq!(
            home.sections
                .iter()
                .map(|s| (s.title.as_str(), s.start, s.len, s.quick))
                .collect::<Vec<_>>(),
            [
                ("Твои плейлисты", 0, 1, false),
                ("Новое для тебя", 1, 2, false),
                ("Волны по настроению", 3, 1, false)
            ]
        );
        assert_eq!(
            home.items[0].url.as_deref(),
            Some("https://music.yandex.ru/users/test.user/playlists/1003")
        );
        assert_eq!(home.items[0].subtitle, "22 трека");
        assert!(
            home.items[0]
                .artwork
                .as_deref()
                .unwrap()
                .ends_with("/400x400")
        );
        assert_eq!(
            home.items[1].url.as_deref(),
            Some("https://music.yandex.ru/album/12")
        );
        assert_eq!(home.items[1].subtitle, "Группа");
        assert!(home.items[1].artwork.is_none());
        assert_eq!(
            home.items[2].url.as_deref(),
            Some("https://music.yandex.ru/artist/40")
        );
        assert_eq!(
            home.items[3].artwork.as_deref().unwrap(),
            "https://avatars.mds.yandex.net/get-music-content/1/wave/400x400"
        );
        assert!(home.items[3].url.is_none());
        assert_eq!(home.items[3].id, "wave:mood:calm");
    }

    #[test]
    fn rejects_unexpected_response_and_keeps_valid_empty_sections_empty() {
        assert!(
            parse(
                "11",
                &json!({"result": null}),
                &json!({"result": null}),
                true
            )
            .is_none()
        );
        let home = parse(
            "11",
            &json!({"result": [{"kind":1, "title":"Личный","owner":{"login":"../other"}}]}),
            &json!({"items": []}),
            true,
        )
        .unwrap();
        assert_eq!(home.sections.len(), 1);
        assert!(home.items[0].url.is_none());
    }
}
