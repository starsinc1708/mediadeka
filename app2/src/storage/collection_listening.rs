use rusqlite::{OptionalExtension, TransactionBehavior, params};

use crate::{
    playback::{
        CollectionKey, CollectionPlan, CollectionProgress, CollectionProgressUpdate, QueueEntry,
    },
    playlist::{PlaylistKey, PlaylistTrack},
};

use super::{
    Result, Store, StoreError,
    history::{key_columns, key_from_columns},
};

const RETAINED_COLLECTIONS: i64 = 24;

pub(super) const MIGRATION: &str = "CREATE TABLE collection_listening (
        kind TEXT NOT NULL CHECK (kind IN ('local', 'source')),
        provider TEXT NOT NULL,
        profile_id TEXT NOT NULL,
        collection_id TEXT NOT NULL CHECK (length(collection_id) > 0),
        activity INTEGER NOT NULL,
        title TEXT NOT NULL,
        artwork TEXT,
        tracks TEXT NOT NULL,
        progress TEXT NOT NULL,
        session TEXT NOT NULL,
        PRIMARY KEY (kind, provider, profile_id, collection_id)
    ) STRICT;";

pub(super) const LOCAL_ALBUM_MIGRATION: &str = "ALTER TABLE collection_listening RENAME TO collection_listening_v14;
    CREATE TABLE collection_listening (
        kind TEXT NOT NULL CHECK (kind IN ('local', 'source', 'local-album')),
        provider TEXT NOT NULL,
        profile_id TEXT NOT NULL,
        collection_id TEXT NOT NULL CHECK (length(collection_id) > 0),
        activity INTEGER NOT NULL,
        title TEXT NOT NULL,
        artwork TEXT,
        tracks TEXT NOT NULL,
        progress TEXT NOT NULL,
        session TEXT NOT NULL,
        PRIMARY KEY (kind, provider, profile_id, collection_id)
    ) STRICT;
    INSERT INTO collection_listening (kind, provider, profile_id, collection_id, activity,
                                      title, artwork, tracks, progress, session)
    SELECT kind, provider, profile_id, collection_id, activity, title, artwork, tracks, progress, session
    FROM collection_listening_v14 ORDER BY rowid;
    DROP TABLE collection_listening_v14;";

pub(crate) struct CollectionRecord {
    pub key: CollectionKey,
    pub title: String,
    pub artwork: Option<String>,
    pub progress: CollectionProgress,
}

