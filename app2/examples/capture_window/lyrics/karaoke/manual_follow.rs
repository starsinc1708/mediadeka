use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, AsyncApp, InputEvent as _, ScrollDelta, ScrollWheelEvent, WindowHandle, point,
    px,
};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::super::{ensure_same_playback, focused_id, node, tree};
use super::{Scene, frame, press, record, require_node, scroll_y, wait};

const FOLLOW: &str = "lyrics-follow";

pub(super) async fn verify(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(view);
    raw.update(cx, |_, window, cx| window.draw(cx).clear(cx))?;
    let (before, target) = view.update(cx, |view, _, cx| {
        let state = view.lyrics_state_for_capture(cx);
        let target = state["line"]
            .as_u64()
            .and_then(|line| view.lyrics_line_target_for_capture(line as usize, cx));
        (state, target)
    })?;
    let target = target.ok_or_else(|| io::Error::other("Строка для колеса вне окна."))?;
    let before_scroll = scroll_y(&before)?;
    raw.update(cx, |_, window, cx| {
        window.dispatch_event(
            ScrollWheelEvent {
                position: target,
                delta: ScrollDelta::Pixels(point(px(0.), px(720.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.draw(cx).clear(cx);
    })?;
    let browsing = wait(view, cx, |state| {
        state["playing"] == true
            && state["scroll_y"]
                .as_f64()
                .is_some_and(|y| y - before_scroll > 20. && y.abs() < 1.)
    })
    .await?;
    let manual = frame(raw, scene, output, "manual-browse", cx)?;
    require_node(&manual, FOLLOW)?;
    let first_line = line(&browsing)?;
    let held = wait(view, cx, |state| {
        state["playing"] == true
            && state["line"]
                .as_u64()
                .is_some_and(|line| line >= first_line + 2)
    })
    .await?;
    let held_scroll = scroll_y(&held)?;
    if (held_scroll - scroll_y(&browsing)?).abs() > 1. {
        return Err(io::Error::other("Автопрокрутка вернулась во время ручного просмотра.").into());
    }
    require_node(&frame(raw, scene, output, "manual-held", cx)?, FOLLOW)?;
    ensure_same_playback(&before, &held)?;

    // Holding playback makes an unintended seek distinguishable from an audio clock update.
    press(raw, "player-play", cx)?;
    wait(view, cx, |state| state["playing"] == false).await?;
    // Play/Pause can acknowledge before the worker's final position sample is polled.
    cx.background_executor()
        .timer(Duration::from_millis(380))
        .await;
    let prior = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
    let started = Instant::now();
    press(raw, FOLLOW, cx)?;
    let resumed = wait(view, cx, |state| {
        state["playing"] == false
            && state["scroll_y"]
                .as_f64()
                .is_some_and(|y| held_scroll - y > 35.)
    })
    .await?;
    let elapsed = started.elapsed().as_secs_f64();
    ensure_same_playback(&prior, &resumed)?;
    let position_delta = position(&resumed)? - position(&prior)?;
    let prior_line = line(&prior)?;
    let resumed_line = line(&resumed)?;
    let focus = tree(raw, cx)?;
    let manual_visible = node(&focus, FOLLOW).is_some();
    if focused_id(&focus) != Some("Name(\"lyrics-source\")") {
        return Err(io::Error::other("Фокус не остался у селектора источника текста.").into());
    }
    if manual_visible
        || resumed_line < prior_line
        || resumed["source"] != before["source"]
        || resumed["selection"] != before["selection"]
        || position_delta.abs() > 0.02
    {
        return Err(io::Error::other(
            format!("Возврат к текущей строке: кнопка осталась {manual_visible}, строка {prior_line}→{resumed_line}, позиция +{position_delta:.3} с за {elapsed:.3} с, источник {}→{}.", before["source"], resumed["source"]),
        )
        .into());
    }
    let active = view.update(cx, |view, _, cx| {
        view.lyrics_line_target_for_capture(resumed_line as usize, cx)
    })?;
    if active.is_none() {
        return Err(io::Error::other("После команды текущая строка осталась вне окна.").into());
    }
    frame(raw, scene, output, "follow-restored", cx)?;
    record(output, "follow-restored", &resumed)?;
    let restored_scroll = scroll_y(&resumed)?;
    press(raw, "player-play", cx)?;
    let (continued, _) = super::follow_karaoke(view, scene, &resumed, cx).await?;
    ensure_same_playback(&resumed, &continued)?;
    if line(&continued)? <= resumed_line {
        return Err(
            io::Error::other("После возобновления активная строка не продвинулась.").into(),
        );
    }
    record(
        output,
        "manual-follow",
        &json!({
            "scroll_dip": [before_scroll, scroll_y(&browsing)?, held_scroll, restored_scroll, scroll_y(&continued)?],
            "lines": [line(&before)?, first_line, line(&held)?, resumed_line, line(&continued)?],
            "player_position_s": [position(&before)?, position(&browsing)?, position(&held)?, position(&resumed)?, position(&continued)?],
            "position_before_resume_s": position(&prior)?,
            "resume_playhead_delta_s": position_delta,
            "resume_elapsed_ms": elapsed * 1_000.,
            "source": resumed["source"],
            "queue_revision": resumed["queue"]["revision"],
            "focus_after": focused_id(&focus),
        }),
    )?;
    Ok(())
}

fn line(state: &Value) -> gpui::Result<u64> {
    state["line"]
        .as_u64()
        .ok_or_else(|| io::Error::other("Активная строка недоступна.").into())
}

fn position(state: &Value) -> gpui::Result<f64> {
    state["player_position"]
        .as_f64()
        .ok_or_else(|| io::Error::other("Позиция плеера недоступна.").into())
}
