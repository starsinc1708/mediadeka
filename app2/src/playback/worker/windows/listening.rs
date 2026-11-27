use std::time::{Duration, Instant};

use super::{Frame, Status};

const POSITION_JITTER: Duration = Duration::from_millis(280);
const SOLOIST_SEEK_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Default)]
pub(super) struct ListeningClock {
    anchor: Option<(f64, Instant)>,
    seek: Option<Seek>,
    revision: u64,
}

enum Seek {
    Mpv,
    Soloist { sequence: u64, requested: Instant },
}

impl ListeningClock {
    pub(super) fn mpv_seeking(&mut self) {
        self.anchor = None;
        self.seek = Some(Seek::Mpv);
    }

    pub(super) fn mpv_seek_pending(&self) -> bool {
        matches!(self.seek, Some(Seek::Mpv))
    }

    pub(super) fn soloist_seek(&mut self, sequence: u64, requested: Instant) {
        self.anchor = None;
        self.seek = Some(Seek::Soloist {
            sequence,
            requested,
        });
    }

    pub(super) fn soloist_position(&mut self, confirmed: Option<u64>, now: Instant) {
        if let Some(Seek::Soloist {
            sequence,
            requested,
        }) = self.seek
        {
            if confirmed == Some(sequence) {
                self.landed();
            } else if now.duration_since(requested) >= SOLOIST_SEEK_TIMEOUT {
                self.seek = None;
            }
        }
    }

    pub(super) fn landed(&mut self) {
        self.seek = None;
        self.revision = self.revision.wrapping_add(1);
    }

    pub(super) fn sample(&mut self, frame: &mut Frame, now: Instant) {
        frame.seek_revision = self.revision;
        if self.seek.is_some() || frame.source.is_none() {
            self.anchor = None;
            return;
        }
        let playing = frame.status == Status::Playing;
        let position = frame.position;
        if let Some((previous, at)) = self.anchor {
            let delta = position - previous;
            let elapsed = now.duration_since(at);
            if let Ok(progress) = Duration::try_from_secs_f64(delta) {
                if progress <= elapsed + POSITION_JITTER {
                    frame.listened += progress;
                } else {
                    // An external seek can change Soloist's position without a local command.
                    self.revision = self.revision.wrapping_add(1);
                    frame.seek_revision = self.revision;
                }
            } else if position.is_finite() && previous - position > POSITION_JITTER.as_secs_f64() {
                self.revision = self.revision.wrapping_add(1);
                frame.seek_revision = self.revision;
            }
            if playing
                && (previous - position).abs() <= POSITION_JITTER.as_secs_f64()
                && position <= previous
            {
                return;
            }
        }
        self.anchor = playing.then_some((position, now));
    }
}
