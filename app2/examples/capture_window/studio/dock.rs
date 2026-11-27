use std::{fs, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::super::render_image;
use super::{
    QUEUE_BUTTON, QUEUE_LIST, Scene, focus, focused, key, named, node, node_label, press,
    queue_candidate, require, snapshot,
};

const START: u64 = 40_000;
const OPEN_MS: u64 = 350;
const CLOSE_MS: u64 = 350;
const PANEL: &str = "right-panel";
const CLOSE_BUTTON: &str = "right-panel-close";
type Region = [u32; 4];
type Pixel = [u8; 4];

// Both routes leave these areas free of rows, headings or controls.
const WIDE_MATERIAL: Region = [1610, 640, 1890, 760];
const WIDE_OUTSIDE: Region = [1220, 650, 1460, 750];
const WIDE_CLOSE_GLYPH: Region = [1865, 88, 1881, 104];
const WIDE_JOIN_PROBES: [u32; 3] = [1660, 1840, 1880];
const WIDE_JOIN_SCAN: std::ops::Range<u32> = 954..966;
const NARROW_MATERIAL: Region = [530, 269, 735, 314];
const NARROW_OUTSIDE: Region = [12, 192, 26, 245];
const NARROW_CLOSE_GLYPH: Region = [757, 76, 773, 92];

pub(super) fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let layout = if scene.reduced {
        [NARROW_MATERIAL, NARROW_OUTSIDE, NARROW_CLOSE_GLYPH]
    } else {
        [WIDE_MATERIAL, WIDE_OUTSIDE, WIDE_CLOSE_GLYPH]
    };
    let capture = Capture {
        window,
        scene,
        output,
        layout,
    };
    let result = if scene.reduced {
        reduced(&capture, cx)
    } else {
        animated(&capture, cx)
    };
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_dock_for_capture(None, cx)
    })?;
    result
}

