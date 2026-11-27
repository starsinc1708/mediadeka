use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    music::{ConfirmedLink, ObjectKind, ObjectRef, links::PublicLink},
    providers::ProviderId,
    telemetry::{self, CatalogInvalidation},
};

use super::{Account, Result, Store};

#[cfg(test)]
mod tests;

pub(super) const MIGRATION: &str = "CREATE TABLE music_snapshot (
    provider TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    region TEXT NOT NULL,
    slot TEXT NOT NULL,
    payload TEXT NOT NULL,
    PRIMARY KEY (provider, profile_id, region, slot)
) STRICT;
CREATE TABLE music_match (
    region TEXT NOT NULL,
    provider TEXT NOT NULL,
    kind TEXT NOT NULL,
    object_id TEXT NOT NULL,
    retry_at INTEGER NOT NULL,
    payload TEXT,
    PRIMARY KEY (region, provider, kind, object_id)
) STRICT;";

pub(super) const REVOCATION_MIGRATION: &str = "CREATE TABLE music_revoked (
    provider TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    credential_ref TEXT NOT NULL,
    PRIMARY KEY (provider, profile_id),
    FOREIGN KEY (provider, profile_id) REFERENCES account (provider, profile_id) ON DELETE CASCADE
) STRICT;";

pub(super) const ALIAS_MIGRATION: &str = "CREATE TABLE music_public_alias (
    region TEXT NOT NULL,
    platform TEXT NOT NULL,
    kind TEXT NOT NULL,
    object_id TEXT NOT NULL,
    native_object TEXT NOT NULL,
    PRIMARY KEY (region, platform, kind, object_id)
) STRICT;";

pub(super) const SNAPSHOT_TIMESTAMP_MIGRATION: &str =
    "ALTER TABLE music_snapshot ADD COLUMN saved_at_ms INTEGER NOT NULL DEFAULT 0;
     ALTER TABLE music_snapshot ADD COLUMN encoded_bytes INTEGER NOT NULL DEFAULT 0;
     UPDATE music_snapshot SET encoded_bytes = length(CAST(payload AS BLOB));";

pub(super) const CATALOG_GENERATION_MIGRATION: &str = "CREATE TABLE catalog_cache_generation (
         singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
         generation INTEGER NOT NULL CHECK (generation >= 0)
     ) STRICT;
     INSERT INTO catalog_cache_generation VALUES (1, 0);";

pub(super) const PLAYLIST_SNAPSHOT_BUDGET_MIGRATION: &str =
    "CREATE INDEX cached_playlist_recency ON music_snapshot(saved_at_ms DESC)
     WHERE slot GLOB 'playlist:*';";

const CATALOG_PAGE_LIMIT: i64 = 1_024;
const CATALOG_DISK_BYTES: i64 = 128 * 1_024 * 1_024;
const CATALOG_DETAIL_SLOTS: &str =
    "slot GLOB 'track:*' OR slot GLOB 'album:*' OR slot GLOB 'artist:*' OR slot GLOB 'genre:*'";
const CATALOG_PLAYLIST_SLOTS: &str = "slot GLOB 'playlist:*'";
const CATALOG_PLAYLIST_LIMIT: i64 = 256;
const CATALOG_PLAYLIST_DISK_BYTES: i64 = 128 * 1_024 * 1_024;

pub(crate) enum CatalogSave {
    Stored {
        saved_at_ms: u64,
        encoded_bytes: usize,
    },
    AccountChanged,
    CacheCleared,
}

pub(crate) enum MusicSnapshot<T> {
    Unchanged,
    Changed {
        value: T,
        saved_at_ms: u64,
        encoded_bytes: usize,
        generation: i64,
    },
}

