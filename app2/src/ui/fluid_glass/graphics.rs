//! Paints the independent visual layers behind the GPUI foreground.

pub(super) mod edge;
mod fallback;
mod row_launch;
pub(super) mod studio;

use std::sync::LazyLock;

use gpui::{
    Bounds, Corners, Hsla, IntoElement, LayerFilter, Pixels, Rgba, Shader, ShaderError,
    ShaderInstance, ShaderSource, Styled, Window, canvas, point, px, rgb, size,
};

use crate::visual::{SurfaceAppearance, VisualFlags};

use super::{
    Geometry,
    audio::modulation::Modulation,
    contextual::{ArtworkPlacement, PatternMix},
    dock::Surface,
    palette::{Field, Palette},
    quality::Quality,
    reactivity::Waveform,
    tokens::{ARTIST_STAGE_MAX_WIDTH, CANVAS, SIDEBAR, TITLEBAR, atmosphere},
    track_change::RowFlight,
    waveform,
};

const SHADER_FLAGS: usize = 5;
const RASTER_PASS: u32 = 32;
const EDGE_PASS: u32 = 64;
const PREVIEW_PASS: u32 = 96;
const PULSE_SHIFT: u32 = 7;
// Bits 7-13 pulse, bit 14 spatial palette, bits 15-20 dock, bits 21-23 raster.
const SPATIAL_BIT: u32 = 1 << 14;
const DOCK_SHIFT: u32 = 15;
const RASTER_SHIFT: u32 = 21;
const HERO_LIGHT_STRIDE: f32 = 32.;
// The inner shell radius is below 64 DIP, leaving two quantized controls
// in the same exact f32 integer range without another uniform allocation.
const SURFACE_STRIDE: f32 = 64.;

#[cfg(feature = "capture")]
struct FullEdgeReference(bool);

#[cfg(feature = "capture")]
impl gpui::Global for FullEdgeReference {}

#[cfg(feature = "capture")]
pub(super) fn set_full_edge_for_capture(full: bool, cx: &mut gpui::App) {
    cx.set_global(FullEdgeReference(full));
}

#[derive(Clone, Copy)]
pub(super) struct Reactivity {
    pub(super) audio: Modulation,
    pub(super) pulse: f32,
    pub(super) exposure: f32,
    pub(super) artwork_placement: ArtworkPlacement,
    pub(super) hero_light: f32,
    pub(super) trace: Option<Waveform>,
    pub(super) pattern: PatternMix,
    pub(super) flags: VisualFlags,
    pub(super) surface: SurfaceAppearance,
    pub(super) row_flight: Option<RowFlight>,
}

#[derive(Clone, Copy)]
pub(super) enum Material {
    Procedural(Quality),
    Flat { high_contrast: bool },
}

