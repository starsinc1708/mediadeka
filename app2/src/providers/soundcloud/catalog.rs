use std::sync::atomic::AtomicBool;

use chrono::{DateTime, Datelike};
use serde_json::Value;
use url::form_urlencoded::Serializer;

use crate::{
    music::{MusicPage, ObjectKind, ObjectRef},
    providers::{Credentials, ProviderError, catalog, transport::*},
};

use super::{PROVIDER, get_with, item, playlist, valid_track_id};

pub(in crate::providers) fn resolve_set(
    reference: &ObjectRef,
    credentials: &Credentials,
    cancelled: &AtomicBool,
) -> Result<ObjectRef, ProviderError> {
    let Credentials::SoundCloud { token, client_id } = credentials else {
        return Err(catalog::missing_access(PROVIDER));
    };
    let url = item::permalink(&reference.id, reference.kind)
        .ok_or_else(|| catalog::invalid_id(PROVIDER))?;
    let value = super::resolve(&agent(), &url, token, client_id)?;
    check_cancelled(cancelled)?;
    if value["kind"] != "playlist" {
        return Err(invalid_response(PROVIDER));
    }
    let id = value["id"]
        .as_u64()
        .ok_or_else(|| invalid_response(PROVIDER))?;
    Ok(ObjectRef {
        provider: PROVIDER,
        kind: if item::is_album(&value) {
            ObjectKind::Album
        } else {
            ObjectKind::Playlist
        },
        id: id.to_string(),
    })
}

pub(in crate::providers) fn read(
    reference: &ObjectRef,
    credentials: &Credentials,
    cancelled: &AtomicBool,
) -> Result<MusicPage, ProviderError> {
    let Credentials::SoundCloud { token, client_id } = credentials else {
        return Err(catalog::missing_access(PROVIDER));
    };
    let agent = agent();
    let fetch = |path: &str| get_with(&agent, path, token, client_id);
    let id = reference.id.as_str();
    let value = if reference.kind == ObjectKind::Track {
        super::stream::track_snapshot(&agent, id, token, client_id)?
    } else if valid_track_id(id) {
        let resource = match reference.kind {
            ObjectKind::Artist => "users",
            ObjectKind::Album | ObjectKind::Playlist => "playlists",
            ObjectKind::Track | ObjectKind::Collection | ObjectKind::Station => {
                return Err(catalog::invalid_id(PROVIDER));
            }
        };
        fetch(&format!("/{resource}/{id}?representation=full"))?
    } else {
        let url =
            item::permalink(id, reference.kind).ok_or_else(|| catalog::invalid_id(PROVIDER))?;
        super::resolve(&agent, &url, token, client_id)?
    };
    check_cancelled(cancelled)?;
    let mut page = from_entry(&value, reference.kind).ok_or_else(|| invalid_response(PROVIDER))?;
    if valid_track_id(id) && page.reference.id != id {
        return Err(invalid_response(PROVIDER));
    }
    match reference.kind {
        ObjectKind::Album => {
            page.tracks =
                playlist::from_response(&agent, &value, token, client_id, cancelled)?.tracks;
        }
        ObjectKind::Artist => {
            let user = &page.reference.id;
            let releases = fetch(&format!(
                "/users/{user}/playlists?limit={}",
                catalog::SEARCH_LIMIT
            ))?;
            page.releases = entries(&releases)?
                .iter()
                .filter(|value| item::is_album(value))
                .filter_map(item::card)
                .collect();
            check_cancelled(cancelled)?;
            let tracks = fetch(&format!(
                "/users/{user}/tracks?limit={}",
                catalog::SEARCH_LIMIT
            ))?;
            page.tracks = entries(&tracks)?
                .iter()
                .filter_map(|row| Some(playlist::track(row, row["id"].as_u64()?.to_string(), None)))
                .collect();
        }
        _ => {}
    }
    Ok(page)
}

pub(in crate::providers) fn search(
    credentials: &Credentials,
    kind: ObjectKind,
    query: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<MusicPage>, ProviderError> {
    let Credentials::SoundCloud { token, client_id } = credentials else {
        return Err(catalog::missing_access(PROVIDER));
    };
    let search = match kind {
        ObjectKind::Track => "tracks",
        ObjectKind::Album | ObjectKind::Playlist => "playlists",
        ObjectKind::Artist => "users",
        ObjectKind::Collection | ObjectKind::Station => return Err(catalog::invalid_id(PROVIDER)),
    };
    let params = Serializer::new(String::new())
        .append_pair("q", query)
        .append_pair("limit", &catalog::SEARCH_LIMIT.to_string())
        .finish();
    let data = get_with(
        &agent(),
        &format!("/search/{search}?{params}"),
        token,
        client_id,
    )?;
    check_cancelled(cancelled)?;
    Ok(entries(&data)?
        .iter()
        .filter_map(|row| from_entry(row, kind))
        .collect())
}

fn entries(value: &Value) -> Result<&[Value], ProviderError> {
    value
        .as_array()
        .or_else(|| value["collection"].as_array())
        .map(Vec::as_slice)
        .ok_or_else(|| invalid_response(PROVIDER))
}

fn from_entry(value: &Value, kind: ObjectKind) -> Option<MusicPage> {
    let actual = match value["kind"].as_str()? {
        "track" => ObjectKind::Track,
        "user" => ObjectKind::Artist,
        "playlist" if item::is_album(value) => ObjectKind::Album,
        "playlist" => ObjectKind::Playlist,
        _ => return None,
    };
    if actual != kind {
        return None;
    }
    let card = item::card(value)?;
    let id = value["id"].as_u64()?.to_string();
    let reference = ObjectRef {
        provider: PROVIDER,
        kind,
        id: id.clone(),
    };
    let mut page = MusicPage::new(reference, card.title);
    page.url = card.url;
    page.artwork = card.artwork;
    if matches!(
        kind,
        ObjectKind::Album | ObjectKind::Track | ObjectKind::Playlist
    ) {
        let publisher = &value["publisher_metadata"];
        page.artist_credits = item::artist_credit(value).into_iter().collect();
        page.artists = page
            .artist_credits
            .iter()
            .map(|credit| credit.name.clone())
            .collect();
        page.year = field(value, "release_date")
            .and_then(|date| DateTime::parse_from_rfc3339(&date).ok())
            .and_then(|date| date.year().try_into().ok());
        page.album = field(publisher, "album_title");
        page.isrc = field(publisher, "isrc");
        page.barcode = field(publisher, "upc");
        page.duration = value["duration"].as_u64().and_then(|ms| ms.try_into().ok());
        if kind == ObjectKind::Track {
            page.tracks.push(playlist::track(value, id, None));
        }
    }
    Some(page)
}
