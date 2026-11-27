use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::super::render_image;
use super::{
    QUEUE_BUTTON, QUEUE_LIST, Scene, key, named, node, node_label, press, require, snapshot, wait,
};

const PLAY: &str = "player-play";
const WALL: Duration = Duration::from_secs(60);
const PHASE: Duration = Duration::from_millis(4600);
const PAIRS: usize = 5;

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    let tree = snapshot(raw, cx)?;
    if node(&tree, QUEUE_LIST).is_some() {
        key(raw, "escape", cx)?;
    }
    if node_label(&tree, PLAY) == Some("Пауза") {
        press(raw, &named(PLAY), cx)?;
    }
    wait(
        cx,
        "неподвижная позиция плеера и закрытая очередь",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok((node(&tree, QUEUE_LIST).is_none()
                && node_label(&tree, PLAY) == Some("Воспроизвести"))
            .then_some(()))
        },
    )
    .await?;
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_track_change_for_capture(WALL, PHASE, cx);
        view.sample_fluid_glass_dock_for_capture(None, cx);
    })?;
    let capture = Capture {
        window,
        scene,
        output,
    };
    let closed = capture.compare("closed", cx)?;
    let mut cases = vec![closed];
    if !scene.reduced && scene.width == 1920. {
        press(raw, &named(QUEUE_BUTTON), cx)?;
        require(
            node_label(&snapshot(raw, cx)?, QUEUE_LIST) == Some("Треки в очереди"),
            "Для второго сравнения требуется пристыкованная очередь.",
        )?;
        let docked = capture.compare("queue", cx)?;
        require(
            docked["edge_device_pixels"].as_u64() > cases[0]["edge_device_pixels"].as_u64(),
            "С открытой очередью нужно отрисовать и её оптическую кромку.",
        )?;
        cases.push(docked);
        key(raw, "escape", cx)?;
    }
    fs::write(
        output.join("optical-edge.json"),
        serde_json::to_vec_pretty(&json!({
            "shader_phase_ms": PHASE.as_millis(),
            "reduced_motion": scene.reduced,
            "cases": cases,
            "timing": "Interleaved forced native GPUI frame wall time with rasterization and GPU readback, without PNG encoding or presentation.",
        }))?,
    )?;
    require(
        cases.iter().all(|case| case["different_rgba_pixels"] == 0),
        "Полноэкранная и ограниченная полосами отрисовка оптики должны дать побайтово одинаковые кадры.",
    )
}

struct Capture<'a> {
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &'a Path,
}

impl Capture<'_> {
    fn compare(&self, state: &str, cx: &mut AsyncApp) -> gpui::Result<Value> {
        let (strips, pixels, viewport) = self.window.update(cx, |view, window, cx| {
            view.fluid_glass_edge_work_for_capture(window, cx)
        })?;
        require(
            strips > 1 && pixels > 0 && pixels < viewport,
            "Оптический проход должен покрывать положительную площадь меньше окна.",
        )?;
        let optimized_name = format!("optical-edge-{state}-strips.png");
        let reference_name = format!("optical-edge-{state}-full.png");
        let result = self.compare_rasters(state, &optimized_name, &reference_name, cx);
        self.set_full(false, cx)?;
        let (difference, times) = result?;
        let medians = times.map(median);
        println!(
            "{} {state}: edge rasterized {pixels}/{viewport} pixels in {strips} strips, {} RGBA pixel differences, offscreen medians {:.2}/{:.2} ms (strips/full)",
            self.scene.name, difference.count, medians[0], medians[1],
        );
        Ok(json!({
            "state": state,
            "edge_strips": strips,
            "edge_device_pixels": pixels,
            "full_viewport_device_pixels": viewport,
            "edge_pixel_fraction": pixels as f64 / viewport as f64,
            "reference_frame": reference_name,
            "optimized_frame": optimized_name,
            "different_rgba_pixels": difference.count,
            "maximum_channel_delta": difference.maximum_delta,
            "changed_bounds_inclusive": difference.bounds,
            "forced_frame_wall_ms": {"strips": times[0], "full": times[1]},
            "median_wall_ms": {"strips": medians[0], "full": medians[1]},
        }))
    }

    fn compare_rasters(
        &self,
        state: &str,
        optimized_name: &str,
        reference_name: &str,
        cx: &mut AsyncApp,
    ) -> gpui::Result<(Difference, [[f64; PAIRS]; 2])> {
        // Warm both code paths before recording any wall times.
        self.frame(false, cx)?;
        self.frame(true, cx)?;
        let optimized = self.frame(false, cx)?;
        let reference = self.frame(true, cx)?;
        let difference = diff_pixels(&optimized, &reference);
        let mut pairs = [[0.0; 2]; PAIRS];
        for (pair, times) in pairs.iter_mut().enumerate() {
            let order = if pair % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            };
            for full in order {
                self.set_full(full, cx)?;
                let start = Instant::now();
                render_image(self.window.into(), self.scene, cx)?;
                times[usize::from(full)] = start.elapsed().as_secs_f64() * 1000.;
            }
        }
        optimized.save(self.output.join(optimized_name))?;
        reference.save(self.output.join(reference_name))?;
        if let Some(bounds) = difference.bounds {
            println!(
                "{} {state}: changed frame bounds {bounds:?}, maximum RGB/A delta {}",
                self.scene.name, difference.maximum_delta
            );
        }
        Ok((
            difference,
            std::array::from_fn(|mode| pairs.map(|times| times[mode])),
        ))
    }

    fn frame(&self, full: bool, cx: &mut AsyncApp) -> gpui::Result<image::RgbaImage> {
        self.set_full(full, cx)?;
        render_image(self.window.into(), self.scene, cx)
    }

    fn set_full(&self, full: bool, cx: &mut AsyncApp) -> gpui::Result<()> {
        self.window.update(cx, |view, _, cx| {
            view.set_fluid_glass_full_edge_for_capture(full, cx);
        })
    }
}

fn median(mut values: [f64; PAIRS]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[PAIRS / 2]
}

#[derive(Default)]
struct Difference {
    count: usize,
    maximum_delta: u8,
    bounds: Option<[u32; 4]>,
}

fn diff_pixels(optimized: &image::RgbaImage, reference: &image::RgbaImage) -> Difference {
    let mut diff = Difference::default();
    for ((x, y, actual), expected) in optimized.enumerate_pixels().zip(reference.pixels()) {
        if actual != expected {
            diff.count += 1;
            let [min_x, min_y, max_x, max_y] = diff.bounds.unwrap_or([x, y, x, y]);
            diff.bounds = Some([min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)]);
            diff.maximum_delta = diff.maximum_delta.max(
                actual
                    .0
                    .into_iter()
                    .zip(expected.0)
                    .map(|(a, b)| a.abs_diff(b))
                    .max()
                    .unwrap_or_default(),
            );
        }
    }
    diff
}
