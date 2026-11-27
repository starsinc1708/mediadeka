
use std::time::Instant;

use gpui::{
    AccessibleAction, App, Bounds, Context, DispatchPhase, IntoElement, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, Point, Subscription,
    WeakEntity, Window, accesskit::ActionData, canvas, div, fill, point, prelude::*, px, rgb, size,
};

use crate::ui::{
    MainView,
    components::scrollbar::{
        ScrollChrome, ScrollSurface, paint_scrollbar, scroll_offset, scrollbar_element,
        set_handle_offset,
    },
    theme::{
        SCROLL_MARKER_ARROW, SCROLL_MARKER_BACKGROUND, SCROLL_MARKER_BORDER, SCROLL_MARKER_DOT,
    },
};

const DEAD_ZONE: Pixels = px(8.);
const MAX_FRAME_SECONDS: f32 = 0.04;

type ScrollListener = fn(&mut MainView, &Window, &mut Context<MainView>);

#[derive(Default)]
pub(super) struct WorkspaceScrolling {
    mode: Mode,
    generation: u64,
    activation: Option<Subscription>,
}

#[derive(Clone, Default)]
enum Mode {
    #[default]
    Idle,
    Thumb {
        grab: Pixels,
        surface: ScrollSurface,
        on_scroll: Option<ScrollListener>,
    },
    Auto(AutoScroll),
}

#[derive(Clone, Copy)]
struct AutoScroll {
    anchor: Point<Pixels>,
    pointer_y: Pixels,
    pressed: bool,
    was_dragged: bool,
    frame_pending: bool,
}

impl MainView {
    pub(super) fn workspace_scrolling(
        &self,
        content: impl IntoElement,
        header_height: Pixels,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let view = cx.weak_entity();
        let surface = ScrollSurface::Area(self.scroll.clone());
        let chrome = ScrollChrome::new(self.uses_fluid_glass(), window);
        let increment = view.clone();
        let decrement = view.clone();
        let set_value = view.clone();
        div()
            .relative()
            .w_full()
            .min_h_0()
            .flex_1()
            .flex()
            .flex_col()
            .child(self.scroll_area(content, header_height, window))
            .child(
                scrollbar_element(
                    "home-scrollbar",
                    "Вертикальная прокрутка главной",
                    &surface,
                    chrome,
                )
                .aria_numeric_value_step(48.)
                .on_a11y_action(AccessibleAction::Increment, move |_, _, cx| {
                    let _ = increment.update(cx, |this, cx| {
                        this.set_home_offset(scroll_offset(&this.scroll) + px(48.), cx);
                    });
                })
                .on_a11y_action(AccessibleAction::Decrement, move |_, _, cx| {
                    let _ = decrement.update(cx, |this, cx| {
                        this.set_home_offset(scroll_offset(&this.scroll) - px(48.), cx);
                    });
                })
                .on_a11y_action(AccessibleAction::SetValue, move |data, _, cx| {
                    if let Some(ActionData::NumericValue(value)) = data
                        && value.is_finite()
                    {
                        let _ = set_value.update(cx, |this, cx| {
                            this.set_home_offset(px(*value as f32), cx);
                        });
                    }
                }),
            )
            .child(self.scrollbar_guide(surface, chrome, cx))
    }

    pub(in crate::ui) fn scrollbar_guide(
        &self,
        surface: ScrollSurface,
        chrome: ScrollChrome,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        self.observed_scrollbar_guide(surface, chrome, None, cx)
    }

