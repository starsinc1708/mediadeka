pub(super) mod artwork;
pub(super) mod content_state;
pub(super) mod entity_link;
pub(super) mod glass_control;
pub(super) mod music;
pub(super) mod overlay;
pub(super) mod scroll_viewport;
pub(super) mod scrollbar;
pub(super) mod search_input;
pub(super) mod slider;
pub(super) mod track_row;

use std::rc::Rc;

use gpui::{
    AccessibleAction, App, ClickEvent, Div, ElementId, FontWeight, Hsla, IntoElement, Pixels,
    Point, Role, SharedString, Stateful, Window, div, point, prelude::*, px, rgb,
};

use super::theme::{ACCENT, ELEVATED, focus_ring, primary, secondary, transparent};

// Adapted from the Guise button at 9ad9588ad11dcdcf31337f3c6b9c1e09573085a5.
// GPUI supplies keyboard activation. See assets/SOURCES.md for provenance.
pub(super) fn button(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    order: isize,
) -> Stateful<Div> {
    let id = id.into();
    let automation_id = id.to_string();
    div()
        .id(id)
        .accessibility_id(automation_id)
        .role(Role::Button)
        .aria_label(name)
        .tab_index(order)
        .flex()
        .items_center()
        .border_1()
        .border_color(transparent())
        .focus_visible(|style| style.border_color(focus_ring()))
        .cursor_pointer()
}

pub(super) fn section_tab(
    id: impl Into<ElementId>,
    label: &'static str,
    selected: bool,
    order: isize,
    glass: Option<glass_control::Material>,
) -> Stateful<Div> {
    use super::fluid_glass::tokens;
    let tab = button(id, label, order)
        .role(Role::Tab)
        .aria_selected(selected)
        .relative()
        .h(px(31.))
        .px(px(8.))
        .rounded(px(4.));
    if let Some(material) = glass {
        return paint_section_tab(tab, material, selected)
            .h(px(tokens::size::SM))
            .text_size(px(tokens::text::BODY))
            .font_weight(if selected {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .child(label);
    }
    tab.text_size(px(13.))
        .text_color(if selected { primary() } else { secondary() })
        .hover(|style| style.bg(rgb(ELEVATED)).text_color(primary()))
        .child(label)
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left(px(8.))
                    .right(px(8.))
                    .h(px(2.))
                    .bg(rgb(ACCENT)),
            )
        })
}

pub(in crate::ui) fn paint_section_tab(
    tab: Stateful<Div>,
    material: glass_control::Material,
    selected: bool,
) -> Stateful<Div> {
    use super::design_system::controls::{paint_page_tab, tab_selection_marker};

    paint_page_tab(tab, material, selected)
        .relative()
        .when(selected, |tab| {
            tab.child(
                div()
                    .absolute()
                    .bottom(px(1.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(tab_selection_marker(material)),
            )
        })
}

pub(in crate::ui) fn fluid_selection_marker(high_contrast: bool) -> Div {
    use super::fluid_glass::tokens;
    let color = if high_contrast {
        rgb(tokens::contrast::ACCENT).into()
    } else {
        Hsla::from(rgb(tokens::ACCENT)).alpha(tokens::selection::MARKER)
    };
    selection_marker(color)
}

fn selection_marker(color: Hsla) -> Div {
    div()
        .absolute()
        .left(px(2.))
        .top_0()
        .bottom_0()
        .flex()
        .items_center()
        .child(div().w(px(2.)).h(px(14.)).rounded_full().bg(color))
}

pub(in crate::ui) fn fluid_current_track_fill(high_contrast: bool) -> Hsla {
    use super::fluid_glass::tokens;
    if high_contrast {
        rgb(tokens::contrast::SELECTED).into()
    } else {
        Hsla::from(rgb(0xffffff)).alpha(tokens::selection::TRACK_TINT)
    }
}

pub(super) trait Activate {
    fn on_activate(self, handler: impl Fn(&(), &mut Window, &mut App) + 'static) -> Self;
    fn on_activate_at(
        self,
        handler: impl Fn(&Option<Point<Pixels>>, &mut Window, &mut App) + 'static,
    ) -> Self;
    fn on_toggle(
        self,
        expanded: bool,
        handler: impl Fn(&(), &mut Window, &mut App) + 'static,
    ) -> Self;
}

pub(in crate::ui) fn activation_position(event: &ClickEvent) -> Point<Pixels> {
    match event {
        ClickEvent::Keyboard(event) => {
            let bounds = event.bounds;
            point(
                bounds.origin.x + (bounds.size.width * 0.17).min(px(112.)),
                bounds.origin.y + bounds.size.height * 0.5,
            )
        }
        _ => event.position(),
    }
}

impl Activate for Stateful<Div> {
    fn on_activate(self, handler: impl Fn(&(), &mut Window, &mut App) + 'static) -> Self {
        self.on_activate_at(move |_, window, cx| handler(&(), window, cx))
    }

    fn on_activate_at(
        self,
        handler: impl Fn(&Option<Point<Pixels>>, &mut Window, &mut App) + 'static,
    ) -> Self {
        let click = Rc::new(handler);
        let invoke = click.clone();
        self.on_click(move |event, window, cx| click(&Some(activation_position(event)), window, cx))
            .on_a11y_action(AccessibleAction::Click, move |_, window, cx| {
                invoke(&None, window, cx)
            })
    }

    fn on_toggle(
        self,
        expanded: bool,
        handler: impl Fn(&(), &mut Window, &mut App) + 'static,
    ) -> Self {
        let toggle = Rc::new(handler);
        let accessible = Rc::clone(&toggle);
        self.aria_expanded(expanded)
            .on_activate(move |event, window, cx| toggle(event, window, cx))
            .on_a11y_action(
                if expanded {
                    AccessibleAction::Collapse
                } else {
                    AccessibleAction::Expand
                },
                move |_, window, cx| accessible(&(), window, cx),
            )
    }
}

pub(super) fn heading(text: impl Into<SharedString>, size: f32) -> impl IntoElement {
    div()
        .text_size(px(size))
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}
