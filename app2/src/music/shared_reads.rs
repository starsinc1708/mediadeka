use std::{
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    providers::{ErrorKind, ProviderError, ProviderId},
    telemetry::{self, CatalogRead},
};

type ReadResult<T> = Result<T, ProviderError>;
const CANCELLATION_POLL: Duration = Duration::from_millis(40);

struct Flight<K, T, P> {
    key: K,
    state: Mutex<FlightState<T, P>>,
    completed: Condvar,
}

struct FlightState<T, P> {
    result: Option<ReadResult<T>>,
    progress: Vec<P>,
}

pub(crate) struct SharedReads<K, T, P = ()> {
    pending: Mutex<Vec<Arc<Flight<K, T, P>>>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl<K: Clone + Eq, T: Clone, P: Clone> SharedReads<K, T, P> {
    pub(crate) const fn new() -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn read_with_progress(
        &self,
        key: &K,
        cancelled: &AtomicBool,
        notify: impl Fn(&P),
        load: impl FnOnce(&dyn Fn(P)) -> ReadResult<T>,
    ) -> ReadResult<T> {
        loop {
            check_cancelled(cancelled)?;
            let (flight, leader) = {
                let mut pending = lock(&self.pending);
                if let Some(flight) = pending.iter().find(|flight| &flight.key == key) {
                    (Arc::clone(flight), false)
                } else {
                    let flight = Arc::new(Flight {
                        key: key.clone(),
                        state: Mutex::new(FlightState {
                            result: None,
                            progress: Vec::new(),
                        }),
                        completed: Condvar::new(),
                    });
                    pending.push(Arc::clone(&flight));
                    (flight, true)
                }
            };
            if leader {
                let _active = telemetry::CatalogFlight::start();
                let leader = Leader {
                    reads: self,
                    flight,
                };
                let result = load(&|progress| {
                    if check_cancelled(cancelled).is_ok() {
                        lock(&leader.flight.state).progress.push(progress.clone());
                        leader.flight.completed.notify_all();
                        notify(&progress);
                    }
                });
                lock(&leader.flight.state).result = Some(result.clone());
                return result;
            }

            telemetry::joined_read();
            let mut delivered = 0;
            loop {
                check_cancelled(cancelled)?;
                let state = lock(&flight.state);
                if let Some(progress) = state.progress.get(delivered).cloned() {
                    delivered += 1;
                    drop(state);
                    notify(&progress);
                } else if let Some(result) = state.result.as_ref() {
                    if matches!(result, Err(error) if error.kind == ErrorKind::Cancelled) {
                        break;
                    }
                    return result.clone();
                } else {
                    let (_state, _) = flight
                        .completed
                        .wait_timeout(state, CANCELLATION_POLL)
                        .unwrap_or_else(PoisonError::into_inner);
                }
            }
            // If the leading caller changed route, an interested waiter can retry.
        }
    }
}

impl<K: Clone + Eq, T: Clone> SharedReads<K, T> {
    pub(crate) fn read(
        &self,
        key: &K,
        cancelled: &AtomicBool,
        load: impl FnOnce() -> ReadResult<T>,
    ) -> ReadResult<T> {
        self.read_with_progress(key, cancelled, |_| {}, |_| load())
    }
}

struct Leader<'a, K, T, P> {
    reads: &'a SharedReads<K, T, P>,
    flight: Arc<Flight<K, T, P>>,
}

impl<K, T, P> Drop for Leader<'_, K, T, P> {
    fn drop(&mut self) {
        lock(&self.reads.pending).retain(|flight| !Arc::ptr_eq(flight, &self.flight));
        lock(&self.flight.state)
            .result
            .get_or_insert_with(|| Err(cancelled_error()));
        self.flight.completed.notify_all();
    }
}

pub(crate) fn check_cancelled(cancelled: &AtomicBool) -> Result<(), ProviderError> {
    if cancelled.load(Ordering::Relaxed) {
        Err(cancelled_error())
    } else {
        Ok(())
    }
}

