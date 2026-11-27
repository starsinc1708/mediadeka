use std::{
    cell::Cell,
    time::{Duration, Instant},
};

use gpui::{App, Pixels, Window, px};

use crate::ui::{
    design_system::motion,
    fluid_glass::{Geometry, tokens, transition_motion_configured},
    music::detail::hero_motion::HeroMotion,
    workspace::Page,
};

pub(in crate::ui::workspace) struct TransitionCoordinator {
    fluid: bool,
    started: Cell<Option<Instant>>,
    pub(in crate::ui::workspace) artwork: Option<HeroMotion>,
    #[cfg(feature = "capture")]
    sample: Option<Duration>,
}

impl TransitionCoordinator {
    pub(in crate::ui::workspace) fn new(fluid: bool) -> Self {
        Self {
            fluid,
            started: Cell::new(None),
            artwork: None,
            #[cfg(feature = "capture")]
            sample: None,
        }
    }

    pub(in crate::ui::workspace) fn select(&mut self, previous: Page, next: Page, immediate: bool) {
        self.artwork = None;
        if self.fluid && transition_motion_configured() && !immediate && previous != next {
            self.started.set(Some(Instant::now()));
        } else {
            self.started.set(None);
        }
    }

    pub(in crate::ui::workspace) fn permits_artwork(&self, window: &Window, cx: &App) -> bool {
        self.fluid
            && transition_motion_configured()
            && !cx.reduce_motion()
            && !window.last_input_was_keyboard()
    }

    pub(in crate::ui::workspace) fn presentation(
        &self,
        window: &Window,
        reduced_motion: bool,
    ) -> (Pixels, f32) {
        let Some(started) = self.started.get() else {
            return (Pixels::ZERO, 1.);
        };
        #[cfg(feature = "capture")]
        let sample = self.sample;
        #[cfg(not(feature = "capture"))]
        let sample: Option<Duration> = None;
        if reduced_motion
            || (sample.is_none() && (!window.is_visible() || !window.is_window_active()))
        {
            self.started.set(None);
            return (Pixels::ZERO, 1.);
        }
        let elapsed = sample.unwrap_or_else(|| started.elapsed());
        let duration = motion::MOTION.normal();
        if elapsed >= duration {
            self.started.set(None);
            return (Pixels::ZERO, 1.);
        }
        if sample.is_none() {
            window.request_animation_frame();
        }
        let progress = motion::out_quad(elapsed.as_secs_f32() / duration.as_secs_f32());
        let distance = tokens::PAGE_OFFSET.min(Geometry::for_window(window).content_bottom());
        let opacity = if self.artwork.is_some() {
            1.
        } else {
            0.86 + 0.14 * progress
        };
        (px(distance * (1. - progress)), opacity)
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui::workspace) fn sample(&mut self, elapsed: Option<Duration>) {
        self.sample = elapsed;
    }
}

#[cfg(feature = "capture")]
impl crate::ui::MainView {
    pub fn set_fluid_glass_page_motion_for_capture(
        &mut self,
        sample: Option<Duration>,
        cx: &mut gpui::Context<Self>,
    ) {
        self.workspace.transitions.sample(sample);
        cx.notify();
    }
}
