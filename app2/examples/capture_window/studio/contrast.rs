use std::{env, fs, path::Path};

use gpui::{AsyncApp, WindowHandle};
use image::RgbImage;
use mediadeka::MainView;
use serde::Serialize;
use serde_json::{Value, json};

use super::{
    QUEUE_LIST, SAVED_RESULT, Scene, focused, frame, luminance, node_label, queue_entry, require,
    snapshot,
};

type Rgb = [u8; 3];
type Region = [u32; 4];

const CANVAS: Rgb = [5, 5, 7];
const SHELL: Rgb = [16, 16, 22];
const TEXT_CONTRAST: f64 = 7.;
const CONTROL_CONTRAST: f64 = 3.;
const FOCUS_CONTRAST: f64 = 4.5;
const COMPACT_INNER_CURVE: Region = [76, 310, 106, 326];
const COMPACT_FILTER_GLYPH: Region = [716, 79, 732, 92];
const COMPACT_FILTER_OUTLINE: Region = [716, 68, 732, 73];

struct Probes {
    body: Region,
    rail: Region,
    player: Region,
    queue: Region,
    seam: Region,
    body_text: Region,
    library_artist: Region,
    player_title: Region,
    player_artist: Region,
    queue_text: Region,
    search_field_border: Region,
    search_focus: Region,
}

const WIDE: Probes = Probes {
    body: [1450, 555, 1840, 860],
    rail: [74, 545, 224, 730],
    player: [380, 983, 495, 1038],
    queue: [1650, 264, 1840, 770],
    seam: [15, 480, 25, 780],
    body_text: [308, 112, 499, 145],
    library_artist: [1049, 200, 1135, 221],
    player_title: [124, 984, 237, 1003],
    player_artist: [124, 1009, 237, 1023],
    queue_text: [1620, 185, 1750, 207],
    search_field_border: [550, 23, 820, 34],
    search_focus: [700, 191, 1060, 205],
};
const COMPACT: Probes = Probes {
    // The reserved refresh slot leaves the filter control at the right of this sample.
    body: [500, 88, 625, 111],
    rail: [53, 145, 70, 166],
    player: [225, 336, 269, 350],
    queue: [615, 220, 760, 303],
    seam: [3, 135, 13, 170],
    body_text: [96, 72, 228, 100],
    library_artist: [142, 200, 277, 215],
    player_title: [70, 337, 192, 352],
    player_artist: [70, 358, 192, 372],
    queue_text: [120, 175, 255, 197],
    search_field_border: [240, 8, 350, 14],
    search_focus: [370, 169, 670, 182],
};

