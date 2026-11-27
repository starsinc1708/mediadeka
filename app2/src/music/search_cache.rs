use std::{
    collections::VecDeque,
    io::{self, Write},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, PoisonError, atomic::AtomicBool},
    thread,
    time::{Duration, Instant},
};

use crate::{
    providers::{ErrorKind, ProviderError, ProviderId, catalog},
    storage::{Account, AccountSummary, Store, StoreError},
};

use super::{
    MusicPage, ObjectKind, ScopedPage, region,
    service::scoped,
    shared_reads::{SharedReads, check_cancelled, trace_source},
};

#[cfg(test)]
mod tests;

const FRESH: Duration = Duration::from_secs(5 * 60);
const STALE: Duration = Duration::from_secs(30 * 60);
const EMPTY: Duration = Duration::from_secs(60);
const DEBOUNCE: Duration = Duration::from_millis(220);
const CANCELLATION_POLL: Duration = Duration::from_millis(25);
const MAX_SCOPES: usize = 64;
const MAX_BYTES: usize = 8 * 1024 * 1024;

type Pages = Arc<[ScopedPage]>;
static PENDING: SharedReads<Key, Pages> = SharedReads::new();
static CACHE: Mutex<Cache> = Mutex::new(Cache {
    entries: VecDeque::new(),
    bytes: 0,
});

pub(crate) enum Dispatch {
    Submitted(Instant),
    ResolveLink,
}

pub(crate) struct SearchBatch {
    pub provider: ProviderId,
    pub profile: String,
    pub pages: Pages,
    pub complete: bool,
}

#[derive(Clone, Eq, PartialEq)]
struct Key {
    directory: PathBuf,
    region: String,
    account: Account,
    kind: ObjectKind,
    query: String,
}

pub(crate) fn search_catalogs(
    kind: ObjectKind,
    query: &str,
    dispatch: Dispatch,
    cancelled: &AtomicBool,
    emit: impl Fn(Result<SearchBatch, String>) + Sync,
) -> Result<(), String> {
    let store = Store::open_default().map_err(|error| error.to_string())?;
    let accounts = store.accounts().map_err(|error| error.to_string())?;
    if accounts.is_empty() {
        return match dispatch {
            Dispatch::Submitted(_) => Ok(()),
            Dispatch::ResolveLink => {
                Err("Подключите музыкальную площадку для поиска по названию.".into())
            }
        };
    }
    let region = region();
    let normalized = normalize(query);
    let ready_at = match dispatch {
        Dispatch::Submitted(when) => Some(when + DEBOUNCE),
        Dispatch::ResolveLink => None,
    };
    thread::scope(|scope| {
        for summary in accounts {
            let store = &store;
            let emit = &emit;
            let region = &region;
            let normalized = &normalized;
            scope.spawn(move || {
                let result = (|| {
                    check_cancelled(cancelled)?;
                    let search = Search::open(store, &summary, region, kind, normalized)?;
                    search.run(query, ready_at, cancelled, |pages, complete| {
                        emit(Ok(SearchBatch {
                            provider: summary.provider,
                            profile: summary.profile.id.clone(),
                            pages,
                            complete,
                        }));
                    })
                })();
                if let Err(error) = result
                    && error.kind != ErrorKind::Cancelled
                    && check_cancelled(cancelled).is_ok()
                {
                    emit(Err(format!(
                        "{} · {}",
                        summary.provider.display_name(),
                        error.message
                    )));
                }
            });
        }
    });
    Ok(())
}

struct Search<'a> {
    store: &'a Store,
    key: Key,
}

impl<'a> Search<'a> {
    fn open(
        store: &'a Store,
        summary: &AccountSummary,
        region: &str,
        kind: ObjectKind,
        normalized: &str,
    ) -> Result<Self, ProviderError> {
        let account = store
            .account(summary.provider, &summary.profile.id)
            .map_err(storage_error)?
            .ok_or_else(account_changed)?;
        let search = Self {
            store,
            key: Key {
                directory: store.directory().to_owned(),
                region: region.to_owned(),
                account,
                kind,
                query: normalized.to_owned(),
            },
        };
        search.ensure_current()?;
        Ok(search)
    }

