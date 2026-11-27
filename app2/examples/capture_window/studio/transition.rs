use std::{fs, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::super::render_image;
use super::{
    COVER, QUEUE_LIST, Scene, TITLE,
    fixture::{SPECTRUM_TONES, WARM_COVER},
    focused, named, node, node_label, press, queue_candidate, require, snapshot, sound, wait,
};

const BASELINE_WALL: u64 = 20_000;
const SWITCH_WALL: u64 = 23_000;
const SHADER_PHASE: Duration = Duration::from_millis(4600);
const NEXT: &str = "player-next";
type Region = [u32; 4];
type Pixel = [u8; 4];

// The player strip is below seek controls. The workspace strip is deep
// inside the route background, away from the selected row and edge lighting.
const PLAYER_PALETTE: Region = [550, 1042, 850, 1052];
const WORKSPACE_PALETTE: Region = [1380, 610, 1580, 700];
const COMPACT_PLAYER_PALETTE: Region = [300, 383, 420, 387];

#[derive(Clone, Copy)]
struct ProbeLayout {
    rim: Region,
    light: Region,
    digital: &'static [Region],
    digital_control: Region,
    art: Region,
    art_center: [u32; 2],
    art_edge: u32,
}

const DESKTOP: ProbeLayout = ProbeLayout {
    rim: [236, 904, 610, 973],
    light: [1380, 71, 1690, 150],
    // Between the provider, date, duration, and control columns. The rows
    // highlighted by Next start below this probe.
    digital: &[
        [1558, 194, 1607, 362],
        [1678, 194, 1699, 362],
        [1731, 194, 1754, 362],
        [1810, 194, 1834, 362],
    ],
    digital_control: [1000, 194, 1138, 362],
    art: [40, 971, 123, 1049],
    art_center: [83, 1010],
    art_edge: 68,
};
const COMPACT: ProbeLayout = ProbeLayout {
    rim: [72, 301, 305, 338],
    light: [510, 63, 769, 105],
    digital: &[[626, 102, 767, 138]],
    digital_control: [440, 102, 525, 138],
    art: [17, 338, 68, 385],
    art_center: [43, 360],
    art_edge: 37,
};

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    let id = playlist_id(raw, cx).await?;
    set_time(window, BASELINE_WALL, cx)?;
    window.update(cx, |view, _, cx| {
        view.play_fluid_glass_transition_fixture_for_capture(id, COVER, WARM_COVER, cx)
    })??;
    let first = sound(window, TITLE, cx).await?;
    set_time(window, BASELINE_WALL + 500, cx)?;
    await_artwork(window, COVER, cx).await?;
    await_palette(window, COVER, cx).await?;
    let quality = window.update(cx, |view, window, cx| {
        view.fluid_glass_quality_for_capture(window, cx)
            .map(|(quality, _, _, _)| quality)
    })?;
    let context = Capture {
        window,
        scene,
        output,
        layout: if scene.width == 1920. {
            DESKTOP
        } else {
            COMPACT
        },
        digital_peak_expected: !scene.reduced
            && matches!(quality, Some("ultra" | "high" | "balanced")),
    };
    let baseline = context.render(0, Some("track-transition-before.png"), cx)?;
    require(
        u16::from(baseline.cover[2]) > u16::from(baseline.cover[0]) + 80,
        "Первый трек должен показать бирюзовую обложку до перехода.",
    )?;

    let next = SPECTRUM_TONES[1].0;
    let next_control = named(NEXT);
    press(raw, &next_control, cx)?;
    let second = sound(window, next, cx).await?;
    let tree = snapshot(raw, cx)?;
    require(
        focused(&tree) == Some(next_control.as_str())
            && node_label(&tree, "player-track") == Some(next)
            && node_label(&tree, "player-play") == Some("Пауза")
            && node(&tree, "player-progress").is_some()
            && node(&tree, QUEUE_LIST).is_none(),
        "Команда «Далее» должна оставить клавиатурный фокус и управление новым треком.",
    )?;
    let samples = if scene.reduced {
        reduced(&context, &baseline, cx).await?
    } else {
        animated(&context, &baseline, cx).await?
    };
    fs::write(
        output.join("track-transition.json"),
        serde_json::to_vec_pretty(&json!({
            "shader_phase_ms": SHADER_PHASE.as_millis(),
            "track_change_wall_ms": SWITCH_WALL,
            "reduced_motion": scene.reduced,
            "first": {"title": TITLE, "playback": first},
            "next": {"title": next, "playback": second},
            "focus_after_next": focused(&tree),
            "rim_region_exclusive_right_bottom": context.region(context.layout.rim),
            "light_region_exclusive_right_bottom": context.region(context.layout.light),
            "digital_artifact_probe": {
                "quality": quality,
                "peak_expected": context.digital_peak_expected,
                "regions_exclusive_right_bottom": context.layout.digital.iter().map(|&region| context.region(region)).collect::<Vec<_>>(),
                "clean_control_exclusive_right_bottom": context.region(context.layout.digital_control),
                "comparison": "new track at 0 ms",
            },
            "cover_center": context.layout.art_center.map(|dip| context.pixel(dip)),
            "expected_square_cover_device_pixels": [context.art_edge(), context.art_edge()],
            "palette_probes": (!scene.reduced).then_some(json!({
                "player_exclusive_right_bottom": context.region(PLAYER_PALETTE),
                "workspace_exclusive_right_bottom": context.region(WORKSPACE_PALETTE),
            })),
            "samples": samples,
        }))?,
    )?;
    println!(
        "{}: checked {} native track-transition samples",
        scene.name,
        samples.len()
    );
    Ok(())
}

