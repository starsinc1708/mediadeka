mod imports;

use std::{collections::HashSet, path::PathBuf, sync::atomic::AtomicBool, time::SystemTime};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
#[cfg(test)]
use std::sync::atomic::Ordering;

use crate::{
    playlist::{Media, Playlist, PlaylistTrack},
    providers::ProviderId,
};

use super::{Result, Store, StoreError};
pub(crate) use imports::{ImportProgress, is_m3u_path};

pub(super) const TAG_CACHE_MIGRATION: &str = imports::TAG_CACHE_MIGRATION;
pub(super) const LIBRARY_LOOKUP_MIGRATION: &str =
    "CREATE INDEX local_playlist_track_path ON local_playlist_track(path);";
pub(super) const LOCAL_PLAYLIST_REVISION_MIGRATION: &str = "ALTER TABLE local_playlist ADD COLUMN revision INTEGER NOT NULL DEFAULT 0 CHECK (revision >= 0);";

pub(super) const MIGRATION: &str = "CREATE TABLE local_playlist (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        name TEXT NOT NULL CHECK (length(trim(name)) > 0)
    ) STRICT;
    CREATE TABLE local_playlist_track (
        playlist_id INTEGER NOT NULL REFERENCES local_playlist(id) ON DELETE CASCADE,
        ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
        path TEXT NOT NULL CHECK (length(path) > 0),
        title TEXT NOT NULL,
        artist TEXT NOT NULL,
        album TEXT,
        duration INTEGER CHECK (duration > 0),
        added TEXT NOT NULL,
        PRIMARY KEY (playlist_id, ordinal)
    ) STRICT;
    CREATE TABLE favorite_track (
        kind TEXT NOT NULL,
        provider TEXT NOT NULL,
        profile_id TEXT NOT NULL,
        track_id TEXT NOT NULL CHECK (length(track_id) > 0),
        CHECK ((kind = 'local' AND provider = '' AND profile_id = '') OR
               (kind = 'guest' AND provider <> '' AND profile_id = '') OR
               (kind = 'account' AND provider <> '' AND profile_id <> '')),
        PRIMARY KEY (kind, provider, profile_id, track_id)
    ) STRICT;";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LocalPlaylistSummary {
    pub id: i64,
    pub name: String,
    pub track_count: usize,
    pub added: Option<String>,
    pub has_files: bool,
}

#[derive(Debug)]
pub(crate) struct LocalPlaylistSnapshot {
    pub(crate) playlist: Playlist,
    pub(crate) revision: Option<u64>,
}

enum ImportDestination<'a> {
    Create(&'a str),
    Append(i64),
}

#[derive(Clone, Copy)]
pub(crate) enum FavoriteScope<'a> {
    Local,
    Guest(ProviderId),
    Account(ProviderId, &'a str),
}

impl<'a> FavoriteScope<'a> {
    pub(super) fn key(self) -> (&'static str, &'static str, &'a str) {
        match self {
            Self::Local => ("local", "", ""),
            Self::Guest(provider) => ("guest", provider.as_str(), ""),
            Self::Account(provider, profile) => ("account", provider.as_str(), profile),
        }
    }
}

