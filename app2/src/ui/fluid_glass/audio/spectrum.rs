use std::f32::consts::TAU;

use super::AudioFeatures;

pub(super) const SAMPLE_RATE: usize = 48_000;
pub(super) const FRAME_BYTES: usize = 2 * size_of::<f32>();
const FFT_SIZE: usize = 2048;
const HOP_SIZE: usize = FFT_SIZE / 2;
const HOP_SECONDS: f32 = HOP_SIZE as f32 / SAMPLE_RATE as f32;
const BAND_ENDS_HZ: [usize; 6] = [60, 250, 500, 2_000, 4_000, 20_001];
const AUDIBLE_END_HZ: usize = 20_000;
const NOISE_FLOOR_LOG10: f32 = -3.7;
const QUIET_FLOOR: f32 = 0.0002;
const LOUDNESS_REFERENCE: f32 = 0.12;
const LOUDNESS_DECAY: f32 = 8.0;
const MIN_BEAT_INTERVAL: f32 = 0.18;

#[derive(Debug, Default, PartialEq)]
pub(super) struct Measurement {
    pub(super) features: AudioFeatures,
    pub(super) envelope_rms: f32,
}

pub(super) struct Spectrum {
    ring: [(f32, f32); FFT_SIZE],
    values: [(f32, f32); FFT_SIZE],
    hann: [f32; FFT_SIZE],
    twiddles: [(f32, f32); FFT_SIZE / 2],
    spectral_scale: f32,
    next: usize,
    until_window: usize,
    previous_bins: [f32; FFT_SIZE / 2],
    loudness: f32,
    flux_average: f32,
    previous_low: f32,
    beat_cooldown: f32,
}

impl Spectrum {
    pub(super) const HOP: f32 = HOP_SECONDS;

    pub(super) fn new() -> Self {
        let hann: [f32; FFT_SIZE] =
            std::array::from_fn(|i| (1. - (TAU * i as f32 / (FFT_SIZE - 1) as f32).cos()) * 0.5);
        let window_energy: f32 = hann.iter().map(|value| value * value).sum();
        Self {
            ring: [(0., 0.); FFT_SIZE],
            values: [(0., 0.); FFT_SIZE],
            hann,
            twiddles: std::array::from_fn(|i| {
                let (sin, cos) = (-TAU * i as f32 / FFT_SIZE as f32).sin_cos();
                (cos, sin)
            }),
            spectral_scale: 0.5 / (FFT_SIZE as f32 * window_energy),
            next: 0,
            until_window: FFT_SIZE,
            previous_bins: [0.; FFT_SIZE / 2],
            loudness: 0.,
            flux_average: 0.,
            previous_low: 0.,
            beat_cooldown: 0.,
        }
    }

    pub(super) fn push(&mut self, bytes: &[u8], mut completed: impl FnMut(Measurement)) {
        for frame in bytes.as_chunks::<FRAME_BYTES>().0 {
            let (left, right) = frame.split_at(4);
            self.ring[self.next] = (sample(left), sample(right));
            self.next = (self.next + 1) & (FFT_SIZE - 1);
            self.until_window -= 1;
            if self.until_window == 0 {
                completed(self.measure());
                self.until_window = HOP_SIZE;
            }
        }
    }

    pub(super) fn reset(&mut self) {
        self.next = 0;
        self.until_window = FFT_SIZE;
        self.previous_bins.fill(0.);
        self.loudness = 0.;
        self.flux_average = 0.;
        self.previous_low = 0.;
        self.beat_cooldown = 0.;
    }

