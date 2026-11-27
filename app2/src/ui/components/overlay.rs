//! Shared menu geometry, layering and controls for context menus and selectors.

use std::{cell::Cell, rc::Rc, time::Duration};

use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, Context, Div, FocusHandle, FontWeight, Hsla,
    IntoElement, KeyDownEvent, MouseDownEvent, Pixels, Role, SharedString, Size, Stateful, Window,
    anchored, deferred, div, point, prelude::*, px, rgb, size,
};
use guise::{Icon, IconName};

use crate::ui::{
    Escape, ShiftTab, Tab,
    design_system::{
        controls::{ControlState, ControlVariant},
        icons::{Glyph, animated_glyph},
        materials::{Interaction, Kind, Material},
        motion::{MOTION, out_cubic},
    },
    fluid_glass::tokens::{contrast, radius, size, space},
    theme::{ELEVATED, primary, secondary},
};

use super::{Activate, button};

const WINDOW_INSET: f32 = space::XS;
const ANCHOR_GAP: f32 = space::XS;
const BORDER: f32 = 1.;
const MENU_RADIUS: f32 = radius::POPOVER;
const MENU_PADDING: f32 = 6.;
const MENU_OPACITY: f32 = 0.99;
const MENU_ITEM_HEIGHT: f32 = size::MD;
const COMPACT_ITEM_HEIGHT: f32 = size::XS;
const MENU_RULE_HEIGHT: f32 = 7.;
const MENU_ICON_SIZE: f32 = 18.;
const MENU_ICON_GAP: f32 = space::MD;
const MENU_MOTION: Duration = MOTION.interaction();

pub(in crate::ui) struct MenuMotion {
    enabled: bool,
    #[cfg(feature = "capture")]
    sample: Option<Duration>,
}

impl From<bool> for MenuMotion {
    fn from(enabled: bool) -> Self {
        Self {
            enabled,
            #[cfg(feature = "capture")]
            sample: None,
        }
    }
}

impl MenuMotion {
    #[cfg(feature = "capture")]
    pub(in crate::ui) fn sampled(mut self, sample: Option<Duration>) -> Self {
        self.sample = sample;
        self
    }

    fn animate(self, panel: Stateful<Div>) -> AnyElement {
        if !self.enabled {
            return panel.into_any_element();
        }
        #[cfg(feature = "capture")]
        if let Some(time) = self.sample {
            let progress = time.as_secs_f32() / MENU_MOTION.as_secs_f32();
            return menu_frame(panel, progress).into_any_element();
        }
        panel
            .with_animation(
                "editorial-menu-enter",
                Animation::new(MENU_MOTION),
                menu_frame,
            )
            .into_any_element()
    }
}

fn menu_frame(panel: Stateful<Div>, progress: f32) -> Stateful<Div> {
    let settle = out_cubic(progress);
    panel
        .opacity(0.82 + 0.18 * settle)
        .layer_scale(0.97 + 0.03 * settle)
}

/// The wrapper records the actual hit target, including keyboard-only triggers.
#[derive(Clone, Default)]
pub(in crate::ui) struct TriggerBounds(Rc<Cell<Bounds<Pixels>>>);

impl TriggerBounds {
    pub(in crate::ui) fn get(&self) -> Bounds<Pixels> {
        self.0.get()
    }

    pub(in crate::ui) fn set(&self, bounds: Bounds<Pixels>) {
        self.0.set(bounds);
    }

    pub(in crate::ui) fn capture(&self, trigger: impl IntoElement) -> Div {
        let bounds = self.clone();
        div()
            .flex_none()
            .child(trigger)
            .on_children_prepainted(move |children, _, _| {
                if let Some(&child) = children.first() {
                    bounds.0.set(child);
                }
            })
    }
}

#[derive(Clone, Copy)]
pub(in crate::ui) struct MenuLayout {
    pub item_height: f32,
    viewport: Size<Pixels>,
    padding: f32,
    width: f32,
}

