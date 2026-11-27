use std::{
    collections::{HashMap, HashSet},
    sync::atomic::AtomicBool,
    time::Duration,
};

use librespot_core::{Session, SpotifyId, SpotifyUri};
use librespot_protocol::metadata::{Album, Artist, ExternalId, Track, album};
use protobuf::Message;
use serde_json::Value;
use tokio::time::timeout;

use crate::{
    music::{ArtistDetails, MusicPage, ObjectKind, ObjectRef},
    playlist::{ArtistCredit, PlaylistTrack},
    providers::{Credentials, ErrorKind, HomeItem, ProviderError, catalog, transport::*},
};

use super::{PROVIDER, librespot::with_session, pathfinder, playlist};

const METADATA_TIMEOUT: Duration = Duration::from_secs(12);
const METADATA_BATCH: usize = 50;
const ARTIST_RELEASE_TIMEOUT: Duration = Duration::from_secs(5);

pub(in crate::providers) fn read(
    reference: &ObjectRef,
    credentials: &Credentials,
    cancelled: &AtomicBool,
) -> Result<MusicPage, ProviderError> {
    if !valid_id(&reference.id) {
        return Err(catalog::invalid_id(PROVIDER));
    }
    let Credentials::Spotify {
        client_id,
        librespot: Some(saved),
        ..
    } = credentials
    else {
        return Err(catalog::missing_access(PROVIDER));
    };
    with_session(client_id, saved, cancelled, async |session| {
        if reference.kind == ObjectKind::Collection {
            let collection = pathfinder::collection(session, &reference.id).await?;
            let mut page = MusicPage::new(reference.clone(), collection.title);
            page.releases = collection.items;
            return Ok(page);
        }
        let uri = uri(reference);
        let parsed = SpotifyUri::from_uri(&uri).map_err(|_| catalog::invalid_id(PROVIDER))?;
        timeout(METADATA_TIMEOUT, async {
            match reference.kind {
                ObjectKind::Track => read_track(session, reference, &parsed).await,
                ObjectKind::Album => read_album(session, reference, &parsed).await,
                ObjectKind::Artist => read_artist(session, reference, &parsed).await,
                _ => Err(catalog::invalid_id(PROVIDER)),
            }
        })
        .await
        .map_err(|_| {
            ProviderError::new(ErrorKind::Network, "Spotify не ответил на запрос карточки.")
        })?
    })
}

pub(in crate::providers) fn search(
    credentials: &Credentials,
    kind: ObjectKind,
    term: &str,
    cancelled: &AtomicBool,
) -> Result<Vec<MusicPage>, ProviderError> {
    let Credentials::Spotify {
        client_id,
        librespot: Some(saved),
        ..
    } = credentials
    else {
        return Err(catalog::missing_access(PROVIDER));
    };
    let key = match kind {
        ObjectKind::Track => "tracksV2",
        ObjectKind::Album => "albumsV2",
        ObjectKind::Artist => "artists",
        ObjectKind::Playlist => "playlists",
        ObjectKind::Collection | ObjectKind::Station => return Err(catalog::invalid_id(PROVIDER)),
    };
    with_session(client_id, saved, cancelled, async |session| {
        let result = pathfinder::search(session, term).await?;
        let list = &result["searchV2"][key]["items"];
        Ok(list
            .as_array()
            .ok_or_else(|| invalid_response(PROVIDER))?
            .iter()
            .filter_map(|entry| search_entry(entry, kind))
            .take(catalog::SEARCH_LIMIT)
            .collect())
    })
}

async fn read_track(
    session: &Session,
    reference: &ObjectRef,
    uri: &SpotifyUri,
) -> Result<MusicPage, ProviderError> {
    let bytes = session
        .spclient()
        .get_track_metadata(uri)
        .await
        .map_err(playlist::service_error)?;
    decode_track_metadata(reference, uri, &bytes)
}

pub(in crate::providers) fn decode_track_metadata(
    reference: &ObjectRef,
    uri: &SpotifyUri,
    bytes: &[u8],
) -> Result<MusicPage, ProviderError> {
    let track = Track::parse_from_bytes(bytes).map_err(|_| invalid_response(PROVIDER))?;
    let row = playlist::metadata_track(&uri.to_uri(), &track)
        .ok_or_else(|| invalid_response(PROVIDER))?;
    let mut page = MusicPage::new(reference.clone(), row.title.clone());
    page.artists.clone_from(&row.artists);
    page.artist_credits.clone_from(&row.artist_credits);
    page.artwork = row.artwork.clone();
    page.album = row.album.clone();
    page.album_ref.clone_from(&row.album_ref);
    page.year = track
        .album
        .date
        .as_ref()
        .and_then(|date| positive(date.year()));
    page.duration = positive(track.duration());
    page.version = nonempty(track.version_title());
    page.isrc = external_id(&track.external_id, &["isrc"]);
    page.tracks.push(row);
    Ok(page)
}