    fn measure(&mut self) -> Measurement {
        let mut energy = 0.;
        let mut peak = 0.0_f32;
        for (i, weight) in self.hann.iter().enumerate() {
            let (left, right) = self.ring[(self.next + i) & (FFT_SIZE - 1)];
            energy += left * left + right * right;
            peak = peak.max(left.abs()).max(right.abs());
            let reversed = i.reverse_bits() >> (usize::BITS - FFT_SIZE.ilog2());
            // One complex FFT handles both real channels, including opposite phase.
            self.values[reversed] = (left * weight, right * weight);
        }
        let rms = (energy / (2 * FFT_SIZE) as f32).sqrt();
        self.loudness = rms.max(self.loudness * (-HOP_SECONDS / LOUDNESS_DECAY).exp());
        self.beat_cooldown = (self.beat_cooldown - HOP_SECONDS).max(0.);
        if rms < QUIET_FLOOR {
            self.previous_bins.fill(0.);
            self.flux_average *= 0.96;
            self.previous_low = 0.;
            return Measurement::default();
        }

        self.fft();
        let (energy, flux, centroid) = self.measure_bins();
        let gain = (LOUDNESS_REFERENCE / self.loudness).sqrt().clamp(0.65, 5.);
        let level = |amplitude| normalize(amplitude * gain);
        let [sub_bass, bass, low_mid, mid, high_mid, treble] =
            energy.map(|power| level(power.sqrt()));
        let low = (energy[0] + energy[1]).sqrt();
        let bass_attack = ((low - self.previous_low) / low.max(QUIET_FLOOR)).max(0.);
        self.previous_low = low;
        let transient = ((flux - self.flux_average * 1.7 - 0.12) * 2.0).clamp(0., 1.);
        self.flux_average += (flux - self.flux_average) * (1. - (-HOP_SECONDS / 0.8).exp());
        let beat = if bass_attack > 0.18 && transient > 0.12 && self.beat_cooldown == 0. {
            self.beat_cooldown = MIN_BEAT_INTERVAL;
            (0.65 * bass_attack + 0.35 * transient) * sub_bass.max(bass)
        } else {
            0.
        };
        Measurement {
            features: AudioFeatures {
                rms: level(rms),
                peak: level(peak),
                sub_bass,
                bass,
                low_mid,
                mid,
                high_mid,
                treble,
                spectral_flux: flux,
                spectral_centroid: centroid,
                transient_strength: transient,
                beat_strength: beat,
            },
            envelope_rms: normalize(rms),
        }
    }

    fn fft(&mut self) {
        for power in 1..=FFT_SIZE.ilog2() {
            let width = 1 << power;
            let half = width / 2;
            let twiddle_step = FFT_SIZE / width;
            for group in self.values.chunks_exact_mut(width) {
                let (left, right) = group.split_at_mut(half);
                for (i, (a, b)) in left.iter_mut().zip(right).enumerate() {
                    let (cos, sin) = self.twiddles[i * twiddle_step];
                    let rotated = (b.0 * cos - b.1 * sin, b.0 * sin + b.1 * cos);
                    *b = (a.0 - rotated.0, a.1 - rotated.1);
                    *a = (a.0 + rotated.0, a.1 + rotated.1);
                }
            }
        }
    }

    fn measure_bins(&mut self) -> ([f32; 6], f32, f32) {
        let mut energy = [0.; 6];
        let mut total = 0.;
        let mut weighted_hz = 0.;
        let mut flux = 0.;
        let mut previous_total = 0.;
        let mut band = 0;
        for bin in 1..FFT_SIZE / 2 {
            let hz = bin * SAMPLE_RATE / FFT_SIZE;
            if hz > AUDIBLE_END_HZ {
                break;
            }
            while hz >= BAND_ENDS_HZ[band] {
                band += 1;
            }
            let power = |(re, im): (f32, f32)| re * re + im * im;
            // These two coefficients give twice the sum of both channels' energies.
            let bin_power = (power(self.values[bin]) + power(self.values[FFT_SIZE - bin]))
                * self.spectral_scale;
            energy[band] += bin_power;
            let magnitude = bin_power.sqrt();
            let previous = std::mem::replace(&mut self.previous_bins[bin], magnitude);
            total += magnitude;
            weighted_hz += magnitude * hz as f32;
            flux += (magnitude - previous).max(0.);
            previous_total += previous;
        }
        (
            energy,
            flux / total.max(previous_total).max(QUIET_FLOOR),
            weighted_hz / (total.max(QUIET_FLOOR) * AUDIBLE_END_HZ as f32),
        )
    }
}

fn normalize(amplitude: f32) -> f32 {
    (1. - amplitude.log10() / NOISE_FLOOR_LOG10).clamp(0., 1.)
}

