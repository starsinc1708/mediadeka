use std::time::Duration;

use librespot_core::{Session, error::ErrorKind as SpotifyError};
use librespot_protocol::{
    entity_extension_data::EntityExtensionData,
    extended_metadata::{BatchedEntityRequest, EntityRequest, ExtensionQuery},
    extension_kind::ExtensionKind,
};
use protobuf::EnumOrUnknown;
use tokio::time::timeout;

use crate::providers::{ErrorKind, ProviderError};

const BATCH_TIMEOUT: Duration = Duration::from_secs(15);

pub(super) async fn read_batch<'a>(
    session: &Session,
    uris: impl IntoIterator<Item = &'a str>,
    kind: ExtensionKind,
) -> Result<impl Iterator<Item = EntityExtensionData>, ProviderError> {
    let entity_request = uris
        .into_iter()
        .map(|uri| EntityRequest {
            entity_uri: uri.to_owned(),
            query: vec![ExtensionQuery {
                extension_kind: EnumOrUnknown::new(kind),
                ..Default::default()
            }],
            ..Default::default()
        })
        .collect();
    let result = timeout(
        BATCH_TIMEOUT,
        session
            .spclient()
            .get_extended_metadata(BatchedEntityRequest {
                entity_request,
                ..Default::default()
            }),
    )
    .await
    .map_err(|_| {
        ProviderError::new(
            ErrorKind::Network,
            "Spotify не ответил на запрос метаданных.",
        )
    })?
    .map_err(|error| match error.kind {
        SpotifyError::Unauthenticated => {
            ProviderError::new(ErrorKind::Expired, "Подключите Spotify заново.")
        }
        SpotifyError::PermissionDenied | SpotifyError::NotFound => {
            ProviderError::new(ErrorKind::Rejected, "Метаданные Spotify недоступны.")
        }
        SpotifyError::ResourceExhausted => {
            ProviderError::new(ErrorKind::Network, "Spotify ограничил чтение метаданных.")
        }
        _ => ProviderError::new(
            ErrorKind::Network,
            "Не удалось прочитать метаданные Spotify.",
        ),
    })?;
    Ok(result
        .extended_metadata
        .into_iter()
        .flat_map(|entry| entry.extension_data))
}
