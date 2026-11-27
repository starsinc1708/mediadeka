mod appearance;
mod artwork_palette;
mod collection_layout;
mod collection_listening;
mod connection_lock;
mod genius;
mod history;
mod home_cache;
mod likes;
mod listening_integrations;
mod lyrics;
mod music;
mod panel;
mod pins;
mod playlists;
mod queue_resume;
mod recommendations;
mod sound_cache;
mod vault;
mod workspace;

use std::{
    env, fmt, fs, io,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};

use crate::providers::{Capabilities, Credentials, Profile, ProviderId, Session};

pub(crate) use collection_layout::{
    CollectionDisplay, CollectionDisplayChange, CollectionKind, CollectionView,
};
pub(crate) use collection_listening::CollectionRecord;
pub(crate) use genius::{GeniusSession, GeniusSnapshot, GeniusSnapshotKind};
pub(crate) use history::{HistoryRecord, LIMIT as HISTORY_LIMIT, LocalTimestamp};
pub(crate) use home_cache::{CacheScope, Snapshot};
pub(crate) use music::{CatalogSave, MusicSnapshot};
pub(crate) use panel::{PanelState, PanelTab};
pub(crate) use pins::{NewPin, PinId, PinnedKind, PinnedObject};
pub(crate) use playlists::{
    FavoriteScope, ImportProgress, LocalPlaylistSnapshot, LocalPlaylistSummary, is_m3u_path,
    playlist_name,
};

const DATABASE: &str = "mediadeka.sqlite3";
const PROFILE_OVERRIDE: &str = "MEDIADEKA_PROFILE_DIR";
const BUSY_TIMEOUT: Duration = Duration::from_secs(3);

const MIGRATIONS: &[&[&str]] = &[
    &["CREATE TABLE account (
        provider TEXT NOT NULL,
        profile_id TEXT NOT NULL,
        display_name TEXT NOT NULL,
        capabilities TEXT NOT NULL,
        credential_ref TEXT NOT NULL,
        PRIMARY KEY (provider, profile_id)
    ) STRICT;
    CREATE TABLE startup (
        id INTEGER PRIMARY KEY CHECK (id = 1),
        kind TEXT NOT NULL,
        provider TEXT,
        profile_id TEXT,
        CHECK ((kind = 'guest' AND provider IS NULL AND profile_id IS NULL) OR
               (kind = 'account' AND provider IS NOT NULL AND profile_id IS NOT NULL)),
        FOREIGN KEY (provider, profile_id) REFERENCES account (provider, profile_id)
    ) STRICT;"],
    &["CREATE TABLE home_cache (
        kind TEXT NOT NULL,
        provider TEXT NOT NULL,
        profile_id TEXT NOT NULL,
        received_at_ms INTEGER NOT NULL CHECK (received_at_ms >= 0),
        payload TEXT NOT NULL,
        CHECK ((kind = 'guest' AND provider = 'youtube-music' AND profile_id = '') OR
               (kind = 'account' AND profile_id <> '')),
        PRIMARY KEY (kind, provider, profile_id)
    ) STRICT;"],
    &["CREATE TABLE account_source (
        provider TEXT PRIMARY KEY NOT NULL,
        profile_id TEXT NOT NULL,
        FOREIGN KEY (provider, profile_id) REFERENCES account (provider, profile_id)
    ) STRICT;
    INSERT INTO account_source (provider, profile_id)
    SELECT provider, profile_id
    FROM account
    WHERE rowid IN (SELECT MAX(rowid) FROM account GROUP BY provider);
    UPDATE account_source
    SET profile_id = (SELECT profile_id FROM startup WHERE id = 1)
    WHERE provider = (SELECT provider FROM startup WHERE id = 1 AND kind = 'account');"],
    &["CREATE TABLE credential_cleanup (
        provider TEXT NOT NULL,
        credential_ref TEXT NOT NULL,
        PRIMARY KEY (provider, credential_ref)
    ) STRICT;
    INSERT INTO startup (id, kind, provider, profile_id)
    SELECT 1, 'account', provider, profile_id FROM account_source
    WHERE NOT EXISTS (SELECT 1 FROM startup WHERE id = 1)
    ORDER BY provider LIMIT 1;"],
    &[playlists::MIGRATION],
    &[music::MIGRATION],
    &[music::REVOCATION_MIGRATION],
    &[music::ALIAS_MIGRATION],
    &[history::MIGRATION],
    &[pins::MIGRATION],
    &[panel::MIGRATION],
    &[pins::STATION_MIGRATION, history::STATION_MIGRATION],
    &[likes::MIGRATION],
    &[collection_listening::MIGRATION],
    &[collection_listening::LOCAL_ALBUM_MIGRATION],
    &[sound_cache::MIGRATION, history::SOUND_MIGRATION],
    &[
        lyrics::MIGRATION,
        genius::MIGRATION,
        panel::LYRICS_TAB_MIGRATION,
    ],
    // Previous versions recorded ambiguous HTTP 401 and missing vault entries as revocations.
    &["DELETE FROM music_revoked;"],
    &[recommendations::MIGRATION],
    &[recommendations::ARTIST_MIGRATION],
    &[panel::AUTOPLAY_MIGRATION],
    &[playlists::TAG_CACHE_MIGRATION],
    &[workspace::MIGRATION, playlists::LIBRARY_LOOKUP_MIGRATION],
    &[listening_integrations::MIGRATION],
    &[appearance::MIGRATION],
    &[appearance::VISUAL_MIGRATION],
    &[collection_layout::MIGRATION],
    &[collection_layout::TRACK_COLUMNS_MIGRATION],
    &[appearance::AUDIO_RESPONSE_MIGRATION],
    &[music::SNAPSHOT_TIMESTAMP_MIGRATION],
    &[playlists::LOCAL_PLAYLIST_REVISION_MIGRATION],
    &[music::CATALOG_GENERATION_MIGRATION],
    &[queue_resume::MIGRATION],
    &[music::PLAYLIST_SNAPSHOT_BUDGET_MIGRATION],
    &[artwork_palette::MIGRATION],
    &[panel::WIDTH_MIGRATION],
    &[appearance::SURFACE_MIGRATION],
];

