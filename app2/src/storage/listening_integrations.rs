use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Serialize, de::DeserializeOwned};

use crate::integrations::{Preferences, Service};

use super::{
    Result, Store, StoreError, cleanup_credentials, remove_credential, report_credential_cleanup,
    target_for_service, vault,
};

pub(super) const MIGRATION: &str = "
    CREATE TABLE listening_preferences (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        payload TEXT NOT NULL
    ) STRICT;
    CREATE TABLE listening_connection (
        service TEXT PRIMARY KEY CHECK (service IN ('lastfm', 'listenbrainz')),
        credential_ref TEXT NOT NULL
    ) STRICT;";

pub(super) fn is_listening_service(service: &str) -> bool {
    Service::ALL.into_iter().any(|value| value.id() == service)
}

impl Store {
    pub(crate) fn listening_preferences(&self) -> Result<Preferences> {
        let payload: Option<String> = self
            .connect()?
            .query_row("SELECT payload FROM listening_preferences", [], |row| {
                row.get(0)
            })
            .optional()?;
        Ok(payload
            .as_deref()
            .map(serde_json::from_str)
            .transpose()?
            .unwrap_or_default())
    }

    pub(crate) fn set_listening_preferences(&self, preferences: &Preferences) -> Result<()> {
        self.connect()?.execute(
            "INSERT INTO listening_preferences (singleton, payload) VALUES (1, ?1)
             ON CONFLICT (singleton) DO UPDATE SET payload = excluded.payload",
            [serde_json::to_string(preferences)?],
        )?;
        Ok(())
    }

    pub(crate) fn listening_account<T: DeserializeOwned>(
        &self,
        service: Service,
    ) -> Result<Option<T>> {
        let mut database = self.connect()?;
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let reference: Option<String> = tx
            .query_row(
                "SELECT credential_ref FROM listening_connection WHERE service = ?1",
                [service.id()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(reference) = reference else {
            return Ok(None);
        };
        let target = target_for_service(service.id(), &reference)?;
        let bytes = vault::read(&target)?.ok_or(StoreError::InvalidData)?;
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    pub(crate) fn set_listening_account<T: Serialize>(
        &self,
        service: Service,
        account: Option<&T>,
    ) -> Result<()> {
        let mut database = self.connect()?;
        let tx = database.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO credential_cleanup (provider, credential_ref)
             SELECT service, credential_ref FROM listening_connection WHERE service = ?1",
            [service.id()],
        )?;
        let new = if let Some(account) = account {
            let reference: String =
                tx.query_row("SELECT hex(randomblob(16))", [], |row| row.get(0))?;
            let target = target_for_service(service.id(), &reference)?;
            vault::write(&target, &serde_json::to_vec(account)?)?;
            Some((reference, target))
        } else {
            None
        };
        let result = (|| -> Result<()> {
            if let Some((reference, _)) = &new {
                tx.execute(
                    "INSERT INTO listening_connection (service, credential_ref) VALUES (?1, ?2)
                     ON CONFLICT (service) DO UPDATE SET credential_ref = excluded.credential_ref",
                    params![service.id(), reference],
                )?;
            } else {
                tx.execute(
                    "DELETE FROM listening_connection WHERE service = ?1",
                    [service.id()],
                )?;
            }
            tx.commit()?;
            Ok(())
        })();
        if result.is_ok() {
            report_credential_cleanup(cleanup_credentials(&database));
        } else if let Some((_, target)) = new {
            remove_credential(&target);
        }
        result
    }
}
