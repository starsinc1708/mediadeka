use std::{collections::HashSet, sync::atomic::AtomicBool};

use serde_json::Value;
use url::Url;

use crate::providers::{
    ErrorKind, ProviderError,
    collections::{Collection, CollectionKind, MAX_PAGES, SavedItem},
    transport::{agent, check_cancelled, field, invalid_response},
};

use super::{API, PROVIDER, get_with, item, playlist::track, valid_track_id};

const PAGE_SIZE: u32 = 50;

pub(in crate::providers) fn read_collection(
    token: &str,
    client_id: &str,
    user: &str,
    kind: CollectionKind,
    cancelled: &AtomicBool,
) -> Result<Collection, ProviderError> {
    if !valid_track_id(user) {
        return Err(invalid_response(PROVIDER));
    }
    let agent = agent();
    let account = get_with(&agent, "/me", token, client_id)?;
    if account["id"].as_u64() != user.parse().ok() {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Подключите нужный профиль SoundCloud.",
        ));
    }
    let pages = |path: &str, visit: &mut dyn FnMut(&Value)| {
        read_pages(
            path,
            cancelled,
            |url| get_with(&agent, url, token, client_id),
            visit,
        )
    };
    let mut seen = HashSet::new();
    match kind {
        CollectionKind::Liked => {
            let mut tracks = Vec::new();
            pages(&format!("/users/{user}/track_likes"), &mut |entry| {
                let row = entry.get("track").unwrap_or(entry);
                if let Some(id) = row["id"].as_u64().map(|id| id.to_string())
                    && valid_track_id(&id)
                    && seen.insert(id.clone())
                {
                    tracks.push(track(row, id, field(entry, "created_at")));
                }
            })?;
            Ok(Collection::Liked(tracks))
        }
        CollectionKind::Playlists => {
            let mut items = Vec::new();
            let mut add = |row: &Value| {
                if row["is_album"] != true
                    && let Some(card) = item::card(row)
                    && card
                        .id
                        .strip_prefix("playlist:")
                        .is_some_and(valid_track_id)
                    && seen.insert(card.id.clone())
                {
                    items.push(card);
                }
            };
            pages(&format!("/users/{user}/playlists_without_albums"), &mut add)?;
            pages("/me/library/all", &mut |entry| {
                if entry["type"] == "playlist-like" {
                    add(&entry["playlist"]);
                }
            })?;
            Ok(Collection::Playlists(items))
        }
        CollectionKind::Albums => {
            let mut albums = Vec::new();
            pages("/me/library/all", &mut |row| {
                let album = &row["playlist"];
                if row["type"] == "playlist-like"
                    && item::is_album(album)
                    && let Some(item) = item::card(album)
                    && seen.insert(item.id.clone())
                {
                    albums.push(SavedItem {
                        item,
                        added: field(row, "created_at"),
                        year: None,
                    });
                }
            })?;
            Ok(Collection::Albums(albums))
        }
        CollectionKind::Artists => {
            let mut artists = Vec::new();
            pages(&format!("/users/{user}/followings"), &mut |row| {
                if let Some(item) = item::artist(row)
                    && seen.insert(item.id.clone())
                {
                    artists.push(SavedItem {
                        item,
                        added: None,
                        year: None,
                    });
                }
            })?;
            Ok(Collection::Artists(artists))
        }
    }
}

fn read_pages(
    path: &str,
    cancelled: &AtomicBool,
    mut fetch: impl FnMut(&str) -> Result<Value, ProviderError>,
    mut visit: impl FnMut(&Value),
) -> Result<(), ProviderError> {
    let mut request = format!("{path}?limit={PAGE_SIZE}&linked_partitioning=1");
    let mut seen = HashSet::new();
    for _ in 0..MAX_PAGES {
        check_cancelled(cancelled)?;
        if !seen.insert(request.clone()) {
            return Err(invalid_response(PROVIDER));
        }
        let data = fetch(&request)?;
        let rows = data["collection"]
            .as_array()
            .or_else(|| data.as_array())
            .ok_or_else(|| invalid_response(PROVIDER))?;
        rows.iter().for_each(&mut visit);
        let Some(next) = data
            .get("next_href")
            .filter(|value| !value.is_null() && *value != "")
        else {
            return Ok(());
        };
        if rows.is_empty() {
            return Err(invalid_response(PROVIDER));
        }
        request = next_page(path, next.as_str()).ok_or_else(|| invalid_response(PROVIDER))?;
    }
    Err(invalid_response(PROVIDER))
}

fn next_page(path: &str, next: Option<&str>) -> Option<String> {
    let next = next
        .filter(|next| next.len() <= 8192)
        .and_then(|next| Url::parse(next).ok())?;
    if next.origin() != Url::parse(API).ok()?.origin()
        || next.path() != path
        || next.password().is_some()
        || !next.username().is_empty()
        || next.fragment().is_some()
    {
        return None;
    }
    let mut output = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in next.query_pairs() {
        if matches!(
            key.as_ref(),
            "client_id" | "oauth_token" | "access_token" | "token"
        ) {
            continue;
        }
        if key == "limit"
            && !value
                .parse::<u32>()
                .is_ok_and(|n| (1..=PAGE_SIZE).contains(&n))
        {
            return None;
        }
        output.append_pair(&key, &value);
    }
    let query = output.finish();
    (!query.is_empty()).then(|| format!("{path}?{query}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn continuation_stays_on_the_same_library_path_and_carries_no_credentials() {
        let path = "/users/3/track_likes";
        assert_eq!(
            next_page(
                path,
                Some(
                    "https://api-v2.soundcloud.com/users/3/track_likes?offset=2&client_id=old&oauth_token=old"
                )
            ),
            Some(format!("{path}?offset=2"))
        );
        assert_eq!(
            next_page(
                path,
                Some("https://example.com/users/3/track_likes?offset=2")
            ),
            None
        );
        assert_eq!(
            next_page(
                path,
                Some("https://api-v2.soundcloud.com/users/4/track_likes?offset=2")
            ),
            None
        );
    }

    #[test]
    fn cyclic_continuation_does_not_return_a_partial_collection() {
        let mut count = 0;
        let result = read_pages(
            "/me/library/all",
            &AtomicBool::new(false),
            |_| {
                count += 1;
                Ok(json!({
                    "collection":[{}],
                    "next_href":"https://api-v2.soundcloud.com/me/library/all?offset=1"
                }))
            },
            |_| {},
        );
        assert!(result.is_err());
        assert_eq!(count, 2);
    }
}