impl Store {
    pub(crate) fn music_matches<T: DeserializeOwned>(
        &self,
        region: &str,
        kind: ObjectKind,
    ) -> Result<Vec<(ObjectRef, T)>> {
        let connection = self.connect()?;
        let mut records = connection.prepare(
            "SELECT provider, object_id, payload FROM music_match
             WHERE region=?1 AND kind=?2 AND payload IS NOT NULL",
        )?;
        let mut rows = records.query(params![region, kind.path()])?;
        let mut matches = Vec::new();
        while let Some(row) = rows.next()? {
            let name: String = row.get(0)?;
            let Some(provider) = ProviderId::ALL
                .into_iter()
                .find(|item| item.as_str() == name)
            else {
                continue;
            };
            let object = ObjectRef {
                provider,
                kind,
                id: row.get(1)?,
            };
            let payload: String = row.get(2)?;
            matches.push((object, serde_json::from_str(&payload)?));
        }
        Ok(matches)
    }

    pub(crate) fn music_snapshot<T: DeserializeOwned>(
        &self,
        provider: ProviderId,
        account: Option<&Account>,
        region: &str,
        slot: &str,
    ) -> Result<Option<T>> {
        Ok(
            match self.versioned_music_snapshot(provider, account, region, slot, None)? {
                Some(MusicSnapshot::Changed { value, .. }) => Some(value),
                _ => None,
            },
        )
    }

    pub(crate) fn versioned_music_snapshot<T: DeserializeOwned>(
        &self,
        provider: ProviderId,
        account: Option<&Account>,
        region: &str,
        slot: &str,
        known: Option<(u64, i64)>,
    ) -> Result<Option<MusicSnapshot<T>>> {
        let (profile, credential) = account.map_or(("", ""), |account| {
            (account.profile.id.as_str(), account.credential_ref.as_str())
        });
        let (known_time, known_generation) = known.map_or((None, None), |(time, generation)| {
            (Some(timestamp(time)), Some(generation))
        });
        let data: Option<(u64, Option<String>, i64)> = self.connect()?.query_row(
            "SELECT saved_at_ms, CASE WHEN saved_at_ms = ?6 AND generation = ?7 THEN NULL ELSE payload END,
                generation
             FROM music_snapshot CROSS JOIN catalog_cache_generation WHERE singleton=1 AND provider=?1 AND profile_id=?2
             AND region=?3 AND slot=?4 AND (?2='' OR EXISTS (
               SELECT 1 FROM account_source JOIN account USING(provider, profile_id)
               WHERE provider=?1 AND profile_id=?2 AND credential_ref=?5 AND NOT EXISTS (
                 SELECT 1 FROM music_revoked WHERE provider=?1 AND profile_id=?2 AND credential_ref=?5)))",
            params![provider.as_str(), profile, region, slot, credential, known_time, known_generation],
            |row| Ok((row.get::<_, i64>(0)?.max(0) as u64, row.get(1)?, row.get(2)?)),
        ).optional()?;
        match data {
            Some((saved_at_ms, Some(payload), generation)) => Ok(Some(MusicSnapshot::Changed {
                value: serde_json::from_str(&payload)?,
                saved_at_ms,
                encoded_bytes: payload.len(),
                generation,
            })),
            Some((_, None, _)) => Ok(Some(MusicSnapshot::Unchanged)),
            None => Ok(None),
        }
    }

    pub(crate) fn save_music_snapshot(
        &self,
        provider: ProviderId,
        account: Option<&Account>,
        region: &str,
        slot: &str,
        value: &impl Serialize,
    ) -> Result<bool> {
        Self::save_music_snapshot_in(&self.connect()?, provider, account, region, slot, value)
    }

    pub(super) fn save_music_snapshot_in(
        connection: &Connection,
        provider: ProviderId,
        account: Option<&Account>,
        region: &str,
        slot: &str,
        value: &impl Serialize,
    ) -> Result<bool> {
        Ok(
            Self::write_music_snapshot(connection, provider, account, region, slot, value)?
                .is_some(),
        )
    }

    pub(crate) fn catalog_generation(&self) -> Result<i64> {
        catalog_generation(&self.connect()?)
    }