fn animated(capture: &Capture<'_>, cx: &mut AsyncApp) -> gpui::Result<()> {
    let closed = capture.frame_at(0, Some("dock-transition-closed.png"), cx)?;
    require(
        !closed.panel && closed.library_rows,
        "Тест стыка начинается с открытой медиатеки.",
    )?;
    press(capture.raw(), &named(QUEUE_BUTTON), cx)?;
    let open_start = capture.frame_at(0, Some("dock-transition-open-start.png"), cx)?;
    require(
        open_start.focused_on(PANEL),
        "Очередь сразу должна получить клавиатурный фокус.",
    )?;
    let open_quarter =
        capture.frame_at(OPEN_MS / 4, Some("dock-transition-open-quarter.png"), cx)?;
    focus(capture.raw(), &named(CLOSE_BUTTON), cx)?;
    let open_mid = capture.frame_at(OPEN_MS / 2, Some("dock-transition-open-mid.png"), cx)?;
    let open_three_quarters = capture.frame_at(
        OPEN_MS * 3 / 4,
        Some("dock-transition-open-three-quarters.png"),
        cx,
    )?;
    let open_end = capture.frame_at(OPEN_MS, Some("dock-transition-open-settled.png"), cx)?;
    require(
        [&open_quarter, &open_mid, &open_three_quarters, &open_end]
            .into_iter()
            .all(Observation::active_panel)
            && open_mid.focused_on(CLOSE_BUTTON)
            && open_end.focused_on(CLOSE_BUTTON)
            && open_mid.close_glyph.is_some()
            && open_mid.close_glyph == open_end.close_glyph,
        "Очередь и кнопка закрытия должны работать без сдвига во всех кадрах открытия.",
    )?;

    key(capture.raw(), "escape", cx)?;
    let close_start = capture.frame_at(OPEN_MS, Some("dock-transition-close-start.png"), cx)?;
    let close_mid = capture.frame_at(
        OPEN_MS + CLOSE_MS / 2,
        Some("dock-transition-close-mid.png"),
        cx,
    )?;
    let close_end = capture.frame_at(
        OPEN_MS + CLOSE_MS,
        Some("dock-transition-close-settled.png"),
        cx,
    )?;

    let reopen_at = OPEN_MS + CLOSE_MS + 100;
    capture.set_time(reopen_at, cx)?;
    press(capture.raw(), &named(QUEUE_BUTTON), cx)?;
    capture.frame_at(reopen_at, None, cx)?;
    let reverse_at = reopen_at + 80;
    let partial = capture.frame_at(reverse_at, None, cx)?;
    key(capture.raw(), "escape", cx)?;
    let reversed = capture.frame_at(reverse_at, None, cx)?;
    let reverse_end = capture.frame_at(reverse_at + CLOSE_MS, None, cx)?;

    let opening = [&open_start, &open_mid, &open_end]
        .map(|frame| material_progress(&closed.material, &frame.material, &open_end.material));
    let closing = [&close_start, &close_mid, &close_end]
        .map(|frame| material_progress(&closed.material, &frame.material, &open_end.material));
    let samples = [
        ("closed", 0, &closed),
        ("open_start", 0, &open_start),
        ("open_quarter", OPEN_MS / 4, &open_quarter),
        ("open_mid", OPEN_MS / 2, &open_mid),
        ("open_three_quarters", OPEN_MS * 3 / 4, &open_three_quarters),
        ("open_settled", OPEN_MS, &open_end),
        ("close_start", OPEN_MS, &close_start),
        ("close_mid", OPEN_MS + CLOSE_MS / 2, &close_mid),
        ("close_settled", OPEN_MS + CLOSE_MS, &close_end),
        ("before_reversal", reverse_at, &partial),
        ("after_reversal", reverse_at, &reversed),
        ("reversal_settled", reverse_at + CLOSE_MS, &reverse_end),
    ];
    capture.save_report(json!({
        "reduced_motion": false,
        "open_material_progress": opening,
        "close_material_progress": closing,
        "reversal_material_pixels_changed": changed(&partial.material, &reversed.material),
        "old_player_edge_closed_rgb_steps": closed.join_steps,
        "old_player_edge_open_rgb_steps": open_end.join_steps,
        "samples": samples.map(|(name, ms, frame)| frame.record(name, ms, &closed)),
    }))?;
    println!("Dock material opening {opening:.3?}, closing {closing:.3?}");
    require(
        closed.join_steps.into_iter().any(|step| step > 14)
            && open_end.join_steps.into_iter().all(|step| step <= 12),
        "При открытии панель и плеер должны образовать один контур без поперечной кромки.",
    )?;
    require(
        matches!(opening, [Some(start), Some(middle), Some(end)]
            if start <= 0.01 && start + 0.01 < middle && middle <= end + 0.03)
            && matches!(closing, [Some(start), Some(middle), Some(end)]
                if start == 1.0 && 0.02 < middle && middle < 0.98 && end == 0.0)
            && open_end.material == close_start.material
            && close_end.material == closed.material
            && samples
                .iter()
                .all(|(_, _, sample)| sample.outside == closed.outside),
        "Стекло очереди должно пройти обе фазы, оставив фон за пределами панели неизменным.",
    )?;
    require(
        open_start.dock_width == 0.
            && open_quarter.dock_width > open_start.dock_width
            && open_mid.dock_width > open_quarter.dock_width
            && open_three_quarters.dock_width > open_mid.dock_width
            && open_end.dock_width > open_three_quarters.dock_width
            && close_start.dock_width == open_end.dock_width
            && close_end.dock_width == 0.
            && partial.dock_width == reversed.dock_width,
        "Ширина панели должна раскрываться и закрываться синхронно с материалом.",
    )?;
    require(
        partial.active_panel()
            && [
                &close_start,
                &close_mid,
                &close_end,
                &reversed,
                &reverse_end,
            ]
            .into_iter()
            .all(Observation::returned_to_library)
            && partial.material == reversed.material
            && reverse_end.material == closed.material,
        "Закрытие должно сразу убрать очередь из навигации, вернуть фокус и сохранить непрерывность стекла.",
    )
}

