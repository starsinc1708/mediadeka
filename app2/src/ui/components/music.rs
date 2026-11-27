use super::artwork;
use crate::ui::{
    fluid_glass::{
        artwork::{self as fluid_artwork, Seed},
        tokens::text,
    },
    theme::{ACCENT, COVER_LARGE_TINT, ELEVATED, PRIMARY, SURFACE},
};
use gpui::{Div, ElementId, FontWeight, Hsla, SharedString, div, prelude::*, px, rgb};
use guise::{ActionIcon, ColorValue, Icon, IconName, Size, Variant};

pub(in crate::ui) const TRACK_ROW_HEIGHT: f32 = 44.;

pub(in crate::ui) fn card_title(label: impl Into<SharedString>) -> Div {
    div()
        .w_full()
        .min_w_0()
        .truncate()
        .text_size(px(text::CARD_TITLE))
        .line_height(px(19.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label.into())
}

pub(in crate::ui) fn control(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    glyph: IconName,
    order: isize,
    enabled: bool,
    primary: bool,
    active: bool,
) -> ActionIcon {
    let variant = if primary && enabled {
        Variant::Filled
    } else if active {
        Variant::Light
    } else {
        Variant::Default
    };
    ActionIcon::new(id, glyph)
        .label(name)
        .size(Size::Lg)
        .glyph_size(px(16.))
        .variant(variant)
        .color(ColorValue::Primary)
        .tab_order(order)
        .disabled(!enabled)
        .when(!enabled, |control| {
            control.accessible_description("Недоступно")
        })
}

pub(in crate::ui) fn compact_control(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    glyph: IconName,
    order: isize,
    enabled: bool,
    active: bool,
) -> ActionIcon {
    let color = if active {
        ColorValue::Primary
    } else {
        ColorValue::from(Hsla::from(rgb(PRIMARY)))
    };
    ActionIcon::new(id, glyph)
        .label(name)
        .size(Size::Md)
        .glyph_size(px(16.))
        .variant(Variant::Subtle)
        .color(color)
        .tab_order(order)
        .disabled(!enabled)
        .when(!enabled, |control| {
            control.accessible_description("Недоступно")
        })
}

pub(in crate::ui) fn cover(url: Option<&str>, edge: f32, radius: f32, large: bool) -> Div {
    let background = gpui::linear_gradient(
        130.,
        gpui::linear_color_stop(rgb(if large { COVER_LARGE_TINT } else { ELEVATED }), 0.),
        gpui::linear_color_stop(rgb(SURFACE), 1.),
    );
    artwork_slot(url, edge, radius, large, background, ACCENT)
}

pub(in crate::ui) fn fluid_cover(
    url: Option<&str>,
    fallback: Option<Seed>,
    edge: f32,
    radius: f32,
) -> Div {
    fluid_cover_slot(edge, radius).child(fluid_artwork::thumbnail(url, fallback, px(radius), false))
}

pub(in crate::ui) fn fluid_cover_slot(edge: f32, radius: f32) -> Div {
    use crate::ui::fluid_glass::tokens;
    artwork_slot(
        None,
        edge,
        radius,
        false,
        rgb(crate::ui::design_system::materials::SMOKED_BASE).into(),
        tokens::MUTED,
    )
}

fn artwork_slot(
    url: Option<&str>,
    edge: f32,
    radius: f32,
    large: bool,
    background: gpui::Background,
    glyph: u32,
) -> Div {
    div()
        .size(px(edge))
        .flex_none()
        .relative()
        .overflow_hidden()
        .rounded(px(radius))
        .bg(background)
        .flex()
        .items_center()
        .justify_center()
        .child(
            Icon::new(IconName::AudioLines)
                .pixel_size(px(edge * 0.36))
                .color(gpui::Hsla::from(rgb(glyph))),
        )
        .children(url.map(|url| artwork::remote_rounded(url, false, px(radius), large)))
}

pub(in crate::ui) fn time_label(seconds: f64) -> String {
    let seconds = seconds.max(0.) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}
