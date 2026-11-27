use std::{fmt, path::PathBuf, sync::atomic::AtomicBool};

use crate::{
    music::library::{PlaylistReceipt, PlaylistScope},
    providers::{self, Credentials, ErrorKind, ProviderError, ProviderId},
    storage::{
        ImportProgress, LocalPlaylistSnapshot, LocalPlaylistSummary, Store, StoreError,
        playlist_name,
    },
};

use super::{Playlist, PlaylistKey};

pub(crate) fn local_playlists() -> Result<Vec<LocalPlaylistSummary>, StoreError> {
    Store::open_default()?.local_playlists()
}

pub(crate) fn source_credentials(
    provider: ProviderId,
    profile: Option<&str>,
) -> Result<Option<Credentials>, AccessError> {
    profile
        .map(|profile| connected_credentials(&Store::open_default()?, provider, profile))
        .transpose()
}

pub(crate) fn connected_credentials(
    store: &Store,
    provider: ProviderId,
    profile: &str,
) -> Result<Credentials, AccessError> {
    let account = store
        .account(provider, profile)?
        .ok_or(AccessError::Disconnected)?;
    store
        .credentials(&account)?
        .ok_or(AccessError::MissingCredentials)
}

pub(crate) fn read_playlist(
    key: &PlaylistKey,
    cancelled: &AtomicBool,
) -> Result<Playlist, ReadError> {
    match key {
        PlaylistKey::Local(id) => {
            local_playlist(&Store::open_default().map_err(ReadError::Store)?, *id)
        }
        PlaylistKey::Source {
            provider,
            profile,
            id,
        } => {
            let credentials =
                source_credentials(*provider, profile.as_deref()).map_err(ReadError::Access)?;
            providers::read_playlist(*provider, credentials.as_ref(), id, cancelled)
                .map_err(ReadError::Provider)
        }
        PlaylistKey::Station { .. } => Err(ReadError::Provider(ProviderError {
            kind: ErrorKind::InvalidInput,
            message: "Станция открывается отдельно от плейлиста.".into(),
        })),
    }
}

pub(crate) fn prefetch_playlist(
    key: &PlaylistKey,
    cancelled: &AtomicBool,
) -> Result<PlaylistReceipt, ReadError> {
    Ok(PlaylistScope::open(key)?.refresh(false, cancelled)?.receipt)
}

fn local_playlist(store: &Store, id: i64) -> Result<Playlist, ReadError> {
    store
        .local_playlist(id)
        .map_err(ReadError::Store)?
        .ok_or(ReadError::Removed)
}

pub(crate) fn read_local_playlist_snapshot(id: i64) -> Result<LocalPlaylistSnapshot, ReadError> {
    Store::open_default()
        .map_err(ReadError::Store)?
        .local_playlist_snapshot(id)
        .map_err(ReadError::Store)?
        .ok_or(ReadError::Removed)
}

#[derive(Debug)]
pub(crate) enum AccessError {
    Store(StoreError),
    Disconnected,
    MissingCredentials,
}

impl AccessError {
    pub(crate) fn needs_sign_in(&self) -> bool {
        matches!(self, Self::Disconnected | Self::MissingCredentials)
    }
}

impl From<StoreError> for AccessError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl fmt::Display for AccessError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Disconnected => {
                formatter.write_str("Источник отключён. Подключите аккаунт в настройках.")
            }
            Self::MissingCredentials => {
                formatter.write_str("Вход в источник нужно подтвердить заново.")
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum ReadError {
    Store(StoreError),
    Removed,
    Access(AccessError),
    Provider(ProviderError),
}

impl ReadError {
    pub(crate) fn needs_sign_in(&self) -> bool {
        match self {
            Self::Access(error) => error.needs_sign_in(),
            Self::Provider(error) => error.needs_sign_in(),
            Self::Store(_) | Self::Removed => false,
        }
    }

    pub(crate) fn can_show_cached(&self) -> bool {
        matches!(
            self,
            Self::Access(AccessError::MissingCredentials)
                | Self::Provider(ProviderError {
                    kind: ErrorKind::Network | ErrorKind::Unavailable | ErrorKind::Expired,
                    ..
                })
        )
    }

    pub(crate) fn revokes_account(&self) -> bool {
        matches!(
            self,
            Self::Provider(ProviderError {
                kind: ErrorKind::Revoked,
                ..
            })
        )
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Removed => formatter.write_str("Плейлист удалён из медиатеки."),
            Self::Access(error) => error.fmt(formatter),
            Self::Provider(error) => error.fmt(formatter),
        }
    }
}

