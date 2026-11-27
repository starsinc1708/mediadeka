use std::time::{Duration, Instant};

use super::audio::{AudioFeatures, Availability, ENVELOPE_POINTS, Monitor, Signal, approach};

const ATTACK: f32 = 0.070;
const WAVEFORM_RELEASE: f32 = 0.095;
const SILENCE: f32 = 0.001;
const MAX_FRAME_STEP: Duration = Duration::from_millis(90);
const RECENT_LEVEL_POINTS: usize = 4;

#[derive(Clone, Copy, Default)]
pub(super) struct Waveform {
    pub(super) envelope: [f32; ENVELOPE_POINTS],
    pub(super) opacity: f32,
}

#[derive(Default)]
pub(super) struct Reactivity {
    capture: Capture,
    features: AudioFeatures,
    waveform: Waveform,
    latest: Signal,
    last_frame: Option<Instant>,
}

#[derive(Default)]
enum Capture {
    #[default]
    Stopped,
    Reading(Monitor),
    Unavailable,
}

impl Reactivity {
    pub(super) fn suspend(&mut self) {
        self.capture = Capture::Stopped;
        self.latest = Signal::default();
        self.features = AudioFeatures::default();
        self.waveform = Waveform::default();
        self.last_frame = None;
    }

    pub(super) fn update(
        &mut self,
        enabled: bool,
        presented: bool,
        playing: bool,
    ) -> AudioFeatures {
        if !enabled || !presented {
            self.suspend();
            return self.features;
        }
        if !playing {
            self.capture = Capture::Stopped;
            self.latest = Signal::default();
            if self.features == AudioFeatures::default() && self.waveform.opacity == 0. {
                self.last_frame = None;
                return self.features;
            }
        } else if matches!(self.capture, Capture::Stopped) {
            self.capture = match Monitor::start() {
                Ok(monitor) => Capture::Reading(monitor),
                Err(error) => {
                    eprintln!("Не удалось запустить анализ звука Fluid Glass: {error}");
                    Capture::Unavailable
                }
            };
        }
        self.latest = match &self.capture {
            Capture::Reading(monitor) => monitor.signal().unwrap_or(self.latest),
            _ => Signal::default(),
        };
        let now = Instant::now();
        let elapsed = self
            .last_frame
            .replace(now)
            .map_or(Duration::ZERO, |last| now.duration_since(last))
            .min(MAX_FRAME_STEP)
            .as_secs_f32();
        self.features = if self.latest.features == AudioFeatures::default() {
            self.features.follow(AudioFeatures::default(), elapsed)
        } else {
            self.latest.features
        };
        self.update_waveform(elapsed);
        self.features
    }

    fn update_waveform(&mut self, elapsed: f32) {
        let envelope = self.latest.envelope;
        let recent = envelope[ENVELOPE_POINTS - RECENT_LEVEL_POINTS..]
            .iter()
            .copied()
            .fold(0., f32::max);
        self.waveform.opacity = approach(
            self.waveform.opacity,
            recent,
            elapsed,
            ATTACK,
            WAVEFORM_RELEASE,
        );
        if self.waveform.opacity == 0. {
            self.waveform.envelope.fill(0.);
        } else if envelope.iter().any(|&level| level > SILENCE) {
            self.waveform.envelope = envelope;
        }
    }

    pub(super) fn waveform(&self) -> Option<Waveform> {
        (self.waveform.opacity > 0.).then_some(self.waveform)
    }

    #[cfg(feature = "capture")]
    pub(super) fn bands(&self) -> [f32; 3] {
        [
            self.features.low(),
            self.features.middle(),
            self.features.high(),
        ]
    }

    pub(super) fn availability(&self) -> Availability {
        match &self.capture {
            Capture::Stopped => Availability::Inactive,
            Capture::Unavailable => Availability::Unavailable,
            Capture::Reading(monitor) => monitor.availability(),
        }
    }

    #[cfg(feature = "capture")]
    pub(super) fn capture_status(&self) -> &'static str {
        match &self.capture {
            Capture::Stopped => "stopped",
            Capture::Unavailable => "unavailable",
            Capture::Reading(monitor) => monitor.diagnosis(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_music_with_a_faster_attack_and_finite_decay_to_silence() {
        let peak = AudioFeatures {
            peak: 1.,
            ..AudioFeatures::default()
        };
        let rise = AudioFeatures::default().follow(peak, 0.05).peak;
        let decay = peak.follow(AudioFeatures::default(), 0.05).peak;
        assert!((rise - 0.632).abs() < 0.001);
        assert!(decay > 0.8);
        assert_eq!(peak.follow(AudioFeatures::default(), 3.).peak, 0.);
    }
}
