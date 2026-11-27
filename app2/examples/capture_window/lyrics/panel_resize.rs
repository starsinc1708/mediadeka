use std::{io, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::{
    SCENES, Scene, ensure_same_pause, ensure_same_playback, focus, focused_id, frame, key, layout,
    native_dpi_change, node, press, record, require_node, start_guest, start_playlist, wait,
};

const WIDE: Scene = Scene {
    reduced: false,
    ..SCENES[0]
};
const BELOW_DOCK: Scene = Scene {
    name: "1439x720",
    width: 1439.,
    height: 720.,
    ..WIDE
};
const NARROW_TALL: Scene = Scene {
    name: "876x768",
    width: 876.,
    height: 768.,
    ..WIDE
};
const COMPACT: Scene = Scene {
    reduced: false,
    ..SCENES[1]
};
const SCALED: Scene = Scene {
    reduced: false,
    ..SCENES[2]
};
const SOURCE: &str = "lyrics-source";

pub(super) async fn capture(playlist: i64, output: &Path, cx: &mut AsyncApp) -> gpui::Result<()> {
    let view = cx.update(|cx| mediadeka::open_capture_window(cx, layout(WIDE)))?;
    let raw = view.into();
    start_guest(view, cx).await?;
    start_playlist(WIDE, view, playlist, 2, cx).await?;
    wait(view, cx, |state| {
        state["source"] == "Локальный файл" && state["lines"] == 4 && state["playing"] == true
    })
    .await?;
    press(raw, "player-play", cx)?;
    wait(view, cx, |state| state["playing"] == false).await?;
    // Reopen from the player to exercise the same return-focus path as a listener.
    press(raw, "player-lyrics", cx)?;
    press(raw, "player-lyrics", cx)?;
    let paused = local_text(view, cx).await?;
    focus(raw, SOURCE, cx)?;

    let mut observations = Vec::new();
    for (scene, name, docked) in [
        (WIDE, "wide", true),
        (BELOW_DOCK, "below-dock", false),
        (NARROW_TALL, "narrow-tall", false),
        (COMPACT, "compact", false),
        (SCALED, "compact-150pct", false),
    ] {
        resize(raw, scene, cx)?;
        observations.push(lyrics_frame(
            view, scene, output, name, docked, &paused, cx,
        )?);
    }

    for (tab, content, name) in [
        ("right-panel-queue", "player-queue-list", "compact-queue"),
        (
            "right-panel-track",
            "playlist-inspector-title",
            "compact-track",
        ),
    ] {
        press(raw, tab, cx)?;
        cx.background_executor()
            .timer(Duration::from_millis(220))
            .await;
        let observed = frame(raw, SCALED, output, name, cx)?;
        panel(&observed, tab, false)?;
        require_node(&observed, content)?;
    }
    press(raw, "right-panel-lyrics", cx)?;
    local_text(view, cx).await?;
    focus(raw, SOURCE, cx)?;
    view.update(cx, |view, _, cx| {
        view.set_fluid_high_contrast_for_capture(true, cx)
    })?;
    observations.push(lyrics_frame(
        view,
        SCALED,
        output,
        "compact-150pct-contrast",
        false,
        &paused,
        cx,
    )?);
    view.update(cx, |view, _, cx| {
        view.set_fluid_high_contrast_for_capture(false, cx)
    })?;

    resize(raw, WIDE, cx)?;
    observations.push(lyrics_frame(
        view,
        WIDE,
        output,
        "wide-restored",
        true,
        &paused,
        cx,
    )?);
    resize(raw, COMPACT, cx)?;
    key(raw, "escape", cx)?;
    let returned = frame(raw, COMPACT, output, "compact-closed", cx)?;
    require_node(&returned, "workspace-content")?;
    if node(&returned, "right-panel").is_some()
        || focused_id(&returned) != Some("Name(\"player-lyrics\")")
    {
        return Err(
            io::Error::other("Escape должен вернуть страницу и фокус кнопке текста.").into(),
        );
    }
    ensure_same_playback(
        &paused,
        &view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?,
    )?;
    record(output, "panel-resize", &json!(observations))?;
    view.update(cx, |_, window, _| window.remove_window())?;
    Ok(())
}

async fn local_text(view: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<Value> {
    view.update(cx, |view, window, cx| {
        view.choose_local_lyrics_for_capture(window, cx)
    })?;
    wait(view, cx, |state| {
        state["source"] == "Локальный файл" && state["lines"] == 4 && state["playing"] == false
    })
    .await
}

fn resize(raw: AnyWindowHandle, scene: Scene, cx: &mut AsyncApp) -> gpui::Result<()> {
    raw.update(cx, |_, window, _| native_dpi_change(window, scene))??
        .apply();
    Ok(())
}

fn lyrics_frame(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    name: &str,
    docked: bool,
    paused: &Value,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let observed = frame(view.into(), scene, output, name, cx)?;
    panel(&observed, "right-panel-lyrics", docked)?;
    require_node(&observed, "lyrics-line-0")?;
    require_node(&observed, SOURCE)?;
    let current = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
    ensure_same_pause(paused, &current)?;
    if focused_id(&observed) != Some("Name(\"lyrics-source\")") {
        return Err(
            io::Error::other(format!("Размер {name} сбросил фокус источника текста.")).into(),
        );
    }
    Ok(json!({
        "frame": name,
        "viewport_dip": [scene.width, scene.height],
        "dpi": scene.dpi,
        "docked": docked,
        "focus": focused_id(&observed),
        "position": current["player_position"],
        "source": current["selection"],
    }))
}

fn panel(tree: &Value, selected: &str, docked: bool) -> gpui::Result<()> {
    for id in [
        "right-panel",
        "right-panel-queue",
        "right-panel-track",
        "right-panel-lyrics",
        "right-panel-close",
        "player-play",
    ] {
        require_node(tree, id)?;
    }
    if node(tree, "workspace-content").is_some() != docked
        || node(tree, "right-panel-resize").is_some() != docked
        || !node(tree, selected).is_some_and(|tab| tab["aria"]["selected"] == true)
    {
        return Err(io::Error::other(format!(
            "Вкладка {selected} должна сохраняться в режиме docked={docked}."
        ))
        .into());
    }
    Ok(())
}
