//! Shared presentation of absent data, initial loads, and refreshes.

use gpui::{App, Div, FontWeight, Role, SharedString, Stateful, div, prelude::*, px};
use guise::{Icon, IconName, theme};

pub(in crate::ui) fn empty_state(
    id: impl Into<SharedString>,
    title: impl Into<SharedString>,
    detail: Option<&'static str>,
    icon: IconName,
    cx: &App,
) -> Stateful<Div> {
    let id = id.into();
    let title = title.into();
    let ink = theme(cx);
    div()
        .id(id.clone())
        .accessibility_id(id)
        .role(Role::Status)
        .aria_label(title.clone())
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(8.))
        .py(px(12.))
        .px(px(10.))
        .text_center()
        .child(
            Icon::new(icon)
                .pixel_size(px(25.))
                .color(ink.dimmed().hsla()),
        )
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(ink.text())
                .child(title),
        )
        .children(detail.map(|detail| {
            div()
                .max_w(px(300.))
                .text_size(px(12.))
                .line_height(px(17.))
                .text_color(ink.dimmed())
                .child(detail)
        }))
}

pub(in crate::ui) fn loading_state(
    id: &'static str,
    title: &'static str,
    cx: &App,
) -> Stateful<Div> {
    empty_state(id, title, None, IconName::LoaderCircle, cx)
}

pub(in crate::ui) fn error_state(
    id: impl Into<SharedString>,
    title: impl Into<SharedString>,
    detail: Option<&'static str>,
    cx: &App,
) -> Stateful<Div> {
    empty_state(id, title, detail, IconName::CircleAlert, cx).role(Role::Alert)
}

pub(in crate::ui) fn refresh_indicator(
    id: &'static str,
    title: &'static str,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(id)
        .accessibility_id(id)
        .role(Role::Status)
        .aria_label(title)
        .size(px(24.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .tooltip(guise::tooltip(title))
        .child(
            Icon::new(IconName::RefreshCw)
                .pixel_size(px(14.))
                .color(theme(cx).dimmed().hsla()),
        )
}
