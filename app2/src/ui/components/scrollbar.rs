//! Vertical scrollbar geometry follows Guise's scrollbar.

use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    time::{Duration, Instant},
};

use gpui::{
    Bounds, Div, Hsla, ListState, Orientation, Pixels, Role, ScrollHandle, Stateful, Window, div,
    fill, point, prelude::*, px, rgb, size,
};

use crate::ui::{
    fluid_glass::{ShellGeometry, tokens as fluid},
    theme::{SCROLLBAR_THUMB, SCROLLBAR_THUMB_HOVER, SCROLLBAR_TROUGH, playlist},
};

// A 12 DIP hit area around a 4 DIP bar: quiet at rest, easier to grab.
const TRACK_WIDTH: Pixels = px(14.);
const TRACK_INSET: Pixels = px(5.);
const MIN_THUMB_HEIGHT: Pixels = px(26.);
const VISUAL_WIDTH: f32 = 4.;
const VISUAL_WIDTH_HOVER: f32 = 6.;
const REST_THUMB_ALPHA: f32 = 0.24;
const HOVER_THUMB_ALPHA: f32 = 0.92;
const SCROLL_THUMB_ALPHA: f32 = 0.66;
const SCROLL_HOLD: Duration = Duration::from_millis(760);
const FADE_RESPONSE: f32 = 18.;

/// Per-scrollbar transient state, keyed by its viewport. Moving the pointer or
/// using the wheel raises the indicator; after a quiet moment it settles back.
/// The element itself stays in place, so its hitbox never moves.
#[derive(Clone, Copy)]
struct BarFade {
    alpha: f32,
    last_tick: Instant,
    last_offset: Pixels,
    last_scroll: Instant,
}

thread_local! {
    static BAR_FADES: RefCell<HashMap<(u32, u32, u32, u32), BarFade>> =
        RefCell::new(HashMap::new());
}

fn bar_key(bounds: Bounds<Pixels>) -> (u32, u32, u32, u32) {
    (
        f32::from(bounds.origin.x).round() as u32,
        f32::from(bounds.origin.y).round() as u32,
        f32::from(bounds.size.width).round() as u32,
        f32::from(bounds.size.height).round() as u32,
    )
}

fn fade_step(current: f32, target: f32, elapsed: Duration) -> f32 {
    let response = 1. - (-elapsed.as_secs_f32() * FADE_RESPONSE).exp();
    current + (target - current) * response
}

fn scrollbar_alpha(surface: &ScrollSurface, hover: bool, dragging: bool, window: &Window) -> f32 {
    let now = Instant::now();
    let key = bar_key(surface.viewport());
    let offset = surface.offset();
    BAR_FADES.with(|fades| {
        let mut fades = fades.borrow_mut();
        if fades.len() > 24 {
            fades.retain(|_, state| now.duration_since(state.last_tick) < Duration::from_secs(8));
        }
        let state = fades.entry(key).or_insert(BarFade {
            alpha: REST_THUMB_ALPHA,
            last_tick: now,
            last_offset: offset,
            last_scroll: now - SCROLL_HOLD,
        });
        if state.last_offset != offset {
            state.last_offset = offset;
            state.last_scroll = now;
        }
        let target = if dragging {
            0.96
        } else if hover {
            HOVER_THUMB_ALPHA
        } else if now.duration_since(state.last_scroll) < SCROLL_HOLD {
            SCROLL_THUMB_ALPHA
        } else {
            REST_THUMB_ALPHA
        };
        let elapsed = now.saturating_duration_since(state.last_tick);
        state.last_tick = now;
        state.alpha = fade_step(state.alpha, target, elapsed);
        if (state.alpha - target).abs() > 0.004 {
            window.request_animation_frame();
        }
        state.alpha
    })
}

/// Placement of the scrollbar inside the safe scroll viewport. The fluid
/// values come from `ShellGeometry`, so the bar cannot enter the inner elbow
/// or the player; classic keeps its original five-DIP inset.
#[derive(Clone, Copy, Debug)]
pub(in crate::ui) struct ScrollChrome {
    pub(in crate::ui) top: Pixels,
    pub(in crate::ui) bottom: Pixels,
    pub(in crate::ui) right: Pixels,
}