fn sample(bytes: &[u8]) -> f32 {
    let value = f32::from_le_bytes(bytes.try_into().expect("32-bit PCM sample"));
    if value.is_finite() {
        value.clamp(-1., 1.)
    } else {
        0.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(left_hz: f32, right_hz: f32, amplitude: f32) -> Vec<u8> {
        let sample =
            |hz: f32, i: usize| (TAU * hz * i as f32 / SAMPLE_RATE as f32).sin() * amplitude;
        (0..FFT_SIZE)
            .flat_map(|i| {
                [sample(left_hz, i), -sample(right_hz, i)]
                    .into_iter()
                    .flat_map(f32::to_le_bytes)
            })
            .collect()
    }

    fn measurements(spectrum: &mut Spectrum, bytes: &[u8]) -> Vec<Measurement> {
        let mut result = Vec::new();
        spectrum.push(bytes, |measurement| result.push(measurement));
        result
    }

    #[test]
    fn separates_quiet_low_mid_and_high_tones() {
        for (frequency, expected) in [(94., 0), (1_000., 1), (8_000., 2)] {
            let observed = measurements(&mut Spectrum::new(), &tone(frequency, frequency, 0.003));
            let signal = observed[0].features;
            let levels = [signal.low(), signal.middle(), signal.high()];
            assert!(levels[expected] > 0.2, "{frequency} Hz: {signal:?}");
            assert!(levels.iter().enumerate().all(|(index, &value)| {
                (0. ..=1.).contains(&value) && (index == expected || value < 0.03)
            }));
        }
    }

    #[test]
    fn keeps_both_stereo_channels_and_releases_to_silence() {
        let mut spectrum = Spectrum::new();
        let bytes = tone(94., 8_000., 0.3);
        let (first, second) = bytes.split_at(bytes.len() / 2);
        assert!(measurements(&mut spectrum, first).is_empty());
        let signal = measurements(&mut spectrum, second)[0].features;
        assert!(signal.low() > 0.6 && signal.high() > 0.6 && signal.middle() < 0.03);
        let silent = measurements(&mut spectrum, &[0; FFT_SIZE * FRAME_BYTES]);
        assert_ne!(silent[0], Measurement::default());
        assert_eq!(silent[1], Measurement::default());
    }

    #[test]
    fn measures_stereo_rms_for_every_window_of_the_same_packet() {
        let mut spectrum = Spectrum::new();
        // This tone completes exactly 40 cycles in each Fourier window.
        let mut packet = tone(937.5, 937.5, 0.5);
        packet.extend(tone(937.5, 937.5, 0.05));
        packet.extend([0; FFT_SIZE * FRAME_BYTES]);
        let observed = measurements(&mut spectrum, &packet);
        assert_eq!(observed.len(), 5);
        assert!((observed[0].envelope_rms - 0.87796).abs() < 1e-5);
        assert!((observed[2].envelope_rms - 0.60769).abs() < 1e-5);
        assert_eq!(observed[4], Measurement::default());
    }

    #[test]
    fn incomplete_window_does_not_affect_rms_after_reset() {
        let mut spectrum = Spectrum::new();
        let loud = tone(937.5, 937.5, 1.);
        assert!(measurements(&mut spectrum, &loud[..loud.len() / 2]).is_empty());
        spectrum.reset();
        assert_eq!(
            measurements(&mut spectrum, &[0; FFT_SIZE * FRAME_BYTES]),
            [Measurement::default()]
        );
    }

    #[test]
    fn isolates_all_six_bands_and_tracks_spectral_center() {
        for (expected, bin) in [1, 6, 16, 45, 125, 342].into_iter().enumerate() {
            let frequency = bin as f32 * SAMPLE_RATE as f32 / FFT_SIZE as f32;
            let features =
                measurements(&mut Spectrum::new(), &tone(frequency, frequency, 0.1))[0].features;
            let levels = [
                features.sub_bass,
                features.bass,
                features.low_mid,
                features.mid,
                features.high_mid,
                features.treble,
            ];
            assert!(levels[expected] > 0.5, "{features:?}");
            for (index, &level) in levels.iter().enumerate() {
                if index != expected {
                    assert!(level < 0.03, "{features:?}");
                }
            }
            assert!((features.spectral_centroid * AUDIBLE_END_HZ as f32 - frequency).abs() < 15.);
            assert!(features.peak > features.rms);
        }
    }

    #[test]
    fn detects_bass_onsets_without_retriggering_a_sustained_tone() {
        let frequency = 93.75;
        let tone = tone(frequency, frequency, 0.3);
        let mut spectrum = Spectrum::new();
        let onset = measurements(&mut spectrum, &tone)[0].features;
        assert!(
            onset.transient_strength > 0.4 && onset.beat_strength > 0.4,
            "{onset:?}"
        );
        for _ in 0..12 {
            for sustained in measurements(&mut spectrum, &tone) {
                let transient = sustained.features.transient_strength;
                assert!(transient < 0.04, "{sustained:?}");
                assert_eq!(sustained.features.beat_strength, 0.);
            }
        }
        measurements(&mut spectrum, &[0; FFT_SIZE * FRAME_BYTES]);
        let resumed = measurements(&mut spectrum, &tone);
        assert!(resumed[0].features.beat_strength > 0.2, "{resumed:?}");
        assert_eq!(resumed[1].features.beat_strength, 0.);
    }
}
