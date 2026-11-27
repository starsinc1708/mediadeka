#[cfg(windows)]
mod publish;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use crate::{music::likes::LikeIdentity, providers::ProviderId};

use super::{Result, Store};

pub(super) const MIGRATION: &str = "CREATE TABLE sound_cache (
    identity TEXT PRIMARY KEY NOT NULL,
    file_name TEXT NOT NULL UNIQUE,
    byte_count INTEGER NOT NULL CHECK (byte_count > 0),
    last_played INTEGER NOT NULL,
    retained INTEGER NOT NULL DEFAULT 0 CHECK (retained IN (0,1)),
    quality TEXT NOT NULL
) STRICT;
CREATE TABLE sound_cache_garbage (file_name TEXT PRIMARY KEY NOT NULL) STRICT;
CREATE TABLE sound_cache_origin (
    identity TEXT NOT NULL REFERENCES sound_cache(identity) ON DELETE CASCADE,
    provider TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    playlist_id TEXT NOT NULL,
    PRIMARY KEY (identity, provider, profile_id, playlist_id)
) STRICT;
CREATE TABLE sound_download_disabled (provider TEXT PRIMARY KEY NOT NULL) STRICT;";

#[cfg(windows)]
pub(crate) struct CachedAudio {
    pub path: PathBuf,
    pub quality: crate::playback::AudioQuality,
}

impl Store {
    fn audio_directory(&self) -> PathBuf {
        self.database.with_file_name("sound-cache")
    }

    #[cfg(windows)]
    pub(crate) fn sound_partial_directory(&self) -> Result<PathBuf> {
        let directory = self.audio_directory().join("partial");
        std::fs::create_dir_all(&directory)?;
        Ok(directory)
    }

    pub(crate) fn sound_download_enabled(&self, provider: ProviderId) -> Result<bool> {
        Ok(!self.connect()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM sound_download_disabled WHERE provider=?1)",
            [provider.as_str()],
            |row| row.get::<_, bool>(0),
        )?)
    }

    pub(crate) fn set_sound_download_enabled(
        &self,
        provider: ProviderId,
        enabled: bool,
    ) -> Result<()> {
        let sql = if enabled {
            "DELETE FROM sound_download_disabled WHERE provider=?1"
        } else {
            "INSERT OR IGNORE INTO sound_download_disabled (provider) VALUES (?1)"
        };
        self.connect()?.execute(sql, [provider.as_str()])?;
        Ok(())
    }

    pub(crate) fn cached_recordings(&self) -> Result<HashSet<LikeIdentity>> {
        let directory = self.audio_directory();
        let connection = self.connect()?;
        let mut rows =
            connection.prepare("SELECT identity, file_name, byte_count FROM sound_cache")?;
        let mut recordings = HashSet::new();
        for row in rows.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })? {
            let (identity, name, size) = row?;
            if existing_file(&directory, &name, size) {
                recordings.insert(serde_json::from_str(&identity)?);
            }
        }
        Ok(recordings)
    }

    #[cfg(windows)]
    pub(crate) fn cached_audio(
        &self,
        entry: &crate::playback::QueueEntry,
    ) -> Result<Option<CachedAudio>> {
        use rusqlite::OptionalExtension;
        let Some(identity) = LikeIdentity::from_track(&entry.playlist, &entry.track) else {
            return Ok(None);
        };
        let identity = serde_json::to_string(&identity)?;
        let connection = self.connect()?;
        let row = connection
            .query_row(
                "SELECT file_name, byte_count, quality FROM sound_cache WHERE identity=?1",
                [&identity],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()?;
        let Some((name, size, quality)) = row else {
            return Ok(None);
        };
        let directory = self.audio_directory();
        if !existing_file(&directory, &name, size) {
            connection.execute(
                "DELETE FROM sound_cache WHERE identity=?1 AND file_name=?2",
                (&identity, &name),
            )?;
            return Ok(None);
        }
        Ok(Some(CachedAudio {
            path: directory.join(name),
            quality: serde_json::from_str(&quality)?,
        }))
    }

    #[cfg(windows)]
    pub(crate) fn touch_cached_audio(&self, path: &Path) -> Result<()> {
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            self.connect()?.execute(
                "UPDATE sound_cache SET last_played=CAST(unixepoch('subsec') * 1000 AS INTEGER) WHERE file_name=?1",
                [name],
            )?;
        }
        Ok(())
    }
}

fn existing_file(directory: &Path, name: &str, size: i64) -> bool {
    // The index accepts only cache-owned filenames. A profile database is
    // never allowed to resolve an arbitrary path for playback.
    valid_name(name)
        && directory
            .join(name)
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.is_file() && i64::try_from(metadata.len()) == Ok(size))
}

fn valid_name(name: &str) -> bool {
    name.starts_with("audio-")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
