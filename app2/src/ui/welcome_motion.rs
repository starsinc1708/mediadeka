use std::time::Duration;

use crate::providers::ProviderId;

const LAYOUT_DURATION: Duration = Duration::from_millis(290);
const OPEN_DURATION: Duration = Duration::from_millis(760);
const LOCAL_DURATION: Duration = Duration::from_millis(560);
const CLOSE_DURATION: Duration = Duration::from_millis(190);

#[derive(Default)]
pub(super) struct ConnectionMotion {
    origin: f32,
    opened: bool,
    layout_started: Duration,
    flight: Option<Activation>,
}

struct Activation {
    provider: ProviderId,
    started: Duration,
    duration: Duration,
    switching: bool,
    surface_origin: f32,
}

#[derive(Clone, Copy)]
pub(super) struct Flight {
    pub provider: ProviderId,
    pub progress: f32,
    pub assembly: f32,
    pub surface: f32,
    pub content: f32,
}

impl Flight {
    pub(super) fn controls_visible(self) -> bool {
        self.content >= 0.10
    }
}

pub(super) struct ConnectionFrame {
    pub layout: f32,
    pub flight: Option<Flight>,
}

impl ConnectionMotion {
    pub(super) fn is_active(&self, frame: &ConnectionFrame) -> bool {
        frame.flight.is_some() || frame.layout != f32::from(self.opened)
    }

    pub(super) fn activate(&mut self, now: Duration, provider: ProviderId, compact: bool) {
        let switching = self.opened;
        let current = self.at(now, false).flight;
        let surface_origin = current.map_or(f32::from(switching), |flight| flight.surface);
        self.set_open(now, true);
        self.flight = Some(Activation {
            provider,
            started: now,
            duration: if compact || switching {
                LOCAL_DURATION
            } else {
                OPEN_DURATION
            },
            switching,
            surface_origin,
        });
    }

    pub(super) fn set_open(&mut self, now: Duration, opened: bool) {
        if self.opened != opened {
            self.origin = self.layout(now);
            self.layout_started = now;
            self.opened = opened;
        }
        if !opened {
            self.flight = None;
        }
    }

    pub(super) fn settle(&mut self) {
        self.origin = f32::from(self.opened);
        self.flight = None;
    }

    pub(super) fn clear_flight(&mut self) {
        self.flight = None;
    }

    pub(super) fn at(&self, now: Duration, reduced: bool) -> ConnectionFrame {
        if reduced {
            return ConnectionFrame {
                layout: f32::from(self.opened),
                flight: None,
            };
        }
        let flight = self.flight.as_ref().and_then(|activation| {
            let progress = fraction(now.saturating_sub(activation.started), activation.duration);
            (progress < 1.).then(|| {
                let assembly = ease_between(progress, 0.25, 0.82);
                let surface =
                    activation.surface_origin + (1. - activation.surface_origin) * assembly;
                let content = if activation.switching && activation.surface_origin == 1. {
                    ease_between(progress, 0.38, 0.87)
                } else {
                    ease_between(progress, 0.73, 0.98)
                };
                Flight {
                    provider: activation.provider,
                    progress,
                    assembly,
                    surface,
                    content,
                }
            })
        });
        ConnectionFrame {
            layout: self.layout(now),
            flight,
        }
    }

    fn layout(&self, now: Duration) -> f32 {
        let time = now.saturating_sub(self.layout_started);
        let duration = if self.opened {
            LAYOUT_DURATION
        } else {
            CLOSE_DURATION
        };
        let progress = ease_out(fraction(time, duration));
        self.origin + (f32::from(self.opened) - self.origin) * progress
    }
}

fn ease_out(progress: f32) -> f32 {
    1. - (1. - progress.clamp(0., 1.)).powi(3)
}

fn ease_between(progress: f32, start: f32, end: f32) -> f32 {
    let t = ((progress - start) / (end - start)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}

fn fraction(elapsed: Duration, duration: Duration) -> f32 {
    (elapsed.as_secs_f32() / duration.as_secs_f32()).min(1.)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_retains_layout_and_retargets_flight() {
        let mut motion = ConnectionMotion::default();
        motion.activate(Duration::ZERO, ProviderId::Spotify, false);
        let interrupt = Duration::from_millis(250);
        let before = motion.at(interrupt, false);
        motion.activate(interrupt, ProviderId::Jellyfin, false);
        let changed = motion.at(interrupt, false);
        assert_eq!(before.layout, changed.layout);
        assert_eq!(changed.flight.unwrap().provider, ProviderId::Jellyfin);
        assert_eq!(
            changed.flight.unwrap().surface,
            before.flight.unwrap().surface
        );
        assert!(motion.at(interrupt, true).flight.is_none());
        assert_eq!(motion.at(interrupt, true).layout, 1.);
        motion.set_open(interrupt, false);
        assert_eq!(motion.at(interrupt, false).layout, before.layout);
        assert_eq!(motion.at(interrupt + CLOSE_DURATION, false).layout, 0.);
    }

    #[test]
    fn surface_assembles_before_content_and_compact_finishes_earlier() {
        let mut motion = ConnectionMotion::default();
        motion.activate(Duration::ZERO, ProviderId::Spotify, false);
        let incoming = motion.at(Duration::from_millis(140), false).flight.unwrap();
        assert_eq!((incoming.surface, incoming.content), (0., 0.));
        let assembling = motion.at(Duration::from_millis(370), false).flight.unwrap();
        assert!(assembling.surface > 0.3 && assembling.surface < 0.6);
        assert_eq!(assembling.content, 0.);
        let settling = motion.at(Duration::from_millis(680), false).flight.unwrap();
        assert!(settling.surface > 0.95 && settling.content > 0.5 && settling.content < 1.);
        assert!(motion.at(OPEN_DURATION, false).flight.is_none());
        motion.set_open(OPEN_DURATION, false);
        motion.activate(OPEN_DURATION, ProviderId::Spotify, true);
        assert!(
            motion
                .at(OPEN_DURATION + LOCAL_DURATION, false)
                .flight
                .is_none()
        );
    }
}
