use rusqlite::{TransactionBehavior, params};

use crate::providers::ProviderId;

use super::{Result, Store, StoreError, provider_id};

pub(super) const MIGRATION: &str = "CREATE TABLE pinned_object (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        kind TEXT NOT NULL CHECK (kind IN ('playlist', 'album', 'artist')),
        provider TEXT NOT NULL,
        profile_id TEXT NOT NULL,
        object_id TEXT NOT NULL CHECK (length(object_id) > 0),
        local_playlist_id INTEGER REFERENCES local_playlist(id) ON DELETE CASCADE,
        title TEXT,
        artwork TEXT,
        link TEXT,
        position INTEGER NOT NULL CHECK (position >= 0),
        UNIQUE (kind, provider, profile_id, object_id),
        CHECK ((provider = '' AND kind = 'playlist' AND local_playlist_id IS NOT NULL
                AND title IS NULL) OR
               (provider <> '' AND local_playlist_id IS NULL AND title IS NOT NULL
                AND length(trim(title)) > 0))
    ) STRICT;
    CREATE INDEX pinned_object_position ON pinned_object (position);
    CREATE TRIGGER pinned_object_close_gap AFTER DELETE ON pinned_object
    BEGIN
        UPDATE pinned_object SET position = position - 1 WHERE position > OLD.position;
    END;";

// SQLite cannot extend the kind constraint in place. Preserve row IDs and the
// AUTOINCREMENT sequence when rebuilding the table in the profile transaction.
pub(super) const STATION_MIGRATION: &str = "
    ALTER TABLE pinned_object RENAME TO pinned_object_v11;
    CREATE TABLE pinned_object (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        kind TEXT NOT NULL CHECK (kind IN ('playlist', 'album', 'artist', 'station')),
        provider TEXT NOT NULL,
        profile_id TEXT NOT NULL,
        object_id TEXT NOT NULL CHECK (length(object_id) > 0),
        local_playlist_id INTEGER REFERENCES local_playlist(id) ON DELETE CASCADE,
        title TEXT,
        artwork TEXT,
        link TEXT,
        position INTEGER NOT NULL CHECK (position >= 0),
        UNIQUE (kind, provider, profile_id, object_id),
        CHECK ((provider = '' AND kind = 'playlist' AND local_playlist_id IS NOT NULL
                AND title IS NULL) OR
               (provider <> '' AND local_playlist_id IS NULL AND title IS NOT NULL
                AND length(trim(title)) > 0))
    ) STRICT;
    INSERT INTO pinned_object
        (id, kind, provider, profile_id, object_id, local_playlist_id, title, artwork, link, position)
    SELECT id, kind, provider, profile_id, object_id, local_playlist_id, title, artwork, link, position
    FROM pinned_object_v11;
    UPDATE sqlite_sequence SET seq = MAX(seq, COALESCE(
        (SELECT seq FROM sqlite_sequence WHERE name = 'pinned_object_v11'), seq
    )) WHERE name = 'pinned_object';
    INSERT INTO sqlite_sequence (name, seq)
    SELECT 'pinned_object', seq FROM sqlite_sequence WHERE name = 'pinned_object_v11'
        AND NOT EXISTS (SELECT 1 FROM sqlite_sequence WHERE name = 'pinned_object');
    DROP TABLE pinned_object_v11;
    CREATE INDEX pinned_object_position ON pinned_object (position);
    CREATE TRIGGER pinned_object_close_gap AFTER DELETE ON pinned_object
    BEGIN
        UPDATE pinned_object SET position = position - 1 WHERE position > OLD.position;
    END;";

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) enum PinnedKind {
    Playlist,
    Album,
    Artist,
    Station,
}

impl PinnedKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Playlist => "playlist",
            Self::Album => "album",
            Self::Artist => "artist",
            Self::Station => "station",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "playlist" => Ok(Self::Playlist),
            "album" => Ok(Self::Album),
            "artist" => Ok(Self::Artist),
            "station" => Ok(Self::Station),
            _ => Err(StoreError::InvalidData),
        }
    }
}

