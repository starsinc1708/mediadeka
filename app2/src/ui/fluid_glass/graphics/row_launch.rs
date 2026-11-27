//! Small, short-lived optical patch on the shared background.

use std::sync::LazyLock;

use gpui::{
    Bounds, Corners, Pixels, Point, Shader, ShaderError, ShaderInstance, ShaderSource, Window,
    point, px, size,
};

use crate::ui::fluid_glass::{Geometry, palette::Palette, track_change::RowFlight};

static SHADER: LazyLock<Result<Shader, ShaderError>> = LazyLock::new(|| {
    let shader = Shader::new(ShaderSource::new(
        "Mediadeka Fluid Glass row light",
        include_str!("row_launch.wgsl"),
    ));
    if let Err(error) = &shader {
        eprintln!("Не удалось зарегистрировать световой импульс Fluid Glass: {error}");
    }
    shader
});

pub(super) fn paint(
    bounds: Bounds<Pixels>,
    geometry: Geometry,
    palette: Palette,
    flight: RowFlight,
    window: &mut Window,
) {
    let Ok(shader) = SHADER.as_ref() else {
        return;
    };
    let from = flight.origin;
    let landing = point(
        bounds.origin.x + px(geometry.inset + geometry.rail + 0.43 * geometry.inner_radius),
        bounds.bottom() - px(geometry.inset + 0.52 * geometry.player),
    );
    let (center, direction, impact) = placement(from, landing, flight.phase);
    let reach = px(118. + 20. * impact);
    let patch = Bounds {
        origin: center - point(reach, reach),
        size: size(reach * 2., reach * 2.),
    };
    window.paint_shader(
        patch,
        Corners::default(),
        shader,
        ShaderInstance::default()
            .params(0, &[direction.x, direction.y, flight.phase, impact])
            .color(0, gpui::Hsla::from(palette.colors[3]))
            .color(1, gpui::Hsla::from(palette.reflection)),
    );
}

fn placement(
    from: Point<Pixels>,
    to: Point<Pixels>,
    phase: f32,
) -> (Point<Pixels>, Point<f32>, f32) {
    let mix =
        |start: Point<Pixels>, end: Point<Pixels>, fraction: f32| start + (end - start) * fraction;
    let ease = |value: f32| {
        let value = value.clamp(0., 1.);
        value * value * (3. - 2. * value)
    };
    // Let the source light settle into the selected row before it starts moving.
    let travel = ease((phase - 0.10) / 0.76);
    let impact = ease((phase - 0.74) / 0.16);
    let control = point(
        from.x + (to.x - from.x) * 0.16,
        to.y - ((to.y - from.y) * 0.34).min(px(148.)),
    );
    let a = mix(from, control, travel);
    let b = mix(control, to, travel);
    let center = mix(a, b, travel);
    let dx = f32::from(b.x - a.x);
    let dy = f32::from(b.y - a.y) * (1. - impact);
    let length = dx.hypot(dy).max(0.001);
    (center, point(dx / length, dy / length), impact)
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn row_light_translates_to_direct3d() {
        let shader = super::SHADER.as_ref().expect("valid row light WGSL");
        gpui::shader_program(shader.id())
            .expect("registered row light")
            .to_hlsl()
            .expect("row light translated to HLSL");
    }
}
