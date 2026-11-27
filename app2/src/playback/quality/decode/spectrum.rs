use std::{
    f64::consts::TAU,
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::atomic::AtomicBool,
};

use super::{FRAME_BYTES, SAMPLE_RATE, check_cancelled, pcm_error, samples};

const WINDOW_FRAMES: usize = 4096;
const MAX_WINDOWS: usize = 24;
const MIN_WINDOWS: usize = 8;
const BAND_HZ: usize = 250;
const BAND_COUNT: usize = SAMPLE_RATE as usize / (2 * BAND_HZ);

/// A recurring low-pass edge near common encoder cutoffs reduces the quality rank.
/// This estimates a tier. It cannot establish which encoder or master was used.
pub(super) fn estimate(
    file: &mut File,
    frames: u64,
    source_rate: u32,
    cancelled: &AtomicBool,
) -> Result<Option<u32>, String> {
    let windows = (frames / WINDOW_FRAMES as u64).min(MAX_WINDOWS as u64) as usize;
    let upper_hz = (source_rate.min(SAMPLE_RATE) / 2).saturating_sub(1_500) as usize;
    let last_band = upper_hz.min(20_500) / BAND_HZ;
    let first_band = 8_000 / BAND_HZ;
    if windows < MIN_WINDOWS || last_band < first_band {
        return Ok(None);
    }
    let mut pcm = vec![0; WINDOW_FRAMES * FRAME_BYTES];
    let mut values = vec![(0., 0.); WINDOW_FRAMES];
    let hann: Vec<_> = (0..WINDOW_FRAMES)
        .map(|index| (1. - (TAU * index as f64 / (WINDOW_FRAMES - 1) as f64).cos()) * 0.5)
        .collect();
    let mut votes = [0; BAND_COUNT];
    let span = frames - WINDOW_FRAMES as u64;
    for index in 0..windows {
        check_cancelled(cancelled)?;
        let start = span * index as u64 / (windows - 1) as u64;
        file.seek(SeekFrom::Start(start * FRAME_BYTES as u64))
            .map_err(pcm_error)?;
        file.read_exact(&mut pcm).map_err(pcm_error)?;
        let (left, right) = pcm
            .as_chunks::<FRAME_BYTES>()
            .0
            .iter()
            .map(samples)
            .fold((0., 0.), |power, (left, right)| {
                (power.0 + left * left, power.1 + right * right)
            });
        for (index, value) in values.iter_mut().enumerate() {
            let frame_index = index.reverse_bits() >> (usize::BITS - WINDOW_FRAMES.ilog2());
            let (l, r) = samples(&pcm.as_chunks::<FRAME_BYTES>().0[frame_index]);
            *value = ((if left >= right { l } else { r }) * hann[frame_index], 0.);
        }
        fft(&mut values);
        let bands = band_power(&values);
        if let Some(edge) = cutoff(&bands, first_band, last_band) {
            votes[edge] += 1;
        }
    }
    let support = |band: usize| votes[band - 1..=band + 1].iter().sum::<usize>();
    let edge = (first_band..=last_band)
        .max_by_key(|&band| (support(band), votes[band]))
        .filter(|&band| votes[band] > 0 && support(band) >= (windows * 2).div_ceil(3));
    Ok(edge.map(|band| match band * BAND_HZ {
        ..12_000 => 64_000,
        12_000..15_500 => 96_000,
        15_500..17_500 => 128_000,
        17_500..19_000 => 160_000,
        _ => 192_000,
    }))
}

fn fft(values: &mut [(f64, f64)]) {
    for power in 1..=values.len().ilog2() {
        let width = 1 << power;
        let half = width / 2;
        for group in values.chunks_exact_mut(width) {
            let (left, right) = group.split_at_mut(half);
            for (index, (a, b)) in left.iter_mut().zip(right).enumerate() {
                let (sine, cosine) = (-TAU * index as f64 / width as f64).sin_cos();
                let rotated = (b.0 * cosine - b.1 * sine, b.0 * sine + b.1 * cosine);
                *b = (a.0 - rotated.0, a.1 - rotated.1);
                *a = (a.0 + rotated.0, a.1 + rotated.1);
            }
        }
    }
}

fn band_power(values: &[(f64, f64)]) -> [f64; BAND_COUNT] {
    let mut bands = [0.; BAND_COUNT];
    for (index, &(real, imaginary)) in values.iter().enumerate().take(values.len() / 2).skip(1) {
        let hz = index * SAMPLE_RATE as usize / values.len();
        bands[hz / BAND_HZ] += real * real + imaginary * imaginary;
    }
    bands
}

fn cutoff(bands: &[f64; BAND_COUNT], first: usize, last: usize) -> Option<usize> {
    let total: f64 = bands.iter().sum();
    (first..=last).find(|&band| {
        let before = bands[band - 1].min(bands[band - 2]);
        // Require broadband evidence below the edge and at least 27 dB of attenuation
        // throughout the upper bands. Leave one band for the encoder transition.
        let above = bands[band + 1..].iter().copied().fold(0., f64::max);
        before > total * 0.000_032 && above < before * 0.002
    })
}
