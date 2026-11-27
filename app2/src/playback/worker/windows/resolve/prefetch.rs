use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use async_channel::Sender;

use crate::{
    music::sound,
    playback::{PREFETCH_TAIL, QueueEntry},
};

use super::{Resolved, speculative};

const PREPARED_LIFETIME: Duration = Duration::from_secs(45);
const RENEW_AFTER: Duration = PREPARED_LIFETIME.saturating_sub(PREFETCH_TAIL);

#[derive(Clone, Default)]
pub(in crate::playback::worker::windows) struct Ticket {
    value: Arc<Mutex<Option<(Instant, Resolved)>>>,
    cancelled: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}

impl Ticket {
    pub(super) fn ready(&self) -> Option<Resolved> {
        let value = self.value.lock().ok()?;
        let (prepared, stream) = value.as_ref()?;
        (prepared.elapsed() < PREPARED_LIFETIME && !self.cancelled.load(Ordering::Acquire))
            .then(|| stream.clone())
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn expires_before_track_end(&self) -> bool {
        self.finished.load(Ordering::Acquire)
            && self.value.lock().is_ok_and(|value| {
                value
                    .as_ref()
                    .is_none_or(|(prepared, _)| prepared.elapsed() >= RENEW_AFTER)
            })
    }
}

#[derive(Clone)]
struct Task {
    entry: QueueEntry,
    ticket: Ticket,
}

impl Task {
    fn cancel(&self) {
        self.ticket.cancel();
        #[cfg(test)]
        tests::observe(tests::Event::Cancelled(self.entry.index));
    }
}

pub(in crate::playback::worker::windows) struct Prefetch {
    worker: Sender<Task>,
    upcoming: Option<Task>,
}

impl Prefetch {
    pub(in crate::playback::worker::windows) fn spawn() -> Result<Self, String> {
        let (worker, queue) = async_channel::bounded::<Task>(1);
        thread::Builder::new()
            .name("mediadeka-audio-prefetch".into())
            .spawn(move || {
                while let Ok(Task { entry, ticket }) = queue.recv_blocking() {
                    if !ticket.cancelled.load(Ordering::Acquire)
                        && let Some(stream) = speculative(&entry, &ticket.cancelled)
                        && let Ok(mut value) = ticket.value.lock()
                        && !ticket.cancelled.load(Ordering::Acquire)
                    {
                        *value = Some((Instant::now(), stream));
                    }
                    ticket.finished.store(true, Ordering::Release);
                    #[cfg(test)]
                    tests::observe(tests::Event::Settled {
                        index: entry.index,
                        ready: ticket.ready().is_some(),
                    });
                }
            })
            .map_err(|_| "Не удалось запустить предзагрузку очереди.")?;
        Ok(Self {
            worker,
            upcoming: None,
        })
    }

    pub(in crate::playback::worker::windows) fn replace(
        &mut self,
        entries: Vec<QueueEntry>,
        near_end: bool,
    ) {
        // Audio preparation follows the actual next occurrence. A local file
        // already needs no network preparation.
        let next = entries
            .into_iter()
            .next()
            .filter(|entry| sound::recording(&entry.playlist, &entry.track).is_some());
        if self
            .upcoming
            .as_ref()
            .zip(next.as_ref())
            .is_some_and(|(task, entry)| {
                task.entry.same_recording(entry)
                    && (!near_end || !task.ticket.expires_before_track_end())
            })
        {
            return;
        }
        if let Some(task) = self.upcoming.take() {
            task.cancel();
        }
        if let Some(entry) = next {
            let task = Task {
                entry,
                ticket: Ticket::default(),
            };
            self.upcoming = Some(task.clone());
            // Keep one queued preparation while an earlier request unwinds.
            let _ = self.worker.force_send(task);
        }
    }

    pub(in crate::playback::worker::windows) fn take(
        &mut self,
        entry: &QueueEntry,
    ) -> Option<Ticket> {
        let task = self
            .upcoming
            .take_if(|task| task.entry.same_recording(entry))?;
        if task.ticket.ready().is_some() {
            Some(task.ticket)
        } else {
            task.cancel();
            None
        }
    }
}

impl Drop for Prefetch {
    fn drop(&mut self) {
        if let Some(task) = &self.upcoming {
            task.cancel();
        }
        self.worker.close();
    }
}

#[cfg(test)]
mod tests;