fn reduced(capture: &Capture<'_>, cx: &mut AsyncApp) -> gpui::Result<()> {
    let closed = capture.frame_at(0, None, cx)?;
    require(
        !closed.panel && closed.library_rows,
        "Компактная проверка начинается с медиатеки.",
    )?;
    press(capture.raw(), &named(QUEUE_BUTTON), cx)?;
    let opened = capture.frame_at(0, Some("dock-transition-reduced-open.png"), cx)?;
    let later = capture.frame_at(OPEN_MS, None, cx)?;
    key(capture.raw(), "escape", cx)?;
    let returned = capture.frame_at(OPEN_MS, Some("dock-transition-reduced-closed.png"), cx)?;
    let settled = capture.frame_at(OPEN_MS + CLOSE_MS, None, cx)?;
    capture.save_report(json!({
        "reduced_motion": true,
        "timed_open_pixels_changed": changed(&opened.material, &later.material),
        "timed_close_pixels_changed": changed(&returned.material, &settled.material),
        "samples": [
            closed.record("closed", 0, &closed),
            opened.record("open_immediate", 0, &closed),
            later.record("open_later", OPEN_MS, &closed),
            returned.record("close_immediate", OPEN_MS, &closed),
            settled.record("close_later", OPEN_MS + CLOSE_MS, &closed),
        ],
    }))?;
    require(
        opened.active_panel()
            && opened.focused_on(PANEL)
            && !opened.library_rows
            && later.active_panel()
            && opened.material == later.material
            && [&returned, &settled]
                .into_iter()
                .all(Observation::returned_to_library)
            && returned.material == closed.material
            && settled.material == closed.material,
        "Компактная очередь при уменьшенном движении должна сразу заменить и вернуть страницу.",
    )
}

struct Capture<'a> {
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &'a Path,
    layout: [Region; 3],
}

impl Capture<'_> {
    fn raw(&self) -> AnyWindowHandle {
        self.window.into()
    }

    fn set_time(&self, millis: u64, cx: &mut AsyncApp) -> gpui::Result<()> {
        self.window.update(cx, |view, _, cx| {
            view.sample_fluid_glass_dock_for_capture(
                Some(Duration::from_millis(START + millis)),
                cx,
            );
        })
    }

    fn frame_at(
        &self,
        millis: u64,
        name: Option<&str>,
        cx: &mut AsyncApp,
    ) -> gpui::Result<Observation> {
        self.set_time(millis, cx)?;
        let tree = snapshot(self.raw(), cx)?;
        let image = render_image(self.raw(), self.scene, cx)?;
        let dock_width = self.window.update(cx, |view, window, _| {
            view.dock_splitter_for_capture(window).0
        })?;
        if let Some(name) = name {
            image.save(self.output.join(name))?;
        }
        let [material, outside, close] = self.layout;
        let panel = node(&tree, PANEL).is_some();
        let queue = node_label(&tree, QUEUE_LIST) == Some("Треки в очереди");
        let close_button = node(&tree, CLOSE_BUTTON).is_some_and(|button| {
            button["aria"]["on_action"]
                .as_array()
                .is_some_and(|actions| actions.iter().any(|action| action == "Click"))
        });
        let current_title = node_label(&tree, "player-track");
        let mut tracks = tree["nodes"]
            .as_object()
            .into_iter()
            .flat_map(|nodes| nodes.values())
            .filter_map(|node| {
                let aria = &node["aria"];
                if aria["role"] != "ListBoxOption" {
                    return None;
                }
                Some((aria["selected"].as_bool()?, aria["label"].as_str()?))
            });
        let selected = tracks.clone().any(|(selected, label)| {
            selected && current_title.is_some_and(|title| label.contains(title))
        });
        let has_next = tracks.any(|(selected, _)| !selected);
        Ok(Observation {
            dock_width,
            material: pixels(&image, material),
            outside: pixels(&image, outside),
            join_steps: if self.scene.reduced {
                [0; 3]
            } else {
                WIDE_JOIN_PROBES.map(|x| vertical_step(&image, x))
            },
            close_glyph: panel.then(|| bright_bounds(&image, close)).flatten(),
            focus: focused(&tree).map(str::to_owned),
            panel,
            queue,
            close_button,
            selected,
            has_next,
            library_rows: queue_candidate(&tree).is_some(),
        })
    }

    fn save_report(&self, mut report: Value) -> gpui::Result<()> {
        report["sample_regions_exclusive_right_bottom"] = json!({
            "material": self.layout[0],
            "outside": self.layout[1],
            "close_glyph": self.layout[2],
        });
        fs::write(
            self.output.join("dock-transition.json"),
            serde_json::to_vec_pretty(&report)?,
        )?;
        Ok(())
    }
}