async fn animated(
    capture: &Capture<'_>,
    before: &Observation,
    cx: &mut AsyncApp,
) -> gpui::Result<Vec<Value>> {
    let at_change = capture.render(0, Some("track-transition-start.png"), cx)?;
    let early = capture.render(90, None, cx)?;
    let onset = capture.render(120, None, cx)?;
    let peak = capture.render(160, Some("track-transition-pulse.png"), cx)?;
    await_artwork(capture.window, WARM_COVER, cx).await?;
    let cover_start = capture.render(250, None, cx)?;
    let cover_quarter = capture.render(310, Some("track-transition-cover-quarter.png"), cx)?;
    let cover_mid = capture.render(370, Some("track-transition-cover-mid.png"), cx)?;
    let quiet = capture.render(390, None, cx)?;
    let palette_start = capture.render(400, None, cx)?;
    await_palette(capture.window, WARM_COVER, cx).await?;
    let cover_three_quarters =
        capture.render(430, Some("track-transition-cover-three-quarters.png"), cx)?;
    let cover_end = capture.render(500, Some("track-transition-cover-settled.png"), cx)?;
    let palette_early = capture.render(640, Some("track-transition-palette-early.png"), cx)?;
    let palette_mid = capture.render(1200, Some("track-transition-palette-mid.png"), cx)?;
    let end = capture.render(2000, Some("track-transition-settled.png"), cx)?;
    let later = capture.render(2200, None, cx)?;

    println!(
        "Track transition rim changes at 90, 120, 160, 390ms {:?}; covers {:?}, palette changes {:?}",
        [&early, &onset, &peak, &quiet].map(|sample| changed(&before.rim, &sample.rim)),
        [before, &at_change, &cover_start, &cover_mid, &cover_end].map(|sample| sample.cover),
        [&quiet, &palette_start, &palette_mid, &end, &later]
            .map(|sample| changed(&before.light, &sample.light)),
    );
    require(
        changed(&before.rim, &early.rim) == 0
            && changed(&before.rim, &onset.rim) > 0
            && changed(&before.rim, &peak.rim) > 100
            && changed(&before.rim, &quiet.rim) == 0,
        "Оптический импульс должен появляться после 90 мс, быть видимым в 160 мс и закончиться к 390 мс.",
    )?;
    require(
        before.cover == at_change.cover
            && before.cover == cover_start.cover
            && between(before.cover[0], cover_mid.cover[0], cover_end.cover[0])
            && between(cover_end.cover[2], cover_mid.cover[2], before.cover[2])
            && capture.warm_square(&cover_end)
            && cover_end.cover == end.cover,
        &format!(
            "Обложка должна сохранять старый кадр до 250 мс и к 500 мс плавно смениться квадратом {} × {} px. Измерено: {:?}.",
            capture.art_edge(),
            capture.art_edge(),
            cover_end.warm_extent
        ),
    )?;
    require(
        before.labels == at_change.labels
            && before.labels == cover_start.labels
            && cover_quarter.labels.len() == 2
            && cover_mid.labels.len() == 2
            && cover_mid
                .labels
                .iter()
                .all(|(_, _, weight)| (*weight - 0.5).abs() < 0.001)
            && cover_three_quarters.labels.len() == 2
            && cover_end.labels.len() == 1
            && cover_end.labels[0].0 == SPECTRUM_TONES[1].0,
        "Название и исполнитель должны пройти через те же фазы, что и обложка.",
    )?;
    require(
        changed(&before.player_light, &quiet.player_light) == 0
            && changed(&before.player_light, &palette_start.player_light) == 0
            && changed(&before.player_light, &palette_mid.player_light) > 100
            && changed(&palette_mid.player_light, &end.player_light) > 100
            && changed(&end.player_light, &later.player_light) == 0,
        "Палитра плеера должна дать промежуточный кадр и завершить переход к 2000 мс.",
    )?;
    let player_progress = [&palette_early, &palette_mid].map(|sample| {
        rgb_progress(
            &before.player_light,
            &sample.player_light,
            &end.player_light,
        )
    });
    require(
        matches!(player_progress, [Some(early), Some(mid)] if 0.0 < early && early < mid && mid < 1.0),
        &format!(
            "Палитра плеера должна продвигаться к целевому цвету на 640 и 1200 мс. Измерено: {player_progress:.3?}."
        ),
    )?;
    require(
        [&palette_early, &palette_mid, &end]
            .into_iter()
            .all(|sample| {
                !sample.workspace_palette.is_empty()
                    && sample.workspace_palette == before.workspace_palette
            }),
        "Глубина рабочей области должна сохранять палитру маршрута при смене трека.",
    )?;
    let digital = check_digital(capture, &at_change, [&early, &peak, &quiet])?;
    println!(
        "Track transition artifact pixels at 90, 160, 390ms {:?}; clean control {:?}; peak max RGB delta {}, opposing red/blue {}",
        digital.map(|sample| sample.changed),
        digital.map(|sample| sample.control_changed),
        digital[1].max_rgb_delta,
        digital[1].opposing_red_blue,
    );
    Ok([
        (0, &at_change),
        (90, &early),
        (120, &onset),
        (160, &peak),
        (250, &cover_start),
        (310, &cover_quarter),
        (370, &cover_mid),
        (390, &quiet),
        (400, &palette_start),
        (430, &cover_three_quarters),
        (500, &cover_end),
        (640, &palette_early),
        (1200, &palette_mid),
        (2000, &end),
        (2200, &later),
    ]
    .into_iter()
    .map(|(ms, sample)| sample.record(ms, before, Some(&end), (ms <= 400).then_some(&at_change)))
    .collect())
}

