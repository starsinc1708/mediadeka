use std::collections::HashMap;

use librespot_core::{Session, SpotifyUri};
use librespot_protocol::{
    extension_kind::ExtensionKind,
    metadata::{Album, Artist},
};
use protobuf::Message;

use crate::providers::{
    ErrorKind, HomeItem, ProviderError, collections::SavedItem, transport::invalid_response,
};

use super::{
    super::{catalog, metadata},
    ARTIST_SET, COLLECTION_SET, Entry, PROVIDER, entries,
};

const BATCH_SIZE: usize = 50;

pub(super) async fn albums(session: &Session) -> Result<Vec<SavedItem>, ProviderError> {
    let saved = entries(session, COLLECTION_SET, is_album).await?;
    cards(session, &saved, ExtensionKind::ALBUM_V4, album_card).await
}

pub(super) async fn artists(session: &Session) -> Result<Vec<SavedItem>, ProviderError> {
    let saved = match entries(session, ARTIST_SET, is_artist).await {
        Ok(saved) if !saved.is_empty() => saved,
        Ok(_) => entries(session, COLLECTION_SET, is_artist).await?,
        Err(error) if error.kind == ErrorKind::Rejected => {
            entries(session, COLLECTION_SET, is_artist).await?
        }
        Err(error) => return Err(error),
    };
    cards(session, &saved, ExtensionKind::ARTIST_V4, artist_card).await
}

fn is_album(uri: &str) -> bool {
    matches!(SpotifyUri::from_uri(uri), Ok(SpotifyUri::Album { .. }))
}

fn is_artist(uri: &str) -> bool {
    matches!(SpotifyUri::from_uri(uri), Ok(SpotifyUri::Artist { .. }))
}

async fn cards(
    session: &Session,
    entries: &[Entry],
    extension: ExtensionKind,
    card: impl Fn(&str, &[u8]) -> Option<(HomeItem, Option<u32>)>,
) -> Result<Vec<SavedItem>, ProviderError> {
    let mut cards = Vec::with_capacity(entries.len());
    let mut missing_metadata = false;
    for batch in entries.chunks(BATCH_SIZE) {
        let mut metadata = metadata::read_batch(
            session,
            batch.iter().map(|entry| entry.uri.as_str()),
            extension,
        )
        .await?
        .map(|entry| (entry.entity_uri.clone(), entry))
        .collect::<HashMap<_, _>>();
        for entry in batch {
            let Some(data) = metadata.remove(&entry.uri) else {
                missing_metadata = true;
                continue;
            };
            if matches!(data.header.status_code, 404 | 410) {
                continue;
            }
            if data.header.status_code >= 400 {
                return Err(ProviderError::new(
                    ErrorKind::Network,
                    "Spotify не вернул метаданные коллекции.",
                ));
            }
            let Some((item, year)) = card(&entry.uri, &data.extension_data.value) else {
                missing_metadata = true;
                continue;
            };
            cards.push(SavedItem {
                item,
                added: entry.added(),
                year,
            });
        }
    }
    if missing_metadata && cards.is_empty() {
        return Err(invalid_response(PROVIDER));
    }
    Ok(cards)
}

fn album_card(uri: &str, bytes: &[u8]) -> Option<(HomeItem, Option<u32>)> {
    let SpotifyUri::Album { id } = SpotifyUri::from_uri(uri).ok()? else {
        return None;
    };
    let album = Album::parse_from_bytes(bytes).ok()?;
    let title = album.name().trim();
    (!title.is_empty()).then(|| {
        (
            HomeItem {
                id: uri.to_owned(),
                title: title.to_owned(),
                subtitle: catalog::artists(&album.artist).join(", "),
                url: Some(format!("https://open.spotify.com/album/{}", id.to_base62())),
                artwork: catalog::album_artwork(&album),
            },
            catalog::album_year(&album),
        )
    })
}

fn artist_card(uri: &str, bytes: &[u8]) -> Option<(HomeItem, Option<u32>)> {
    let SpotifyUri::Artist { id } = SpotifyUri::from_uri(uri).ok()? else {
        return None;
    };
    let artist = Artist::parse_from_bytes(bytes).ok()?;
    let title = artist.name().trim();
    (!title.is_empty()).then(|| {
        (
            HomeItem {
                id: uri.to_owned(),
                title: title.to_owned(),
                subtitle: String::new(),
                url: Some(format!(
                    "https://open.spotify.com/artist/{}",
                    id.to_base62()
                )),
                artwork: catalog::artist_artwork(&artist),
            },
            None,
        )
    })
}
