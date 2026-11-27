use std::{
    fs, io,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{AnyWindowHandle, AsyncApp, Bounds, Pixels, WindowHandle, point, px};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::super::super::render_image;
use super::super::{
    QUEUE_BUTTON, QUEUE_LIST, Scene,
    control_feedback::{events, mouse_down, mouse_move, mouse_up},
    focus, focused, frame, key, named, node, node_label, press, require, settle_panel, snapshot,
    sound, wait,
};

const PLAY: &str = "player-play";
const SEEK: &str = "player-progress";
const QUALITY: &str = "player-sound-details";
const QUALITY_PANEL: &str = "player-sound-popover";
const INSPECTOR: &str = "player-track-inspector";
const SEEK_FRACTION: f32 = 0.72;
type MeasuredLayout = ([Bounds<Pixels>; 3], Bounds<Pixels>, Option<f64>);

pub(in super::super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let idle = frame(raw, scene, output, "player-idle", cx)?;
    let (idle_zones, _, _) = layout(window, cx)?;
    let (track, title) = idle["nodes"]
        .as_object()
        .and_then(|nodes| {
            nodes.values().find_map(|row| {
                let id = row["element_id"].as_str()?;
                let (title, _) = row["aria"]["label"]
                    .as_str()?
                    .strip_prefix("Слушать · ")?
                    .split_once(" · ")?;
                id.starts_with("Name(\"library-row-").then_some((id, title))
            })
        })
        .ok_or_else(|| io::Error::other("В медиатеке нет видимой строки локального WAV."))?;
    press(raw, track, cx)?;
    sound(window, title, cx).await?;
    press(raw, &named(PLAY), cx)?;
    wait(cx, "libmpv поставил WAV на паузу", |cx| {
        Ok((node_label(&snapshot(raw, cx)?, PLAY) == Some("Воспроизвести")).then_some(()))
    })
    .await?;
    let playing = frame(raw, scene, output, "player-paused", cx)?;
    let duration = node(&playing, SEEK)
        .and_then(|node| node["aria"]["max_numeric_value"].as_f64())
        .filter(|duration| *duration > 0.)
        .ok_or_else(|| io::Error::other("Длительность WAV отсутствует в ползунке."))?;
    let (active_zones, seek, _) = layout(window, cx)?;
    press(raw, &named(QUEUE_BUTTON), cx)?;
    settle_panel(raw, scene, cx).await?;
    let queue = frame(raw, scene, output, "player-queue-open", cx)?;
    let (queue_zones, _, _) = layout(window, cx)?;
    require(
        node(&queue, QUEUE_LIST).is_some()
            && [idle_zones, active_zones, queue_zones]
                .iter()
                .all(|zones| in_order(*zones, scene.width) && zones[1] == active_zones[1]),
        "Три зоны плеера должны сохранять центр при появлении метаданных и открытии очереди.",
    )?;
    key(raw, "escape", cx)?;

    press(raw, &named(QUALITY), cx)?;
    settle_panel(raw, scene, cx).await?;
    let quality = frame(raw, scene, output, "player-sound-popover", cx)?;
    require(
        node_label(&quality, QUALITY_PANEL) == Some("Качество звука")
            && focused(&quality) == Some(named(QUALITY_PANEL).as_str()),
        "Кнопка качества должна открыть доступный popover и перенести фокус.",
    )?;
    key(raw, "escape", cx)?;
    let restored = snapshot(raw, cx)?;
    require(
        node(&restored, QUALITY_PANEL).is_none()
            && focused(&restored) == Some(named(QUALITY).as_str()),
        "Escape должен закрыть popover и вернуть фокус кнопке качества.",
    )?;
    press(raw, &named(INSPECTOR), cx)?;
    settle_panel(raw, scene, cx).await?;
    let inspector = frame(raw, scene, output, "player-track-inspector", cx)?;
    require(
        node(&inspector, "track-panel-sound-details").is_some()
            && node_label(&inspector, "player-track") == Some(title),
        "Инспектор должен показать ту же запись и её подтверждённый источник.",
    )?;
    key(raw, "escape", cx)?;
    require(
        focused(&snapshot(raw, cx)?) == Some(named(INSPECTOR).as_str()),
        "Фокус должен вернуться кнопке инспектора.",
    )?;

    let cursor = point(
        seek.left() + seek.size.width * SEEK_FRACTION,
        seek.center().y,
    );
    let expected = f64::from(SEEK_FRACTION) * duration;
    let before_hover = seek_value(&snapshot(raw, cx)?);
    events(raw, [mouse_move(cursor, false)], cx)?;
    let hover = frame(raw, scene, output, "player-seek-preview", cx)?;
    let preview = layout(window, cx)?.2;
    require(
        seek.size.height >= px(22.)
            && preview.is_some_and(|seconds| (seconds - expected).abs() < 0.75)
            && seek_value(&hover) == before_hover,
        "Наведение должно показать цель перемотки, сохранив текущую позицию.",
    )?;
    events(raw, [mouse_down(cursor), mouse_up(cursor)], cx)?;
    let clicked = wait(cx, "перемотка мышью", |cx| {
        Ok(seek_value(&snapshot(raw, cx)?).filter(|position| (position - expected).abs() < 0.75))
    })
    .await?;
    events(
        raw,
        [mouse_move(point(px(scene.width * 0.5), px(80.)), false)],
        cx,
    )?;
    focus(raw, &named(SEEK), cx)?;
    key(raw, "left", cx)?;
    let keyboard = wait(cx, "перемотка стрелкой", |cx| {
        Ok(seek_value(&snapshot(raw, cx)?)
            .filter(|position| (*position - (clicked - 5.)).abs() < 0.5))
    })
    .await?;
    require(
        layout(window, cx)?.2.is_none(),
        "Предпросмотр должен сброситься при уходе указателя.",
    )?;

    // Keep the shader phase fixed across timing samples.
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_for_capture(Duration::from_millis(1200), cx);
    })?;
    let timing = timing(window, scene, cx)?;
    fs::write(
        output.join("player-layout.json"),
        serde_json::to_vec_pretty(&json!({
            "client_dip": [scene.width, scene.height],
            "dpi": scene.dpi,
            "zones": {
                "idle": idle_zones.map(rect),
                "with_track": active_zones.map(rect),
                "with_queue": queue_zones.map(rect),
            },
            "seek": {"bounds_dip": rect(seek), "preview": preview, "mouse": clicked, "keyboard": keyboard},
            "quality_focus_restored": true,
            "inspector_focus_restored": true,
            "timing": timing,
        }))?,
    )?;
    println!(
        "{}: player draw submit median {:.2} ms, full draw + readback median {:.2} ms",
        scene.name,
        timing["draw_submit_median_ms"].as_f64().unwrap_or_default(),
        timing["capture_median_ms"].as_f64().unwrap_or_default(),
    );
    Ok(())
}

