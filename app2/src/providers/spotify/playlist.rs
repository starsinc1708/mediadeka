use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use librespot_core::{Error, Session, SpotifyId, SpotifyUri, error::ErrorKind as SpotifyError};
use librespot_protocol::{
    extension_kind::ExtensionKind,
    metadata::Track,
    playlist4_external::{Item, ListAttributes, SelectedListContent},
};
use protobuf::Message;
use tokio::time::timeout;
use ureq::http::Method;

use crate::providers::{
    ErrorKind, HomeItem, ProviderError,
    playlists::{PlaylistPage, PlaylistProgress, unavailable_track},
    transport::*,
};
use crate::{
    music::{ObjectKind, ObjectRef},
    playlist::{Media, Playlist, PlaylistOwner, PlaylistTrack},
};

use super::{
    PROVIDER,
    catalog::{artist_credits, catalog_gid},
    metadata,
};

const PAGE_SIZE: usize = 100;
const PAGE_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) async fn library_cards(
    session: &Session,
    limit: Option<usize>,
) -> Result<Vec<HomeItem>, ProviderError> {
    let mut entries = Vec::new();
    let mut offset = 0;
    let mut seen = HashSet::new();
    for _ in 0..crate::providers::collections::MAX_PAGES {
        let bytes = timeout(
            PAGE_TIMEOUT,
            session.spclient().get_rootlist(offset, Some(PAGE_SIZE)),
        )
        .await
        .map_err(|_| timed_out())?
        .map_err(service_error)?;
        let list = SelectedListContent::parse_from_bytes(&bytes)
            .map_err(|_| invalid_response(PROVIDER))?;
        let playlist_cards = list
            .contents
            .items
            .iter()
            .zip(&list.contents.meta_items)
            .filter_map(|(entry, meta)| {
                let SpotifyUri::Playlist { id, .. } = SpotifyUri::from_uri(entry.uri()).ok()?
                else {
                    return None;
                };
                let id = id.to_base62();
                let title = nonempty(meta.attributes.name())?;
                Some(HomeItem {
                    id: format!("spotify:playlist:{id}"),
                    title,
                    subtitle: String::new(),
                    url: Some(format!("https://open.spotify.com/playlist/{id}")),
                    artwork: playlist_artwork(&meta.attributes),
                })
            });
        let remaining = limit.map_or(usize::MAX, |limit| limit.saturating_sub(entries.len()));
        entries.extend(
            playlist_cards
                .filter(|card| seen.insert(card.id.clone()))
                .take(remaining),
        );
        let count = list.contents.items.len();
        offset += count;
        if limit == Some(entries.len())
            || (!list.contents.truncated()
                && usize::try_from(list.length()).is_ok_and(|length| offset >= length))
        {
            return Ok(entries);
        }
        if count == 0 {
            return Err(invalid_response(PROVIDER));
        }
    }
    Err(invalid_response(PROVIDER))
}

pub(super) async fn load(session: &Session, id: &str) -> Result<Playlist, ProviderError> {
    load_with_progress(session, id, &mut |_| Ok(())).await
}

pub(super) async fn load_with_progress(
    session: &Session,
    id: &str,
    progress: &mut PlaylistProgress<'_>,
) -> Result<Playlist, ProviderError> {
    let playlist_id = SpotifyId::from_base62(id).map_err(|_| invalid_response(PROVIDER))?;
    let mut first = page(session, &playlist_id, 0).await?;
    let revision = first.revision().to_vec();
    let mut list = Playlist {
        name: nonempty(first.attributes.name()).ok_or_else(|| invalid_response(PROVIDER))?,
        owner: nonempty(first.owner_username()).map(PlaylistOwner::Named),
        artwork: playlist_artwork(&first.attributes),
        link: Some(format!("https://open.spotify.com/playlist/{id}")),
        tracks: Vec::new(),
    };
    let mut metadata = HashMap::new();
    loop {
        let offset = list.tracks.len();
        let total = usize::try_from(first.length()).map_err(|_| invalid_response(PROVIDER))?;
        let content = first
            .contents
            .take()
            .ok_or_else(|| invalid_response(PROVIDER))?;
        let next = offset + content.items.len();
        if next > total || (next == offset && (content.truncated() || next < total)) {
            return Err(invalid_response(PROVIDER));
        }
        metadata_for_page(session, content.items.iter().map(Item::uri), &mut metadata).await?;
        list.tracks.extend(
            content
                .items
                .iter()
                .map(|entry| playlist_track(entry, &metadata)),
        );
        if !content.truncated() && next == total {
            break;
        }
        if next < total && next > offset {
            progress(PlaylistPage {
                playlist: &list,
                tracks: &list.tracks[offset..],
                offset,
                total: Some(total),
            })?;
        }
        first = page(session, &playlist_id, next).await?;
        if revision.is_empty()
            || first.revision() != revision
            || usize::try_from(first.length()) != Ok(total)
        {
            return Err(ProviderError::new(
                ErrorKind::InvalidResponse,
                "Плейлист Spotify изменился во время загрузки. Обновите страницу.",
            ));
        }
    }
    Ok(list)
}

