use std::{
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, Pixels, PlatformInput, Point, WindowHandle, point, px,
};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::super::render_image;
use super::{
    Scene, focus, focused, key, luminance, named, node, node_label, press, require, snapshot, wait,
};

const PLAY: &str = "player-play";
const PAUSE_LABEL: &str = "Пауза";
const PLAY_LABEL: &str = "Воспроизвести";
const SEEK: &str = "player-progress";
const VOLUME: &str = "player-volume";
type Region = [u32; 4];

struct Probe {
    target: [f32; 2],
    outside: [f32; 2],
    fill: Region,
    control: Region,
}

const WIDE: Probe = Probe {
    target: [960., 1000.],
    outside: [1500., 700.],
    fill: [945, 994, 950, 1006],
    control: [934, 974, 986, 1026],
};
const COMPACT: Probe = Probe {
    target: [400., 352.],
    outside: [600., 109.],
    fill: [389, 346, 393, 357],
    control: [377, 330, 422, 373],
};

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let probe = if scene.reduced { COMPACT } else { WIDE };
    let capture = Capture {
        window,
        scene,
        probe,
    };
    let raw = capture.raw();
    let target = pointer(capture.probe.target);
    let outside = pointer(capture.probe.outside);
    let idle_transport = capture.transport(cx)?;
    require(
        idle_transport.label == PLAY_LABEL,
        "Тест кнопки начинается на паузе.",
    )?;
    events(raw, [mouse_move(outside, false)], cx)?;
    let idle = capture.frame(cx)?;

    let hover_started = Instant::now();
    events(raw, [mouse_move(target, false)], cx)?;
    delay(cx, 45).await;
    let hover_sample_ms = hover_started.elapsed().as_secs_f64() * 1000.;
    let hover_sample = capture.frame(cx)?;
    delay(cx, 165).await;
    let hover = capture.frame(cx)?;

    events(raw, [mouse_down(target)], cx)?;
    delay(cx, 120).await;
    let pressed = capture.frame(cx)?;
    let held_transport = capture.transport(cx)?;

    events(raw, [mouse_move(outside, true), mouse_up(outside)], cx)?;
    delay(cx, 170).await;
    let released = capture.frame(cx)?;
    let released_transport = capture.transport(cx)?;

    events(
        raw,
        [
            mouse_move(target, false),
            mouse_down(target),
            mouse_up(target),
        ],
        cx,
    )?;
    let clicked = capture
        .await_transport(PAUSE_LABEL, Some(released_transport.position), cx)
        .await?;
    let playing = capture.frame(cx)?;
    events(
        raw,
        [
            mouse_move(target, false),
            mouse_down(target),
            mouse_up(target),
        ],
        cx,
    )?;
    delay(cx, 90).await;
    events(raw, [mouse_down(target), mouse_up(target)], cx)?;
    let double_clicked = wait(cx, "два быстрых клика Play/Pause", |cx| {
        let state = capture.transport(cx)?;
        Ok((state.label == PAUSE_LABEL && state.track == clicked.track).then_some(state))
    })
    .await?;
    events(raw, [mouse_move(outside, false)], cx)?;

    press(raw, &named(PLAY), cx)?;
    let keyboard_paused = capture.await_transport(PLAY_LABEL, None, cx).await?;
    let keyboard_focus = capture.frame(cx)?;
    key(raw, "enter", cx)?;
    let keyboard_playing = capture
        .await_transport(PAUSE_LABEL, Some(keyboard_paused.position), cx)
        .await?;
    press(raw, &named(PLAY), cx)?;
    capture.await_transport(PLAY_LABEL, None, cx).await?;

    let early_luma = hover_sample.luma;
    let hover_intermediate = idle.luma + 0.001 < early_luma && early_luma < hover.luma - 0.001;
    let fixed_footprint = idle.bounds.is_some()
        && [&hover, &pressed, &released]
            .into_iter()
            .all(|stage| stage.bounds == idle.bounds);
    let frames = [
        ("idle", &idle),
        ("hover-early", &hover_sample),
        ("hover-settled", &hover),
        ("pressed", &pressed),
        ("release-outside", &released),
        ("clicked", &playing),
        ("keyboard-focus", &keyboard_focus),
    ];
    for (name, frame) in frames {
        frame
            .image
            .save(output.join(format!("control-feedback-{name}.png")))?;
    }
    let neighbor_halo =
        (scene.width >= 1100.)
            .then_some(idle.bounds)
            .flatten()
            .map(|[_, top, right, bottom]| {
                // Repeat stays enabled with a single queued track, unlike Next.
                let x = right as f32 + 79.;
                let y = (top + bottom) as f32 * 0.5;
                (x, y)
            });
    let neighbor_frame = if let Some((x, y)) = neighbor_halo {
        events(raw, [mouse_move(pointer([x, y]), false)], cx)?;
        delay(cx, 180).await;
        let frame = capture.frame(cx)?;
        frame.image.save(output.join("neighbor-halo.png"))?;
        events(raw, [mouse_move(outside, false)], cx)?;
        Some(frame.report("neighbor-halo"))
    } else {
        None
    };
    fs::write(
        output.join("control-feedback.json"),
        serde_json::to_vec_pretty(&json!({
            "reduced_motion": scene.reduced,
            "pointer_control_center_dip": capture.probe.target,
            "outside_release_dip": capture.probe.outside,
            "neighbor_halo_pointer_dip": neighbor_halo,
            "neighbor_halo": neighbor_frame,
            "fill_region_exclusive_right_bottom": capture.probe.fill,
            "control_region_exclusive_right_bottom": capture.probe.control,
            "hover_early_frame_started_at_ms": hover_sample_ms,
            "hover_intermediate_observed": hover_intermediate,
            "fixed_control_footprint": fixed_footprint,
            "samples": frames.map(|(name, frame)| frame.report(name)),
            "transport": {
                "idle": idle_transport.report(),
                "held": held_transport.report(),
                "released_outside": released_transport.report(),
                "clicked": clicked.report(),
                "double_clicked": double_clicked.report(),
                "keyboard_paused": keyboard_paused.report(),
                "keyboard_playing": keyboard_playing.report(),
            },
        }))?,
    )?;
    let glyph_pressed = if scene.reduced {
        pressed.glyph_pixels == idle.glyph_pixels
    } else {
        pressed.glyph_pixels * 100 < idle.glyph_pixels * 99
            && pressed.glyph_pixels * 100 > idle.glyph_pixels * 75
    };
    println!(
        "{} Play fill luma {:.4} → {:.4} → {:.4}, pressed {:.4}, canceled {:.4}; glyph pixels {} → {} (hover intermediate {hover_intermediate}, scaled {glyph_pressed})",
        scene.name,
        idle.luma,
        early_luma,
        hover.luma,
        pressed.luma,
        released.luma,
        idle.glyph_pixels,
        pressed.glyph_pixels,
    );
    require(
        hover.luma > idle.luma + 0.005
            && pressed.luma < hover.luma - 0.005
            && pressed.luma > 0.68
            && hover.luma - pressed.luma < 0.28
            && released.fill == idle.fill
            && fixed_footprint
            && glyph_pressed
            && (!scene.reduced || hover_sample.fill == hover.fill),
        "Наведение и нажатие должны менять тонировку без сдвига кнопки, сжимать только глиф и восстанавливать начальное состояние.",
    )?;
    require(
        held_transport.label == PLAY_LABEL
            && released_transport.label == PLAY_LABEL
            && (released_transport.position - idle_transport.position).abs() < 0.05
            && [
                &held_transport,
                &released_transport,
                &clicked,
                &keyboard_paused,
                &keyboard_playing,
            ]
            .into_iter()
            .all(|state| state.track == idle_transport.track)
            && keyboard_paused.focus.as_deref() == Some(named(PLAY).as_str())
            && keyboard_playing.focus.as_deref() == Some(named(PLAY).as_str()),
        "Отпускание за пределами кнопки должно отменить действие, а мышь и клавиатура должны управлять текущим треком.",
    )?;
    verify_play_morph(&capture, output, cx).await?;
    verify_sliders(&capture, output, cx).await?;
    verify_devices(&capture, output, cx).await
}

