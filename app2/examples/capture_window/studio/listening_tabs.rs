use std::{fs, io, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle, point, px};
use image::RgbaImage;
use mediadeka::MainView;
use serde_json::{Value, json};

use super::{
    Scene,
    control_feedback::{events, mouse_down, mouse_move, mouse_up},
    focus, focused, frame, key, named, node, press, require, snapshot, sound,
};

const COVER: &str = "listening-cover";
const TEXT: &str = "listening-text";
const LISTENING: &str = "player-listening";
const TRANSITION_MS: u64 = 140;

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    high_contrast: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let tree = snapshot(raw, cx)?;
    let (id, track) = tree["nodes"]
        .as_object()
        .and_then(|nodes| {
            nodes.values().find_map(|row| {
                let id = row["element_id"].as_str()?;
                let label = row["aria"]["label"].as_str()?.strip_prefix("Слушать · ")?;
                let (title, _) = label.split_once(" · ")?;
                id.starts_with("Name(\"library-row-").then_some((id, title))
            })
        })
        .ok_or_else(|| io::Error::other("Для приёмки вкладок нужна строка локального WAV."))?;
    press(raw, id, cx)?;
    sound(window, track, cx).await?;
    let before = state(window, cx)?;

    let outside = point(px(3.), px(3.));
    events(raw, [mouse_move(outside, false)], cx)?;
    press(raw, &named(LISTENING), cx)?;
    let cover = frame(raw, scene, output, "listening-cover-selected", cx)?;
    require(selection(&cover, COVER, TEXT), "Выбрана вкладка «Обложка».")?;

    // The toolbar has two equal wings and the 32 DIP tabs are vertically
    // centered in its 50 DIP compact or 72 DIP desktop row.
    let center_y = if scene.width <= 1100. { 25. } else { 36. };
    let cover_point = point(px(scene.width * 0.5 - 40.), px(center_y));
    let text_point = point(px(scene.width * 0.5 + 40.), px(center_y));
    events(raw, [mouse_move(text_point, false)], cx)?;
    let hover = frame(raw, scene, output, "listening-text-hover", cx)?;
    require(
        selection(&hover, COVER, TEXT),
        "Наведение сохраняет выбранный вид.",
    )?;
    sample_presentation(window, 0, cx)?;
    events(raw, [mouse_down(text_point), mouse_up(text_point)], cx)?;
    events(raw, [mouse_move(outside, false)], cx)?;
    let text_initial = frame(raw, scene, output, "listening-text-initial", cx)?;
    require(
        selection(&text_initial, TEXT, COVER)
            && node(&text_initial, "lyrics-fullscreen").is_some()
            && focused(&text_initial) == Some(named(TEXT).as_str()),
        "Нажатие указателем сразу открывает текст и переносит выбор и фокус.",
    )?;

    let midpoint_ms = TRANSITION_MS / 2;
    sample_presentation(window, midpoint_ms, cx)?;
    frame(raw, scene, output, "listening-text-midway", cx)?;
    events(raw, [mouse_down(cover_point), mouse_up(cover_point)], cx)?;
    events(raw, [mouse_move(outside, false)], cx)?;
    let reverse = frame(raw, scene, output, "listening-cover-reverse-start", cx)?;
    require(
        selection(&reverse, COVER, TEXT) && node(&reverse, "listening-fullscreen").is_some(),
        "Обратное переключение сразу возвращает вид обложки.",
    )?;
    sample_presentation(window, midpoint_ms + TRANSITION_MS, cx)?;
    frame(raw, scene, output, "listening-cover-reverse-end", cx)?;
    events(raw, [mouse_down(text_point), mouse_up(text_point)], cx)?;
    events(raw, [mouse_move(outside, false)], cx)?;
    sample_presentation(window, midpoint_ms + 2 * TRANSITION_MS, cx)?;
    frame(raw, scene, output, "listening-text-selected", cx)?;

    focus(raw, &named(COVER), cx)?;
    let keyboard = frame(raw, scene, output, "listening-cover-keyboard", cx)?;
    require(
        selection(&keyboard, TEXT, COVER) && focused(&keyboard) == Some(named(COVER).as_str()),
        "Клавиатурный фокус перемещается независимо от выбора вкладки.",
    )?;
    key(raw, "enter", cx)?;
    let again = frame(raw, scene, output, "listening-cover-keyboard-selected", cx)?;
    require(
        selection(&again, COVER, TEXT) && node(&again, "listening-fullscreen").is_some(),
        "Enter возвращает обложку.",
    )?;
    key(raw, "escape", cx)?;
    let restored = snapshot(raw, cx)?;
    require(
        node(&restored, "listening-tabs").is_none()
            && focused(&restored) == Some(named(LISTENING).as_str()),
        "Escape возвращает клавиатурный фокус к кнопке полноэкранного режима.",
    )?;
    let after = state(window, cx)?;
    require(
        after["playing"] == true
            && before["queue"] == after["queue"]
            && before["current_track"] == after["current_track"]
            && after["player_position"].as_f64() >= before["player_position"].as_f64(),
        "Переключение вкладок сохраняет запись, очередь и позицию воспроизведения.",
    )?;
    let motion_expected = !scene.reduced
        && !high_contrast
        && !["off", "none"]
            .iter()
            .any(|value| configured("MEDIADEKA_GLASS_EFFECTS", value))
        && !configured("MEDIADEKA_GLASS_QUALITY", "static");
    let paint = verify_paint(output, scene, center_y, motion_expected)?;
    fs::write(
        output.join("listening-tabs.json"),
        serde_json::to_vec_pretty(&json!({
            "high_contrast": high_contrast,
            "reduced_motion": scene.reduced,
            "focus_restored": focused(&restored),
            "selection": [COVER, TEXT, COVER],
            "playback_before": before["player_position"],
            "playback_after": after["player_position"],
            "paint": paint,
        }))?,
    )?;
    Ok(())
}

