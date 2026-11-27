use std::sync::atomic::AtomicBool;

use serde_json::Value;

use super::{
    PROVIDER, at, auth, me,
    playlist::{link, track, valid_item_id},
    profile_user, server,
};
use crate::providers::{
    ErrorKind, HomeItem, ProviderError,
    collections::{Collection, CollectionKind, MAX_PAGES, SavedItem},
    transport::{agent, check_cancelled, decode, field, invalid_response},
};

const PAGE_SIZE: usize = 200;

pub(in crate::providers) fn read_collection(
    address: &str,
    device: &str,
    token: &str,
    profile_id: &str,
    kind: CollectionKind,
    cancelled: &AtomicBool,
) -> Result<Collection, ProviderError> {
    let server = server(address)?;
    let user = profile_user(&server, profile_id)?;
    if !me(&server, device, token)?.0.eq_ignore_ascii_case(user) {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Подключите нужный профиль Jellyfin.",
        ));
    }
    let authorization = auth(device, Some(token))?;
    let agent = agent();
    let endpoint = at(&server, "Items");
    let mut liked = Vec::new();
    let mut playlists = Vec::new();
    let mut saved = Vec::new();
    let (item_type, prefix) = match kind {
        CollectionKind::Liked => ("Audio", "track"),
        CollectionKind::Albums => ("MusicAlbum", "album"),
        CollectionKind::Artists => ("MusicArtist", "artist"),
        CollectionKind::Playlists => ("Playlist", "playlist"),
    };
    let mut offset = 0;
    let mut expected_total = None;
    for _ in 0..MAX_PAGES {
        check_cancelled(cancelled)?;
        let mut request = agent
            .get(endpoint.as_str())
            .header("Authorization", &authorization)
            .query("UserId", user)
            .query("Recursive", "true")
            .query("StartIndex", offset.to_string())
            .query("Limit", PAGE_SIZE.to_string())
            .query("IncludeItemTypes", item_type)
            .query("SortBy", "SortName");
        if kind != CollectionKind::Playlists {
            request = request.query("Filters", "IsFavorite");
        }
        let data: Value = decode(PROVIDER, request.call())?;
        let rows = data["Items"]
            .as_array()
            .ok_or_else(|| invalid_response(PROVIDER))?;
        let total = data["TotalRecordCount"]
            .as_u64()
            .and_then(|count| usize::try_from(count).ok())
            .ok_or_else(|| invalid_response(PROVIDER))?;
        if expected_total.is_some_and(|expected| total != expected)
            || offset + rows.len() > total
            || (rows.is_empty() && offset < total)
        {
            return Err(invalid_response(PROVIDER));
        }
        expected_total = Some(total);
        match kind {
            CollectionKind::Liked => liked.extend(
                rows.iter()
                    .enumerate()
                    .map(|(index, row)| track(row, offset + index)),
            ),
            CollectionKind::Playlists => playlists.extend(
                rows.iter()
                    .filter(|row| {
                        row["MediaType"]
                            .as_str()
                            .is_none_or(|kind| kind == "Audio" || kind == "Unknown")
                    })
                    .filter_map(|row| {
                        let id = field(row, "Id").filter(|id| valid_item_id(id))?;
                        Some(HomeItem {
                            id: format!("playlist:{id}"),
                            title: field(row, "Name")?,
                            subtitle: "Плейлист".into(),
                            url: Some(link(&server, &id)),
                            artwork: None,
                        })
                    }),
            ),
            CollectionKind::Albums | CollectionKind::Artists => {
                saved.extend(rows.iter().filter_map(|row| {
                    let id = field(row, "Id").filter(|id| valid_item_id(id))?;
                    Some(SavedItem {
                        item: HomeItem {
                            id: format!("{prefix}:{id}"),
                            title: field(row, "Name")?,
                            subtitle: field(row, "AlbumArtist").unwrap_or_default(),
                            url: Some(link(&server, &id)),
                            artwork: None,
                        },
                        // Jellyfin exposes its item's creation date, not the favorite date.
                        added: None,
                        year: row["ProductionYear"]
                            .as_u64()
                            .and_then(|year| u32::try_from(year).ok())
                            .filter(|&year| year > 0),
                    })
                }));
            }
        }
        offset += rows.len();
        if offset == total {
            return Ok(match kind {
                CollectionKind::Liked => Collection::Liked(liked),
                CollectionKind::Albums => Collection::Albums(saved),
                CollectionKind::Artists => Collection::Artists(saved),
                CollectionKind::Playlists => Collection::Playlists(playlists),
            });
        }
    }
    Err(invalid_response(PROVIDER))
}