impl MenuLayout {
    pub(in crate::ui) fn new(window: &Window, width: f32, classic_height: Option<f32>) -> Self {
        let viewport = window.viewport_size();
        Self {
            item_height: classic_height.unwrap_or_else(|| {
                if viewport.height < px(500.) {
                    COMPACT_ITEM_HEIGHT
                } else {
                    MENU_ITEM_HEIGHT
                }
            }),
            padding: MENU_PADDING,
            width: width.min(f32::from(viewport.width - px(2. * WINDOW_INSET)).max(1.)),
            viewport,
        }
    }

    pub(in crate::ui) fn frame(
        self,
        id: &'static str,
        label: &'static str,
        material: Option<Material>,
    ) -> Stateful<Div> {
        let panel = div()
            .id(id)
            .accessibility_id(id)
            .role(Role::Menu)
            .aria_label(label)
            .w(px(self.width))
            .max_h(self.viewport.height - px(2. * WINDOW_INSET))
            .occlude()
            .p(px(self.padding))
            .border_1()
            .rounded(px(MENU_RADIUS))
            .shadow_lg()
            .flex()
            .flex_col()
            .overflow_y_scroll();
        if let Some(material) = material {
            menu_surface(panel, material)
        } else {
            panel
        }
    }

    pub(in crate::ui) fn rule(self, color: Hsla) -> Div {
        div()
            .flex_none()
            .h(px(1.))
            .my(px((MENU_RULE_HEIGHT - 1.) * 0.5))
            .bg(color)
    }

    pub(in crate::ui) fn geometry(
        self,
        anchor: Bounds<Pixels>,
        align_end: bool,
        (items, rules): (usize, usize),
    ) -> Bounds<Pixels> {
        let inset = px(WINDOW_INSET);
        let gap = px(ANCHOR_GAP);
        let height = px(self.item_height * items.max(1) as f32
            + MENU_RULE_HEIGHT * rules as f32
            + 2. * (self.padding + BORDER));
        let below = anchor.bottom() + gap;
        let top = if below + height <= self.viewport.height - inset {
            below
        } else {
            anchor.top() - gap - height
        }
        .clamp(inset, (self.viewport.height - inset - height).max(inset));
        let left = if align_end {
            anchor.right() - px(self.width)
        } else {
            anchor.left()
        }
        .clamp(
            inset,
            (self.viewport.width - inset - px(self.width)).max(inset),
        );
        Bounds::new(point(left, top), size(px(self.width), height))
    }

    pub(in crate::ui) fn place(
        self,
        anchor: Bounds<Pixels>,
        align_end: bool,
        (items, rules): (usize, usize),
        panel: Stateful<Div>,
        motion: impl Into<MenuMotion>,
        dismiss: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let bounds = self.geometry(anchor, align_end, (items, rules));
        let panel = motion.into().animate(panel);
        // Measuring the trigger is local to its scroll container. Both the
        // backdrop and popup paint in window coordinates, above that container.
        deferred(
            anchored().position(point(px(0.), px(0.))).child(
                div()
                    .w(self.viewport.width)
                    .h(self.viewport.height)
                    .relative()
                    .child(
                        div()
                            .id("editorial-menu-backdrop")
                            .absolute()
                            .inset_0()
                            .occlude()
                            .on_any_mouse_down(dismiss),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(bounds.origin.x)
                            .top(bounds.origin.y)
                            .child(panel),
                    ),
            ),
        )
        .priority(2)
    }
}

pub(in crate::ui) fn menu_colors(material: Material) -> (Hsla, Hsla, Hsla) {
    if material.contrast() {
        return (
            rgb(contrast::ELEVATED).into(),
            rgb(contrast::RULE).into(),
            rgb(contrast::PRIMARY).into(),
        );
    }
    let style = material.kind(Kind::SmokedGlass).style(Interaction::Rest);
    (
        style.flat.alpha(MENU_OPACITY),
        style.edge.unwrap_or_else(gpui::transparent_black),
        style.ink,
    )
}

pub(in crate::ui) fn menu_surface(panel: Stateful<Div>, material: Material) -> Stateful<Div> {
    let (background, edge, ink) = menu_colors(material);
    panel
        .bg(background)
        .border_color(edge)
        .text_color(ink)
        .rounded(px(MENU_RADIUS))
}

