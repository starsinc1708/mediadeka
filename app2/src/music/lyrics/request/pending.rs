use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    music::lyrics::{Line, LyricsVersions, Source, Word, local},
    storage::Account,
    telemetry,
};

const CANCELLATION_POLL: Duration = Duration::from_millis(40);
pub(super) const LOCAL_RETENTION: Duration = Duration::from_secs(30 * 60);
pub(super) const ABSENCE_RETENTION: Duration = Duration::from_secs(15 * 60);
const CACHE_LIMIT: usize = 128;
const CACHE_BYTES: usize = 8 * 1024 * 1024;
const SOURCE_COUNT: usize = Source::Genius as usize + 1;
const CURRENT_REQUEST_LIMIT: u8 = 2;

pub(super) static NETWORK: Network = Network::new();

pub(super) struct Network {
    active: Mutex<[u8; SOURCE_COUNT]>,
    free: Condvar,
}

impl Network {
    const fn new() -> Self {
        Self {
            active: Mutex::new([0; SOURCE_COUNT]),
            free: Condvar::new(),
        }
    }

    pub(super) fn acquire(
        &self,
        source: Source,
        foreground: &AtomicBool,
        cancelled: &AtomicBool,
    ) -> Option<Slot<'_>> {
        // Queue warmup starts only when the source is idle. A current track
        // may still make progress while a previous request finishes.
        let limit = || {
            if foreground.load(Ordering::Relaxed) {
                CURRENT_REQUEST_LIMIT
            } else {
                1
            }
        };
        let index = source as usize;
        let mut active = lock(&self.active);
        while active[index] >= limit() && !cancelled.load(Ordering::Relaxed) {
            (active, _) = self
                .free
                .wait_timeout(active, CANCELLATION_POLL)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        active[index] += 1;
        Some(Slot {
            network: self,
            index,
        })
    }
}

pub(super) struct Slot<'a> {
    network: &'a Network,
    index: usize,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        lock(&self.network.active)[self.index] -= 1;
        self.network.free.notify_all();
    }
}

#[derive(Clone, Eq, PartialEq)]
pub(super) enum Key {
    Local(PathBuf, Option<local::Revision>),
    Remote {
        directory: PathBuf,
        account: Option<Account>,
        key: String,
    },
}

pub(super) struct Loaded {
    pub text: Option<Arc<LyricsVersions>>,
    pub retain_for: Option<Duration>,
}

type Result = std::result::Result<Option<Arc<LyricsVersions>>, String>;

struct Flight {
    foreground: AtomicBool,
    outcome: Mutex<Option<Outcome>>,
    ready: Condvar,
}

#[derive(Clone)]
enum Outcome {
    Complete(Result),
    Cancelled,
}

struct Cached {
    key: Key,
    text: Option<Arc<LyricsVersions>>,
    expires: Instant,
    weight: usize,
}

#[derive(Default)]
struct Requests {
    flights: Vec<(Key, Arc<Flight>)>,
    cache: VecDeque<Cached>,
    bytes: usize,
}

static REQUESTS: Mutex<Option<Requests>> = Mutex::new(None);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(super) fn read(
    key: Key,
    cancelled: &AtomicBool,
    foreground: bool,
    retry_missing: bool,
    load: impl FnOnce(&AtomicBool) -> std::result::Result<Loaded, String>,
) -> Result {
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }

        let (flight, leader) = {
            let mut state = lock(&REQUESTS);
            let requests = state.get_or_insert_with(Requests::default);
            if let Some(text) = requests.cached(&key, retry_missing) {
                return Ok(text);
            }
            requests.join(&key, foreground)
        };
        if leader {
            let leader = Leader { key: &key, flight };
            let result = load(&leader.flight.foreground);
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let result = result.map(|loaded| {
                if let Some(retention) = loaded.retain_for {
                    lock(&REQUESTS).as_mut().expect("joined request").remember(
                        &key,
                        loaded.text.clone(),
                        retention,
                    );
                }
                loaded.text
            });
            *lock(&leader.flight.outcome) = Some(Outcome::Complete(result.clone()));
            return result;
        }

        telemetry::joined_read();
        let mut outcome = lock(&flight.outcome);
        while outcome.is_none() && !cancelled.load(Ordering::Relaxed) {
            (outcome, _) = flight
                .ready
                .wait_timeout(outcome, CANCELLATION_POLL)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if let Some(Outcome::Complete(result)) = outcome.as_ref() {
            return result.clone();
        }
        // A caller still interested in this answer replaces a cancelled leader.
    }
}

impl Requests {
    fn join(&mut self, key: &Key, foreground: bool) -> (Arc<Flight>, bool) {
        if let Some((_, flight)) = self.flights.iter().find(|(existing, _)| existing == key) {
            if foreground && !flight.foreground.swap(true, Ordering::Relaxed) {
                NETWORK.free.notify_all();
            }
            return (Arc::clone(flight), false);
        }
        let flight = Arc::new(Flight {
            foreground: AtomicBool::new(foreground),
            outcome: Mutex::new(None),
            ready: Condvar::new(),
        });
        self.flights.push((key.clone(), Arc::clone(&flight)));
        (flight, true)
    }