struct Observation {
    dock_width: f32,
    material: Vec<Pixel>,
    outside: Vec<Pixel>,
    join_steps: [u8; 3],
    close_glyph: Option<Region>,
    focus: Option<String>,
    panel: bool,
    queue: bool,
    close_button: bool,
    selected: bool,
    has_next: bool,
    library_rows: bool,
}

impl Observation {
    fn focused_on(&self, name: &str) -> bool {
        self.focus.as_deref() == Some(named(name).as_str())
    }

    fn active_panel(&self) -> bool {
        self.panel && self.queue && self.close_button && self.selected && self.has_next
    }

    fn returned_to_library(&self) -> bool {
        !self.panel
            && !self.queue
            && !self.close_button
            && self.library_rows
            && self.focused_on(QUEUE_BUTTON)
    }

    fn record(&self, name: &str, ms: u64, closed: &Self) -> Value {
        json!({
            "state": name,
            "ms": ms,
            "dock_width_dip": self.dock_width,
            "panel_mounted": self.panel,
            "queue_available": self.queue,
            "close_action_available": self.close_button,
            "selected_track": self.selected,
            "following_track": self.has_next,
            "library_rows": self.library_rows,
            "focus": self.focus,
            "close_glyph_bounds_inclusive": self.close_glyph,
            "material_pixels_changed_from_closed": changed(&self.material, &closed.material),
            "outside_pixels_changed_from_closed": changed(&self.outside, &closed.outside),
        })
    }
}

fn material_progress(closed: &[Pixel], sample: &[Pixel], open: &[Pixel]) -> Option<f64> {
    let magnitude = |a: &[Pixel], b: &[Pixel]| {
        a.iter()
            .zip(b)
            .map(|(left, right)| {
                (0..3)
                    .map(|channel| u64::from(left[channel].abs_diff(right[channel])))
                    .sum::<u64>()
            })
            .sum::<u64>()
    };
    let full = magnitude(closed, open);
    (full > 0).then(|| magnitude(closed, sample) as f64 / full as f64)
}

fn changed(a: &[Pixel], b: &[Pixel]) -> usize {
    a.iter().zip(b).filter(|(a, b)| a != b).count()
}

fn pixels(image: &image::RgbaImage, [left, top, right, bottom]: Region) -> Vec<Pixel> {
    (top..bottom)
        .flat_map(|y| (left..right).map(move |x| image.get_pixel(x, y).0))
        .collect()
}

fn vertical_step(image: &image::RgbaImage, x: u32) -> u8 {
    WIDE_JOIN_SCAN
        .flat_map(|y| {
            let above = image.get_pixel(x, y).0;
            let below = image.get_pixel(x, y + 1).0;
            [0, 1, 2].map(|i| above[i].abs_diff(below[i]))
        })
        .max()
        .unwrap_or_default()
}

fn bright_bounds(image: &image::RgbaImage, [left, top, right, bottom]: Region) -> Option<Region> {
    let mut bounds = None;
    for (x, y) in (top..bottom).flat_map(|y| (left..right).map(move |x| (x, y))) {
        let [r, g, b, _] = image.get_pixel(x, y).0;
        if r.min(g).min(b) > 178 {
            let [min_x, min_y, max_x, max_y] = bounds.unwrap_or([x, y, x, y]);
            bounds = Some([min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)]);
        }
    }
    bounds
}