pub(super) fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    queue_background_top: u32,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let compact = scene.width == 800.;
    let probes = if scene.width == 1920. {
        &WIDE
    } else {
        &COMPACT
    };
    let library = read_image(output, "library")?;
    let search = read_image(output, "search")?;
    let playing = read_image(output, "playing")?;
    let queue = read_image(output, "queue")?;
    let [left, top, right, bottom] = probes.queue;
    let queue_background = [left, top.max(queue_background_top), right, bottom];
    require(
        queue_background[1] < bottom,
        "Под треками не осталось области для замера фона.",
    )?;
    let surfaces = [
        SurfaceSample::sample("library_body", &library, probes.body, CANVAS),
        SurfaceSample::sample("navigation_rail", &library, probes.rail, SHELL),
        SurfaceSample::sample("player", &playing, probes.player, SHELL),
        SurfaceSample::sample("queue", &queue, queue_background, SHELL),
    ];
    let text = [
        ContrastSample::sample("library_heading", &library, probes.body_text, CANVAS),
        ContrastSample::sample("library_artist", &library, probes.library_artist, CANVAS),
        ContrastSample::sample("player_track_title", &playing, probes.player_title, SHELL),
        ContrastSample::sample("player_artist", &playing, probes.player_artist, SHELL),
        ContrastSample::sample("queue_track", &queue, probes.queue_text, SHELL),
    ];
    let seam = ContrastSample::sample("shell_outline", &library, probes.seam, SHELL);
    let search_border = horizontal_line(
        &library,
        probes.search_field_border,
        SHELL,
        CONTROL_CONTRAST,
    );
    let focus = horizontal_line(&search, probes.search_focus, CANVAS, FOCUS_CONTRAST);
    let queue_junction = compact.then(|| junction_stroke(&queue, COMPACT_INNER_CURVE));
    let filters = if compact {
        let library_tree = read_tree(output, "library")?;
        Some(FilterSample {
            accessible_label: node_label(&library_tree, "library-filters").map(str::to_owned),
            label: ContrastSample::sample(
                "library_filters_glyph",
                &library,
                COMPACT_FILTER_GLYPH,
                SHELL,
            ),
            outline: horizontal_line(&library, COMPACT_FILTER_OUTLINE, CANVAS, CONTROL_CONTRAST),
        })
    } else {
        None
    };
    let search_tree = read_tree(output, "search")?;
    let search_focus = focused(&search_tree);
    let round_trip = verify_live_toggle(window, scene, output, probes, cx)?;
    fs::write(
        output.join("high-contrast.json"),
        serde_json::to_vec_pretty(&json!({
            "client_pixels": [library.width(), library.height()],
            "palette": { "canvas_rgb": CANVAS, "opaque_material_rgb": SHELL },
            "native_fill_samples": surfaces,
            "text_samples": text,
            "shell_outline": seam,
            "compact_queue_junction": queue_junction,
            "compact_filters": filters,
            "search_field_border": search_border,
            "search_result": { "keyboard_focus": search_focus, "visible_focus_line": focus },
            "while_queue_open": round_trip,
        }))?,
    )?;

    require(
        surfaces.iter().all(SurfaceSample::is_flat),
        &format!(
            "Контрастные фон, меню, плеер и очередь должны сохранить сплошные заливки: {surfaces:?}"
        ),
    )?;
    require(
        text.iter().all(|sample| sample.contrast >= TEXT_CONTRAST)
            && seam.contrast >= CONTROL_CONTRAST,
        &format!(
            "Текст должен достигать 7:1, контур оболочки 3:1. Текст {text:?}, контур {seam:?}."
        ),
    )?;
    require(
        search_border.is_visible(),
        &format!(
            "Рамка глобального поиска должна сохранять контраст 3:1 вдоль поля: {search_border:?}."
        ),
    )?;
    require(
        filters.as_ref().is_none_or(FilterSample::is_visible),
        &format!(
            "Кнопка «Фильтры» должна сохранять доступное имя, контраст глифа 7:1 и контур 3:1: {filters:?}."
        ),
    )?;
    require(
        queue_junction
            .as_ref()
            .is_none_or(JunctionSample::is_visible),
        &format!(
            "Контур внутреннего изгиба должен оставаться виден при открытой очереди: {queue_junction:?}."
        ),
    )?;
    require(
        search_focus == Some(SAVED_RESULT) && focus.is_visible(),
        &format!(
            "Строка поиска должна сохранять доступный и видимый клавиатурный фокус: {focus:?}."
        ),
    )?;
    println!(
        "{} high contrast: opaque shell, keyboard focus, real library/search/playback/queue, runtime palette",
        scene.name
    );
    Ok(())
}

fn read_image(output: &Path, stem: &str) -> gpui::Result<RgbImage> {
    Ok(image::open(output.join(format!("{stem}.png")))?.to_rgb8())
}

fn read_tree(output: &Path, stem: &str) -> gpui::Result<Value> {
    Ok(serde_json::from_slice(&fs::read(
        output.join(format!("{stem}-a11y.json")),
    )?)?)
}

#[derive(Debug, Serialize)]
struct SurfaceSample {
    name: &'static str,
    region_exclusive: Region,
    expected_rgb: Rgb,
    first_pixel_rgb: Rgb,
    matching_pixels: usize,
    total_pixels: usize,
}

