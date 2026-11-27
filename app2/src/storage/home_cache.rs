use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::providers::{Home, ProviderId};

use super::{Result, Store, commit_if_current};

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum CacheScope<'a> {
    Guest,
    Account(ProviderId, &'a str),
}

impl<'a> CacheScope<'a> {
    fn key(self) -> (&'static str, &'static str, &'a str) {
        match self {
            Self::Guest => ("guest", ProviderId::YouTubeMusic.as_str(), ""),
            Self::Account(provider, profile_id) => ("account", provider.as_str(), profile_id),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Snapshot {
    pub home: Home,
    pub received_at_ms: i64,
}

impl Snapshot {
    pub(crate) fn received(home: Home) -> Self {
        Self {
            home,
            received_at_ms: unix_time_ms(),
        }
    }

    pub(crate) fn is_recent(&self, max_age: Duration) -> bool {
        let now = unix_time_ms();
        let minimum = now.saturating_sub(max_age.as_millis().min(i64::MAX as u128) as i64);
        self.received_at_ms > 0 && (minimum..=now).contains(&self.received_at_ms)
    }
}

impl Store {
    pub(crate) fn cache(&self, scope: CacheScope<'_>) -> Result<Option<Snapshot>> {
        read_cache(&self.connect()?, scope)
    }

    pub(crate) fn save_session_cache<T>(
        &self,
        scope: CacheScope<'_>,
        snapshot: &Snapshot,
        permit: impl FnOnce() -> Option<T>,
    ) -> Result<()> {
        self.save_cache_inner(scope, snapshot, true, permit)
    }

    // Fixtures exercise cache isolation even for profiles that have not signed in.
    #[cfg(test)]
    pub(crate) fn save_cache<T>(
        &self,
        scope: CacheScope<'_>,
        snapshot: &Snapshot,
        permit: impl FnOnce() -> Option<T>,
    ) -> Result<()> {
        self.save_cache_inner(scope, snapshot, false, permit)
    }

    fn save_cache_inner<T>(
        &self,
        scope: CacheScope<'_>,
        snapshot: &Snapshot,
        require_connection: bool,
        permit: impl FnOnce() -> Option<T>,
    ) -> Result<()> {
        let payload = serde_json::to_string(&snapshot.home)?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        write_cache(&transaction, scope, snapshot, &payload, require_connection)?;
        commit_if_current(transaction, permit)?;
        Ok(())
    }

    pub(crate) fn merge_session_cache<T, R>(
        &self,
        scope: CacheScope<'_>,
        merge: impl FnOnce(Option<&Home>) -> (Home, R),
        permit: impl FnOnce() -> Option<T>,
    ) -> Result<(Snapshot, R)> {
        let mut connection = self.connect()?;
        // Serialize the read and write so overlapping partial updates keep each other's shelves.
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = read_cache(&transaction, scope)?;
        let (home, result) = merge(previous.as_ref().map(|snapshot| &snapshot.home));
        let snapshot = Snapshot::received(home);
        let payload = serde_json::to_string(&snapshot.home)?;
        write_cache(&transaction, scope, &snapshot, &payload, true)?;
        commit_if_current(transaction, permit)?;
        Ok((snapshot, result))
    }
}

fn read_cache(connection: &Connection, scope: CacheScope<'_>) -> Result<Option<Snapshot>> {
    let (kind, provider, profile_id) = scope.key();
    let snapshot = connection
        .query_row(
            "SELECT received_at_ms, payload FROM home_cache
             WHERE kind = ?1 AND provider = ?2 AND profile_id = ?3",
            params![kind, provider, profile_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    snapshot
        .map(|(received_at_ms, payload)| {
            Ok(Snapshot {
                home: serde_json::from_str(&payload)?,
                received_at_ms,
            })
        })
        .transpose()
}

fn write_cache(
    connection: &Connection,
    scope: CacheScope<'_>,
    snapshot: &Snapshot,
    payload: &str,
    require_connection: bool,
) -> Result<()> {
    let (kind, provider, profile_id) = scope.key();
    connection.execute(
        "INSERT INTO home_cache (kind, provider, profile_id, received_at_ms, payload)
         SELECT ?1, ?2, ?3, ?4, ?5
         WHERE NOT ?6 OR CASE WHEN ?1 = 'guest' THEN NOT EXISTS (
             SELECT 1 FROM account_source WHERE provider = ?2
         ) ELSE EXISTS (
             SELECT 1 FROM account_source JOIN account USING (provider, profile_id)
             WHERE provider = ?2 AND profile_id = ?3 AND NOT EXISTS (
                 SELECT 1 FROM music_revoked
                 WHERE provider = ?2 AND profile_id = ?3 AND credential_ref = account.credential_ref
             )
         ) END
         ON CONFLICT (kind, provider, profile_id) DO UPDATE SET
            received_at_ms = excluded.received_at_ms, payload = excluded.payload
         WHERE excluded.received_at_ms >= home_cache.received_at_ms",
        params![
            kind,
            provider,
            profile_id,
            snapshot.received_at_ms,
            payload,
            require_connection
        ],
    )?;
    Ok(())
}

fn unix_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_millis().min(i64::MAX as u128) as i64)
}
