use std::f32::consts::PI;

use gpui::{App, Bounds, Hsla, Pixels, Point, TransformationMatrix, Window, size};

use super::{motion, tinted};

const OUTLINE: (&str, &[u8]) = ("heart.svg", include_bytes!("../../../../assets/heart.svg"));
const FILLED: (&str, &[u8]) = (
    "heart-filled.svg",
    include_bytes!("../../../../assets/heart-filled.svg"),
);

pub(super) fn paint(
    window: &mut Window,
    origin: Point<Pixels>,
    side: Pixels,
    progress: f32,
    ink: Hsla,
    cx: &App,
) {
    let fill = motion::smoothstep(progress);
    let scale = 1. + 0.08 * (PI * fill).sin().max(0.);
    let bounds = Bounds::new(origin, size(side, side));
    let center = bounds.center();
    let dpi = window.scale_factor();
    let transform = TransformationMatrix::unit()
        .translate(center.scale(dpi))
        .scale(size(scale, scale))
        .translate(center.scale(-dpi));
    for ((path, bytes), opacity) in [(OUTLINE, 1. - fill), (FILLED, fill)] {
        if opacity > 0.001
            && let Err(error) = window.paint_svg(
                bounds,
                path.into(),
                Some(bytes),
                transform,
                tinted(ink, opacity),
                cx,
            )
        {
            eprintln!("Could not draw {path}: {error}");
        }
    }
}
