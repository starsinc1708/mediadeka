use std::{
    cell::Cell,
    collections::VecDeque,
    io::{self, Read},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use crate::providers::{ProviderId, provider_index};

static API_BYTES: AtomicU64 = AtomicU64::new(0);
static ARTWORK_BYTES: AtomicU64 = AtomicU64::new(0);
static API_REQUESTS: AtomicU64 = AtomicU64::new(0);
static JOINED_READS: AtomicU64 = AtomicU64::new(0);
static CATALOG_FRESH: AtomicU64 = AtomicU64::new(0);
static CATALOG_STALE: AtomicU64 = AtomicU64::new(0);
static CATALOG_MISS: AtomicU64 = AtomicU64::new(0);
static CATALOG_ACTIVE: AtomicU64 = AtomicU64::new(0);
static CATALOG_FAILURES: AtomicU64 = AtomicU64::new(0);
static CATALOG_CANCELLED: AtomicU64 = AtomicU64::new(0);
static CATALOG_SOURCE_READS: [AtomicU64; ProviderId::ALL.len()] =
    [const { AtomicU64::new(0) }; ProviderId::ALL.len()];
static CATALOG_RESIDENT: Mutex<(usize, usize)> = Mutex::new((0, 0));
static CATALOG_LATENCIES: Mutex<VecDeque<Duration>> = Mutex::new(VecDeque::new());
static CATALOG_INVALIDATIONS: Mutex<CatalogInvalidations> = Mutex::new(CatalogInvalidations {
    cleared: 0,
    account_changed: 0,
    credential_revoked: 0,
    latest: None,
});
const CATALOG_LATENCY_SAMPLES: usize = 128;

thread_local! {
    static PREFETCH_BODY_BYTES: Cell<Option<u64>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Received {
    pub api_bytes: u64,
    pub artwork_bytes: u64,
    pub api_requests: u64,
    pub joined_reads: u64,
}

pub(crate) fn received() -> Received {
    Received {
        api_bytes: API_BYTES.load(Ordering::Relaxed),
        artwork_bytes: ARTWORK_BYTES.load(Ordering::Relaxed),
        api_requests: API_REQUESTS.load(Ordering::Relaxed),
        joined_reads: JOINED_READS.load(Ordering::Relaxed),
    }
}

pub(crate) fn api_request(
    request: ureq::http::Request<ureq::SendBody>,
    next: ureq::middleware::MiddlewareNext<'_>,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    API_REQUESTS.fetch_add(1, Ordering::Relaxed);
    next.handle(request)
}

pub(crate) fn joined_read() {
    JOINED_READS.fetch_add(1, Ordering::Relaxed);
}

pub(crate) enum CatalogAccess {
    Fresh,
    Stale,
    Miss,
}

pub(crate) fn catalog_open(access: CatalogAccess) {
    let counter = match access {
        CatalogAccess::Fresh => &CATALOG_FRESH,
        CatalogAccess::Stale => &CATALOG_STALE,
        CatalogAccess::Miss => &CATALOG_MISS,
    };
    counter.fetch_add(1, Ordering::Relaxed);
}

pub(crate) struct CatalogFlight;

impl CatalogFlight {
    pub(crate) fn start() -> Self {
        CATALOG_ACTIVE.fetch_add(1, Ordering::Relaxed);
        Self
    }
}

impl Drop for CatalogFlight {
    fn drop(&mut self) {
        CATALOG_ACTIVE.fetch_sub(1, Ordering::Relaxed);
    }
}

pub(crate) enum CatalogRead {
    Completed,
    Failed,
    Cancelled,
}

pub(crate) fn catalog_source_read(provider: ProviderId, elapsed: Duration, status: CatalogRead) {
    CATALOG_SOURCE_READS[provider_index(provider)].fetch_add(1, Ordering::Relaxed);
    match status {
        CatalogRead::Completed => {}
        CatalogRead::Failed => {
            CATALOG_FAILURES.fetch_add(1, Ordering::Relaxed);
        }
        CatalogRead::Cancelled => {
            CATALOG_CANCELLED.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
    let mut samples = CATALOG_LATENCIES
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if samples.len() == CATALOG_LATENCY_SAMPLES {
        samples.pop_front();
    }
    samples.push_back(elapsed);
}

pub(crate) fn catalog_resident(pages: usize, bytes: usize) {
    *CATALOG_RESIDENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = (pages, bytes);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CatalogInvalidation {
    CacheCleared,
    AccountChanged,
    CredentialRevoked,
}

#[derive(Clone, Copy)]
pub(crate) struct CatalogInvalidations {
    pub cleared: u64,
    pub account_changed: u64,
    pub credential_revoked: u64,
    pub latest: Option<CatalogInvalidation>,
}

pub(crate) fn catalog_invalidated(reason: CatalogInvalidation) {
    let mut events = CATALOG_INVALIDATIONS
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    match reason {
        CatalogInvalidation::CacheCleared => events.cleared += 1,
        CatalogInvalidation::AccountChanged => events.account_changed += 1,
        CatalogInvalidation::CredentialRevoked => events.credential_revoked += 1,
    }
    events.latest = Some(reason);
}

pub(crate) struct Catalog {
    pub fresh: u64,
    pub stale: u64,
    pub miss: u64,
    pub active: u64,
    pub failures: u64,
    pub cancelled: u64,
    pub resident_pages: usize,
    pub resident_bytes: usize,
    pub source_latency: Option<Latency>,
    pub source_reads: [(ProviderId, u64); ProviderId::ALL.len()],
    pub invalidations: CatalogInvalidations,
}

pub(crate) struct Latency {
    pub mean: Duration,
    pub p95: Duration,
}

pub(crate) fn catalog() -> Catalog {
    let (resident_pages, resident_bytes) = *CATALOG_RESIDENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let mut samples = CATALOG_LATENCIES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .copied()
        .collect::<Vec<_>>();
    let source_latency = (!samples.is_empty()).then(|| {
        samples.sort_unstable();
        let index = (samples.len() * 95).div_ceil(100) - 1;
        Latency {
            mean: samples.iter().sum::<Duration>() / samples.len() as u32,
            p95: samples[index],
        }
    });
    Catalog {
        fresh: CATALOG_FRESH.load(Ordering::Relaxed),
        stale: CATALOG_STALE.load(Ordering::Relaxed),
        miss: CATALOG_MISS.load(Ordering::Relaxed),
        active: CATALOG_ACTIVE.load(Ordering::Relaxed),
        failures: CATALOG_FAILURES.load(Ordering::Relaxed),
        cancelled: CATALOG_CANCELLED.load(Ordering::Relaxed),
        resident_pages,
        resident_bytes,
        source_latency,
        source_reads: ProviderId::ALL.map(|provider| {
            let reads = CATALOG_SOURCE_READS[provider_index(provider)].load(Ordering::Relaxed);
            (provider, reads)
        }),
        invalidations: *CATALOG_INVALIDATIONS
            .lock()
            .unwrap_or_else(PoisonError::into_inner),
    }
}

pub(crate) fn api_body(reader: impl Read) -> impl Read {
    CountedReader {
        reader,
        kind: BodyKind::Api,
    }
}

pub(crate) fn artwork_body(reader: impl Read) -> impl Read {
    CountedReader {
        reader,
        kind: BodyKind::Artwork,
    }
}

/// Measures synchronous API response body bytes consumed on this worker only.
pub(crate) fn prefetch_body_bytes<T>(read: impl FnOnce() -> T) -> (T, u64) {
    struct Restore(Option<u64>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PREFETCH_BODY_BYTES.set(self.0);
        }
    }

    let _restore = Restore(PREFETCH_BODY_BYTES.replace(Some(0)));
    let result = read();
    (result, PREFETCH_BODY_BYTES.get().unwrap_or(0))
}

enum BodyKind {
    Api,
    Artwork,
}

struct CountedReader<R> {
    reader: R,
    kind: BodyKind,
}

impl<R: Read> Read for CountedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.reader.read(buffer)?;
        if count != 0 {
            let bytes = count as u64;
            match self.kind {
                BodyKind::Api => {
                    API_BYTES.fetch_add(bytes, Ordering::Relaxed);
                    PREFETCH_BODY_BYTES
                        .update(|current| current.map(|count| count.saturating_add(bytes)));
                }
                BodyKind::Artwork => {
                    ARTWORK_BYTES.fetch_add(bytes, Ordering::Relaxed);
                }
            }
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speculative_bytes_count_only_api_bodies_on_the_prefetch_worker() {
        let (value, measured) = prefetch_body_bytes(|| {
            std::thread::spawn(|| {
                io::copy(&mut api_body(&b"foreground"[..]), &mut io::sink()).unwrap()
            })
            .join()
            .unwrap();
            io::copy(&mut artwork_body(&b"cover"[..]), &mut io::sink()).unwrap();
            io::copy(&mut api_body(&b"metadata"[..]), &mut io::sink()).unwrap();
            io::copy(&mut api_body(&b"second"[..]), &mut io::sink()).unwrap();
            17
        });
        assert_eq!((value, measured), (17, 14));
        assert_eq!(prefetch_body_bytes(|| ()).1, 0);
    }
}
