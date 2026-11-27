//! Native profile acceptance and offscreen rendering wall time.

use std::{
    env, fs, io,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::{Value, json};

use super::super::render_image;
use super::{Scene, require};

// A color-field patch between the desktop toolbar and track list.
const BACKGROUND: [u32; 4] = [1370, 80, 1700, 170];
const PHASES_MS: [u64; 2] = [0, 2000];
const WARM_SAMPLES: usize = 8;

pub(super) fn requested() -> Option<String> {
    env::var("MEDIADEKA_GLASS_QUALITY")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
}

pub(super) fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    requested: &str,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let selected = match requested {
        "ultra" => ("ultra", 1.0, 60, true),
        "high" => ("high", 0.75, 60, true),
        "balanced" => ("balanced", 0.50, 30, true),
        "eco" => ("eco", 0.375, 15, false),
        "static" => ("static", 0.50, 0, false),
        _ => {
            return Err(io::Error::other(format!(
                "Неизвестный профиль Fluid Glass {requested:?}."
            ))
            .into());
        }
    };
    let expected = if scene.reduced {
        ("static", 0.50, 0, false)
    } else {
        selected
    };
    let (actual, device, dpi_scale) = window.update(cx, |view, window, cx| {
        (
            view.fluid_glass_quality_for_capture(window, cx),
            window.gpu_specs(),
            window.scale_factor(),
        )
    })?;
    let effects = !env::var_os("MEDIADEKA_GLASS_EFFECTS").is_some_and(|value| value == "off");
    require(
        actual == effects.then_some(expected),
        &format!(
            "Профиль {requested} в сцене {} вернул {actual:?}.",
            scene.name
        ),
    )?;
    let Some((effective, raster_scale, fps, reactive_audio)) = actual else {
        return Ok(());
    };
    let pixels = [
        (scene.width * dpi_scale).round() as u32,
        (scene.height * dpi_scale).round() as u32,
    ];
    let probe = (scene.width == 1920. && scene.height == 1080. && scene.dpi == 96)
        .then(|| probe(window, scene, output, fps, cx))
        .transpose()?;
    fs::write(
        output.join("quality-profile.json"),
        serde_json::to_vec_pretty(&json!({
            "requested": requested,
            "effective": effective,
            "device": device,
            "dimensions_dip": [scene.width, scene.height],
            "frame_pixels": pixels,
            "dpi_scale": dpi_scale,
            "reduced_motion": scene.reduced,
            "background_raster_scale": raster_scale,
            "nominal_background_pixels": pixels.map(|extent| (extent as f32 * raster_scale).ceil() as u32),
            "target_atmosphere_fps": fps,
            "audio_reactive": reactive_audio,
            "probe": probe,
        }))?,
    )?;
    println!(
        "{}: Fluid Glass {requested} → {effective}, background {:.1}%, target {fps} fps, audio {reactive_audio}",
        scene.name,
        raster_scale * 100.
    );
    Ok(())
}

fn probe(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    fps: u32,
    cx: &mut AsyncApp,
) -> gpui::Result<Value> {
    let [start, end] = PHASES_MS;
    let before = sample(window, scene, start, cx)?;
    let after = sample(window, scene, end, cx)?;
    let [left, top, right, bottom] = BACKGROUND;
    let changed = (top..bottom)
        .flat_map(|y| (left..right).map(move |x| (x, y)))
        .filter(|&(x, y)| before.get_pixel(x, y) != after.get_pixel(x, y))
        .count();

    let mut elapsed_ms = [0.; WARM_SAMPLES];
    for (index, elapsed) in elapsed_ms.iter_mut().enumerate() {
        let now = Instant::now();
        sample(window, scene, end + (index + 1) as u64 * 40, cx)?;
        *elapsed = now.elapsed().as_secs_f64() * 1000.;
    }
    let mut sorted = elapsed_ms;
    sorted.sort_by(f64::total_cmp);
    let median_ms = (sorted[WARM_SAMPLES / 2 - 1] + sorted[WARM_SAMPLES / 2]) / 2.;

    let frames = ["quality-0ms.png", "quality-2000ms.png"];
    before.save(output.join(frames[0]))?;
    after.save(output.join(frames[1]))?;
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_for_capture(Duration::ZERO, cx);
    })?;
    require(
        (changed > 0) == (fps > 0),
        &format!(
            "Движение фона не соответствует профилю {fps} кадр/с. Изменилось {changed} пикселей."
        ),
    )?;
    println!(
        "Fluid Glass profile: {changed} changed background pixels and {median_ms:.2} ms median offscreen capture including GPU readback"
    );
    Ok(json!({
        "region_exclusive_right_bottom": BACKGROUND,
        "shader_phases_ms": PHASES_MS,
        "frames": frames,
        "changed_pixels": changed,
        "forced_offscreen_frame_wall_ms": elapsed_ms,
        "median_wall_ms": median_ms,
        "measurement": "Warmed forced GPUI draw, native GPU rasterization and readback, excluding PNG encoding and screen presentation.",
    }))
}

fn sample(
    window: WindowHandle<MainView>,
    scene: Scene,
    phase_ms: u64,
    cx: &mut AsyncApp,
) -> gpui::Result<image::RgbaImage> {
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_for_capture(Duration::from_millis(phase_ms), cx);
    })?;
    render_image(window.into(), scene, cx)
}
