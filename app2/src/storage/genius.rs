use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};

use super::{
    Result, Store, StoreError, cleanup_credentials, report_credential_cleanup, target_for_service,
    vault,
};

pub(super) const SERVICE: &str = "genius";
const FRESH_SECONDS: i64 = 24 * 60 * 60;
const RETAIN_SECONDS: i64 = 7 * FRESH_SECONDS;
const MAX_SNAPSHOTS: i64 = 384;
const MAX_SNAPSHOT_BYTES: usize = 256 * 1024;

pub(super) const MIGRATION: &str = "
    CREATE TABLE genius_connection (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        epoch INTEGER NOT NULL CHECK (epoch >= 0),
        credential_ref TEXT
    ) STRICT;
    INSERT INTO genius_connection (singleton, epoch) VALUES (1, 0);
    CREATE TABLE genius_access (
        url TEXT PRIMARY KEY NOT NULL,
        access INTEGER NOT NULL CHECK (access BETWEEN 0 AND 2)
    ) STRICT;
    CREATE TABLE genius_snapshot (
        recording TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN ('details', 'annotations', 'lyrics')),
        url TEXT NOT NULL,
        payload TEXT NOT NULL,
        fetched INTEGER NOT NULL,
        expires INTEGER NOT NULL,
        PRIMARY KEY (recording, kind)
    ) STRICT;
    CREATE INDEX genius_snapshot_url ON genius_snapshot (url);";

pub(crate) struct GeniusSession {
    pub epoch: i64,
    pub token: String,
}

#[derive(Clone, Copy)]
pub(crate) enum GeniusSnapshotKind {
    Details,
    Annotations,
    Lyrics,
}

impl GeniusSnapshotKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Details => "details",
            Self::Annotations => "annotations",
            Self::Lyrics => "lyrics",
        }
    }
}

pub(crate) struct GeniusSnapshot<T> {
    pub url: String,
    pub value: T,
    pub fresh: bool,
}

impl Store {
    fn genius_database(&self) -> Result<Connection> {
        let db = self.connect()?;
        db.pragma_update(None, "secure_delete", true)?;
        Ok(db)
    }

    pub(crate) fn genius_connected(&self) -> Result<bool> {
        Ok(connection_state(&self.connect()?)?.1.is_some())
    }

    pub(crate) fn genius_session(&self) -> Result<Option<GeniusSession>> {
        let (epoch, reference) = connection_state(&self.connect()?)?;
        let Some(reference) = reference else {
            return Ok(None);
        };
        let target = target_for_service(SERVICE, &reference)?;
        let Some(secret) = vault::read(&target)? else {
            return if self.genius_epoch_current(epoch)? {
                Err(StoreError::InvalidData)
            } else {
                Ok(None)
            };
        };
        let token = String::from_utf8(secret).map_err(|_| StoreError::InvalidData)?;
        Ok(self
            .genius_epoch_current(epoch)?
            .then_some(GeniusSession { epoch, token }))
    }

    pub(crate) fn genius_epoch_current(&self, expected: i64) -> Result<bool> {
        let (epoch, reference) = connection_state(&self.connect()?)?;
        Ok(epoch == expected && reference.is_some())
    }

    pub(crate) fn set_genius_token(&self, token: Option<&str>) -> Result<()> {
        let mut database = self.genius_database()?;
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (_, old) = connection_state(&tx)?;
        if let Some(reference) = &old {
            let target = target_for_service(SERVICE, reference)?;
            if let Some(token) = token
                && vault::read(&target)?.as_deref() == Some(token.as_bytes())
            {
                return Ok(());
            }
            tx.execute(
                "INSERT OR IGNORE INTO credential_cleanup (provider, credential_ref)
                 VALUES (?1, ?2)",
                params![SERVICE, reference],
            )?;
        } else if token.is_none() {
            return Ok(());
        }
        let new = if let Some(token) = token {
            let reference = tx.query_row("SELECT hex(randomblob(16))", [], |row| {
                row.get::<_, String>(0)
            })?;
            let target = target_for_service(SERVICE, &reference)?;
            vault::write(&target, token.as_bytes())?;
            Some((reference, target))
        } else {
            None
        };

        let result = (|| -> Result<()> {
            tx.execute(
                "UPDATE genius_connection SET epoch = epoch + 1, credential_ref = ?1
                 WHERE singleton = 1",
                [new.as_ref().map(|(reference, _)| reference)],
            )?;
            tx.execute_batch("DELETE FROM genius_access; DELETE FROM genius_snapshot;")?;
            tx.commit()?;
            Ok(())
        })();
        if result.is_err() {
            if let Some((_, target)) = new {
                super::remove_credential(&target);
            }
        } else {
            report_credential_cleanup(cleanup_credentials(&database));
        }
        result
    }

