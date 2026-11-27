use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::{Playback, QueueEntry, Repeat, Status};

pub(crate) const RESUME_LIMIT: usize = 10_000;
const RETAIN_PLAYED: usize = 64;

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ResumeEntry {
    pub(crate) entry: QueueEntry,
    pub(crate) autoplay: bool,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct QueueResumePlan {
    pub(crate) session: String,
    pub(crate) entries: Vec<ResumeEntry>,
    pub(crate) order: Vec<usize>,
    pub(crate) shuffle: bool,
    #[serde(skip)]
    pub(super) start: usize,
    #[serde(skip)]
    pub(super) revision: u64,
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct QueueResumeProgress {
    pub(crate) current: Option<usize>,
    pub(crate) position_ms: u64,
    pub(crate) repeat: Repeat,
}

#[derive(Clone)]
pub(crate) struct QueueResumeUpdate {
    pub(crate) plan: Arc<QueueResumePlan>,
    pub(crate) progress: QueueResumeProgress,
}

pub(crate) struct QueueResume {
    pub(crate) plan: QueueResumePlan,
    pub(crate) progress: QueueResumeProgress,
}

impl QueueResumePlan {
    pub(super) fn window_start(count: usize, cursor: Option<usize>) -> usize {
        cursor
            .unwrap_or_default()
            .saturating_sub(RETAIN_PLAYED)
            .min(count.saturating_sub(RESUME_LIMIT))
    }

    pub(super) fn new(
        entries: Vec<ResumeEntry>,
        order: Vec<usize>,
        shuffle: bool,
        start: usize,
        revision: u64,
    ) -> Self {
        static NEXT_SESSION: AtomicU64 = AtomicU64::new(0);
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |now| now.as_nanos());
        let process = std::process::id();
        let sequence = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        Self {
            session: format!("{timestamp}-{process}-{sequence}"),
            entries,
            order,
            shuffle,
            start,
            revision,
        }
    }
}

impl QueueResume {
    pub(crate) fn retain_accessible(
        &mut self,
        mut allowed: impl FnMut(&QueueEntry) -> bool,
    ) -> bool {
        let count = self.plan.entries.len();
        if count > RESUME_LIMIT
            || self.plan.order.len() != count
            || self.progress.current.is_some_and(|cursor| cursor >= count)
        {
            return false;
        }
        let mut seen = vec![false; count];
        for &index in &self.plan.order {
            let Some(entry) = seen.get_mut(index) else {
                return false;
            };
            if std::mem::replace(entry, true) {
                return false;
            }
        }
        let mut remap = Vec::with_capacity(count);
        let mut entries = Vec::with_capacity(count);
        for entry in self.plan.entries.drain(..) {
            if entry.entry.is_playable() && allowed(&entry.entry) {
                remap.push(Some(entries.len()));
                entries.push(entry);
            } else {
                remap.push(None);
            }
        }
        let mut updated = None;
        let mut replacement = None;
        let mut order = Vec::with_capacity(entries.len());
        for (position, index) in self.plan.order.drain(..).enumerate() {
            let Some(index) = remap[index] else {
                continue;
            };
            if self.progress.current == Some(position) {
                updated = Some(order.len());
            } else if self
                .progress
                .current
                .is_some_and(|cursor| position > cursor)
            {
                replacement.get_or_insert(order.len());
            }
            order.push(index);
        }
        if updated.is_none() {
            self.progress.position_ms = 0;
        }
        self.progress.current = updated.or(replacement).or_else(|| {
            self.progress
                .current
                .and_then(|_| order.len().checked_sub(1))
        });
        self.plan.entries = entries;
        self.plan.order = order;
        true
    }
}

impl Playback {
    pub(crate) fn restore_queue(&mut self, saved: QueueResume) {
        self.queue.restore(saved.plan, saved.progress.current);
        self.repeat = saved.progress.repeat;
        if let Some(current) = self.current() {
            let seconds = saved.progress.position_ms as f64 / 1_000.;
            let position = seconds.min(current.track.duration.map_or(f64::MAX, f64::from));
            self.resume_position = Some(position);
            self.frame.position = position;
        }
    }

    pub(crate) fn queue_resume_update(
        &self,
        known: Option<&Arc<QueueResumePlan>>,
    ) -> Option<QueueResumeUpdate> {
        if self.queue.revision() == 0 {
            return None;
        }
        let plan = known
            .filter(|plan| {
                plan.revision == self.queue.revision()
                    && self.current_index().is_none_or(|cursor| {
                        (plan.start..plan.start + plan.order.len()).contains(&cursor)
                    })
            })
            .cloned()
            .unwrap_or_else(|| {
                let start =
                    QueueResumePlan::window_start(self.queue.entries().len(), self.current_index());
                Arc::new(self.queue.resume_plan(start))
            });
        let current = self.current_index().map(|index| index - plan.start);
        let position = if self.frame.completed || current.is_none() {
            0.
        } else {
            self.position()
        };
        Some(QueueResumeUpdate {
            plan,
            progress: QueueResumeProgress {
                current,
                position_ms: (position.max(0.) * 1_000.).round() as u64,
                repeat: self.repeat,
            },
        })
    }

    pub(super) fn play_current_or_resume(&mut self) {
        let resume = self.resume_position;
        self.start_current(resume.unwrap_or(0.));
        self.resume_position = resume;
    }

    pub(crate) fn dormant(&self) -> bool {
        self.resume_position.is_some() && matches!(self.frame.status, Status::Idle)
    }
}
