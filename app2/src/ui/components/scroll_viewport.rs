//! Composites scroll content against the shell with GPUI's native edge mask.

mod read_ahead;

pub(in crate::ui) use read_ahead::ReadAhead;

use gpui::{
    AnyElement, App, Bounds, ContentMask, DispatchPhase, Element, ElementId, GlobalElementId,
    Hitbox, HitboxBehavior, InspectorElementId, IntoElement, LayoutId, Pixels, ScrollWheelEvent,
    Window, div, prelude::*, px,
};

use crate::ui::design_system::motion::smoothstep;

use super::scrollbar::ScrollSurface;

#[derive(Clone, Copy)]
pub(in crate::ui) enum Density {
    Content,
    Rows(Pixels),
    Panel,
}

/// Wrap only the scrolling layer. Scrollbars and pinned controls remain
/// siblings. Reading geometry in paint uses today's prepaint, including the
/// post-layout viewport and the clamped/remeasured scroll position.
pub(in crate::ui) struct FluidScrollViewport {
    content: Option<AnyElement>,
    surface: ScrollSurface,
    enabled: bool,
    density: Density,
    side: Pixels,
    header: Pixels,
}

impl FluidScrollViewport {
    pub(in crate::ui) fn new(
        content: impl IntoElement,
        surface: ScrollSurface,
        enabled: bool,
        density: Density,
    ) -> Self {
        Self {
            content: Some(content.into_any_element()),
            surface,
            enabled,
            density,
            side: Pixels::ZERO,
            header: Pixels::ZERO,
        }
    }

    /// The workspace may reserve a lateral transition next to its shell rim.
    pub(in crate::ui) fn side_fade(mut self, side: Pixels) -> Self {
        self.side = side;
        self
    }

    /// Reserve the visible footprint of an overlay without resizing the scroll
    /// layout. Only the exposed part of the viewport paints or accepts clicks.
    pub(in crate::ui) fn under_header(mut self, height: Pixels) -> Self {
        self.header = height;
        self
    }

    fn visible_bounds(&self, mut bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        let inset = self.header.clamp(Pixels::ZERO, bounds.size.height);
        bounds.origin.y += inset;
        bounds.size.height -= inset;
        bounds
    }
}

impl IntoElement for FluidScrollViewport {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for FluidScrollViewport {
    type RequestLayoutState = AnyElement;
    type PrepaintState = Option<Hitbox>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let mut content = self
            .content
            .take()
            .expect("viewport is laid out once per frame");
        (content.request_layout(window, cx), content)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        content: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Hitbox> {
        if self.header <= Pixels::ZERO {
            content.prepaint(window, cx);
            return None;
        }
        let visible = self.visible_bounds(bounds);
        window.with_content_mask(Some(ContentMask { bounds: visible }), |window| {
            content.prepaint(window, cx);
        });
        let mut header = bounds;
        header.size.height = visible.top() - bounds.top();
        Some(window.insert_hitbox(header, HitboxBehavior::Normal))
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        content: &mut AnyElement,
        header: &mut Option<Hitbox>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(header) = header.take() {
            let surface = self.surface.clone();
            window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                if phase == DispatchPhase::Bubble
                    && header.should_handle_scroll(window)
                    && surface.set_offset(surface.offset() - event.delta.pixel_delta(px(20.)).y)
                {
                    cx.stop_propagation();
                    window.refresh();
                }
            });
            let visible = self.visible_bounds(bounds);
            window.with_content_mask(Some(ContentMask { bounds: visible }), |window| {
                self.paint_visible(visible, content, window, cx);
            });
        } else {
            self.paint_visible(bounds, content, window, cx);
        }
    }
}

impl FluidScrollViewport {
    fn paint_visible(
        &self,
        bounds: Bounds<Pixels>,
        content: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        if !self.enabled {
            content.paint(window, cx);
            return;
        }

        #[cfg(feature = "capture")]
        if let Some(settings) = cx.try_global::<CapturePaint>() {
            if !settings.visible {
                return;
            }
            if !settings.mask {
                content.paint(window, cx);
                return;
            }
        }

        let height = self
            .surface
            .viewport()
            .size
            .height
            .min(bounds.size.height + self.header);
        let (top, bottom) = fade_amounts(
            height,
            self.surface.offset(),
            self.surface.extent(),
            self.density,
        );
        let side = self.side.min(bounds.size.width * 0.10);
        // GPUI List accepts Styled::fade_edges but its paint path does not
        // resolve style filters. Apply that same filter around the completed
        // child paint, after the list has measured its visible items.
        let filter = div()
            .fade_edges(
                top.min(bounds.size.height * 0.20),
                bottom.min(bounds.size.height * 0.45),
            )
            .fade_sides(side, side)
            .style()
            .filter
            .unwrap_or_default();
        window.with_filter(bounds, filter, Pixels::ZERO, |window| {
            content.paint(window, cx)
        });
    }
}

#[cfg(feature = "capture")]
struct CapturePaint {
    mask: bool,
    visible: bool,
}

#[cfg(feature = "capture")]
impl gpui::Global for CapturePaint {}

#[cfg(feature = "capture")]
impl crate::ui::MainView {
    pub fn set_scroll_content_paint_for_capture(mask: bool, visible: bool, cx: &mut App) {
        cx.set_global(CapturePaint { mask, visible });
    }
}

fn fade_amounts(
    height: Pixels,
    offset: Pixels,
    extent: Pixels,
    density: Density,
) -> (Pixels, Pixels) {
    if height <= Pixels::ZERO || extent <= Pixels::ZERO {
        return (Pixels::ZERO, Pixels::ZERO);
    }
    let scale = match density {
        Density::Content => 1.,
        Density::Rows(height) => (height / px(48.)).clamp(0.72, 1.05),
        Density::Panel => 0.72,
    };
    let offset = offset.clamp(Pixels::ZERO, extent);
    let top = ((height * 0.05).clamp(px(12.), px(32.)) * scale).min(height * 0.14);
    let bottom = ((height * 0.07).clamp(px(16.), px(48.)) * scale).min(height * 0.18);
    let zone = |distance: Pixels, width: Pixels| {
        let share = (distance / width).clamp(0., 1.);
        (width * smoothstep(share)).min(distance)
    };
    (zone(offset, top), zone(extent - offset, bottom))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fits_and_endpoints_leave_the_required_edge_open() {
        let depth = |offset, extent| fade_amounts(px(600.), offset, extent, Density::Rows(px(44.)));
        assert_eq!(depth(px(0.), px(0.)), (px(0.), px(0.)));
        assert_eq!(depth(px(-10.), px(500.)).0, px(0.));
        assert_eq!(depth(px(510.), px(500.)).1, px(0.));
        assert!(depth(px(0.), px(500.)).1 > px(0.));
        assert!(depth(px(500.), px(500.)).0 > px(0.));
    }

    #[test]
    fn depth_tracks_measured_height_density_and_the_remaining_scroll() {
        let sample = |height, offset, density| fade_amounts(height, offset, px(700.), density);
        let rows = Density::Rows(px(44.));
        let tall = sample(px(900.), px(350.), rows);
        let compact = sample(px(300.), px(350.), rows);
        let panel = sample(px(300.), px(350.), Density::Panel);
        assert!(tall.0 > compact.0 && tall.1 > compact.1);
        assert!(compact.0 > panel.0 && compact.1 > panel.1);
        assert!(sample(px(900.), px(695.), rows).1 <= px(5.));
    }
}
