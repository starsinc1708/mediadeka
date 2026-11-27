use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use super::{FRAME_BYTES, SAMPLE_RATE, validate};

#[test]
#[ignore = "requires the bundled libmpv beside the test executable"]
fn validates_audio_and_rejects_silent_edges_wrong_duration_and_corruption() {
    let directory = tempfile::tempdir().unwrap();
    let valid = directory.path().join("audible.wav");
    let leading = directory.path().join("leading.wav");
    let trailing = directory.path().join("trailing.wav");
    let truncated = directory.path().join("truncated.wav");
    wave(&valid, |frame| frame > 1000, 4);
    wave(&leading, |frame| frame >= SAMPLE_RATE * 5 / 2, 4);
    wave(&trailing, |frame| frame < SAMPLE_RATE * 3 / 2, 4);
    let mut broken = fs::read(&valid).unwrap();
    broken.truncate(44 + SAMPLE_RATE as usize * FRAME_BYTES * 3);
    fs::write(&truncated, broken).unwrap();
    let active = AtomicBool::new(false);
    let quality = validate(&valid, Some(4), &active).unwrap();
    assert_eq!(quality.label(), "PCM · 1536 кбит/с · 48 кГц");
    assert_eq!(quality.rank(), (true, 1_536_000, 48_000));
    assert!(
        validate(&valid, Some(9), &active)
            .unwrap_err()
            .contains("Длительность")
    );
    assert!(
        validate(&leading, Some(4), &active)
            .unwrap_err()
            .contains("В начале")
    );
    assert!(
        validate(&trailing, Some(4), &active)
            .unwrap_err()
            .contains("В конце")
    );
    assert!(validate(&truncated, Some(4), &active).is_err());
    assert!(validate(&valid, None, &active).is_err());
    assert!(
        validate(&valid, Some(4), &AtomicBool::new(true))
            .unwrap_err()
            .contains("отменена")
    );
    println!(
        "libmpv decoded {quality:?} and rejected wrong duration, silent edges, truncation and cancellation"
    );
}

#[test]
#[ignore = "requires libmpv and broadband native and MP3-128-transcoded FLAC fixtures"]
fn spectral_rank_detects_a_lossy_to_lossless_transcode() {
    let read_fixture = |name| PathBuf::from(env::var_os(name).expect("path to FLAC fixture"));
    let native = read_fixture("MEDIADEKA_AUDIO_NATIVE_FLAC");
    let transcoded = read_fixture("MEDIADEKA_AUDIO_TRANSCODED_FLAC");
    let active = AtomicBool::new(false);
    let native = validate(&native, Some(4), &active).unwrap();
    let transcoded = validate(&transcoded, Some(4), &active).unwrap();
    assert_eq!(native.codec, "flac");
    assert_eq!(transcoded.codec, "flac");
    assert!(native.bitrate_bps.is_some());
    assert!(transcoded.bitrate_bps.is_some());
    assert!(native.rank().0, "native {native:?}");
    assert_eq!(
        transcoded.rank(),
        (false, 128_000, 48_000),
        "{transcoded:?}"
    );
    println!("native {native:?}; converted from MP3-128 {transcoded:?}");
}

fn wave(path: &Path, is_audible: impl Fn(u32) -> bool, seconds: u32) {
    let frames = SAMPLE_RATE * seconds;
    let data_size = frames * FRAME_BYTES as u32;
    let mut data = Vec::new();
    data.extend(b"RIFF");
    data.extend((36 + data_size).to_le_bytes());
    data.extend(b"WAVEfmt ");
    data.extend(16_u32.to_le_bytes());
    data.extend(1_u16.to_le_bytes());
    data.extend(2_u16.to_le_bytes());
    data.extend(SAMPLE_RATE.to_le_bytes());
    data.extend((SAMPLE_RATE * FRAME_BYTES as u32).to_le_bytes());
    data.extend((FRAME_BYTES as u16).to_le_bytes());
    data.extend(16_u16.to_le_bytes());
    data.extend(b"data");
    data.extend(data_size.to_le_bytes());
    let mut random = 1_u32;
    for frame in 0..frames {
        random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let sample = if is_audible(frame) {
            (random >> 18) as i16 - 8192
        } else {
            0
        };
        data.extend(sample.to_le_bytes());
        data.extend(sample.to_le_bytes());
    }
    fs::write(path, data).unwrap();
}