async fn verify_play_morph(
    capture: &Capture,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = capture.raw();
    delay(cx, 240).await;
    let rest = capture.frame(cx)?;
    press(raw, &named(PLAY), cx)?;
    capture.await_transport(PAUSE_LABEL, None, cx).await?;
    delay(cx, 45).await;
    let between = capture.frame(cx)?;
    press(raw, &named(PLAY), cx)?;
    capture.await_transport(PLAY_LABEL, None, cx).await?;
    let reversed = capture.frame(cx)?;
    delay(cx, 240).await;
    let settled = capture.frame(cx)?;
    for (name, frame) in [
        ("rest", &rest),
        ("intermediate", &between),
        ("reversed", &reversed),
        ("settled", &settled),
    ] {
        frame
            .image
            .save(output.join(format!("play-morph-{name}.png")))?;
    }
    let bounds = rest
        .bounds
        .ok_or_else(|| io::Error::other("Нет опорного круга Play/Pause."))?;
    let x = (bounds[0] + bounds[2]) / 2;
    let y = (bounds[1] + bounds[3]) / 2;
    let center = [x - 9, y - 8, x + 11, y + 9];
    let mask = |frame: &Frame| {
        pixels(&frame.image, center)
            .map(|(_, _, rgb)| rgb.into_iter().max().unwrap_or_default() < 110)
            .collect::<Vec<_>>()
    };
    let origin = mask(&rest);
    let middle = mask(&between);
    let reverse = mask(&reversed);
    let final_mask = mask(&settled);
    let differences = |other: &[bool]| origin.iter().zip(other).filter(|(a, b)| a != b).count();
    fs::write(
        output.join("play-morph.json"),
        serde_json::to_vec_pretty(&json!({
            "reduced_motion": capture.scene.reduced,
            "intermediate_pixels_changed": differences(&middle),
            "reversed_pixels_changed": differences(&reverse),
            "settled_pixels_changed": differences(&final_mask),
        }))?,
    )?;
    require(
        differences(&middle) > 3 && final_mask == origin,
        "Быстрый повтор Play/Pause должен вернуть исходный глиф без сдвига кнопки.",
    )
}