impl Store {
    pub(crate) fn save_collection_listening(
        &self,
        update: &CollectionProgressUpdate,
        replace_plan: bool,
    ) -> Result<()> {
        let plan = &update.plan;
        let (kind, provider, profile, id) = collection_columns(&plan.key)?;
        let progress = serde_json::to_string(&update.progress)?;
        let tracks = replace_plan.then(|| encode_entries(plan)).transpose()?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let activity = transaction.query_row(
            "SELECT COALESCE(MAX(activity), 0) + 1 FROM collection_listening",
            [],
            |row| row.get::<_, i64>(0),
        )?;
        if let Some(tracks) = tracks {
            transaction.execute(
                "INSERT INTO collection_listening VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT DO UPDATE SET activity = excluded.activity, title = excluded.title,
                                           artwork = excluded.artwork, tracks = excluded.tracks,
                                           progress = excluded.progress, session = excluded.session",
                params![kind, provider, profile, id, activity, plan.title, plan.artwork, tracks, progress, plan.session],
            )?;
            transaction.execute(
                "DELETE FROM collection_listening WHERE rowid NOT IN (
                    SELECT rowid FROM collection_listening ORDER BY activity DESC, rowid DESC LIMIT ?1
                 )",
                [RETAINED_COLLECTIONS],
            )?;
        } else {
            transaction.execute(
                "UPDATE collection_listening SET activity = ?5, progress = ?6
                 WHERE (kind, provider, profile_id, collection_id, session) = (?1, ?2, ?3, ?4, ?7)",
                params![
                    kind,
                    provider,
                    profile,
                    id,
                    activity,
                    progress,
                    plan.session
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn collection_records(&self) -> Result<Vec<CollectionRecord>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT kind, provider, profile_id, collection_id, COALESCE(local.name, title), artwork, progress
             FROM collection_listening AS listening
             LEFT JOIN local_playlist AS local ON kind = 'local' AND local.id = CAST(collection_id AS INTEGER)
             WHERE kind IN ('source', 'local-album') OR local.id IS NOT NULL
             ORDER BY activity DESC, listening.rowid DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;
        rows.map(|row| {
            let (kind, provider, profile, id, title, artwork, progress) = row?;
            Ok(CollectionRecord {
                key: collection_key(&kind, &provider, &profile, &id)?,
                title,
                artwork,
                progress: serde_json::from_str(&progress)?,
            })
        })
        .collect()
    }

    pub(crate) fn collection_listening(
        &self,
        key: CollectionKey,
    ) -> Result<Option<(CollectionPlan, CollectionProgress)>> {
        let (kind, provider, profile, id) = collection_columns(&key)?;
        let connection = self.connect()?;
        connection.query_row(
            "SELECT COALESCE(local.name, title), artwork, tracks, progress
             FROM collection_listening
             LEFT JOIN local_playlist AS local ON kind = 'local' AND local.id = CAST(collection_id AS INTEGER)
             WHERE (kind, provider, profile_id, collection_id) = (?1, ?2, ?3, ?4)
                AND (kind IN ('source', 'local-album') OR local.id IS NOT NULL)",
            params![kind, provider, profile, id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, String>(2)?, row.get::<_, String>(3)?)),
        ).optional()?.map(|(title, artwork, tracks, progress)| {
            let entries = read_entries(&key, &tracks)?;
            let plan = CollectionPlan::from_entries(key, title, artwork, entries);
            Ok((plan, serde_json::from_str(&progress)?))
        }).transpose()
    }
}

fn collection_columns(key: &CollectionKey) -> Result<(&'static str, String, String, String)> {
    match key {
        CollectionKey::Playlist(key) => {
            let (kind, provider, profile, id, _) = key_columns(key)?;
            Ok((kind, provider, profile, id))
        }
        CollectionKey::LocalAlbum { title, artist } => Ok((
            "local-album",
            String::new(),
            String::new(),
            serde_json::to_string(&(title, artist))?,
        )),
    }
}

fn collection_key(kind: &str, provider: &str, profile: &str, id: &str) -> Result<CollectionKey> {
    if kind == "local-album" {
        let (title, artist) = serde_json::from_str(id)?;
        Ok(CollectionKey::LocalAlbum { title, artist })
    } else {
        key_from_columns(kind, provider, profile, id).map(CollectionKey::Playlist)
    }
}

fn encode_entries(plan: &CollectionPlan) -> Result<String> {
    match &plan.key {
        CollectionKey::Playlist(_) => {
            let rows: Vec<_> = plan
                .entries
                .iter()
                .map(|entry| (entry.index, &entry.track))
                .collect();
            Ok(serde_json::to_string(&rows)?)
        }
        CollectionKey::LocalAlbum { .. } => {
            let rows = plan
                .entries
                .iter()
                .map(|entry| {
                    let PlaylistKey::Local(id) = entry.playlist else {
                        return Err(StoreError::InvalidData);
                    };
                    Ok((entry.index, id, &entry.track))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(serde_json::to_string(&rows)?)
        }
    }
}

fn read_entries(key: &CollectionKey, payload: &str) -> Result<Vec<QueueEntry>> {
    fn entry(playlist: PlaylistKey, index: usize, track: PlaylistTrack) -> QueueEntry {
        QueueEntry {
            playlist,
            playlist_revision: None,
            recommendations: Vec::new(),
            index,
            track,
        }
    }

    match key {
        CollectionKey::Playlist(key) => Ok(serde_json::from_str::<Vec<(usize, PlaylistTrack)>>(
            payload,
        )?
        .into_iter()
        .map(|(index, track)| entry(key.clone(), index, track))
        .collect()),
        CollectionKey::LocalAlbum { .. } => Ok(serde_json::from_str::<
            Vec<(usize, i64, PlaylistTrack)>,
        >(payload)?
        .into_iter()
        .map(|(index, id, track)| entry(PlaylistKey::Local(id), index, track))
        .collect()),
    }
}