/// The unique identity of a pinned object. A missing provider means a local playlist.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct PinId {
    pub kind: PinnedKind,
    pub provider: Option<ProviderId>,
    pub profile: String,
    pub object_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NewPin {
    pub id: PinId,
    pub title: Option<String>,
    pub artwork: Option<String>,
    pub link: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PinnedObject {
    pub id: PinId,
    pub title: String,
    pub artwork: Option<String>,
    pub link: Option<String>,
    pub position: i64,
}

impl Store {
    /// Pin an object once. A repeated request leaves the existing position untouched.
    pub(crate) fn pin_object(&self, pin: &NewPin) -> Result<bool> {
        validate(pin)?;
        let (kind, provider, profile, object_id) = pin.id.columns();
        let local_playlist = pin
            .id
            .provider
            .is_none()
            .then(|| object_id.parse::<i64>())
            .transpose()
            .map_err(|_| StoreError::InvalidData)?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "INSERT INTO pinned_object
                (kind, provider, profile_id, object_id, local_playlist_id, title, artwork, link, position)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                     (SELECT COALESCE(MAX(position) + 1, 0) FROM pinned_object))
             ON CONFLICT (kind, provider, profile_id, object_id) DO NOTHING",
            params![
                kind,
                provider,
                profile,
                object_id,
                local_playlist,
                pin.title,
                pin.artwork,
                pin.link
            ],
        )?;
        transaction.commit()?;
        Ok(changed != 0)
    }

    pub(crate) fn unpin_object(&self, id: &PinId) -> Result<bool> {
        let (kind, provider, profile, object_id) = id.columns();
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let removed = transaction.execute(
            "DELETE FROM pinned_object
             WHERE kind = ?1 AND provider = ?2 AND profile_id = ?3 AND object_id = ?4",
            params![kind, provider, profile, object_id],
        )? != 0;
        transaction.commit()?;
        Ok(removed)
    }

    pub(crate) fn pinned_objects(&self) -> Result<Vec<PinnedObject>> {
        let connection = self.connect()?;
        let mut statement = connection.prepare(
            "SELECT object.kind, object.provider, object.profile_id, object.object_id,
                    CASE WHEN object.provider = '' THEN playlist.name ELSE object.title END,
                    object.artwork, object.link, object.position
             FROM pinned_object object
             LEFT JOIN local_playlist playlist ON playlist.id = object.local_playlist_id
             ORDER BY object.position, object.id",
        )?;
        statement
            .query_map([], |row| {
                let kind: String = row.get(0)?;
                let provider: String = row.get(1)?;
                Ok((
                    kind,
                    provider,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(
                |(kind, provider, profile, object_id, title, artwork, link, position)| {
                    Ok(PinnedObject {
                        id: PinId {
                            kind: PinnedKind::parse(&kind)?,
                            provider: (!provider.is_empty())
                                .then(|| provider_id(&provider))
                                .transpose()?,
                            profile,
                            object_id,
                        },
                        title: title.ok_or(StoreError::InvalidData)?,
                        artwork,
                        link,
                        position,
                    })
                },
            )
            .collect()
    }

    /// Shift a pin one place. Returns false at the edge and for an unknown pin.
    pub(crate) fn move_pin(&self, id: &PinId, up: bool) -> Result<bool> {
        self.arrange_pin(id, |from, length| {
            let to = if up {
                from.checked_sub(1)?
            } else {
                let next = from + 1;
                (next < length).then_some(next)?
            };
            Some(to)
        })
    }

    /// Put a pin at a position. The position is clamped to the current range.
    pub(crate) fn move_pin_to(&self, id: &PinId, position: i64) -> Result<bool> {
        self.arrange_pin(id, |from, length| {
            let to = position.clamp(0, length as i64 - 1) as usize;
            (to != from).then_some(to)
        })
    }

    fn arrange_pin(
        &self,
        id: &PinId,
        target: impl FnOnce(usize, usize) -> Option<usize>,
    ) -> Result<bool> {
        let (kind, provider, profile, object_id) = id.columns();
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let order = {
            let mut statement =
                transaction.prepare("SELECT id FROM pinned_object ORDER BY position, id")?;
            statement
                .query_map([], |row| row.get::<_, i64>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let element = transaction
            .query_row(
                "SELECT id FROM pinned_object
                 WHERE kind = ?1 AND provider = ?2 AND profile_id = ?3 AND object_id = ?4",
                params![kind, provider, profile, object_id],
                |row| row.get::<_, i64>(0),
            )
            .ok();
        let Some(element) = element else {
            return Ok(false);
        };
        let Some(from) = order.iter().position(|item| *item == element) else {
            return Ok(false);
        };
        let Some(to) = target(from, order.len()) else {
            return Ok(false);
        };
        let mut order = order;
        let element = order.remove(from);
        order.insert(to, element);
        for (position, element) in order.iter().enumerate() {
            transaction.execute(
                "UPDATE pinned_object SET position = ?1 WHERE id = ?2",
                params![position as i64, element],
            )?;
        }
        transaction.commit()?;
        Ok(true)
    }
}

fn validate(pin: &NewPin) -> Result<()> {
    if pin.id.object_id.trim().is_empty() {
        return Err(StoreError::InvalidData);
    }
    match pin.id.provider {
        None if pin.id.kind != PinnedKind::Playlist || pin.title.is_some() => {
            Err(StoreError::InvalidData)
        }
        None => Ok(()),
        Some(_)
            if pin
                .title
                .as_deref()
                .is_none_or(|title| title.trim().is_empty()) =>
        {
            Err(StoreError::InvalidData)
        }
        Some(_) => Ok(()),
    }
}

impl PinId {
    fn columns(&self) -> (&'static str, &'static str, &str, &str) {
        (
            self.kind.as_str(),
            self.provider.map_or("", ProviderId::as_str),
            &self.profile,
            &self.object_id,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{DATABASE, MIGRATIONS, schema_version};

    fn playlist(id: i64) -> NewPin {
        NewPin {
            id: PinId {
                kind: PinnedKind::Playlist,
                provider: None,
                profile: String::new(),
                object_id: id.to_string(),
            },
            title: None,
            artwork: None,
            link: None,
        }
    }

    fn album(id: &str) -> NewPin {
        NewPin {
            id: PinId {
                kind: PinnedKind::Album,
                provider: Some(ProviderId::Spotify),
                profile: "profile".into(),
                object_id: id.into(),
            },
            title: Some(format!("Album {id}")),
            artwork: Some("https://example.test/cover".into()),
            link: None,
        }
    }

    fn station(profile: &str) -> NewPin {
        NewPin {
            id: PinId {
                kind: PinnedKind::Station,
                provider: Some(ProviderId::YandexMusic),
                profile: profile.into(),
                object_id: "user:onyourwave".into(),
            },
            title: Some("Моя волна".into()),
            artwork: None,
            link: None,
        }
    }

    fn positions(store: &Store) -> Vec<(String, i64)> {
        store
            .pinned_objects()
            .unwrap()
            .into_iter()
            .map(|pin| (pin.title, pin.position))
            .collect()
    }

    fn seed(store: &Store) -> i64 {
        let connection = store.connect().unwrap();
        connection
            .execute("INSERT INTO local_playlist (name) VALUES ('Пин')", [])
            .unwrap();
        connection.last_insert_rowid()
    }

    #[test]
    fn pinning_is_idempotent_and_reads_the_local_name() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let id = seed(&store);
        assert!(store.pin_object(&playlist(id)).unwrap());
        assert!(!store.pin_object(&playlist(id)).unwrap());
        assert!(store.pin_object(&album("one")).unwrap());
        assert!(!store.pin_object(&album("one")).unwrap());
        assert_eq!(
            positions(&store),
            [("Пин".to_owned(), 0), ("Album one".to_owned(), 1)]
        );
    }

    #[test]
    fn renaming_and_deleting_a_local_playlist_updates_its_pin() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let id = seed(&store);
        assert!(store.pin_object(&playlist(id)).unwrap());
        assert!(store.rename_local_playlist(id, "Новое имя").unwrap());
        assert_eq!(positions(&store), [("Новое имя".to_owned(), 0)]);
        assert!(store.pin_object(&album("one")).unwrap());
        // The cascade removes the first pin, and the next one closes the gap.
        assert!(store.delete_local_playlist(id).unwrap());
        assert_eq!(positions(&store), [("Album one".to_owned(), 0)]);
    }

    #[test]
    fn moves_keep_positions_consecutive_from_zero() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let id = seed(&store);
        let names = ["a", "b", "c"];
        assert!(store.pin_object(&playlist(id)).unwrap());
        for name in names {
            assert!(store.pin_object(&album(name)).unwrap());
        }
        let order = |store: &Store| {
            store
                .pinned_objects()
                .unwrap()
                .into_iter()
                .map(|pin| pin.title)
                .collect::<Vec<_>>()
        };
        assert_eq!(order(&store), ["Пин", "Album a", "Album b", "Album c"]);
        let first = PinId {
            kind: PinnedKind::Album,
            provider: Some(ProviderId::Spotify),
            profile: "profile".into(),
            object_id: "a".into(),
        };
        assert!(store.move_pin(&first, false).unwrap());
        assert_eq!(order(&store), ["Пин", "Album b", "Album a", "Album c"]);
        assert!(store.move_pin(&first, false).unwrap());
        assert_eq!(order(&store), ["Пин", "Album b", "Album c", "Album a"]);
        assert!(!store.move_pin(&first, false).unwrap());
        assert!(store.move_pin_to(&first, 0).unwrap());
        assert_eq!(order(&store), ["Album a", "Пин", "Album b", "Album c"]);
        assert!(!store.move_pin_to(&first, 0).unwrap());
        for (position, pin) in store.pinned_objects().unwrap().iter().enumerate() {
            assert_eq!(pin.position, position as i64);
        }
        assert!(store.unpin_object(&first).unwrap());
        assert_eq!(order(&store), ["Пин", "Album b", "Album c"]);
        for (position, pin) in store.pinned_objects().unwrap().iter().enumerate() {
            assert_eq!(pin.position, position as i64);
        }
    }

    #[test]
    fn a_track_cannot_be_pinned_and_a_provider_pin_needs_a_title() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let mut broken = album("x");
        broken.title = None;
        assert!(matches!(
            store.pin_object(&broken),
            Err(StoreError::InvalidData)
        ));
        let mut orphan = playlist(4);
        orphan.id.provider = Some(ProviderId::Spotify);
        assert!(matches!(
            store.pin_object(&orphan),
            Err(StoreError::InvalidData)
        ));
    }

    #[test]
    fn stations_are_distinct_by_profile_and_follow_pin_order() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let first = station("first");
        let second = station("second");
        assert!(store.pin_object(&album("album")).unwrap());
        assert!(store.pin_object(&first).unwrap());
        assert!(!store.pin_object(&first).unwrap());
        assert!(store.pin_object(&second).unwrap());
        assert!(store.move_pin_to(&second.id, 0).unwrap());
        let pinned = store.pinned_objects().unwrap();
        assert_eq!(pinned[0].id, second.id);
        assert_eq!(pinned[2].id, first.id);

        assert!(store.unpin_object(&second.id).unwrap());
        assert_eq!(
            positions(&store),
            [("Album album".into(), 0), ("Моя волна".into(), 1)]
        );
        assert_eq!(store.pinned_objects().unwrap()[1].id, first.id);
    }

    #[test]
    fn station_migration_preserves_existing_pins_and_the_local_playlist_cascade() {
        const PREVIOUS_SCHEMA: usize = 11;

        let directory = tempfile::tempdir().unwrap();
        let legacy = Store {
            database: directory.path().join(DATABASE),
        };
        let connection = legacy.connect().unwrap();
        for migration in MIGRATIONS[..PREVIOUS_SCHEMA]
            .iter()
            .flat_map(|version| *version)
        {
            connection.execute_batch(migration).unwrap();
        }
        connection
            .pragma_update(None, "user_version", PREVIOUS_SCHEMA as i64)
            .unwrap();
        drop(connection);
        let local_id = seed(&legacy);
        let mut remote = album("existing");
        remote.link = Some("https://open.spotify.com/album/existing".into());
        let deleted = album("deleted");
        assert!(legacy.pin_object(&remote).unwrap());
        assert!(legacy.pin_object(&playlist(local_id)).unwrap());
        assert!(legacy.move_pin_to(&remote.id, 1).unwrap());
        assert!(legacy.pin_object(&deleted).unwrap());
        assert!(legacy.unpin_object(&deleted.id).unwrap());
        let original_pins = legacy.pinned_objects().unwrap();

        let store = Store::open(directory.path()).unwrap();
        let connection = store.connect().unwrap();
        assert_eq!(schema_version(&connection).unwrap(), MIGRATIONS.len());
        assert_eq!(store.pinned_objects().unwrap(), original_pins);
        assert!(store.pin_object(&station("new-profile")).unwrap());
        let station_rowid: i64 = connection
            .query_row(
                "SELECT id FROM pinned_object WHERE kind = 'station'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(station_rowid, 4);
        assert!(store.delete_local_playlist(local_id).unwrap());
        assert_eq!(
            positions(&store),
            [("Album existing".into(), 0), ("Моя волна".into(), 1)]
        );
    }
}