    pub(in crate::ui) fn observed_scrollbar_guide(
        &self,
        surface: ScrollSurface,
        chrome: ScrollChrome,
        on_scroll: Option<ScrollListener>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let mode = self.workspace.scrolling.mode.clone();
        let on_playlist = self.music_page() == crate::ui::workspace::Page::Playlist;
        let fluid_glass = self.uses_fluid_glass();
        let view = cx.weak_entity();
        canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                paint_guide(window, &surface, chrome, mode, on_playlist, fluid_glass);
                register_pointer_handlers(window, view, surface, on_scroll);
            },
        )
        .absolute()
        .inset_0()
    }

    pub(in crate::ui) fn stop_workspace_scrolling(&mut self) -> bool {
        let scrolling = &mut self.workspace.scrolling;
        match std::mem::take(&mut scrolling.mode) {
            Mode::Idle => return false,
            Mode::Thumb { surface, .. } => surface.end_drag(),
            Mode::Auto(_) => {}
        }
        scrolling.generation = scrolling.generation.wrapping_add(1);
        true
    }

    fn on_scroll_press(
        &mut self,
        surface: &ScrollSurface,
        on_scroll: Option<ScrollListener>,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.stop_workspace_scrolling() {
            cx.notify();
            return true;
        }
        if self.playlist_menu_open()
            || self.is_settings_login()
            || self.soloist.is_open()
            || !surface.viewport().contains(&event.position)
        {
            return false;
        }
        let chrome = ScrollChrome::new(self.uses_fluid_glass(), window);
        let Some(track) = surface.track_with(chrome) else {
            return false;
        };
        match event.button {
            MouseButton::Middle if matches!(surface, ScrollSurface::Area(_)) => {
                self.workspace.scrolling.mode = Mode::Auto(AutoScroll {
                    anchor: event.position,
                    pointer_y: event.position.y,
                    pressed: true,
                    was_dragged: false,
                    frame_pending: false,
                });
            }
            MouseButton::Left if track.bounds.contains(&event.position) => {
                let grab = track.grab_offset(event.position.y, surface.offset());
                surface.start_drag();
                self.workspace.scrolling.mode = Mode::Thumb {
                    grab,
                    surface: surface.clone(),
                    on_scroll,
                };
                self.set_scrollbar_offset(
                    surface,
                    track.offset_at(event.position.y, grab),
                    on_scroll,
                    window,
                    cx,
                );
            }
            _ => return false,
        }
        if self.workspace.scrolling.activation.is_none() {
            self.workspace.scrolling.activation =
                Some(cx.observe_window_activation(window, |this, window, cx| {
                    if !window.is_window_active() && this.stop_workspace_scrolling() {
                        cx.notify();
                    }
                }));
        }
        cx.notify();
        true
    }

    fn on_scroll_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let fluid_glass = self.uses_fluid_glass();
        match &mut self.workspace.scrolling.mode {
            Mode::Idle => false,
            Mode::Thumb {
                grab,
                surface,
                on_scroll,
            } => {
                if event.pressed_button != Some(MouseButton::Left) {
                    self.stop_workspace_scrolling();
                    cx.notify();
                } else if let Some(track) =
                    surface.track_with(ScrollChrome::new(fluid_glass, window))
                {
                    let (surface, target, on_scroll) = (
                        surface.clone(),
                        track.offset_at(event.position.y, *grab),
                        *on_scroll,
                    );
                    self.set_scrollbar_offset(&surface, target, on_scroll, window, cx);
                }
                true
            }
            Mode::Auto(auto) => {
                auto.pointer_y = event.position.y;
                auto.was_dragged |=
                    auto.pressed && (auto.pointer_y - auto.anchor.y).abs() > DEAD_ZONE;
                self.schedule_home_scroll_frame(window, cx, Instant::now());
                true
            }
        }
    }

    pub(in crate::ui) fn set_scrollbar_offset(
        &mut self,
        surface: &ScrollSurface,
        target: Pixels,
        on_scroll: Option<ScrollListener>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if surface.set_offset(target) {
            if let Some(on_scroll) = on_scroll {
                on_scroll(self, window, cx);
            }
            cx.notify();
        }
    }

    fn on_scroll_release(&mut self, event: &MouseUpEvent, cx: &mut Context<Self>) -> bool {
        match &mut self.workspace.scrolling.mode {
            Mode::Thumb { .. } if event.button == MouseButton::Left => {
                self.stop_workspace_scrolling();
                cx.notify();
                true
            }
            Mode::Auto(auto) if event.button == MouseButton::Middle => {
                if auto.was_dragged
                    || (auto.pressed && (event.position.y - auto.anchor.y).abs() > DEAD_ZONE)
                {
                    self.stop_workspace_scrolling();
                    cx.notify();
                } else {
                    auto.pressed = false;
                }
                true
            }
            _ => false,
        }
    }

    fn schedule_home_scroll_frame(&mut self, window: &Window, cx: &Context<Self>, since: Instant) {
        let Mode::Auto(auto) = &mut self.workspace.scrolling.mode else {
            return;
        };
        if auto.frame_pending
            || !can_scroll_towards(
                scroll_offset(&self.scroll),
                self.scroll.max_offset().y,
                scroll_speed(auto.pointer_y - auto.anchor.y),
            )
        {
            return;
        }
        auto.frame_pending = true;
        let generation = self.workspace.scrolling.generation;
        let view = cx.weak_entity();
        window.on_next_frame(move |window, cx| {
            let _ = view.update(cx, |this, cx| {
                this.advance_home_autoscroll(generation, since, window, cx);
            });
        });
    }

    fn advance_home_autoscroll(
        &mut self,
        generation: u64,
        since: Instant,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.scrolling.generation != generation {
            return;
        }
        let Mode::Auto(auto) = &mut self.workspace.scrolling.mode else {
            return;
        };
        auto.frame_pending = false;
        let speed = scroll_speed(auto.pointer_y - auto.anchor.y);
        let is_capture = self.capture_time.is_some();
        if (!window.is_window_active() && !is_capture)
            || ScrollSurface::Area(self.scroll.clone())
                .track_with(ScrollChrome::new(self.uses_fluid_glass(), window))
                .is_none()
        {
            self.stop_workspace_scrolling();
            cx.notify();
            return;
        }
        let now = Instant::now();
        let elapsed = (now - since).as_secs_f32().min(MAX_FRAME_SECONDS);
        self.set_home_offset(scroll_offset(&self.scroll) + px(speed * elapsed), cx);
        self.schedule_home_scroll_frame(window, cx, now);
    }

    fn set_home_offset(&self, desired: Pixels, cx: &mut Context<Self>) -> bool {
        if !set_handle_offset(&self.scroll, desired) {
            return false;
        }
        cx.notify();
        true
    }

    #[cfg(feature = "capture")]
    pub fn home_autoscroll_active_for_capture(&self) -> bool {
        matches!(self.workspace.scrolling.mode, Mode::Auto(_))
    }

    #[cfg(feature = "capture")]
    pub fn home_scroll_bounds_for_capture(&self) -> Bounds<Pixels> {
        self.scroll.bounds()
    }

    #[cfg(feature = "capture")]
    pub fn home_scroll_content_bounds_for_capture(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Bounds<Pixels> {
        let mut bounds = self.home_scroll_bounds_for_capture();
        if self.music_page() == super::Page::Object {
            let content = self.adaptive_content(window);
            let compact = content.compact() || window.viewport_size().height < px(560.);
            if let Some(hero) = self.object_hero_scroll(content.width, compact, cx) {
                let top =
                    (hero.height() + self.object_hero_columns_height()).min(bounds.size.height);
                bounds.origin.y += top;
                bounds.size.height -= top;
            }
        }
        bounds
    }

    #[cfg(feature = "capture")]
    pub fn home_scrollbar_for_capture(&self, window: &Window) -> Option<(Bounds<Pixels>, Pixels)> {
        ScrollSurface::Area(self.scroll.clone())
            .track_for_capture(ScrollChrome::new(self.uses_fluid_glass(), window))
    }
}

