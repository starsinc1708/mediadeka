use std::{
    cell::RefCell,
    collections::HashMap,
    time::{Duration, Instant},
};

use gpui::{IntoElement, SpanId, Window};

use crate::ui::{
    design_system::motion::{ExitingSpans, MOTION, out_quad},
    music::detail::hero_motion::HeroMotion,
};

const LAYOUT_TRAVEL: Duration = Duration::from_millis(280);

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Surface {
    Row,
    Cover,
}

/// The list only requests geometry for the rows it lays out. A new projection
/// takes each of those origins from the previous painted frame.
#[derive(Default)]
pub(super) struct Reflow {
    transition: Option<LayoutChange>,
    pub(super) exits: ExitingSpans,
    #[cfg(feature = "capture")]
    pub sample: Option<Duration>,
}

struct LayoutChange {
    began: Instant,
    surface: Surface,
    moving: RefCell<HashMap<SpanId, Option<HeroMotion>>>,
}

impl Reflow {
    pub(super) fn paint_exits(&self, animate: bool) -> impl IntoElement {
        self.exits.layer(animate, self.sample_time())
    }

    pub(super) fn empty_opacity(&self) -> f32 {
        if self.exits.is_empty() {
            return 1.;
        }
        self.transition.as_ref().map_or(1., |transition| {
            let elapsed = self
                .sample_time()
                .unwrap_or_else(|| transition.began.elapsed());
            let progress = elapsed.as_secs_f32() / MOTION.normal().as_secs_f32();
            out_quad(progress * 2. - 1.)
        })
    }

    fn sample_time(&self) -> Option<Duration> {
        #[cfg(feature = "capture")]
        {
            self.sample
        }
        #[cfg(not(feature = "capture"))]
        {
            None
        }
    }

    pub(super) fn begin(&mut self, surface: Surface) {
        self.transition = Some(LayoutChange {
            began: Instant::now(),
            surface,
            moving: RefCell::default(),
        });
    }

    pub(super) fn at(
        &self,
        id: SpanId,
        surface: Surface,
        window: &Window,
    ) -> (Option<HeroMotion>, f32) {
        let Some(change) = self
            .transition
            .as_ref()
            .filter(|change| change.surface == surface)
        else {
            return (None, 1.);
        };
        let sample = self.sample_time();
        let elapsed = sample.unwrap_or_else(|| change.began.elapsed());
        if elapsed >= LAYOUT_TRAVEL {
            change.moving.borrow_mut().clear();
            return (None, 1.);
        }
        let Some(motion) = *change.moving.borrow_mut().entry(id).or_insert_with(|| {
            window
                .rendered_span(id)
                .map(|origin| HeroMotion::with_start(origin, id, change.began, LAYOUT_TRAVEL))
        }) else {
            if sample.is_none() && window.is_visible() && window.is_window_active() {
                window.request_animation_frame();
            }
            let opacity = out_quad(elapsed.as_secs_f32() / LAYOUT_TRAVEL.as_secs_f32());
            return (None, 0.24 + 0.76 * opacity);
        };
        #[cfg(feature = "capture")]
        let motion = sample.map_or(motion, |sample| {
            let mut motion = motion;
            motion.sample(sample);
            motion
        });
        (Some(motion), 1.)
    }
}
