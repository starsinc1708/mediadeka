use std::time::{Duration, Instant};

use gpui::{Context, Task};

use crate::{music::lyrics, playback::QueueEntry, storage::Store};

use super::{Job, LyricsView};

const RETRY_DELAY: Duration = Duration::from_secs(20);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(2 * 60);

#[derive(Default)]
pub(super) struct Prefetch {
    window: Vec<QueueEntry>,
    results: Vec<(QueueEntry, Outcome)>,
    active: Option<Active>,
    wake: Option<(Instant, Task<()>)>,
}

struct Active {
    entry: QueueEntry,
    _job: Job,
}

enum Outcome {
    Prepared,
    Missing,
    Retry { at: Instant, delay: Duration },
}

impl Prefetch {
    pub(super) fn reconcile<'a>(
        &mut self,
        current: Option<&QueueEntry>,
        upcoming: impl Iterator<Item = &'a QueueEntry>,
    ) {
        let window: Vec<_> = upcoming
            .filter(|entry| !current.is_some_and(|current| same_recording(current, entry)))
            .collect();
        let wanted = |entry: &QueueEntry| window.iter().any(|item| same_recording(item, entry));
        if self.active.as_ref().is_some_and(|active| {
            !wanted(&active.entry)
                && !current.is_some_and(|current| same_recording(current, &active.entry))
        }) {
            self.active = None;
        }
        self.results.retain(|(entry, _)| wanted(entry));
        if !self
            .window
            .iter()
            .zip(&window)
            .all(|(old, new)| same_recording(old, new))
            || self.window.len() != window.len()
        {
            self.window = window.into_iter().map(recording).collect();
        }
    }

    fn pending(&self, now: Instant) -> Option<QueueEntry> {
        self.window
            .iter()
            .find(|entry| {
                match self
                    .results
                    .iter()
                    .find(|(done, _)| same_recording(done, entry))
                {
                    None => true,
                    Some((_, Outcome::Retry { at, .. })) => *at <= now,
                    Some((_, Outcome::Prepared | Outcome::Missing)) => false,
                }
            })
            .cloned()
    }

    fn finish(&mut self, entry: QueueEntry, result: Result<bool, String>, now: Instant) {
        if !self
            .window
            .iter()
            .any(|wanted| same_recording(wanted, &entry))
        {
            return;
        }
        let previous = self
            .results
            .iter()
            .position(|(seen, _)| same_recording(seen, &entry))
            .map(|index| self.results.swap_remove(index).1);
        let outcome = match result {
            Ok(true) => Outcome::Prepared,
            Ok(false) => Outcome::Missing,
            Err(_) => {
                let delay = match previous {
                    Some(Outcome::Retry { delay, .. }) => (delay * 2).min(RETRY_MAX_DELAY),
                    _ => RETRY_DELAY,
                };
                Outcome::Retry {
                    at: now + delay,
                    delay,
                }
            }
        };
        self.results.push((entry, outcome));
    }

    fn next_retry(&self) -> Option<Instant> {
        self.results
            .iter()
            .filter_map(|(_, result)| match result {
                Outcome::Retry { at, .. } => Some(*at),
                Outcome::Prepared | Outcome::Missing => None,
            })
            .min()
    }

    fn schedule_retry(&mut self, now: Instant, cx: &mut Context<LyricsView>) {
        let next = self.next_retry();
        if self.wake.as_ref().map(|(at, _)| *at) == next {
            return;
        }
        self.wake = next.map(|at| {
            let task = cx.spawn(async move |view, cx| {
                cx.background_executor()
                    .timer(at.saturating_duration_since(now))
                    .await;
                let _ = view.update(cx, |view, cx| {
                    view.prefetch.wake = None;
                    view.start_upcoming(cx);
                });
            });
            (at, task)
        });
    }

    #[cfg(feature = "capture")]
    pub(super) fn counts(&self) -> (usize, usize, usize) {
        (
            self.window.len(),
            self.results
                .iter()
                .filter(|(_, outcome)| matches!(outcome, Outcome::Prepared | Outcome::Missing))
                .count(),
            self.results
                .iter()
                .filter(|(_, outcome)| matches!(outcome, Outcome::Prepared))
                .count(),
        )
    }
}

impl LyricsView {
    pub(super) fn start_upcoming(&mut self, cx: &mut Context<Self>) {
        if self.load.is_some() || self.prefetch.active.is_some() {
            return;
        }
        let now = Instant::now();
        let Some(entry) = self.prefetch.pending(now) else {
            self.prefetch.schedule_retry(now, cx);
            return;
        };
        self.prefetch.wake = None;
        let requested = entry.clone();
        let job = Job::run::<(), bool>(
            cx,
            move |flag, _| {
                let store = Store::open_default().map_err(|error| error.to_string())?;
                let mut found = false;
                lyrics::prefetch(&store, &requested, flag, |_| {
                    found = true;
                })?;
                Ok(found)
            },
            |_, (), _| {},
            |view, result, cx| {
                if let Some(active) = view.prefetch.active.take() {
                    view.prefetch.finish(active.entry, result, Instant::now());
                }
                view.start_upcoming(cx);
            },
        );
        self.prefetch.active = Some(Active { entry, _job: job });
    }
}

#[cfg(test)]
mod tests;

fn same_recording(a: &QueueEntry, b: &QueueEntry) -> bool {
    a.same_recording(b)
        && a.track.title == b.track.title
        && a.track.artist == b.track.artist
        && a.track.artists == b.track.artists
        && a.track.album == b.track.album
        && a.track.duration == b.track.duration
}

fn recording(entry: &QueueEntry) -> QueueEntry {
    QueueEntry {
        playlist: entry.playlist.clone(),
        playlist_revision: entry.playlist_revision,
        index: entry.index,
        track: entry.track.clone(),
        recommendations: Vec::new(),
    }
}