async fn verify_devices(capture: &Capture, output: &Path, cx: &mut AsyncApp) -> gpui::Result<()> {
    const BUTTON: &str = "player-devices";
    const PANEL: &str = "player-panel";
    let raw = capture.raw();
    press(raw, &named(BUTTON), cx)?;
    let current = wait(
        cx,
        "Выбранное устройство вывода",
        |cx| {
            let tree = snapshot(raw, cx)?;
            if node_label(&tree, PANEL) != Some("Устройство вывода") {
                return Ok(None);
            }
            Ok(tree["nodes"].as_object().and_then(|nodes| {
                nodes.values().map(|node| &node["aria"]).find_map(|aria| {
                    if aria["role"] == "ListBoxOption" && aria["selected"] == true {
                        aria["label"].as_str().map(str::to_owned)
                    } else {
                        None
                    }
                })
            }))
        },
    )
    .await?;
    render_image(raw, capture.scene, cx)?.save(output.join("device-selector.png"))?;
    key(raw, "escape", cx)?;
    let closed = snapshot(raw, cx)?;
    require(
        node(&closed, PANEL).is_none() && focused(&closed) == Some(named(BUTTON).as_str()),
        "Escape должен закрыть селектор вывода и вернуть фокус кнопке плеера.",
    )?;
    println!(
        "{} output selector: {current}, focus restored",
        capture.scene.name
    );
    Ok(())
}