    pub(crate) fn genius_access(&self, epoch: i64, url: &str) -> Result<Option<i64>> {
        Ok(self.connect()?.query_row(
            "SELECT (SELECT access FROM genius_access WHERE url = ?2)
             FROM genius_connection
             WHERE singleton = 1 AND epoch = ?1 AND credential_ref IS NOT NULL",
            params![epoch, url],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn set_genius_access(&self, epoch: i64, url: &str, access: i64) -> Result<()> {
        let mut database = self.genius_database()?;
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "UPDATE genius_connection SET epoch = epoch + 1
             WHERE singleton = 1 AND epoch = ?1 AND credential_ref IS NOT NULL",
            [epoch],
        )?;
        if changed != 1 {
            return Err(StoreError::InvalidData);
        }
        tx.execute(
            "INSERT INTO genius_access (url, access) VALUES (?1, ?2)
             ON CONFLICT (url) DO UPDATE SET access = excluded.access",
            params![url, access],
        )?;
        tx.execute(
            "DELETE FROM genius_snapshot WHERE url = ?1
             AND ((kind = 'lyrics' AND ?2 <> 2) OR (kind = 'annotations' AND ?2 = 0))",
            params![url, access],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn genius_snapshot<T: DeserializeOwned>(
        &self,
        epoch: i64,
        recording: &str,
        kind: GeniusSnapshotKind,
    ) -> Result<Option<GeniusSnapshot<T>>> {
        let row: Option<(String, String, bool)> = self
            .connect()?
            .query_row(
                "SELECT url, payload, fetched > unixepoch() - ?4
             FROM genius_snapshot
             WHERE recording = ?2 AND kind = ?3 AND fetched <= unixepoch()
               AND expires > unixepoch()
               AND EXISTS (SELECT 1 FROM genius_connection
                   WHERE singleton = 1 AND epoch = ?1 AND credential_ref IS NOT NULL)
               AND (kind = 'details' OR (kind = 'lyrics' AND EXISTS (
                   SELECT 1 FROM genius_access WHERE url = genius_snapshot.url AND access = 2
               )) OR (kind = 'annotations' AND NOT EXISTS (
                   SELECT 1 FROM genius_access WHERE url = genius_snapshot.url AND access = 0
               )))",
                params![epoch, recording, kind.as_str(), FRESH_SECONDS],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        row.map(|(url, payload, fresh)| {
            Ok(GeniusSnapshot {
                url,
                value: serde_json::from_str(&payload)?,
                fresh,
            })
        })
        .transpose()
    }

    pub(crate) fn save_genius_snapshot(
        &self,
        epoch: i64,
        recording: &str,
        kind: GeniusSnapshotKind,
        url: &str,
        value: &impl Serialize,
        max_age: Option<i64>,
    ) -> Result<()> {
        let max_age = max_age.unwrap_or_default().clamp(0, RETAIN_SECONDS);
        let payload = if max_age > 0 {
            let payload = serde_json::to_string(value)?;
            (payload.len() <= MAX_SNAPSHOT_BYTES).then_some(payload)
        } else {
            None
        };
        let mut database = self.genius_database()?;
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (current, reference) = connection_state(&tx)?;
        if current != epoch || reference.is_none() {
            return Ok(());
        }
        if let Some(payload) = payload {
            tx.execute(
                "INSERT INTO genius_snapshot (recording, kind, url, payload, fetched, expires)
                 SELECT ?1, ?2, ?3, ?4, unixepoch(), unixepoch() + ?5
                 WHERE (?2 = 'details' OR (?2 = 'lyrics' AND EXISTS (
                     SELECT 1 FROM genius_access WHERE url = ?3 AND access = 2
                 )) OR (?2 = 'annotations' AND NOT EXISTS (
                     SELECT 1 FROM genius_access WHERE url = ?3 AND access = 0
                 )))
                 ON CONFLICT (recording, kind) DO UPDATE
                 SET url = excluded.url, payload = excluded.payload,
                     fetched = excluded.fetched, expires = excluded.expires",
                params![recording, kind.as_str(), url, payload, max_age],
            )?;
        } else {
            tx.execute(
                "DELETE FROM genius_snapshot WHERE recording = ?1 AND kind = ?2",
                params![recording, kind.as_str()],
            )?;
        }
        tx.execute(
            "DELETE FROM genius_snapshot WHERE expires <= unixepoch() OR fetched > unixepoch()
             OR rowid IN (
                 SELECT rowid FROM genius_snapshot ORDER BY fetched DESC, rowid DESC LIMIT -1 OFFSET ?1
             )",
            [MAX_SNAPSHOTS],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn prune_genius_snapshots(&self) -> Result<()> {
        self.genius_database()?.execute(
            "DELETE FROM genius_snapshot WHERE expires <= unixepoch() OR fetched > unixepoch()",
            [],
        )?;
        Ok(())
    }
}

fn connection_state(db: &Connection) -> Result<(i64, Option<String>)> {
    Ok(db.query_row(
        "SELECT epoch, credential_ref FROM genius_connection WHERE singleton = 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> (tempfile::TempDir, Store) {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        store
            .connect()
            .unwrap()
            .execute(
                "UPDATE genius_connection SET credential_ref = 'test' WHERE singleton = 1",
                [],
            )
            .unwrap();
        (directory, store)
    }

    #[test]
    fn genius_snapshots_cannot_resurface_after_a_grant_is_revoked() {
        use GeniusSnapshotKind::*;

        let (_directory, store) = profile();
        let url = "https://genius.com/Test-track-lyrics";
        let other = "https://genius.com/Other-track-lyrics";
        let retain = Some(RETAIN_SECONDS);
        store.set_genius_access(0, url, 2).unwrap();
        store.set_genius_access(1, other, 2).unwrap();
        store
            .save_genius_snapshot(2, "one", Lyrics, url, &"original", retain)
            .unwrap();
        store
            .save_genius_snapshot(2, "one", Details, url, &"credits", retain)
            .unwrap();
        store
            .save_genius_snapshot(2, "one", Annotations, url, &"note", retain)
            .unwrap();
        store
            .save_genius_snapshot(2, "two", Lyrics, other, &"other song", retain)
            .unwrap();

        let get = |epoch, key, kind| {
            store
                .genius_snapshot::<String>(epoch, key, kind)
                .unwrap()
                .map(|snapshot| snapshot.value)
        };
        assert_eq!(get(2, "one", Lyrics).as_deref(), Some("original"));
        store.set_genius_access(2, url, 0).unwrap();
        assert!(!store.genius_epoch_current(2).unwrap());
        assert_eq!(get(3, "one", Lyrics), None);
        assert_eq!(get(3, "one", Annotations), None);
        assert_eq!(get(3, "one", Details).as_deref(), Some("credits"));
        assert_eq!(get(3, "two", Lyrics).as_deref(), Some("other song"));
        store
            .save_genius_snapshot(2, "one", Lyrics, url, &"late", retain)
            .unwrap();
        store
            .save_genius_snapshot(3, "one", Annotations, url, &"late", retain)
            .unwrap();
        assert_eq!(get(3, "one", Annotations), None);
        store.set_genius_access(3, url, 2).unwrap();
        assert_eq!(get(4, "one", Lyrics), None);

        store
            .save_genius_snapshot(4, "two", Lyrics, other, &"uncacheable", None)
            .unwrap();
        assert_eq!(get(4, "two", Lyrics), None);
        assert!(store.set_genius_access(3, url, 2).is_err());
    }

    #[test]
    fn genius_full_text_requires_save_access_and_has_bounded_expiry() {
        let (_directory, store) = profile();
        let kind = GeniusSnapshotKind::Lyrics;
        let url = "https://genius.com/Test-track-lyrics";
        let get = |epoch| {
            store
                .genius_snapshot::<String>(epoch, "track", kind)
                .unwrap()
        };
        store
            .save_genius_snapshot(0, "track", kind, url, &"text", Some(i64::MAX))
            .unwrap();
        assert!(get(0).is_none());
        store.set_genius_access(0, url, 1).unwrap();
        store
            .save_genius_snapshot(1, "track", kind, url, &"text", Some(i64::MAX))
            .unwrap();
        assert!(get(1).is_none());
        store.set_genius_access(1, url, 2).unwrap();
        store
            .save_genius_snapshot(2, "track", kind, url, &"text", Some(i64::MAX))
            .unwrap();
        assert!(get(2).unwrap().fresh);

        let db = store.connect().unwrap();
        let age = db
            .query_row("SELECT expires - fetched FROM genius_snapshot", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        assert_eq!(age, RETAIN_SECONDS);
        db.execute(
            "UPDATE genius_snapshot SET fetched = unixepoch() - ?1",
            [FRESH_SECONDS],
        )
        .unwrap();
        assert!(!get(2).unwrap().fresh);
        db.execute("UPDATE genius_snapshot SET expires = unixepoch()", [])
            .unwrap();
        assert!(get(2).is_none());
        store.prune_genius_snapshots().unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM genius_snapshot", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
    }
}
