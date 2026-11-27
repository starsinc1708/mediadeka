use std::sync::atomic::AtomicBool;

use serde_json::Value;

use crate::music::{ObjectKind, ObjectRef};
use crate::playlist::{Media, Playlist, PlaylistTrack};
use crate::providers::{ProviderError, catalog, playlists::unavailable_track, transport::*};

use super::{
    API, PROVIDER, get,
    landing::{cover, valid_login, valid_uuid},
    numeric_id,
};

pub(super) const SITE: &str = "https://music.yandex.ru";

fn digits(value: &str) -> bool {
    !value.is_empty() && value.len() <= 20 && value.bytes().all(|byte| byte.is_ascii_digit())
}

pub(crate) fn valid_track_id(id: &str) -> bool {
    id.split_once(':').map_or_else(
        || digits(id),
        |(track, album)| digits(track) && digits(album),
    )
}

pub(crate) fn valid_playlist_id(id: &str) -> bool {
    valid_uuid(id) || user_playlist_parts(id).is_some()
}

fn user_playlist_parts(id: &str) -> Option<(&str, &str)> {
    let (owner, kind) = id.split_once(':')?;
    (valid_login(owner) && digits(kind)).then_some((owner, kind))
}

pub(crate) fn playlist_url(id: &str) -> Option<String> {
    if valid_uuid(id) {
        return Some(format!("{SITE}/playlists/{id}"));
    }
    let (owner, kind) = user_playlist_parts(id)?;
    Some(format!("{SITE}/users/{owner}/playlists/{kind}"))
}

pub(crate) fn playlist(
    token: Option<&str>,
    id: &str,
    cancelled: &AtomicBool,
) -> Result<Playlist, ProviderError> {
    check_cancelled(cancelled)?;
    let endpoint = if let Some((owner, kind)) = user_playlist_parts(id) {
        format!("/users/{owner}/playlists/{kind}")
    } else if valid_uuid(id) {
        format!("/playlist/{id}?richTracks=true")
    } else {
        return Err(catalog::invalid_id(PROVIDER));
    };
    let agent = agent();
    let response: Value = match token {
        Some(token) => get(&agent, &endpoint, token),
        None => decode(PROVIDER, agent.get(&format!("{API}{endpoint}")).call()),
    }?;
    check_cancelled(cancelled)?;
    parse(response.get("result").unwrap_or(&response), id)
}

fn parse(value: &Value, id: &str) -> Result<Playlist, ProviderError> {
    let name = field(value, "title").ok_or_else(|| invalid_response(PROVIDER))?;
    let rows = value["tracks"]
        .as_array()
        .map(Vec::as_slice)
        .or_else(|| (value["trackCount"].as_u64() == Some(0)).then_some(&[][..]))
        .ok_or_else(|| invalid_response(PROVIDER))?;
    let link = field(value, "playlistUuid")
        .filter(|id| valid_uuid(id))
        .and_then(|uuid| playlist_url(&uuid))
        .or_else(|| {
            let (_, kind) = user_playlist_parts(id)?;
            let owner = field(&value["owner"], "login")?;
            playlist_url(&format!("{owner}:{kind}"))
        })
        .or_else(|| playlist_url(id));
    Ok(Playlist {
        name,
        owner: field(&value["owner"], "displayName")
            .or_else(|| field(&value["owner"], "name"))
            .or_else(|| field(&value["owner"], "login"))
            .map(crate::playlist::PlaylistOwner::Named),
        link,
        artwork: cover(value),
        tracks: rows
            .iter()
            .enumerate()
            .map(|(position, entry)| track(entry, position))
            .collect(),
    })
}

pub(super) fn track(entry: &Value, position: usize) -> PlaylistTrack {
    let track = entry.get("track").unwrap_or(entry);
    let track_id = numeric_id(&track["id"]).or_else(|| numeric_id(&entry["id"]));
    let entry_album = numeric_id(&entry["albumId"]);
    let mut albums = track["albums"].as_array().into_iter().flatten();
    let album = match entry_album.as_ref() {
        Some(id) => albums.find(|album| numeric_id(&album["id"]).as_ref() == Some(id)),
        None => albums.next(),
    };
    let album_id = entry_album.or_else(|| album.and_then(|album| numeric_id(&album["id"])));
    let id = track_id
        .as_deref()
        .zip(album_id.as_deref())
        .map(|(track, album)| format!("{track}:{album}"))
        .or(track_id)
        .unwrap_or_else(|| format!("missing:{position}"));
    let added = field(entry, "timestamp");
    let Some(title) = field(track, "title") else {
        return PlaylistTrack {
            added,
            ..unavailable_track(id)
        };
    };
    let artist_credits = super::catalog::artist_credits(track).collect::<Vec<_>>();
    let artists: Vec<_> = artist_credits
        .iter()
        .map(|credit| credit.name.clone())
        .collect();
    PlaylistTrack {
        media: (valid_track_id(&id)
            && track["available"] != false
            && field(track, "error").is_none())
        .then(|| Media::Provider {
            provider: PROVIDER,
            id: id.clone(),
        }),
        id,
        title: crate::music::qualified_track_title(&title, field(track, "version").as_deref()),
        artist: artists.join(", "),
        artists,
        artist_credits,
        album_ref: album_id.and_then(|id| ObjectRef::native(PROVIDER, ObjectKind::Album, id)),
        album: album.and_then(|album| field(album, "title")),
        artwork: cover(track).or_else(|| album.and_then(cover)),
        added,
        duration: track["durationMs"]
            .as_u64()
            .and_then(|ms| u32::try_from(ms / 1000).ok()),
    }
}
