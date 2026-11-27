use std::{
    collections::{HashMap, HashSet},
    sync::atomic::AtomicBool,
    time::Duration,
};

use chrono::Datelike;
use librespot_core::Session;
use protobuf::{CodedInputStream, CodedOutputStream, rt::skip_field_for_tag};
use tokio::time::timeout;
use ureq::http::{
    HeaderMap, HeaderValue, Method,
    header::{ACCEPT, CONTENT_TYPE},
};

use crate::{
    playlist::PlaylistTrack,
    providers::{
        Credentials, ErrorKind, Profile, ProviderError,
        collections::{Collection, CollectionKind, MAX_PAGES},
        transport::invalid_response,
    },
};

use super::{
    PROVIDER,
    librespot::with_session,
    playlist::{library_cards, metadata_for_page, track_id, unavailable_recording},
};

mod saved;

const COLLECTION_PATH: &str = "/collection/v2/paging";
const CONTENT: &str = "application/vnd.collection-v2.spotify.proto";
const COLLECTION_SET: &str = "collection";
const ARTIST_SET: &str = "artist";
const PAGE_SIZE: i32 = 300;
const METADATA_BATCH: usize = 30;
const PAGE_TIMEOUT: Duration = Duration::from_secs(15);
const SPOTIFY_FOUNDING_YEAR: i32 = 2006;

pub(in crate::providers) fn read_collection(
    credentials: &Credentials,
    profile: &Profile,
    kind: CollectionKind,
    cancelled: &AtomicBool,
) -> Result<Collection, ProviderError> {
    let Credentials::Spotify {
        client_id,
        librespot: Some(saved),
        ..
    } = credentials
    else {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Подключите Spotify заново.",
        ));
    };
    if saved.username != profile.id {
        return Err(ProviderError::new(
            ErrorKind::Expired,
            "Подключите нужный профиль Spotify.",
        ));
    }
    with_session(client_id, saved, cancelled, async |session| match kind {
        CollectionKind::Liked => liked(session).await.map(Collection::Liked),
        CollectionKind::Albums => saved::albums(session).await.map(Collection::Albums),
        CollectionKind::Artists => saved::artists(session).await.map(Collection::Artists),
        CollectionKind::Playlists => library_cards(session, None)
            .await
            .map(Collection::Playlists),
    })
}

async fn liked(session: &Session) -> Result<Vec<PlaylistTrack>, ProviderError> {
    let entries = entries(session, COLLECTION_SET, |uri| track_id(uri).is_some()).await?;
    let mut rows = Vec::new();
    for batch in entries.chunks(METADATA_BATCH) {
        let mut metadata = HashMap::new();
        metadata_for_page(
            session,
            batch.iter().map(|entry| entry.uri.as_str()),
            &mut metadata,
        )
        .await?;
        rows.extend(batch.iter().map(|entry| {
            let track = metadata
                .remove(&entry.uri)
                .flatten()
                .unwrap_or_else(|| unavailable_recording(&entry.uri));
            PlaylistTrack {
                added: entry.added(),
                ..track
            }
        }));
    }
    Ok(rows)
}

async fn entries(
    session: &Session,
    set: &str,
    accepts: impl Fn(&str) -> bool,
) -> Result<Vec<Entry>, ProviderError> {
    let mut entries = Vec::new();
    let mut seen_uris = HashSet::new();
    let mut seen_tokens = HashSet::new();
    let mut revision = None;
    let mut token = String::new();
    for _ in 0..MAX_PAGES {
        let result = page(session, set, &token).await?;
        if !result.sync_token.is_empty() {
            if revision
                .as_ref()
                .is_some_and(|previous| previous != &result.sync_token)
            {
                return Err(ProviderError::new(
                    ErrorKind::InvalidResponse,
                    "Коллекция Spotify изменилась во время загрузки. Повторите запрос.",
                ));
            }
            revision = Some(result.sync_token);
        }
        entries.extend(result.items.into_iter().filter(|entry| {
            !entry.is_removed && accepts(&entry.uri) && seen_uris.insert(entry.uri.clone())
        }));
        token = result.next_page_token;
        if token.is_empty() {
            return Ok(entries);
        }
        if !seen_tokens.insert(token.clone()) {
            return Err(invalid_response(PROVIDER));
        }
    }
    Err(invalid_response(PROVIDER))
}