async fn read_album(
    session: &Session,
    reference: &ObjectRef,
    uri: &SpotifyUri,
) -> Result<MusicPage, ProviderError> {
    let bytes = session
        .spclient()
        .get_album_metadata(uri)
        .await
        .map_err(playlist::service_error)?;
    let album = Album::parse_from_bytes(&bytes).map_err(|_| invalid_response(PROVIDER))?;
    let mut page = MusicPage::new(
        reference.clone(),
        nonempty(album.name()).ok_or_else(|| invalid_response(PROVIDER))?,
    );
    page.artist_credits = artist_credits(&album.artist).collect();
    page.artists = page
        .artist_credits
        .iter()
        .map(|credit| credit.name.clone())
        .collect();
    page.artwork = album_artwork(&album);
    page.year = album_year(&album);
    page.version = nonempty(album.version_title());
    page.barcode = external_id(&album.external_id, &["upc", "ean"]);
    page.single = album.type_() == album::Type::SINGLE;
    let uris = album
        .disc
        .iter()
        .flat_map(|disc| &disc.track)
        .filter_map(track_uri)
        .collect::<Vec<_>>();
    page.tracks = read_tracks(session, &uris).await?;
    page.fill_missing_track_albums();
    Ok(page)
}

async fn read_artist(
    session: &Session,
    reference: &ObjectRef,
    uri: &SpotifyUri,
) -> Result<MusicPage, ProviderError> {
    let bytes = session
        .spclient()
        .get_artist_metadata(uri)
        .await
        .map_err(playlist::service_error)?;
    let artist = Artist::parse_from_bytes(&bytes).map_err(|_| invalid_response(PROVIDER))?;
    let mut page = MusicPage::new(
        reference.clone(),
        nonempty(artist.name()).ok_or_else(|| invalid_response(PROVIDER))?,
    );
    page.artwork = artist_artwork(&artist);
    let country = session.country();
    let tracks = artist
        .top_track
        .iter()
        .find(|entry| entry.country() == country)
        .or_else(|| artist.top_track.first());
    let uris: Vec<_> = tracks
        .into_iter()
        .flat_map(|entry| &entry.track)
        .filter_map(track_uri)
        .collect();
    let (tracks, overview) = tokio::join!(
        read_tracks(session, &uris),
        timeout(
            ARTIST_RELEASE_TIMEOUT,
            pathfinder::artist(session, &reference.id)
        ),
    );
    page.tracks = tracks?;
    if let Ok(Ok(data)) = overview {
        let artist_union = &data["artistUnion"];
        let mut details = ArtistDetails {
            banner: artwork_from_images(
                PROVIDER,
                &artist_union["visuals"]["headerImage"]["sources"],
            ),
            monthly_listeners: artist_union["stats"]["monthlyListeners"].as_u64(),
            ..Default::default()
        };
        page.releases = artist_releases(&artist_union["discography"], &page.title, &mut details);
        page.artwork =
            artwork_from_images(PROVIDER, &artist_union["visuals"]["avatarImage"]["sources"])
                .or(page.artwork);
        page.artist_details = Some(details);
    }
    if page.releases.is_empty() {
        page.releases = embedded_artist_releases(&artist, &page.title);
    }
    Ok(page)
}

fn embedded_artist_releases(artist: &Artist, title: &str) -> Vec<HomeItem> {
    let mut seen = HashSet::new();
    artist
        .album_group
        .iter()
        .chain(&artist.single_group)
        .chain(&artist.compilation_group)
        .flat_map(|group| &group.album)
        .filter_map(|album| {
            let id = SpotifyId::from_raw(album.gid()).ok()?.to_base62();
            let reference = ObjectRef {
                provider: PROVIDER,
                kind: ObjectKind::Album,
                id,
            };
            Some(HomeItem {
                id: self::uri(&reference),
                title: nonempty(album.name())?,
                subtitle: title.to_owned(),
                url: reference.url(),
                artwork: album_artwork(album),
            })
        })
        .filter(|card| seen.insert(card.id.clone()))
        .collect()
}

fn artist_releases(discography: &Value, title: &str, details: &mut ArtistDetails) -> Vec<HomeItem> {
    let mut seen = HashSet::new();
    ["albums", "singles", "compilations"]
        .iter()
        .flat_map(|group| discography[group]["items"].as_array().into_iter().flatten())
        .flat_map(|group| group["releases"]["items"].as_array().into_iter().flatten())
        .filter_map(|release| {
            let id = field(release, "id").filter(|id| valid_id(id))?;
            let reference = ObjectRef {
                provider: PROVIDER,
                kind: ObjectKind::Album,
                id,
            };
            let item = HomeItem {
                id: uri(&reference),
                title: field(release, "name")?,
                subtitle: release["date"]["year"].as_u64().map_or_else(
                    || title.to_owned(),
                    |year| {
                        let kind = if release["type"] == "SINGLE" {
                            "Сингл"
                        } else {
                            "Альбом"
                        };
                        format!("{year} · {kind}")
                    },
                ),
                url: reference.url(),
                artwork: artwork_from_images(PROVIDER, &release["coverArt"]["sources"]),
            };
            if let Some(kind) = release["type"].as_str() {
                details
                    .release_is_single
                    .insert(item.id.clone(), kind == "SINGLE");
            }
            Some(item)
        })
        .filter(|card| seen.insert(card.id.clone()))
        .collect()
}

