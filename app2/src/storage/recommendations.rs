use rusqlite::params;

use crate::music::{likes::LikeIdentity, recommendations::Reaction};

use super::{Result, Store, StoreError};

pub(super) const MIGRATION: &str = "CREATE TABLE recommendation_reaction (
        ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
        identity TEXT NOT NULL UNIQUE,
        reaction TEXT NOT NULL CHECK (reaction IN ('more', 'hide'))
    ) STRICT;";

pub(super) const ARTIST_MIGRATION: &str = "ALTER TABLE recommendation_reaction
    ADD COLUMN artist TEXT CHECK (artist IS NULL OR (reaction = 'more' AND length(artist) > 0));";

impl Store {
    pub(crate) fn write_recommendation_reaction(
        &self,
        identity: &LikeIdentity,
        reaction: Reaction,
        artist: Option<&str>,
    ) -> Result<()> {
        let value = match reaction {
            Reaction::MoreLikeThis => "more",
            Reaction::Hide => "hide",
        };
        // Each known identity keeps only its latest action. A new ordinal also
        // preserves write order if different identities are later linked.
        self.connect()?.execute(
            "INSERT OR REPLACE INTO recommendation_reaction (identity, reaction, artist)
             VALUES (?1, ?2, ?3)",
            params![serde_json::to_string(identity)?, value, artist],
        )?;
        Ok(())
    }

    pub(crate) fn recommendation_reactions(
        &self,
    ) -> Result<Vec<(LikeIdentity, Reaction, Option<String>)>> {
        let connection = self.connect()?;
        let mut query = connection.prepare(
            "SELECT identity, reaction, artist FROM recommendation_reaction ORDER BY ordinal",
        )?;
        query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get(2)?,
                ))
            })?
            .map(|row| {
                let (identity, value, artist) = row?;
                let reaction = match value.as_str() {
                    "more" => Reaction::MoreLikeThis,
                    "hide" => Reaction::Hide,
                    _ => return Err(StoreError::InvalidData),
                };
                Ok((serde_json::from_str(&identity)?, reaction, artist))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::MIGRATIONS;

    #[test]
    fn adding_the_artist_credit_preserves_an_existing_reaction() {
        let directory = tempfile::tempdir().unwrap();
        let previous = MIGRATIONS
            .iter()
            .position(|migration| migration.contains(&ARTIST_MIGRATION))
            .unwrap();
        let store = Store::versioned_fixture(directory.path(), previous).unwrap();
        let identity = LikeIdentity {
            provider: None,
            profile: None,
            id: "saved-recording".into(),
        };
        let connection = store.connect().unwrap();
        connection
            .execute(
                "INSERT INTO recommendation_reaction (identity, reaction) VALUES (?1, 'more')",
                [serde_json::to_string(&identity).unwrap()],
            )
            .unwrap();
        drop(connection);
        drop(store);

        let reopened = Store::open(directory.path()).unwrap();
        assert_eq!(
            reopened.recommendation_reactions().unwrap(),
            [(identity, Reaction::MoreLikeThis, None)]
        );
    }
}
