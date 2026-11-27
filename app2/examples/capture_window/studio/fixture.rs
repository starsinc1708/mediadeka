use std::{
    f64::consts::TAU,
    fs::{self, File},
    io::{self, BufWriter, Cursor, Write as _},
    path::Path,
};

use lofty::{
    config::WriteOptions,
    tag::{Accessor as _, Tag, TagExt as _, TagType},
};

use super::{COVER, TITLE};

pub(super) const WARM_COVER: &str = "https://i.scdn.co/image/mediadeka-fluid-glass-warm-cover";
pub(super) const MULTICOLOR_COVER: &str =
    "https://i.scdn.co/image/mediadeka-fluid-glass-multi-cover";
pub(super) const WAVEFORM_TONE_HZ: f64 = 1000.;

pub(super) const SPECTRUM_TONES: [(&str, f64); 3] = [
    (TITLE, 80.),
    ("Северный ветер", WAVEFORM_TONE_HZ),
    ("Голоса вдали", 8000.),
];

const TRACKS: [(&str, &str, &str, f64); 8] = [
    (
        SPECTRUM_TONES[0].0,
        "Полярная линия",
        "Открытая вода",
        SPECTRUM_TONES[0].1,
    ),
    (
        SPECTRUM_TONES[1].0,
        "Берег",
        "Линия прибоя",
        SPECTRUM_TONES[1].1,
    ),
    (
        SPECTRUM_TONES[2].0,
        "Лунный свет",
        "Радиоволна",
        SPECTRUM_TONES[2].1,
    ),
    ("Новые берега", "Ручьи", "Песок и соль", 294.),
    ("Последнее окно", "Дальний план", "Корабли", 330.),
    ("Станции без имён", "Берег", "Дорога на юг", 349.),
    ("Следы на стекле", "Полярная линия", "Переулки", 392.),
    ("Свет в переходе", "Город во сне", "Горизонт", 440.),
];

pub(super) fn seed(profile: &Path, extended_playback: bool) -> gpui::Result<()> {
    let mut paths = Vec::with_capacity(TRACKS.len());
    for (index, (title, artist, album, hz)) in TRACKS.into_iter().enumerate() {
        let path = profile.join(format!("track-{index}.wav"));
        let seconds = if extended_playback && index == 0 {
            60
        } else {
            18 + index as u32 * 4
        };
        write_wave(&path, hz, seconds)?;
        let mut tags = Tag::new(TagType::RiffInfo);
        tags.set_title(title.to_owned());
        tags.set_artist(artist.to_owned());
        tags.set_album(album.to_owned());
        tags.save_to_path(&path, WriteOptions::default())?;
        paths.push(path);
    }
    mediadeka::seed_playlist_capture(profile, &paths)?;
    // A rectangular cyan image exercises cover cropping and palette extraction.
    let pixels = image::RgbImage::from_fn(320, 200, |x, y| {
        let dx = x as i32 - 160;
        let dy = y as i32 - 100;
        if dx * dx + dy * dy < 62 * 62 {
            image::Rgb([52, 193, 202])
        } else {
            image::Rgb([22, 100, 148])
        }
    });
    seed_cover(profile, "cover", COVER, pixels)?;

    let warm = image::Rgb([231, 95, 37]);
    seed_cover(
        profile,
        "cover-warm",
        WARM_COVER,
        image::RgbImage::from_pixel(256, 256, warm),
    )?;
    seed_cover(
        profile,
        "cover-multicolor",
        MULTICOLOR_COVER,
        image::RgbImage::from_fn(256, 256, |x, _| match x {
            0..160 => warm,
            160..220 => image::Rgb([45, 80, 222]),
            _ => image::Rgb([205, 39, 144]),
        }),
    )
}

fn seed_cover(profile: &Path, name: &str, url: &str, pixels: image::RgbImage) -> gpui::Result<()> {
    let cover = profile.join(format!("{name}.cover"));
    // The artwork cache wraps PNG bytes with the format tag 2.
    let mut encoded = Cursor::new(vec![2]);
    encoded.set_position(1);
    pixels.write_to(&mut encoded, image::ImageFormat::Png)?;
    fs::write(&cover, encoded.into_inner())?;
    mediadeka::MainView::seed_artist_artwork_for_capture(profile, url, &cover)?;
    Ok(())
}

pub(super) fn write_wave(path: &Path, hz: f64, seconds: u32) -> io::Result<()> {
    const RATE: u32 = 44_100;
    const FRAME_BYTES: u32 = 2;
    const MODULATION_HZ: f64 = 2.;
    const QUIET_GAIN: f64 = 0.28;
    let data_size = RATE * seconds * FRAME_BYTES;
    let mut file = BufWriter::new(File::create(path)?);
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_size).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16_u32.to_le_bytes())?;
    file.write_all(&1_u16.to_le_bytes())?; // PCM
    file.write_all(&1_u16.to_le_bytes())?; // Mono
    file.write_all(&RATE.to_le_bytes())?;
    file.write_all(&(RATE * FRAME_BYTES).to_le_bytes())?;
    file.write_all(&(FRAME_BYTES as u16).to_le_bytes())?;
    file.write_all(&16_u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_size.to_le_bytes())?;
    let second: Vec<_> = (0..RATE)
        .flat_map(|sample| {
            let time = f64::from(sample) / f64::from(RATE);
            // The carrier and raised-cosine envelope complete whole cycles in
            // one second, so the recurring block has no phase discontinuity.
            let gain = if hz == WAVEFORM_TONE_HZ {
                let pulse = (1. - (time * MODULATION_HZ * TAU).cos()) * 0.5;
                QUIET_GAIN + (1. - QUIET_GAIN) * pulse
            } else {
                1.
            };
            let carrier = (f64::from(sample) * hz * TAU / f64::from(RATE)).sin();
            let amplitude = 2000. * carrier * gain;
            (amplitude as i16).to_le_bytes()
        })
        .collect();
    for _ in 0..seconds {
        file.write_all(&second)?;
    }
    file.flush()
}
