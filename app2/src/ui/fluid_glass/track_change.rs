use std::time::Duration;

use gpui::{Pixels, Point, Size};

pub(in crate::ui) const COVER_DELAY: Duration = Duration::from_millis(250);
pub(in crate::ui) const COVER_FADE: Duration = Duration::from_millis(240);
pub(super) const PALETTE_DELAY: Duration = Duration::from_millis(400);

const PULSE_START: Duration = Duration::from_millis(100);
const PULSE_DURATION: Duration = Duration::from_millis(260);
const ROW_FLIGHT: Duration = Duration::from_millis(420);

#[derive(Default)]
pub(super) struct TrackChange {
    started: Option<Duration>,
}

impl TrackChange {
    pub(super) fn age(&mut self, phase: Duration) -> Duration {
        phase.saturating_sub(*self.started.get_or_insert(phase))
    }

    pub(super) fn pulse(age: Duration) -> f32 {
        let elapsed = age.saturating_sub(PULSE_START);
        if elapsed >= PULSE_DURATION {
            return 0.;
        }
        elapsed.as_secs_f32() / PULSE_DURATION.as_secs_f32()
    }
}

type TrackPosition = (u64, usize);

#[derive(Clone, Copy)]
pub(super) struct RowOrigin {
    pub(super) point: Point<Pixels>,
    pub(super) viewport: Size<Pixels>,
}

#[derive(Default)]
pub(super) struct RowLaunch {
    pending: Option<(TrackPosition, RowOrigin)>,
    flight: Option<(TrackPosition, RowOrigin, Option<Duration>)>,
}

#[derive(Clone, Copy)]
pub(super) struct RowFlight {
    pub(super) origin: Point<Pixels>,
    pub(super) phase: f32,
}

impl RowLaunch {
    pub(super) fn cancel(&mut self) -> bool {
        self.pending = None;
        self.flight.take().is_some()
    }

    pub(super) fn arm(&mut self, track: TrackPosition, origin: RowOrigin) {
        self.pending = Some((track, origin));
        self.flight = None;
    }

    pub(super) fn track_changed(&mut self, position: Option<TrackPosition>) {
        if self
            .pending
            .is_some_and(|(track, _)| Some(track) != position)
        {
            self.pending = None;
        }
        if self
            .flight
            .is_some_and(|(track, _, _)| Some(track) != position)
        {
            self.flight = None;
        }
    }

    pub(super) fn sound_started(&mut self, position: Option<TrackPosition>) -> bool {
        if let Some((track, origin)) = self.pending
            && Some(track) == position
        {
            self.pending = None;
            self.flight = Some((track, origin, None));
            return true;
        }
        false
    }

    pub(super) fn sample(&mut self, now: Duration, viewport: Size<Pixels>) -> Option<RowFlight> {
        let (_, origin, started) = self.flight.as_mut()?;
        let elapsed = now.saturating_sub(*started.get_or_insert(now));
        if origin.viewport != viewport || elapsed >= ROW_FLIGHT {
            self.flight = None;
            return None;
        }
        Some(RowFlight {
            origin: origin.point,
            phase: elapsed.as_secs_f32() / ROW_FLIGHT.as_secs_f32(),
        })
    }

    #[cfg(feature = "capture")]
    pub(super) fn origin(&self) -> Option<Point<Pixels>> {
        self.flight.map(|(_, origin, _)| origin.point)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px};

    #[test]
    fn pulse_obeys_the_player_and_palette_timing() {
        let millis = Duration::from_millis;
        let mut change = TrackChange::default();
        assert_eq!(change.age(millis(30)), Duration::ZERO);
        assert_eq!(TrackChange::pulse(change.age(millis(110))), 0.);
        assert!(TrackChange::pulse(change.age(millis(190))) > 0.2);
        assert!(TrackChange::pulse(COVER_DELAY) > 0.);
        assert_eq!(TrackChange::pulse(PALETTE_DELAY), 0.);
    }

    #[test]
    fn row_light_waits_for_matching_sound_and_discards_a_stale_origin() {
        let viewport = gpui::size(px(800.), px(400.));
        let origin = RowOrigin {
            point: point(px(160.), px(120.)),
            viewport,
        };
        let mut launch = RowLaunch::default();
        launch.arm((1, 0), origin);
        assert!(launch.sample(Duration::ZERO, viewport).is_none());
        assert!(!launch.sound_started(Some((2, 0))));
        launch.track_changed(Some((2, 0)));
        assert!(!launch.sound_started(Some((1, 0))));
        launch.arm((2, 0), origin);
        assert!(launch.sound_started(Some((2, 0))));
        assert_eq!(launch.sample(Duration::ZERO, viewport).unwrap().phase, 0.);
        let middle = launch.sample(ROW_FLIGHT / 2, viewport).unwrap();
        assert_eq!(middle.origin, origin.point);
        assert_eq!(middle.phase, 0.5);
        assert!(!launch.sound_started(Some((2, 0))));
        assert!(launch.sample(ROW_FLIGHT, viewport).is_none());
    }
}
