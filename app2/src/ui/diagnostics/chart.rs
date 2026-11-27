use gpui::{
    FontWeight, IntoElement, PathBuilder, SharedString, canvas, div, fill, point, prelude::*, px,
    rgb, size,
};

use super::{card, muted};
use crate::ui::theme::{DIAGNOSTICS_GRID, DIAGNOSTICS_TARGET};

pub(super) struct Series {
    name: &'static str,
    color: u32,
    values: Vec<Option<f64>>,
}

impl Series {
    pub(super) fn new(
        name: &'static str,
        color: u32,
        values: impl Iterator<Item = Option<f64>>,
    ) -> Self {
        Self {
            name,
            color,
            values: values.collect(),
        }
    }
}

pub(super) fn chart<const N: usize>(
    title: &'static str,
    unit: &'static str,
    target: Option<f64>,
    capacity: usize,
    series: [Series; N],
) -> impl IntoElement {
    let highest = series
        .iter()
        .flat_map(|series| series.values.iter().flatten().copied())
        .chain(target)
        .fold(1., f64::max);
    let ceiling = (highest * 1.15).ceil();
    let legend = div().flex().flex_wrap().gap(px(11.)).children(
        series
            .iter()
            .map(|series| legend_key(series.name, series.color, false))
            .chain(target.map(|value| {
                legend_key(
                    format!("Монитор · {value:.1} {unit}"),
                    DIAGNOSTICS_TARGET,
                    true,
                )
            })),
    );
    let plot = canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let grid = rgb(DIAGNOSTICS_GRID);
            for fraction in [0., 0.5, 1.] {
                let mut tick = bounds;
                tick.origin.y += (bounds.size.height - px(1.)) * fraction;
                tick.size.height = px(1.);
                window.paint_quad(fill(tick, grid));
            }
            let y = |value: f64| {
                bounds.origin.y + bounds.size.height * (1. - (value / ceiling).clamp(0., 1.)) as f32
            };
            if let Some(target) = target {
                let mut x = bounds.origin.x;
                while x < bounds.right() {
                    window.paint_quad(fill(
                        gpui::Bounds {
                            origin: point(x, y(target)),
                            size: size(px(6.).min(bounds.right() - x), px(2.)),
                        },
                        rgb(DIAGNOSTICS_TARGET),
                    ));
                    x += px(10.);
                }
            }
            for series in &series {
                let mut path = PathBuilder::stroke(px(1.5));
                let step = (bounds.size.width - px(1.)) / (capacity - 1) as f32;
                let first = capacity.saturating_sub(series.values.len());
                let mut has_previous = false;
                for (index, value) in series.values.iter().enumerate() {
                    if let Some(value) = value {
                        let position =
                            point(bounds.origin.x + step * (first + index) as f32, y(*value));
                        if has_previous {
                            path.line_to(position);
                        } else {
                            path.move_to(position);
                        }
                        has_previous = true;
                    } else {
                        has_previous = false;
                    }
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, rgb(series.color));
                }
            }
        },
    )
    .size_full();

    card()
        .flex_1()
        .min_w_0()
        .gap(px(7.))
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(14.))
                        .child(title),
                )
                .child(muted(unit)),
        )
        .child(
            div()
                .flex()
                .gap(px(8.))
                .h(px(86.))
                .child(div().flex_1().min_w_0().child(plot))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .items_end()
                        .justify_between()
                        .min_w(px(26.))
                        .flex_none()
                        .child(muted(format!("{ceiling:.0}")))
                        .child(muted("0")),
                ),
        )
        .child(legend)
}

fn legend_key(name: impl Into<SharedString>, color: u32, dashed: bool) -> gpui::Div {
    let stroke = |width| div().w(px(width)).h(px(2.)).bg(rgb(color));
    div()
        .flex()
        .items_center()
        .gap(px(5.))
        .child(if dashed {
            div().flex().gap(px(2.)).child(stroke(4.)).child(stroke(4.))
        } else {
            stroke(10.)
        })
        .child(muted(name))
}
