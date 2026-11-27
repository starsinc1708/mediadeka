use std::time::Duration;

use super::tokens::PAGE_DURATION;

pub(super) struct Exposure {
    target: f32,
    painted: f32,
    transition: Option<(f32, Option<Duration>)>,
}

impl Exposure {
    pub(super) fn new(exposure: f32) -> Self {
        Self {
            target: exposure,
            painted: exposure,
            transition: None,
        }
    }

    pub(super) fn target(&self) -> f32 {
        self.target
    }

    pub(super) fn select(&mut self, exposure: f32) {
        self.target = exposure;
        self.transition = (self.painted != exposure).then_some((self.painted, None));
    }

    pub(super) fn sample(&mut self, now: Duration, animated: bool) -> f32 {
        if animated && let Some((from, start)) = &mut self.transition {
            let elapsed = now.saturating_sub(*start.get_or_insert(now));
            let phase = elapsed.as_secs_f32() / PAGE_DURATION.as_secs_f32();
            if phase < 1. {
                let eased = phase * phase * (3. - 2. * phase);
                self.painted = *from + (self.target - *from) * eased;
                return self.painted;
            }
        }
        self.transition = None;
        self.painted = self.target;
        self.painted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_reversal_starts_at_the_painted_exposure() {
        let mut exposure = Exposure::new(0.30);
        exposure.select(0.20);
        assert_eq!(exposure.sample(Duration::ZERO, true), 0.30);
        let middle = exposure.sample(PAGE_DURATION / 2, true);
        assert!((0.20..0.30).contains(&middle));
        exposure.select(0.30);
        assert_eq!(exposure.sample(PAGE_DURATION / 2, true), middle);
        assert_eq!(exposure.sample(PAGE_DURATION, false), 0.30);
    }
}
