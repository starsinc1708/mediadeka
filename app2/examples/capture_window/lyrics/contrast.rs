use std::{io, path::Path};

use gpui::{AsyncApp, WindowHandle};
use image::RgbImage;
use mediadeka::MainView;
use serde_json::{Value, json};

use super::{Scene, ensure_same_pause, focused_id, frame, record, require_node, tree};

const CANVAS: [u8; 3] = [5, 5, 7];
const PLAYER: [u8; 3] = [16, 16, 22];
const ACCENT: [u8; 3] = [248, 230, 189];

pub(super) fn verify(
    view: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = view.into();
    let initial_tree = tree(raw, cx)?;
    let focus = focused_id(&initial_tree)
        .ok_or_else(|| io::Error::other("В режиме текста отсутствует клавиатурный фокус."))?;
    if focus != "Name(\"listening-text\")" {
        return Err(
            io::Error::other("Перед сменой контраста ожидается фокус вкладки «Текст».").into(),
        );
    }
    let initial = view.update(cx, |view, _, cx| view.lyrics_state_for_capture(cx))?;
    let active = initial["line"]
        .as_u64()
        .filter(|_| !initial["word"].is_null())
        .ok_or_else(|| io::Error::other("Перед замером требуется синхронизированное слово."))?;
    let mut samples = Vec::new();
    for (enabled, name) in [
        (true, "contrast-on"),
        (false, "contrast-off"),
        (true, "contrast-restored"),
    ] {
        view.update(cx, |view, _, cx| {
            view.set_fluid_high_contrast_for_capture(enabled, cx)
        })?;
        let applied = frame(raw, scene, output, name, cx)?;
        require_node(&applied, "lyrics-fullscreen")?;
        require_node(&applied, "fluid-listening-artwork")?;
        require_node(&applied, "player-play")?;
        require_node(&applied, &format!("lyrics-line-{active}"))?;
        let (state, row) = view.update(cx, |view, _, cx| {
            (
                view.lyrics_state_for_capture(cx),
                view.lyrics_line_target_for_capture(active as usize, cx),
            )
        })?;
        ensure_same_pause(&initial, &state)?;
        if focused_id(&applied) != Some(focus)
            || state["line"] != initial["line"]
            || state["word"] != initial["word"]
        {
            return Err(
                io::Error::other("Смена контраста изменила фокус или синхронный текст.").into(),
            );
        }
        let row = row.ok_or_else(|| io::Error::other("Текущая строка вне кадра."))?;
        let image = image::open(output.join(format!("{name}.png")))?.to_rgb8();
        let sample = pixels(&image, f32::from(row.y), scene);
        let opaque = sample["canvas_rgb"] == json!(CANVAS) && sample["player_rgb"] == json!(PLAYER);
        let highlighted = sample["accent_columns"]
            .as_u64()
            .is_some_and(|columns| columns >= 8);
        if opaque != enabled || highlighted != enabled {
            return Err(io::Error::other(format!(
                "Кадр {name} не соответствует режиму контрастности: {sample}"
            ))
            .into());
        }
        samples.push(json!({"frame": name, "enabled": enabled, "focus": focus, "pixels": sample}));
    }
    record(
        output,
        "listening-contrast",
        &json!({"initial": initial, "samples": samples}),
    )?;
    println!(
        "{}: opaque listening surface, timed accent and retained focus across contrast changes",
        scene.name
    );
    Ok(())
}

fn pixels(image: &RgbImage, row_center: f32, scene: Scene) -> Value {
    let (width, height) = image.dimensions();
    let to_pixels = |dip: f32| (dip * scene.dpi as f32 / 96.).round() as u32;
    let compact = scene.height < 500.;
    let player_inset = to_pixels(if compact { 15. } else { 28. });
    let row = to_pixels(row_center);
    let radius = to_pixels(if compact { 20. } else { 30. });
    let range = row.saturating_sub(radius)..(row + radius).min(height);
    // Count columns to disambiguate the colored glyphs from the two-pixel row marker.
    let accent_columns = (width / 5..width)
        .filter(|&x| {
            range.clone().any(|y| {
                image
                    .get_pixel(x, y)
                    .0
                    .iter()
                    .zip(ACCENT)
                    .all(|(found, expected)| found.abs_diff(expected) <= 5)
            })
        })
        .count();
    json!({
        "canvas_rgb": image.get_pixel(width - to_pixels(12.), height / 2).0,
        "player_rgb": image.get_pixel(width / 2, height - player_inset).0,
        "accent_columns": accent_columns,
    })
}