async fn page(
    session: &Session,
    id: &SpotifyId,
    offset: usize,
) -> Result<SelectedListContent, ProviderError> {
    let endpoint = format!(
        "/playlist/v2/playlist/{}?decorate=revision,attributes,length,owner&from={offset}&length={PAGE_SIZE}",
        id.to_base62()
    );
    let bytes = timeout(
        PAGE_TIMEOUT,
        session
            .spclient()
            .request(&Method::GET, &endpoint, None, None),
    )
    .await
    .map_err(|_| timed_out())?
    .map_err(service_error)?;
    SelectedListContent::parse_from_bytes(&bytes).map_err(|_| invalid_response(PROVIDER))
}

pub(super) async fn metadata_for_page<'a>(
    session: &Session,
    uris: impl Iterator<Item = &'a str>,
    cache: &mut HashMap<String, Option<PlaylistTrack>>,
) -> Result<(), ProviderError> {
    let missing = uris
        .filter(|uri| !cache.contains_key(*uri) && track_id(uri).is_some())
        .collect::<HashSet<_>>();
    if missing.is_empty() {
        return Ok(());
    }
    let mut rows = metadata::read_batch(session, missing.iter().copied(), ExtensionKind::TRACK_V4)
        .await?
        .filter_map(|entry| {
            let track = Track::parse_from_bytes(&entry.extension_data.as_ref()?.value).ok()?;
            Some((
                entry.entity_uri.clone(),
                metadata_track(&entry.entity_uri, &track)?,
            ))
        })
        .collect::<HashMap<_, _>>();
    for uri in missing {
        cache.insert(uri.to_owned(), rows.remove(uri));
    }
    Ok(())
}

fn playlist_track(
    entry: &Item,
    metadata: &HashMap<String, Option<PlaylistTrack>>,
) -> PlaylistTrack {
    let added = chrono::DateTime::from_timestamp_millis(entry.attributes.timestamp())
        .filter(|_| entry.attributes.timestamp() > 0)
        .map(|date| date.to_rfc3339());
    let track = metadata
        .get(entry.uri())
        .and_then(Option::as_ref)
        .cloned()
        .unwrap_or_else(|| unavailable_recording(entry.uri()));
    PlaylistTrack { added, ..track }
}

pub(super) fn track_id(uri: &str) -> Option<String> {
    match SpotifyUri::from_uri(uri).ok()? {
        SpotifyUri::Track { id } => Some(id.to_base62()),
        _ => None,
    }
}

pub(super) fn unavailable_recording(uri: &str) -> PlaylistTrack {
    unavailable_track(track_id(uri).unwrap_or_else(|| uri.to_owned()))
}

pub(super) fn metadata_track(uri: &str, track: &Track) -> Option<PlaylistTrack> {
    let id = track_id(uri)?;
    let playable = (!track.file.is_empty() || !track.alternative.is_empty())
        .then(|| track_id(track.canonical_uri()).unwrap_or_else(|| id.clone()));
    let album_art = track
        .album
        .cover_group
        .image
        .iter()
        .chain(&track.album.cover)
        .find_map(|image| cdn_image(image.file_id()));
    let artist_credits = artist_credits(&track.artist).collect::<Vec<_>>();
    let artists: Vec<_> = artist_credits
        .iter()
        .map(|credit| credit.name.clone())
        .collect();
    Some(PlaylistTrack {
        id,
        title: nonempty(track.name())?,
        artist: artists.join(", "),
        artists,
        artist_credits,
        album_ref: catalog_gid(track.album.gid())
            .and_then(|id| ObjectRef::native(PROVIDER, ObjectKind::Album, id)),
        album: nonempty(track.album.name()),
        artwork: album_art,
        added: None,
        duration: u32::try_from(track.duration())
            .ok()
            .filter(|ms| *ms > 0)
            .map(|ms| ms / 1000),
        media: playable.map(|id| Media::Provider {
            provider: PROVIDER,
            id,
        }),
    })
}

fn nonempty(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn playlist_artwork(attributes: &ListAttributes) -> Option<String> {
    attributes
        .picture_size
        .iter()
        .find_map(|image| artwork_url(PROVIDER, image.url()))
        .or_else(|| cdn_image(attributes.picture()))
}

pub(super) fn cdn_image(bytes: &[u8]) -> Option<String> {
    use std::fmt::Write;
    if bytes.len() != 20 || bytes.iter().all(|byte| *byte == 0) {
        return None;
    }
    let mut url = String::with_capacity(64);
    url.push_str("https://i.scdn.co/image/");
    for byte in bytes {
        write!(url, "{byte:02x}").expect("writing to a String is infallible");
    }
    Some(url)
}

pub(super) fn timed_out() -> ProviderError {
    ProviderError::new(
        ErrorKind::Network,
        "Spotify не ответил при чтении плейлиста.",
    )
}

pub(super) fn service_error(error: Error) -> ProviderError {
    let (kind, text) = match error.kind {
        SpotifyError::Unauthenticated => (ErrorKind::Expired, "Подключите Spotify заново."),
        SpotifyError::PermissionDenied | SpotifyError::NotFound => (
            ErrorKind::Rejected,
            "Spotify не дал доступ к этому плейлисту.",
        ),
        SpotifyError::ResourceExhausted => (
            ErrorKind::Network,
            "Spotify временно ограничил чтение плейлистов.",
        ),
        _ => (ErrorKind::Network, "Не удалось прочитать плейлист Spotify."),
    };
    ProviderError::new(kind, text)
}
