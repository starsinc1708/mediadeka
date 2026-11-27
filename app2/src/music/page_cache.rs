use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError, atomic::AtomicBool},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    playlist::PlaylistTrack,
    providers::{Credentials, ErrorKind, HomeItem, ProviderError, catalog},
    storage::{Account, CatalogSave, MusicSnapshot, Store, StoreError},
    telemetry,
};

use super::{
    MusicPage, ObjectKind, ObjectRef,
    shared_reads::{SharedReads, cache_cleared, trace_source},
};

const MAX_RESIDENT_PAGES: usize = 64;
const MAX_RESIDENT_BYTES: usize = 16 * 1024 * 1024;
const MINUTE_MS: u64 = 60 * 1_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
const DAY_MS: u64 = 24 * HOUR_MS;
static PENDING: SharedReads<(Key, i64), CachedPage> = SharedReads::new();

pub(crate) fn clear_catalog_cache() -> Result<i64, StoreError> {
    let store = Store::open_default()?;
    let (_, generation) = store.clear_catalog_snapshots()?;
    let mut resident = lock_resident();
    resident.retire(store.directory(), generation);
    telemetry::catalog_resident(resident.entries.len(), resident.bytes);
    telemetry::catalog_invalidated(telemetry::CatalogInvalidation::CacheCleared);
    Ok(generation)
}

#[derive(Clone, Eq, PartialEq)]
struct Key {
    directory: PathBuf,
    region: String,
    object: ObjectRef,
    account: Option<Account>,
}

#[derive(Clone, Eq, PartialEq)]
pub(crate) struct PageReceipt {
    scope: Arc<Key>,
    saved_at_ms: u64,
    generation: i64,
}

impl std::fmt::Debug for PageReceipt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageReceipt").finish_non_exhaustive()
    }
}

pub(super) struct PageQuery<'a> {
    store: &'a Store,
    key: Key,
    slot: String,
}

#[derive(Clone, Debug)]
pub(super) struct CachedPage {
    pub(super) data: Arc<MusicPage>,
    modified_ms: u64,
    generation: i64,
}

impl CachedPage {
    pub(super) fn fresh(&self, kind: ObjectKind) -> bool {
        fresh_snapshot(kind, self.modified_ms)
    }
}

pub(super) fn fresh_snapshot(kind: ObjectKind, saved_at_ms: u64) -> bool {
    let ttl = match kind {
        ObjectKind::Track => DAY_MS,
        ObjectKind::Album => 3 * DAY_MS,
        ObjectKind::Artist => HOUR_MS,
        ObjectKind::Collection => 15 * MINUTE_MS,
        ObjectKind::Playlist => 5 * MINUTE_MS,
        ObjectKind::Station => 3 * MINUTE_MS,
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |age| age.as_millis() as u64);
    saved_at_ms != 0 && now.checked_sub(saved_at_ms).is_some_and(|age| age < ttl)
}

impl<'a> PageQuery<'a> {
    pub(super) fn resolve(
        store: &'a Store,
        object: &ObjectRef,
        profile: Option<&str>,
        region: &str,
    ) -> Result<Self, ProviderError> {
        let account = match profile {
            Some(id) => Some(
                store
                    .account(object.provider, id)
                    .map_err(storage_error)?
                    .ok_or_else(|| {
                        access_error(
                            ErrorKind::Expired,
                            "Для этой страницы нужно подключить аккаунт.",
                        )
                    })?,
            ),
            None => store
                .preferred_account(object.provider)
                .map_err(storage_error)?,
        };
        if let Some(account) = &account
            && store.account_revoked(account).map_err(storage_error)?
        {
            return Err(access_error(
                ErrorKind::Revoked,
                "Подтвердите вход в источник заново.",
            ));
        }
        Ok(Self::for_account(store, object, account, region))
    }

    pub(super) fn for_account(
        store: &'a Store,
        object: &ObjectRef,
        account: Option<Account>,
        region: &str,
    ) -> Self {
        Self {
            store,
            key: Key {
                directory: store.directory().to_owned(),
                region: region.to_owned(),
                object: object.clone(),
                account,
            },
            slot: format!("{}:{}", object.kind.path(), object.id),
        }
    }

    pub(super) fn account(&self) -> Option<&Account> {
        self.key.account.as_ref()
    }

    pub(super) fn cached(&self) -> Result<Option<CachedPage>, ProviderError> {
        let resident = {
            let mut cache = lock_resident();
            let index = cache.entries.iter().position(|entry| entry.key == self.key);
            index.and_then(|index| {
                let entry = cache.entries.remove(index)?;
                let page = entry.page.clone();
                cache.entries.push_back(entry);
                Some(page)
            })
        };
        let stored = self
            .store
            .versioned_music_snapshot(
                self.key.object.provider,
                self.account(),
                &self.key.region,
                &self.slot,
                resident
                    .as_ref()
                    .map(|page| (page.modified_ms, page.generation)),
            )
            .map_err(storage_error)?;
        Ok(match stored {
            Some(MusicSnapshot::Unchanged) => resident,
            Some(MusicSnapshot::Changed {
                value,
                saved_at_ms,
                encoded_bytes,
                generation,
            }) => Some(self.remember(value, saved_at_ms, encoded_bytes, generation)),
            None => {
                self.evict();
                None
            }
        })
    }

    pub(super) fn refresh(&self, cancelled: &AtomicBool) -> Result<Arc<MusicPage>, ProviderError> {
        self.refresh_cached(cancelled).map(|page| page.data)
    }