async fn read_tracks(
    session: &Session,
    uris: &[String],
) -> Result<Vec<PlaylistTrack>, ProviderError> {
    let mut cache = HashMap::new();
    for page in uris.chunks(METADATA_BATCH) {
        playlist::metadata_for_page(session, page.iter().map(String::as_str), &mut cache).await?;
    }
    Ok(uris
        .iter()
        .map(|uri| {
            cache
                .get(uri)
                .and_then(Option::as_ref)
                .cloned()
                .unwrap_or_else(|| playlist::unavailable_recording(uri))
        })
        .collect())
}

fn uri(reference: &ObjectRef) -> String {
    format!("spotify:{}:{}", reference.kind.path(), reference.id)
}

fn valid_id(id: &str) -> bool {
    id.len() == 22 && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

fn track_uri(track: &Track) -> Option<String> {
    let id = SpotifyId::from_raw(track.gid()).ok()?.to_base62();
    Some(format!("spotify:track:{id}"))
}

pub(super) fn artists(values: &[Artist]) -> Vec<String> {
    values
        .iter()
        .filter_map(|artist| nonempty(artist.name()))
        .collect()
}

pub(super) fn artist_credits(values: &[Artist]) -> impl Iterator<Item = ArtistCredit> {
    values.iter().filter_map(|artist| {
        let name = nonempty(artist.name())?;
        Some(ArtistCredit::from_catalog(
            name,
            PROVIDER,
            catalog_gid(artist.gid()),
        ))
    })
}

pub(super) fn catalog_gid(bytes: &[u8]) -> Option<String> {
    let id = SpotifyId::from_raw(bytes).ok()?;
    bytes.iter().any(|&byte| byte != 0).then(|| id.to_base62())
}

fn external_id(values: &[ExternalId], kinds: &[&str]) -> Option<String> {
    values
        .iter()
        .find(|id| {
            kinds
                .iter()
                .any(|kind| id.type_().eq_ignore_ascii_case(kind))
        })
        .and_then(|id| nonempty(id.id()))
}

pub(super) fn album_artwork(album: &Album) -> Option<String> {
    album
        .cover_group
        .image
        .iter()
        .chain(&album.cover)
        .find_map(|image| playlist::cdn_image(image.file_id()))
}

pub(super) fn album_year(album: &Album) -> Option<u32> {
    album.date.as_ref().and_then(|date| positive(date.year()))
}

pub(super) fn artist_artwork(artist: &Artist) -> Option<String> {
    artist
        .portrait_group
        .image
        .iter()
        .chain(&artist.portrait)
        .find_map(|image| playlist::cdn_image(image.file_id()))
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn positive(value: i32) -> Option<u32> {
    u32::try_from(value).ok().filter(|value| *value > 0)
}

fn search_entry(entry: &Value, kind: ObjectKind) -> Option<MusicPage> {
    let row = entry.pointer("/item/data").or_else(|| entry.get("data"))?;
    let uri = row["uri"].as_str()?;
    let id = uri
        .strip_prefix("spotify:")?
        .strip_prefix(kind.path())?
        .strip_prefix(':')?;
    if !valid_id(id) {
        return None;
    }
    let title = field(row, "name").or_else(|| field(&row["profile"], "name"))?;
    let mut page = MusicPage::new(
        ObjectRef {
            provider: PROVIDER,
            kind,
            id: id.to_owned(),
        },
        title,
    );
    page.artist_credits = row["artists"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|artist| {
            let name = field(&artist["profile"], "name")?;
            let reference = artist["uri"]
                .as_str()
                .and_then(ObjectRef::from_url)
                .filter(|object| object.provider == PROVIDER && object.kind == ObjectKind::Artist);
            Some(ArtistCredit { name, reference })
        })
        .collect();
    page.artists = page
        .artist_credits
        .iter()
        .map(|credit| credit.name.clone())
        .collect();
    let album = &row["albumOfTrack"];
    page.album = field(album, "name");
    page.album_ref = album["uri"]
        .as_str()
        .and_then(ObjectRef::from_url)
        .filter(|object| object.provider == PROVIDER && object.kind == ObjectKind::Album);
    page.year = row["date"]["year"]
        .as_u64()
        .or_else(|| album["date"]["year"].as_u64())
        .and_then(|year| year.try_into().ok());
    page.single = row["type"]
        .as_str()
        .is_some_and(|value| value.eq_ignore_ascii_case("single"));
    page.duration = row["duration"]["totalMilliseconds"]
        .as_u64()
        .or_else(|| row["trackDuration"]["totalMilliseconds"].as_u64())
        .and_then(|ms| ms.try_into().ok());
    page.artwork = [
        &row["coverArt"]["sources"],
        &album["coverArt"]["sources"],
        &row["visuals"]["avatarImage"]["sources"],
        &row["images"]["items"][0]["sources"],
    ]
    .into_iter()
    .find_map(|images| artwork_from_images(PROVIDER, images));
    Some(page)
}

#[cfg(test)]
mod tests;
