use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::{Context, Div, IntoElement, Role, Stateful, Task, Window, div, prelude::*, px};
use guise::{Button, Chip, Size, Variant};

use crate::{
    music::{
        recommendations::{self, Mode, PLAYLISTS_PER_PROVIDER, Ranking, Selection, wave},
        station::recommendations::{self as stations, StationSource},
    },
    playback::QueueEntry,
    providers::{Home, ProviderId},
    state::HomeView,
    storage::{PanelTab, Store, StoreError},
    ui::{
        components::{Activate, section_tab},
        design_system::controls::ControlVariant,
        theme::secondary,
    },
};

use super::{FeedSource, MainView};

mod continuation;
mod feedback;
mod radio;

const MIX_EMPTY: &str = "Доступных треков для микса нет.";

#[derive(Default)]
pub(super) struct MixUi {
    mode: Mode,
    starting: Option<(Start, Job)>,
    notice: Option<String>,
    radio_notice: Option<String>,
    pending_radio_queue: Option<u64>,
    wave: continuation::WaveUi,
    feedback: feedback::FeedbackUi,
    #[cfg(feature = "capture")]
    pub(super) saved_only: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Start {
    Mix,
    Wave,
    Radio,
}

struct Started {
    entries: Vec<QueueEntry>,
    unavailable: Vec<ProviderId>,
    wave: Option<wave::Feed>,
}

struct Job {
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl MixUi {
    pub(super) fn clear(&mut self) {
        if self.pending_radio_queue.take().is_some() {
            self.radio_notice = None;
        }
        self.starting = None;
        self.notice = None;
    }

    fn access(&self) -> wave::Access {
        #[cfg(feature = "capture")]
        if self.saved_only {
            return wave::Access::Saved;
        }
        wave::Access::Live
    }

    pub(super) fn status(&self, fluid: bool, cx: &Context<MainView>) -> Option<Stateful<Div>> {
        let message = self.notice.as_ref()?;
        if fluid && message == MIX_EMPTY {
            return Some(crate::ui::components::content_state::empty_state(
                "home-mix-status",
                MIX_EMPTY,
                None,
                guise::IconName::Music2,
                cx,
            ));
        }
        Some(
            div()
                .id("home-mix-status")
                .accessibility_id("home-mix-status")
                .role(Role::Status)
                .aria_label(message.clone())
                .text_size(px(12.))
                .text_color(secondary())
                .child(message.clone()),
        )
    }
}

impl MainView {
    fn home_recommendation_feed<'a>(
        &'a self,
        source: &FeedSource,
        active: &FeedSource,
        home: &'a HomeView,
        refreshing: bool,
    ) -> Option<&'a Home> {
        self.home
            .feeds
            .get(source, active, home, refreshing)
            .snapshot()
            .map(|snapshot| &snapshot.home)
    }