async fn verify_sliders(capture: &Capture, output: &Path, cx: &mut AsyncApp) -> gpui::Result<()> {
    let raw = capture.raw();
    let value = |tree: &Value, id| {
        node(tree, id)
            .filter(|slider| slider["aria"]["role"] == "Slider")
            .and_then(|slider| slider["aria"]["numeric_value"].as_f64())
    };
    focus(raw, &named(SEEK), cx)?;
    key(raw, "home", cx)?;
    wait(cx, "Начало трека после Home", |cx| {
        Ok(value(&snapshot(raw, cx)?, SEEK).filter(|position| *position < 0.2))
    })
    .await?;
    key(raw, "right", cx)?;
    let seek = wait(cx, "Перемотка на 5 секунд", |cx| {
        let tree = snapshot(raw, cx)?;
        Ok(value(&tree, SEEK).filter(|position| {
            (position - 5.).abs() < 0.2 && focused(&tree) == Some(named(SEEK).as_str())
        }))
    })
    .await?;
    render_image(raw, capture.scene, cx)?.save(output.join("slider-seek-focused.png"))?;

    focus(raw, &named(VOLUME), cx)?;
    let start = value(&snapshot(raw, cx)?, VOLUME)
        .ok_or_else(|| io::Error::other("У ползунка громкости отсутствует значение."))?;
    let (lower, raise) = if start >= 0.05 {
        ("left", "right")
    } else {
        ("right", "left")
    };
    key(raw, lower, cx)?;
    let volume = wait(
        cx,
        "Изменение громкости с клавиатуры",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok(value(&tree, VOLUME).filter(|volume| {
                ((volume - start).abs() - 0.05).abs() < 0.001
                    && focused(&tree) == Some(named(VOLUME).as_str())
            }))
        },
    )
    .await?;
    render_image(raw, capture.scene, cx)?.save(output.join("slider-volume-focused.png"))?;
    key(raw, raise, cx)?;
    wait(cx, "Восстановление громкости", |cx| {
        Ok(value(&snapshot(raw, cx)?, VOLUME).filter(|volume| (volume - start).abs() < 0.001))
    })
    .await?;
    fs::write(
        output.join("sliders.json"),
        serde_json::to_vec_pretty(
            &json!({"seek_seconds": seek, "volume_before": start, "volume_after_arrow": volume}),
        )?,
    )?;
    println!(
        "{} sliders: seek {seek:.2}s, volume {start:.2} → {volume:.2} → {start:.2}",
        capture.scene.name
    );
    Ok(())
}

struct Capture {
    window: WindowHandle<MainView>,
    scene: Scene,
    probe: Probe,
}

impl Capture {
    fn raw(&self) -> AnyWindowHandle {
        self.window.into()
    }

    fn frame(&self, cx: &mut AsyncApp) -> gpui::Result<Frame> {
        let image = render_image(self.raw(), self.scene, cx)?;
        let fill: Vec<_> = pixels(&image, self.probe.fill)
            .map(|(_, _, rgb)| rgb)
            .collect();
        let luma = fill.iter().copied().map(luminance).sum::<f64>() / fill.len() as f64;
        let mut bounds = None;
        for (x, y, [r, g, b]) in pixels(&image, self.probe.control) {
            if r.min(g).min(b) > 140 {
                let [min_x, min_y, max_x, max_y] = bounds.unwrap_or([x, y, x, y]);
                bounds = Some([min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)]);
            }
        }
        let glyph = bounds.map_or((0, None), |bounds| glyph_metrics(&image, bounds));
        Ok(Frame {
            image,
            fill,
            luma,
            bounds,
            glyph_pixels: glyph.0,
            glyph_bounds: glyph.1,
        })
    }

    fn transport(&self, cx: &mut AsyncApp) -> gpui::Result<Transport> {
        let tree = snapshot(self.raw(), cx)?;
        let position = self
            .window
            .update(cx, |view, _, cx| view.sound_state_for_capture(cx))?
            .and_then(|sound| sound["position"].as_f64())
            .ok_or_else(|| io::Error::other("Нет позиции локального трека."))?;
        Ok(Transport {
            label: node_label(&tree, PLAY).unwrap_or_default().to_owned(),
            track: node_label(&tree, "player-track")
                .unwrap_or_default()
                .to_owned(),
            focus: focused(&tree).map(str::to_owned),
            position,
        })
    }

    async fn await_transport(
        &self,
        expected: &str,
        from: Option<f64>,
        cx: &mut AsyncApp,
    ) -> gpui::Result<Transport> {
        wait(cx, expected, |cx| {
            let state = self.transport(cx)?;
            Ok(
                (state.label == expected && from.is_none_or(|from| state.position > from + 0.15))
                    .then_some(state),
            )
        })
        .await
    }
}