async fn reduced(
    capture: &Capture<'_>,
    before: &Observation,
    cx: &mut AsyncApp,
) -> gpui::Result<Vec<Value>> {
    await_artwork(capture.window, WARM_COVER, cx).await?;
    await_palette(capture.window, WARM_COVER, cx).await?;
    let immediate = capture.render(0, Some("track-transition-reduced.png"), cx)?;
    let peak_time = capture.render(160, None, cx)?;
    let settled_time = capture.render(2000, None, cx)?;
    require(
        capture.warm_square(&immediate)
            && immediate.labels.len() == 1
            && immediate.labels[0].0 == SPECTRUM_TONES[1].0
            && changed(&before.player_light, &immediate.player_light) > 100
            && [&peak_time, &settled_time].into_iter().all(|sample| {
                sample.cover == immediate.cover
                    && changed(&sample.rim, &immediate.rim) == 0
                    && changed(&sample.light, &immediate.light) == 0
                    && changed(&sample.player_light, &immediate.player_light) == 0
                    && sample.digital_delta(&immediate).is_clear()
            }),
        &format!(
            "Уменьшенное движение должно сразу применить квадратную обложку {} × {} px и палитру. Измерено: {:?}.",
            capture.art_edge(),
            capture.art_edge(),
            immediate.warm_extent
        ),
    )?;
    Ok([(0, &immediate), (160, &peak_time), (2000, &settled_time)]
        .into_iter()
        .map(|(ms, sample)| sample.record(ms, before, None, Some(&immediate)))
        .collect())
}

fn check_digital(
    capture: &Capture<'_>,
    before: &Observation,
    samples: [&Observation; 3],
) -> gpui::Result<[DigitalDelta; 3]> {
    let deltas = samples.map(|sample| sample.digital_delta(before));
    let [early, peak, quiet] = deltas;
    let peak_matches_profile = if capture.digital_peak_expected {
        peak.changed > 100 && peak.max_rgb_delta >= 2
    } else {
        peak.changed == 0
    };
    require(
        early.is_clear() && quiet.is_clear() && peak.control_changed == 0 && peak_matches_profile,
        &format!(
            "Локальный сбой должен появиться только на пике импульса при подходящем профиле. Кадры 90, 160 и 390 мс: {deltas:?}."
        ),
    )?;
    Ok(deltas)
}