/// Separates the scene/submit half from the readback half. The capture value
/// is the full native frame cost that acceptance reports elsewhere.
fn timing(window: WindowHandle<MainView>, scene: Scene, cx: &mut AsyncApp) -> gpui::Result<Value> {
    let raw = AnyWindowHandle::from(window);
    let mut draw = Vec::with_capacity(12);
    let mut capture = Vec::with_capacity(12);
    for _ in 0..12 {
        let started = Instant::now();
        raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
        draw.push(started.elapsed().as_secs_f64() * 1000.);
        let started = Instant::now();
        let _ = render_image(raw, scene, cx)?;
        capture.push(started.elapsed().as_secs_f64() * 1000.);
    }
    draw.sort_by(f64::total_cmp);
    capture.sort_by(f64::total_cmp);
    let median = |samples: &[f64]| (samples[5] + samples[6]) * 0.5;
    Ok(json!({
        "samples": 12,
        "timing": "window.draw submit vs window.draw + D3D11 render_to_image readback",
        "draw_submit_median_ms": median(&draw),
        "capture_median_ms": median(&capture),
    }))
}

fn layout(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<MeasuredLayout> {
    window.update(cx, |view, _, cx| view.player_layout_for_capture(cx))
}

fn rect(bounds: Bounds<Pixels>) -> [f32; 4] {
    [bounds.left(), bounds.top(), bounds.right(), bounds.bottom()].map(f32::from)
}

fn in_order([track, transport, tools]: [Bounds<Pixels>; 3], width: f32) -> bool {
    track.size.width > px(0.)
        && tools.size.width > px(0.)
        && track.right() <= transport.left()
        && transport.right() <= tools.left()
        && (f32::from(transport.center().x) - width * 0.5).abs() <= 1.
}

fn seek_value(tree: &serde_json::Value) -> Option<f64> {
    node(tree, SEEK)?["aria"]["numeric_value"].as_f64()
}
