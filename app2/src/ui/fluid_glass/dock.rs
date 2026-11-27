use std::time::Duration;

use crate::ui::design_system::motion;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub(in crate::ui) struct Surface {
    pub(in crate::ui) width: f32,
    pub(in crate::ui) strength: f32,
}

#[derive(Default)]
pub(in crate::ui) struct Dock {
    open: bool,
    painted: f32,
    motion: Option<Motion>,
}

struct Motion {
    from: f32,
    start: Duration,
}

impl Dock {
    pub(in crate::ui) fn surface(&self, expanded_width: f32) -> Surface {
        Surface {
            width: expanded_width * self.painted,
            strength: self.painted,
        }
    }

    pub(in crate::ui) fn frame(&mut self, open: bool, now: Duration, animate: bool) -> bool {
        let target = if open { 1. } else { 0. };
        if self.open != open {
            self.open = open;
            self.motion = (self.painted != target).then_some(Motion {
                from: self.painted,
                start: now,
            });
        }
        if animate && let Some(motion) = &self.motion {
            let elapsed = now.saturating_sub(motion.start);
            let duration = motion::MOTION.slow();
            if elapsed < duration {
                let eased = motion::out_cubic(elapsed.as_secs_f32() / duration.as_secs_f32());
                self.painted = motion.from + (target - motion.from) * eased;
                return true;
            }
        }
        self.motion = None;
        self.painted = target;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dock_width_and_material_open_together_and_reverse_from_the_last_frame() {
        let ms = Duration::from_millis;
        let mut dock = Dock::default();
        assert!(dock.frame(true, ms(0), true));
        assert_eq!(dock.surface(320.), Surface::default());
        assert!(dock.frame(true, ms(100), true));
        let middle = dock.surface(320.);
        assert!(middle.strength > 0. && middle.strength < 1.);
        assert_eq!(middle.width, middle.strength * 320.);
        assert!(dock.frame(false, ms(100), true));
        assert_eq!(dock.surface(320.), middle);
        assert!(dock.frame(false, ms(150), true));
        assert!(dock.surface(320.).width < middle.width);
        assert!(!dock.frame(false, ms(100) + motion::MOTION.slow(), true));
        assert_eq!(dock.surface(320.), Surface::default());
    }

    #[test]
    fn reduced_motion_commits_the_geometry_and_drops_the_clock() {
        let mut dock = Dock::default();
        assert!(!dock.frame(true, Duration::ZERO, false));
        assert_eq!(
            dock.surface(320.),
            Surface {
                width: 320.,
                strength: 1.
            }
        );
        assert!(!dock.frame(false, Duration::ZERO, false));
        assert_eq!(dock.surface(320.), Surface::default());
    }

    #[test]
    fn resizing_the_window_uses_its_new_dock_width_without_restarting() {
        let ms = Duration::from_millis;
        let mut dock = Dock::default();
        dock.frame(true, ms(0), true);
        dock.frame(true, ms(80), true);
        let before = dock.surface(240.);
        let after = dock.surface(360.);
        assert_eq!(before.strength, after.strength);
        assert!(after.width > before.width);
        assert!(!dock.frame(true, motion::MOTION.slow(), true));
        assert_eq!(dock.surface(360.).width, 360.);
    }
}