#[derive(Clone, Debug)]
pub(crate) struct Store {
    database: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Account {
    pub provider: ProviderId,
    pub profile: Profile,
    pub capabilities: Capabilities,
    credential_ref: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AccountSummary {
    pub provider: ProviderId,
    pub profile: Profile,
}

#[derive(Debug)]
pub(crate) enum Startup {
    Welcome,
    Guest,
    Account(Account),
}

enum AccountWrite<'a> {
    Activate,
    Connect,
    Update(&'a str),
}

#[derive(Debug)]
pub(crate) enum StoreError {
    Io(io::Error),
    Sqlite(rusqlite::Error),
    InvalidData,
    PlaylistInput(String),
    ImportCancelled,
    NoProfileFolder,
    NewerVersion(i64),
}

impl fmt::Display for StoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "Хранилище Windows: {error}"),
            Self::Sqlite(error) => write!(formatter, "Хранилище SQLite: {error}"),
            Self::InvalidData => formatter.write_str("Повреждены данные профиля Mediadeka."),
            Self::PlaylistInput(message) => formatter.write_str(message),
            Self::ImportCancelled => formatter.write_str("Импорт отменён."),
            Self::NoProfileFolder => formatter.write_str("Недоступна папка профиля Windows."),
            Self::NewerVersion(version) => {
                write!(
                    formatter,
                    "Схема профиля Mediadeka версии {version} новее этой сборки."
                )
            }
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Sqlite(error)
    }
}

impl From<serde_json::Error> for StoreError {
    fn from(_: serde_json::Error) -> Self {
        Self::InvalidData
    }
}

type Result<T> = std::result::Result<T, StoreError>;

impl Store {
    #[cfg(test)]
    pub(crate) fn versioned_fixture(directory: &Path, version: usize) -> Result<Self> {
        let store = Self {
            database: directory.join(DATABASE),
        };
        let mut connection = store.connect()?;
        let transaction = connection.transaction()?;
        for sql in MIGRATIONS[..version]
            .iter()
            .flat_map(|migration| *migration)
        {
            transaction.execute_batch(sql)?;
        }
        transaction.pragma_update(None, "user_version", version as i64)?;
        transaction.commit()?;
        Ok(store)
    }

    pub(crate) fn directory(&self) -> &Path {
        self.database
            .parent()
            .expect("the profile store always has a parent directory")
    }
    pub(crate) fn open_default() -> Result<Self> {
        Self::open(Self::profile_directory()?)
    }

