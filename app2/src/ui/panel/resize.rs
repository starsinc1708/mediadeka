//! The dock's splitter shares its live width with layout and optical geometry.

use gpui::{
    AccessibleAction, Context, DispatchPhase, Div, FocusHandle, Hsla, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Orientation, Pixels, Role, Stateful, Window,
    accesskit::ActionData, canvas, div, prelude::*, px, rgb, transparent_black,
};

use crate::ui::fluid_glass::{ShellGeometry, tokens};

use super::{MainView, inline};

pub(super) const MIN_WIDTH: f32 = 280.;
const MAX_WIDTH: f32 = 420.;
const PAGE_RESERVE: f32 = 720.;
const KEY_STEP: f32 = 16.;
const HANDLE_WIDTH: f32 = 10.;
const TAB_ORDER: isize = 595;

pub(super) struct Resize {
    width_dip: Option<u16>,
    drag: Option<Drag>,
    focus: FocusHandle,
}

#[derive(Clone, Copy)]
struct Drag {
    origin: Pixels,
    initial_width: f32,
    preference_before_drag: Option<u16>,
}

impl Resize {
    pub(super) fn new(cx: &mut Context<MainView>) -> Self {
        Self {
            width_dip: None,
            drag: None,
            focus: cx.focus_handle().tab_index(TAB_ORDER).tab_stop(true),
        }
    }

    pub(super) fn width(&self, window: &Window, fallback: f32) -> f32 {
        self.width_dip
            .map(f32::from)
            .unwrap_or(fallback)
            .clamp(MIN_WIDTH, width_limit(window))
    }

    pub(super) fn preferred_width(&self) -> Option<u16> {
        self.width_dip
    }

    pub(super) fn restore_width(&mut self, width: Option<u16>) {
        self.width_dip = self.width_dip.or(width);
    }

    pub(super) fn stop_drag(&mut self) -> bool {
        self.drag
            .take()
            .is_some_and(|drag| drag.preference_before_drag != self.width_dip)
    }
}

impl MainView {
    pub(super) fn reset_unavailable_panel_resize(&self, window: &Window, cx: &Context<Self>) {
        if self.panel.resize.drag.is_none() && !self.panel.resize.focus.is_focused(window) {
            return;
        }
        let handle = cx.weak_entity();
        window.on_next_frame(move |window, cx| {
            if !inline(window) {
                let _ = handle.update(cx, |view, cx| {
                    if view.panel.resize.stop_drag() {
                        view.persist_panel(cx);
                    }
                    if view.panel.open && view.panel.resize.focus.is_focused(window) {
                        window.focus(&view.panel.focus, cx);
                    }
                });
            }
        });
    }

