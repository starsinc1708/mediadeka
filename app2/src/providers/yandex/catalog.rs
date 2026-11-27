use std::sync::atomic::AtomicBool;

use serde_json::Value;
use url::form_urlencoded::Serializer;

use crate::{
    music::{ArtistDetails, MusicPage, ObjectKind, ObjectRef},
    playlist::{ArtistCredit, Media, PlaylistTrack},
    providers::{Credentials, HomeItem, ProviderError, catalog, transport::*},
};

use super::{
    API, PROVIDER, get,
    landing::{cover, valid_uuid},
    numeric_id, playlists,
};

mod tag;

pub(crate) use tag::{tag_slug, tag_url};

pub(in crate::providers) fn read(
    reference: &ObjectRef,
    credentials: Option<&Credentials>,
    cancelled: &AtomicBool,
) -> Result<MusicPage, ProviderError> {
    let id = &reference.id;
    if reference.kind == ObjectKind::Collection {
        return tag::read(reference, credentials, cancelled);
    }
    let path = match reference.kind {
        ObjectKind::Track if playlists::valid_track_id(id) => format!("/tracks/{id}"),
        ObjectKind::Album if digits(id) => format!("/albums/{id}/with-tracks"),
        ObjectKind::Artist if digits(id) => format!("/artists/{id}/brief-info"),
        _ => return Err(catalog::invalid_id(PROVIDER)),
    };
    let response = request(&agent(), &path, credentials)?;
    check_cancelled(cancelled)?;
    let result = &response["result"];
    let mut page = match reference.kind {
        ObjectKind::Track => track(&result[0], Some(id)),
        ObjectKind::Album => album(result),
        ObjectKind::Artist => artist(&result["artist"]),
        ObjectKind::Playlist | ObjectKind::Collection | ObjectKind::Station => None,
    }
    .filter(|page| same_id(reference, &page.reference))
    .ok_or_else(|| invalid_response(PROVIDER))?;
    if reference.kind == ObjectKind::Artist {
        let mut details = ArtistDetails {
            monthly_listeners: result["stats"]["lastMonthListeners"].as_u64(),
            biography: field(&result["artist"]["description"], "text"),
            related: ArtistDetails::related_preview(
                reference,
                result["similarArtists"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(artist)
                    .map(|artist| HomeItem {
                        id: format!("artist:{}", artist.reference.id),
                        title: artist.title,
                        subtitle: String::new(),
                        url: artist.url,
                        artwork: artist.artwork,
                    }),
            ),
            ..Default::default()
        };
        page.releases = result["albums"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(album)
            .map(|release| {
                let single = release.single;
                let item = release_item(release);
                details.release_is_single.insert(item.id.clone(), single);
                item
            })
            .collect();
        page.tracks = tracks(result["popularTracks"].as_array().into_iter().flatten());
        page.artist_details = Some(details);
    }
    Ok(page)
}

pub(in crate::providers) fn search(
    credentials: &Credentials,
    kind: ObjectKind,
    query: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<MusicPage>, ProviderError> {
    let (entity, collection) = match kind {
        ObjectKind::Track => ("track", "tracks"),
        ObjectKind::Album => ("album", "albums"),
        ObjectKind::Artist => ("artist", "artists"),
        ObjectKind::Playlist => ("playlist", "playlists"),
        ObjectKind::Collection | ObjectKind::Station => return Err(catalog::invalid_id(PROVIDER)),
    };
    let params = Serializer::new(String::new())
        .append_pair("text", query)
        .append_pair("type", entity)
        .append_pair("page", "0")
        .finish();
    let result = request(&agent(), &format!("/search?{params}"), Some(credentials))?;
    check_cancelled(cancelled)?;
    let group = &result["result"][collection];
    if group.is_null() && result["result"].is_object() {
        return Ok(Vec::new());
    }
    Ok(group["results"]
        .as_array()
        .ok_or_else(|| invalid_response(PROVIDER))?
        .iter()
        .filter_map(|item| match kind {
            ObjectKind::Track => track(item, None),
            ObjectKind::Album => album(item),
            ObjectKind::Artist => artist(item),
            ObjectKind::Playlist => playlist(item),
            ObjectKind::Collection | ObjectKind::Station => None,
        })
        .take(catalog::SEARCH_LIMIT)
        .collect())
}

fn request(
    agent: &ureq::Agent,
    path: &str,
    credentials: Option<&Credentials>,
) -> Result<Value, ProviderError> {
    match credentials {
        Some(Credentials::YandexMusic { access_token, .. }) => get(agent, path, access_token),
        None => decode(PROVIDER, agent.get(&format!("{API}{path}")).call()),
        _ => Err(catalog::missing_access(PROVIDER)),
    }
}

fn same_id(requested: &ObjectRef, found: &ObjectRef) -> bool {
    if requested.kind == ObjectKind::Track && !requested.id.contains(':') {
        return found.id.split(':').next() == Some(requested.id.as_str());
    }
    requested == found
}

fn digits(id: &str) -> bool {
    !id.contains(':') && playlists::valid_track_id(id)
}

fn page(id: String, kind: ObjectKind, title: String) -> MusicPage {
    let reference = ObjectRef {
        provider: PROVIDER,
        kind,
        id,
    };
    let url = reference.url();
    let mut page = MusicPage::new(reference, title);
    page.url = url;
    page
}

fn track(value: &Value, requested_id: Option<&str>) -> Option<MusicPage> {
    let track_id = numeric_id(&value["id"])?;
    let album_id = requested_id.and_then(|id| id.split_once(':').map(|(_, album)| album));
    let mut albums = value["albums"].as_array().into_iter().flatten();
    let album = match album_id {
        Some(id) => albums.find(|album| numeric_id(&album["id"]).as_deref() == Some(id)),
        None => albums.next(),
    };
    let id = album
        .and_then(|album| numeric_id(&album["id"]))
        .map_or_else(|| track_id.clone(), |album| format!("{track_id}:{album}"));
    let mut page = page(id, ObjectKind::Track, field(value, "title")?);
    page.artwork = cover(value).or_else(|| album.and_then(cover));
    page.album = album.and_then(|album| field(album, "title"));
    page.album_ref = album
        .and_then(|album| numeric_id(&album["id"]))
        .and_then(|id| ObjectRef::native(PROVIDER, ObjectKind::Album, id));
    page.year = album.and_then(year);
    page.duration = value["durationMs"]
        .as_u64()
        .and_then(|ms| ms.try_into().ok());
    page.version = field(value, "version");
    page.isrc = field(value, "isrc");
    let mut row = playlists::track(value, 0);
    page.artists.clone_from(&row.artists);
    page.artist_credits.clone_from(&row.artist_credits);
    row.album.clone_from(&page.album);
    row.album_ref.clone_from(&page.album_ref);
    if row.id != page.reference.id {
        row.id.clone_from(&page.reference.id);
        if let Some(Media::Provider { id, .. }) = &mut row.media {
            id.clone_from(&page.reference.id);
        }
    }
    page.tracks.push(row);
    Some(page)
}

pub(super) fn album(value: &Value) -> Option<MusicPage> {
    if value["metaType"]
        .as_str()
        .is_some_and(|kind| kind != "music")
    {
        return None;
    }
    let mut page = page(
        numeric_id(&value["id"])?,
        ObjectKind::Album,
        field(value, "title")?,
    );
    page.artist_credits = artist_credits(value).collect();
    page.artists = page
        .artist_credits
        .iter()
        .map(|credit| credit.name.clone())
        .collect();
    page.artwork = cover(value);
    page.year = year(value);
    page.version = field(value, "version");
    page.barcode = field(value, "upc").or_else(|| field(value, "ean"));
    page.single = [&value["type"], &value["albumType"]]
        .into_iter()
        .any(|kind| kind == "single");
    page.tracks = tracks(
        value["volumes"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|disc| disc.as_array().into_iter().flatten()),
    );
    page.fill_missing_track_albums();
    Some(page)
}

pub(super) fn artist(value: &Value) -> Option<MusicPage> {
    let mut page = page(
        numeric_id(&value["id"])?,
        ObjectKind::Artist,
        field(value, "name")?,
    );
    page.artwork = cover(value);
    Some(page)
}

fn playlist(value: &Value) -> Option<MusicPage> {
    let id = field(value, "playlistUuid")
        .filter(|id| valid_uuid(id))
        .or_else(|| user_playlist_id(value))?;
    let mut page = page(id, ObjectKind::Playlist, field(value, "title")?);
    page.artwork = cover(value);
    page.artists = field(&value["owner"], "name").into_iter().collect();
    Some(page)
}

fn user_playlist_id(value: &Value) -> Option<String> {
    let owner = numeric_id(&value["uid"]).or_else(|| numeric_id(&value["owner"]["uid"]))?;
    let kind = numeric_id(&value["kind"])?;
    Some(format!("{owner}:{kind}"))
}

fn year(value: &Value) -> Option<u32> {
    value["year"].as_u64()?.try_into().ok()
}

pub(super) fn artist_credits(value: &Value) -> impl Iterator<Item = ArtistCredit> {
    value["artists"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|artist| {
            let name = field(artist, "name")?;
            Some(ArtistCredit::from_catalog(
                name,
                PROVIDER,
                numeric_id(&artist["id"]),
            ))
        })
}

fn tracks<'a>(values: impl Iterator<Item = &'a Value>) -> Vec<PlaylistTrack> {
    values
        .enumerate()
        .map(|(position, entry)| playlists::track(entry, position))
        .collect()
}

fn release_item(page: MusicPage) -> HomeItem {
    let format = if page.single {
        "Сингл"
    } else {
        "Альбом"
    };
    HomeItem {
        id: format!("album:{}", page.reference.id),
        title: page.title,
        subtitle: page
            .year
            .map_or_else(|| format.to_owned(), |year| format!("{year} · {format}")),
        artwork: page.artwork,
        url: page.url,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yandex_track_links_preserve_requested_release_and_partial_artist_credits() {
        let recording = serde_json::json!({
            "id": 10,
            "title": "Recording",
            "artists": [
                {"id": 15, "name": "Lead & Band"},
                {"name": "Unlinked guest"},
                {"id": 17, "name": "Second guest"}
            ],
            "albums": [{"id": 70, "title": "First edition"}, {"id": 73, "title": "Chosen edition"}]
        });
        let page = track(&recording, Some("10:73")).unwrap();
        assert_eq!(page.album.as_deref(), Some("Chosen edition"));
        assert_eq!(page.album_ref.as_ref().unwrap().id, "73");
        assert_eq!(page.artist_credits[0].reference.as_ref().unwrap().id, "15");
        assert_eq!(page.artist_credits[1].name, "Unlinked guest");
        assert!(page.artist_credits[1].reference.is_none());
        assert_eq!(page.artist_credits[2].reference.as_ref().unwrap().id, "17");
        assert_eq!(page.artist_credits, page.tracks[0].artist_credits);
        assert_eq!(page.album_ref, page.tracks[0].album_ref);

        let playlist_row =
            playlists::track(&serde_json::json!({"track": recording, "albumId": 99}), 0);
        assert_eq!(playlist_row.album_ref.unwrap().id, "99");
        assert!(playlist_row.album.is_none());
    }

    #[test]
    #[ignore = "public Yandex Music catalog"]
    fn yandex_reads_public_album_and_recording() {
        let cancelled = AtomicBool::new(false);
        let reference = |kind, id: &str| ObjectRef {
            provider: PROVIDER,
            kind,
            id: id.to_owned(),
        };
        let album = read(&reference(ObjectKind::Album, "573515"), None, &cancelled).unwrap();
        assert!(!album.title.is_empty());
        assert!(!album.tracks.is_empty());
        let recording = read(&reference(ObjectKind::Track, "756442"), None, &cancelled).unwrap();
        assert_eq!(recording.title, "Rocket Man");
        assert!(recording.duration.is_some());
        let credit = recording
            .artist_credits
            .iter()
            .find(|credit| credit.reference.is_some())
            .expect("the public recording supplies at least one artist ID");
        let artist = read(credit.reference.as_ref().unwrap(), None, &cancelled).unwrap();
        assert_eq!(artist.title, credit.name);
        let release = read(recording.album_ref.as_ref().unwrap(), None, &cancelled).unwrap();
        assert_eq!(recording.album.as_deref(), Some(release.title.as_str()));
    }
}