    pub(crate) fn profile_directory() -> Result<PathBuf> {
        env::var_os(PROFILE_OVERRIDE)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                env::var_os("LOCALAPPDATA")
                    .filter(|root| !root.is_empty())
                    .map(|root| PathBuf::from(root).join("Mediadeka/app2"))
            })
            .ok_or(StoreError::NoProfileFolder)
    }

    #[cfg(windows)]
    pub(crate) fn uses_default_profile() -> bool {
        env::var_os(PROFILE_OVERRIDE).is_none_or(|value| value.is_empty())
    }

    pub(crate) fn open(directory: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(directory.as_ref())?;
        let store = Self {
            database: directory.as_ref().join(DATABASE),
        };
        let mut connection = store.connect()?;
        if schema_version(&connection)? < MIGRATIONS.len() {
            let transaction =
                connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            // Another process may have migrated the profile while we waited for the writer.
            let version = schema_version(&transaction)?;
            for (index, migrations) in MIGRATIONS.iter().enumerate().skip(version) {
                for migration in *migrations {
                    transaction.execute_batch(migration)?;
                }
                if migrations.contains(&music::PLAYLIST_SNAPSHOT_BUDGET_MIGRATION) {
                    music::trim_playlist_snapshots(&transaction)?;
                }
                transaction.pragma_update(None, "user_version", index as i64 + 1)?;
            }
            transaction.commit()?;
        }
        let journal_mode: String =
            connection.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        if !journal_mode.eq_ignore_ascii_case("wal") {
            connection.pragma_update(None, "journal_mode", "WAL")?;
        }
        Ok(store)
    }

    fn connect(&self) -> Result<Connection> {
        let connection = Connection::open(&self.database)?;
        connection.busy_timeout(BUSY_TIMEOUT)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        Ok(connection)
    }

    pub(crate) fn startup(&self) -> Result<Startup> {
        let connection = self.connect()?;
        Self::startup_in(&connection)
    }

    pub(crate) fn retry_credential_cleanup(&self) {
        report_credential_cleanup(
            self.connect()
                .and_then(|connection| cleanup_credentials(&connection)),
        );
    }

    fn startup_in(connection: &Connection) -> Result<Startup> {
        let route = connection
            .query_row(
                "SELECT startup.kind, account.provider, account.profile_id, account.display_name,
                    account.capabilities, account.credential_ref
             FROM startup LEFT JOIN account USING (provider, profile_id) WHERE startup.id = 1",
                [],
                |row| {
                    if row.get::<_, String>(0)? == "guest" {
                        return Ok(None);
                    }
                    Ok(Some((
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, String>(5)?,
                    )))
                },
            )
            .optional()?;
        match route {
            None => Ok(Self::first_account(connection)?.map_or(Startup::Welcome, Startup::Account)),
            Some(None) => Ok(Startup::Guest),
            Some(Some(account)) => Ok(Startup::Account(account_from_columns(account)?)),
        }
    }

    fn first_account(connection: &Connection) -> Result<Option<Account>> {
        connection
            .query_row(
                "SELECT provider, profile_id, display_name, capabilities, credential_ref
                 FROM account_source JOIN account USING (provider, profile_id)
                 ORDER BY provider LIMIT 1",
                [],
                account_columns,
            )
            .optional()?
            .map(account_from_columns)
            .transpose()
    }

    pub(crate) fn accounts(&self) -> Result<Vec<AccountSummary>> {
        let connection = self.connect()?;
        let mut query = connection.prepare(
            "SELECT provider, profile_id, display_name
             FROM account_source JOIN account USING (provider, profile_id)
             ORDER BY provider",
        )?;
        query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    Profile {
                        id: row.get(1)?,
                        name: row.get(2)?,
                    },
                ))
            })?
            .map(|row| {
                let (provider, profile) = row?;
                let provider = parse_provider(&provider).ok_or(StoreError::InvalidData)?;
                Ok(AccountSummary { provider, profile })
            })
            .collect()
    }

    pub(crate) fn account(&self, provider: ProviderId, id: &str) -> Result<Option<Account>> {
        self.source_account(provider, Some(id))
    }

    pub(crate) fn preferred_account(&self, provider: ProviderId) -> Result<Option<Account>> {
        self.source_account(provider, None)
    }

    fn source_account(&self, provider: ProviderId, id: Option<&str>) -> Result<Option<Account>> {
        self.connect()?
            .query_row(
                "SELECT provider, profile_id, display_name, capabilities, credential_ref
                 FROM account_source JOIN account USING (provider, profile_id)
                 WHERE provider = ?1 AND (?2 IS NULL OR profile_id = ?2)",
                params![provider.as_str(), id],
                account_columns,
            )
            .optional()?
            .map(account_from_columns)
            .transpose()
    }

    pub(crate) fn select_account<T>(
        &self,
        provider: ProviderId,
        id: &str,
        permit: impl FnOnce() -> Option<T>,
    ) -> Result<bool> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let is_current = transaction.query_row(
            "SELECT EXISTS (SELECT 1 FROM account_source WHERE provider = ?1 AND profile_id = ?2)",
            params![provider.as_str(), id],
            |row| row.get::<_, bool>(0),
        )?;
        if !is_current {
            return Ok(false);
        }
        activate_account(&transaction, provider, id)?;
        commit_if_current(transaction, permit)
    }

    pub(crate) fn use_guest<T>(&self, permit: impl FnOnce() -> Option<T>) -> Result<bool> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = transaction.execute(
            "INSERT INTO startup (id, kind)
             SELECT 1, 'guest' WHERE NOT EXISTS (SELECT 1 FROM account_source WHERE provider = ?1)
             ON CONFLICT (id) DO UPDATE SET kind = 'guest', provider = NULL, profile_id = NULL",
            [ProviderId::YouTubeMusic.as_str()],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        commit_if_current(transaction, permit)
    }

    pub(crate) fn upgrade_guest_route<T>(&self, permit: impl FnOnce() -> Option<T>) -> Result<()> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "UPDATE startup SET kind = 'account', provider = ?1,
                 profile_id = (SELECT profile_id FROM account_source WHERE provider = ?1)
             WHERE id = 1 AND kind = 'guest'
                 AND EXISTS (SELECT 1 FROM account_source WHERE provider = ?1)",
            [ProviderId::YouTubeMusic.as_str()],
        )?;
        commit_if_current(transaction, permit)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn welcome<T>(&self, permit: impl FnOnce() -> Option<T>) -> Result<()> {
        self.update_route("DELETE FROM startup WHERE id = 1", permit)
    }

    #[cfg(test)]
    fn update_route<T>(&self, sql: &str, permit: impl FnOnce() -> Option<T>) -> Result<()> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(sql, [])?;
        commit_if_current(transaction, permit)?;
        Ok(())
    }

    pub(crate) fn credentials(&self, account: &Account) -> Result<Option<Credentials>> {
        let mut connection = self.connect()?;
        // Serialize the lookup with credential replacement across application instances.
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some(reference) = credential_ref(&transaction, account.provider, &account.profile.id)?
        else {
            return Ok(None);
        };
        let Some(bytes) = vault::read(&target(account.provider, &reference)?)? else {
            return Ok(None);
        };
        let (profile_id, credentials): (String, Credentials) = serde_json::from_slice(&bytes)?;
        if profile_id != account.profile.id || credentials.provider() != account.provider {
            return Err(StoreError::InvalidData);
        }
        Ok(Some(credentials))
    }

    pub(crate) fn use_account<T>(
        &self,
        session: &Session,
        permit: impl FnOnce() -> Option<T>,
        committed: impl FnOnce(),
    ) -> Result<bool> {
        self.save_account(session, AccountWrite::Activate, permit, committed)
            .map(|route| route.is_some())
    }

    pub(crate) fn connect_account<T>(
        &self,
        session: &Session,
        permit: impl FnOnce() -> Option<T>,
        committed: impl FnOnce(),
    ) -> Result<Option<Startup>> {
        self.save_account(session, AccountWrite::Connect, permit, committed)
    }

    pub(crate) fn update_account<T>(
        &self,
        session: &Session,
        previous: &Account,
        permit: impl FnOnce() -> Option<T>,
    ) -> Result<bool> {
        self.reconnect_account(session, previous, permit, || {})
            .map(|route| route.is_some())
    }

    pub(crate) fn reconnect_account<T>(
        &self,
        session: &Session,
        previous: &Account,
        permit: impl FnOnce() -> Option<T>,
        committed: impl FnOnce(),
    ) -> Result<Option<Startup>> {
        if previous.provider != session.provider || previous.profile.id != session.profile.id {
            return Err(StoreError::InvalidData);
        }
        self.save_account(
            session,
            AccountWrite::Update(&previous.credential_ref),
            permit,
            committed,
        )
    }

    fn save_account<T>(
        &self,
        session: &Session,
        write: AccountWrite<'_>,
        permit: impl FnOnce() -> Option<T>,
        committed: impl FnOnce(),
    ) -> Result<Option<Startup>> {
        if session.profile.id.trim().is_empty()
            || session.credentials.provider() != session.provider
        {
            return Err(StoreError::InvalidData);
        }
        let credential = serde_json::to_vec(&(&session.profile.id, &session.credentials))?;
        let capabilities = serde_json::to_string(&session.capabilities)?;
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let provider = session.provider;
        let profile = &session.profile;
        let previous_ref = credential_ref(&transaction, provider, &profile.id)?;
        match write {
            AccountWrite::Update(expected) => {
                let bound = transaction.query_row(
                    "SELECT EXISTS (SELECT 1 FROM account_source WHERE provider = ?1 AND profile_id = ?2)",
                    params![provider.as_str(), profile.id],
                    |row| row.get::<_, bool>(0),
                )?;
                if !bound || previous_ref.as_deref() != Some(expected) {
                    return Ok(None);
                }
            }
            AccountWrite::Connect
                if transaction.query_row(
                    "SELECT EXISTS (SELECT 1 FROM account_source WHERE provider = ?1)",
                    [provider.as_str()],
                    |row| row.get::<_, bool>(0),
                )? =>
            {
                return Ok(None);
            }
            _ => {}
        }
        let existing_route = matches!(write, AccountWrite::Connect)
            .then(|| Self::startup_in(&transaction))
            .transpose()?;
        let reference = transaction.query_row("SELECT hex(randomblob(16))", [], |row| {
            row.get::<_, String>(0)
        })?;
        let new_target = target(provider, &reference)?;
        vault::write(&new_target, &credential)?;

        let saved = (|| -> Result<Option<Startup>> {
            transaction.execute(
                "INSERT INTO account (provider, profile_id, display_name, capabilities, credential_ref)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (provider, profile_id) DO UPDATE SET display_name = excluded.display_name,
                    capabilities = excluded.capabilities, credential_ref = excluded.credential_ref",
                params![provider.as_str(), profile.id, profile.name, capabilities, reference],
            )?;
            if !matches!(write, AccountWrite::Update(_)) {
                transaction.execute(
                    "INSERT INTO account_source (provider, profile_id) VALUES (?1, ?2)
                     ON CONFLICT (provider) DO UPDATE SET profile_id = excluded.profile_id",
                    params![provider.as_str(), profile.id],
                )?;
                match existing_route {
                    Some(Startup::Account(account)) => {
                        activate_account(&transaction, account.provider, &account.profile.id)?;
                    }
                    Some(Startup::Guest) => {}
                    Some(Startup::Welcome) | None => {
                        activate_account(&transaction, provider, &profile.id)?
                    }
                }
            }
            let route = Self::startup_in(&transaction)?;
            let Some(_permit) = permit() else {
                return Ok(None);
            };
            transaction.commit()?;
            committed();
            Ok(Some(route))
        })();
        if !matches!(saved, Ok(Some(_))) {
            remove_credential(&new_target);
        } else if let Some(reference) = previous_ref
            && let Ok(previous_target) = target(provider, &reference)
        {
            remove_credential(&previous_target);
        }
        saved
    }

    pub(crate) fn disconnect_account<T>(
        &self,
        provider: ProviderId,
        id: &str,
        permit: impl FnOnce() -> Option<T>,
    ) -> Result<Option<Startup>> {
        let mut connection = self.connect()?;
        let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let reference = transaction
            .query_row(
                "SELECT credential_ref FROM account_source JOIN account USING (provider, profile_id)
                 WHERE provider = ?1 AND profile_id = ?2",
                params![provider.as_str(), id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let Some(reference) = reference else {
            return Ok(None);
        };
        // Validate the vault key before recording the durable deletion.
        target(provider, &reference)?;
        let route = Self::startup_in(&transaction)?;
        transaction.execute(
            "DELETE FROM account_source WHERE provider = ?1 AND profile_id = ?2",
            params![provider.as_str(), id],
        )?;
        let route = if matches!(&route, Startup::Account(account)
            if account.provider == provider && account.profile.id == id)
        {
            if let Some(account) = Self::first_account(&transaction)? {
                activate_account(&transaction, account.provider, &account.profile.id)?;
                Startup::Account(account)
            } else {
                transaction.execute(
                    "INSERT INTO startup (id, kind) VALUES (1, 'guest')
                     ON CONFLICT (id) DO UPDATE SET kind = 'guest', provider = NULL, profile_id = NULL",
                    [],
                )?;
                Startup::Guest
            }
        } else {
            route
        };
        transaction.execute(
            "DELETE FROM home_cache WHERE kind = 'account' AND provider = ?1 AND profile_id = ?2",
            params![provider.as_str(), id],
        )?;
        playlists::delete_account_favorites(&transaction, provider, id)?;
        likes::delete_account_like_links(&transaction, provider, id)?;
        music::delete_account_snapshots(&transaction, provider, id)?;
        transaction.execute(
            "INSERT INTO credential_cleanup (provider, credential_ref) VALUES (?1, ?2)
             ON CONFLICT DO NOTHING",
            params![provider.as_str(), reference],
        )?;
        transaction.execute(
            "DELETE FROM account WHERE provider = ?1 AND profile_id = ?2",
            params![provider.as_str(), id],
        )?;
        if !commit_if_current(transaction, permit)? {
            return Ok(None);
        }
        report_credential_cleanup(cleanup_credentials(&connection));
        Ok(Some(route))
    }
}

fn schema_version(connection: &Connection) -> Result<usize> {
    let version =
        connection.pragma_query_value(None, "user_version", |row| row.get::<_, i64>(0))?;
    usize::try_from(version)
        .ok()
        .filter(|version| *version <= MIGRATIONS.len())
        .ok_or(StoreError::NewerVersion(version))
}

type AccountColumns = (String, String, String, String, String);

fn account_columns(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountColumns> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

fn account_from_columns(
    (provider, id, name, capabilities, credential_ref): AccountColumns,
) -> Result<Account> {
    Ok(Account {
        provider: parse_provider(&provider).ok_or(StoreError::InvalidData)?,
        profile: Profile { id, name },
        capabilities: serde_json::from_str(&capabilities)?,
        credential_ref,
    })
}

fn cleanup_credentials(connection: &Connection) -> Result<()> {
    let entries = {
        let mut query =
            connection.prepare("SELECT provider, credential_ref FROM credential_cleanup")?;
        query
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut failure = None;
    for (provider, reference) in entries {
        let target = target_for_service(&provider, &reference)?;
        if let Err(error) = vault::delete(&target) {
            failure.get_or_insert(error);
            continue;
        }
        connection.execute(
            "DELETE FROM credential_cleanup WHERE provider = ?1 AND credential_ref = ?2",
            params![provider, reference],
        )?;
    }
    failure.map_or(Ok(()), |error| Err(error.into()))
}

fn report_credential_cleanup(result: Result<()>) {
    if let Err(error) = result {
        eprintln!("Не удалось убрать данные подключения Mediadeka: {error}");
    }
}

fn activate_account(connection: &Connection, provider: ProviderId, id: &str) -> Result<()> {
    connection.execute(
        "INSERT INTO startup (id, kind, provider, profile_id) VALUES (1, 'account', ?1, ?2)
         ON CONFLICT (id) DO UPDATE SET kind = 'account', provider = excluded.provider,
            profile_id = excluded.profile_id",
        params![provider.as_str(), id],
    )?;
    Ok(())
}

fn commit_if_current<T>(
    transaction: Transaction<'_>,
    permit: impl FnOnce() -> Option<T>,
) -> Result<bool> {
    let Some(_permit) = permit() else {
        return Ok(false);
    };
    transaction.commit()?;
    Ok(true)
}

fn credential_ref(
    connection: &Connection,
    provider: ProviderId,
    profile_id: &str,
) -> Result<Option<String>> {
    Ok(connection
        .query_row(
            "SELECT credential_ref FROM account WHERE provider = ?1 AND profile_id = ?2",
            params![provider.as_str(), profile_id],
            |row| row.get(0),
        )
        .optional()?)
}

fn target(provider: ProviderId, reference: &str) -> Result<String> {
    target_for_service(provider.as_str(), reference)
}

fn target_for_service(service: &str, reference: &str) -> Result<String> {
    if (service != genius::SERVICE
        && !listening_integrations::is_listening_service(service)
        && parse_provider(service).is_none())
        || reference.len() != 32
        || !reference.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(StoreError::InvalidData);
    }
    Ok(format!("Mediadeka/app2/{service}/{reference}"))
}

fn remove_credential(target: &str) {
    if let Err(error) = vault::delete(target) {
        eprintln!("Не удалось убрать старые данные подключения Mediadeka: {error}");
    }
}

fn parse_provider(id: &str) -> Option<ProviderId> {
    ProviderId::ALL
        .into_iter()
        .find(|provider| provider.as_str() == id)
}

/// A provider column of a profile table. An unknown value means damaged data.
fn provider_id(id: &str) -> Result<ProviderId> {
    parse_provider(id).ok_or(StoreError::InvalidData)
}

#[cfg(test)]
#[path = "storage/tests.rs"]
mod tests;
