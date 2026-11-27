use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use crate::{
    music::station::recommendations::{
        self as stations, StationCandidates, StationSessions, StationSource,
    },
    playback::QueueEntry,
    providers::ProviderId,
    storage::{Store, StoreError},
};

use super::{Mix, Mode, Ranking, Selection, assemble_candidates};

const REFILL_REMAINING: usize = 4;

#[derive(Clone, Copy)]
pub(crate) enum Access {
    Live,
    #[cfg(any(test, feature = "capture"))]
    Saved,
}

#[derive(Clone)]
pub(crate) struct Feed {
    personal: Arc<Mix>,
    stations: Arc<[StationSource]>,
    sessions: StationSessions,
    mode: Mode,
    access: Access,
}

pub(crate) struct Batch {
    pub feed: Feed,
    pub unavailable: Vec<ProviderId>,
    ranking: Ranking,
    candidates: Vec<QueueEntry>,
}

pub(crate) fn should_refill(position: Option<usize>, queue_len: usize) -> bool {
    position.is_some_and(|index| queue_len.saturating_sub(index + 1) <= REFILL_REMAINING)
}

pub(crate) fn start(
    store: &Store,
    playlists: Vec<Selection>,
    sources: Vec<StationSource>,
    mode: Mode,
    access: Access,
    cancelled: &AtomicBool,
) -> Result<Batch, StoreError> {
    let read = || {
        read_stations(
            store,
            &sources,
            StationSessions::default(),
            access,
            cancelled,
        )
    };
    let (personal, station_batch) = thread::scope(|scope| {
        if playlists.is_empty() {
            return (Ok(Mix::default()), read());
        }
        let station_job = scope.spawn(read);
        let personal = match access {
            Access::Live => assemble_candidates(store, playlists, cancelled),
            #[cfg(any(test, feature = "capture"))]
            Access::Saved => super::assemble_cached_candidates(store, playlists, cancelled),
        };
        (
            personal,
            station_job.join().expect("recommendation station worker"),
        )
    });
    Feed {
        personal: Arc::new(personal?),
        stations: sources.into(),
        sessions: StationSessions::default(),
        mode,
        access,
    }
    .finish_batch(store, station_batch)
}

impl Feed {
    pub(crate) fn refill(
        mut self,
        store: &Store,
        cancelled: &AtomicBool,
    ) -> Result<Batch, StoreError> {
        let stations = read_stations(
            store,
            &self.stations,
            std::mem::take(&mut self.sessions),
            self.access,
            cancelled,
        );
        self.finish_batch(store, stations)
    }

    fn finish_batch(
        mut self,
        store: &Store,
        stations: StationCandidates,
    ) -> Result<Batch, StoreError> {
        let StationCandidates {
            mut entries,
            unavailable,
            sessions,
        } = stations;
        self.sessions = sessions;
        entries.sort_by_key(|entry| entry.index);
        let mut connected = BTreeSet::new();
        let mut lost = BTreeSet::new();
        for (provider, profile) in self
            .personal
            .entries
            .iter()
            .filter_map(|entry| entry.playlist.connected_source())
            .filter_map(|(provider, profile)| Some((provider, profile?)))
            .collect::<BTreeSet<_>>()
        {
            let account = store.account(provider, profile)?;
            if let Some(account) = account
                && !store.account_revoked(&account)?
            {
                connected.insert((provider, Some(profile)));
            } else {
                lost.insert(provider);
            }
        }
        let mut personal = self
            .personal
            .entries
            .iter()
            .filter(|entry| {
                entry
                    .playlist
                    .connected_source()
                    .is_some_and(|source| connected.contains(&source))
            })
            .cloned();
        let mut stations = entries.into_iter();
        let mut candidates = Vec::new();
        loop {
            let station = stations.next();
            let playlist = personal.next();
            if station.is_none() && playlist.is_none() {
                break;
            }
            candidates.extend(station);
            candidates.extend(playlist);
        }
        let mut unavailable =
            [self.personal.unavailable.as_slice(), unavailable.as_slice()].concat();
        unavailable.extend(lost);
        unavailable.sort_unstable();
        unavailable.dedup();
        Ok(Batch {
            ranking: Ranking::load(store, self.mode)?,
            feed: self,
            unavailable,
            candidates,
        })
    }
}

impl Batch {
    pub(crate) fn entries(&self, queued: &[QueueEntry]) -> Vec<QueueEntry> {
        // The ordinary queue also contains already played entries until replaced,
        // so using it here excludes both upcoming and recent wave recordings.
        self.ranking.entries(&self.candidates, queued)
    }

    pub(crate) fn continuation(
        &self,
        queue: u64,
        current: u64,
        queued: impl Iterator<Item = QueueEntry>,
        cancelled: &AtomicBool,
    ) -> Option<Vec<QueueEntry>> {
        if queue != current || cancelled.load(Ordering::Relaxed) {
            return None;
        }
        Some(self.entries(&queued.collect::<Vec<_>>()))
    }

    #[cfg(all(test, windows))]
    pub(crate) fn candidates(&self) -> &[QueueEntry] {
        &self.candidates
    }
}

fn read_stations(
    store: &Store,
    sources: &[StationSource],
    sessions: StationSessions,
    access: Access,
    cancelled: &AtomicBool,
) -> StationCandidates {
    if cancelled.load(Ordering::Relaxed) {
        return StationCandidates {
            entries: Vec::new(),
            unavailable: Vec::new(),
            sessions,
        };
    }
    match access {
        Access::Live => stations::load(store, sources, sessions, cancelled),
        #[cfg(any(test, feature = "capture"))]
        Access::Saved => stations::load_cached(store, sources, sessions, cancelled),
    }
}