static SHADER: LazyLock<Result<Shader, ShaderError>> = LazyLock::new(|| {
    let tint = rgb(SIDEBAR);
    let canvas = rgb(CANVAS);
    let source = format!(
        "const GLASS_TINT: vec3<f32> = vec3<f32>({:.8}, {:.8}, {:.8});\n\
         const NEUTRAL_CANVAS: vec3<f32> = vec3<f32>({:.8}, {:.8}, {:.8});\n\
         const TITLEBAR_HEIGHT: f32 = {:.2};\n\
         const OPTICAL_EDGE_LIMIT: f32 = {:.1};\n\
         const ARTIST_STAGE_MAX_WIDTH: f32 = {:.1};\n\
         const LIBRARY_EXPOSURE: f32 = {:.2};\n\
         const HERO_LIGHT_STRIDE: f32 = {:.1};\n\
         const SURFACE_STRIDE: f32 = {:.1};\n\
         const DEBUG_MODE_STRIDE: f32 = {:.1};\n\
         const STUDIO_SCENE: u32 = {}u;\n\
         const STUDIO_SIZE: vec2<f32> = vec2<f32>({:.1}, {:.1});\n{}",
        tint.r,
        tint.g,
        tint.b,
        canvas.r,
        canvas.g,
        canvas.b,
        TITLEBAR,
        edge::EDGE_LIMIT,
        ARTIST_STAGE_MAX_WIDTH,
        atmosphere::LIBRARY,
        HERO_LIGHT_STRIDE,
        SURFACE_STRIDE,
        studio::MODE_STRIDE,
        studio::STUDIO_SCENE,
        studio::TEST_SIZE.0,
        studio::TEST_SIZE.1,
        concat!(
            include_str!("graphics/shell_mask.wgsl"),
            include_str!("graphics/common/visual.wgsl"),
            include_str!("graphics/patterns/thermal_flow.wgsl"),
            include_str!("graphics/patterns/fluted_glass.wgsl"),
            include_str!("graphics/patterns/interference.wgsl"),
            include_str!("graphics/patterns/signal_distortion.wgsl"),
            include_str!("graphics/patterns/optical_caustics.wgsl"),
            include_str!("graphics/patterns/light_leak.wgsl"),
            include_str!("graphics/patterns/gem_smoke.wgsl"),
            include_str!("graphics/patterns/swirl.wgsl"),
            include_str!("graphics/patterns/liquid_metal.wgsl"),
            include_str!("graphics/patterns/registry.wgsl"),
            include_str!("graphics/atmosphere.wgsl"),
            include_str!("graphics/distortion.wgsl"),
            include_str!("graphics/material.wgsl"),
            include_str!("graphics/reading_light.wgsl"),
            include_str!("graphics/composite.wgsl"),
        ),
    );
    let shader = Shader::new(ShaderSource::new("Mediadeka Fluid Glass", source));
    if let Err(error) = &shader {
        eprintln!("Не удалось зарегистрировать материал Fluid Glass: {error}");
    }
    shader
});

