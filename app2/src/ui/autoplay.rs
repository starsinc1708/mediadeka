use gpui::{Context, Div, Role, Toggled, div, prelude::*, px, rgb};

use crate::{
    music::recommendations::radio::{self, Seed},
    playback::ContinuationPoint,
    storage::Store,
};

use super::{
    MainView,
    components::{Activate, button},
    design_system::controls::{ControlState, ControlVariant},
    station::{Job, run},
    theme::{ACCENT, ELEVATED, secondary},
};

#[cfg(all(test, windows))]
pub(super) mod acceptance;

#[derive(Default)]
pub(super) struct AutoplayUi {
    enabled: Option<bool>,
    attempted: Option<ContinuationPoint>,
    pending: Option<Job>,
    notice: Option<String>,
}

impl AutoplayUi {
    pub(super) fn enabled(&self) -> bool {
        self.enabled.unwrap_or_default()
    }

    pub(super) fn restore(&mut self, enabled: bool) {
        self.enabled.get_or_insert(enabled);
    }

    fn toggle(&mut self) {
        self.enabled = Some(!self.enabled());
        self.attempted = None;
        self.pending = None;
        self.notice = None;
    }
}

impl MainView {
    pub(super) fn autoplay_control(&self, cx: &Context<Self>) -> Div {
        let enabled = self.autoplay.enabled();
        let controls = self.glass_controls(cx);
        let variant = ControlVariant::TextAction;
        let ink = controls.map_or_else(
            || {
                if enabled {
                    rgb(ACCENT).into()
                } else {
                    secondary()
                }
            },
            |material| {
                variant
                    .resolve(
                        material,
                        ControlState {
                            active: enabled,
                            ..ControlState::default()
                        },
                    )
                    .fg
            },
        );
        let toggle = button("queue-autoplay", "Автопродолжение", 609)
            .role(Role::Switch)
            .aria_toggled(if enabled {
                Toggled::True
            } else {
                Toggled::False
            })
            .justify_between()
            .gap(px(7.))
            .h(px(27.))
            .px(px(5.))
            .rounded(px(5.))
            .text_size(px(12.))
            .child("Автопродолжение")
            .child(
                div()
                    .text_color(ink)
                    .child(if enabled { "Вкл." } else { "Выкл." }),
            );
        let toggle = if let Some(controls) = controls {
            variant.paint(toggle, controls, ControlState::default())
        } else {
            toggle.hover(|style| style.bg(rgb(ELEVATED)))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .pb(px(5.))
            .child(toggle.on_activate(cx.listener(|view, _, _, cx| view.toggle_autoplay(cx))))
            .children(self.autoplay.notice.as_ref().map(|notice| {
                div()
                    .id("queue-autoplay-status")
                    .accessibility_id("queue-autoplay-status")
                    .role(Role::Status)
                    .aria_label(notice.clone())
                    .text_size(px(11.))
                    .text_color(secondary())
                    .child(notice.clone())
            }))
    }

    fn toggle_autoplay(&mut self, cx: &mut Context<Self>) {
        self.autoplay.toggle();
        self.persist_panel(cx);
        self.autoplay_player_changed(cx);
        cx.notify();
    }

    pub(super) fn autoplay_player_changed(&mut self, cx: &mut Context<Self>) {
        let player = self.player.read(cx);
        let queue = player.queue_replacement_revision();
        let point = player.snapshot().continuation.filter(|_| {
            self.autoplay.enabled()
                && !self.recommendation_reaction_pending()
                && !self.provider_station_running()
                && !self.recommendation_manages_queue(queue)
        });
        if self
            .autoplay
            .attempted
            .is_some_and(|attempted| attempted.queue != queue)
        {
            self.autoplay.attempted = None;
            self.autoplay.notice = None;
            self.autoplay.pending = None;
        }
        let Some(point) = point else {
            if self.autoplay.pending.take().is_some() {
                self.autoplay.attempted = None;
                self.autoplay.notice = None;
            }
            return;
        };
        if player.has_queued_next() {
            if self.autoplay.pending.take().is_some() {
                self.autoplay.notice = None;
                self.player.update(cx, |bar, cx| {
                    bar.command(cx, |playback| playback.continue_after(point, Vec::new()));
                });
            }
            return;
        }
        if self.autoplay.attempted == Some(point) {
            return;
        }
        let Some(entry) = player.current().cloned() else {
            return;
        };
        let (mode, access) = self.recommendation_options();
        let revision = player.snapshot().queue_revision;
        let queued = player.entries().cloned().collect::<Vec<_>>();
        self.autoplay.attempted = Some(point);
        self.autoplay.notice = Some("Ищем продолжение…".into());
        self.autoplay.pending = Some(run(
            cx,
            move |cancelled| {
                let store = Store::open_default().map_err(|error| error.to_string())?;
                let batch = radio::start(
                    &store,
                    &Seed::Track(Box::new(entry)),
                    mode,
                    access,
                    cancelled,
                )
                .map_err(|error| error.to_string())?;
                let entries = batch.entries(&queued);
                Ok::<_, String>((batch, entries))
            },
            move |view, result, cx| {
                view.autoplay.pending = None;
                let player = view.player.read(cx);
                if !view.autoplay.enabled()
                    || player.snapshot().continuation != Some(point)
                    || view.provider_station_running()
                    || view.recommendation_manages_queue(point.queue)
                {
                    view.autoplay.notice = None;
                    return;
                }
                let entries = match result {
                    Ok((batch, draft)) => {
                        let entries = if player.snapshot().queue_revision != revision {
                            batch.entries(&player.entries().cloned().collect::<Vec<_>>())
                        } else {
                            draft
                        };
                        view.autoplay.notice = if entries.is_empty() {
                            Some("Очередь закончилась. Рекомендаций пока нет.".into())
                        } else {
                            None
                        };
                        entries
                    }
                    Err(error) => {
                        eprintln!("Не удалось загрузить автопродолжение Mediadeka: {error}");
                        view.autoplay.notice =
                            Some("Не удалось загрузить продолжение очереди.".into());
                        Vec::new()
                    }
                };
                if !entries.is_empty() || player.has_queued_next() {
                    view.player.update(cx, |bar, cx| {
                        bar.command(cx, |playback| playback.continue_after(point, entries));
                    });
                }
            },
        ));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use gpui::TestAppContext;

    use super::*;
    use crate::appearance::Appearance;

    #[test]
    fn turning_autoplay_off_cancels_the_pending_worker_and_ignores_its_result() {
        let mut context = TestAppContext::single();
        let window = context.add_window(|window, cx| {
            crate::ui::theme::install(cx, false);
            MainView::new(window, cx, false, Some(true), false, Appearance::classic())
        });
        let cancelled = window
            .update(&mut context, |view, _, cx| {
                view.autoplay.toggle();
                let pending = run(
                    cx,
                    |_| Ok::<_, String>(()),
                    |_, _, _| {
                        panic!("a cancelled request must not update the view");
                    },
                );
                let cancelled = pending.cancellation();
                view.autoplay.pending = Some(pending);
                view.autoplay.notice = Some("Ищем продолжение…".into());
                view.autoplay.toggle();
                assert!(!view.autoplay.enabled());
                assert!(view.autoplay.pending.is_none());
                assert!(view.autoplay.notice.is_none());
                cancelled
            })
            .unwrap();
        context.run_until_parked();
        assert!(cancelled.load(Ordering::Relaxed));
    }
}
