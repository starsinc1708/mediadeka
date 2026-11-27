//! Settled geometry for the same glyphs used by animated controls.

use gpui::{AnyElement, App, Hsla, Pixels, canvas, point, prelude::*};
use guise::IconName;

use super::{
    AnimatedIcon, Glyph, STROKE, Shape, arc_points, line, paint_polygon, paint_stroke, stroke_path,
};

pub(super) fn render(name: IconName, side: Pixels, ink: Hsla) -> Option<AnyElement> {
    let (glyph, active) = match name {
        IconName::Play => (Glyph::PlayPause, false),
        IconName::Pause => (Glyph::PlayPause, true),
        IconName::SkipBack => (Glyph::Previous, false),
        IconName::SkipForward => (Glyph::Next, false),
        IconName::Shuffle => (Glyph::Shuffle, false),
        IconName::Repeat => (Glyph::Repeat, false),
        IconName::Repeat1 => (Glyph::RepeatOne, true),
        IconName::Volume2 => (Glyph::Volume, false),
        IconName::VolumeX => (Glyph::Volume, true),
        IconName::ListMusic => (Glyph::Queue, false),
        IconName::Captions => (Glyph::Lyrics, false),
        IconName::Maximize2 => (Glyph::Fullscreen, false),
        IconName::Heart => (Glyph::Like, false),
        IconName::Pin => (Glyph::Pin, false),
        IconName::Ellipsis => (Glyph::More, false),
        IconName::SlidersHorizontal => (Glyph::Filter, false),
        IconName::Copy => (Glyph::Copy, false),
        IconName::ChevronDown => (Glyph::ChevronDown, false),
        IconName::ChevronUp => (Glyph::ChevronDown, true),
        _ => {
            let shapes = static_shapes(name)?;
            return Some(
                icon_canvas(side, ink, move |window, origin, side, ink, _| {
                    for shape in &shapes {
                        match shape {
                            Shape::Fill(points, _) => {
                                paint_polygon(window, points, origin, side, ink)
                            }
                            Shape::Stroke(points, width, _) => {
                                paint_stroke(window, points, *width, origin, side, ink);
                            }
                        }
                    }
                })
                .into_any_element(),
            );
        }
    };
    Some(
        canvas_icon(
            AnimatedIcon::at(glyph, f32::from(u8::from(active))),
            side,
            ink,
        )
        .into_any_element(),
    )
}

pub(super) fn canvas_icon(icon: AnimatedIcon, side: Pixels, ink: Hsla) -> impl IntoElement {
    icon_canvas(side, ink, move |window, origin, side, ink, cx| {
        icon.paint(window, origin, side, ink, cx)
    })
}

pub(super) fn icon_canvas(
    side: Pixels,
    ink: Hsla,
    paint: impl Fn(&mut gpui::Window, gpui::Point<Pixels>, Pixels, Hsla, &App) + 'static,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, cx| {
            let side = bounds.size.width.min(bounds.size.height);
            let origin = point(
                bounds.origin.x + (bounds.size.width - side) * 0.5,
                bounds.origin.y + (bounds.size.height - side) * 0.5,
            );
            paint(window, origin, side, ink, cx);
        },
    )
    .size(side)
    .flex_none()
}

fn static_shapes(name: IconName) -> Option<Vec<Shape>> {
    use std::f32::consts::TAU;
    let stroke = |points: &[_]| stroke_path(points, STROKE, 1.);
    let circle = |center, radius| stroke(&arc_points(center, radius, 0., TAU, 32));
    Some(match name {
        IconName::Search => vec![
            circle([0.43, 0.43], 0.25),
            line([0.61, 0.61], [0.82, 0.82], STROKE, 1.),
        ],
        IconName::X => vec![
            line([0.28, 0.28], [0.72, 0.72], STROKE, 1.),
            line([0.72, 0.28], [0.28, 0.72], STROKE, 1.),
        ],
        IconName::Check => vec![stroke(&[[0.21, 0.52], [0.40, 0.71], [0.79, 0.31]])],
        IconName::House => vec![
            stroke(&[[0.12, 0.48], [0.50, 0.17], [0.88, 0.48]]),
            stroke(&[
                [0.22, 0.43],
                [0.22, 0.82],
                [0.42, 0.82],
                [0.42, 0.61],
                [0.58, 0.61],
                [0.58, 0.82],
                [0.78, 0.82],
                [0.78, 0.43],
            ]),
        ],
        IconName::History => vec![
            stroke(&arc_points([0.52, 0.5], 0.31, -2.55, 2.97, 30)),
            stroke(&[[0.24, 0.18], [0.24, 0.36], [0.42, 0.36]]),
            stroke(&[[0.52, 0.31], [0.52, 0.51], [0.66, 0.59]]),
        ],
        IconName::Disc3 => vec![
            circle([0.5, 0.5], 0.34),
            circle([0.5, 0.5], 0.085),
            stroke(&arc_points([0.5, 0.5], 0.225, -2.75, -1.95, 12)),
            stroke(&arc_points([0.5, 0.5], 0.225, 0.38, 1.18, 12)),
        ],
        IconName::Music2 => vec![
            circle([0.36, 0.68], 0.12),
            stroke(&[[0.48, 0.68], [0.48, 0.23], [0.76, 0.36]]),
        ],
        IconName::AudioLines => [0.16, 0.29, 0.47, 0.31, 0.18]
            .into_iter()
            .enumerate()
            .map(|(index, height)| {
                let x = 0.17 + (index as f32) * 0.165;
                line([x, 0.5 - height / 2.], [x, 0.5 + height / 2.], STROKE, 1.)
            })
            .collect(),
        _ => return None,
    })
}