pub(crate) enum LocalChange {
    Create {
        name: String,
        files: Vec<PathBuf>,
    },
    Append {
        id: i64,
        files: Vec<PathBuf>,
    },
    Rename {
        id: i64,
        name: String,
    },
    Delete(i64),
    RemoveTrack {
        id: i64,
        index: usize,
        path: String,
        revision: Option<u64>,
    },
}

#[derive(Debug)]
pub(crate) enum LocalChanged<T = Box<LocalPlaylistSnapshot>> {
    Created(i64, String),
    Appended(i64, T),
    Renamed(i64, String),
    Deleted(i64),
    TrackRemoved(i64, T),
}

impl<T> LocalChanged<T> {
    /// Prepare the updated playlist on the worker before handing it to the view.
    pub(crate) fn map_playlist<U>(self, prepare: impl FnOnce(i64, T) -> U) -> LocalChanged<U> {
        match self {
            Self::Created(id, name) => LocalChanged::Created(id, name),
            Self::Appended(id, playlist) => LocalChanged::Appended(id, prepare(id, playlist)),
            Self::Renamed(id, name) => LocalChanged::Renamed(id, name),
            Self::Deleted(id) => LocalChanged::Deleted(id),
            Self::TrackRemoved(id, playlist) => {
                LocalChanged::TrackRemoved(id, prepare(id, playlist))
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum ChangeError {
    Store(StoreError),
    Removed,
    TrackChanged,
}

impl From<StoreError> for ChangeError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl fmt::Display for ChangeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(formatter),
            Self::Removed => formatter.write_str("Плейлист уже удалён."),
            Self::TrackChanged => {
                formatter.write_str("Список изменился. Обновите плейлист и повторите действие.")
            }
        }
    }
}

impl LocalChange {
    pub(crate) fn run(
        self,
        cancelled: &AtomicBool,
        progress: impl FnMut(ImportProgress),
    ) -> Result<LocalChanged, ChangeError> {
        self.apply_with_progress(&Store::open_default()?, cancelled, progress)
    }

    #[cfg(test)]
    fn apply(self, store: &Store) -> Result<LocalChanged, ChangeError> {
        self.apply_with_progress(store, &AtomicBool::new(false), |_| {})
    }

    fn apply_with_progress(
        self,
        store: &Store,
        cancelled: &AtomicBool,
        progress: impl FnMut(ImportProgress),
    ) -> Result<LocalChanged, ChangeError> {
        match self {
            Self::Create { name, files } => {
                let name = playlist_name(&name)?.to_owned();
                let id = store.import_local_playlist(&name, &files, cancelled, progress)?;
                Ok(LocalChanged::Created(id, name))
            }
            Self::Append { id, files } => store
                .append_local_playlist(id, &files, cancelled, progress)?
                .map(|snapshot| LocalChanged::Appended(id, Box::new(snapshot)))
                .ok_or(ChangeError::Removed),
            Self::Rename { id, name } => {
                let name = playlist_name(&name)?.to_owned();
                store
                    .rename_local_playlist(id, &name)?
                    .then_some(LocalChanged::Renamed(id, name))
                    .ok_or(ChangeError::Removed)
            }
            Self::Delete(id) => store
                .delete_local_playlist(id)?
                .then_some(LocalChanged::Deleted(id))
                .ok_or(ChangeError::Removed),
            Self::RemoveTrack {
                id,
                index,
                path,
                revision,
            } => store
                .remove_local_playlist_track(id, index, &path, revision)?
                .map(|snapshot| LocalChanged::TrackRemoved(id, Box::new(snapshot)))
                .ok_or(ChangeError::TrackChanged),
        }
    }
}

#[cfg(test)]
mod tests;