fn register_pointer_handlers(
    window: &mut Window,
    view: WeakEntity<MainView>,
    surface: ScrollSurface,
    on_scroll: Option<ScrollListener>,
) {
    let pressed = view.clone();
    window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
        intercept(phase, &pressed, cx, |view, cx| {
            view.on_scroll_press(&surface, on_scroll, event, window, cx)
        });
    });
    let moved = view.clone();
    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
        intercept(phase, &moved, cx, |view, cx| {
            view.on_scroll_move(event, window, cx)
        });
    });
    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
        intercept(phase, &view, cx, |view, cx| {
            view.on_scroll_release(event, cx)
        });
    });
}

fn intercept(
    phase: DispatchPhase,
    view: &WeakEntity<MainView>,
    cx: &mut App,
    handle: impl FnOnce(&mut MainView, &mut Context<MainView>) -> bool,
) {
    if phase == DispatchPhase::Capture && view.update(cx, handle).unwrap_or(false) {
        cx.stop_propagation();
    }
}

fn scroll_speed(displacement: Pixels) -> f32 {
    let direction = f32::from(displacement).signum();
    let distance = f32::from((displacement.abs() - DEAD_ZONE).max(Pixels::ZERO));
    direction * (distance * 10.).min(2400.)
}

fn can_scroll_towards(offset: Pixels, extent: Pixels, speed: f32) -> bool {
    (speed > 0. && offset < extent) || (speed < 0. && offset > Pixels::ZERO)
}

