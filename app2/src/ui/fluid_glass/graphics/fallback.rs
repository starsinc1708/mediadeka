use gpui::{BorderStyle, Bounds, PathBuilder, Pixels, Window, point, px, quad, rgb};

use super::super::{
    Geometry,
    tokens::{SIDEBAR, TITLEBAR, contrast},
};

pub(super) fn paint(
    geometry: Geometry,
    dock_width: f32,
    high_contrast: bool,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let inset = px(geometry.inset);
    let outer = bounds.inset(inset);
    if geometry.rail == 0. {
        let height = px(geometry.player);
        if height > Pixels::ZERO && height < outer.size.height {
            let player = Bounds::from_corners(
                point(outer.left(), outer.bottom() - height),
                point(outer.right(), outer.bottom()),
            );
            let (color, border) = if high_contrast {
                (contrast::SURFACE, px(contrast::SHELL_STROKE))
            } else {
                (SIDEBAR, px(0.))
            };
            window.paint_quad(quad(
                player,
                px(geometry.outer_radius),
                rgb(color),
                border,
                rgb(contrast::RULE),
                BorderStyle::Solid,
            ));
        }
        return;
    }
    if outer.size.width <= px(geometry.rail) || outer.size.height <= px(geometry.player) {
        return;
    }
    let rail_right = outer.left() + px(geometry.rail);
    let player_top = outer.bottom() - px(geometry.player);
    let corner = px(geometry
        .outer_radius
        .min(geometry.player.min(geometry.rail) * 0.5));
    let junction = px(geometry.inner_radius)
        .min(outer.right() - rail_right)
        .min(player_top - outer.top());
    let contour = |builder| {
        shell_path(
            builder,
            outer,
            rail_right,
            player_top,
            corner,
            junction,
            px(dock_width),
        )
    };
    if let Ok(path) = contour(PathBuilder::fill()).build() {
        window.paint_path(
            path,
            rgb(if high_contrast {
                contrast::SURFACE
            } else {
                SIDEBAR
            }),
        );
    }
    if high_contrast
        && let Ok(path) = contour(PathBuilder::stroke(px(contrast::SHELL_STROKE))).build()
    {
        window.paint_path(path, rgb(contrast::RULE));
    }
}

fn shell_path(
    mut contour: PathBuilder,
    outer: Bounds<Pixels>,
    rail_right: Pixels,
    player_top: Pixels,
    corner: Pixels,
    junction: Pixels,
    dock_width: Pixels,
) -> PathBuilder {
    let arc = |path: &mut PathBuilder, x, y, radius, convex| {
        path.arc_to(point(radius, radius), px(0.), false, convex, point(x, y));
    };

    contour.move_to(point(outer.left() + corner, outer.top()));
    contour.line_to(point(rail_right - corner, outer.top()));
    arc(&mut contour, rail_right, outer.top() + corner, corner, true);
    contour.line_to(point(rail_right, player_top - junction));
    arc(
        &mut contour,
        rail_right + junction,
        player_top,
        junction,
        false,
    );
    let dock_top = outer.top() + px(TITLEBAR);
    if dock_width > Pixels::ZERO && player_top > dock_top {
        let dock_left = outer.right() - dock_width;
        let cap = (corner * 0.5).min(dock_width * 0.5);
        let bend = junction
            .min((dock_left - rail_right) * 0.5)
            .min(player_top - dock_top - cap)
            .max(Pixels::ZERO);
        contour.line_to(point(dock_left - bend, player_top));
        arc(&mut contour, dock_left, player_top - bend, bend, false);
        contour.line_to(point(dock_left, dock_top + cap));
        arc(&mut contour, dock_left + cap, dock_top, cap, true);
        contour.line_to(point(outer.right() - cap, dock_top));
        arc(&mut contour, outer.right(), dock_top + cap, cap, true);
    } else {
        contour.line_to(point(outer.right() - corner, player_top));
        arc(
            &mut contour,
            outer.right(),
            player_top + corner,
            corner,
            true,
        );
    }
    contour.line_to(point(outer.right(), outer.bottom() - corner));
    arc(
        &mut contour,
        outer.right() - corner,
        outer.bottom(),
        corner,
        true,
    );
    contour.line_to(point(outer.left() + corner, outer.bottom()));
    arc(
        &mut contour,
        outer.left(),
        outer.bottom() - corner,
        corner,
        true,
    );
    contour.line_to(point(outer.left(), outer.top() + corner));
    arc(
        &mut contour,
        outer.left() + corner,
        outer.top(),
        corner,
        true,
    );
    contour.close();
    contour
}