    fn home_mix_selections<'a>(
        &'a self,
        source: &'a FeedSource,
        active: &FeedSource,
        home: &'a HomeView,
        refreshing: bool,
    ) -> impl Iterator<Item = Selection> + 'a {
        self.home_recommendation_feed(source, active, home, refreshing)
            .zip(source.profile.as_deref())
            .into_iter()
            .flat_map(move |(feed, profile)| {
                recommendations::personal_playlists(feed, source.provider, profile)
                    .take(PLAYLISTS_PER_PROVIDER)
            })
    }

    fn home_station_selections(
        &self,
        source: &FeedSource,
        active: &FeedSource,
        home: &HomeView,
        refreshing: bool,
    ) -> Vec<StationSource> {
        let Some(profile) = source.profile.as_deref() else {
            return Vec::new();
        };
        stations::selections(
            self.home_recommendation_feed(source, active, home, refreshing),
            source.provider,
            profile,
        )
    }

    pub(super) fn home_recommendation_controls(
        &self,
        visible: &[&FeedSource],
        active: &FeedSource,
        home: &HomeView,
        refreshing: bool,
        cx: &Context<Self>,
    ) -> Option<Div> {
        if self.home.filter.is_some() {
            return None;
        }
        let starting_mix = self
            .home
            .mix
            .starting
            .as_ref()
            .is_some_and(|(start, _)| *start == Start::Mix);
        let busy = self.home.mix.starting.is_some() || self.home.mix.feedback.is_saving();
        let personal = visible.iter().any(|source| {
            self.home_mix_selections(source, active, home, refreshing)
                .next()
                .is_some()
        });
        let wave = personal
            || visible.iter().any(|source| {
                !self
                    .home_station_selections(source, active, home, refreshing)
                    .is_empty()
            });
        let running = self.recommendation_wave_running();
        if !wave && !running && !busy {
            return None;
        }
        let mode = self.home.mix.mode;
        let modes = [
            ("home-mix-familiar", "Знакомое", Mode::Familiar),
            ("home-mix-discoveries", "Открытия", Mode::Discoveries),
        ];
        let glass = self.glass_controls(cx);
        let mix_label = if starting_mix {
            "Собираем микс…"
        } else {
            "Собрать микс"
        };
        let wave_label = if running {
            "Остановить волну"
        } else {
            "Общая волна"
        };
        Some(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .id("home-mix-modes")
                        .accessibility_id("home-mix-modes")
                        .role(Role::TabList)
                        .aria_label("Режим рекомендаций")
                        .flex()
                        .gap(px(4.))
                        .children(modes.into_iter().enumerate().map(
                            |(index, (id, label, option))| {
                                let order = 34 + index as isize;
                                let activate = move |view: &mut Self, cx: &mut Context<Self>| {
                                    view.home.mix.mode = option;
                                    cx.notify();
                                };
                                if glass.is_some() {
                                    section_tab(id, label, mode == option, order, glass)
                                        .on_activate(
                                            cx.listener(move |view, _, _, cx| activate(view, cx)),
                                        )
                                        .into_any_element()
                                } else {
                                    Chip::new(id, label)
                                        .as_tab()
                                        .size(Size::Sm)
                                        .checked(mode == option)
                                        .tab_order(order)
                                        .on_change(
                                            cx.listener(move |view, _, _, cx| activate(view, cx)),
                                        )
                                        .into_any_element()
                                }
                            },
                        )),
                )
                .child(
                    Button::new("home-build-mix", mix_label)
                        .variant(Variant::Default)
                        .size(Size::Xs)
                        .tab_order(36)
                        .disabled(busy || !personal)
                        .when_some(glass, |button, material| {
                            ControlVariant::TextAction.text_button(button, material)
                        })
                        .on_click(cx.listener(|view, _, window, cx| {
                            view.start_home_mix(Start::Mix, window, cx)
                        })),
                )
                .child(
                    Button::new("home-general-wave", wave_label)
                        .size(Size::Xs)
                        .variant(if running {
                            Variant::Default
                        } else {
                            Variant::Light
                        })
                        .tab_order(37)
                        .disabled(!running && (busy || !wave))
                        .when_some(glass, |button, material| {
                            ControlVariant::SecondaryButton.text_button(button, material)
                        })
                        .on_click(cx.listener(|view, _, window, cx| {
                            if view.recommendation_wave_running() {
                                view.stop_recommendation_wave(cx);
                            } else {
                                view.start_home_mix(Start::Wave, window, cx);
                            }
                        })),
                ),
        )
    }

    fn start_home_mix(&mut self, kind: Start, window: &Window, cx: &mut Context<Self>) {
        if self.home.filter.is_some()
            || self.home.mix.starting.is_some()
            || self.home.mix.feedback.is_saving()
        {
            return;
        }
        let Some((home, refreshing)) = self.state.home_view() else {
            return;
        };
        let active = FeedSource::active(home);
        let sources = FeedSource::available(home);
        let selections = sources
            .iter()
            .flat_map(|source| self.home_mix_selections(source, &active, home, refreshing))
            .collect::<Vec<_>>();
        let stations = match kind {
            Start::Mix | Start::Radio => Vec::new(),
            Start::Wave => sources
                .iter()
                .flat_map(|source| self.home_station_selections(source, &active, home, refreshing))
                .collect(),
        };
        if selections.is_empty() && stations.is_empty() {
            return;
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let work = cancelled.clone();
        let applying = cancelled.clone();
        let window = window.window_handle();
        let before = self.player.read(cx).snapshot().queue_revision;
        let access = self.home.mix.access();
        let mode = self.home.mix.mode;
        self.home.mix.pending_radio_queue = None;
        self.home.mix.wave.cancel_refill();
        self.home.mix.notice = None;
        let task = cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    load_first(
                        &Store::open_default()?,
                        kind,
                        selections,
                        stations,
                        mode,
                        access,
                        &work,
                    )
                })
                .await;
            let _ = window.update(cx, |_, window, cx| {
                let _ = view.update(cx, |view, cx| {
                    if applying.load(Ordering::Relaxed) {
                        return;
                    }
                    view.home.mix.starting = None;
                    let same_sources = view
                        .state
                        .home_view()
                        .is_some_and(|(home, _)| FeedSource::available(home) == sources);
                    if !same_sources || view.player.read(cx).snapshot().queue_revision != before {
                        view.home.mix.notice =
                            Some("Подключения или очередь изменились. Соберите микс снова.".into());
                    } else {
                        match result {
                            Ok(mix) => view.play_home_mix(mix, window, cx),
                            Err(error) => {
                                eprintln!("Не удалось собрать микс Mediadeka: {error}");
                                view.home.mix.notice = Some("Не удалось собрать микс.".into());
                            }
                        }
                    }
                    view.recommendation_player_changed(cx);
                    cx.notify();
                });
            });
        });
        self.home.mix.starting = Some((
            kind,
            Job {
                cancelled,
                _task: task,
            },
        ));
        cx.notify();
    }

    fn play_home_mix(&mut self, mix: Started, window: &mut Window, cx: &mut Context<Self>) {
        if mix.entries.is_empty() {
            self.home.mix.notice = Some(MIX_EMPTY.into());
            return;
        }
        self.home.mix.notice = unavailable_notice(&mix.unavailable);
        self.ensure_playlist_favorites(&mix.entries[0].playlist, cx);
        self.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| playback.play(mix.entries, 0));
        });
        let revision = self.player.read(cx).queue_replacement_revision();
        self.home.mix.radio_notice = None;
        self.home
            .mix
            .wave
            .begin(mix.wave.map(|feed| (Start::Wave, feed)), revision);
        self.open_panel(PanelTab::Queue, false, window, cx);
    }
}

fn load_first(
    store: &Store,
    kind: Start,
    selections: Vec<Selection>,
    stations: Vec<StationSource>,
    mode: Mode,
    access: wave::Access,
    cancelled: &AtomicBool,
) -> Result<Started, StoreError> {
    if kind == Start::Wave {
        let batch = wave::start(store, selections, stations, mode, access, cancelled)?;
        return Ok(Started {
            entries: batch.entries(&[]),
            unavailable: batch.unavailable,
            wave: Some(batch.feed),
        });
    }
    let mix = match access {
        wave::Access::Live => recommendations::assemble_candidates(store, selections, cancelled),
        #[cfg(any(test, feature = "capture"))]
        wave::Access::Saved => {
            recommendations::assemble_cached_candidates(store, selections, cancelled)
        }
    }?;
    Ok(Started {
        entries: Ranking::load(store, mode)?.entries(&mix.entries, &[]),
        unavailable: mix.unavailable,
        wave: None,
    })
}

fn unavailable_notice(unavailable: &[ProviderId]) -> Option<String> {
    if unavailable.is_empty() {
        return None;
    }
    let providers = unavailable
        .iter()
        .map(|provider| provider.display_name())
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("Часть рекомендаций недоступна · {providers}"))
}