struct Frame {
    image: image::RgbaImage,
    fill: Vec<[u8; 3]>,
    luma: f64,
    bounds: Option<Region>,
    glyph_pixels: usize,
    glyph_bounds: Option<Region>,
}

impl Frame {
    fn report(&self, name: &str) -> Value {
        json!({
            "state": name,
            "relative_fill_luminance": self.luma,
            "painted_control_bounds_inclusive": self.bounds,
            "glyph_pixels": self.glyph_pixels,
            "glyph_bounds_inclusive": self.glyph_bounds,
        })
    }
}

/// Count the dark glyph pixels inside the bright control face. The inset keeps
/// the dark background outside the circle out of the measurement.
fn glyph_metrics(
    image: &image::RgbaImage,
    [left, top, right, bottom]: Region,
) -> (usize, Option<Region>) {
    const INSET: i64 = 5;
    const GLYPH_MAX: u8 = 110;
    let left = (i64::from(left) + INSET).max(0) as u32;
    let top = (i64::from(top) + INSET).max(0) as u32;
    let right = (i64::from(right) - INSET).max(i64::from(left)) as u32;
    let bottom = (i64::from(bottom) - INSET).max(i64::from(top)) as u32;
    let mut count = 0;
    let mut bounds = None;
    for (x, y, [r, g, b]) in pixels(image, [left, top, right, bottom]) {
        if r.max(g).max(b) < GLYPH_MAX {
            count += 1;
            let [min_x, min_y, max_x, max_y] = bounds.unwrap_or([x, y, x, y]);
            bounds = Some([min_x.min(x), min_y.min(y), max_x.max(x), max_y.max(y)]);
        }
    }
    (count, bounds)
}

struct Transport {
    label: String,
    track: String,
    focus: Option<String>,
    position: f64,
}

impl Transport {
    fn report(&self) -> Value {
        json!({"label": self.label, "track": self.track, "focus": self.focus, "position": self.position})
    }
}

fn pixels(
    image: &image::RgbaImage,
    [left, top, right, bottom]: Region,
) -> impl Iterator<Item = (u32, u32, [u8; 3])> + '_ {
    (top..bottom).flat_map(move |y| {
        (left..right).map(move |x| {
            let [r, g, b, _] = image.get_pixel(x, y).0;
            (x, y, [r, g, b])
        })
    })
}

fn pointer([x, y]: [f32; 2]) -> Point<Pixels> {
    point(px(x), px(y))
}

pub(super) fn mouse_move(position: Point<Pixels>, held: bool) -> PlatformInput {
    MouseMoveEvent {
        position,
        pressed_button: held.then_some(MouseButton::Left),
        ..Default::default()
    }
    .to_platform_input()
}

pub(super) fn mouse_down(position: Point<Pixels>) -> PlatformInput {
    MouseDownEvent {
        position,
        button: MouseButton::Left,
        click_count: 1,
        ..Default::default()
    }
    .to_platform_input()
}

pub(super) fn mouse_up(position: Point<Pixels>) -> PlatformInput {
    MouseUpEvent {
        position,
        button: MouseButton::Left,
        click_count: 1,
        ..Default::default()
    }
    .to_platform_input()
}

pub(super) fn events<const N: usize>(
    raw: AnyWindowHandle,
    events: [PlatformInput; N],
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    raw.update(cx, |_, window, cx| {
        for input in events {
            window.dispatch_event(input, cx);
        }
    })?;
    // Update hitboxes and deliver hover callbacks before the timed image read.
    raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))
}

async fn delay(cx: &AsyncApp, millis: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(millis))
        .await;
}
