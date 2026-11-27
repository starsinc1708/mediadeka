use gpui::{App, Context, Window};

use crate::{
    providers::ProviderId,
    ui::{
        design_system::motion::{MOTION, Slide},
        fluid_glass::transition_motion_configured,
    },
};

use super::MainView;

pub(super) type ShelfKey = (ProviderId, usize);

#[derive(Clone, Copy)]
pub(super) struct ShelfTurn {
    slide: Slide,
    page_layout: (usize, usize),
}

impl MainView {
    pub(super) fn shelf_motion_enabled(&self, cx: &App) -> bool {
        self.uses_fluid_glass()
            && transition_motion_configured()
            && !self.reduced_motion
            && !cx.reduce_motion()
            && !self.fluid_high_contrast(cx)
    }

    pub(super) fn shelf_turn_position(
        &self,
        key: ShelfKey,
        page_layout: (usize, usize),
        cx: &App,
    ) -> Option<f32> {
        let now = self.presentation_time();
        let turn = self.home.shelf_turns.get(&key)?;
        (self.shelf_motion_enabled(cx) && turn.page_layout == page_layout && turn.slide.active(now))
            .then(|| turn.slide.position(now))
    }

    pub(super) fn turn_shelf(
        &mut self,
        key: ShelfKey,
        (size, count): (usize, usize),
        next: bool,
        by_pointer: bool,
        cx: &mut Context<Self>,
    ) {
        let last = count.saturating_sub(1);
        let selected = self
            .home
            .shelf_pages
            .get(&key)
            .copied()
            .unwrap_or(0)
            .min(last);
        let target = if next {
            (selected + 1).min(last)
        } else {
            selected.saturating_sub(1)
        };
        if target == selected {
            return;
        }
        let now = self.presentation_time();
        let from = self
            .shelf_turn_position(key, (size, count), cx)
            .unwrap_or(selected as f32);
        self.home
            .shelf_turns
            .retain(|_, turn| turn.slide.active(now));
        self.home.shelf_turns.remove(&key);
        if by_pointer && self.shelf_motion_enabled(cx) {
            self.home.shelf_turns.insert(
                key,
                ShelfTurn {
                    slide: Slide::new(from, target as f32, now, MOTION.normal()),
                    page_layout: (size, count),
                },
            );
        }
        self.home.shelf_pages.insert(key, target);
        cx.notify();
    }

    #[cfg(feature = "capture")]
    pub fn home_shelf_turn_for_capture(&self, provider: ProviderId, shelf: usize) -> (usize, f32) {
        let key = (provider, shelf);
        let target = self.home.shelf_pages.get(&key).copied().unwrap_or(0);
        let position = self
            .home
            .shelf_turns
            .get(&key)
            .map_or(target as f32, |turn| {
                turn.slide.position(self.presentation_time())
            });
        (target, position)
    }
}

pub(super) fn keep_shelf_moving(window: &mut Window) {
    if window.is_visible() && window.is_window_active() {
        window.request_animation_frame();
    }
}

#[cfg(feature = "capture")]
pub(super) fn arrow_span(shelf: usize, next: bool) -> gpui::SpanId {
    crate::ui::music::detail::hero_motion::cover_span(("home-shelf-arrow", shelf, next))
}