pub(crate) fn trace_source<T>(
    provider: ProviderId,
    cancelled: &AtomicBool,
    load: impl FnOnce() -> Result<T, ProviderError>,
) -> Result<T, ProviderError> {
    check_cancelled(cancelled)?;
    let started = Instant::now();
    let result = load();
    let status = match &result {
        _ if cancelled.load(Ordering::Relaxed) => CatalogRead::Cancelled,
        Err(error) if error.kind == ErrorKind::Cancelled => CatalogRead::Cancelled,
        Err(_) => CatalogRead::Failed,
        Ok(_) => CatalogRead::Completed,
    };
    telemetry::catalog_source_read(provider, started.elapsed(), status);
    check_cancelled(cancelled)?;
    result
}

fn cancelled_error() -> ProviderError {
    ProviderError {
        kind: ErrorKind::Cancelled,
        message: "Загрузка прервана.".into(),
    }
}

pub(crate) fn cache_cleared() -> ProviderError {
    ProviderError {
        kind: ErrorKind::Unavailable,
        message: "Снимки каталога очищены. Повторите загрузку страницы.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc::sync_channel, thread, time::Instant};

    #[test]
    fn concurrent_catalog_reads_share_one_load_and_isolate_stores() {
        let reads = &SharedReads::new();
        let key = "pending-test-store";
        let flag = AtomicBool::new(false);
        let (started, running) = sync_channel(0);
        let (release, wait) = sync_channel(0);
        thread::scope(|scope| {
            let flag = &flag;
            let first = scope.spawn(move || {
                reads.read(&key, flag, || {
                    started.send(()).unwrap();
                    wait.recv_timeout(Duration::from_secs(3)).unwrap();
                    Ok(Arc::new("Shared"))
                })
            });
            running.recv_timeout(Duration::from_secs(3)).unwrap();
            let second = scope.spawn(|| {
                reads.read(&key, flag, || {
                    panic!("a concurrent read started another load")
                })
            });
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if lock(&reads.pending)
                    .iter()
                    .any(|flight| flight.key == key && Arc::strong_count(flight) > 2)
                {
                    break;
                }
                assert!(Instant::now() < deadline, "the second caller did not join");
                thread::yield_now();
            }
            let isolated = reads
                .read(&"other-pending-test-store", flag, || Ok(Arc::new("Other")))
                .unwrap();
            release.send(()).unwrap();
            assert!(Arc::ptr_eq(
                &first.join().unwrap().unwrap(),
                &second.join().unwrap().unwrap()
            ));
            assert_eq!(*isolated, "Other");
        });
    }

    #[test]
    fn joining_catalog_read_replays_received_pages_before_completion() {
        let reads = &SharedReads::<_, _, usize>::new();
        let flag = &AtomicBool::new(false);
        let (first_page, ready) = sync_channel(0);
        let (release, hold) = sync_channel(0);
        let (pages, observed) = sync_channel(2);
        thread::scope(|scope| {
            let leader = scope.spawn(move || {
                reads.read_with_progress(
                    &"playlist",
                    flag,
                    |_| {},
                    |publish| {
                        publish(200);
                        first_page.send(()).unwrap();
                        hold.recv_timeout(Duration::from_secs(3)).unwrap();
                        publish(400);
                        Ok(420)
                    },
                )
            });
            ready.recv_timeout(Duration::from_secs(3)).unwrap();
            let follower = scope.spawn(move || {
                reads.read_with_progress(
                    &"playlist",
                    flag,
                    |page| pages.send(*page).unwrap(),
                    |_| panic!("joining a paginated read started another load"),
                )
            });
            assert_eq!(observed.recv_timeout(Duration::from_secs(3)).unwrap(), 200);
            release.send(()).unwrap();
            assert_eq!(observed.recv_timeout(Duration::from_secs(3)).unwrap(), 400);
            assert_eq!(leader.join().unwrap().unwrap(), 420);
            assert_eq!(follower.join().unwrap().unwrap(), 420);
        });
    }
}
