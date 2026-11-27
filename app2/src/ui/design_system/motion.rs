//! Motion tokens and the interpolation functions every transition shares.

use std::time::Duration;

mod fragments;
pub(in crate::ui) use fragments::ExitingSpans;

/// Durations from the Fluid Glass 3.0 motion map. Every component reads these
/// instead of carrying its own literal.
pub(in crate::ui) struct MotionTokens {
    /// Press feedback: the surface sinking under the pointer.
    pub instant_ms: u64,
    /// Hover light and small reveals.
    pub fast_ms: u64,
    pub interaction_ms: u64,
    pub page_ms: u64,
    /// Navigation, tabs and page-level crossfades.
    pub normal_ms: u64,
    /// Shell geometry: the right panel growing out of the player.
    pub slow_ms: u64,
    /// Artwork travelling between a collection and its detail page.
    pub shared_artwork_ms: u64,
    /// Artwork travelling between the player and immersive listening.
    pub immersive_ms: u64,
    /// Palette flow between tracks.
    pub palette_ms: u64,
    /// Change of the automatic pattern within the active page's family.
    pub track_pattern_ms: u64,
}

pub(in crate::ui) const MOTION: MotionTokens = MotionTokens {
    instant_ms: 80,
    fast_ms: 140,
    interaction_ms: 160,
    page_ms: 200,
    normal_ms: 220,
    slow_ms: 350,
    shared_artwork_ms: 420,
    immersive_ms: 520,
    palette_ms: 1600,
    track_pattern_ms: 1350,
};

impl MotionTokens {
    pub(in crate::ui) const fn instant(self) -> Duration {
        Duration::from_millis(self.instant_ms)
    }

    pub(in crate::ui) const fn fast(self) -> Duration {
        Duration::from_millis(self.fast_ms)
    }

    pub(in crate::ui) const fn interaction(self) -> Duration {
        Duration::from_millis(self.interaction_ms)
    }

    pub(in crate::ui) const fn page(self) -> Duration {
        Duration::from_millis(self.page_ms)
    }

    pub(in crate::ui) const fn normal(self) -> Duration {
        Duration::from_millis(self.normal_ms)
    }

    pub(in crate::ui) const fn slow(self) -> Duration {
        Duration::from_millis(self.slow_ms)
    }

    pub(in crate::ui) const fn shared_artwork(self) -> Duration {
        Duration::from_millis(self.shared_artwork_ms)
    }

    pub(in crate::ui) const fn immersive(self) -> Duration {
        Duration::from_millis(self.immersive_ms)
    }

    pub(in crate::ui) const fn palette(self) -> Duration {
        Duration::from_millis(self.palette_ms)
    }

    pub(in crate::ui) const fn track_pattern(self) -> Duration {
        Duration::from_millis(self.track_pattern_ms)
    }
}

/// Smooth acceleration and release. Spatial moves and palette blends use it.
pub(in crate::ui) fn smoothstep(progress: f32) -> f32 {
    let progress = progress.clamp(0., 1.);
    progress * progress * (3. - 2. * progress)
}

const SPRING_FREQUENCY: f32 = 28.0;
const SPRING_REST_DISTANCE: f32 = 0.001;
const SPRING_REST_SPEED: f32 = 0.04;

/// Critically damped spring. Keeping velocity lets a retargeted spring continue
/// from its current position instead of restarting, which is what makes every
/// interaction interruptible.
#[derive(Clone, Copy, Default)]
pub(in crate::ui) struct Spring {
    value: f32,
    velocity: f32,
    target: f32,
}

impl Spring {
    pub(in crate::ui) fn at(value: f32) -> Self {
        Self {
            value,
            velocity: 0.,
            target: value,
        }
    }

    pub(in crate::ui) fn value(self) -> f32 {
        self.value
    }

    pub(in crate::ui) fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Reduced motion: commit the target without animating.
    pub(in crate::ui) fn snap(&mut self) {
        self.value = self.target;
        self.velocity = 0.;
    }

    pub(in crate::ui) fn settled(&self) -> bool {
        self.value == self.target && self.velocity == 0.
    }

    pub(in crate::ui) fn advance(&mut self, seconds: f32) {
        // Exact critically damped solution for this interval.
        let offset = self.value - self.target;
        let slope = self.velocity + SPRING_FREQUENCY * offset;
        let decay = (-SPRING_FREQUENCY * seconds).exp();
        self.value = self.target + (offset + slope * seconds) * decay;
        self.velocity = (self.velocity - slope * SPRING_FREQUENCY * seconds) * decay;

        if (self.value - self.target).abs() < SPRING_REST_DISTANCE
            && self.velocity.abs() < SPRING_REST_SPEED
        {
            self.value = self.target;
            self.velocity = 0.0;
        }
    }
}

/// Reaches the target quickly, then settles. Reveals and geometry.
pub(in crate::ui) fn out_cubic(progress: f32) -> f32 {
    1. - (1. - progress.clamp(0., 1.)).powi(3)
}

/// Softer start than [`out_cubic`], for short slider and fade distances.
pub(in crate::ui) fn out_quad(progress: f32) -> f32 {
    let progress = progress.clamp(0., 1.);
    progress * (2. - progress)
}

/// One retargetable position evaluated against the presentation clock.
#[derive(Clone, Copy)]
pub(in crate::ui) struct Slide {
    from: f32,
    target: f32,
    started: Duration,
    duration: Duration,
}

impl Slide {
    pub(in crate::ui) fn new(
        from: f32,
        target: f32,
        started: Duration,
        duration: Duration,
    ) -> Self {
        Self {
            from,
            target,
            started,
            duration,
        }
    }

    pub(in crate::ui) fn position(self, now: Duration) -> f32 {
        if !self.active(now) {
            return self.target;
        }
        let progress = now.saturating_sub(self.started).as_secs_f32() / self.duration.as_secs_f32();
        self.from + (self.target - self.from) * out_quad(progress)
    }

    pub(in crate::ui) fn active(self, now: Duration) -> bool {
        self.from != self.target && now.saturating_sub(self.started) < self.duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_curve_starts_at_zero_and_lands_exactly_on_one() {
        for curve in [smoothstep, out_cubic, out_quad] {
            assert_eq!(curve(0.), 0.);
            assert_eq!(curve(1.), 1.);
            assert_eq!(curve(2.), 1.);
            assert_eq!(curve(-1.), 0.);
        }
    }

    #[test]
    fn curves_are_monotonic_and_never_overshoot() {
        for curve in [smoothstep, out_cubic, out_quad] {
            let mut previous = 0.;
            for step in 1..=100 {
                let value = curve(step as f32 / 100.);
                assert!(value >= previous && value <= 1.);
                previous = value;
            }
        }
    }

    #[test]
    fn tokens_follow_the_motion_map_order() {
        assert!(MOTION.instant() < MOTION.fast());
        assert!(MOTION.fast() < MOTION.normal());
        assert!(MOTION.normal() < MOTION.slow());
        assert!(MOTION.slow() < MOTION.palette());
    }
}