impl ScrollChrome {
    pub(in crate::ui) fn new(fluid_glass: bool, window: &Window) -> Self {
        if fluid_glass {
            let insets = ShellGeometry::for_window(window).scrollbar_insets();
            Self {
                top: insets.top,
                bottom: insets.bottom,
                right: insets.right,
            }
        } else {
            Self {
                top: TRACK_INSET,
                bottom: TRACK_INSET,
                right: TRACK_INSET,
            }
        }
    }

    #[cfg(test)]
    pub(in crate::ui) fn classic() -> Self {
        Self {
            top: TRACK_INSET,
            bottom: TRACK_INSET,
            right: TRACK_INSET,
        }
    }
}

pub(in crate::ui) fn with_list_height_hints(
    viewport: Div,
    list: &ListState,
    row_height: Pixels,
) -> Div {
    let list = list.clone();
    let previous_width = Cell::new(list.viewport_bounds().size.width);
    viewport.on_children_prepainted(move |_, window, _| {
        let width = list.viewport_bounds().size.width;
        if previous_width.replace(width) != width {
            // GPUI clears estimates on width changes. Restore them before
            // painting the scrollbar or letting a drag capture its extent.
            list.clone().with_uniform_item_height(row_height);
            // Accessibility geometry was built before list layout.
            window.on_next_frame(|window, _| window.refresh());
        }
    })
}

#[derive(Clone)]
pub(in crate::ui) enum ScrollSurface {
    Area(ScrollHandle),
    VirtualList(ListState),
}

impl ScrollSurface {
    pub(in crate::ui) fn viewport(&self) -> Bounds<Pixels> {
        match self {
            Self::Area(handle) => handle.bounds(),
            Self::VirtualList(list) => list.viewport_bounds(),
        }
    }

    pub(in crate::ui) fn extent(&self) -> Pixels {
        match self {
            Self::Area(handle) => handle.max_offset().y,
            Self::VirtualList(list) => list.max_offset_for_scrollbar().y,
        }
    }

    pub(in crate::ui) fn offset(&self) -> Pixels {
        match self {
            Self::Area(handle) => scroll_offset(handle),
            Self::VirtualList(list) => -list.scroll_px_offset_for_scrollbar().y,
        }
    }

    pub(in crate::ui) fn offset_for_key(&self, key: &str, step: Pixels) -> Option<Pixels> {
        Some(match key {
            "up" => self.offset() - step,
            "down" => self.offset() + step,
            "pageup" => self.offset() - self.viewport().size.height,
            "pagedown" => self.offset() + self.viewport().size.height,
            "home" => Pixels::ZERO,
            "end" => self.extent(),
            _ => return None,
        })
    }

    pub(in crate::ui) fn set_offset(&self, desired: Pixels) -> bool {
        match self {
            Self::Area(handle) => set_handle_offset(handle, desired),
            Self::VirtualList(list) => {
                let desired = desired.clamp(Pixels::ZERO, self.extent());
                if desired == self.offset() {
                    return false;
                }
                list.set_offset_from_scrollbar(point(Pixels::ZERO, -desired));
                true
            }
        }
    }

    pub(in crate::ui) fn start_drag(&self) {
        if let Self::VirtualList(list) = self {
            list.scrollbar_drag_started();
        }
    }

    pub(in crate::ui) fn end_drag(&self) {
        if let Self::VirtualList(list) = self {
            list.scrollbar_drag_ended();
        }
    }

