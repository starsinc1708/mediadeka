//! Native decorative trace of recent PCM amplitude in the player glass.

use std::sync::LazyLock;

use gpui::{
    Bounds, Corners, Hsla, Pixels, Shader, ShaderError, ShaderInstance, ShaderSource, Window,
    point, px, size,
};

use super::{Geometry, audio::ENVELOPE_POINTS, palette::Palette, reactivity::Waveform, tokens};

const POINTS_PER_SIDE: usize = ENVELOPE_POINTS / 2;
const _: () = assert!(POINTS_PER_SIDE == gpui::SHADER_PARAM_COUNT);

static SHADER: LazyLock<Result<Shader, ShaderError>> = LazyLock::new(|| {
    let shader = Shader::new(ShaderSource::new(
        "Mediadeka Fluid Glass waveform",
        include_str!("graphics/waveform.wgsl"),
    ));
    if let Err(error) = &shader {
        eprintln!("Не удалось зарегистрировать звуковой след Fluid Glass: {error}");
    }
    shader
});

pub(super) fn paint(
    bounds: Bounds<Pixels>,
    geometry: Geometry,
    palette: Palette,
    trace: Waveform,
    window: &mut Window,
) {
    if bounds.size.width < px(tokens::WAVEFORM_MIN_WIDTH)
        || bounds.size.height < px(tokens::WAVEFORM_MIN_HEIGHT)
    {
        return;
    }
    let Ok(shader) = SHADER.as_ref() else { return };
    let width = (bounds.size.width * 0.105).min(px(tokens::WAVEFORM_MAX_WIDTH));
    let height = px(tokens::WAVEFORM_HEIGHT);
    let center = bounds.center().x;
    let gap = px(tokens::WAVEFORM_CENTER_GAP);
    let top = bounds.bottom()
        - px(geometry.inset() + geometry.player() * tokens::WAVEFORM_CENTER_FROM_BOTTOM)
        - height * 0.5;
    for ((samples, x), [first, last]) in trace
        .envelope
        .as_chunks::<POINTS_PER_SIDE>()
        .0
        .iter()
        .zip([center - gap - width, center + gap])
        .zip([
            [palette.colors[0], palette.colors[1]],
            [palette.colors[1], palette.colors[2]],
        ])
    {
        let instance = ShaderInstance::default()
            .params(0, samples)
            .color(0, Hsla::from(first).alpha(trace.opacity))
            .color(1, last)
            .color(2, palette.colors[3]);
        window.paint_shader(
            Bounds {
                origin: point(x, top),
                size: size(width, height),
            },
            Corners::default(),
            shader,
            instance,
        );
    }
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn waveform_translates_to_direct3d() {
        let shader = super::SHADER.as_ref().expect("valid waveform WGSL");
        gpui::shader_program(shader.id())
            .expect("registered waveform shader")
            .to_hlsl()
            .expect("waveform translated to HLSL");
    }
}
