use std::{
    hash::{Hash, Hasher},
    time::{Duration, Instant},
};

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayerFilter, LayoutId, Pixels, SpanId, Window, deferred, point, size,
};

use crate::ui::design_system::motion::{MOTION, out_quad};

pub(in crate::ui) const HERO_ARTWORK_SPAN: SpanId = 0x6d65646961686572;
pub(in crate::ui) const PLAYER_ARTWORK_SPAN: SpanId = 0x6d65646961706c79;
pub(in crate::ui) const IMMERSIVE_ARTWORK_SPAN: SpanId = 0x6d65646961696d6d;

#[derive(Clone, Copy)]
pub(in crate::ui) struct HeroMotion {
    origin: Bounds<Pixels>,
    target: SpanId,
    started: Instant,
    duration: Duration,
    #[cfg(feature = "capture")]
    sample: Option<Duration>,
}

impl HeroMotion {
    pub(in crate::ui) fn start(origin: Bounds<Pixels>, target: SpanId) -> Self {
        Self::with_duration(origin, target, MOTION.shared_artwork())
    }

    pub(in crate::ui) fn with_duration(
        origin: Bounds<Pixels>,
        target: SpanId,
        duration: Duration,
    ) -> Self {
        Self::with_start(origin, target, Instant::now(), duration)
    }

    pub(in crate::ui) fn with_start(
        origin: Bounds<Pixels>,
        target: SpanId,
        started: Instant,
        duration: Duration,
    ) -> Self {
        Self {
            origin,
            target,
            started,
            duration,
            #[cfg(feature = "capture")]
            sample: None,
        }
    }