    pub(crate) fn clear_catalog_snapshots(&self) -> Result<(usize, i64)> {
        let mut connection = self.connect()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let generation = tx.query_row(
            "UPDATE catalog_cache_generation SET generation=generation+1 WHERE singleton=1 RETURNING generation",
            [],
            |row| row.get(0),
        )?;
        let removed = tx.execute(
            &format!(
                "DELETE FROM music_snapshot WHERE {CATALOG_DETAIL_SLOTS} OR {CATALOG_PLAYLIST_SLOTS}"
            ),
            [],
        )?;
        tx.commit()?;
        Ok((removed, generation))
    }

    pub(crate) fn save_catalog_snapshot(
        &self,
        provider: ProviderId,
        account: Option<&Account>,
        region: &str,
        slot: &str,
        value: &impl Serialize,
        generation: i64,
    ) -> Result<CatalogSave> {
        let mut connection = self.connect()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if catalog_generation(&tx)? != generation {
            return Ok(CatalogSave::CacheCleared);
        }
        let saved = Self::write_music_snapshot(&tx, provider, account, region, slot, value)?;
        if saved.is_some() {
            if slot.starts_with("playlist:") {
                trim_playlist_snapshots(&tx)?;
            } else {
                trim_catalog_snapshots(
                    &tx,
                    CATALOG_DETAIL_SLOTS,
                    CATALOG_PAGE_LIMIT,
                    CATALOG_DISK_BYTES,
                )?;
            }
        }
        tx.commit()?;
        Ok(match saved {
            Some((saved_at_ms, encoded_bytes)) => CatalogSave::Stored {
                saved_at_ms,
                encoded_bytes,
            },
            None => CatalogSave::AccountChanged,
        })
    }

