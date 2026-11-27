use std::{fs, io, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::{Value, json};

#[path = "player_layout/controls.rs"]
mod controls;
pub(super) use controls::verify as verify_controls;

use super::{
    QUEUE_LIST, Scene, fixture::SPECTRUM_TONES, focus, focused, frame, key, named,
    native_dpi_change, node, node_label, queue_candidate, require, snapshot, wait,
};

const LISTENING: &str = "player-listening";
const COVER_VIEW: &str = "listening-fullscreen";
const WIDTH_BELOW_LISTENING_TOOL: f32 = 1119.;
const ARTWORK_PHASES: [u64; 5] = [0, 130, 260, 390, 520];

pub(super) fn verify_artwork_motion(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    focus(raw, &named(LISTENING), cx)?;
    let before = read_playback(window, cx)?;
    let original = artwork_extent(window, cx)?;
    window.update(cx, |view, _, cx| {
        view.sample_listening_artwork_for_capture(Some(Duration::ZERO), cx);
    })?;
    key(raw, "enter", cx)?;
    let (forward, forward_reveal) = capture_artwork_phases(window, scene, output, "expand", cx)?;
    let inside = read_playback(window, cx)?;
    key(raw, "escape", cx)?;
    let (reverse, reverse_reveal) = capture_artwork_phases(window, scene, output, "return", cx)?;
    window.update(cx, |view, _, cx| {
        view.sample_listening_artwork_for_capture(None, cx);
    })?;
    let final_tree = snapshot(raw, cx)?;
    let final_state = read_playback(window, cx)?;
    let report = json!({
        "reduced_motion": scene.reduced,
        "phase_ms": ARTWORK_PHASES,
        "original_cover_dip": original,
        "forward_cover_dip": forward,
        "reverse_cover_dip": reverse,
        "forward_reveal": forward_reveal,
        "reverse_reveal": reverse_reveal,
        "playback": [before.clone(), inside.clone(), final_state.clone()],
        "return_focus": focused(&final_tree),
    });
    fs::write(
        output.join("listening-artwork-motion.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    let ends_match = |left: [f32; 4], right: [f32; 4]| {
        left.into_iter()
            .zip(right)
            .all(|(left, right)| (left - right).abs() <= 2.)
    };
    let geometry = if scene.reduced {
        forward.iter().all(|extent| ends_match(*extent, forward[4]))
            && reverse.iter().all(|extent| ends_match(*extent, original))
    } else {
        ends_match(forward[0], original)
            && ends_match(reverse[0], forward[4])
            && ends_match(reverse[4], original)
            && forward.windows(2).all(|pair| pair[0][2] < pair[1][2])
            && reverse.windows(2).all(|pair| pair[0][2] > pair[1][2])
    };
    let reveal_ok = if scene.reduced {
        forward_reveal
            .iter()
            .all(|reveal| reveal == &forward_reveal[4])
    } else {
        forward_reveal[0].0[0] == 0.55
            && forward_reveal[0].1[0] == 0.10
            && forward_reveal[0].2[0] == 0.20
            && forward_reveal.windows(2).all(|pair| {
                pair[0].0[0] <= pair[1].0[0]
                    && pair[1].0[1] <= pair[0].0[1]
                    && pair[0].1[0] <= pair[1].1[0]
                    && pair[0].2[0] <= pair[1].2[0]
            })
            && forward_reveal[4].0 == [1., 0.]
            && forward_reveal[4].1 == [1., 0.]
            && forward_reveal[4].2 == [1., 0.]
    };
    require(
        geometry
            && reveal_ok
            && inside["listening_mode"] == "cover"
            && final_state["listening_mode"].is_null()
            && before["current_track"] == final_state["current_track"]
            && before["queue"] == final_state["queue"]
            && position(&final_state)? >= position(&before)?
            && focused(&final_tree) == Some(named(LISTENING).as_str()),
        &format!(
            "Обложка должна перейти от плеера к сцене и обратно с сохранением записи и фокуса: {report}."
        ),
    )?;
    verify_quick_exit(window, scene, output, (original, forward[2]), cx)
}

fn verify_quick_exit(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    (original, entered): ([f32; 4], [f32; 4]),
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    focus(raw, &named(LISTENING), cx)?;
    window.update(cx, |view, _, cx| {
        view.sample_listening_artwork_for_capture(Some(Duration::ZERO), cx);
    })?;
    key(raw, "enter", cx)?;
    window.update(cx, |view, _, cx| {
        view.sample_listening_artwork_for_capture(Some(Duration::from_millis(260)), cx);
    })?;
    frame(
        window.into(),
        scene,
        output,
        "listening-quick-exit-before",
        cx,
    )?;
    let before = artwork_extent(window, cx)?;
    let playback_before = read_playback(window, cx)?;
    key(raw, "escape", cx)?;
    window.update(cx, |view, _, cx| {
        view.sample_listening_artwork_for_capture(Some(Duration::ZERO), cx);
    })?;
    let tree = frame(
        window.into(),
        scene,
        output,
        "listening-quick-exit-after",
        cx,
    )?;
    let after = artwork_extent(window, cx)?;
    let playback_after = read_playback(window, cx)?;
    let report = json!({
        "original_cover_dip": original,
        "reduced_motion": scene.reduced,
        "entered_mid_transition": before,
        "returned_from_mid_transition": after,
        "playback_before_escape": playback_before,
        "playback_after_escape": playback_after,
        "focus_after_escape": focused(&tree),
    });
    fs::write(
        output.join("listening-quick-exit.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    let same = |left: [f32; 4], right: [f32; 4]| {
        left.into_iter()
            .zip(right)
            .all(|(left, right)| (left - right).abs() <= 2.)
    };
    let return_origin = if scene.reduced { original } else { before };
    require(
        same(before, entered)
            && same(after, return_origin)
            && playback_after["listening_mode"].is_null()
            && playback_before["current_track"] == playback_after["current_track"]
            && position(&playback_after)? >= position(&playback_before)?
            && focused(&tree) == Some(named(LISTENING).as_str()),
        &format!("Быстрый выход должен соблюдать выбранный режим движения: {report}."),
    )?;
    verify_resize_during_motion(window, scene, output, cx)
}

fn verify_resize_during_motion(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let compact = Scene {
        name: "listening-resize",
        width: 800.,
        height: 400.,
        dpi: 96,
        reduced: false,
    };
    focus(raw, &named(LISTENING), cx)?;
    let before = read_playback(window, cx)?;
    window.update(cx, |view, _, cx| {
        view.sample_listening_artwork_for_capture(Some(Duration::ZERO), cx);
    })?;
    key(raw, "enter", cx)?;
    window.update(cx, |view, _, cx| {
        view.sample_listening_artwork_for_capture(Some(Duration::from_millis(130)), cx);
    })?;
    frame(
        window.into(),
        scene,
        output,
        "listening-resize-130-before",
        cx,
    )?;
    super::resize::resize(raw, compact, cx)?;
    window.update(cx, |view, _, cx| {
        view.sample_listening_artwork_for_capture(Some(Duration::from_millis(260)), cx);
    })?;
    frame(
        window.into(),
        compact,
        output,
        "listening-resize-260-after",
        cx,
    )?;
    let resized = artwork_extent(window, cx)?;
    window.update(cx, |view, _, cx| {
        view.sample_listening_artwork_for_capture(Some(Duration::from_millis(520)), cx);
    })?;
    frame(
        window.into(),
        compact,
        output,
        "listening-resize-520-settled",
        cx,
    )?;
    let settled = artwork_extent(window, cx)?;
    key(raw, "escape", cx)?;
    let tree = frame(
        window.into(),
        compact,
        output,
        "listening-resize-return",
        cx,
    )?;
    let after = read_playback(window, cx)?;
    super::resize::resize(raw, scene, cx)?;
    let report = json!({
        "resized_cover_mid": resized,
        "resized_cover_settled": settled,
        "playback_before": before,
        "playback_after": after,
        "focus_after": focused(&tree),
    });
    fs::write(
        output.join("listening-resize.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    require(
        resized[2] > 68.
            && settled[2] > 84.
            && settled[0] >= 0.
            && settled[1] >= 0.
            && after["listening_mode"].is_null()
            && before["current_track"] == after["current_track"]
            && focused(&tree) == Some(named(LISTENING).as_str()),
        &format!("Смена размера во время перехода должна сохранить обложку и фокус: {report}."),
    )
}

type Reveal = ([f32; 2], [f32; 2], [f32; 2]);

fn capture_artwork_phases(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    direction: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<(Vec<[f32; 4]>, Vec<Reveal>)> {
    let mut extents = Vec::new();
    let mut reveals = Vec::new();
    for millis in ARTWORK_PHASES {
        window.update(cx, |view, _, cx| {
            view.sample_listening_artwork_for_capture(Some(Duration::from_millis(millis)), cx);
        })?;
        let name = format!("listening-artwork-{direction}-{millis:03}ms");
        frame(window.into(), scene, output, &name, cx)?;
        extents.push(artwork_extent(window, cx)?);
        reveals.push(window.update(cx, |view, _, _| view.listening_reveal_for_capture())?);
    }
    Ok((extents, reveals))
}

fn artwork_extent(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<[f32; 4]> {
    let bounds = window
        .update(cx, |view, window, _| {
            view.listening_artwork_bounds_for_capture(window)
        })?
        .ok_or_else(|| io::Error::other("На нативном кадре нет границ обложки."))?;
    Ok([
        f32::from(bounds.origin.x),
        f32::from(bounds.origin.y),
        f32::from(bounds.size.width),
        f32::from(bounds.size.height),
    ])
}

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let title = SPECTRUM_TONES[1].0;
    let listening = named(LISTENING);
    let desktop = scene.width == 1920.;
    if desktop {
        focus(raw, &named("player-lyrics"), cx)?;
        key(raw, "tab", cx)?;
    } else {
        focus(raw, &listening, cx)?;
    }
    let initial_tree = frame(raw, scene, output, "player-listening-control", cx)?;
    let before = read_playback(window, cx)?;
    let before_position = position(&before)?;
    require(
        node(&initial_tree, LISTENING).is_some_and(|control| {
            control["aria"]["role"] == "Button"
                && control["aria"]["label"]
                    .as_str()
                    .is_some_and(|name| !name.is_empty())
        }) && focused(&initial_tree) == Some(listening.as_str())
            && node_label(&initial_tree, "player-track") == Some(title)
            && node_label(&initial_tree, "player-play") == Some("Пауза")
            && node(&initial_tree, QUEUE_LIST).is_none()
            && queue_candidate(&initial_tree).is_some()
            && before["current_track"].is_string()
            && before["playing"] == true
            && before["listening_mode"].is_null()
            && before["native_fullscreen"] == false,
        "Клавиатура должна сфокусировать кнопку режима прослушивания на играющей записи в медиатеке.",
    )?;

    key(raw, "enter", cx)?;
    let inside = wait(
        cx,
        "режим обложки, загруженная обложка и ход воспроизведения",
        |cx| {
            snapshot(raw, cx)?;
            let state = read_playback(window, cx)?;
            let ready = state["listening_mode"] == "cover"
                && state["artwork_decoded"] == true
                && position(&state)? > before_position + 0.15;
            Ok(ready.then_some(state))
        },
    )
    .await?;
    let cover_tree = frame(raw, scene, output, "player-listening-open", cx)?;
    require(
        node_label(&cover_tree, COVER_VIEW) == Some("Полноэкранное прослушивание")
            && focused(&cover_tree) == Some(named(COVER_VIEW).as_str())
            && node(&cover_tree, "listening-cover")
                .is_some_and(|tab| tab["aria"]["selected"] == true)
            && node_label(&cover_tree, "listening-track") == Some(title)
            && node(&cover_tree, "listening-back").is_some()
            && node_label(&cover_tree, "player-play") == Some("Пауза")
            && same_playback(&before, &inside),
        "Enter должен открыть обложку текущей записи, переместить фокус и сохранить воспроизведение.",
    )?;

    key(raw, "escape", cx)?;
    let returned_tree = frame(raw, scene, output, "player-listening-return", cx)?;
    let returned = read_playback(window, cx)?;
    let width_handoff = desktop
        .then(|| verify_width_handoff(window, scene, output, &returned, cx))
        .transpose()?;
    let report = json!({
        "entry": if desktop { "desktop_player_tool_after_lyrics" } else { "compact_cover_button" },
        "keyboard_focus": {
            "before": focused(&initial_tree),
            "inside_cover": focused(&cover_tree),
            "after_escape": focused(&returned_tree),
        },
        "playback": { "before": before, "inside_cover": inside, "after_escape": returned },
        "desktop_width_handoff": width_handoff,
    });
    fs::write(
        output.join("player-listening.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    require(
        returned["listening_mode"].is_null()
            && node(&returned_tree, COVER_VIEW).is_none()
            && focused(&returned_tree) == Some(listening.as_str())
            && node_label(&returned_tree, "player-track") == Some(title)
            && node_label(&returned_tree, "player-play") == Some("Пауза")
            && queue_candidate(&returned_tree).is_some()
            && same_playback(&before, &returned)
            && position(&returned)? >= position(&inside)?,
        &format!(
            "Escape должен вернуть медиатеку и исходный фокус при текущем воспроизведении: {report}."
        ),
    )?;
    println!(
        "{}: listening cover keyboard entry and Escape retained playback and focus",
        scene.name
    );
    Ok(())
}

fn verify_width_handoff(
    window: WindowHandle<MainView>,
    original: Scene,
    output: &Path,
    baseline: &Value,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let raw = AnyWindowHandle::from(window);
    let narrow = Scene {
        width: WIDTH_BELOW_LISTENING_TOOL,
        height: 720.,
        ..original
    };
    raw.update(cx, |_, window, _| native_dpi_change(window, narrow))??
        .apply();
    let narrowed = frame(raw, narrow, output, "player-listening-width-below-tool", cx)
        .and_then(|tree| read_playback(window, cx).map(|playback| (tree, playback)));
    raw.update(cx, |_, window, _| native_dpi_change(window, original))??
        .apply();
    let (narrow_tree, narrow_state) = narrowed?;
    let restored_tree = frame(raw, original, output, "player-listening-width-restored", cx)?;
    let restored_state = read_playback(window, cx)?;
    let listening = named(LISTENING);
    let phases = [
        (narrow, &narrow_tree, &narrow_state),
        (original, &restored_tree, &restored_state),
    ];
    let evidence = phases.map(|(scene, tree, playback)| {
        json!({
            "client_dip": [scene.width, scene.height],
            "focus": focused(tree),
            "listening_label": node_label(tree, LISTENING),
            "playback": playback,
        })
    });
    require(
        phases.into_iter().all(|(_, tree, playback)| {
            focused(tree) == Some(listening.as_str())
                && node_label(tree, LISTENING).is_some_and(|label| !label.is_empty())
                && same_playback(baseline, playback)
        }),
        &format!(
            "Фокус должен следовать за командой «Прослушивание» между обложкой и панелью инструментов: {evidence:?}."
        ),
    )?;
    Ok(json!(evidence))
}

fn read_playback(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<Value> {
    window.update(cx, |view, window, cx| {
        let state = view.lyrics_state_for_capture(cx);
        let artwork_decoded = state["listening_mode"] == "cover"
            && view.listening_artwork_decoded_for_capture(window, cx);
        json!({
            "listening_mode": state["listening_mode"],
            "artwork_decoded": artwork_decoded,
            "native_fullscreen": window.is_fullscreen(),
            "current_track": state["current_track"],
            "playing": state["playing"],
            "player_position": state["player_position"],
            "queue": state["queue"],
        })
    })
}

fn same_playback(before: &Value, observed: &Value) -> bool {
    observed["current_track"] == before["current_track"]
        && observed["queue"] == before["queue"]
        && observed["playing"] == true
        && observed["native_fullscreen"] == false
}

fn position(state: &Value) -> gpui::Result<f64> {
    state["player_position"]
        .as_f64()
        .ok_or_else(|| io::Error::other("Нет нативной позиции воспроизведения.").into())
}
