//! Replays the last pixels of a departed item without carrying its input state.

use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

use gpui::{IntoElement, SpanId, Styled, canvas};

use super::{MOTION, out_quad};

#[derive(Clone, Default)]
pub(in crate::ui) struct ExitingSpans(Rc<RefCell<Vec<Fade>>>);

struct Fade {
    id: SpanId,
    started: Instant,
    previous_alpha: f32,
}

#[derive(Clone, Copy)]
enum Sample {
    Elapsed(Duration),
    At(Instant),
}

impl ExitingSpans {
    pub(in crate::ui) fn start(&self, id: SpanId, started: Instant) {
        let mut fades = self.0.borrow_mut();
        if !fades.iter().any(|fade| fade.id == id) {
            fades.push(Fade {
                id,
                started,
                previous_alpha: 1.,
            });
        }
    }

    pub(in crate::ui) fn forget(&self, id: SpanId) {
        self.0.borrow_mut().retain(|fade| fade.id != id);
    }

    pub(in crate::ui) fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn painted_alpha(&self, id: SpanId) -> f32 {
        self.0
            .borrow()
            .iter()
            .find(|fade| fade.id == id)
            .map_or(0., |fade| fade.previous_alpha)
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn strongest_alpha(&self) -> f32 {
        self.0
            .borrow()
            .iter()
            .map(|fade| fade.previous_alpha)
            .fold(0., f32::max)
    }

    pub(in crate::ui) fn layer(&self, animate: bool, sample: Option<Duration>) -> impl IntoElement {
        self.replay(animate, sample.map(Sample::Elapsed))
    }

    pub(in crate::ui) fn layer_at(
        &self,
        animate: bool,
        sample: Option<Instant>,
    ) -> impl IntoElement {
        self.replay(animate, sample.map(Sample::At))
    }

    fn replay(&self, animate: bool, sample: Option<Sample>) -> impl IntoElement {
        let fades = self.0.clone();
        canvas(
            |_, _, _| (),
            move |_, (), window, cx| {
                let mut fades = fades.borrow_mut();
                let live = window.is_visible() && window.is_window_active();
                let had_fades = !fades.is_empty();
                if !animate || cx.reduce_motion() || (!live && sample.is_none()) {
                    fades.clear();
                    return;
                }
                fades.retain_mut(|fade| {
                    let elapsed = match sample {
                        Some(Sample::Elapsed(elapsed)) => elapsed,
                        Some(Sample::At(now)) => now.saturating_duration_since(fade.started),
                        None => fade.started.elapsed(),
                    };
                    let alpha = (1.
                        - out_quad(elapsed.as_secs_f32() / MOTION.normal().as_secs_f32()))
                    .min(fade.previous_alpha);
                    if alpha <= 0. || !window.replay_span(fade.id, alpha / fade.previous_alpha) {
                        return false;
                    }
                    // Replay draws from the preceding frame, which already includes its fade.
                    fade.previous_alpha = alpha;
                    true
                });
                if live && sample.is_none() && had_fades {
                    window.request_animation_frame();
                }
            },
        )
        .absolute()
        .inset_0()
    }
}