    fn run(
        &self,
        query: &str,
        ready_at: Option<Instant>,
        cancelled: &AtomicBool,
        emit: impl Fn(Pages, bool),
    ) -> Result<(), ProviderError> {
        check_cancelled(cancelled)?;
        let cached = lock_cache().read(&self.key, Instant::now());
        if let Some((pages, fresh)) = cached {
            if fresh || ready_at.is_some() {
                emit(pages, fresh);
            }
            if fresh {
                return Ok(());
            }
        }
        if let Some(ready_at) = ready_at {
            wait(ready_at, cancelled)?;
        }
        let pages = PENDING.read(&self.key, cancelled, || {
            // Another window may have filled the cache while this query waited.
            if let Some((pages, true)) = lock_cache().read(&self.key, Instant::now()) {
                return Ok(pages);
            }
            self.ensure_current()?;
            let account = &self.key.account;
            let credentials = self
                .store
                .credentials(account)
                .map_err(storage_error)?
                .ok_or_else(|| search_error(ErrorKind::Expired, "Подтвердите вход заново."))?;
            let pages = trace_source(account.provider, cancelled, || {
                catalog::search(
                    account.provider,
                    &credentials,
                    self.key.kind,
                    query,
                    cancelled,
                )
            })?;
            self.ensure_current()?;
            let pages: Pages = pages
                .into_iter()
                .map(|page| scoped(page, Some(account)))
                .collect();
            let bytes = batch_bytes(&self.key, &pages);
            lock_cache().insert(self.key.clone(), pages.clone(), bytes, Instant::now());
            Ok(pages)
        })?;
        self.ensure_current()?;
        check_cancelled(cancelled)?;
        emit(pages, true);
        Ok(())
    }

    fn ensure_current(&self) -> Result<(), ProviderError> {
        let account = &self.key.account;
        if self
            .store
            .account(account.provider, &account.profile.id)
            .map_err(storage_error)?
            .as_ref()
            != Some(account)
            || self.store.account_revoked(account).map_err(storage_error)?
        {
            return Err(account_changed());
        }
        Ok(())
    }
}

fn wait(deadline: Instant, cancelled: &AtomicBool) -> Result<(), ProviderError> {
    loop {
        check_cancelled(cancelled)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        thread::sleep(remaining.min(CANCELLATION_POLL));
    }
}

fn normalize(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn account_changed() -> ProviderError {
    search_error(
        ErrorKind::Expired,
        "Подключение изменилось. Проверьте вход.",
    )
}

fn storage_error(error: StoreError) -> ProviderError {
    search_error(ErrorKind::InvalidResponse, error.to_string())
}

fn search_error(kind: ErrorKind, message: impl Into<String>) -> ProviderError {
    ProviderError {
        kind,
        message: message.into(),
    }
}

fn lock_cache() -> MutexGuard<'static, Cache> {
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Entry {
    key: Key,
    pages: Pages,
    loaded: Instant,
    bytes: usize,
}

struct Cache {
    entries: VecDeque<Entry>,
    bytes: usize,
}

impl Cache {
    fn read(&mut self, key: &Key, now: Instant) -> Option<(Pages, bool)> {
        let index = self.entries.iter().position(|entry| &entry.key == key)?;
        let entry = self.entries.remove(index)?;
        let age = now.saturating_duration_since(entry.loaded);
        let (fresh_for, keep_for) = if entry.pages.is_empty() {
            (EMPTY, EMPTY)
        } else {
            (FRESH, STALE)
        };
        if age >= keep_for {
            self.bytes -= entry.bytes;
            return None;
        }
        let result = (entry.pages.clone(), age < fresh_for);
        self.entries.push_back(entry);
        Some(result)
    }

    fn insert(&mut self, key: Key, pages: Pages, bytes: usize, loaded: Instant) {
        if let Some(index) = self.entries.iter().position(|entry| entry.key == key)
            && let Some(old) = self.entries.remove(index)
        {
            self.bytes -= old.bytes;
        }
        if bytes > MAX_BYTES {
            return;
        }
        while self.entries.len() >= MAX_SCOPES || self.bytes + bytes > MAX_BYTES {
            self.bytes -= self
                .entries
                .pop_front()
                .expect("cache exceeds its budget")
                .bytes;
        }
        self.bytes += bytes;
        self.entries.push_back(Entry {
            key,
            pages,
            loaded,
            bytes,
        });
    }
}

fn batch_bytes(key: &Key, pages: &[ScopedPage]) -> usize {
    let mut bytes = size_of::<Entry>()
        + key.directory.as_os_str().len()
        + key.query.len()
        + key.region.len()
        + key.account.profile.id.len()
        + key.account.profile.name.len();
    for page in pages {
        let mut encoded = Size::default();
        if serde_json::to_writer(&mut encoded, &*page.page).is_err() {
            return usize::MAX;
        }
        bytes = bytes.saturating_add(
            encoded.0
                + size_of::<ScopedPage>()
                + size_of::<MusicPage>()
                + page.profile.as_ref().map_or(0, String::len),
        );
    }
    bytes
}

#[derive(Default)]
struct Size(usize);

impl Write for Size {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
