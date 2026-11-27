use rusqlite::{OptionalExtension, TransactionBehavior, params};

use super::{Result, Store, StoreError};

pub(super) const MIGRATION: &str = "CREATE TABLE artwork_palette (
    content_hash BLOB NOT NULL CHECK(length(content_hash) = 32),
    algorithm_version INTEGER NOT NULL,
    artwork_generation INTEGER NOT NULL,
    swatches BLOB NOT NULL CHECK(length(swatches) BETWEEN 5 AND 53),
    PRIMARY KEY (content_hash, algorithm_version)
) STRICT";

impl Store {
    pub(crate) fn artwork_palette(
        &self,
        content: &[u8; 32],
        version: i64,
        generation: u64,
    ) -> Result<Option<Vec<u8>>> {
        let generation = i64::try_from(generation).map_err(|_| StoreError::InvalidData)?;
        Ok(self.connect()?.query_row(
            "SELECT swatches FROM artwork_palette WHERE content_hash=?1 AND algorithm_version=?2 AND artwork_generation=?3",
            params![content, version, generation],
            |row| row.get(0),
        ).optional()?)
    }

    pub(crate) fn save_artwork_palette(
        &self,
        content: &[u8; 32],
        version: i64,
        generation: u64,
        bytes: &[u8],
        limit: i64,
    ) -> Result<()> {
        let generation = i64::try_from(generation).map_err(|_| StoreError::InvalidData)?;
        let mut connection = self.connect()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM artwork_palette WHERE artwork_generation<>?1",
            [generation],
        )?;
        tx.execute(
            "INSERT INTO artwork_palette (content_hash, algorithm_version, artwork_generation, swatches) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (content_hash, algorithm_version) DO UPDATE SET artwork_generation=excluded.artwork_generation, swatches=excluded.swatches",
            params![content, version, generation, bytes],
        )?;
        tx.execute(
            "DELETE FROM artwork_palette WHERE rowid IN (SELECT rowid FROM artwork_palette ORDER BY rowid DESC LIMIT -1 OFFSET ?1)",
            [limit],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn clear_artwork_palettes(&self) -> Result<()> {
        self.connect()?.execute("DELETE FROM artwork_palette", [])?;
        Ok(())
    }
}
