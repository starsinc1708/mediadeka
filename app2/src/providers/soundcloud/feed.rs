use std::{collections::HashSet, sync::atomic::AtomicBool};

use serde_json::Value;
use url::Url;

use super::{API, PROVIDER, item};
use crate::providers::{
    HomeItem, ProviderError,
    transport::{HOME_LIMIT, check_cancelled, field, invalid_response},
};

const STREAM: &str = "/stream";
const MAX_PAGES: usize = 3;

pub(super) fn load(
    cancelled: &AtomicBool,
    mut get: impl FnMut(&str) -> Result<Value, ProviderError>,
) -> Result<Vec<HomeItem>, ProviderError> {
    let mut path = format!("{STREAM}?limit={HOME_LIMIT}&linked_partitioning=1");
    let mut pages = HashSet::new();
    let mut events = HashSet::new();
    let mut items = Vec::new();
    for _ in 0..MAX_PAGES {
        check_cancelled(cancelled)?;
        if !pages.insert(path.clone()) {
            break;
        }
        let response = get(&path);
        check_cancelled(cancelled)?;
        let response = response?;
        let rows = response["collection"]
            .as_array()
            .ok_or_else(|| invalid_response(PROVIDER))?;
        for row in rows {
            if let Some(item) = track(row) {
                let event = (
                    item.id.clone(),
                    row["type"].as_str().unwrap_or_default().to_owned(),
                    row["user"]["id"].as_u64(),
                    row["created_at"].as_str().unwrap_or_default().to_owned(),
                );
                if events.insert(event) {
                    items.push(item);
                    if items.len() == HOME_LIMIT {
                        break;
                    }
                }
            }
        }
        if items.len() == HOME_LIMIT {
            break;
        }
        let Some(next) = response["next_href"].as_str().and_then(next_path) else {
            break;
        };
        path = next;
    }
    Ok(items)
}

fn next_path(next: &str) -> Option<String> {
    let api = Url::parse(API).ok()?;
    let url = api.join(next).ok()?;
    if url.origin() != api.origin()
        || url.path() != STREAM
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    let mut has_cursor = false;
    for (key, value) in url.query_pairs() {
        if matches!(key.as_ref(), "cursor" | "offset") && !value.is_empty() {
            query.append_pair(&key, &value);
            has_cursor = true;
        }
    }
    if !has_cursor {
        return None;
    }
    query
        .append_pair("limit", &HOME_LIMIT.to_string())
        .append_pair("linked_partitioning", "1");
    Some(format!("{STREAM}?{}", query.finish()))
}

fn track(row: &Value) -> Option<HomeItem> {
    if !row["promoted"].is_null() && row["promoted"].as_bool() != Some(false) {
        return None;
    }
    let repost = match row["type"].as_str()? {
        "track" => false,
        "track-repost" => true,
        _ => return None,
    };
    let mut item = item::track(&row["track"])?;
    if repost {
        let repost = field(&row["user"], "username")
            .map_or_else(|| "Репост".to_owned(), |name| format!("Репост · {name}"));
        item.subtitle = if item.subtitle.is_empty() {
            repost
        } else {
            format!("{} · {repost}", item.subtitle)
        };
    }
    Some(item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::ErrorKind;
    use serde_json::json;
    use std::sync::atomic::Ordering;

    #[test]
    fn follows_a_sparse_page_and_keeps_publications_and_reposts_in_order() {
        let track = json!({
            "id": 42, "kind": "track", "title": "Track", "user": {
                "username": "Artist", "avatar_url": "https://i1.sndcdn.com/avatar?tracking=example"
            },
            "permalink_url": "https://soundcloud.com/artist/track?tracking=example#section"
        });
        let top = json!({"type":"track-repost", "user":{"id":12,"username":"Listener"}, "created_at":"2026-11-22T10:00:00Z", "track":track});
        let mut requests = Vec::new();
        let result = load(&AtomicBool::new(false), |path| {
            requests.push(path.to_owned());
            Ok(if requests.len() == 1 {
                json!({
                    "collection": [
                        {"type":"playlist","playlist":{"id":12}},
                        {"type":"track","track":track,"promoted":true},
                        top
                    ],
                    "next_href":"https://api-v2.soundcloud.com/stream?offset=2026%3A11&client_id=previous&oauth_token=secret&limit=1000"
                })
            } else {
                json!({"collection":[top, {"type":"track", "track":track, "created_at":"2026-11-21T10:00:00Z"}]})
            })
        })
        .unwrap();
        assert_eq!(
            requests,
            [
                "/stream?limit=24&linked_partitioning=1",
                "/stream?offset=2026%3A11&limit=24&linked_partitioning=1",
            ]
        );
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].subtitle, "Artist · Репост · Listener");
        assert_eq!(result[1].subtitle, "Artist");
        assert_eq!(
            result[0].url.as_deref(),
            Some("https://soundcloud.com/artist/track")
        );
        assert_eq!(
            result[0].artwork.as_deref(),
            Some("https://i1.sndcdn.com/avatar")
        );
    }

    #[test]
    fn caps_sparse_pages_and_refuses_continuations_outside_the_stream() {
        let cancelled = AtomicBool::new(false);
        let mut requests = 0;
        let result = load(&cancelled, |_| {
            requests += 1;
            Ok(json!({"collection": [], "next_href":format!("/stream?cursor={requests}")}))
        })
        .unwrap();
        assert_eq!(requests, MAX_PAGES);
        assert!(result.is_empty());
        for next in [
            "https://example.com/stream?cursor=foo",
            "https://api-v2.soundcloud.com/me?cursor=foo",
            "https://guest@api-v2.soundcloud.com/stream?cursor=foo",
            "http://api-v2.soundcloud.com/stream?cursor=foo",
            "/stream?client_id=foo",
        ] {
            assert!(next_path(next).is_none(), "{next}");
        }
    }

    #[test]
    fn stops_paging_when_the_snapshot_is_full() {
        let mut requests = 0;
        let result = load(&AtomicBool::new(false), |_| {
            requests += 1;
            let rows: Vec<_> = (1..=HOME_LIMIT + 1)
                .map(|id| json!({"type":"track", "track":{"id":id, "title":format!("Track {id}")}}))
                .collect();
            Ok(json!({"collection":rows, "next_href":"/stream?cursor=next"}))
        })
        .unwrap();
        assert_eq!(requests, 1);
        assert_eq!(result.len(), HOME_LIMIT);
    }

    #[test]
    fn omits_private_links_and_stops_after_cancellation() {
        let private = track(&json!({
            "type":"track",
            "track":{"id":1, "title":"Private", "sharing":"private",
                "permalink_url":"https://soundcloud.com/name/track/s-secret",
                "artwork_url":"https://i1.sndcdn.com/cover"}
        }))
        .unwrap();
        assert!(private.url.is_none() && private.artwork.is_none());
        let cancelled = AtomicBool::new(false);
        let error = load(&cancelled, |_| {
            cancelled.store(true, Ordering::Relaxed);
            Ok(json!({"collection":[], "next_href":"/stream?cursor=next"}))
        })
        .unwrap_err();
        assert_eq!(error.kind, ErrorKind::Cancelled);
    }
}
