use std::{fmt, sync::atomic::AtomicBool};

use crate::{
    playlist::{
        PlaylistKey, PlaylistTrack,
        library::{AccessError, source_credentials},
    },
    providers::{self, Credentials, ErrorKind, ProviderError, ProviderId, yandex::station},
};

pub(crate) mod recommendations;

pub(crate) const MY_WAVE: &str = "user:onyourwave";

pub(crate) struct Feed {
    pub tracks: Vec<PlaylistTrack>,
    pub rotor: Option<Rotor>,
}

pub(crate) struct Rotor {
    pub session_id: String,
    pub batch_id: String,
}

#[derive(Debug)]
pub(crate) enum StationError {
    InvalidTarget,
    Access(AccessError),
    Provider(ProviderError),
}

impl StationError {
    pub(crate) fn needs_sign_in(&self) -> bool {
        match self {
            Self::InvalidTarget => false,
            Self::Access(error) => error.needs_sign_in(),
            Self::Provider(error) => error.needs_sign_in(),
        }
    }
}

impl fmt::Display for StationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTarget => formatter.write_str("Некорректная станция."),
            Self::Access(error) => error.fmt(formatter),
            Self::Provider(error) => error.fmt(formatter),
        }
    }
}

pub(crate) fn start(key: &PlaylistKey, cancelled: &AtomicBool) -> Result<Feed, StationError> {
    let PlaylistKey::Station {
        provider,
        profile,
        id,
    } = key
    else {
        return Err(StationError::InvalidTarget);
    };
    if !matches!(provider, ProviderId::YandexMusic | ProviderId::SoundCloud) {
        return Err(StationError::Provider(unsupported()));
    }
    let credentials =
        source_credentials(*provider, profile.as_deref()).map_err(StationError::Access)?;
    start_provider(*provider, id, credentials.as_ref(), cancelled).map_err(StationError::Provider)
}

fn start_provider(
    provider: ProviderId,
    id: &str,
    credentials: Option<&Credentials>,
    cancelled: &AtomicBool,
) -> Result<Feed, ProviderError> {
    match provider {
        ProviderId::YandexMusic => {
            station::start(yandex_access(credentials)?, id, cancelled).map(from_batch)
        }
        ProviderId::SoundCloud => providers::read_playlist(provider, credentials, id, cancelled)
            .map(|playlist| Feed {
                tracks: playlist.tracks,
                rotor: None,
            }),
        _ => Err(unsupported()),
    }
}

pub(crate) fn refill(
    key: &PlaylistKey,
    session: &str,
    recent: &[String],
    cancelled: &AtomicBool,
) -> Result<Feed, StationError> {
    refill_provider(Some(&yandex_credentials(key)?), session, recent, cancelled)
        .map_err(StationError::Provider)
}

fn refill_provider(
    credentials: Option<&Credentials>,
    session: &str,
    recent: &[String],
    cancelled: &AtomicBool,
) -> Result<Feed, ProviderError> {
    station::refill(yandex_access(credentials)?, session, recent, cancelled).map(from_batch)
}

pub(crate) fn feedback(
    key: &PlaylistKey,
    session: &str,
    batch: &str,
    feedback: station::StationFeedback<'_>,
    cancelled: &AtomicBool,
) -> Result<(), String> {
    let credentials = yandex_credentials(key).map_err(|error| error.to_string())?;
    let token = yandex_access(Some(&credentials)).map_err(|error| error.to_string())?;
    station::send_feedback(token, session, batch, feedback, cancelled)
        .map_err(|error| error.to_string())
}

fn yandex_credentials(key: &PlaylistKey) -> Result<Credentials, StationError> {
    let PlaylistKey::Station {
        provider: ProviderId::YandexMusic,
        profile,
        ..
    } = key
    else {
        return Err(StationError::InvalidTarget);
    };
    source_credentials(ProviderId::YandexMusic, profile.as_deref())
        .map_err(StationError::Access)?
        .ok_or_else(|| StationError::Provider(yandex_signed_out()))
}

fn yandex_access(credentials: Option<&Credentials>) -> Result<&str, ProviderError> {
    match credentials {
        Some(Credentials::YandexMusic { access_token, .. }) => Ok(access_token),
        _ => Err(yandex_signed_out()),
    }
}

fn yandex_signed_out() -> ProviderError {
    ProviderError {
        kind: ErrorKind::Expired,
        message: "Подключите Яндекс Музыку для запуска станции.".into(),
    }
}

fn unsupported() -> ProviderError {
    ProviderError {
        kind: ErrorKind::Unavailable,
        message: "Для этой площадки станции пока недоступны.".into(),
    }
}

fn from_batch(batch: station::StationBatch) -> Feed {
    Feed {
        tracks: batch.tracks,
        rotor: Some(Rotor {
            session_id: batch.session_id,
            batch_id: batch.batch_id,
        }),
    }
}