    fn write_music_snapshot(
        connection: &Connection,
        provider: ProviderId,
        account: Option<&Account>,
        region: &str,
        slot: &str,
        value: &impl Serialize,
    ) -> Result<Option<(u64, usize)>> {
        let (profile, credential) = account.map_or(("", ""), |account| {
            (account.profile.id.as_str(), account.credential_ref.as_str())
        });
        let data = serde_json::to_string(value)?;
        Ok(connection.query_row(
            "INSERT INTO music_snapshot (
                provider, profile_id, region, slot, payload, saved_at_ms, encoded_bytes)
             SELECT ?1, ?2, ?3, ?4, ?5, CAST(unixepoch('subsec') * 1000 AS INTEGER), ?7
             WHERE ?2='' OR EXISTS (
                SELECT 1 FROM account_source JOIN account USING(provider, profile_id)
                WHERE provider=?1 AND profile_id=?2 AND credential_ref=?6 AND NOT EXISTS (
                  SELECT 1 FROM music_revoked WHERE provider=?1 AND profile_id=?2 AND credential_ref=?6))
             ON CONFLICT(provider,profile_id,region,slot) DO UPDATE SET payload=excluded.payload,
                saved_at_ms=max(excluded.saved_at_ms, music_snapshot.saved_at_ms + 1),
                encoded_bytes=excluded.encoded_bytes
             RETURNING saved_at_ms",
            params![
                provider.as_str(), profile, region, slot, data, credential,
                i64::try_from(data.len()).unwrap_or(i64::MAX)
            ],
            |row| Ok((row.get::<_, i64>(0)?.max(0) as u64, data.len())),
        ).optional()?)
    }

    pub(crate) fn forget_music_account(&self, account: &Account) -> Result<bool> {
        let provider = account.provider.as_str();
        let id = &account.profile.id;
        let credential = &account.credential_ref;
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "INSERT INTO music_revoked (provider, profile_id, credential_ref)
             SELECT ?1, ?2, ?3 WHERE EXISTS (
               SELECT 1 FROM account_source JOIN account USING(provider, profile_id)
               WHERE provider=?1 AND profile_id=?2 AND credential_ref=?3)
             ON CONFLICT(provider,profile_id) DO UPDATE SET credential_ref=excluded.credential_ref
             WHERE music_revoked.credential_ref <> excluded.credential_ref",
            params![provider, id, credential],
        )?;
        if changed != 0 {
            delete_account_snapshots(&transaction, account.provider, id)?;
            transaction.execute(
                "DELETE FROM home_cache WHERE kind='account' AND provider=?1 AND profile_id=?2",
                params![provider, id],
            )?;
        }
        transaction.commit()?;
        if changed != 0 {
            telemetry::catalog_invalidated(CatalogInvalidation::CredentialRevoked);
        }
        Ok(changed != 0)
    }

    pub(crate) fn account_revoked(&self, account: &Account) -> Result<bool> {
        Ok(self.connect()?.query_row(
            "SELECT EXISTS (SELECT 1 FROM music_revoked
             WHERE provider=?1 AND profile_id=?2 AND credential_ref=?3)",
            params![
                account.provider.as_str(),
                account.profile.id,
                account.credential_ref
            ],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn music_match<T: DeserializeOwned>(
        &self,
        region: &str,
        object: &ObjectRef,
    ) -> Result<Option<T>> {
        let data: Option<String> = self.connect()?.query_row(
            "SELECT payload FROM music_match WHERE region=?1 AND provider=?2 AND kind=?3 AND object_id=?4",
            params![region, object.provider.as_str(), object.kind.path(), object.id],
            |row| row.get(0),
        ).optional()?.flatten();
        data.map(|payload| serde_json::from_str(&payload).map_err(Into::into))
            .transpose()
    }

    pub(crate) fn native_music_alias(
        &self,
        region: &str,
        link: &PublicLink,
    ) -> Result<Option<ObjectRef>> {
        let data: Option<String> = self.connect()?.query_row(
            "SELECT native_object FROM music_public_alias WHERE region=?1 AND platform=?2 AND kind=?3 AND object_id=?4",
            params![region, link.platform, link.kind.path(), link.id],
            |row| row.get(0),
        ).optional()?;
        data.map(|value| serde_json::from_str(&value).map_err(Into::into))
            .transpose()
    }

    pub(crate) fn native_music_aliases(
        &self,
        region: &str,
        kind: ObjectKind,
    ) -> Result<Vec<(ObjectRef, ObjectRef)>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT platform, object_id, native_object FROM music_public_alias
             WHERE region=?1 AND kind=?2",
        )?;
        let mut rows = statement.query(params![region, kind.path()])?;
        let mut aliases = Vec::new();
        while let Some(row) = rows.next()? {
            let platform: String = row.get(0)?;
            let Ok(provider) = super::provider_id(&platform) else {
                continue;
            };
            let alias = ObjectRef {
                provider,
                kind,
                id: row.get(1)?,
            };
            let stored: String = row.get(2)?;
            let native: ObjectRef = serde_json::from_str(&stored)?;
            if native.kind == kind && native != alias && alias.is_valid() && native.is_valid() {
                aliases.push((alias, native));
            }
        }
        Ok(aliases)
    }

    pub(crate) fn remember_music_aliases(
        &self,
        region: &str,
        source: &ObjectRef,
        links: &[ConfirmedLink],
    ) -> Result<()> {
        let data = serde_json::to_string(source)?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        for link in links {
            transaction.execute(
                "INSERT INTO music_public_alias(region,platform,kind,object_id,native_object) VALUES (?1,?2,?3,?4,?5)
                 ON CONFLICT(region,platform,kind,object_id) DO NOTHING",
                params![region, link.platform, source.kind.path(), link.id, data],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn claim_music_match(
        &self,
        region: &str,
        object: &ObjectRef,
        now: u64,
        until: u64,
    ) -> Result<bool> {
        let now = timestamp(now);
        let until = timestamp(until);
        Ok(self.connect()?.execute(
            "INSERT INTO music_match (region,provider,kind,object_id,retry_at) VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(region,provider,kind,object_id) DO UPDATE SET retry_at=excluded.retry_at
             WHERE music_match.retry_at <= ?6",
            params![region, object.provider.as_str(), object.kind.path(), object.id, until, now],
        )? != 0)
    }

    pub(crate) fn reopen_completed_music_match(
        &self,
        region: &str,
        object: &ObjectRef,
        missing: ProviderId,
        until: u64,
    ) -> Result<bool> {
        Ok(self.connect()?.execute(
            "UPDATE music_match SET retry_at=?5
             WHERE region=?1 AND provider=?2 AND kind=?3 AND object_id=?4 AND retry_at=?6
             AND NOT EXISTS (
                 SELECT 1 FROM json_each(music_match.payload, '$.links')
                 WHERE json_extract(value, '$.platform') = ?7
             )",
            params![
                region,
                object.provider.as_str(),
                object.kind.path(),
                object.id,
                timestamp(until),
                i64::MAX,
                missing.as_str()
            ],
        )? != 0)
    }

    pub(crate) fn save_music_match(
        &self,
        region: &str,
        objects: &[ObjectRef],
        lease: u64,
        retry_at: u64,
        payload: &impl Serialize,
    ) -> Result<()> {
        let lease = timestamp(lease);
        let retry_at = timestamp(retry_at);
        let Some((source, aliases)) = objects.split_first() else {
            return Ok(());
        };
        let payload = serde_json::to_string(payload)?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction()?;
        let written = transaction.execute(
            "UPDATE music_match SET retry_at=?5,payload=?6
             WHERE region=?1 AND provider=?2 AND kind=?3 AND object_id=?4 AND retry_at=?7",
            params![
                region,
                source.provider.as_str(),
                source.kind.path(),
                source.id,
                retry_at,
                payload,
                lease
            ],
        )?;
        if written != 0 {
            for alias in aliases {
                transaction.execute(
                    "INSERT INTO music_match (region,provider,kind,object_id,retry_at,payload)
                     VALUES (?1,?2,?3,?4,?5,?6)
                     ON CONFLICT(region,provider,kind,object_id) DO UPDATE SET
                       retry_at=excluded.retry_at,payload=excluded.payload
                     WHERE music_match.payload IS NULL OR music_match.retry_at < excluded.retry_at",
                    params![
                        region,
                        alias.provider.as_str(),
                        alias.kind.path(),
                        alias.id,
                        retry_at,
                        payload
                    ],
                )?;
            }
        }
        transaction.commit()?;
        Ok(())
    }
}

fn timestamp(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn catalog_generation(connection: &Connection) -> Result<i64> {
    Ok(connection.query_row(
        "SELECT generation FROM catalog_cache_generation WHERE singleton=1",
        [],
        |row| row.get(0),
    )?)
}

pub(super) fn trim_playlist_snapshots(connection: &Connection) -> Result<()> {
    trim_catalog_snapshots(
        connection,
        CATALOG_PLAYLIST_SLOTS,
        CATALOG_PLAYLIST_LIMIT,
        CATALOG_PLAYLIST_DISK_BYTES,
    )
}

fn trim_catalog_snapshots(
    connection: &Connection,
    slots: &str,
    max_pages: i64,
    max_bytes: i64,
) -> Result<()> {
    connection.execute(
        &format!(
            "WITH snapshots AS (
            SELECT rowid, saved_at_ms, encoded_bytes FROM music_snapshot
            WHERE {slots}
         ),
         ranked AS (
            SELECT rowid, ROW_NUMBER() OVER recent AS pages, SUM(encoded_bytes) OVER recent AS bytes
            FROM snapshots WHERE encoded_bytes <= ?2
            WINDOW recent AS (ORDER BY saved_at_ms DESC, rowid DESC)
         )
         DELETE FROM music_snapshot WHERE rowid IN (
            SELECT rowid FROM ranked WHERE pages > ?1 OR bytes > ?2
            UNION ALL SELECT rowid FROM snapshots WHERE encoded_bytes > ?2
         )"
        ),
        params![max_pages, max_bytes],
    )?;
    Ok(())
}

pub(super) fn delete_account_snapshots(
    transaction: &Transaction<'_>,
    provider: ProviderId,
    id: &str,
) -> Result<()> {
    transaction.execute(
        "DELETE FROM music_snapshot WHERE provider=?1 AND profile_id=?2",
        params![provider.as_str(), id],
    )?;
    Ok(())
}
