use std::{
    collections::HashSet,
    sync::atomic::{AtomicBool, Ordering},
};

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::{
    music::likes::{LikeIdentity, timestamp_ms},
    playlist::PlaylistTrack,
    providers::ProviderId,
};

use super::{Account, FavoriteScope, Result, Store, StoreError, provider_id};

pub(super) const MIGRATION: &str = "
CREATE TABLE liked_local_record (
    identity TEXT PRIMARY KEY NOT NULL,
    payload TEXT NOT NULL
) STRICT;
CREATE TABLE liked_remote_change (
    provider TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    track_id TEXT NOT NULL,
    written_at_ms INTEGER NOT NULL,
    payload TEXT,
    PRIMARY KEY (provider, profile_id, track_id),
    FOREIGN KEY (provider, profile_id) REFERENCES account (provider, profile_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE confirmed_track_link (
    source TEXT NOT NULL,
    provider TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    target TEXT NOT NULL,
    PRIMARY KEY (source, provider, profile_id),
    FOREIGN KEY (provider, profile_id) REFERENCES account (provider, profile_id) ON DELETE CASCADE
) STRICT;
CREATE TABLE like_destinations (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    payload TEXT NOT NULL
) STRICT;";

// Collection reads on some services remain stale briefly after a confirmed write.
const REMOTE_SETTLE_MS: i64 = 120_000;

impl Store {
    pub(crate) fn like_defaults(&self) -> Result<Vec<Option<ProviderId>>> {
        let settings: Option<String> = self
            .connect()?
            .query_row(
                "SELECT payload FROM like_destinations WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()?;
        match settings {
            Some(settings) => Ok(serde_json::from_str(&settings)?),
            None => Ok(vec![None]),
        }
    }

    pub(crate) fn set_like_defaults(&self, defaults: &[Option<ProviderId>]) -> Result<()> {
        self.connect()?.execute(
            "INSERT INTO like_destinations(id,payload) VALUES(1,?1)
             ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",
            [serde_json::to_string(defaults)?],
        )?;
        Ok(())
    }

    pub(crate) fn local_liked_tracks(&self) -> Result<Vec<PlaylistTrack>> {
        let connection = self.connect()?;
        let mut query = connection.prepare("SELECT payload FROM liked_local_record")?;
        query
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }

    pub(crate) fn local_liked_track(
        &self,
        identity: &LikeIdentity,
    ) -> Result<(String, Option<PlaylistTrack>)> {
        let key = serde_json::to_string(identity)?;
        let id = saved_local_id(identity, &key);
        let payload: Option<String> = self
            .connect()?
            .query_row(
                "SELECT payload FROM liked_local_record WHERE identity=?1",
                [&key],
                |row| row.get(0),
            )
            .optional()?;
        let track = payload
            .map(|payload| serde_json::from_str(&payload))
            .transpose()?;
        Ok((id, track))
    }

    pub(crate) fn local_like_identities(&self) -> Result<HashSet<LikeIdentity>> {
        let connection = self.connect()?;
        let mut modern = connection.prepare("SELECT identity FROM liked_local_record")?;
        let mut identities = modern
            .query_map([], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect::<Result<HashSet<_>>>()?;
        let mut legacy = connection.prepare(
            "SELECT favorite.provider, favorite.profile_id, favorite.track_id
             FROM favorite_track favorite
             LEFT JOIN account_source active USING (provider, profile_id)
             WHERE favorite.kind <> 'account' OR active.provider IS NOT NULL",
        )?;
        let mut rows = legacy.query([])?;
        while let Some(row) = rows.next()? {
            let provider: String = row.get(0)?;
            let profile: String = row.get(1)?;
            identities.insert(LikeIdentity {
                provider: (!provider.is_empty())
                    .then(|| provider_id(&provider))
                    .transpose()?,
                profile: (!profile.is_empty()).then_some(profile),
                id: row.get(2)?,
            });
        }
        Ok(identities)
    }

    pub(crate) fn local_like(&self, identity: &LikeIdentity) -> Result<bool> {
        let (kind, provider, profile) = legacy_scope(identity).key();
        let saved = self.connect()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM liked_local_record WHERE identity=?1)
             OR EXISTS(SELECT 1 FROM favorite_track
                       WHERE kind=?2 AND provider=?3 AND profile_id=?4 AND track_id=?5)",
            params![
                serde_json::to_string(identity)?,
                kind,
                provider,
                profile,
                identity.id
            ],
            |row| row.get(0),
        )?;
        Ok(saved)
    }

    pub(crate) fn local_like_ids(&self, scope: FavoriteScope<'_>) -> Result<HashSet<String>> {
        let provider = match scope {
            FavoriteScope::Local => None,
            FavoriteScope::Guest(provider) | FavoriteScope::Account(provider, _) => Some(provider),
        };
        let (_, _, profile) = scope.key();
        let mut ids = self.favorite_tracks(scope)?;
        let connection = self.connect()?;
        let mut query = connection.prepare(
            "SELECT json_extract(identity,'$.id') FROM liked_local_record
             WHERE json_extract(identity,'$.provider') IS json_extract(?1,'$')
               AND json_extract(identity,'$.profile') IS NULLIF(?2,'')",
        )?;
        for id in query.query_map(params![serde_json::to_string(&provider)?, profile], |row| {
            row.get::<_, String>(0)
        })? {
            ids.insert(id?);
        }
        Ok(ids)
    }

    pub(crate) fn set_local_like(
        &self,
        identity: &LikeIdentity,
        track: &PlaylistTrack,
        liked: bool,
        cancelled: &AtomicBool,
    ) -> Result<()> {
        let key = serde_json::to_string(identity)?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if cancelled.load(Ordering::Relaxed) {
            return Ok(());
        }
        if liked {
            let mut track = track.clone();
            track.id = saved_local_id(identity, &key);
            track.added = Some(chrono::Utc::now().to_rfc3339());
            transaction.execute(
                "INSERT INTO liked_local_record(identity,payload) VALUES(?1,?2)
                 ON CONFLICT(identity) DO NOTHING",
                params![key, serde_json::to_string(&track)?],
            )?;
            if identity.provider.is_none() {
                transaction.execute(
                    "INSERT INTO favorite_track(kind,provider,profile_id,track_id)
                     VALUES('local','','',?1) ON CONFLICT DO NOTHING",
                    [&identity.id],
                )?;
            }
        } else {
            transaction.execute("DELETE FROM liked_local_record WHERE identity=?1", [&key])?;
            let (kind, provider, profile) = legacy_scope(identity).key();
            transaction.execute(
                "DELETE FROM favorite_track WHERE kind=?1 AND provider=?2 AND profile_id=?3 AND track_id=?4",
                params![kind, provider, profile, identity.id],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub(crate) fn remember_track_confirmation(
        &self,
        source: &LikeIdentity,
        target: &LikeIdentity,
        account: &Account,
    ) -> Result<()> {
        let changed = self.connect()?.execute(
            "INSERT INTO confirmed_track_link(source,provider,profile_id,target)
             SELECT ?1,provider,profile_id,?4 FROM account_source JOIN account USING(provider,profile_id)
             WHERE provider=?2 AND profile_id=?3 AND credential_ref=?5
             ON CONFLICT(source,provider,profile_id) DO UPDATE SET target=excluded.target",
            params![
                serde_json::to_string(source)?, account.provider.as_str(),
                account.profile.id, serde_json::to_string(target)?, account.credential_ref,
            ],
        )?;
        (changed == 1).then_some(()).ok_or(StoreError::InvalidData)
    }

    pub(crate) fn confirmed_track_pairs(&self) -> Result<Vec<(LikeIdentity, LikeIdentity)>> {
        let connection = self.connect()?;
        let mut query = connection.prepare("SELECT source,target FROM confirmed_track_link")?;
        query
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .map(|row| {
                let (source, target) = row?;
                Ok((
                    serde_json::from_str(&source)?,
                    serde_json::from_str(&target)?,
                ))
            })
            .collect()
    }

    pub(crate) fn record_remote_like(
        &self,
        account: &Account,
        track: &PlaylistTrack,
        liked: bool,
    ) -> Result<PlaylistTrack> {
        let mut track = track.clone();
        track.added = Some(chrono::Utc::now().to_rfc3339());
        let payload = liked.then(|| serde_json::to_string(&track)).transpose()?;
        let changed = self.connect()?.execute(
            "INSERT INTO liked_remote_change(provider,profile_id,track_id,written_at_ms,payload)
             SELECT provider,profile_id,?3,?4,?5 FROM account_source JOIN account USING(provider,profile_id)
             WHERE provider=?1 AND profile_id=?2 AND credential_ref=?6 AND NOT EXISTS (
                 SELECT 1 FROM music_revoked WHERE provider=?1 AND profile_id=?2 AND credential_ref=?6)
             ON CONFLICT(provider,profile_id,track_id) DO UPDATE SET
                 written_at_ms=excluded.written_at_ms, payload=excluded.payload",
            params![account.provider.as_str(), account.profile.id, track.id, timestamp_ms(), payload, account.credential_ref],
        )?;
        (changed == 1)
            .then_some(track)
            .ok_or(StoreError::InvalidData)
    }

    pub(crate) fn remote_like_override(&self, account: &Account, id: &str) -> Result<Option<bool>> {
        let payload: Option<Option<String>> = self.connect()?.query_row(
            "SELECT payload FROM liked_remote_change JOIN account_source USING(provider,profile_id)
             JOIN account USING(provider,profile_id)
             WHERE provider=?1 AND profile_id=?2 AND track_id=?3 AND credential_ref=?4
             AND NOT EXISTS (
                SELECT 1 FROM music_revoked WHERE provider=?1 AND profile_id=?2 AND credential_ref=?4)",
            params![account.provider.as_str(), account.profile.id, id, account.credential_ref],
            |row| row.get(0),
        ).optional()?;
        Ok(payload.map(|value| value.is_some()))
    }

    pub(crate) fn apply_liked_changes(
        &self,
        provider: ProviderId,
        profile: &str,
        tracks: &mut Vec<PlaylistTrack>,
    ) -> Result<()> {
        Self::apply_liked_changes_in(&self.connect()?, provider, profile, tracks)
    }

    fn apply_liked_changes_in(
        connection: &rusqlite::Connection,
        provider: ProviderId,
        profile: &str,
        tracks: &mut Vec<PlaylistTrack>,
    ) -> Result<()> {
        let mut query = connection.prepare(
            "SELECT track_id,payload FROM liked_remote_change JOIN account_source USING(provider,profile_id)
             JOIN account USING(provider,profile_id)
             WHERE provider=?1 AND profile_id=?2 AND NOT EXISTS (
                SELECT 1 FROM music_revoked
                WHERE provider=?1 AND profile_id=?2 AND credential_ref=account.credential_ref)",
        )?;
        let changes = query.query_map(params![provider.as_str(), profile], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;
        let mut removed = HashSet::new();
        let mut added = Vec::new();
        for change in changes {
            let (id, payload) = change?;
            if let Some(payload) = payload {
                added.push((id, serde_json::from_str::<PlaylistTrack>(&payload)?));
            } else {
                removed.insert(id);
            }
        }
        tracks.retain(|track| !removed.contains(&track.id));
        let current = tracks
            .iter()
            .map(|track| track.id.clone())
            .collect::<HashSet<_>>();
        tracks.extend(
            added
                .into_iter()
                .filter(|(id, _)| !current.contains(id))
                .map(|(_, track)| track),
        );
        Ok(())
    }

    pub(crate) fn save_liked_snapshot(
        &self,
        account: &Account,
        region: &str,
        slot: &str,
        read_started_ms: i64,
        collection: &mut crate::providers::collections::Collection,
    ) -> Result<bool> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let provider = account.provider;
        let profile = &account.profile.id;
        if !Self::save_music_snapshot_in(
            &transaction,
            provider,
            Some(account),
            region,
            slot,
            collection,
        )? {
            return Ok(false);
        }
        let crate::providers::collections::Collection::Liked(tracks) = collection else {
            return Err(StoreError::InvalidData);
        };
        // A newer like survives when it races a remote read.
        transaction.execute(
            "DELETE FROM liked_remote_change WHERE provider=?1 AND profile_id=?2 AND written_at_ms < ?3",
            params![provider.as_str(), profile, read_started_ms.saturating_sub(REMOTE_SETTLE_MS)],
        )?;
        Self::apply_liked_changes_in(&transaction, provider, profile, tracks)?;
        transaction.commit()?;
        Ok(true)
    }
}

fn legacy_scope(identity: &LikeIdentity) -> FavoriteScope<'_> {
    match (identity.provider, identity.profile.as_deref()) {
        (None, _) => FavoriteScope::Local,
        (Some(provider), Some(profile)) => FavoriteScope::Account(provider, profile),
        (Some(provider), None) => FavoriteScope::Guest(provider),
    }
}

fn saved_local_id(identity: &LikeIdentity, serialized: &str) -> String {
    if identity.provider.is_some() {
        format!("liked:{serialized}")
    } else {
        identity.id.clone()
    }
}

pub(super) fn delete_account_like_links(
    connection: &Connection,
    provider: ProviderId,
    profile: &str,
) -> Result<()> {
    connection.execute(
        "DELETE FROM confirmed_track_link WHERE json_extract(source,'$.provider')=json_extract(?1,'$')
         AND json_extract(source,'$.profile')=?2",
        params![serde_json::to_string(&provider)?, profile],
    )?;
    Ok(())
}