async fn page(session: &Session, set: &str, cursor: &str) -> Result<Page, ProviderError> {
    use librespot_core::error::ErrorKind as SpotifyError;

    let payload =
        encode_request(&session.username(), set, cursor).map_err(|_| invalid_response(PROVIDER))?;
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(CONTENT));
    headers.insert(ACCEPT, HeaderValue::from_static(CONTENT));
    let bytes = timeout(
        PAGE_TIMEOUT,
        session.spclient().request(
            &Method::POST,
            COLLECTION_PATH,
            Some(headers),
            Some(&payload),
        ),
    )
    .await
    .map_err(|_| {
        ProviderError::new(
            ErrorKind::Network,
            "Spotify не ответил на запрос коллекции.",
        )
    })?
    .map_err(|error| match error.kind {
        SpotifyError::Unauthenticated => {
            ProviderError::new(ErrorKind::Expired, "Подключите Spotify заново.")
        }
        SpotifyError::PermissionDenied | SpotifyError::NotFound => {
            ProviderError::new(ErrorKind::Rejected, "Коллекция Spotify недоступна.")
        }
        SpotifyError::ResourceExhausted => {
            ProviderError::new(ErrorKind::Network, "Spotify ограничил чтение коллекции.")
        }
        _ => ProviderError::new(
            ErrorKind::Network,
            "Не удалось прочитать коллекцию Spotify.",
        ),
    })?;
    decode_page(&bytes).map_err(|_| invalid_response(PROVIDER))
}

// The pinned librespot contains collection2v2.proto but does not generate its Rust module.
#[derive(Default)]
struct Page {
    items: Vec<Entry>,
    next_page_token: String,
    sync_token: String,
}

#[derive(Default)]
struct Entry {
    uri: String,
    added_at: i32,
    is_removed: bool,
}

impl Entry {
    fn added(&self) -> Option<String> {
        chrono::DateTime::from_timestamp(i64::from(self.added_at), 0)
            .filter(|date| date.year() >= SPOTIFY_FOUNDING_YEAR)
            .map(|date| date.to_rfc3339())
    }
}

fn encode_request(username: &str, set: &str, cursor: &str) -> protobuf::Result<Vec<u8>> {
    let mut payload = Vec::new();
    let mut writer = CodedOutputStream::vec(&mut payload);
    writer.write_string(1, username)?;
    writer.write_string(2, set)?;
    writer.write_string(3, cursor)?;
    writer.write_int32(4, PAGE_SIZE)?;
    writer.flush()?;
    drop(writer);
    Ok(payload)
}

fn decode_page(bytes: &[u8]) -> protobuf::Result<Page> {
    let mut page = Page::default();
    let mut reader = CodedInputStream::from_bytes(bytes);
    while let Some(tag) = reader.read_raw_tag_or_eof()? {
        match tag {
            10 => page.items.push(decode_entry(&reader.read_bytes()?)?),
            18 => page.next_page_token = reader.read_string()?,
            26 => page.sync_token = reader.read_string()?,
            _ => skip_field_for_tag(tag, &mut reader)?,
        }
    }
    Ok(page)
}

fn decode_entry(bytes: &[u8]) -> protobuf::Result<Entry> {
    let mut entry = Entry::default();
    let mut reader = CodedInputStream::from_bytes(bytes);
    while let Some(tag) = reader.read_raw_tag_or_eof()? {
        match tag {
            10 => entry.uri = reader.read_string()?,
            16 => entry.added_at = reader.read_int32()?,
            24 => entry.is_removed = reader.read_bool()?,
            _ => skip_field_for_tag(tag, &mut reader)?,
        }
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spotify_collection_ignores_dates_before_spotify_existed() {
        let mut entry = Entry {
            added_at: 882_057_600,
            ..Entry::default()
        };
        assert!(entry.added().is_none());
        entry.added_at = 1_700_000_000;
        assert!(entry.added().is_some());
    }

    #[test]
    fn collection_wire_retains_cursor_revision_and_removal() {
        let mut entry = Vec::new();
        {
            let mut writer = CodedOutputStream::vec(&mut entry);
            writer
                .write_string(1, "spotify:track:75UGzVK6Obcgg4TToPEJHk")
                .unwrap();
            writer.write_int32(2, 42).unwrap();
            writer.write_bool(3, true).unwrap();
            writer.write_string(4, "ignored context").unwrap();
            writer.flush().unwrap();
        }
        let mut page = Vec::new();
        {
            let mut writer = CodedOutputStream::vec(&mut page);
            writer.write_bytes(1, &entry).unwrap();
            writer.write_string(2, "next").unwrap();
            writer.write_string(3, "revision").unwrap();
            writer.flush().unwrap();
        }
        let parsed = decode_page(&page).unwrap();
        assert_eq!(parsed.next_page_token, "next");
        assert_eq!(parsed.sync_token, "revision");
        assert_eq!(parsed.items[0].added_at, 42);
        assert!(parsed.items[0].is_removed);
        assert_eq!(
            track_id(&parsed.items[0].uri).as_deref(),
            Some("75UGzVK6Obcgg4TToPEJHk")
        );
        assert!(decode_page(&page[..page.len() - 2]).is_err());
    }
}