    pub(super) fn receipt(&self, page: &CachedPage) -> PageReceipt {
        PageReceipt {
            scope: Arc::new(self.key.clone()),
            saved_at_ms: page.modified_ms,
            generation: page.generation,
        }
    }

    pub(super) fn refresh_cached(
        &self,
        cancelled: &AtomicBool,
    ) -> Result<CachedPage, ProviderError> {
        self.refresh_with(cancelled, |credentials| {
            catalog::read(&self.key.object, credentials, cancelled)
        })
    }

    fn refresh_with(
        &self,
        cancelled: &AtomicBool,
        load: impl FnOnce(Option<&Credentials>) -> Result<MusicPage, ProviderError>,
    ) -> Result<CachedPage, ProviderError> {
        let generation = self.store.catalog_generation().map_err(storage_error)?;
        PENDING.read(&(self.key.clone(), generation), cancelled, || {
            // A request can reach this slot after another worker has already
            // committed and finished. Check freshness inside the shared flight.
            if let Ok(Some(page)) = self.cached()
                && page.fresh(self.key.object.kind)
            {
                return Ok(page);
            }
            let credentials = self
                .account()
                .map(|account| {
                    self.store
                        .credentials(account)
                        .map_err(storage_error)?
                        .ok_or_else(|| {
                            access_error(ErrorKind::Expired, "Подтвердите вход в источник заново.")
                        })
                })
                .transpose()?;
            let result = trace_source(self.key.object.provider, cancelled, || {
                load(credentials.as_ref())
            });
            if let Err(error) = &result
                && error.kind == ErrorKind::Revoked
                && let Some(account) = self.account()
            {
                let _ = self.store.forget_music_account(account);
                self.evict();
            }
            let value = result?;
            let saved = self
                .store
                .save_catalog_snapshot(
                    self.key.object.provider,
                    self.account(),
                    &self.key.region,
                    &self.slot,
                    &value,
                    generation,
                )
                .map_err(storage_error)?;
            match saved {
                CatalogSave::Stored {
                    saved_at_ms,
                    encoded_bytes,
                } => Ok(self.remember(value, saved_at_ms, encoded_bytes, generation)),
                CatalogSave::AccountChanged => Err(access_error(
                    ErrorKind::Revoked,
                    "Подключение изменилось во время загрузки.",
                )),
                CatalogSave::CacheCleared => Err(cache_cleared()),
            }
        })
    }

    fn remember(
        &self,
        value: MusicPage,
        modified_ms: u64,
        encoded_bytes: usize,
        generation: i64,
    ) -> CachedPage {
        // The JSON size covers string storage. Include in-memory row and item
        // headers, which are larger than serialized empty or partial records.
        let bytes = encoded_bytes
            .saturating_add(size_of::<MusicPage>())
            .saturating_add(
                value
                    .tracks
                    .capacity()
                    .saturating_mul(size_of::<PlaylistTrack>()),
            )
            .saturating_add(
                value
                    .releases
                    .capacity()
                    .saturating_mul(size_of::<HomeItem>()),
            );
        let page = CachedPage {
            data: Arc::new(value),
            modified_ms,
            generation,
        };
        let mut resident = lock_resident();
        resident.insert(self.key.clone(), page.clone(), bytes);
        telemetry::catalog_resident(resident.entries.len(), resident.bytes);
        page
    }

    fn evict(&self) {
        let mut resident = lock_resident();
        resident.remove(&self.key);
        telemetry::catalog_resident(resident.entries.len(), resident.bytes);
    }
}

fn lock_resident() -> std::sync::MutexGuard<'static, ResidentCache> {
    static CACHE: Mutex<ResidentCache> = Mutex::new(ResidentCache {
        entries: VecDeque::new(),
        bytes: 0,
        generations: BTreeMap::new(),
    });
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

struct ResidentPage {
    key: Key,
    page: CachedPage,
    bytes: usize,
}

struct ResidentCache {
    entries: VecDeque<ResidentPage>,
    bytes: usize,
    generations: BTreeMap<PathBuf, i64>,
}

impl ResidentCache {
    fn remove(&mut self, key: &Key) {
        if let Some(index) = self.entries.iter().position(|entry| &entry.key == key)
            && let Some(entry) = self.entries.remove(index)
        {
            self.bytes -= entry.bytes;
        }
    }

    fn insert(&mut self, key: Key, page: CachedPage, bytes: usize) {
        if self
            .generations
            .get(&key.directory)
            .is_some_and(|generation| page.generation < *generation)
        {
            return;
        }
        self.remove(&key);
        if bytes > MAX_RESIDENT_BYTES {
            return;
        }
        while self.entries.len() >= MAX_RESIDENT_PAGES || self.bytes + bytes > MAX_RESIDENT_BYTES {
            let evicted = self
                .entries
                .pop_front()
                .expect("a full cache has a resident page");
            self.bytes -= evicted.bytes;
        }
        self.bytes += bytes;
        self.entries.push_back(ResidentPage { key, page, bytes });
    }

    fn retire(&mut self, directory: &std::path::Path, generation: i64) {
        let latest = self
            .generations
            .entry(directory.to_owned())
            .or_insert(generation);
        *latest = (*latest).max(generation);
        self.entries
            .retain(|entry| entry.key.directory != directory || entry.page.generation >= *latest);
        self.bytes = self.entries.iter().map(|entry| entry.bytes).sum();
    }
}

fn storage_error(error: StoreError) -> ProviderError {
    access_error(ErrorKind::InvalidResponse, error.to_string())
}

fn access_error(kind: ErrorKind, message: impl Into<String>) -> ProviderError {
    ProviderError {
        kind,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests;