pub(in crate::ui) fn menu_item(
    row: Stateful<Div>,
    layout: MenuLayout,
    material: Material,
    pointed: bool,
) -> Stateful<Div> {
    ControlVariant::MenuItem
        .paint(
            row,
            material,
            ControlState {
                active: pointed,
                ..ControlState::default()
            },
        )
        .w_full()
        .h(px(layout.item_height))
        .flex_none()
        .px(px(8.))
        .gap(px(MENU_ICON_GAP))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
}

/// Applies the Fluid Glass style to an unavailable menu row.
pub(in crate::ui) fn menu_item_disabled(
    row: Stateful<Div>,
    layout: MenuLayout,
    material: Material,
) -> Stateful<Div> {
    ControlVariant::MenuItem
        .paint(
            row,
            material,
            ControlState {
                disabled: true,
                ..ControlState::default()
            },
        )
        .w_full()
        .h(px(layout.item_height))
        .flex_none()
        .px(px(8.))
        .gap(px(MENU_ICON_GAP))
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_default()
        .opacity(0.55)
}

pub(in crate::ui) fn menu_icon(glyph: IconName) -> Icon {
    Icon::new(glyph).pixel_size(px(MENU_ICON_SIZE))
}

pub(in crate::ui) fn menu_value(
    row: Stateful<Div>,
    title: SharedString,
    checked: bool,
) -> Stateful<Div> {
    row.child(div().flex_1().min_w_0().truncate().child(title))
        .when(checked, |row| row.child(menu_icon(IconName::Check)))
}

pub(in crate::ui) fn select_trigger(
    id: &'static str,
    name: impl Into<SharedString>,
    value: impl Into<SharedString>,
    order: isize,
    expanded: bool,
    material: Option<Material>,
    cx: &App,
) -> Stateful<Div> {
    let state = ControlState {
        expanded,
        ..ControlState::default()
    };
    let trigger = button(id, name, order)
        .aria_expanded(expanded)
        .flex_none()
        .h(px(32.))
        .px(px(8.))
        .gap(px(6.))
        .text_size(px(13.))
        .child(div().flex_1().min_w_0().truncate().child(value.into()));
    if let Some(material) = material {
        let ink = ControlVariant::SelectTrigger.resolve(material, state).fg;
        ControlVariant::SelectTrigger
            .paint(trigger, material, state)
            .child(animated_glyph(
                id,
                Glyph::ChevronDown,
                expanded,
                14.,
                ink,
                cx,
            ))
    } else {
        trigger
            .rounded(px(6.))
            .text_color(secondary())
            .hover(|style| style.bg(rgb(ELEVATED)).text_color(primary()))
            .child(
                menu_icon(if expanded {
                    IconName::ChevronUp
                } else {
                    IconName::ChevronDown
                })
                .pixel_size(px(14.)),
            )
    }
}

pub(in crate::ui) struct SelectChoice {
    pub id: SharedString,
    pub label: SharedString,
    pub checked: bool,
    pub enabled: bool,
    pub separator_before: bool,
    pub description: Option<SharedString>,
}

impl SelectChoice {
    pub(in crate::ui) fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        checked: bool,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            checked,
            enabled: true,
            separator_before: false,
            description: None,
        }
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn separated(mut self) -> Self {
        self.separator_before = true;
        self
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn disabled(mut self, reason: impl Into<SharedString>) -> Self {
        self.enabled = false;
        self.description = Some(reason.into());
        self
    }
}

pub(in crate::ui) struct SelectLabel {
    pub trigger: &'static str,
    pub menu: &'static str,
    pub label: &'static str,
    pub value: SharedString,
    pub enabled: bool,
}

pub(in crate::ui) struct Select {
    bounds: TriggerBounds,
    trigger: FocusHandle,
    menu: FocusHandle,
    order: isize,
    menu_width: f32,
    trigger_width: Option<f32>,
    cursor: Option<usize>,
    return_focus: Option<FocusHandle>,
    #[cfg(feature = "capture")]
    capture: std::cell::RefCell<Option<SelectCapture>>,
    #[cfg(feature = "capture")]
    motion_sample: Option<Duration>,
}

#[cfg(feature = "capture")]
#[derive(Clone)]
pub(in crate::ui) struct SelectCapture {
    pub menu: Bounds<Pixels>,
    pub rows: Vec<(usize, Bounds<Pixels>)>,
}