    pub(super) fn resize_handle(&self, window: &Window, cx: &Context<Self>) -> Stateful<Div> {
        let move_view = cx.weak_entity();
        let up_view = move_view.clone();
        let set_view = move_view.clone();
        let increment = cx.listener(|view, _: &(), window, cx| {
            view.set_panel_width(view.right_panel_width(window) + KEY_STEP, window, cx);
        });
        let decrement = cx.listener(|view, _: &(), window, cx| {
            view.set_panel_width(view.right_panel_width(window) - KEY_STEP, window, cx);
        });
        let contrast = self.fluid_high_contrast(cx);
        let ink = Hsla::from(rgb(if contrast {
            tokens::contrast::PRIMARY
        } else {
            tokens::PRIMARY
        }));
        div()
            .id("right-panel-resize")
            .accessibility_id("right-panel-resize")
            .role(Role::Splitter)
            .aria_label("Ширина правой панели")
            .aria_description("Стрелки влево и вправо изменяют ширину панели")
            .aria_orientation(Orientation::Vertical)
            .aria_min_numeric_value(f64::from(MIN_WIDTH))
            .aria_max_numeric_value(f64::from(width_limit(window)))
            .aria_numeric_value(f64::from(self.right_panel_width(window)))
            .aria_numeric_value_step(f64::from(KEY_STEP))
            .track_focus(&self.panel.resize.focus)
            .absolute()
            .left(px(-4.))
            .top_0()
            .bottom_0()
            .w(px(HANDLE_WIDTH))
            .flex()
            .items_center()
            .justify_end()
            .pr(px(1.))
            .border_1()
            .border_color(transparent_black())
            .cursor_col_resize()
            .hover(move |style| style.bg(ink.alpha(0.07)))
            .focus_visible(move |style| style.border_color(ink))
            .child(
                div()
                    .w(px(2.))
                    .h(px(34.))
                    .rounded_full()
                    .bg(ink.alpha(if contrast { 0.82 } else { 0.36 })),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, event: &MouseDownEvent, window, cx| {
                    view.panel.resize.drag = Some(Drag {
                        origin: event.position.x,
                        initial_width: view.right_panel_width(window),
                        preference_before_drag: view.panel.resize.preferred_width(),
                    });
                    window.focus(&view.panel.resize.focus, cx);
                    cx.stop_propagation();
                }),
            )
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                let current = view.right_panel_width(window);
                let next = match event.keystroke.key.as_str() {
                    "left" => current + KEY_STEP,
                    "right" => current - KEY_STEP,
                    "home" => MIN_WIDTH,
                    "end" => width_limit(window),
                    _ => return,
                };
                view.set_panel_width(next, window, cx);
                cx.stop_propagation();
            }))
            .on_a11y_action(AccessibleAction::Increment, move |_, window, cx| {
                increment(&(), window, cx);
            })
            .on_a11y_action(AccessibleAction::Decrement, move |_, window, cx| {
                decrement(&(), window, cx);
            })
            .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
                if let Some(ActionData::NumericValue(value)) = data
                    && value.is_finite()
                {
                    let _ = set_view.update(cx, |view, cx| {
                        view.set_panel_width(*value as f32, window, cx)
                    });
                }
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, (), window, _| {
                        let moved = move_view.clone();
                        let released = up_view.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if phase == DispatchPhase::Capture
                                && moved
                                    .update(cx, |view, cx| view.drag_splitter(event, window, cx))
                                    .unwrap_or(false)
                            {
                                cx.stop_propagation();
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture
                                && event.button == MouseButton::Left
                                && released
                                    .update(cx, |view, cx| view.finish_panel_drag(cx))
                                    .unwrap_or(false)
                            {
                                cx.stop_propagation();
                            }
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
    }

    fn drag_splitter(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(drag) = self.panel.resize.drag else {
            return false;
        };
        if event.pressed_button != Some(MouseButton::Left) || !inline(window) {
            self.finish_panel_drag(cx);
            return false;
        }
        let width = drag.initial_width + f32::from(drag.origin - event.position.x);
        self.resize_to(width, window, cx);
        true
    }

    fn finish_panel_drag(&mut self, cx: &mut Context<Self>) -> bool {
        let was_dragging = self.panel.resize.drag.is_some();
        if self.panel.resize.stop_drag() {
            self.persist_panel(cx);
        }
        was_dragging
    }

    fn set_panel_width(&mut self, width: f32, window: &Window, cx: &mut Context<Self>) {
        if self.resize_to(width, window, cx) {
            self.persist_panel(cx);
        }
    }

    fn resize_to(&mut self, width: f32, window: &Window, cx: &mut Context<Self>) -> bool {
        let width = width.clamp(MIN_WIDTH, width_limit(window)).round();
        if width != self.right_panel_width(window) {
            self.panel.resize.width_dip = Some(width as u16);
            cx.notify();
            return true;
        }
        false
    }
}

fn width_limit(window: &Window) -> f32 {
    let content = ShellGeometry::for_window(window)
        .content_bounds()
        .size
        .width;
    (f32::from(content) - PAGE_RESERVE).clamp(MIN_WIDTH, MAX_WIDTH)
}

#[cfg(feature = "capture")]
impl MainView {
    pub fn dock_splitter_for_capture(&self, window: &Window) -> (f32, Pixels) {
        let width = self.docked_panel_width(window);
        let geometry = ShellGeometry::for_workspace(window, width);
        (width, geometry.content_bounds().right())
    }
}
