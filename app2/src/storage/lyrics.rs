use rusqlite::{OptionalExtension as _, params};

use crate::music::lyrics::LyricsVersions;

use super::{Account, Result, Store};

pub(super) const MIGRATION: &str = "
CREATE TABLE lyrics_snapshot (
    key TEXT NOT NULL,
    provider TEXT NOT NULL,
    profile_id TEXT NOT NULL,
    credential_ref TEXT NOT NULL,
    refresh_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    payload TEXT NOT NULL,
    PRIMARY KEY (key, provider, profile_id, credential_ref)
) STRICT;
CREATE INDEX lyrics_snapshot_expiry ON lyrics_snapshot (expires_at);
CREATE TRIGGER remove_disconnected_lyrics AFTER DELETE ON account
BEGIN
    DELETE FROM lyrics_snapshot
    WHERE provider = old.provider AND profile_id = old.profile_id;
END;
CREATE TRIGGER remove_replaced_lyrics AFTER UPDATE OF credential_ref ON account
WHEN old.credential_ref <> new.credential_ref
BEGIN
    DELETE FROM lyrics_snapshot
    WHERE provider = old.provider AND profile_id = old.profile_id;
END;";

pub(crate) struct LyricsSnapshot {
    pub versions: LyricsVersions,
    pub fresh: bool,
}

impl Store {
    pub(crate) fn lyrics_snapshot(
        &self,
        key: &str,
        account: Option<&Account>,
        now: i64,
    ) -> Result<Option<LyricsSnapshot>> {
        let (provider, profile, credential) = scope(account);
        let data: Option<(String, bool)> = self.connect()?.query_row(
            "SELECT payload, refresh_at > ?5 FROM lyrics_snapshot
             WHERE key=?1 AND provider=?2 AND profile_id=?3 AND credential_ref=?4 AND expires_at > ?5
             AND (?2='' OR EXISTS (
                 SELECT 1 FROM account_source JOIN account USING(provider, profile_id)
                 WHERE provider=?2 AND profile_id=?3 AND credential_ref=?4 AND NOT EXISTS (
                     SELECT 1 FROM music_revoked
                     WHERE provider=?2 AND profile_id=?3 AND credential_ref=?4)))",
            params![key, provider, profile, credential, now],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        data.map(|(payload, fresh)| {
            Ok(LyricsSnapshot {
                versions: serde_json::from_str(&payload)?,
                fresh,
            })
        })
        .transpose()
    }

    pub(crate) fn save_lyrics_snapshot(
        &self,
        key: &str,
        account: Option<&Account>,
        now: i64,
        max_age: i64,
        versions: &LyricsVersions,
    ) -> Result<bool> {
        let (provider, profile, credential) = scope(account);
        let expires_at = now.saturating_add(max_age);
        let refresh_at = expires_at.min(now.saturating_add(24 * 60 * 60));
        let payload = serde_json::to_string(versions)?;
        Ok(self.connect()?.execute(
            "INSERT INTO lyrics_snapshot
                 (key, provider, profile_id, credential_ref, refresh_at, expires_at, payload)
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7 WHERE ?2='' OR EXISTS (
                 SELECT 1 FROM account_source JOIN account USING(provider, profile_id)
                 WHERE provider=?2 AND profile_id=?3 AND credential_ref=?4 AND NOT EXISTS (
                     SELECT 1 FROM music_revoked
                     WHERE provider=?2 AND profile_id=?3 AND credential_ref=?4))
             ON CONFLICT(key, provider, profile_id, credential_ref) DO UPDATE SET
                 refresh_at=excluded.refresh_at, expires_at=excluded.expires_at, payload=excluded.payload",
            params![key, provider, profile, credential, refresh_at, expires_at, payload],
        )? != 0)
    }

    pub(crate) fn purge_expired_lyrics(&self, now: i64) -> Result<()> {
        let connection = self.connect()?;
        connection.execute("DELETE FROM lyrics_snapshot WHERE expires_at <= ?1", [now])?;
        Ok(())
    }

    pub(crate) fn lyrics_account_is_current(&self, account: &Account) -> Result<bool> {
        let (provider, profile, credential) = scope(Some(account));
        Ok(self.connect()?.query_row(
            "SELECT EXISTS (
                 SELECT 1 FROM account_source JOIN account USING(provider, profile_id)
                 WHERE provider=?1 AND profile_id=?2 AND credential_ref=?3 AND NOT EXISTS (
                     SELECT 1 FROM music_revoked
                     WHERE provider=?1 AND profile_id=?2 AND credential_ref=?3))",
            params![provider, profile, credential],
            |row| row.get(0),
        )?)
    }
}

