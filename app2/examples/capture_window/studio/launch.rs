use std::{fs, path::Path, time::Duration};

use gpui::{AsyncApp, WindowHandle};
use image::RgbaImage;
use mediadeka::MainView;
use serde_json::json;

use super::{super::render_image, Scene, require};

pub(super) fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let (origin, player_top, enabled) = window.update(cx, |view, window, cx| {
        let geometry = view.fluid_glass_geometry_for_capture(window);
        let quality = view.fluid_glass_quality_for_capture(window, cx);
        (
            view.fluid_glass_row_launch_origin_for_capture(cx),
            scene.height - geometry[0] - geometry[2],
            quality.is_some_and(|(name, _, _, _)| matches!(name, "balanced" | "high" | "ultra")),
        )
    })?;
    require(
        origin.is_some() == enabled,
        "Ожидался источник светового импульса от клавиатурной строки после подтверждения libmpv.",
    )?;
    let mut observations = Vec::new();
    let mut centers = Vec::new();
    let dpi = scene.dpi as f64 / 96.;
    for (name, milliseconds) in [
        ("source", 46),
        ("travel", 207),
        ("player", 363),
        ("ended", 450),
    ] {
        window.update(cx, |view, _, cx| {
            view.sample_fluid_glass_palette_for_capture(
                Duration::from_millis(milliseconds),
                Duration::ZERO,
                cx,
            );
        })?;
        let painted = render_image(window.into(), scene, cx)?;
        window.update(cx, |view, _, cx| {
            view.show_fluid_glass_row_launch_for_capture(false, cx)
        })?;
        let reference = render_image(window.into(), scene, cx);
        window.update(cx, |view, _, cx| {
            view.show_fluid_glass_row_launch_for_capture(true, cx)
        })?;
        let reference = reference?;
        painted.save(output.join(format!("row-light-{name}.png")))?;
        let difference = Difference::between(&painted, &reference);
        let center = difference.center(dpi);
        observations.push(json!({
            "phase_ms": milliseconds,
            "changed_pixels": difference.count,
            "peak_rgb_delta": difference.peak,
            "center_dip": center,
        }));
        centers.push(center);
    }
    fs::write(
        output.join("row-light.json"),
        serde_json::to_vec_pretty(&json!({
            "confirmed_pcm_source": origin,
            "player_top_dip": player_top,
            "observations": observations,
        }))?,
    )?;
    if let Some(origin) = origin {
        require(
            matches!(
                centers.as_slice(),
                [Some(source), Some(travel), Some(player), None]
                    if (source[0] - f64::from(origin[0])).abs() < 24.
                        && (source[1] - f64::from(origin[1])).abs() < 24.
                        && travel[1] > source[1] + 65.
                        && player[1] > travel[1] + 65.
                        && player[1] > f64::from(player_top)
            ),
            &format!(
                "Пиксели света должны пройти от строки в стекло плеера и погаснуть. {centers:?}"
            ),
        )?;
    } else {
        require(
            centers.iter().all(Option::is_none),
            "На выключенном или статическом материале появились пиксели импульса.",
        )?;
    }
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_for_capture(Duration::ZERO, cx);
    })?;
    println!("{} row light: {centers:?}", scene.name);
    Ok(())
}

#[derive(Default)]
struct Difference {
    weight: u64,
    sum_x: u64,
    sum_y: u64,
    count: u32,
    peak: u8,
}

impl Difference {
    fn between(painted: &RgbaImage, reference: &RgbaImage) -> Self {
        let mut diff = Self::default();
        for (x, y, rgba) in painted.enumerate_pixels() {
            let reference = reference.get_pixel(x, y);
            let peak = (0..3)
                .map(|channel| rgba[channel].abs_diff(reference[channel]))
                .max()
                .unwrap_or_default();
            if peak >= 4 {
                let weight = u64::from(peak);
                diff.weight += weight;
                diff.sum_x += u64::from(x) * weight;
                diff.sum_y += u64::from(y) * weight;
                diff.count += 1;
                diff.peak = diff.peak.max(peak);
            }
        }
        diff
    }

    fn center(&self, dpi: f64) -> Option<[f64; 2]> {
        (self.count > 10).then(|| {
            [
                self.sum_x as f64 / self.weight as f64 / dpi,
                self.sum_y as f64 / self.weight as f64 / dpi,
            ]
        })
    }
}
