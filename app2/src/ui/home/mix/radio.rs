use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::{Context, Window};

use crate::{
    music::{
        ObjectKind, ScopedPage,
        recommendations::radio::{self, Seed},
    },
    playback::QueueEntry,
    storage::{PanelTab, Store},
    ui::MainView,
};

use super::{Job, Start, unavailable_notice};

impl MainView {
    pub(in crate::ui) fn recommendation_options(
        &self,
    ) -> (
        crate::music::recommendations::Mode,
        crate::music::recommendations::wave::Access,
    ) {
        (self.home.mix.mode, self.home.mix.access())
    }

    pub(in crate::ui) fn can_start_track_radio(&self, entry: &QueueEntry) -> bool {
        entry.is_playable() && self.has_radio_connections()
    }

    pub(in crate::ui) fn can_start_artist_radio(&self, data: &ScopedPage) -> bool {
        data.page.reference.kind == ObjectKind::Artist && self.has_radio_connections()
    }

    fn has_radio_connections(&self) -> bool {
        self.state.home_view().is_some_and(|(home, _)| {
            home.accounts
                .iter()
                .any(|account| radio::has_track_radio(account.provider))
        })
    }

    pub(in crate::ui) fn start_track_radio(
        &mut self,
        entry: QueueEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.can_start_track_radio(&entry) {
            self.start_contextual_radio(Seed::Track(Box::new(entry)), window, cx);
        }
    }

    pub(in crate::ui) fn start_artist_radio(
        &mut self,
        data: ScopedPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.can_start_artist_radio(&data) {
            self.start_contextual_radio(Seed::Artist(data), window, cx);
        }
    }

    fn start_contextual_radio(&mut self, seed: Seed, window: &mut Window, cx: &mut Context<Self>) {
        self.home.mix.starting = None;
        self.home.mix.wave.cancel_refill();
        if let Seed::Track(entry) = &seed {
            self.ensure_playlist_favorites(&entry.playlist, cx);
            self.player.update(cx, |bar, cx| {
                bar.command(cx, |playback| playback.play(vec![(**entry).clone()], 0));
            });
            self.home.mix.wave.begin(None, 0);
            self.home
                .mix
                .wave
                .mark_queue(self.player.read(cx).queue_replacement_revision());
        }
        self.open_panel(PanelTab::Queue, false, window, cx);
        let player = self.player.read(cx);
        let queue = player.queue_replacement_revision();
        let revision = player.snapshot().queue_revision;
        let queued = player.entries().cloned().collect::<Vec<_>>();
        let track = matches!(seed, Seed::Track(_));
        let access = self.home.mix.access();
        let mode = self.home.mix.mode;
        let cancelled = Arc::new(AtomicBool::new(false));
        let work = cancelled.clone();
        let applying = cancelled.clone();
        let task = cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let batch = radio::start(&Store::open_default()?, &seed, mode, access, &work)?;
                    let entries = batch.entries(if track { &queued } else { &[] });
                    Ok::<_, crate::storage::StoreError>((batch, entries))
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                if applying.load(Ordering::Relaxed) {
                    return;
                }
                view.home.mix.starting = None;
                view.home.mix.pending_radio_queue = None;
                let player = view.player.read(cx);
                if player.queue_replacement_revision() != queue
                    || (!track && player.snapshot().queue_revision != revision)
                {
                    view.home.mix.radio_notice = None;
                } else {
                    match result {
                        Ok((batch, draft)) => {
                            let entries = if track && player.snapshot().queue_revision != revision {
                                batch.entries(&player.entries().cloned().collect::<Vec<_>>())
                            } else {
                                draft
                            };
                            if entries.is_empty() {
                                view.home.mix.radio_notice =
                                    Some("Для радио пока нет рекомендаций.".into());
                            } else {
                                view.home.mix.radio_notice = unavailable_notice(&batch.unavailable);
                                view.player.update(cx, |bar, cx| {
                                    bar.command(cx, |playback| {
                                        if track {
                                            playback.append_continuation(entries);
                                        } else {
                                            playback.play(entries, 0);
                                        }
                                    });
                                });
                                let queue = view.player.read(cx).queue_replacement_revision();
                                view.home
                                    .mix
                                    .wave
                                    .begin(Some((Start::Radio, batch.feed)), queue);
                            }
                        }
                        Err(error) => {
                            eprintln!("Не удалось открыть радио Mediadeka: {error}");
                            view.home.mix.radio_notice = Some("Не удалось загрузить радио.".into());
                        }
                    }
                }
                view.recommendation_player_changed(cx);
                view.autoplay_player_changed(cx);
                cx.notify();
            });
        });
        self.home.mix.radio_notice = Some("Загружаем радио…".into());
        self.home.mix.pending_radio_queue = Some(queue);
        self.home.mix.starting = Some((
            Start::Radio,
            Job {
                cancelled,
                _task: task,
            },
        ));
        cx.notify();
    }
}