impl SurfaceSample {
    fn sample(name: &'static str, image: &RgbImage, region: Region, expected: Rgb) -> Self {
        let [left, top, right, bottom] = region;
        Self {
            name,
            region_exclusive: region,
            expected_rgb: expected,
            first_pixel_rgb: image.get_pixel(left, top).0,
            matching_pixels: samples(image, region)
                .filter(|pixel| *pixel == expected)
                .count(),
            total_pixels: ((right - left) * (bottom - top)) as usize,
        }
    }

    fn is_flat(&self) -> bool {
        self.matching_pixels == self.total_pixels
    }
}

#[derive(Debug, Serialize)]
struct ContrastSample {
    name: &'static str,
    region_exclusive: Region,
    peak_rgb: Rgb,
    contrast: f64,
}

impl ContrastSample {
    fn sample(name: &'static str, image: &RgbImage, region: Region, background: Rgb) -> Self {
        let (peak_rgb, peak_luminance) = samples(image, region)
            .map(|rgb| (rgb, luminance(rgb)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .expect("Nonempty native contrast sample");
        Self {
            name,
            region_exclusive: region,
            peak_rgb,
            contrast: color_contrast(peak_luminance, luminance(background)),
        }
    }
}

#[derive(Debug, Serialize)]
struct HorizontalLineSample {
    region_exclusive: Region,
    minimum_contrast: f64,
    probe_width: usize,
    lit_pixels: usize,
}

#[derive(Debug, Serialize)]
struct FilterSample {
    accessible_label: Option<String>,
    label: ContrastSample,
    outline: HorizontalLineSample,
}

impl FilterSample {
    fn is_visible(&self) -> bool {
        self.accessible_label.as_deref() == Some("Фильтры")
            && self.label.contrast >= TEXT_CONTRAST
            && self.outline.is_visible()
    }
}

impl HorizontalLineSample {
    fn is_visible(&self) -> bool {
        self.lit_pixels >= self.probe_width * 9 / 10
    }
}

fn horizontal_line(
    image: &RgbImage,
    region: Region,
    background: Rgb,
    minimum_contrast: f64,
) -> HorizontalLineSample {
    let [left, top, right, bottom] = region;
    let background = luminance(background);
    let lit_pixels = (top..bottom)
        .map(|y| {
            (left..right)
                .filter(|&x| {
                    color_contrast(luminance(image.get_pixel(x, y).0), background)
                        >= minimum_contrast
                })
                .count()
        })
        .max()
        .unwrap_or_default();
    HorizontalLineSample {
        region_exclusive: region,
        minimum_contrast,
        probe_width: (right - left) as usize,
        lit_pixels,
    }
}

fn samples(image: &RgbImage, [left, top, right, bottom]: Region) -> impl Iterator<Item = Rgb> + '_ {
    (top..bottom).flat_map(move |y| (left..right).map(move |x| image.get_pixel(x, y).0))
}

fn color_contrast(lighter: f64, darker: f64) -> f64 {
    (lighter + 0.05) / (darker + 0.05)
}

#[derive(Debug, Serialize)]
struct JunctionSample {
    region_exclusive: Region,
    minimum_row_contrast: f64,
    visible_rows: usize,
    total_rows: usize,
}

impl JunctionSample {
    fn is_visible(&self) -> bool {
        self.visible_rows == self.total_rows
    }
}

fn junction_stroke(image: &RgbImage, region: Region) -> JunctionSample {
    let [left, top, right, bottom] = region;
    let mut minimum_row_contrast = f64::INFINITY;
    let mut visible_rows = 0;
    for y in top..bottom {
        let row = ContrastSample::sample("junction", image, [left, y, right, y + 1], SHELL);
        minimum_row_contrast = minimum_row_contrast.min(row.contrast);
        visible_rows += usize::from(row.contrast >= CONTROL_CONTRAST);
    }
    JunctionSample {
        region_exclusive: region,
        minimum_row_contrast,
        visible_rows,
        total_rows: (bottom - top) as usize,
    }
}

#[derive(Debug, Serialize)]
struct LiveState {
    quality: Option<&'static str>,
    geometry_dip: [f32; 7],
    focus: Option<String>,
    current_track: Option<String>,
    transport_label: Option<String>,
    queue_label: Option<String>,
    queue_selected: [Value; 2],
}

impl LiveState {
    fn read(window: WindowHandle<MainView>, tree: &Value, cx: &mut AsyncApp) -> gpui::Result<Self> {
        let (quality, geometry_dip) = window.update(cx, |view, window, cx| {
            (
                view.fluid_glass_quality_for_capture(window, cx)
                    .map(|(quality, _, _, _)| quality),
                view.fluid_glass_geometry_for_capture(window),
            )
        })?;
        Ok(Self {
            quality,
            geometry_dip,
            focus: focused(tree).map(str::to_owned),
            current_track: node_label(tree, "player-track").map(str::to_owned),
            transport_label: node_label(tree, "player-play").map(str::to_owned),
            queue_label: node_label(tree, QUEUE_LIST).map(str::to_owned),
            queue_selected: std::array::from_fn(|index| {
                queue_entry(tree, index)
                    .map(|row| row["aria"]["selected"].clone())
                    .unwrap_or(Value::Null)
            }),
        })
    }

