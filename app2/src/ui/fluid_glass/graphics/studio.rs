//! Inspection views use the same material and optical uniforms as the shell.

use gpui::ShaderInstance;
#[cfg(feature = "capture")]
use gpui::{Corners, Div, Styled, canvas, div, prelude::*, px};

#[cfg(feature = "capture")]
use super::{FIELD_WORD_SCALE, preview_shader};
use super::{
    Geometry, HERO_LIGHT_STRIDE, PatternMix, Quality, Reactivity, Surface, material_instance,
};
use crate::{
    ui::fluid_glass::palette::{Field, Palette},
    visual::VisualFlags,
};

pub(super) const TEST_SIZE: (f32, f32) = (800., 400.);
// The flags sit above the radius and its 8-bit Hero light multiplier.
pub(super) const MODE_STRIDE: f32 = HERO_LIGHT_STRIDE * 256.;
pub(super) const STUDIO_SCENE: u32 = 8;
#[cfg(feature = "capture")]
pub(in crate::ui) const TEST_PULSE: f32 = 0.29;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u32)]
#[cfg_attr(not(feature = "capture"), allow(dead_code))]
pub(in crate::ui) enum Pass {
    #[default]
    Composite,
    Distance,
    Normals,
    Mask,
    Uv,
    Refraction,
    Edge,
}

impl Pass {
    #[cfg(feature = "capture")]
    pub(in crate::ui) const ALL: [Self; 7] = [
        Self::Composite,
        Self::Distance,
        Self::Normals,
        Self::Mask,
        Self::Uv,
        Self::Refraction,
        Self::Edge,
    ];

    #[cfg(feature = "capture")]
    pub(in crate::ui) const fn label(self) -> &'static str {
        match self {
            Self::Composite => "Композит",
            Self::Distance => "SDF Distance",
            Self::Normals => "Surface Normals",
            Self::Mask => "Material Mask",
            Self::Uv => "UV Coordinates",
            Self::Refraction => "Refraction Offset",
            Self::Edge => "Edge Lighting",
        }
    }

    fn apply(self, instance: ShaderInstance, studio: bool) -> ShaderInstance {
        let mode = self as u32 + if studio { STUDIO_SCENE } else { 0 };
        instance.param(3, instance.params[3] + MODE_STRIDE * mode as f32)
    }
}

pub(in crate::ui) const EFFECTS: [(u8, &str); 7] = [
    (VisualFlags::FLUTED, "Флютинг"),
    (VisualFlags::REFRACTION, "Преломление"),
    (VisualFlags::DISPERSION, "Дисперсия"),
    (VisualFlags::GRAIN, "Зерно"),
    (VisualFlags::BLOOM, "Свечение"),
    (VisualFlags::GLITCH, "Глитч"),
    (VisualFlags::VIGNETTE, "Виньетка"),
];

#[cfg(feature = "capture")]
pub(in crate::ui) fn comparison_flags(flag: u8) -> (VisualFlags, VisualFlags) {
    let base = if flag == VisualFlags::DISPERSION {
        VisualFlags::REFRACTION
    } else {
        0
    };
    (
        VisualFlags::from_bits(base),
        VisualFlags::from_bits(base | flag),
    )
}

#[cfg(feature = "capture")]
pub(in crate::ui::fluid_glass) fn tile(
    palette: Palette,
    pattern: PatternMix,
    flags: VisualFlags,
    quality: Quality,
    seconds: f32,
    pass: Pass,
) -> (Div, String) {
    let instance = scene_instance(
        palette,
        pattern,
        flags,
        quality,
        seconds,
        quality.frames_per_second() > 0,
        (TEST_PULSE, Default::default()),
    );
    let instance = pass.apply(instance, true);
    let view = div()
        .w_full()
        .h(px(142.))
        .rounded(px(8.))
        .overflow_hidden()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    if let Some(shader) = preview_shader() {
                        window.paint_shader(bounds, Corners::all(px(8.)), shader, instance);
                    }
                },
            )
            .size_full(),
        );
    let word = |lane: usize| (instance.colors[lane].a * FIELD_WORD_SCALE).round() as u32;
    let field = |lane, shift| ((word(lane) >> shift) & 63_u32) as f32 / 63.;
    let uniforms = format!(
        "{} · seed {} · scale {:.2} · rotation {:.2} · intensity {:.2} · speed {:.2} · distortion {:.2} · softness {:.2} · complexity {:.2} · flags {:#04x} · {} октав",
        pattern.a.as_str(),
        word(3) & 255,
        field(1, 0) * 2.6,
        field(1, 12) * std::f32::consts::TAU,
        field(1, 18),
        field(1, 6),
        field(2, 0),
        field(2, 6),
        field(2, 12),
        flags.bits(),
        quality.warp_octaves(),
    );
    (view, uniforms)
}

pub(in crate::ui::fluid_glass) fn live_preview(
    palette: Palette,
    pattern: PatternMix,
    flags: VisualFlags,
    quality: Quality,
    seconds: f32,
    animated: bool,
    surface: crate::visual::SurfaceAppearance,
) -> ShaderInstance {
    Pass::Composite.apply(
        scene_instance(
            palette,
            pattern,
            flags,
            quality,
            seconds,
            animated,
            (0., surface),
        ),
        true,
    )
}

fn scene_instance(
    palette: Palette,
    pattern: PatternMix,
    flags: VisualFlags,
    quality: Quality,
    seconds: f32,
    animated: bool,
    (pulse, surface): (f32, crate::visual::SurfaceAppearance),
) -> ShaderInstance {
    let reactivity = Reactivity {
        audio: Default::default(),
        pulse,
        exposure: crate::ui::fluid_glass::tokens::atmosphere::LIBRARY,
        artwork_placement: Default::default(),
        hero_light: 1.,
        trace: None,
        pattern,
        flags,
        surface,
        row_flight: None,
    };
    material_instance(
        Geometry::for_size(TEST_SIZE.0, TEST_SIZE.1),
        Surface::default(),
        seconds,
        animated,
        Field::uniform(palette),
        reactivity,
        quality,
    )
}

#[cfg(feature = "capture")]
#[derive(Clone, Copy, Default)]
struct Capture {
    pass: Pass,
    pulse: Option<f32>,
}

#[cfg(feature = "capture")]
impl gpui::Global for Capture {}

#[cfg(feature = "capture")]
pub(in crate::ui::fluid_glass) fn set_capture(pass: Pass, test_pulse: bool, cx: &mut gpui::App) {
    cx.set_global(Capture {
        pass,
        pulse: test_pulse.then_some(TEST_PULSE),
    });
}

#[cfg(feature = "capture")]
pub(super) fn capture_pass(cx: &gpui::App) -> Pass {
    cx.try_global::<Capture>()
        .map_or(Pass::Composite, |state| state.pass)
}

#[cfg(feature = "capture")]
pub(super) fn capture_reactivity(mut reactivity: Reactivity, cx: &gpui::App) -> Reactivity {
    if let Some(pulse) = cx.try_global::<Capture>().and_then(|state| state.pulse) {
        reactivity.pulse = pulse;
    }
    reactivity
}

#[cfg(feature = "capture")]
pub(super) fn capture_instance(instance: ShaderInstance, cx: &gpui::App) -> ShaderInstance {
    capture_pass(cx).apply(instance, false)
}