fn paint_guide(
    window: &mut Window,
    surface: &ScrollSurface,
    chrome: ScrollChrome,
    mode: Mode,
    on_playlist: bool,
    fluid_glass: bool,
) {
    paint_scrollbar(
        window,
        surface,
        chrome,
        matches!(mode, Mode::Thumb { .. }),
        on_playlist,
        fluid_glass,
    );
    if let Mode::Auto(auto) = mode {
        paint_anchor(window, auto.anchor);
    }
}

fn paint_anchor(window: &mut Window, origin: Point<Pixels>) {
    let marker = Bounds::centered_at(origin, size(px(28.), px(28.)));
    window.paint_quad(
        fill(marker, rgb(SCROLL_MARKER_BACKGROUND))
            .corner_radii(px(14.))
            .border_widths(px(1.))
            .border_color(rgb(SCROLL_MARKER_BORDER)),
    );
    let dot = Bounds::centered_at(origin, size(px(3.), px(3.)));
    window.paint_quad(fill(dot, rgb(SCROLL_MARKER_DOT)).corner_radii(px(1.5)));
    let mut arrows = PathBuilder::stroke(px(1.25));
    for side in [-1., 1.] {
        arrows.move_to(point(origin.x - px(3.), origin.y + px(6.) * side));
        arrows.line_to(point(origin.x, origin.y + px(9.) * side));
        arrows.line_to(point(origin.x + px(3.), origin.y + px(6.) * side));
    }
    if let Ok(arrows) = arrows.build() {
        window.paint_path(arrows, rgb(SCROLL_MARKER_ARROW));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_speed_has_dead_zone_direction_and_limit() {
        assert_eq!(scroll_speed(px(8.)), 0.);
        assert_eq!(scroll_speed(px(-8.)), 0.);
        assert!(scroll_speed(px(50.)) > 0.);
        assert_eq!(scroll_speed(px(-50.)), -scroll_speed(px(50.)));
        assert_eq!(scroll_speed(px(5000.)), 2400.);
    }

    #[test]
    fn active_frames_stop_at_edges_and_restart_when_the_direction_changes() {
        let extent = px(1000.);
        assert!(!can_scroll_towards(Pixels::ZERO, extent, 0.));
        assert!(!can_scroll_towards(Pixels::ZERO, extent, -15.));
        assert!(can_scroll_towards(Pixels::ZERO, extent, 15.));
        assert!(!can_scroll_towards(extent, extent, 15.));
        assert!(can_scroll_towards(extent, extent, -15.));
    }
}