impl Store {
    pub(crate) fn local_playlists(&self) -> Result<Vec<LocalPlaylistSummary>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT playlist.id, playlist.name, track.path, track.added
             FROM local_playlist playlist
             LEFT JOIN local_playlist_track track ON track.playlist_id = playlist.id
             ORDER BY playlist.id DESC, track.ordinal",
        )?;
        let mut rows = statement.query([])?;
        let mut playlists: Vec<LocalPlaylistSummary> = Vec::new();
        while let Some(row) = rows.next()? {
            let id = row.get(0)?;
            if playlists.last().is_none_or(|playlist| playlist.id != id) {
                playlists.push(LocalPlaylistSummary {
                    id,
                    name: row.get(1)?,
                    track_count: 0,
                    added: None,
                    has_files: false,
                });
            }
            let playlist = playlists.last_mut().expect("current playlist");
            if let Some(path) = row.get::<_, Option<String>>(2)? {
                playlist.track_count += 1;
                if !playlist.has_files {
                    playlist.has_files = std::path::Path::new(&path).is_file();
                }
                let added: String = row.get(3)?;
                if playlist
                    .added
                    .as_ref()
                    .is_none_or(|previous| &added < previous)
                {
                    playlist.added = Some(added);
                }
            }
        }
        Ok(playlists)
    }

    pub(crate) fn local_library_tracks(&self) -> Result<Vec<(i64, PlaylistTrack)>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT path, title, artist, album, duration, added, playlist_id
             FROM local_playlist_track
             ORDER BY added, playlist_id, ordinal",
        )?;
        let mut seen = HashSet::new();
        Ok(statement
            .query_map([], |row| Ok((row.get(6)?, file_track(row)?)))?
            .filter_map(|row: rusqlite::Result<(i64, PlaylistTrack)>| match row {
                Ok((id, track)) => seen.insert(track.id.clone()).then_some(Ok((id, track))),
                Err(error) => Some(Err(error)),
            })
            .collect::<rusqlite::Result<_>>()?)
    }

    pub(crate) fn local_library_track(&self, path: &str) -> Result<Option<(i64, PlaylistTrack)>> {
        Ok(self
            .connect()?
            .query_row(
                "SELECT path, title, artist, album, duration, added, playlist_id
             FROM local_playlist_track WHERE path = ?1
             ORDER BY added, playlist_id, ordinal LIMIT 1",
                [path],
                |row| Ok((row.get(6)?, file_track(row)?)),
            )
            .optional()?)
    }

    pub(crate) fn local_playlist(&self, id: i64) -> Result<Option<Playlist>> {
        Ok(self
            .local_playlist_snapshot(id)?
            .map(|record| record.playlist))
    }

    pub(crate) fn local_playlist_snapshot(&self, id: i64) -> Result<Option<LocalPlaylistSnapshot>> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        Self::local_playlist_snapshot_in(&transaction, id)
    }

    fn local_playlist_snapshot_in(
        connection: &Connection,
        id: i64,
    ) -> Result<Option<LocalPlaylistSnapshot>> {
        let row = connection
            .query_row(
                "SELECT name, revision FROM local_playlist WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        let Some((name, revision)) = row else {
            return Ok(None);
        };
        let mut statement = connection.prepare(
            "SELECT path, title, artist, album, duration, added
             FROM local_playlist_track WHERE playlist_id = ?1 ORDER BY ordinal",
        )?;
        let tracks = statement
            .query_map([id], file_track)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(Some(LocalPlaylistSnapshot {
            playlist: Playlist {
                name,
                owner: None,
                artwork: None,
                link: None,
                tracks,
            },
            revision: (revision != 0).then_some(revision as u64),
        }))
    }

    #[cfg(any(test, feature = "capture"))]
    pub(crate) fn create_local_playlist(&self, name: &str, files: &[PathBuf]) -> Result<i64> {
        self.import_local_playlist(name, files, &AtomicBool::new(false), |_| {})
    }

    pub(crate) fn import_local_playlist(
        &self,
        name: &str,
        files: &[PathBuf],
        cancelled: &AtomicBool,
        progress: impl FnMut(ImportProgress),
    ) -> Result<i64> {
        self.write_local_playlist(
            ImportDestination::Create(playlist_name(name)?),
            files,
            cancelled,
            progress,
            |_, id| Ok(id),
        )?
        .ok_or(StoreError::InvalidData)
    }

    pub(crate) fn append_local_playlist(
        &self,
        id: i64,
        files: &[PathBuf],
        cancelled: &AtomicBool,
        progress: impl FnMut(ImportProgress),
    ) -> Result<Option<LocalPlaylistSnapshot>> {
        self.write_local_playlist(
            ImportDestination::Append(id),
            files,
            cancelled,
            progress,
            |transaction, id| {
                Self::local_playlist_snapshot_in(transaction, id)?.ok_or(StoreError::InvalidData)
            },
        )
    }

    fn write_local_playlist<T>(
        &self,
        destination: ImportDestination<'_>,
        files: &[PathBuf],
        cancelled: &AtomicBool,
        mut progress: impl FnMut(ImportProgress),
        read_result: impl FnOnce(&Connection, i64) -> Result<T>,
    ) -> Result<Option<T>> {
        imports::check_cancelled(cancelled)?;
        let mut connection = self.connect()?;
        let staged = imports::stage_tracks(&mut connection, files, cancelled, &mut progress)?;
        if matches!(destination, ImportDestination::Append(_)) && staged.count != 0 {
            progress(ImportProgress::Appending {
                count: staged.count,
                titles: staged.preview,
            });
        }
        imports::check_cancelled(cancelled)?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        imports::check_cancelled(cancelled)?;
        let (id, offset) = match destination {
            ImportDestination::Create(name) => {
                transaction.execute("INSERT INTO local_playlist (name) VALUES (?1)", [name])?;
                (transaction.last_insert_rowid(), 0)
            }
            ImportDestination::Append(id) => {
                let offset = transaction
                    .query_row(
                        "SELECT COALESCE((SELECT MAX(ordinal) + 1
                         FROM local_playlist_track WHERE playlist_id = ?1), 0)
                         FROM local_playlist WHERE id = ?1",
                        [id],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?;
                let Some(offset) = offset else {
                    return Ok(None);
                };
                if staged.count == 0 {
                    return Err(StoreError::PlaylistInput(
                        "В выбранных файлах нет треков.".into(),
                    ));
                }
                (id, offset)
            }
        };
        if staged.count != 0 {
            let added = DateTime::<Utc>::from(SystemTime::now())
                .format("%Y-%m-%d")
                .to_string();
            transaction.execute(
                "INSERT INTO local_playlist_track
                    (playlist_id, ordinal, path, title, artist, album, duration, added)
                 SELECT ?1, ordinal + ?2, path, title, artist, album, duration, ?3
                 FROM temp.import_track",
                params![id, offset, added],
            )?;
        }
        let result = read_result(&transaction, id)?;
        imports::check_cancelled(cancelled)?;
        transaction.commit()?;
        Ok(Some(result))
    }

    pub(crate) fn rename_local_playlist(&self, id: i64, name: &str) -> Result<bool> {
        let changed = self.connect()?.execute(
            "UPDATE local_playlist SET name = ?1 WHERE id = ?2",
            params![playlist_name(name)?, id],
        )?;
        Ok(changed != 0)
    }

    pub(crate) fn delete_local_playlist(&self, id: i64) -> Result<bool> {
        let changed = self
            .connect()?
            .execute("DELETE FROM local_playlist WHERE id = ?1", [id])?;
        Ok(changed != 0)
    }

    pub(crate) fn remove_local_playlist_track(
        &self,
        id: i64,
        index: usize,
        path: &str,
        revision: Option<u64>,
    ) -> Result<Option<LocalPlaylistSnapshot>> {
        let index = i64::try_from(index).map_err(|_| StoreError::InvalidData)?;
        let revision = i64::try_from(revision.unwrap_or(0)).map_err(|_| StoreError::InvalidData)?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "DELETE FROM local_playlist_track WHERE playlist_id = ?1 AND path = ?3
             AND ordinal = (SELECT ordinal FROM local_playlist_track WHERE playlist_id = ?1
                            ORDER BY ordinal LIMIT 1 OFFSET ?2)
             AND (SELECT revision FROM local_playlist WHERE id = ?1) = ?4",
            params![id, index, path, revision],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        transaction.execute(
            "UPDATE local_playlist SET revision = revision + 1 WHERE id = ?1",
            [id],
        )?;
        let snapshot =
            Self::local_playlist_snapshot_in(&transaction, id)?.ok_or(StoreError::InvalidData)?;
        transaction.commit()?;
        Ok(Some(snapshot))
    }

    pub(crate) fn favorite_tracks(&self, scope: FavoriteScope<'_>) -> Result<HashSet<String>> {
        let (kind, provider, profile_id) = scope.key();
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT track_id FROM favorite_track
             WHERE kind = ?1 AND provider = ?2 AND profile_id = ?3 AND (
                 ?1 <> 'account' OR EXISTS (
                    SELECT 1 FROM account_source WHERE provider = ?2 AND profile_id = ?3
                 )
             )",
        )?;
        let tracks = statement
            .query_map(params![kind, provider, profile_id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(tracks)
    }

    #[cfg(test)]
    pub(crate) fn set_track_favorite(
        &self,
        scope: FavoriteScope<'_>,
        track_id: &str,
        favorite: bool,
        cancelled: &AtomicBool,
    ) -> Result<bool> {
        if track_id.is_empty() {
            return Err(StoreError::PlaylistInput(
                "Неизвестный идентификатор трека.".into(),
            ));
        }
        let (kind, provider, profile_id) = scope.key();
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // A write that passes this check before disconnect starts is ordered
        // before the transaction that removes the account's marks.
        if cancelled.load(Ordering::SeqCst) {
            return Ok(false);
        }
        if let FavoriteScope::Account(..) = scope {
            let account_exists: bool = transaction.query_row(
                "SELECT EXISTS (SELECT 1 FROM account_source WHERE provider = ?1 AND profile_id = ?2)",
                params![provider, profile_id],
                |row| row.get(0),
            )?;
            if !account_exists {
                return Err(StoreError::PlaylistInput("Аккаунт отключён.".into()));
            }
        }
        let sql = if favorite {
            "INSERT INTO favorite_track (kind, provider, profile_id, track_id)
             VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING"
        } else {
            "DELETE FROM favorite_track WHERE kind = ?1 AND provider = ?2 AND profile_id = ?3
             AND track_id = ?4"
        };
        transaction.execute(sql, params![kind, provider, profile_id, track_id])?;
        transaction.commit()?;
        Ok(true)
    }
}

fn file_track(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlaylistTrack> {
    let path: String = row.get(0)?;
    Ok(PlaylistTrack {
        media: Some(Media::File(PathBuf::from(&path))),
        id: path,
        title: row.get(1)?,
        artist: row.get(2)?,
        artists: Vec::new(),
        artist_credits: Vec::new(),
        album_ref: None,
        album: row.get(3)?,
        duration: row.get(4)?,
        added: row.get(5)?,
        artwork: None,
    })
}

pub(super) fn delete_account_favorites(
    connection: &Connection,
    provider: ProviderId,
    profile_id: &str,
) -> Result<()> {
    connection.execute(
        "DELETE FROM favorite_track WHERE kind = 'account' AND provider = ?1 AND profile_id = ?2",
        params![provider.as_str(), profile_id],
    )?;
    Ok(())
}

pub(crate) fn playlist_name(name: &str) -> Result<&str> {
    let name = name.trim();
    if name.is_empty() || name.contains('\0') {
        return Err(StoreError::PlaylistInput(
            "Введите название плейлиста.".into(),
        ));
    }
    Ok(name)
}

#[cfg(test)]
mod tests;
