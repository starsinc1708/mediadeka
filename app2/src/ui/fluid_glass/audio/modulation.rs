use crate::visual::{AudioChannel, AudioResponse, VisualFlags};

use super::AudioFeatures;

/// Bounded inputs to the existing material passes. Base appearance stays in
/// VisualSettings, and the audio path never edits the user's effect flags.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(in crate::ui::fluid_glass) struct Modulation {
    pub(in crate::ui::fluid_glass) bass: f32,
    pub(in crate::ui::fluid_glass) mid: f32,
    pub(in crate::ui::fluid_glass) treble: f32,
    pub(in crate::ui::fluid_glass) light: f32,
    pub(in crate::ui::fluid_glass) reflection: f32,
    pub(in crate::ui::fluid_glass) dispersion: f32,
    pub(in crate::ui::fluid_glass) pulse: f32,
    pub(in crate::ui::fluid_glass) glitch: f32,
}

impl Modulation {
    pub(in crate::ui::fluid_glass) fn new(
        response: AudioResponse,
        effects: VisualFlags,
        audio: AudioFeatures,
    ) -> Self {
        let gain = response.profile.gain() * f32::from(response.sensitivity()) / 100.;
        let channel = |kind, level: f32| {
            if response.allows(kind) {
                (level * gain).clamp(0., 1.)
            } else {
                0.
            }
        };
        let low = audio.low();
        let middle = audio.middle();
        let high = audio.high();
        let dispersion =
            effects.enabled(VisualFlags::REFRACTION) && effects.enabled(VisualFlags::DISPERSION);
        Self {
            bass: channel(AudioChannel::Patterns, low),
            mid: channel(AudioChannel::Patterns, middle),
            treble: channel(AudioChannel::Patterns, high),
            light: channel(AudioChannel::Lighting, 0.78 * audio.rms + 0.22 * audio.peak),
            reflection: channel(AudioChannel::Reflections, 0.40 * middle + 0.60 * high),
            dispersion: channel(AudioChannel::Dispersion, if dispersion { high } else { 0. }),
            pulse: channel(
                AudioChannel::Pulses,
                audio.beat_strength * (0.7 + 0.3 * low),
            ),
            glitch: channel(
                AudioChannel::Glitch,
                if effects.enabled(VisualFlags::GLITCH) {
                    let onset = 0.7 * audio.transient_strength + 0.3 * audio.spectral_flux;
                    ((onset - 0.42) / 0.58).max(0.) * (0.80 + 0.20 * audio.spectral_centroid)
                } else {
                    0.
                },
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visual::AudioProfile;

    #[test]
    fn respects_profile_and_disabled_channels_and_effects() {
        let features = AudioFeatures {
            rms: 0.7,
            peak: 0.9,
            bass: 0.8,
            mid: 0.7,
            treble: 0.6,
            spectral_flux: 0.8,
            spectral_centroid: 0.5,
            transient_strength: 0.95,
            beat_strength: 0.7,
            ..AudioFeatures::default()
        };
        let mut response = AudioResponse::standard();
        let balanced = Modulation::new(response, VisualFlags::ALL, features);
        response.profile = AudioProfile::Immersive;
        let immersive = Modulation::new(response, VisualFlags::ALL, features);
        assert!(immersive.pulse > balanced.pulse && immersive.glitch > balanced.glitch);
        response.toggle_channel(AudioChannel::Patterns);
        let muted = Modulation::new(response, VisualFlags::NONE, features);
        assert_eq!(
            [
                muted.bass,
                muted.mid,
                muted.treble,
                muted.dispersion,
                muted.glitch
            ],
            [0.; 5]
        );
        assert_eq!(muted.light, immersive.light);
        response.enabled = false;
        assert_eq!(
            Modulation::new(response, VisualFlags::ALL, features),
            Modulation::default()
        );
    }
}