impl Select {
    pub(in crate::ui) fn new(order: isize, cx: &App) -> Self {
        Self {
            bounds: TriggerBounds::default(),
            trigger: cx.focus_handle().tab_index(order).tab_stop(true),
            menu: cx.focus_handle(),
            order,
            menu_width: 240.,
            trigger_width: None,
            cursor: None,
            return_focus: None,
            #[cfg(feature = "capture")]
            capture: std::cell::RefCell::new(None),
            #[cfg(feature = "capture")]
            motion_sample: None,
        }
    }

    pub(in crate::ui) fn with_menu_width(mut self, width: f32) -> Self {
        self.menu_width = width;
        self
    }

    /// Reserve a stable trigger width for a selector whose value changes the
    /// label, so neighbouring commands do not shift.
    pub(in crate::ui) fn with_trigger_width(mut self, width: f32) -> Self {
        self.trigger_width = Some(width);
        self
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn trigger_bounds_for_capture(&self) -> Bounds<Pixels> {
        self.bounds.get()
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn menu_bounds_for_capture(&self) -> Option<Bounds<Pixels>> {
        self.capture.borrow().as_ref().map(|capture| capture.menu)
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn sample_menu_for_capture(&mut self, sample: Option<Duration>) {
        self.motion_sample = sample;
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn row_bounds_for_capture(&self, choice: usize) -> Option<Bounds<Pixels>> {
        self.capture
            .borrow()
            .as_ref()?
            .rows
            .iter()
            .find(|(index, _)| *index == choice)
            .map(|(_, bounds)| *bounds)
    }

    pub(in crate::ui) fn reset(&mut self) {
        self.cursor = None;
        self.return_focus = None;
    }

    pub(in crate::ui) fn open_from(
        &mut self,
        cursor: usize,
        origin: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.cursor = Some(cursor);
        self.return_focus = origin;
        window.focus(&self.menu, cx);
    }

    pub(in crate::ui) fn focus_trigger(&self, window: &mut Window, cx: &mut App) {
        window.focus(&self.trigger, cx);
    }

    pub(in crate::ui) fn close(&mut self, window: &mut Window, cx: &mut App) -> bool {
        if self.cursor.take().is_some() {
            if let Some(origin) = self.return_focus.take() {
                window.focus(&origin, cx);
            } else {
                self.focus_trigger(window, cx);
            }
            true
        } else {
            false
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(in crate::ui) fn view<V: 'static>(
        &self,
        label: SelectLabel,
        choices: Vec<SelectChoice>,
        state: fn(&mut V) -> &mut Self,
        choose: impl Fn(&mut V, usize, &mut Window, &mut Context<V>) + 'static,
        material: Option<Material>,
        motion: bool,
        window: &Window,
        cx: &Context<V>,
    ) -> Div {
        #[cfg(feature = "capture")]
        self.capture.replace(None);
        let opened = label.enabled && self.cursor.is_some() && !choices.is_empty();
        let initial = choices
            .iter()
            .position(|choice| choice.checked && choice.enabled)
            .or_else(|| choices.iter().position(|choice| choice.enabled))
            .unwrap_or(0);
        let count = choices.len();
        let enabled = choices
            .iter()
            .map(|choice| choice.enabled)
            .collect::<Vec<_>>();
        let rules = choices
            .iter()
            .filter(|choice| choice.separator_before)
            .count();
        let trigger = select_trigger(
            label.trigger,
            format!("{} · {}", label.label, label.value),
            label.value,
            self.order,
            opened,
            material,
            cx,
        );
        let trigger = if let Some(width) = self.trigger_width {
            trigger.w(px(width)).min_w(px(width)).justify_start()
        } else {
            trigger
        };
        let trigger = trigger
            .when(label.enabled, |trigger| {
                trigger.track_focus(&self.trigger).on_toggle(
                    opened,
                    cx.listener(move |view, _, window, cx| {
                        let picker = state(view);
                        if !picker.close(window, cx) && count > 0 {
                            picker.open_from(initial, None, window, cx);
                        }
                        cx.notify();
                    }),
                )
            })
            .when(!label.enabled, |trigger| {
                trigger
                    .tab_stop(false)
                    .cursor_default()
                    .opacity(0.55)
                    .aria_description("Недоступно")
            });
        self.bounds
            .capture(trigger)
            .when_some(self.cursor.filter(|_| opened), |select, cursor| {
                let choose = Rc::new(choose);
                let key_choose = choose.clone();
                let layout =
                    MenuLayout::new(window, self.menu_width, material.is_none().then_some(30.));
                let anchor = self.bounds.get();
                let align_end = anchor.center().x > window.viewport_size().width / 2.;
                #[cfg(feature = "capture")]
                {
                    let menu = layout.geometry(anchor, align_end, (count, rules));
                    let inset = px(layout.padding + BORDER);
                    let width = menu.size.width - px(2. * (layout.padding + BORDER));
                    let mut rows = Vec::with_capacity(count);
                    let mut top = menu.origin.y + inset;
                    for (index, choice) in choices.iter().enumerate() {
                        if choice.separator_before {
                            top += px(MENU_RULE_HEIGHT);
                        }
                        rows.push((
                            index,
                            Bounds::new(
                                point(menu.origin.x + inset, top),
                                size(width, px(layout.item_height)),
                            ),
                        ));
                        top += px(layout.item_height);
                    }
                    self.capture.replace(Some(SelectCapture { menu, rows }));
                }
                let rule = material.map_or_else(crate::ui::theme::surface_border, |material| {
                    menu_colors(material).1
                });
                #[cfg(feature = "capture")]
                let menu_motion = MenuMotion::from(motion).sampled(self.motion_sample);
                #[cfg(not(feature = "capture"))]
                let menu_motion = MenuMotion::from(motion);
                let panel = layout
                    .frame(label.menu, label.label, material)
                    .track_focus(&self.menu)
                    .when(material.is_none(), |panel| {
                        panel
                            .bg(rgb(ELEVATED))
                            .border_color(crate::ui::theme::surface_border())
                    })
                    .on_action(cx.listener(move |view, _: &Escape, window, cx| {
                        state(view).close(window, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .on_action(cx.listener(move |view, _: &Tab, window, cx| {
                        state(view).close(window, cx);
                        cx.notify();
                    }))
                    .on_action(cx.listener(move |view, _: &ShiftTab, window, cx| {
                        state(view).close(window, cx);
                        cx.notify();
                    }))
                    .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
                        let picker = state(view);
                        let Some(cursor) = picker.cursor.map(|cursor| cursor.min(count - 1)) else {
                            return;
                        };
                        if event.keystroke.key == "tab" {
                            // Leave normal focus traversal to GPUI, but do not
                            // leave an orphaned panel behind.
                            picker.close(window, cx);
                            cx.notify();
                            return;
                        }
                        match event.keystroke.key.as_str() {
                            "up" => {
                                if let Some(next) = next_enabled(&enabled, cursor, false) {
                                    picker.cursor = Some(next);
                                }
                            }
                            "down" => {
                                if let Some(next) = next_enabled(&enabled, cursor, true) {
                                    picker.cursor = Some(next);
                                }
                            }
                            "enter"
                                if picker.menu.is_focused(window)
                                    && enabled.get(cursor) == Some(&true) =>
                            {
                                picker.close(window, cx);
                                key_choose(view, cursor, window, cx);
                            }
                            _ => return,
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .children(choices.into_iter().enumerate().flat_map(|(index, option)| {
                        let SelectChoice {
                            id,
                            label,
                            checked,
                            enabled,
                            separator_before,
                            description,
                        } = option;
                        let pointed =
                            cursor.min(count - 1) == index && self.menu.is_focused(window);
                        let mut elements = Vec::with_capacity(1 + usize::from(separator_before));
                        if separator_before {
                            elements.push(layout.rule(rule).into_any_element());
                        }
                        if enabled {
                            let row = button(id, label.clone(), self.order + 1 + index as isize)
                                .tab_stop(false)
                                .role(Role::MenuItem)
                                .aria_selected(checked)
                                .when(pointed, |row| row.aria_active_descendant());
                            let row = if let Some(material) = material {
                                menu_item(row, layout, material, pointed)
                            } else {
                                row.w_full()
                                    .h(px(layout.item_height))
                                    .flex_none()
                                    .px(px(8.))
                                    .gap(px(8.))
                                    .rounded(px(6.))
                                    .text_size(px(13.))
                                    .when(pointed, |row| row.bg(rgb(crate::ui::theme::ACTIVE)))
                                    .hover(|style| style.bg(rgb(crate::ui::theme::ACTIVE)))
                            };
                            let choose = choose.clone();
                            elements.push(
                                menu_value(row, label, checked)
                                    .on_activate(cx.listener(move |view, _, window, cx| {
                                        state(view).close(window, cx);
                                        choose(view, index, window, cx);
                                        cx.notify();
                                    }))
                                    .into_any_element(),
                            );
                        } else {
                            // Keep the row and its explanation in the tree
                            // with a disabled state and no activation action.
                            let reason = description.unwrap_or_else(|| "Недоступно".into());
                            let row = button(id, label.clone(), self.order + 1 + index as isize)
                                .tab_stop(false)
                                .role(Role::MenuItem)
                                .aria_label(format!("{label} · {reason}"))
                                .aria_selected(checked)
                                .aria_description(reason)
                                .a11y_synthetic_children(|tree| tree.parent_node().set_disabled())
                                .on_any_mouse_down(|_, window, _| {
                                    // Keep the menu's keyboard cursor and focus
                                    // where they are; the row has no command.
                                    window.prevent_default();
                                });
                            let row = if let Some(material) = material {
                                menu_item_disabled(row, layout, material)
                            } else {
                                row.w_full()
                                    .h(px(layout.item_height))
                                    .flex_none()
                                    .px(px(8.))
                                    .gap(px(8.))
                                    .rounded(px(6.))
                                    .text_size(px(13.))
                                    .cursor_default()
                                    .opacity(0.55)
                            };
                            elements.push(menu_value(row, label, checked).into_any_element());
                        }
                        elements
                    }));
                select.child(layout.place(
                    anchor,
                    align_end,
                    (count, rules),
                    panel,
                    menu_motion,
                    cx.listener(move |view, _, window, cx| {
                        state(view).close(window, cx);
                        cx.stop_propagation();
                        cx.notify();
                    }),
                ))
            })
    }
}

fn next_enabled(enabled: &[bool], current: usize, forward: bool) -> Option<usize> {
    let count = enabled.len();
    if count == 0 || !enabled.iter().any(|enabled| *enabled) {
        return None;
    }
    let mut index = current.min(count - 1);
    for _ in 0..count {
        index = if forward {
            (index + 1) % count
        } else {
            (index + count - 1) % count
        };
        if enabled[index] {
            return Some(index);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{point, px, size};

    #[test]
    fn menu_cursor_skips_disabled_rows_in_both_directions() {
        let enabled = [true, false, true, false];
        assert_eq!(next_enabled(&enabled, 0, true), Some(2));
        assert_eq!(next_enabled(&enabled, 2, true), Some(0));
        assert_eq!(next_enabled(&enabled, 0, false), Some(2));
        assert_eq!(next_enabled(&enabled, 2, false), Some(0));
        assert_eq!(next_enabled(&[false, false], 0, true), None);
        assert_eq!(next_enabled(&[], 0, true), None);
    }

    #[test]
    fn menu_geometry_stays_inside_the_viewport_and_anchors_to_the_trigger() {
        let layout = MenuLayout {
            item_height: 36.,
            viewport: size(px(800.), px(400.)),
            padding: MENU_PADDING,
            width: 235.,
        };
        let high_trigger = Bounds::new(point(px(560.), px(330.)), size(px(168.), px(32.)));
        let above = layout.geometry(high_trigger, true, (4, 0));
        assert_eq!(above.right(), high_trigger.right());
        assert_eq!(above.bottom(), high_trigger.top() - px(ANCHOR_GAP));
        let low_trigger = Bounds::new(point(px(560.), px(10.)), size(px(168.), px(32.)));
        let below = layout.geometry(low_trigger, true, (4, 0));
        assert_eq!(below.top(), low_trigger.bottom() + px(ANCHOR_GAP));
        assert!(below.bottom() <= px(400.));
        assert!(below.right() <= px(800.));
    }
}
