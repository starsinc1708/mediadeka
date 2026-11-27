use std::{fs, path::Path};

use image::RgbImage;
use serde::Serialize;
use serde_json::json;

use super::{luminance, require};

type Rgb = [u8; 3];
type Region = [u32; 4];

// Evidence is captured at 1920×1080 and 100% DPI in the standard Studio flow.
const FRAMES: [(&str, &str); 4] = [
    ("solar", "library.png"),
    ("cyan_artwork", "cover-palette-4600.png"),
    ("active_bass", "audio-80-hz-playing.png"),
    ("chromatic_pulse", "track-transition-pulse.png"),
];
const ROW_START: u32 = 185;
const ROW_HEIGHT: u32 = 44;
const ROWS: u32 = 8;
const ROW_SAMPLE_Y: [u32; 2] = [7, 36];
const UPPER_ATMOSPHERE: Region = [1460, 75, 1688, 146];

// The table uses theme::SECONDARY for metadata and Fluid Glass SECONDARY
// for enqueue. Each x range sits between glyphs. The row interiors include
// the actual selected-track tint while leaving space around divider lines.
const METADATA: Rgb = [195, 186, 177];
const QUEUE_ACTION: Rgb = [176, 170, 185];
const PROBES: [Probe; 4] = [
    Probe::metadata("before_date", [1572, 1608]),
    Probe::metadata("between_date_and_duration", [1679, 1698]),
    // The active row can also show a focus outline at x=1753.
    Probe::metadata("between_duration_and_actions", [1746, 1753]),
    Probe {
        name: "before_enqueue_action",
        x: [1810, 1834],
        foreground: QUEUE_ACTION,
        minimum: 3.,
    },
];

pub(super) fn verify(output: &Path) -> gpui::Result<()> {
    let captures = FRAMES
        .into_iter()
        .map(|(state, frame)| {
            let image = image::open(output.join(frame))?.into_rgb8();
            require(
                image.dimensions() == (1920, 1080),
                &format!("Замер читаемости {frame} требует кадр 1920 × 1080."),
            )?;
            Ok(FrameSample {
                state,
                frame,
                probes: PROBES.map(|probe| probe.measure(&image)),
                upper_atmosphere_mean_rgb: mean_rgb(&image, UPPER_ATMOSPHERE),
            })
        })
        .collect::<gpui::Result<Vec<_>>>()?;
    let minimum_metadata = minimum(&captures, METADATA);
    let minimum_queue_action = minimum(&captures, QUEUE_ACTION);
    fs::write(
        output.join("workspace-readability.json"),
        serde_json::to_vec_pretty(&json!({
            "coordinates": "physical_pixels_at_100_percent_dpi",
            "library_row_top": ROW_START,
            "library_row_stride": ROW_HEIGHT,
            "library_row_count": ROWS,
            "row_interior_y_offsets_exclusive": ROW_SAMPLE_Y,
            "upper_atmosphere_region_exclusive": UPPER_ATMOSPHERE,
            "minimum_metadata_contrast": minimum_metadata,
            "minimum_enqueue_icon_contrast": minimum_queue_action,
            "samples": captures,
        }))?,
    )?;
    let failures: Vec<_> = captures
        .iter()
        .flat_map(|capture| {
            capture
                .probes
                .iter()
                .map(move |probe| (capture.frame, probe))
        })
        .filter(|(_, probe)| probe.contrast < probe.minimum_contrast)
        .map(|(frame, probe)| {
            format!(
                "{frame} {} {:.2}:1 at {:?} RGB {:?}",
                probe.zone, probe.contrast, probe.worst_pixel, probe.actual_background_rgb
            )
        })
        .collect();
    require(
        failures.is_empty(),
        &format!(
            "Контраст в строках медиатеки ниже целевого. {}.",
            failures.join(", ")
        ),
    )?;
    let [solar, cyan, ..] = captures.as_slice() else {
        unreachable!("Reading area evidence includes Solar and cyan");
    };
    let [solar_red, _, solar_blue] = solar.upper_atmosphere_mean_rgb;
    let [cyan_red, _, cyan_blue] = cyan.upper_atmosphere_mean_rgb;
    require(
        solar_red > solar_blue + 8. && cyan_blue > cyan_red + 8.,
        "Над строками медиатеки должны оставаться различимы тёплое и бирюзовое световые поля.",
    )?;
    println!(
        "Workspace readability: metadata ≥{minimum_metadata:.2}:1, enqueue ≥{minimum_queue_action:.2}:1 across Solar, cyan, bass and pulse"
    );
    Ok(())
}

#[derive(Clone, Copy)]
struct Probe {
    name: &'static str,
    x: [u32; 2],
    foreground: Rgb,
    minimum: f64,
}

impl Probe {
    const fn metadata(name: &'static str, x: [u32; 2]) -> Self {
        Self {
            name,
            x,
            foreground: METADATA,
            minimum: 4.5,
        }
    }

    fn measure(self, image: &RgbImage) -> ContrastSample {
        let [left, right] = self.x;
        let [start, end] = ROW_SAMPLE_Y;
        let (worst_pixel, background, background_luma) = (0..ROWS)
            .flat_map(|row| {
                ROW_START + row * ROW_HEIGHT + start..ROW_START + row * ROW_HEIGHT + end
            })
            .flat_map(|y| (left..right).map(move |x| [x, y]))
            .map(|[x, y]| {
                let rgb = image.get_pixel(x, y).0;
                ([x, y], rgb, luminance(rgb))
            })
            .max_by(|a, b| a.2.total_cmp(&b.2))
            .expect("Nonempty metadata row interiors");
        ContrastSample {
            zone: self.name,
            x_span_exclusive: self.x,
            nominal_foreground_rgb: self.foreground,
            actual_background_rgb: background,
            worst_pixel,
            contrast: (luminance(self.foreground) + 0.05) / (background_luma + 0.05),
            minimum_contrast: self.minimum,
        }
    }
}

#[derive(Serialize)]
struct ContrastSample {
    zone: &'static str,
    x_span_exclusive: [u32; 2],
    nominal_foreground_rgb: Rgb,
    actual_background_rgb: Rgb,
    worst_pixel: [u32; 2],
    contrast: f64,
    minimum_contrast: f64,
}

#[derive(Serialize)]
struct FrameSample {
    state: &'static str,
    frame: &'static str,
    probes: [ContrastSample; PROBES.len()],
    upper_atmosphere_mean_rgb: [f64; 3],
}

fn minimum(samples: &[FrameSample], foreground: Rgb) -> f64 {
    samples
        .iter()
        .flat_map(|sample| &sample.probes)
        .filter(|probe| probe.nominal_foreground_rgb == foreground)
        .map(|probe| probe.contrast)
        .fold(f64::INFINITY, f64::min)
}

fn mean_rgb(image: &RgbImage, [left, top, right, bottom]: Region) -> [f64; 3] {
    let mut sums = [0.; 3];
    for y in top..bottom {
        for x in left..right {
            for (sum, channel) in sums.iter_mut().zip(image.get_pixel(x, y).0) {
                *sum += f64::from(channel);
            }
        }
    }
    let count = f64::from((right - left) * (bottom - top));
    sums.map(|sum| sum / count)
}