fn verify_paint(
    output: &Path,
    scene: Scene,
    center_y: f32,
    motion_expected: bool,
) -> gpui::Result<Value> {
    let read =
        |name| image::open(output.join(format!("{name}.png"))).map(image::DynamicImage::into_rgba8);
    let cover = read("listening-cover-selected")?;
    let text = read("listening-text-selected")?;
    let hover = read("listening-text-hover")?;
    let scale = scene.dpi as f32 / 96.;
    let x = |offset: f32| ((scene.width * 0.5 + offset) * scale).round() as u32;
    let y = |offset: f32| ((center_y + offset) * scale).round() as u32;
    let shade = |frame: &RgbaImage, x, y| {
        let pixel = frame.get_pixel(x, y);
        (f32::from(pixel[0]) + f32::from(pixel[1]) + f32::from(pixel[2])) / 3.
    };
    let marker = |frame: &RgbaImage, slot: f32| {
        (y(11.)..=y(15.))
            .map(|y| {
                (x(slot - 13.)..=x(slot + 13.))
                    .filter(|x| shade(frame, *x, y) > 100.)
                    .count()
            })
            .max()
            .unwrap_or(0) as f32
            / scale
    };
    let cover_marker = marker(&cover, -40.);
    let text_marker = marker(&text, 40.);
    require(
        (14. ..=20.).contains(&cover_marker)
            && (14. ..=20.).contains(&text_marker)
            && marker(&cover, 40.) < 2.
            && marker(&text, -40.) < 2.,
        "Короткая акцентная черта видна только под выбранной вкладкой.",
    )?;
    let gap_delta = (shade(&cover, x(0.), y(0.)) - shade(&cover, x(-96.), y(0.))).abs();
    let hover_delta = shade(&hover, x(64.), y(0.)) - shade(&cover, x(64.), y(0.));
    require(
        gap_delta < 10.
            && (shade(&hover, x(0.), y(0.)) - shade(&cover, x(0.), y(0.))).abs() < 5.
            && hover_delta > 8.,
        "Нативные пиксели должны подтвердить прозрачный зазор и локальную подложку наведения.",
    )?;

    let center = |name| -> gpui::Result<f32> {
        let image = read(name)?;
        let row = (y(11.)..=y(14.))
            .map(|y| {
                (x(-56.)..=x(56.))
                    .filter(|x| {
                        let [r, g, b, _] = image.get_pixel(*x, y).0;
                        r.max(g).max(b) - r.min(g).min(b) > 24 && shade(&image, *x, y) > 100.
                    })
                    .collect::<Vec<_>>()
            })
            // Skip the outline on the bottom edge when recording a focus frame.
            .filter(|row| (14. ..=20.).contains(&(row.len() as f32 / scale)))
            .max_by_key(Vec::len)
            .ok_or_else(|| io::Error::other(format!("Не найдена метка вкладки в кадре {name}.")))?;
        Ok(row.iter().sum::<u32>() as f32 / row.len() as f32 / scale - scene.width * 0.5)
    };
    let initial = center("listening-text-initial")?;
    let middle = center("listening-text-midway")?;
    let reverse = center("listening-cover-reverse-start")?;
    let returned = center("listening-cover-reverse-end")?;
    let keyboard = center("listening-cover-keyboard-selected")?;
    let at = |actual: f32, expected: f32| (actual - expected).abs() < 1.;
    let expected_positions = if motion_expected {
        at(initial, -40.) && at(middle, 20.) && at(reverse, middle)
    } else {
        at(initial, 40.) && at(middle, 40.) && at(reverse, -40.)
    };
    require(
        expected_positions && at(returned, -40.) && at(keyboard, -40.),
        &format!(
            "Метка вкладок должна сохранять видимую позицию при развороте и сразу реагировать на клавиатуру. Координаты: {initial}, {middle}, {reverse}, {returned}, {keyboard}."
        ),
    )?;
    Ok(json!({
        "cover_marker_dip": cover_marker,
        "text_marker_dip": text_marker,
        "rest_gap_delta": gap_delta,
        "hover_surface_delta": hover_delta,
        "motion_expected": motion_expected,
        "marker_centers_dip": {
            "pointer_initial": initial,
            "pointer_midpoint": middle,
            "reverse_start": reverse,
            "reverse_end": returned,
            "keyboard": keyboard,
        },
    }))
}

fn configured(name: &str, value: &str) -> bool {
    std::env::var(name).is_ok_and(|current| current.trim().eq_ignore_ascii_case(value))
}

fn sample_presentation(
    window: WindowHandle<MainView>,
    elapsed_ms: u64,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    window.update(cx, |view, _, cx| {
        view.sample_presentation_for_capture(Duration::from_millis(elapsed_ms), cx)
    })
}

fn selection(tree: &Value, selected: &str, other: &str) -> bool {
    node(tree, "listening-tabs").is_some_and(|tablist| tablist["aria"]["role"] == "TabList")
        && [(selected, true), (other, false)]
            .into_iter()
            .all(|(id, selected)| {
                node(tree, id).is_some_and(|tab| {
                    tab["aria"]["role"] == "Tab" && tab["aria"]["selected"] == selected
                })
            })
}

fn state(window: WindowHandle<MainView>, cx: &mut AsyncApp) -> gpui::Result<Value> {
    window.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))
}