pub(super) fn layer(
    geometry: Geometry,
    dock: Surface,
    seconds: f32,
    animated: bool,
    palette: Field,
    material: Material,
    reactivity: Reactivity,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _cx| {
            if let Material::Procedural(quality) = material
                && let Ok(shader) = SHADER.as_ref()
            {
                #[cfg(feature = "capture")]
                let reactivity = studio::capture_reactivity(reactivity, _cx);
                let instance = material_instance(
                    geometry, dock, seconds, animated, palette, reactivity, quality,
                );
                #[cfg(feature = "capture")]
                if studio::capture_pass(_cx) != studio::Pass::Composite {
                    window.paint_shader(
                        bounds,
                        Corners::default(),
                        shader,
                        studio::capture_instance(instance, _cx),
                    );
                    return;
                }
                let scale = quality.raster_scale();
                paint_material(bounds, scale, shader, instance, window);
                if scale != 1. {
                    let flags = instance.params[SHADER_FLAGS];
                    let instance = instance.param(SHADER_FLAGS, flags + EDGE_PASS as f32);
                    #[cfg(feature = "capture")]
                    let full_edge = _cx
                        .try_global::<FullEdgeReference>()
                        .is_some_and(|mode| mode.0);
                    #[cfg(not(feature = "capture"))]
                    let full_edge = false;
                    if full_edge {
                        window.paint_shader(bounds, Corners::default(), shader, instance);
                    } else {
                        edge::paint(bounds, geometry, dock.width, shader, instance, window);
                    }
                }
                if let Some(trace) = reactivity.trace {
                    waveform::paint(bounds, geometry, palette.player, trace, window);
                }
                if let Some(flight) = reactivity.row_flight {
                    row_launch::paint(bounds, geometry, palette.player, flight, window);
                }
            } else {
                let high_contrast = matches!(
                    material,
                    Material::Flat {
                        high_contrast: true
                    }
                );
                fallback::paint(geometry, dock.width, high_contrast, bounds, window);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

fn paint_material(
    bounds: Bounds<Pixels>,
    raster_scale: f32,
    shader: &Shader,
    instance: ShaderInstance,
    window: &mut Window,
) {
    if raster_scale == 1. {
        window.paint_shader(bounds, Corners::default(), shader, instance);
        return;
    }

    // The shader quad and filter source must occupy the same physical texels.
    // The parent clip trims the final partial pixel after upsampling.
    let dpi = window.scale_factor();
    let raster_length = |edge: Pixels| px((f32::from(edge) * dpi * raster_scale).ceil() / dpi);
    let source = Bounds {
        origin: bounds.origin,
        size: size(
            raster_length(bounds.size.width),
            raster_length(bounds.size.height),
        ),
    };
    let filter = LayerFilter {
        scale: Some(raster_scale.recip()),
        scale_origin: Some(point(0., 0.)),
        ..LayerFilter::default()
    };
    let flags = instance.params[SHADER_FLAGS];
    window.with_filter(source, filter, Pixels::ZERO, |window| {
        window.paint_shader(
            source,
            Corners::default(),
            shader,
            instance.param(SHADER_FLAGS, flags + RASTER_PASS as f32),
        );
    });
}

fn material_instance(
    geometry: Geometry,
    dock: Surface,
    seconds: f32,
    animated: bool,
    palette: Field,
    reactivity: Reactivity,
    quality: Quality,
) -> ShaderInstance {
    let pattern = reactivity.pattern;
    let params = pattern.params;
    let [warm, mid, cool, highlight] = palette.player.colors.map(|color| rgb(rgb24(color)));
    let spatial = palette.player != palette.upper;
    let pulse = (reactivity.pulse * 127.).round() as u32;
    let dock_strength = (dock.strength * 63.).round() as u32;
    let raster_step = (quality.raster_scale() * 8.).round() as u32 - 1;
    let animation = u32::from(animated)
        | (u32::from(animated && quality.glitch() && pulse > 0) << 1)
        | (quality.warp_octaves().clamp(1, 2) << 2)
        | (pulse << PULSE_SHIFT)
        | (u32::from(spatial) * SPATIAL_BIT)
        | (dock_strength << DOCK_SHIFT)
        | ((raster_step & 7) << RASTER_SHIFT);
    let audio = reactivity.audio;
    let lane0 = quantize(audio.bass, 31)
        | quantize(audio.mid, 31) << 5
        | quantize(audio.treble, 31) << 10
        | quantize(audio.light, 31) << 15
        | quantize(audio.reflection, 15) << 20;
    let exposure = reactivity.artwork_placement as u32
        | quantize(reactivity.exposure, 255) << 2
        | quantize(audio.dispersion, 31) << 10
        | quantize(audio.pulse, 31) << 15
        | quantize(audio.glitch, 15) << 20;
    let lane1 = pack_six([
        quantize(params.scale / 2.6, 63),
        quantize(params.speed, 63),
        quantize(params.rotation / std::f32::consts::TAU, 63),
        quantize(params.intensity, 63),
    ]);
    let lane2 = pack_six([
        quantize(params.distortion, 63),
        quantize(params.softness, 63),
        quantize(params.complexity, 63),
        quantize(pattern.mix, 63),
    ]);
    let lane3 = quantize(params.seed as f32 / 255., 255)
        | (pattern.a.shader_id() & 15) << 8
        | (pattern.b.shader_id() & 15) << 12
        | (reactivity.flags.bits() as u32) << 16;
    let lane3 = lane3 as f32 / FIELD_WORD_SCALE;
    let percent = |value: u8| quantize(f32::from(value) / 100., 63);
    let surface = percent(reactivity.surface.opacity) | percent(reactivity.surface.blur) << 6;
    ShaderInstance::at(seconds)
        .params(
            0,
            &[
                geometry.inset,
                geometry.rail,
                geometry.player,
                geometry.outer_radius
                    + quantize(reactivity.hero_light, 255) as f32 * HERO_LIGHT_STRIDE,
            ],
        )
        .params(
            4,
            &[
                geometry.inner_radius + surface as f32 * SURFACE_STRIDE,
                animation as f32,
                dock.width,
                exposure as f32,
            ],
        )
        .params(
            8,
            &[
                rgb24(palette.player.shadow) as f32,
                rgb24(palette.player.reflection) as f32,
                rgb24(palette.upper.shadow) as f32,
                rgb24(palette.upper.reflection) as f32,
            ],
        )
        .params(12, &palette.upper.colors.map(|color| rgb24(color) as f32))
        .color(0, Hsla::from(warm).alpha(lane0 as f32 / FIELD_WORD_SCALE))
        .color(1, Hsla::from(mid).alpha(lane1))
        .color(2, Hsla::from(cool).alpha(lane2))
        .color(3, Hsla::from(highlight).alpha(lane3))
}

pub(in crate::ui) fn preview_instance(
    palette: Palette,
    pattern: PatternMix,
    flags: VisualFlags,
    quality: Quality,
    seconds: f32,
) -> ShaderInstance {
    let field = Field::uniform(palette);
    let reactivity = Reactivity {
        audio: Modulation::default(),
        pulse: 0.,
        exposure: 0.35,
        artwork_placement: ArtworkPlacement::None,
        hero_light: 1.,
        trace: None,
        pattern,
        flags,
        surface: SurfaceAppearance::default(),
        row_flight: None,
    };
    let geometry = Geometry::preview();
    let instance = material_instance(
        geometry,
        Surface::default(),
        seconds,
        true,
        field,
        reactivity,
        quality,
    );
    let base = instance.params[SHADER_FLAGS];
    instance.param(SHADER_FLAGS, base + PREVIEW_PASS as f32)
}

pub(in crate::ui) fn preview_shader() -> Option<&'static Shader> {
    SHADER.as_ref().ok()
}

const FIELD_WORD_SCALE: f32 = 16_777_216.;

fn quantize(value: f32, max: u32) -> u32 {
    (value.clamp(0., 1.) * max as f32).round() as u32
}

fn pack_six(values: [u32; 4]) -> f32 {
    (values[0] | (values[1] << 6) | (values[2] << 12) | (values[3] << 18)) as f32 / FIELD_WORD_SCALE
}

fn rgb24(color: Rgba) -> u32 {
    let channel = |value: f32| (value * 255.).round() as u32;
    (channel(color.r) << 16) | (channel(color.g) << 8) | channel(color.b)
}

#[cfg(all(test, windows))]
mod tests {
    use super::{DOCK_SHIFT, PULSE_SHIFT, RASTER_SHIFT, SHADER, SPATIAL_BIT};

    #[test]
    fn animation_word_fields_do_not_overlap() {
        assert_eq!(SPATIAL_BIT, 1 << 14);
        assert_eq!(DOCK_SHIFT, 15);
        assert_eq!(PULSE_SHIFT, 7);
        assert_eq!(RASTER_SHIFT, 21);
        let word = SPATIAL_BIT | (5 << DOCK_SHIFT) | (17 << PULSE_SHIFT) | (3 << RASTER_SHIFT);
        assert_ne!(word & SPATIAL_BIT, 0);
        assert_eq!((word >> DOCK_SHIFT) & 63, 5);
        assert_eq!((word >> PULSE_SHIFT) & 127, 17);
        assert_eq!((word >> RASTER_SHIFT) & 7, 3);
    }

    #[test]
    fn fluid_glass_translates_to_direct3d() {
        let shader = SHADER.as_ref().expect("valid Fluid Glass WGSL");
        let hlsl = gpui::shader_program(shader.id())
            .expect("registered Fluid Glass shader")
            .to_hlsl()
            .expect("Fluid Glass translated to HLSL");
        std::fs::create_dir_all("target/acceptance").unwrap();
        std::fs::write("target/acceptance/fluid-glass.hlsl", &hlsl.source).unwrap();
        eprintln!("Fluid Glass HLSL: {} bytes", hlsl.source.len());
    }
}