fn between(low: u8, value: u8, high: u8) -> bool {
    value.saturating_sub(low) >= 10 && high.saturating_sub(value) >= 10
}

pub(super) async fn playlist_id(raw: AnyWindowHandle, cx: &mut AsyncApp) -> gpui::Result<i64> {
    press(raw, &named("home-playlists"), cx)?;
    let id = wait(
        cx,
        "доступная строка тестового плейлиста",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(tree["nodes"].as_object().and_then(|nodes| {
                nodes.values().find_map(|node| {
                    node["element_id"]
                        .as_str()?
                        .strip_prefix("Name(\"local-playlist-")?
                        .strip_suffix("\")")?
                        .parse::<i64>()
                        .ok()
                })
            }))
        },
    )
    .await?;
    press(raw, &named("studio-library"), cx)?;
    wait(
        cx,
        "возврат к трекам перед проверкой перехода",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(queue_candidate(&tree).is_some().then_some(()))
        },
    )
    .await?;
    Ok(id)
}

async fn await_artwork(
    window: WindowHandle<MainView>,
    url: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    wait(
        cx,
        "декодирование обложки плеера",
        |cx| {
            snapshot(window.into(), cx)?;
            window.update(cx, |view, _, cx| {
                view.artist_artwork_decoded_for_capture(url, cx)
                    .then_some(())
            })
        },
    )
    .await
}

async fn await_palette(
    window: WindowHandle<MainView>,
    url: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    wait(cx, "палитра обложки плеера", |cx| {
        snapshot(window.into(), cx)?;
        window.update(cx, |view, _, cx| {
            view.fluid_glass_palette_ready_for_capture(url, cx)
                .then_some(())
        })
    })
    .await
}

fn set_time(window: WindowHandle<MainView>, wall: u64, cx: &mut AsyncApp) -> gpui::Result<()> {
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_track_change_for_capture(
            Duration::from_millis(wall),
            SHADER_PHASE,
            cx,
        );
    })
}

struct Capture<'a> {
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &'a Path,
    layout: ProbeLayout,
    digital_peak_expected: bool,
}

impl Capture<'_> {
    fn pixel(&self, dip: u32) -> u32 {
        (dip * self.scene.dpi + 48) / 96
    }

    fn region(&self, region: Region) -> Region {
        region.map(|dip| self.pixel(dip))
    }

    fn art_edge(&self) -> u32 {
        self.pixel(self.layout.art_edge)
    }

    fn warm_square(&self, sample: &Observation) -> bool {
        let edge = self.art_edge();
        let tolerance = u32::from(!self.scene.dpi.is_multiple_of(96));
        sample.cover[0] >= 220
            && sample.cover[2] <= 60
            && sample.warm_extent.is_some_and(|[width, height]| {
                width.abs_diff(edge) <= tolerance
                    && height.abs_diff(edge) <= tolerance
                    && width.abs_diff(height) <= tolerance
            })
    }

    fn render(
        &self,
        elapsed: u64,
        artifact: Option<&str>,
        cx: &mut AsyncApp,
    ) -> gpui::Result<Observation> {
        set_time(self.window, SWITCH_WALL + elapsed, cx)?;
        let image = render_image(self.window.into(), self.scene, cx)?;
        if let Some(name) = artifact {
            image.save(self.output.join(name))?;
        }
        let [x, y] = self.layout.art_center.map(|dip| self.pixel(dip));
        Ok(Observation {
            rim: crop_pixels(&image, self.region(self.layout.rim)),
            light: crop_pixels(&image, self.region(self.layout.light)),
            digital: self
                .layout
                .digital
                .iter()
                .flat_map(|&region| crop_pixels(&image, self.region(region)))
                .collect(),
            digital_control: crop_pixels(&image, self.region(self.layout.digital_control)),
            player_light: crop_pixels(
                &image,
                self.region(if self.scene.width == 1920. {
                    PLAYER_PALETTE
                } else {
                    COMPACT_PLAYER_PALETTE
                }),
            ),
            workspace_palette: if self.scene.width == 1920. {
                crop_pixels(&image, self.region(WORKSPACE_PALETTE))
            } else {
                Vec::new()
            },
            cover: image.get_pixel(x, y).0,
            labels: self
                .window
                .update(cx, |view, _, cx| view.fluid_track_labels_for_capture(cx))?,
            warm_extent: warm_extent(&image, self.region(self.layout.art)),
        })
    }
}

