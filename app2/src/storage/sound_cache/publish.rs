use std::{
    fs::{self, File},
    path::Path,
};

use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    music::likes::LikeIdentity,
    playback::{AudioQuality, QueueEntry},
};

use super::{Result, Store, existing_file, valid_name};
use crate::storage::StoreError;

const MAX_CACHE_BYTES: i64 = 2 * 1024 * 1024 * 1024;

impl Store {
    pub(crate) fn publish_cached_audio(
        &self,
        entry: &QueueEntry,
        path: &Path,
        quality: &AudioQuality,
    ) -> Result<bool> {
        self.publish_with_limit(entry, path, quality, MAX_CACHE_BYTES)
    }

    fn publish_with_limit(
        &self,
        entry: &QueueEntry,
        path: &Path,
        quality: &AudioQuality,
        limit: i64,
    ) -> Result<bool> {
        let Some(identity) = LikeIdentity::from_track(&entry.playlist, &entry.track) else {
            return Ok(false);
        };
        let identity = serde_json::to_string(&identity)?;
        let mut input = File::open(path)?;
        let size = i64::try_from(input.metadata()?.len()).map_err(|_| StoreError::InvalidData)?;
        if size <= 0 || size > limit {
            return Ok(false);
        }
        let directory = self.audio_directory();
        fs::create_dir_all(&directory)?;
        let mut output = tempfile::Builder::new()
            .prefix("audio-")
            .tempfile_in(&directory)?;
        if std::io::copy(&mut input, &mut output)? != size as u64 {
            return Err(StoreError::InvalidData);
        }
        output.as_file().sync_all()?;
        let name = output
            .path()
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(StoreError::InvalidData)?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing = transaction
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
        if let Some((file, bytes, rank)) = existing {
            let current: AudioQuality = serde_json::from_str(&rank)?;
            if existing_file(&directory, &file, bytes) && current.rank() >= quality.rank() {
                return Ok(false);
            }
            transaction.execute(
                "INSERT OR IGNORE INTO sound_cache_garbage VALUES (?1)",
                [&file],
            )?;
        }
        transaction.execute(
            "INSERT INTO sound_cache (identity, file_name, byte_count, last_played, quality)
             VALUES (?1, ?2, ?3, CAST(unixepoch('subsec') * 1000 AS INTEGER), ?4)
             ON CONFLICT(identity) DO UPDATE SET
                 file_name=excluded.file_name, byte_count=excluded.byte_count, quality=excluded.quality",
            params![identity, name, size, serde_json::to_string(quality)?],
        )?;
        if let Some((provider, profile)) = entry.playlist.connected_source() {
            let id = match &entry.playlist {
                crate::playlist::PlaylistKey::Source { id, .. }
                | crate::playlist::PlaylistKey::Station { id, .. } => id,
                crate::playlist::PlaylistKey::Local(_) => unreachable!(),
            };
            transaction.execute(
                "INSERT OR IGNORE INTO sound_cache_origin VALUES (?1,?2,?3,?4)",
                params![identity, provider.as_str(), profile.unwrap_or_default(), id],
            )?;
        }
        let mut total: i64 = transaction.query_row(
            "SELECT coalesce(sum(byte_count), 0) FROM sound_cache",
            [],
            |row| row.get(0),
        )?;
        if total > limit {
            let removable = {
                let mut candidates = transaction.prepare(
                    "SELECT identity, file_name, byte_count FROM sound_cache AS cache
                     WHERE retained=0 AND identity<>?1 AND NOT EXISTS (
                         SELECT 1 FROM sound_cache_origin AS origin
                         JOIN pinned_object AS pin USING (provider,profile_id)
                         WHERE origin.identity=cache.identity AND
                           (origin.playlist_id=pin.object_id OR origin.playlist_id=pin.kind||':'||pin.object_id))
                     ORDER BY last_played, identity",
                )?;
                candidates
                    .query_map([&identity], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, i64>(2)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()?
            };
            for (old_identity, file, bytes) in removable {
                transaction.execute("DELETE FROM sound_cache WHERE identity=?1", [old_identity])?;
                transaction.execute(
                    "INSERT OR IGNORE INTO sound_cache_garbage VALUES (?1)",
                    [file],
                )?;
                total -= bytes;
                if total <= limit {
                    break;
                }
            }
            if total > limit {
                return Ok(false);
            }
        }
        let path = output
            .into_temp_path()
            .keep()
            .map_err(|error| error.error)?;
        if let Err(error) = transaction.commit() {
            let _ = fs::remove_file(path);
            return Err(error.into());
        }
        self.clean_sound_cache()?;
        Ok(true)
    }

    pub(super) fn clean_sound_cache(&self) -> Result<()> {
        let directory = self.audio_directory();
        let connection = self.connect()?;
        let files = {
            let mut queued = connection.prepare("SELECT file_name FROM sound_cache_garbage")?;
            queued
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for file in files {
            if valid_name(&file) {
                match fs::remove_file(directory.join(&file)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => continue,
                }
            }
            connection.execute("DELETE FROM sound_cache_garbage WHERE file_name=?1", [file])?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        playlist::{Media, PlaylistKey, PlaylistTrack},
        providers::ProviderId,
    };

    fn entry(id: &str) -> QueueEntry {
        QueueEntry {
            playlist: PlaylistKey::Source {
                provider: ProviderId::Jellyfin,
                profile: Some("profile".into()),
                id: "playlist".into(),
            },
            playlist_revision: None,
            recommendations: Vec::new(),
            index: 0,
            track: PlaylistTrack {
                id: id.into(),
                title: id.into(),
                artist: "Artist".into(),
                artists: Vec::new(),
                artist_credits: Vec::new(),
                album_ref: None,
                album: None,
                artwork: None,
                added: None,
                duration: Some(3),
                media: Some(Media::Provider {
                    provider: ProviderId::Jellyfin,
                    id: id.into(),
                }),
            },
        }
    }

    #[test]
    fn replacement_and_lru_are_atomic_and_retained_items_survive() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let file = directory.path().join("fixture.bin");
        fs::write(&file, b"12345678").unwrap();
        let low = AudioQuality::new("mp3".into(), Some(128_000), 44_100);
        let high = AudioQuality::new("flac".into(), Some(768_000), 48_000);
        let first = entry("first");
        let retained = entry("retained");
        let last = entry("last");
        assert!(store.publish_with_limit(&first, &file, &low, 16).unwrap());
        let old = store.cached_audio(&first).unwrap().unwrap().path;
        assert!(store.publish_with_limit(&first, &file, &high, 16).unwrap());
        assert!(!old.exists());
        assert!(!store.publish_with_limit(&first, &file, &low, 16).unwrap());
        assert!(
            store
                .publish_with_limit(&retained, &file, &low, 16)
                .unwrap()
        );
        store.connect().unwrap().execute(
            "UPDATE sound_cache SET retained=1,last_played=0 WHERE json_extract(identity,'$.id')='retained'",
            [],
        ).unwrap();
        assert!(store.publish_with_limit(&last, &file, &low, 16).unwrap());
        let reopened = Store::open(directory.path()).unwrap();
        assert!(reopened.cached_audio(&first).unwrap().is_none());
        assert_eq!(
            reopened.cached_audio(&retained).unwrap().unwrap().quality,
            low
        );
        assert!(reopened.cached_audio(&last).unwrap().is_some());
        assert_eq!(reopened.cached_recordings().unwrap().len(), 2);
    }
}