    pub(in crate::ui) fn track_with(&self, chrome: ScrollChrome) -> Option<Track> {
        Track::new(self.viewport(), self.extent(), chrome)
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn track_for_capture(
        &self,
        chrome: ScrollChrome,
    ) -> Option<(Bounds<Pixels>, Pixels)> {
        let track = self.track_with(chrome)?;
        Some((
            track.bounds,
            track.thumb_top(self.offset()) + track.thumb_height / 2.,
        ))
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn scrollbar_size_for_capture(
        &self,
        chrome: ScrollChrome,
    ) -> Option<(f32, f32)> {
        let track = self.track_with(chrome)?;
        Some((f32::from(track.extent), f32::from(track.thumb_height)))
    }
}

pub(in crate::ui) fn scrollbar_element(
    id: &'static str,
    label: &'static str,
    surface: &ScrollSurface,
    chrome: ScrollChrome,
) -> Stateful<Div> {
    div()
        .id(id)
        .accessibility_id(id)
        .role(Role::ScrollBar)
        .aria_label(label)
        .aria_orientation(Orientation::Vertical)
        .aria_min_numeric_value(0.)
        .aria_max_numeric_value(f64::from(f32::from(surface.extent())))
        .aria_numeric_value(f64::from(f32::from(surface.offset())))
        .absolute()
        .top(chrome.top)
        .bottom(chrome.bottom)
        .right(chrome.right)
        .w(TRACK_WIDTH)
        .cursor_pointer()
}

pub(in crate::ui) struct Track {
    pub(in crate::ui) bounds: Bounds<Pixels>,
    extent: Pixels,
    thumb_height: Pixels,
    travel: Pixels,
}

impl Track {
    fn new(viewport: Bounds<Pixels>, extent: Pixels, chrome: ScrollChrome) -> Option<Self> {
        let height = viewport.size.height - chrome.top - chrome.bottom;
        if extent <= px(0.5) || height <= Pixels::ZERO {
            return None;
        }
        let ratio = viewport.size.height / (viewport.size.height + extent);
        let thumb_height = (height * ratio).max(MIN_THUMB_HEIGHT).min(height);
        Some(Self {
            bounds: Bounds::new(
                point(
                    viewport.right() - chrome.right - TRACK_WIDTH,
                    viewport.top() + chrome.top,
                ),
                size(TRACK_WIDTH, height),
            ),
            extent,
            thumb_height,
            travel: height - thumb_height,
        })
    }

    fn thumb_top(&self, offset: Pixels) -> Pixels {
        self.bounds.top() + self.travel * (offset / self.extent).clamp(0., 1.)
    }

    pub(in crate::ui) fn grab_offset(&self, pointer_y: Pixels, offset: Pixels) -> Pixels {
        let from_thumb = pointer_y - self.thumb_top(offset);
        if (Pixels::ZERO..=self.thumb_height).contains(&from_thumb) {
            from_thumb
        } else {
            self.thumb_height / 2.
        }
    }

    pub(in crate::ui) fn offset_at(&self, pointer_y: Pixels, grab: Pixels) -> Pixels {
        if self.travel <= Pixels::ZERO {
            return Pixels::ZERO;
        }
        let fraction = (pointer_y - grab - self.bounds.top()) / self.travel;
        self.extent * fraction.clamp(0., 1.)
    }
}

pub(in crate::ui) fn scroll_offset(scroll: &ScrollHandle) -> Pixels {
    (-scroll.offset().y).clamp(Pixels::ZERO, scroll.max_offset().y)
}

pub(in crate::ui) fn set_handle_offset(handle: &ScrollHandle, desired: Pixels) -> bool {
    let old = handle.offset();
    let new_y = -desired.clamp(Pixels::ZERO, handle.max_offset().y);
    if old.y == new_y {
        return false;
    }
    handle.set_offset(point(old.x, new_y));
    true
}

pub(in crate::ui) fn paint_scrollbar(
    window: &mut Window,
    surface: &ScrollSurface,
    chrome: ScrollChrome,
    dragging: bool,
    on_playlist: bool,
    fluid_glass: bool,
) {
    let Some(track) = surface.track_with(chrome) else {
        return;
    };
    let center = track.bounds.center().x;
    let hover = track.bounds.contains(&window.mouse_position()) || dragging;
    let fluid_alpha = fluid_glass.then(|| scrollbar_alpha(surface, hover, dragging, window));
    let (trough_color, thumb_color, thumb_hover) = if fluid_glass {
        // The glass is darker than the classic shell. A neutral light thumb keeps
        // the control discoverable over both the bright and the smoked areas,
        // and it rests quieter than it reads under the pointer.
        (
            Hsla::from(rgb(fluid::SURFACE)).alpha(0.0),
            Hsla::from(rgb(fluid::SECONDARY)).alpha(REST_THUMB_ALPHA),
            Hsla::from(rgb(fluid::PRIMARY)).alpha(HOVER_THUMB_ALPHA),
        )
    } else if on_playlist {
        (
            Hsla::from(rgb(playlist::PANEL)),
            Hsla::from(rgb(playlist::TRACK_LINE)),
            Hsla::from(rgb(playlist::SIGNAL)),
        )
    } else {
        (
            Hsla::from(rgb(SCROLLBAR_TROUGH)),
            Hsla::from(rgb(SCROLLBAR_THUMB)),
            Hsla::from(rgb(SCROLLBAR_THUMB_HOVER)),
        )
    };
    if fluid_alpha.is_none() {
        let trough = Bounds::new(
            point(center - px(2.), track.bounds.top()),
            size(px(4.), track.bounds.size.height),
        );
        window.paint_quad(fill(trough, trough_color).corner_radii(px(2.)));
    }
    let (width, color) = if let Some(alpha) = fluid_alpha {
        let progress =
            ((alpha - REST_THUMB_ALPHA) / (HOVER_THUMB_ALPHA - REST_THUMB_ALPHA)).clamp(0., 1.);
        let width = px(VISUAL_WIDTH + (VISUAL_WIDTH_HOVER - VISUAL_WIDTH) * progress);
        let color = Hsla::from(rgb(if alpha > 0.5 {
            fluid::PRIMARY
        } else {
            fluid::SECONDARY
        }))
        .alpha(alpha);
        (width, color)
    } else if hover {
        (px(VISUAL_WIDTH_HOVER), thumb_hover)
    } else {
        (px(VISUAL_WIDTH), thumb_color)
    };
    let thumb = Bounds::new(
        point(center - width / 2., track.thumb_top(surface.offset())),
        size(width, track.thumb_height),
    );
    window.paint_quad(fill(thumb, color).corner_radii(width / 2.));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_track_clears_the_bottom_elbow() {
        let viewport = Bounds::new(point(px(20.), px(40.)), size(px(640.), px(500.)));
        let chrome = ScrollChrome {
            top: px(12.),
            bottom: px(48.),
            right: px(8.),
        };
        let track = Track::new(viewport, px(2500.), chrome).expect("overflowing track");
        assert_eq!(track.bounds.bottom(), viewport.bottom() - px(48.));
        assert_eq!(track.bounds.right(), viewport.right() - px(8.));
        assert_eq!(track.bounds.top(), viewport.top() + px(12.));
    }

    #[test]
    fn scrollbar_fade_approaches_its_target_without_overshoot() {
        let mut alpha = REST_THUMB_ALPHA;
        for _ in 0..120 {
            let next = fade_step(alpha, HOVER_THUMB_ALPHA, Duration::from_millis(16));
            assert!(next >= alpha && next <= HOVER_THUMB_ALPHA);
            alpha = next;
        }
        assert!((alpha - HOVER_THUMB_ALPHA).abs() < 0.01);
        for _ in 0..120 {
            alpha = fade_step(alpha, REST_THUMB_ALPHA, Duration::from_millis(16));
        }
        assert!((alpha - REST_THUMB_ALPHA).abs() < 0.01);
    }

    #[test]
    fn thumb_maps_both_ends_of_vertical_scroll() {
        let viewport = Bounds::new(point(px(20.), px(40.)), size(px(640.), px(500.)));
        assert!(Track::new(viewport, Pixels::ZERO, ScrollChrome::classic()).is_none());
        let track =
            Track::new(viewport, px(2500.), ScrollChrome::classic()).expect("overflowing track");
        assert_eq!(track.thumb_top(Pixels::ZERO), track.bounds.top());
        assert_eq!(
            track.thumb_top(track.extent) + track.thumb_height,
            track.bounds.bottom()
        );
        let midpoint_y = track.thumb_top(track.extent / 2.) + track.thumb_height / 2.;
        let midpoint = track.offset_at(midpoint_y, track.thumb_height / 2.);
        assert!((midpoint - track.extent / 2.).abs() < px(0.01));
        assert_eq!(track.offset_at(px(-100.), px(10.)), Pixels::ZERO);
        assert_eq!(track.offset_at(px(1000.), px(10.)), track.extent);
    }
}