fn scope(account: Option<&Account>) -> (&str, &str, &str) {
    account.map_or(("", "", ""), |account| {
        (
            account.provider.as_str(),
            &account.profile.id,
            &account.credential_ref,
        )
    })
}

#[cfg(test)]
mod tests {
    use crate::music::lyrics::{Lyrics, Source, parse_lrc};

    use super::*;

    #[test]
    fn lyrics_snapshot_survives_reopen_and_is_bounded() {
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(profile.path()).unwrap();
        let lyrics = Lyrics::plain(Source::Lrclib, "Контрольная строка");
        let limit = 2 * 24 * 60 * 60;
        assert!(
            store
                .save_lyrics_snapshot("version A", None, 1, limit, &lyrics.clone().into())
                .unwrap()
        );
        drop(store);
        let store = Store::open(profile.path()).unwrap();
        let cached = store
            .lyrics_snapshot("version A", None, 2)
            .unwrap()
            .unwrap();
        assert_eq!(cached.versions.preferred(), &lyrics);
        assert!(cached.fresh);
        assert!(
            !store
                .lyrics_snapshot("version A", None, limit)
                .unwrap()
                .unwrap()
                .fresh
        );
        assert!(
            store
                .lyrics_snapshot("version B", None, 2)
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .lyrics_snapshot("version A", None, limit + 1)
                .unwrap()
                .is_none()
        );
        store.purge_expired_lyrics(limit + 1).unwrap();
    }

    #[test]
    fn lyrics_snapshot_keeps_both_versions_and_reads_the_legacy_shape() {
        let profile = tempfile::tempdir().unwrap();
        let store = Store::open(profile.path()).unwrap();
        let synchronized = parse_lrc(Source::Lrclib, "[00:02.50]Строка с меткой").unwrap();
        let plain = Lyrics::plain(Source::Lrclib, "Отдельная строка без метки");
        let legacy = serde_json::to_string(&synchronized).unwrap();
        store
            .connect()
            .unwrap()
            .execute(
                "INSERT INTO lyrics_snapshot VALUES ('old','','','',10,20,?1)",
                [legacy],
            )
            .unwrap();
        let restored = store
            .lyrics_snapshot("old", None, 2)
            .unwrap()
            .unwrap()
            .versions;
        assert_eq!(restored.preferred(), &synchronized);
        assert!(restored.alternate_plain().is_none());

        let versions = LyricsVersions::from_source(synchronized.clone(), Some(plain.clone()));
        assert!(
            store
                .save_lyrics_snapshot("both", None, 2, 20, &versions)
                .unwrap()
        );
        drop(store);
        let reopened = Store::open(profile.path()).unwrap();
        let restored = reopened
            .lyrics_snapshot("both", None, 3)
            .unwrap()
            .unwrap()
            .versions;
        assert_eq!(restored.preferred(), &synchronized);
        assert_eq!(restored.alternate_plain(), Some(&plain));
        let payload: String = reopened
            .connect()
            .unwrap()
            .query_row(
                "SELECT payload FROM lyrics_snapshot WHERE key='both'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Lyrics>(&payload).unwrap(),
            synchronized
        );
    }
}
