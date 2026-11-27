use rusqlite::OptionalExtension;

use super::{Result, Store};

pub(super) const MIGRATION: &str = "CREATE TABLE workspace_route (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    payload TEXT NOT NULL
) STRICT;";

impl Store {
    pub(crate) fn local_playlist_exists(&self, id: i64) -> Result<bool> {
        Ok(self.connect()?.query_row(
            "SELECT EXISTS(SELECT 1 FROM local_playlist WHERE id = ?1)",
            [id],
            |row| row.get(0),
        )?)
    }

    pub(crate) fn workspace_route(&self) -> Result<Option<String>> {
        Ok(self
            .connect()?
            .query_row(
                "SELECT payload FROM workspace_route WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()?)
    }

    pub(crate) fn save_workspace_route(&self, route: &str) -> Result<()> {
        self.connect()?.execute(
            "INSERT INTO workspace_route(id, payload) VALUES(1, ?1)
             ON CONFLICT(id) DO UPDATE SET payload = excluded.payload",
            [route],
        )?;
        Ok(())
    }
}
