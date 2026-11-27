use std::{
    collections::{HashMap, HashSet},
    sync::atomic::AtomicBool,
};

use crate::{
    playlist::PlaylistTrack,
    providers::{
        HomeItem, ProviderError,
        collections::{Collection, CollectionKind, SavedItem},
        transport::{agent, check_cancelled, field, invalid_response},
    },
};

use super::{PROVIDER, catalog, get, landing::playlist_item, numeric_id, playlists::track};

const BATCH_SIZE: usize = 100;

pub(in crate::providers) fn read_collection(
    token: &str,
    user: &str,
    kind: CollectionKind,
    cancelled: &AtomicBool,
) -> Result<Collection, ProviderError> {
    if user.is_empty() || !user.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_response(PROVIDER));
    }
    let agent = agent();
    match kind {
        CollectionKind::Liked => liked(&agent, token, user, cancelled).map(Collection::Liked),
        CollectionKind::Albums => {
            saved(&agent, token, user, "album", catalog::album).map(Collection::Albums)
        }
        CollectionKind::Artists => {
            saved(&agent, token, user, "artist", catalog::artist).map(Collection::Artists)
        }
        CollectionKind::Playlists => {
            playlists(&agent, token, user, cancelled).map(Collection::Playlists)
        }
    }
}

fn saved(
    agent: &ureq::Agent,
    token: &str,
    user: &str,
    key: &'static str,
    parse: impl Fn(&serde_json::Value) -> Option<crate::music::MusicPage>,
) -> Result<Vec<SavedItem>, ProviderError> {
    let data = get(agent, &format!("/users/{user}/likes/{key}s"), token)?;
    let rows = data["result"]
        .as_array()
        .ok_or_else(|| invalid_response(PROVIDER))?;
    let mut seen = HashSet::new();
    Ok(rows
        .iter()
        .filter_map(|row| {
            let page = parse(&row[key])?;
            if !seen.insert(page.reference.id.clone()) {
                return None;
            }
            Some(SavedItem {
                item: HomeItem {
                    id: format!("{key}:{}", page.reference.id),
                    title: page.title,
                    subtitle: page.artists.join(", "),
                    url: page.url,
                    artwork: page.artwork,
                },
                added: field(row, "timestamp"),
                year: page.year,
            })
        })
        .collect())
}

fn liked(
    agent: &ureq::Agent,
    token: &str,
    user: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<PlaylistTrack>, ProviderError> {
    let data = get(agent, &format!("/users/{user}/likes/tracks"), token)?;
    let entries = data["result"]["library"]["tracks"]
        .as_array()
        .ok_or_else(|| invalid_response(PROVIDER))?;
    let mut rows = Vec::with_capacity(entries.len());
    for batch in entries.chunks(BATCH_SIZE) {
        check_cancelled(cancelled)?;
        let ids = batch
            .iter()
            .map(|entry| {
                let id = numeric_id(&entry["id"]).ok_or_else(|| invalid_response(PROVIDER))?;
                Ok(match numeric_id(&entry["albumId"]) {
                    Some(album) => format!("{id}:{album}"),
                    None => id,
                })
            })
            .collect::<Result<Vec<_>, ProviderError>>()?;
        let response = get(
            agent,
            &format!("/tracks?track-ids={}", ids.join(",")),
            token,
        )?;
        let full = response["result"]
            .as_array()
            .ok_or_else(|| invalid_response(PROVIDER))?;
        let by_id = full
            .iter()
            .filter_map(|row| Some((numeric_id(&row["id"])?, row)))
            .collect::<HashMap<_, _>>();
        for entry in batch {
            let mut entry = entry.clone();
            if let Some(id) = numeric_id(&entry["id"])
                && let Some(value) = by_id.get(&id)
            {
                entry["track"] = (*value).clone();
            }
            rows.push(track(&entry, rows.len()));
        }
    }
    Ok(rows)
}

fn playlists(
    agent: &ureq::Agent,
    token: &str,
    user: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<HomeItem>, ProviderError> {
    let owned = get(agent, &format!("/users/{user}/playlists/list"), token)?;
    check_cancelled(cancelled)?;
    let liked = get(agent, &format!("/users/{user}/likes/playlists"), token)?;
    let mut seen = HashSet::new();
    let owned = owned["result"]
        .as_array()
        .ok_or_else(|| invalid_response(PROVIDER))?;
    let liked = liked["result"]
        .as_array()
        .ok_or_else(|| invalid_response(PROVIDER))?;
    Ok(owned
        .iter()
        .filter(|playlist| playlist["kind"] != 3)
        .filter_map(|playlist| playlist_item(user, playlist))
        .chain(
            liked
                .iter()
                .filter_map(|entry| playlist_item(user, &entry["playlist"])),
        )
        .filter(|playlist| seen.insert(playlist.id.clone()))
        .collect())
}
