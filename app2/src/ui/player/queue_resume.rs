use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{Context, EventEmitter};

use crate::playback::{QueueResume, QueueResumeUpdate};

use super::PlayerBar;

const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(5);

pub(super) struct Tracking {
    pending_restore: bool,
    previous: Option<QueueResumeUpdate>,
    checkpointed_at: Instant,
}

impl EventEmitter<QueueResumeUpdate> for PlayerBar {}

impl PlayerBar {
    pub(in crate::ui) fn begin_queue_restore(&mut self) {
        self.queue_resume = Some(Tracking {
            pending_restore: self.playback.queue_revision() == 0,
            previous: None,
            checkpointed_at: Instant::now(),
        });
    }

    pub(in crate::ui) fn finish_queue_restore(
        &mut self,
        restored: Option<QueueResume>,
        cx: &mut Context<Self>,
    ) {
        let Some(tracking) = &mut self.queue_resume else {
            return;
        };
        if std::mem::take(&mut tracking.pending_restore)
            && let Some(restored) = restored
        {
            self.playback.restore_queue(restored);
            self.refresh_cover();
            self.sync_progress(cx);
            cx.notify();
        }
    }

    pub(in crate::ui) fn dormant(&self) -> bool {
        self.playback.dormant()
    }

    pub(super) fn cancel_queue_restore(&mut self) {
        if let Some(tracking) = &mut self.queue_resume {
            tracking.pending_restore = false;
        }
    }

    pub(super) fn publish_queue_resume(&mut self, flush: bool, cx: &mut Context<Self>) {
        if let Some(update) = self.queue_resume_checkpoint(flush, false) {
            cx.emit(update);
        }
    }

    pub(in crate::ui) fn queue_resume_on_close(&mut self) -> Option<QueueResumeUpdate> {
        // Submit the final state even after an earlier checkpoint failed to commit.
        self.queue_resume_checkpoint(true, true)
    }

    fn queue_resume_checkpoint(&mut self, flush: bool, closing: bool) -> Option<QueueResumeUpdate> {
        let tracking = self.queue_resume.as_mut()?;
        if tracking.pending_restore {
            return None;
        }
        let previous = tracking.previous.as_ref();
        let update = self
            .playback
            .queue_resume_update(previous.map(|update| &update.plan))?;
        let changed = previous.is_none_or(|previous| {
            !Arc::ptr_eq(&update.plan, &previous.plan)
                || update.progress.current != previous.progress.current
                || update.progress.repeat != previous.progress.repeat
        });
        if !closing
            && !changed
            && ((!flush && tracking.checkpointed_at.elapsed() < CHECKPOINT_INTERVAL)
                || previous.is_some_and(|previous| previous.progress == update.progress))
        {
            return None;
        }
        tracking.checkpointed_at = Instant::now();
        tracking.previous = Some(update.clone());
        Some(update)
    }
}
