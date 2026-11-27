use rusqlite::{OptionalExtension, params};

use crate::playback::{QueueResume, QueueResumeUpdate};

use super::{Result, Store, StoreError};

const MAX_PLAN_BYTES: usize = 32 * 1024 * 1024;

pub(super) const MIGRATION: &str = "CREATE TABLE queue_resume (
    singleton INTEGER PRIMARY KEY CHECK (singleton=1),
    session TEXT NOT NULL CHECK (length(session)>0),
    plan TEXT NOT NULL,
    progress TEXT NOT NULL
) STRICT;";

impl Store {
    pub(crate) fn save_queue_resume(
        &self,
        update: &QueueResumeUpdate,
        replace_plan: bool,
    ) -> Result<()> {
        let progress = serde_json::to_string(&update.progress)?;
        let connection = self.connect()?;
        let session = &update.plan.session;
        if replace_plan {
            let plan = serde_json::to_string(update.plan.as_ref())?;
            if plan.len() > MAX_PLAN_BYTES {
                return Err(StoreError::InvalidData);
            }
            connection.execute(
                "INSERT INTO queue_resume VALUES (1, ?1, ?2, ?3)
                 ON CONFLICT(singleton) DO UPDATE SET
                    session=excluded.session, plan=excluded.plan, progress=excluded.progress",
                params![session, plan, progress],
            )?;
        } else {
            connection.execute(
                "UPDATE queue_resume SET progress=?2 WHERE singleton=1 AND session=?1",
                params![session, progress],
            )?;
        }
        Ok(())
    }

    pub(crate) fn queue_resume(&self) -> Result<Option<QueueResume>> {
        let mut connection = self.connect()?;
        let tx = connection.transaction()?;
        let row = tx
            .query_row(
                "SELECT plan, progress FROM queue_resume WHERE singleton=1
             AND length(CAST(plan AS BLOB))<=?1",
                [MAX_PLAN_BYTES as i64],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?;
        let Some((plan, progress)) = row else {
            return Ok(None);
        };
        let mut queue = QueueResume {
            plan: serde_json::from_str(&plan)?,
            progress: serde_json::from_str(&progress)?,
        };
        let active = {
            let mut query = tx.prepare(
                "SELECT provider, profile_id FROM account_source JOIN account USING(provider, profile_id)
                 WHERE NOT EXISTS (SELECT 1 FROM music_revoked WHERE music_revoked.provider=account.provider
                    AND music_revoked.profile_id=account.profile_id AND music_revoked.credential_ref=account.credential_ref)",
            )?;
            query
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let valid = queue.retain_accessible(|entry| {
            entry
                .playlist
                .connected_source()
                .is_none_or(|(provider, profile)| {
                    profile.is_none_or(|profile| {
                        active
                            .iter()
                            .any(|(source, id)| source == provider.as_str() && id == profile)
                    })
                })
        });
        if !valid {
            return Err(StoreError::InvalidData);
        }
        Ok((!queue.plan.order.is_empty()).then_some(queue))
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        playback::{Playback, QueueEntry},
        playlist::{Media, PlaylistKey, PlaylistTrack},
        providers::ProviderId,
    };

    use super::*;

    fn track(title: &str, playlist: PlaylistKey, index: usize, media: Media) -> QueueEntry {
        QueueEntry {
            playlist,
            playlist_revision: Some(2),
            index,
            track: PlaylistTrack {
                id: title.into(),
                title: title.into(),
                artist: String::new(),
                artists: Vec::new(),
                artist_credits: Vec::new(),
                album: None,
                album_ref: None,
                artwork: None,
                added: None,
                duration: Some(30),
                media: Some(media),
            },
            recommendations: Vec::new(),
        }
    }

    #[test]
    fn queue_resume_checks_access_and_isolates_late_progress() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        assert!(store.queue_resume().unwrap().is_none());
        let provider = ProviderId::YandexMusic;
        let remote = track(
            "remote",
            PlaylistKey::Source {
                provider,
                profile: Some("offline".into()),
                id: "owned".into(),
            },
            0,
            Media::Provider {
                provider,
                id: "11".into(),
            },
        );
        let local = track(
            "file",
            PlaylistKey::Local(42),
            9,
            Media::File("file.wav".into()),
        );
        let mut player = Playback::new();
        player.append(vec![remote, local.clone(), local]);
        let mut old = player.queue_resume_update(None).unwrap();
        old.progress.current = Some(0);
        old.progress.position_ms = 8_250;
        store.save_queue_resume(&old, true).unwrap();

        let safe = Store::open(directory.path())
            .unwrap()
            .queue_resume()
            .unwrap()
            .unwrap();
        assert_eq!(safe.plan.entries.len(), 2);
        assert_eq!(safe.progress.current, Some(0));
        assert_eq!(safe.progress.position_ms, 0);
        let mut restored = Playback::new();
        restored.restore_queue(safe);
        assert!(restored.dormant());
        assert_eq!(restored.current().unwrap().track.title, "file");

        let mut newer = restored.queue_resume_update(None).unwrap();
        newer.progress.current = Some(1);
        newer.progress.position_ms = 4_500;
        store.save_queue_resume(&newer, true).unwrap();
        old.progress.position_ms = 10_000;
        store.save_queue_resume(&old, false).unwrap();
        let saved = store.queue_resume().unwrap().unwrap();
        assert_eq!(saved.progress.current, Some(1));
        assert_eq!(saved.progress.position_ms, 4_500);
        assert_eq!(saved.plan.entries[saved.plan.order[1]].entry.index, 9);
    }
}