    fn age(self) -> Duration {
        #[cfg(feature = "capture")]
        if let Some(sample) = self.sample {
            return sample;
        }
        self.started.elapsed()
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn sample(&mut self, elapsed: Duration) -> Bounds<Pixels> {
        self.sample = Some(elapsed);
        self.origin
    }

    pub(in crate::ui) fn opacity(self) -> f32 {
        0.72 + 0.28 * out_quad(self.progress())
    }

    pub(in crate::ui) fn progress(self) -> f32 {
        (self.age().as_secs_f32() / self.duration.as_secs_f32()).clamp(0., 1.)
    }

    /// Progress of one staged reveal inside the shared transition. `start` and
    /// `end` are fractions of the transition, so every phase lands before the
    /// cover reaches its target.
    pub(in crate::ui) fn phased(self, start: f32, end: f32) -> f32 {
        let (start, end) = (start.clamp(0., 1.), end.clamp(0., 1.));
        if end <= start {
            return f32::from(u8::from(self.progress() >= end));
        }
        out_quad(((self.progress() - start) / (end - start)).clamp(0., 1.))
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn sampled(self) -> bool {
        self.sample.is_some()
    }

    #[cfg(not(feature = "capture"))]
    pub(in crate::ui) fn sampled(self) -> bool {
        false
    }

    pub(in crate::ui) fn active(self) -> bool {
        self.age() < self.duration
    }

    pub(in crate::ui) fn painted_square_edge(self, edge: Pixels) -> Pixels {
        let square = Bounds::new(point(Pixels::ZERO, Pixels::ZERO), size(edge, edge));
        self.filter(square, self.age())
            .map_or(edge, |(bounds, _)| bounds.size.width)
    }

    fn filter(
        self,
        bounds: Bounds<Pixels>,
        elapsed: Duration,
    ) -> Option<(Bounds<Pixels>, LayerFilter)> {
        if elapsed >= self.duration
            || bounds.size.width <= Pixels::ZERO
            || bounds.size.height <= Pixels::ZERO
        {
            return None;
        }
        let remaining = 1. - out_quad(elapsed.as_secs_f32() / self.duration.as_secs_f32());
        let initial_scale = (self.origin.size.width / bounds.size.width)
            .min(self.origin.size.height / bounds.size.height);
        let scale = 1. + (initial_scale - 1.) * remaining;
        let offset = self.origin.center() - bounds.center();
        let translate = point(offset.x * remaining, offset.y * remaining);
        let size = size(bounds.size.width * scale, bounds.size.height * scale);
        let origin = bounds.center() + translate - point(size.width * 0.5, size.height * 0.5);
        Some((
            Bounds { origin, size },
            LayerFilter {
                scale: Some(scale),
                translate: Some(translate),
                ..Default::default()
            },
        ))
    }
}

pub(in crate::ui) fn cover_span(key: impl Hash) -> SpanId {
    let mut hash = std::hash::DefaultHasher::new();
    "mediadeka.hero-cover".hash(&mut hash);
    key.hash(&mut hash);
    hash.finish()
}

pub(in crate::ui) fn shared_artwork(
    cover: impl IntoElement,
    id: SpanId,
    motion: Option<HeroMotion>,
) -> AnyElement {
    let motion = motion.filter(|motion| motion.target == id && motion.active());
    let artwork = MovingArtwork {
        child: Some(cover.into_any_element()),
        id,
        motion,
        replayable: false,
    };
    if motion.is_some() {
        deferred(artwork).into_any_element()
    } else {
        artwork.into_any_element()
    }
}

/// Moves painted pixels without touching layout or hitboxes. Reveals use it so
/// the transport keeps its working coordinates while it settles into place.
pub(in crate::ui) fn paint_offset(child: impl IntoElement, offset: Pixels) -> AnyElement {
    PaintOffset {
        child: Some(child.into_any_element()),
        offset,
    }
    .into_any_element()
}

struct PaintOffset {
    child: Option<AnyElement>,
    offset: Pixels,
}

impl IntoElement for PaintOffset {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for PaintOffset {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

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
        let mut child = self
            .child
            .take()
            .expect("paint offset child is laid out once per frame");
        (child.request_layout(window, cx), child)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        child: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        child: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        if self.offset == Pixels::ZERO {
            child.paint(window, cx);
            return;
        }
        let filter = LayerFilter {
            translate: Some(point(Pixels::ZERO, self.offset)),
            ..Default::default()
        };
        window.with_filter(bounds, filter, Pixels::ZERO, |window| {
            child.paint(window, cx)
        });
    }
}

/// Tracks a live list item within its scroll layer and retains its painted operations.
pub(in crate::ui) fn painted_fragment(
    child: impl IntoElement,
    id: SpanId,
    motion: Option<HeroMotion>,
) -> AnyElement {
    MovingArtwork {
        child: Some(child.into_any_element()),
        id,
        motion: motion.filter(|motion| motion.target == id && motion.active()),
        replayable: true,
    }
    .into_any_element()
}

struct MovingArtwork {
    child: Option<AnyElement>,
    id: SpanId,
    motion: Option<HeroMotion>,
    replayable: bool,
}

impl IntoElement for MovingArtwork {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for MovingArtwork {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();

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
        let mut artwork = self
            .child
            .take()
            .expect("artwork is laid out once per frame");
        (artwork.request_layout(window, cx), artwork)
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        artwork: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        artwork.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        artwork: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let transform = self
            .motion
            .filter(|_| !cx.reduce_motion())
            .and_then(|motion| motion.filter(bounds, motion.age()));
        let visible = transform.as_ref().map_or(bounds, |(visible, _)| *visible);
        let mut paint = |window: &mut Window| {
            if let Some((_, transform)) = transform {
                window.with_filter(bounds, transform, Pixels::ZERO, |window| {
                    artwork.paint(window, cx)
                });
            } else {
                artwork.paint(window, cx);
            }
        };
        if transform.is_some() && window.is_visible() && window.is_window_active() {
            window.request_animation_frame();
        }
        if self.replayable && visible_span(window, visible) {
            window.with_paint_span(self.id, visible, paint);
        } else {
            if visible_span(window, visible) {
                window.with_paint_span(self.id, visible, |_| {});
            }
            paint(window);
        }
    }
}

fn visible_span(window: &Window, bounds: Bounds<Pixels>) -> bool {
    bounds.size.width > Pixels::ZERO
        && bounds.size.height > Pixels::ZERO
        && bounds.intersects(&window.content_mask().bounds)
}

#[cfg(test)]
mod tests {
    use super::{HERO_ARTWORK_SPAN, HeroMotion, MOTION};
    use gpui::{Bounds, point, px, size};

    #[test]
    fn square_starts_at_its_card_without_changing_aspect_ratio() {
        let source = Bounds {
            origin: point(px(60.), px(220.)),
            size: size(px(140.), px(140.)),
        };
        let target = Bounds {
            origin: point(px(308.), px(93.)),
            size: size(px(300.), px(300.)),
        };
        let motion = HeroMotion::start(source, HERO_ARTWORK_SPAN);
        let duration = MOTION.shared_artwork();
        let (painted_start, start) = motion.filter(target, std::time::Duration::ZERO).unwrap();
        assert!((start.scale.unwrap() - 140. / 300.).abs() < 0.00001);
        assert!((f32::from(painted_start.origin.x - source.origin.x)).abs() < 0.001);
        assert!((f32::from(painted_start.origin.y - source.origin.y)).abs() < 0.001);
        let (painted_middle, middle) = motion.filter(target, duration / 2).unwrap();
        assert!(middle.scale.unwrap() > start.scale.unwrap());
        assert!(motion.filter(target, duration).is_none());

        let reverse = HeroMotion::start(painted_middle, 1);
        let (reverse_start, _) = reverse.filter(source, std::time::Duration::ZERO).unwrap();
        assert!((f32::from(reverse_start.size.width - painted_middle.size.width)).abs() < 0.001);
        assert!((f32::from(reverse_start.origin.x - painted_middle.origin.x)).abs() < 0.001);
    }
}
