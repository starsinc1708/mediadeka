use std::{collections::HashSet, sync::atomic::AtomicBool};

use serde_json::{Value, json};

use super::{
    Auth, PROVIDER, account,
    feed::{LIBRARY_PLAYLISTS, home_item},
    named,
    playlist::playlist,
    request_with_agent, signed_out, valid_browse_token,
};
use crate::providers::{
    Credentials, ErrorKind, HomeItem, Profile, ProviderError,
    collections::{Collection, CollectionKind, MAX_PAGES, SavedItem},
    transport::{agent, check_cancelled, field, invalid_response},
};

const LIKED_SONGS: &str = "LM";
const LIBRARY_ALBUMS: &str = "FEmusic_liked_albums";
const LIBRARY_SUBSCRIPTIONS: &str = "FEmusic_library_corpus_artists";

pub(in crate::providers) fn read_collection(
    credentials: &Credentials,
    profile: &Profile,
    kind: CollectionKind,
    cancelled: &AtomicBool,
) -> Result<Collection, ProviderError> {
    // The account index belongs to a browser session and can move when a different
    // account is signed out. Restore verifies the saved channel before reading it.
    let session = account::restore(credentials.clone(), profile, cancelled)?;
    let auth = Auth::from_credentials(&session.credentials);
    match kind {
        CollectionKind::Liked => playlist(Some(&session.credentials), LIKED_SONGS, cancelled)
            .map(|playlist| Collection::Liked(playlist.tracks)),
        CollectionKind::Playlists => cards(
            auth,
            LIBRARY_PLAYLISTS,
            "Плейлист",
            |id| id.starts_with("browse:VL") && id != "browse:VLLM",
            cancelled,
        )
        .map(Collection::Playlists),
        CollectionKind::Albums => {
            saved(auth, LIBRARY_ALBUMS, "Альбом", "browse:MPRE", cancelled).map(Collection::Albums)
        }
        CollectionKind::Artists => saved(
            auth,
            LIBRARY_SUBSCRIPTIONS,
            "Исполнитель",
            "browse:UC",
            cancelled,
        )
        .map(Collection::Artists),
    }
}

fn saved(
    auth: Auth<'_>,
    browse_id: &str,
    label: &str,
    prefix: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<SavedItem>, ProviderError> {
    cards(
        auth,
        browse_id,
        label,
        |id| id.starts_with(prefix),
        cancelled,
    )
    .map(|items| {
        items
            .into_iter()
            .map(|item| SavedItem {
                item,
                added: None,
                year: None,
            })
            .collect()
    })
}

fn cards(
    auth: Auth<'_>,
    browse_id: &str,
    label: &str,
    include: impl Fn(&str) -> bool,
    cancelled: &AtomicBool,
) -> Result<Vec<HomeItem>, ProviderError> {
    let agent = agent();
    let mut body = json!({"browseId": browse_id});
    let mut visitor = None;
    let mut seen_tokens = HashSet::new();
    let mut seen_items = HashSet::new();
    let mut items = Vec::new();
    for _ in 0..MAX_PAGES {
        check_cancelled(cancelled)?;
        let data = request_with_agent(
            &agent,
            false,
            "browse",
            body,
            Some(auth),
            visitor.as_deref(),
            None,
        )?;
        if signed_out(&data) {
            return Err(ProviderError::new(
                ErrorKind::Expired,
                "Войдите в YouTube Music снова.",
            ));
        }
        if visitor.is_none() {
            visitor = field(&data["responseContext"], "visitorData");
        }
        let (rows, next) = grid_page(&data)?;
        items.extend(
            rows.into_iter()
                .filter_map(|row| home_item(row, label))
                .filter(|item| include(&item.id))
                .filter(|item| seen_items.insert(item.id.clone())),
        );
        let Some(token) = next else {
            return Ok(items);
        };
        if !valid_browse_token(token) || !seen_tokens.insert(token.to_owned()) {
            return Err(invalid_response(PROVIDER));
        }
        body = json!({"continuation":token});
    }
    Err(invalid_response(PROVIDER))
}

fn grid_page(data: &Value) -> Result<(Vec<&Value>, Option<&str>), ProviderError> {
    let mut grids = Vec::new();
    for renderer in [
        "gridRenderer",
        "gridContinuation",
        "musicShelfRenderer",
        "musicShelfContinuation",
    ] {
        named(data, renderer, &mut grids);
    }
    let mut actions = Vec::new();
    named(data, "appendContinuationItemsAction", &mut actions);
    if grids.is_empty() && actions.is_empty() {
        let mut sections = Vec::new();
        named(
            &data["contents"]["singleColumnBrowseResultsRenderer"],
            "sectionListRenderer",
            &mut sections,
        );
        if sections.iter().any(|section| {
            section["contents"].as_array().is_some_and(|rows| {
                rows.iter()
                    .all(|row| row.get("musicEmptyStateRenderer").is_some())
            })
        }) {
            return Ok((Vec::new(), None));
        }
        return Err(invalid_response(PROVIDER));
    }
    let rows = grids
        .iter()
        .flat_map(|grid| {
            grid["items"]
                .as_array()
                .or_else(|| grid["contents"].as_array())
                .into_iter()
                .flatten()
        })
        .chain(
            actions
                .iter()
                .flat_map(|action| action["continuationItems"].as_array().into_iter().flatten()),
        )
        .collect::<Vec<_>>();
    let next = grids.into_iter()
        .flat_map(|grid| grid["continuations"].as_array().into_iter().flatten())
        .find_map(|entry| entry["nextContinuationData"]["continuation"].as_str())
        .or_else(|| rows.iter().find_map(|row| {
            row["continuationItemRenderer"]["continuationEndpoint"]["continuationCommand"]["token"].as_str()
        }));
    Ok((rows, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_pagination_uses_the_grid_token_and_ignores_sort_tokens() {
        let value = json!({
            "sortChip":{"nextContinuationData":{"continuation":"sort"}},
            "gridRenderer":{
                "items":[{"musicTwoRowItemRenderer":{}}],
                "continuations":[{"nextContinuationData":{"continuation":"next"}}]
            }
        });
        let (rows, token) = grid_page(&value).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(token, Some("next"));
    }

    #[test]
    fn subscriptions_read_the_shelf_and_continue_without_a_grid() {
        let value = json!({
            "continuationContents": {
                "musicShelfContinuation": {
                    "contents": [{"musicResponsiveListItemRenderer": {
                        "navigationEndpoint": {"browseEndpoint": {"browseId": "UC_artist"}},
                        "flexColumns": [{"musicResponsiveListItemFlexColumnRenderer": {
                            "text": {"runs": [{"text": "Artist"}]}
                        }}]
                    }}],
                    "continuations": [{"nextContinuationData": {"continuation": "next"}}]
                }
            }
        });
        let (rows, next) = grid_page(&value).unwrap();
        assert_eq!(next, Some("next"));
        assert_eq!(home_item(rows[0], "").unwrap().id, "browse:UC_artist");
    }

    #[test]
    fn album_title_navigation_opens_the_record_in_the_catalog() {
        let row = json!({"musicTwoRowItemRenderer": {"title": {"runs": [{
            "text": "Album",
            "navigationEndpoint": {"browseEndpoint": {"browseId": "MPRE_test"}}
        }]}}});
        assert_eq!(home_item(&row, "").unwrap().id, "browse:MPRE_test");
    }
}
