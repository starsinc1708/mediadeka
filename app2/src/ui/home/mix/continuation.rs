use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::Context;

use crate::{music::recommendations::wave, storage::Store, ui::MainView};

use super::{Job, Start, unavailable_notice};

#[derive(Default)]
pub(super) struct WaveUi {
    active: Option<Active>,
    managed_queue: Option<u64>,
    refill: Option<Job>,
}

struct Active {
    feed: wave::Feed,
    kind: Start,
    queue: u64,
}

impl WaveUi {
    pub(super) fn begin(&mut self, feed: Option<(Start, wave::Feed)>, queue: u64) {
        self.cancel_refill();
        if feed.is_some() {
            self.mark_queue(queue);
        }
        self.active = feed.map(|(kind, feed)| Active { feed, kind, queue });
    }

    pub(super) fn mark_queue(&mut self, queue: u64) {
        self.managed_queue = Some(queue);
    }

    pub(super) fn cancel_refill(&mut self) {
        self.refill = None;
    }
}

impl MainView {
    pub(in crate::ui) fn recommendation_manages_queue(&self, queue: u64) -> bool {
        self.home.mix.wave.managed_queue == Some(queue)
            || self.contextual_radio_running()
            || self.recommendation_wave_running()
    }

    pub(in crate::ui) fn recommendation_wave_running(&self) -> bool {
        self.recommendation_running(Start::Wave)
    }

    pub(in crate::ui) fn contextual_radio_running(&self) -> bool {
        self.recommendation_running(Start::Radio)
    }

    fn recommendation_running(&self, expected: Start) -> bool {
        let mix = &self.home.mix;
        mix.wave
            .active
            .as_ref()
            .is_some_and(|active| active.kind == expected)
            || mix
                .starting
                .as_ref()
                .is_some_and(|(kind, _)| *kind == expected)
    }

    pub(super) fn stop_recommendation_wave(&mut self, cx: &mut Context<Self>) {
        self.stop_recommendation_feed(Start::Wave);
        self.home.mix.notice = Some("Общая волна остановлена.".into());
        cx.notify();
    }

    pub(super) fn stop_contextual_radio(&mut self, cx: &mut Context<Self>) {
        self.stop_recommendation_feed(Start::Radio);
        self.home.mix.radio_notice = Some("Радио остановлено.".into());
        cx.notify();
    }

    fn stop_recommendation_feed(&mut self, expected: Start) {
        let mix = &mut self.home.mix;
        if mix
            .starting
            .as_ref()
            .is_some_and(|(kind, _)| *kind == expected)
        {
            mix.starting = None;
            if expected == Start::Radio {
                mix.pending_radio_queue = None;
            }
        }
        if mix
            .wave
            .active
            .as_ref()
            .is_some_and(|active| active.kind == expected)
        {
            mix.wave.begin(None, 0);
        }
    }

    pub(in crate::ui) fn recommendation_player_changed(&mut self, cx: &mut Context<Self>) {
        let mix = &mut self.home.mix;
        let player = self.player.read(cx);
        let queue = player.queue_replacement_revision();
        if mix
            .wave
            .managed_queue
            .is_some_and(|managed| managed != queue)
        {
            mix.wave.managed_queue = None;
            if mix.pending_radio_queue != Some(queue) {
                mix.radio_notice = None;
            }
        }
        if mix
            .pending_radio_queue
            .is_some_and(|pending| pending != queue)
        {
            mix.pending_radio_queue = None;
            mix.starting = None;
            mix.radio_notice = None;
        }
        let Some(active) = &mix.wave.active else {
            return;
        };
        if queue != active.queue {
            mix.radio_notice = None;
            mix.wave.begin(None, 0);
            return;
        }
        if mix.wave.refill.is_some()
            || mix.starting.is_some()
            || mix.feedback.is_saving()
            || !wave::should_refill(player.current_index(), player.queue_len())
        {
            return;
        }
        let feed = active.feed.clone();
        let kind = active.kind;
        let queue = active.queue;
        let previous = player.snapshot().queue_revision;
        let queued = player.entries().cloned().collect::<Vec<_>>();
        let cancelled = Arc::new(AtomicBool::new(false));
        let work = cancelled.clone();
        let applying = cancelled.clone();
        let task = cx.spawn(async move |view, cx| {
            let batch = cx
                .background_executor()
                .spawn(async move {
                    let batch = feed.refill(&Store::open_default()?, &work)?;
                    let entries = batch.entries(&queued);
                    Ok::<_, crate::storage::StoreError>((batch, entries))
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                if applying.load(Ordering::Relaxed) {
                    return;
                }
                let player = view.player.read(cx);
                if player.queue_replacement_revision() != queue {
                    view.home.mix.wave.begin(None, 0);
                    cx.notify();
                    return;
                }
                match batch {
                    Ok((batch, draft)) => {
                        let entries = if player.snapshot().queue_revision == previous {
                            draft
                        } else {
                            let Some(entries) = batch.continuation(
                                queue,
                                player.queue_replacement_revision(),
                                player.entries().cloned(),
                                &applying,
                            ) else {
                                return;
                            };
                            entries
                        };
                        let notice = if kind == Start::Radio {
                            &mut view.home.mix.radio_notice
                        } else {
                            &mut view.home.mix.notice
                        };
                        *notice = unavailable_notice(&batch.unavailable);
                        if entries.is_empty() {
                            view.home.mix.wave.begin(None, 0);
                            *notice = Some(
                                if kind == Start::Radio {
                                    "Для радио пока нет новых рекомендаций."
                                } else {
                                    "Для общей волны пока нет новых рекомендаций."
                                }
                                .into(),
                            );
                        } else {
                            view.home.mix.wave.begin(Some((kind, batch.feed)), queue);
                            view.player.update(cx, |bar, cx| {
                                bar.command(cx, |playback| playback.append_continuation(entries));
                            });
                        }
                    }
                    Err(error) => {
                        eprintln!("Не удалось пополнить рекомендации Mediadeka: {error}");
                        view.home.mix.wave.begin(None, 0);
                        if kind == Start::Radio {
                            view.home.mix.radio_notice = Some("Не удалось пополнить радио.".into());
                        } else {
                            view.home.mix.notice = Some("Не удалось пополнить общую волну.".into());
                        }
                    }
                }
                cx.notify();
            });
        });
        mix.wave.refill = Some(Job {
            cancelled,
            _task: task,
        });
    }
}
