mod spectrum;
#[cfg(test)]
mod tests;

use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use super::{
    super::mpv::{Event, Mpv},
    AudioQuality,
};

const DURATION_TOLERANCE_SECONDS: u32 = 2;
const DECODED_DURATION_TOLERANCE: f64 = 0.25;
const SAMPLE_RATE: u32 = 48_000;
const FRAME_BYTES: usize = 4;
const DECODE_TIMEOUT: Duration = Duration::from_secs(30);
const SILENCE_SECONDS: usize = 2;
const SILENCE_WINDOW_FRAMES: usize = SAMPLE_RATE as usize / 50;
// Integer stereo samples have full scale 32768. -60 dBFS RMS is 32.768.
const SILENCE_POWER: f64 = 32.768 * 32.768;

pub(crate) fn validate(
    path: &Path,
    expected_duration: Option<u32>,
    cancelled: &AtomicBool,
) -> Result<AudioQuality, String> {
    check_cancelled(cancelled)?;
    let expected = expected_duration
        .filter(|seconds| *seconds > 0)
        .ok_or("Нельзя проверить длительность файла без длительности записи.")?;
    let input = path
        .to_str()
        .filter(|_| path.is_absolute() && path.is_file())
        .ok_or("Файл для проверки звука недоступен.")?;
    check_container_length(path)?;
    let temp = tempfile::tempdir().map_err(|error| format!("Папка проверки звука: {error}"))?;
    let pcm = temp.path().join("decoded.pcm");
    let (mut quality, duration) = decode(
        input,
        pcm.to_str()
            .ok_or("Неподдерживаемый путь проверки звука.")?,
        expected,
        cancelled,
    )?;
    check_cancelled(cancelled)?;
    let mut pcm = File::open(pcm).map_err(pcm_error)?;
    let bytes = pcm.metadata().map_err(pcm_error)?.len();
    let frames = bytes / FRAME_BYTES as u64;
    let decoded_duration = frames as f64 / f64::from(SAMPLE_RATE);
    if bytes % FRAME_BYTES as u64 != 0
        || frames == 0
        || (decoded_duration - duration).abs() > DECODED_DURATION_TOLERANCE
        || !matches_duration(decoded_duration, expected)
    {
        return Err("Декодированные отсчёты не подтверждают длительность записи.".into());
    }
    check_sound_at_edges(&mut pcm, frames)?;
    if let Some(estimate) = spectrum::estimate(&mut pcm, frames, quality.sample_rate_hz, cancelled)?
        && (quality.is_lossless()
            || quality
                .bitrate_bps
                .is_some_and(|measured| measured >= estimate * 5 / 4))
    {
        quality.spectral_bitrate_bps = Some(estimate);
    }
    check_cancelled(cancelled)?;
    Ok(quality)
}

fn decode(
    input: &str,
    output: &str,
    expected: u32,
    cancelled: &AtomicBool,
) -> Result<(AudioQuality, f64), String> {
    // The extra second puts the decode limit beyond the accepted duration range.
    let limit = expected.saturating_add(DURATION_TOLERANCE_SECONDS + 1);
    let mpv = Mpv::open_pcm_decode(output, SAMPLE_RATE, limit)?;
    let id = mpv.enqueue(input, 0.)?;
    let mut duration = None;
    let mut quality = None;
    let deadline = Instant::now() + DECODE_TIMEOUT;
    while Instant::now() < deadline {
        check_cancelled(cancelled)?;
        match mpv.wait_event(0.025) {
            Some(Event::Loaded) => {
                let seconds = mpv
                    .double(c"duration")
                    .ok_or("libmpv не определил длительность.")?;
                if !matches_duration(seconds, expected) {
                    return Err("Длительность файла отличается от длительности записи.".into());
                }
                duration = Some(seconds);
            }
            Some(Event::Restarted) if quality.is_none() => {
                if mpv.string(c"current-ao").as_deref() != Some("pcm") {
                    return Err("libmpv не подтвердил декодирование PCM.".into());
                }
                quality = mpv.audio_quality();
                mpv.set(c"pause", "no")?;
            }
            Some(Event::End {
                id: finished,
                result,
            }) if finished == id => {
                result?;
                let quality = quality.ok_or("libmpv не подтвердил аудиодорожку файла.")?;
                if quality.bitrate_bps.is_none() {
                    return Err("libmpv не определил битрейт аудиодорожки.".into());
                }
                let duration = duration.ok_or("libmpv не определил длительность.")?;
                return Ok((quality, duration));
            }
            Some(Event::DecodeError | Event::Overflow | Event::UnexpectedStop) => {
                return Err("Ошибка декодирования аудиофайла libmpv.".into());
            }
            _ => {}
        }
    }
    Err("Истекло время проверки аудиофайла.".into())
}