    fn retains_context(&self, other: &Self) -> bool {
        self.geometry_dip == other.geometry_dip
            && self.focus == other.focus
            && self.current_track == other.current_track
            && self.transport_label == other.transport_label
            && self.queue_label == other.queue_label
            && self.queue_selected == other.queue_selected
    }
}

fn verify_live_toggle(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    probes: &Probes,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let raw = window.into();
    let tree = snapshot(raw, cx)?;
    let before = LiveState::read(window, &tree, cx)?;
    let regular = toggle_and_capture(window, scene, output, false, cx)?;
    let after = toggle_and_capture(window, scene, output, true, cx)?;
    let regular_frame = read_image(output, "high-contrast-live-off")?;
    let after_frame = read_image(output, "high-contrast-live-on")?;
    let regular_fill = SurfaceSample::sample("regular_rail", &regular_frame, probes.rail, SHELL);
    let restored_fill = SurfaceSample::sample("restored_rail", &after_frame, probes.rail, SHELL);
    let effects_on = !env::var_os("MEDIADEKA_GLASS_EFFECTS").is_some_and(|value| value == "off");
    let retained = before.retains_context(&regular) && before.retains_context(&after);
    let evidence = json!({
        "high_contrast_before": before,
        "regular_palette": regular,
        "high_contrast_restored": after,
        "regular_rail_fill": regular_fill,
        "restored_rail_fill": restored_fill,
        "geometry_playback_queue_and_focus_retained": retained,
    });
    fs::write(
        output.join("high-contrast-live.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    require(
        before.quality.is_none()
            && regular.quality.is_some() == effects_on
            && after.quality.is_none()
            && before.queue_label.as_deref() == Some("Треки в очереди")
            && before.focus.is_some()
            && before.current_track.is_some()
            && before.transport_label.as_deref() == Some("Пауза")
            && before.queue_selected == [Value::Bool(false), Value::Bool(true)]
            && retained
            && !regular_fill.is_flat()
            && restored_fill.is_flat(),
        &format!(
            "Смена контрастности должна сохранить геометрию, фокус, очередь и текущую запись: {evidence}"
        ),
    )?;
    Ok(evidence)
}

fn toggle_and_capture(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    enabled: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<LiveState> {
    window.update(cx, |view, _, cx| {
        view.set_fluid_high_contrast_for_capture(enabled, cx)
    })?;
    let stem = if enabled {
        "high-contrast-live-on"
    } else {
        "high-contrast-live-off"
    };
    let tree = frame(window.into(), scene, output, stem, cx)?;
    LiveState::read(window, &tree, cx)
}
