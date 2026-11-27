use std::{
    collections::{HashMap, HashSet},
    sync::atomic::AtomicBool,
};

use serde_json::Value;
use url::form_urlencoded::Serializer;

use crate::{
    music::{MusicPage, ObjectKind, ObjectRef},
    providers::{Credentials, ErrorKind, HomeItem, ProviderError, catalog, transport::*},
};

use super::{
    super::{PROVIDER, landing::cover, playlists::SITE},
    page, playlist, request, user_playlist_id,
};

const TAG_PLAYLIST_BATCH: usize = 50;

pub(crate) fn tag_slug(id: &str) -> Option<&str> {
    let slug = id.strip_prefix("tag:")?;
    (!slug.is_empty()
        && slug.len() <= 128
        && slug
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    .then_some(slug)
}

pub(crate) fn tag_url(id: &str) -> Option<String> {
    Some(format!("{}/tag/{}", SITE, tag_slug(id)?))
}

pub(super) fn read(
    reference: &ObjectRef,
    credentials: Option<&Credentials>,
    cancelled: &AtomicBool,
) -> Result<MusicPage, ProviderError> {
    let slug = tag_slug(&reference.id).ok_or_else(|| catalog::invalid_id(PROVIDER))?;
    check_cancelled(cancelled)?;
    let agent = agent();
    let data = request(&agent, &format!("/tags/{slug}/playlist-ids"), credentials)?;
    check_cancelled(cancelled)?;
    let result = &data["result"];
    let tag = &result["tag"];
    let title = field(tag, "name")
        .filter(|_| tag["value"].as_str() == Some(slug))
        .ok_or_else(|| {
            ProviderError::new(ErrorKind::Unavailable, "Подборка Яндекс Музыки не найдена.")
        })?;
    let ids = tag_playlist_ids(&result["ids"]).ok_or_else(|| invalid_response(PROVIDER))?;
    let mut page = page(reference.id.clone(), ObjectKind::Collection, title);
    page.artwork = cover(tag);
    for batch in ids.chunks(TAG_PLAYLIST_BATCH) {
        check_cancelled(cancelled)?;
        let params = Serializer::new(String::new())
            .append_pair("playlistIds", &batch.join(","))
            .finish();
        let response = request(&agent, &format!("/playlists?{params}"), credentials)?;
        check_cancelled(cancelled)?;
        let rows = response["result"]["playlists"]
            .as_array()
            .ok_or_else(|| invalid_response(PROVIDER))?;
        page.releases.extend(tag_releases(batch, rows));
    }
    Ok(page)
}

fn tag_playlist_ids(value: &Value) -> Option<Vec<String>> {
    let mut seen = HashSet::new();
    Some(
        value
            .as_array()?
            .iter()
            .filter_map(user_playlist_id)
            .filter(|id| seen.insert(id.clone()))
            .collect(),
    )
}

fn tag_releases(batch: &[String], values: &[Value]) -> Vec<HomeItem> {
    let mut cards: HashMap<_, _> = values
        .iter()
        .filter_map(|value| {
            let id = user_playlist_id(value)?;
            let page = playlist(value)?;
            Some((
                id,
                HomeItem {
                    id: format!("playlist:{}", page.reference.id),
                    title: page.title,
                    subtitle: page.artists.join(", "),
                    url: page.url,
                    artwork: page.artwork,
                },
            ))
        })
        .collect();
    batch.iter().filter_map(|id| cards.remove(id)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tag_releases_keep_the_tags_order_and_a_valid_playlist_identity() {
        let ids = tag_playlist_ids(&json!([
            {"uid": 103372440, "kind": 2620},
            {"uid": 103372440, "kind": 1446},
            {"uid": "invalid", "kind": 1010},
            {"uid": 103372440, "kind": 2620}
        ]))
        .expect("tag ids");
        assert_eq!(ids, ["103372440:2620", "103372440:1446"]);
        let rows = [
            json!({"uid": 103372440, "kind": 1446, "title":"Second"}),
            json!({"uid": 7, "kind": 1000, "title":"Not requested"}),
            json!({
                "uid": 103372440, "kind": 2620, "title": "First",
                "playlistUuid": "f7efde37-1ae5-d8ba-b3bd-367168541a14"
            }),
        ];
        let cards = tag_releases(&ids, &rows);
        assert_eq!(cards.len(), 2);
        assert_eq!(cards[0].id, "playlist:f7efde37-1ae5-d8ba-b3bd-367168541a14");
        assert_eq!(cards[1].id, "playlist:103372440:1446");
    }

    #[test]
    #[ignore = "public Yandex Music catalog"]
    fn yandex_reads_shared_playlist_and_public_tag() {
        let cancelled = AtomicBool::new(false);
        let old =
            ObjectRef::from_url("https://music.yandex.ru/users/yamusic-podcast/playlists/1019")
                .expect("legacy link");
        let modern = ObjectRef::from_url(
            "https://music.yandex.ru/playlists/d54b98ef-0250-3e6d-aa13-4c0fb7bb0c43",
        )
        .expect("UUID link");
        let old_page = catalog::read(&old, None, &cancelled).expect("legacy playlist");
        let new_page = catalog::read(&modern, None, &cancelled).expect("UUID playlist");
        assert_eq!(old_page.url, new_page.url);
        assert_eq!(old_page.title, new_page.title);
        assert!(!new_page.tracks.is_empty());

        let tag = ObjectRef::from_url("https://music.yandex.ru/tag/autumn").expect("tag link");
        let page = catalog::read(&tag, None, &cancelled).expect("tag");
        assert_eq!(page.reference, tag);
        assert_eq!(
            page.url.as_deref(),
            Some("https://music.yandex.ru/tag/autumn")
        );
        assert!(!page.releases.is_empty());
        for item in page.releases {
            let reference = ObjectRef::from_item(PROVIDER, &item).expect("playlist card");
            assert_eq!(reference.kind, ObjectKind::Playlist);
            assert_eq!(reference.url(), item.url);
        }
    }
}
