use std::sync::LazyLock;

use gpui::{
    Bounds, Corners, IntoElement, Pixels, Point, Shader, ShaderError, ShaderInstance, ShaderSource,
    Styled, Window, canvas, point, px, rgb, size,
};

use super::{ACCENTS, CONNECTION_RADIUS};

const TRANSFER_REACH: f32 = 86.0;

static FOG: LazyLock<Result<Shader, ShaderError>> = LazyLock::new(|| {
    Shader::new(ShaderSource::new(
        "Mediadeka provider fog",
        concat!(include_str!("optics.wgsl"), include_str!("fog.wgsl")),
    ))
});

#[derive(Clone, Copy)]
pub(in crate::ui) struct Transfer {
    pub progress: f32,
    pub assembly: f32,
    pub from: Point<Pixels>,
    pub to: Bounds<Pixels>,
    pub provider: usize,
    pub compact: bool,
}

pub(in crate::ui) fn transfer_fog(transfer: Transfer) -> impl IntoElement {
    fog_canvas(move |bounds, window, shader| {
        let Some(&accent) = ACCENTS.get(transfer.provider) else {
            return;
        };
        let progress = transfer.progress.clamp(0.0, 1.0);
        if progress <= 0.0 || progress >= 1.0 {
            return;
        }
        let source = bounds.origin + transfer.from;
        let pane = Bounds::new(bounds.origin + transfer.to.origin, transfer.to.size);
        let entry = fog_entry(source, pane, transfer.compact);
        let path = Bounds::from_corners(
            point(source.x.min(entry.x), source.y.min(entry.y)),
            point(source.x.max(entry.x), source.y.max(entry.y)),
        );
        let quad = snap_quad(
            pane.union(&path).dilate(px(TRANSFER_REACH)),
            window.scale_factor(),
        );
        let pane_origin = pane.origin - quad.origin;
        let stage_top = f32::from(bounds.top() - quad.top());
        let stage_bottom = f32::from(bounds.bottom() - quad.top());
        let instance = ShaderInstance::at(progress)
            .params(0, &route_coordinates(quad, source, entry))
            .params(
                4,
                &[
                    f32::from(pane_origin.x),
                    f32::from(pane_origin.y),
                    f32::from(pane.size.width),
                    f32::from(pane.size.height),
                ],
            )
            .params(
                8,
                &[
                    1.0,
                    progress,
                    transfer.assembly.clamp(0.0, 1.0),
                    stage_bottom,
                ],
            )
            .params(
                12,
                &[
                    f32::from(u8::from(transfer.compact)),
                    CONNECTION_RADIUS,
                    transfer.provider as f32 * 1.37,
                    stage_top,
                ],
            )
            .color(0, rgb(accent));
        window.paint_shader(quad, Corners::default(), shader, instance);
    })
}

fn fog_entry(source: Point<Pixels>, pane: Bounds<Pixels>, compact: bool) -> Point<Pixels> {
    if compact {
        let toward_center = pane.center() - source;
        let offset_x = f32::from(toward_center.x).clamp(-46.0, 46.0);
        let offset_y = f32::from(toward_center.y).clamp(-26.0, 38.0);
        return source + point(px(31.0 + offset_x * 0.5), px(offset_y - 20.0));
    }
    let height = pane.size.height * 0.72;
    let margin = (pane.size.height - height) * 0.5;
    point(
        pane.left() + px(4.0),
        source.y.clamp(pane.top() + margin, pane.bottom() - margin),
    )
}

fn route_coordinates(quad: Bounds<Pixels>, source: Point<Pixels>, end: Point<Pixels>) -> [f32; 4] {
    let source = source - quad.origin;
    let end = end - quad.origin;
    [
        f32::from(source.x),
        f32::from(source.y),
        f32::from(end.x),
        f32::from(end.y),
    ]
}

fn snap_quad(bounds: Bounds<Pixels>, scale: f32) -> Bounds<Pixels> {
    let left = (f32::from(bounds.left()) * scale).floor() / scale;
    let top = (f32::from(bounds.top()) * scale).floor() / scale;
    let right = (f32::from(bounds.right()) * scale).ceil() / scale;
    let bottom = (f32::from(bounds.bottom()) * scale).ceil() / scale;
    Bounds::new(
        point(px(left), px(top)),
        size(px(right - left), px(bottom - top)),
    )
}

fn fog_canvas(
    paint: impl FnOnce(Bounds<Pixels>, &mut Window, &Shader) + 'static,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            if let Ok(shader) = &*FOG {
                paint(bounds, window, shader);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

#[cfg(all(test, windows))]
mod tests {
    use super::FOG;

    #[test]
    fn fog_shader_translates_to_direct3d() {
        let shader = FOG.as_ref().expect("valid provider fog WGSL");
        gpui::shader_program(shader.id())
            .expect("registered provider fog shader")
            .to_hlsl()
            .expect("provider fog translated to HLSL");
    }
}
