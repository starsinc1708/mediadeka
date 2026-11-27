use std::time::Duration;

use crate::ui::design_system::motion::Spring;

const HOVER_SLOTS: usize = super::ACCENTS.len();
const ARROW_TRAVEL: f32 = 2.5;

#[derive(Default)]
pub(in crate::ui) struct Motion {
    last: Option<Duration>,
    paused: bool,
    reduced: bool,
    hovers: [Spring; HOVER_SLOTS],
}

#[derive(Clone, Copy)]
pub(in crate::ui) struct MotionFrame {
    reduced: bool,
    hovers: [f32; HOVER_SLOTS],
}

impl MotionFrame {
    pub(in crate::ui) fn hover(self, index: usize) -> f32 {
        self.hovers.get(index).copied().unwrap_or_default()
    }

    pub(in crate::ui) fn shift(self, index: usize) -> f32 {
        if self.reduced {
            0.0
        } else {
            self.hover(index) * ARROW_TRAVEL
        }
    }
}

impl Motion {
    pub(in crate::ui) fn is_active(&self) -> bool {
        self.hovers.iter().any(|spring| !spring.settled())
    }

    pub(in crate::ui) fn frame(
        &mut self,
        now: Duration,
        hover: Option<usize>,
        reduced: bool,
        paused: bool,
    ) -> MotionFrame {
        let delta = self
            .last
            .replace(now)
            .map_or(Duration::ZERO, |last| now.saturating_sub(last));
        let advancing = !self.paused && !paused && !self.reduced && !reduced;
        self.paused = paused;
        self.reduced = reduced;

        if advancing {
            for spring in &mut self.hovers {
                spring.advance(delta.as_secs_f32());
            }
        }

        for (index, spring) in self.hovers.iter_mut().enumerate() {
            spring.set_target(f32::from(u8::from(hover == Some(index))));
            if reduced {
                spring.snap();
            }
        }

        MotionFrame {
            reduced,
            hovers: self.hovers.map(|spring| spring.value().clamp(0.0, 1.0)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interruption_preserves_progress_and_retargets_the_light() {
        let mut motion = Motion::default();
        motion.frame(Duration::ZERO, Some(0), false, false);

        let switching = motion.frame(Duration::from_millis(65), Some(4), false, false);
        assert!(switching.hover(0) > 0.0 && switching.hover(0) < 1.0);
        assert_eq!(switching.hover(4), 0.0);
        let settled = motion.frame(Duration::from_millis(350), Some(4), false, false);
        assert!(settled.hover(0) < 0.01);
        assert!(settled.hover(4) > 0.99);
    }

    #[test]
    fn pause_freezes_hover_and_reduced_motion_applies_highlight_without_shift() {
        let mut motion = Motion::default();
        motion.frame(Duration::ZERO, Some(2), false, false);
        let active = motion.frame(Duration::from_millis(45), Some(2), false, false);
        let paused = motion.frame(Duration::from_secs(1), Some(2), false, true);
        let resumed = motion.frame(Duration::from_secs(2), Some(2), false, false);
        let reduced = motion.frame(Duration::from_secs(3), Some(2), true, false);

        assert_eq!(active.shift(2), paused.shift(2));
        assert_eq!(paused.shift(2), resumed.shift(2));
        assert_eq!(reduced.shift(2), 0.0);
        assert_eq!(reduced.hover(2), 1.0);
    }

    #[test]
    fn hover_ends_frame_requests_when_it_settles() {
        let mut motion = Motion::default();
        assert!(!motion.is_active());
        motion.frame(Duration::ZERO, Some(2), false, false);
        assert!(motion.is_active());
        motion.frame(Duration::from_secs(1), Some(2), false, false);
        assert!(!motion.is_active());
    }
}