fn check_container_length(path: &Path) -> Result<(), String> {
    let read_error = |error| format!("Не удалось прочитать файл для проверки звука: {error}");
    let mut file = File::open(path).map_err(read_error)?;
    let mut header = [0; 12];
    file.read_exact(&mut header).map_err(read_error)?;
    // FFmpeg can read a shortened WAV or AIFF to EOF and report only the available duration.
    // Check the declared container length as well as the duration reported by libmpv.
    let size = match (&header[..4], &header[8..12]) {
        (b"RIFF", b"WAVE") => u32::from_le_bytes(header[4..8].try_into().expect("RIFF size")),
        (b"FORM", b"AIFF" | b"AIFC") => {
            u32::from_be_bytes(header[4..8].try_into().expect("FORM size"))
        }
        _ => return Ok(()),
    };
    if u64::from(size) + 8 > file.metadata().map_err(read_error)?.len() {
        return Err("Длина файла меньше указанной в заголовке аудио.".into());
    }
    Ok(())
}

fn matches_duration(seconds: f64, expected: u32) -> bool {
    seconds > 0. && (seconds - f64::from(expected)).abs() <= f64::from(DURATION_TOLERANCE_SECONDS)
}

fn check_sound_at_edges(file: &mut File, frames: u64) -> Result<(), String> {
    let edge_frames = frames.min(SILENCE_SECONDS as u64 * u64::from(SAMPLE_RATE));
    let mut edge = vec![0; edge_frames as usize * FRAME_BYTES];
    for (offset, error) in [
        (0, "В начале аудиофайла слишком длинная тишина."),
        (
            frames - edge_frames,
            "В конце аудиофайла слишком длинная тишина.",
        ),
    ] {
        file.seek(SeekFrom::Start(offset * FRAME_BYTES as u64))
            .map_err(pcm_error)?;
        file.read_exact(&mut edge).map_err(pcm_error)?;
        if !edge
            .chunks(SILENCE_WINDOW_FRAMES * FRAME_BYTES)
            .any(audible)
        {
            return Err(error.into());
        }
    }
    Ok(())
}

fn audible(window: &[u8]) -> bool {
    let mut power = [0.; 2];
    for frame in window.as_chunks::<FRAME_BYTES>().0 {
        let (left, right) = samples(frame);
        power[0] += left * left;
        power[1] += right * right;
    }
    power[0].max(power[1]) > SILENCE_POWER * (window.len() / FRAME_BYTES) as f64
}

fn samples(frame: &[u8; FRAME_BYTES]) -> (f64, f64) {
    let [left, right] = frame.as_chunks::<2>().0.try_into().expect("stereo frame");
    (
        f64::from(i16::from_le_bytes(left)),
        f64::from(i16::from_le_bytes(right)),
    )
}

fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Проверка аудиофайла отменена.".into());
    }
    Ok(())
}

fn pcm_error(error: std::io::Error) -> String {
    format!("Не удалось прочитать отсчёты libmpv: {error}")
}