    fn cached(&mut self, key: &Key, retry_missing: bool) -> Option<Option<Arc<LyricsVersions>>> {
        let position = self.cache.iter().position(|entry| entry.key == *key)?;
        let entry = self.cache.remove(position)?;
        if entry.expires <= Instant::now() || retry_missing && entry.text.is_none() {
            self.bytes -= entry.weight;
            return None;
        }
        let result = entry.text.clone();
        self.cache.push_back(entry);
        Some(result)
    }

    fn remember(&mut self, key: &Key, text: Option<Arc<LyricsVersions>>, retention: Duration) {
        let text_bytes = text.as_deref().map_or(0, |versions| {
            size_of::<LyricsVersions>()
                + std::iter::once(versions.preferred())
                    .chain(versions.alternate_plain())
                    .map(|lyrics| {
                        lyrics.lines.capacity() * size_of::<Line>()
                            + lyrics
                                .lines
                                .iter()
                                .map(|line| {
                                    line.text.capacity() + line.words.capacity() * size_of::<Word>()
                                })
                                .sum::<usize>()
                            + lyrics.attribution.as_ref().map_or(0, String::capacity)
                    })
                    .sum::<usize>()
        });
        let weight = size_of::<Cached>()
            + text_bytes
            + match key {
                Key::Local(path, _) => path.as_os_str().len(),
                Key::Remote { directory, key, .. } => directory.as_os_str().len() + key.len(),
            };
        if weight > CACHE_BYTES {
            return;
        }
        while self.cache.len() >= CACHE_LIMIT || self.bytes + weight > CACHE_BYTES {
            let entry = self.cache.pop_front().expect("cache exceeds its budget");
            self.bytes -= entry.weight;
        }
        self.bytes += weight;
        self.cache.push_back(Cached {
            key: key.clone(),
            text,
            expires: Instant::now() + retention,
            weight,
        });
    }
}

struct Leader<'a> {
    key: &'a Key,
    flight: Arc<Flight>,
}

impl Drop for Leader<'_> {
    fn drop(&mut self) {
        if let Some(requests) = lock(&REQUESTS).as_mut() {
            requests.flights.retain(|(key, _)| key != self.key);
        }
        lock(&self.flight.outcome).get_or_insert(Outcome::Cancelled);
        self.flight.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::lyrics::Lyrics;
    use std::{sync::mpsc, thread};

    fn key(directory: &std::path::Path) -> Key {
        Key::Remote {
            directory: directory.to_owned(),
            account: None,
            key: "recording".into(),
        }
    }

    #[test]
    fn concurrent_reads_join_the_same_scoped_load() {
        let profile = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let key = key(profile.path());
        let other_key = Key::Remote {
            directory: other.path().to_owned(),
            account: None,
            key: "recording".into(),
        };
        let cancel = AtomicBool::new(false);
        let (started, running) = mpsc::channel();
        let (release, hold) = mpsc::channel();
        thread::scope(|scope| {
            let owner_key = key.clone();
            let cancel = &cancel;
            let first = scope.spawn(move || {
                read(owner_key, cancel, false, false, |foreground| {
                    started.send(()).unwrap();
                    hold.recv_timeout(Duration::from_secs(4)).unwrap();
                    assert!(foreground.load(Ordering::Relaxed));
                    Ok(Loaded {
                        text: Some(Arc::new(
                            Lyrics::plain(Source::Lrclib, "Known lyrics").into(),
                        )),
                        retain_for: None,
                    })
                })
            });
            running.recv_timeout(Duration::from_secs(2)).unwrap();
            let follower_key = key.clone();
            let follower = scope.spawn(move || {
                read(follower_key, cancel, true, false, |_| {
                    panic!("duplicate lookup")
                })
            });
            let deadline = Instant::now() + Duration::from_secs(2);
            loop {
                if lock(&REQUESTS)
                    .as_ref()
                    .unwrap()
                    .flights
                    .iter()
                    .any(|(running, flight)| running == &key && Arc::strong_count(flight) > 2)
                {
                    break;
                }
                assert!(Instant::now() < deadline, "lookup was not joined");
                thread::yield_now();
            }
            assert!(
                read(other_key, cancel, true, false, |_| Ok(Loaded {
                    text: None,
                    retain_for: None
                }))
                .unwrap()
                .is_none()
            );
            release.send(()).unwrap();
            let first = first.join().unwrap().unwrap().unwrap();
            let second = follower.join().unwrap().unwrap().unwrap();
            assert!(Arc::ptr_eq(&first, &second));
        });
    }

    #[test]
    fn confirmed_absence_is_reused_but_a_failure_is_retried() {
        let profile = tempfile::tempdir().unwrap();
        let key = key(profile.path());
        let cancel = AtomicBool::new(false);
        assert!(read(key.clone(), &cancel, true, false, |_| Err("offline".into())).is_err());
        assert!(
            read(key.clone(), &cancel, true, false, |_| Ok(Loaded {
                text: None,
                retain_for: Some(ABSENCE_RETENTION),
            }))
            .unwrap()
            .is_none()
        );
        assert!(
            read(key.clone(), &cancel, true, false, |_| panic!(
                "confirmed absence requested again"
            ))
            .unwrap()
            .is_none()
        );
        assert!(
            read(key, &cancel, true, true, |_| Err(
                "retry reached the service".into()
            ))
            .is_err()
        );
    }
}