struct Observation {
    rim: Vec<Pixel>,
    light: Vec<Pixel>,
    digital: Vec<Pixel>,
    digital_control: Vec<Pixel>,
    player_light: Vec<Pixel>,
    workspace_palette: Vec<Pixel>,
    cover: Pixel,
    labels: Vec<(String, String, f32)>,
    warm_extent: Option<[u32; 2]>,
}

impl Observation {
    fn record(
        &self,
        ms: u64,
        baseline: &Self,
        palette_end: Option<&Self>,
        digital_reference: Option<&Self>,
    ) -> Value {
        let palette = palette_end.and_then(|end| {
            rgb_progress(
                &baseline.player_light,
                &self.player_light,
                &end.player_light,
            )
        });
        let digital = digital_reference.map(|reference| self.digital_delta(reference));
        json!({
            "ms": ms,
            "rim_pixels_changed_from_before": changed(&self.rim, &baseline.rim),
            "light_pixels_changed_from_before": changed(&self.light, &baseline.light),
            "cover_center_rgba": self.cover,
            "labels": self.labels,
            "warm_cover_extent": self.warm_extent,
            "player_palette_projected_rgb_progress": palette,
            "workspace_palette_pixels_changed_from_before": (!self.workspace_palette.is_empty())
                .then(|| changed(&baseline.workspace_palette, &self.workspace_palette)),
            "digital_artifact": digital.map(|digital| json!({
                "changed_pixels": digital.changed,
                "clean_control_changed_pixels": digital.control_changed,
                "maximum_rgb_channel_delta": digital.max_rgb_delta,
                "pixels_with_opposing_red_blue_deltas": digital.opposing_red_blue,
            })),
        })
    }

    fn digital_delta(&self, reference: &Self) -> DigitalDelta {
        let mut delta = DigitalDelta {
            control_changed: changed(&self.digital_control, &reference.digital_control),
            ..Default::default()
        };
        for (current, initial) in self.digital.iter().zip(&reference.digital) {
            if current == initial {
                continue;
            }
            let [red, green, blue, _] =
                std::array::from_fn(|i| i16::from(current[i]) - i16::from(initial[i]));
            delta.changed += 1;
            let max_rgb_delta = red
                .unsigned_abs()
                .max(green.unsigned_abs())
                .max(blue.unsigned_abs());
            delta.max_rgb_delta = delta.max_rgb_delta.max(max_rgb_delta as u8);
            delta.opposing_red_blue += usize::from(red != 0 && red.signum() == -blue.signum());
        }
        delta
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct DigitalDelta {
    changed: usize,
    control_changed: usize,
    max_rgb_delta: u8,
    opposing_red_blue: usize,
}

impl DigitalDelta {
    fn is_clear(self) -> bool {
        self.changed == 0 && self.control_changed == 0
    }
}

fn crop_pixels(image: &image::RgbaImage, [left, top, right, bottom]: Region) -> Vec<Pixel> {
    (top..bottom)
        .flat_map(|y| (left..right).map(move |x| image.get_pixel(x, y).0))
        .collect()
}

fn changed(before: &[Pixel], after: &[Pixel]) -> usize {
    before
        .iter()
        .zip(after)
        .filter(|(before, after)| before != after)
        .count()
}

fn rgb_progress(initial: &[Pixel], sample: &[Pixel], target: &[Pixel]) -> Option<f64> {
    let mut dot_product = 0.0;
    let mut target_squared = 0.0;
    for ((before, current), settled) in initial.iter().zip(sample).zip(target) {
        for channel in 0..3 {
            let completed = f64::from(settled[channel]) - f64::from(before[channel]);
            let current = f64::from(current[channel]) - f64::from(before[channel]);
            dot_product += current * completed;
            target_squared += completed * completed;
        }
    }
    (target_squared > 0.0).then(|| dot_product / target_squared)
}

fn warm_extent(image: &image::RgbaImage, [left, top, right, bottom]: Region) -> Option<[u32; 2]> {
    let mut bounds = None;
    for (x, y) in (top..bottom).flat_map(|y| (left..right).map(move |x| (x, y))) {
        let [r, g, b, _] = image.get_pixel(x, y).0;
        if r > 190 && (62..150).contains(&g) && b < 75 {
            let [min_x, min_y, max_x, max_y] = bounds.unwrap_or([x, y, x, y]);
            bounds = Some([min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)]);
        }
    }
    bounds.map(|[min_x, min_y, max_x, max_y]| [max_x - min_x + 1, max_y - min_y + 1])
}
