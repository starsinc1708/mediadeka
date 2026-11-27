use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};

use gpui::{AsyncApp, WindowHandle};
use image::RgbaImage;
use mediadeka::{
    GlassPalette, MainView, PatternId, PatternSelection, QualityChoice, VisualFlags, VisualSettings,
};
use serde_json::{Value, json};

use super::{super::render_image, QUEUE_BUTTON, Scene, named, press, require};

const PHASE: Duration = Duration::from_millis(7500);
const PASSES: [&str; 7] = [
    "composite",
    "sdf",
    "normals",
    "mask",
    "uv",
    "refraction",
    "edge",
];

pub(super) fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let capture = Capture {
        window,
        scene,
        output,
    };
    if scene.name == "1920x1080" {
        capture.effects(cx)?;
    }
    capture.geometry(cx)
}

struct Capture<'a> {
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &'a Path,
}

impl Capture<'_> {
    fn set_visual(&self, visual: VisualSettings, cx: &mut AsyncApp) -> gpui::Result<()> {
        self.window.update(cx, |view, window, cx| {
            view.set_visual_for_capture(GlassPalette::Solar, visual, window, cx);
            view.sample_fluid_glass_palette_for_capture(PHASE, PHASE, cx);
        })
    }

    fn frame(&self, stem: &str, cx: &mut AsyncApp) -> gpui::Result<(RgbaImage, f64)> {
        let start = Instant::now();
        let frame = render_image(self.window.into(), self.scene, cx)?;
        let capture_ms = start.elapsed().as_secs_f64() * 1000.;
        frame.save(self.output.join(format!("{stem}.png")))?;
        Ok((frame, capture_ms))
    }

    fn effects(&self, cx: &mut AsyncApp) -> gpui::Result<()> {
        let base = VisualSettings {
            pattern: PatternSelection::Fixed(PatternId::ThermalFlow),
            quality: QualityChoice::Ultra,
            effects: VisualFlags::NONE,
            ..VisualSettings::default()
        };
        self.window.update(cx, |view, _, cx| {
            view.set_fluid_shader_diagnostics_for_capture(0, true, cx);
        })?;
        let mut measurements = Vec::new();
        for (name, flags, region) in [
            ("fluted", VisualFlags::FLUTED, [120, 880, 430, 970]),
            ("refraction", VisualFlags::REFRACTION, [246, 790, 302, 961]),
            ("dispersion", VisualFlags::DISPERSION, [258, 800, 340, 961]),
            ("grain", VisualFlags::GRAIN, [1250, 570, 1540, 790]),
            ("bloom", VisualFlags::BLOOM, [900, 120, 1710, 850]),
            ("glitch", VisualFlags::GLITCH, [1510, 218, 1840, 368]),
            ("vignette", VisualFlags::VIGNETTE, [40, 650, 240, 820]),
        ] {
            let off = if flags == VisualFlags::DISPERSION {
                VisualFlags::REFRACTION
            } else {
                0
            };
            measurements.push(self.compare(
                name,
                VisualSettings {
                    effects: VisualFlags::from_bits(off),
                    ..base
                },
                VisualSettings {
                    effects: VisualFlags::from_bits(off | flags),
                    ..base
                },
                region,
                cx,
            )?);
        }
        for (name, before, after) in [
            (
                "intensity",
                VisualSettings {
                    intensity: 0.,
                    ..base
                },
                VisualSettings {
                    intensity: 1.,
                    ..base
                },
            ),
            (
                "speed",
                VisualSettings { speed: 0., ..base },
                VisualSettings { speed: 1., ..base },
            ),
        ] {
            measurements.push(self.compare(name, before, after, [980, 225, 1730, 850], cx)?);
        }
        let optical = VisualSettings {
            effects: VisualFlags::from_bits(VisualFlags::REFRACTION),
            ..base
        };
        let surface = optical.surface;
        for (name, low, high, region) in [
            (
                "opacity",
                mediadeka::SurfaceAppearance {
                    opacity: 0,
                    ..surface
                },
                mediadeka::SurfaceAppearance {
                    opacity: 100,
                    ..surface
                },
                [220, 810, 440, 980],
            ),
            (
                "blur",
                mediadeka::SurfaceAppearance { blur: 0, ..surface },
                mediadeka::SurfaceAppearance {
                    blur: 100,
                    ..surface
                },
                [1580, 900, 1900, 1049],
            ),
        ] {
            measurements.push(self.compare(
                name,
                VisualSettings {
                    surface: low,
                    ..optical
                },
                VisualSettings {
                    surface: high,
                    ..optical
                },
                region,
                cx,
            )?);
        }
        self.window.update(cx, |view, _, cx| {
            view.set_fluid_shader_diagnostics_for_capture(0, false, cx);
        })?;
        fs::write(
            self.output.join("effect-pairs.json"),
            serde_json::to_vec_pretty(&json!({
                "phase_seconds": PHASE.as_secs_f32(),
                "palette": "solar",
                "pattern": "thermal-flow",
                "profile": "ultra",
                "timing": "total native offscreen draw and readback, in ms",
                "effects": measurements,
            }))?,
        )?;
        Ok(())
    }

    fn compare(
        &self,
        name: &str,
        before: VisualSettings,
        after: VisualSettings,
        [left, top, right, bottom]: [u32; 4],
        cx: &mut AsyncApp,
    ) -> gpui::Result<Value> {
        self.set_visual(before, cx)?;
        let _ = render_image(self.window.into(), self.scene, cx)?;
        let (before, before_ms) = self.frame(&format!("{name}-before"), cx)?;
        self.set_visual(after, cx)?;
        let _ = render_image(self.window.into(), self.scene, cx)?;
        let (after, after_ms) = self.frame(&format!("{name}-after"), cx)?;
        let mut total = 0_u64;
        let mut changed = 0_u32;
        let mut peak = 0_u8;
        for y in top..bottom {
            for x in left..right {
                let a = before.get_pixel(x, y);
                let b = after.get_pixel(x, y);
                let delta = (0..3)
                    .map(|channel| a[channel].abs_diff(b[channel]))
                    .max()
                    .unwrap();
                total += u64::from(delta);
                changed += u32::from(delta > 0);
                peak = peak.max(delta);
            }
        }
        require(
            changed > 20 && peak >= 2,
            &format!(
                "Эффект {name} не различим в своей области ({changed} изменённых пикселей, шаг {peak})."
            ),
        )?;
        Ok(json!({
            "effect": name, "region": [left, top, right, bottom],
            "changed_pixels": changed, "peak_rgb_step": peak,
            "mean_rgb_step": total as f64 / f64::from((right - left) * (bottom - top)),
            "before_capture_ms": before_ms, "after_capture_ms": after_ms,
        }))
    }

    fn geometry(&self, cx: &mut AsyncApp) -> gpui::Result<()> {
        let raw = self.window.into();
        let visual = VisualSettings {
            pattern: PatternSelection::Fixed(PatternId::FlutedGlass),
            quality: QualityChoice::Balanced,
            ..VisualSettings::default()
        };
        for quality in QualityChoice::ALL {
            self.set_visual(VisualSettings { quality, ..visual }, cx)?;
            self.frame(&format!("junction-{}-closed", quality.as_str()), cx)?;
        }
        self.set_visual(visual, cx)?;
        for (index, pass) in PASSES.iter().enumerate() {
            self.window.update(cx, |view, _, cx| {
                view.set_fluid_shader_diagnostics_for_capture(index, false, cx);
            })?;
            let (image, _) = self.frame(&format!("junction-closed-{pass}"), cx)?;
            if *pass == "mask" {
                let [inset, ..] = self.window.update(cx, |view, window, _| {
                    view.fluid_glass_geometry_for_capture(window)
                })?;
                let scale = self.scene.dpi as f32 / 96.;
                let x = image.width() / 2;
                let y = ((self.scene.height - inset - 3.) * scale).round() as u32;
                require(
                    image.get_pixel(x, y).0[..3] == [255; 3],
                    "Диагностическая маска не доходит до плеера в координатах окна.",
                )?;
            }
        }
        self.window.update(cx, |view, _, cx| {
            view.set_fluid_shader_diagnostics_for_capture(0, false, cx);
            view.sample_fluid_glass_dock_for_capture(Some(PHASE), cx);
        })?;
        press(raw, &named(QUEUE_BUTTON), cx)?;
        for (index, millis) in [0, 90, 180, 270, 360].into_iter().enumerate() {
            self.window.update(cx, |view, _, cx| {
                view.sample_fluid_glass_dock_for_capture(
                    Some(PHASE + Duration::from_millis(millis)),
                    cx,
                );
            })?;
            self.frame(&format!("junction-dock-{:03}", index * 25), cx)?;
        }
        for (index, pass) in PASSES.iter().enumerate().skip(1) {
            self.window.update(cx, |view, _, cx| {
                view.set_fluid_shader_diagnostics_for_capture(index, false, cx);
            })?;
            self.frame(&format!("junction-open-{pass}"), cx)?;
        }
        for quality in QualityChoice::ALL {
            self.set_visual(VisualSettings { quality, ..visual }, cx)?;
            self.frame(&format!("junction-{}-open", quality.as_str()), cx)?;
        }
        self.window.update(cx, |view, _, cx| {
            view.set_fluid_shader_diagnostics_for_capture(0, false, cx);
            view.sample_fluid_glass_dock_for_capture(None, cx);
        })?;
        press(raw, &named(QUEUE_BUTTON), cx)
    }
}
