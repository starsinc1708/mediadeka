use std::sync::atomic::AtomicBool;

use serde_json::Value;
use url::Url;

use crate::{
    music::{ObjectKind, ObjectRef},
    playlist::{ArtistCredit, Media, Playlist, PlaylistTrack},
};

use super::{PROVIDER, at, auth, me, server};
use crate::providers::{
    ErrorKind, ProviderError,
    playlists::{PlaylistPage, PlaylistProgress, unavailable_track},
    transport::{agent, check_cancelled, decode, field, invalid_response},
};

const PAGE_SIZE: usize = 200;
const TICKS_PER_SECOND: u64 = 10_000_000;

pub(crate) fn valid_item_id(id: &str) -> bool {
    match id.as_bytes() {
        bytes if bytes.len() == 32 => bytes.iter().all(u8::is_ascii_hexdigit),
        bytes if bytes.len() == 36 => bytes.iter().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                *byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        }),
        _ => false,
    }
}

pub(super) fn link(server: &Url, id: &str) -> String {
    at(server, &format!("web/index.html#!/details?id={id}")).into()
}

pub(crate) fn playlist(
    address: &str,
    device: &str,
    token: &str,
    id: &str,
    cancelled: &AtomicBool,
    progress: &mut PlaylistProgress<'_>,
) -> Result<Playlist, ProviderError> {
    let server = server(address)?;
    let (user, _) = me(&server, device, token)?;
    check_cancelled(cancelled)?;
    let header = auth(device, Some(token))?;
    let agent = agent();
    let metadata: Value = decode(
        PROVIDER,
        agent
            .get(at(&server, &format!("Items/{id}")).as_str())
            .header("Authorization", &header)
            .query("UserId", &user)
            .call(),
    )?;
    if metadata["Type"] != "Playlist" {
        return Err(invalid_response(PROVIDER));
    }
    let name = field(&metadata, "Name").ok_or_else(|| invalid_response(PROVIDER))?;
    let endpoint = at(&server, &format!("Playlists/{id}/Items"));
    let mut playlist = Playlist {
        name,
        owner: None,
        artwork: None,
        link: Some(link(&server, id)),
        tracks: Vec::new(),
    };
    let mut expected_total = None;
    loop {
        check_cancelled(cancelled)?;
        let page: Value = decode(
            PROVIDER,
            agent
                .get(endpoint.as_str())
                .header("Authorization", &header)
                .query("UserId", &user)
                .query("StartIndex", playlist.tracks.len().to_string())
                .query("Limit", PAGE_SIZE.to_string())
                .call(),
        )?;
        let rows = page["Items"]
            .as_array()
            .ok_or_else(|| invalid_response(PROVIDER))?;
        let total = page["TotalRecordCount"]
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| invalid_response(PROVIDER))?;
        if expected_total.is_some_and(|expected| total != expected)
            || playlist.tracks.len() + rows.len() > total
            || (rows.is_empty() && playlist.tracks.len() < total)
        {
            return Err(ProviderError::new(
                ErrorKind::InvalidResponse,
                "Порядок плейлиста Jellyfin изменился во время загрузки. Обновите страницу.",
            ));
        }
        expected_total = Some(total);
        let offset = playlist.tracks.len();
        playlist.tracks.extend(
            rows.iter()
                .enumerate()
                .map(|(index, row)| track(row, offset + index)),
        );
        if playlist.tracks.len() == total {
            break;
        }
        check_cancelled(cancelled)?;
        progress(PlaylistPage {
            playlist: &playlist,
            tracks: &playlist.tracks[offset..],
            offset,
            total: Some(total),
        })?;
    }
    Ok(playlist)
}

pub(super) fn track(row: &Value, position: usize) -> PlaylistTrack {
    let id = field(row, "Id").filter(|id| valid_item_id(id));
    let entry = id
        .clone()
        .or_else(|| field(row, "PlaylistItemId"))
        .unwrap_or_else(|| format!("missing:{position}"));
    let Some(title) = field(row, "Name") else {
        return unavailable_track(entry);
    };
    let artist_credits = artist_credits(row);
    let artists: Vec<_> = artist_credits
        .iter()
        .map(|credit| credit.name.clone())
        .collect();
    PlaylistTrack {
        id: entry,
        title,
        artist: artists.join(", "),
        artists,
        artist_credits,
        album_ref: album_ref(row),
        album: field(row, "Album"),
        artwork: None,
        added: None,
        duration: row["RunTimeTicks"]
            .as_u64()
            .and_then(|ticks| u32::try_from(ticks / TICKS_PER_SECOND).ok()),
        media: id
            .filter(|_| row["Type"] == "Audio")
            .map(|id| Media::Provider {
                provider: PROVIDER,
                id,
            }),
    }
}

pub(super) fn album_ref(row: &Value) -> Option<ObjectRef> {
    catalog_id(row, "AlbumId").and_then(|id| ObjectRef::native(PROVIDER, ObjectKind::Album, id))
}

fn catalog_id(row: &Value, key: &str) -> Option<String> {
    field(row, key)
        .filter(|id| valid_item_id(id) && id.bytes().any(|byte| !matches!(byte, b'0' | b'-')))
}

fn credited_items(row: &Value, key: &str) -> Vec<ArtistCredit> {
    row[key]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let name = field(item, "Name")?;
            Some(ArtistCredit::from_catalog(
                name,
                PROVIDER,
                catalog_id(item, "Id"),
            ))
        })
        .collect()
}

pub(super) fn artist_credits(row: &Value) -> Vec<ArtistCredit> {
    let artists = credited_items(row, "ArtistItems");
    if !artists.is_empty()
        && row["ArtistItems"]
            .as_array()
            .is_some_and(|items| items.len() == artists.len())
    {
        return artists;
    }
    let names = row["Artists"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect::<Vec<_>>();
    if !names.is_empty() {
        return names
            .into_iter()
            .map(|name| {
                let mut matches = artists.iter().filter(|credit| credit.name == name);
                let reference = matches
                    .next()
                    .filter(|_| matches.next().is_none())
                    .and_then(|credit| credit.reference.clone());
                ArtistCredit {
                    name: name.to_owned(),
                    reference,
                }
            })
            .collect();
    }
    if !artists.is_empty() {
        return artists;
    }
    if row["Type"] == "MusicAlbum" {
        return credited_items(row, "AlbumArtists");
    }
    artists
}

pub(crate) fn track_stream(
    address: &str,
    device: &str,
    token: &str,
    id: &str,
) -> Result<String, ProviderError> {
    if !valid_item_id(id) {
        return Err(invalid_response(PROVIDER));
    }
    // Validate the stored token using the same rules as the authenticated catalog.
    auth(device, Some(token))?;
    let mut stream = at(&server(address)?, &format!("Audio/{id}/stream"));
    // This URL goes directly to the engine. It is never stored in the playlist model.
    stream
        .query_pairs_mut()
        .append_pair("static", "true")
        .append_pair("api_key", token);
    Ok(stream.into())
}
