use std::time::{Duration, Instant};

pub(super) struct Clock {
    origin: Instant,
    phase: Duration,
    last_frame: Option<Duration>,
}

impl Clock {
    pub(super) fn new() -> Self {
        Self {
            origin: Instant::now(),
            phase: Duration::ZERO,
            last_frame: None,
        }
    }

    pub(super) fn phase(&mut self, wall: Option<Duration>, active: bool) -> Duration {
        if active {
            let wall = wall.unwrap_or_else(|| self.origin.elapsed());
            if let Some(previous) = self.last_frame.replace(wall) {
                self.phase += wall.saturating_sub(previous);
            }
        } else {
            self.pause();
        }
        self.phase
    }

    pub(super) fn pause(&mut self) -> bool {
        self.last_frame.take().is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_resumed_frame_keeps_the_last_presented_phase() {
        let mut clock = Clock::new();
        let secs = Duration::from_secs;
        assert_eq!(clock.phase(Some(secs(10)), true), secs(0));
        assert_eq!(clock.phase(Some(secs(12)), true), secs(2));
        assert_eq!(clock.phase(Some(secs(17)), false), secs(2));
        assert_eq!(clock.phase(Some(secs(77)), false), secs(2));
        assert_eq!(clock.phase(Some(secs(80)), true), secs(2));
        assert_eq!(clock.phase(Some(secs(81)), true), secs(3));
        assert!(clock.pause());
        assert!(!clock.pause());
        assert_eq!(clock.phase(Some(secs(800)), true), secs(3));
    }
}
