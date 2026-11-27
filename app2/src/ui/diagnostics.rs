mod chart;
mod frames;
mod gpu_memory;
mod memory;

use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, Div, FontWeight, IntoElement, Role,
    TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, prelude::*, px, rgb,
    size,
};

use crate::{
    storage::Store,
    telemetry::{self, Received},
};

use super::{
    MainView,
    components::{artwork::artwork_disk_usage, heading},
    music::PrefetchReport,
    theme::{
        DIAGNOSTICS_API, DIAGNOSTICS_ARTWORK, DIAGNOSTICS_BACKGROUND, DIAGNOSTICS_CARD,
        DIAGNOSTICS_CARD_BORDER, DIAGNOSTICS_FRAME, DIAGNOSTICS_INTERVAL, FONT_FAMILY, primary,
        secondary,
    },
    workspace::ContentSample,
};
use chart::{Series, chart};
use frames::{FrameHistory, FrameSample};

const SAMPLE_PERIOD: Duration = Duration::from_millis(250);
const CACHE_SAMPLE_PERIOD: Duration = Duration::from_secs(15);
const HISTORY_POINTS: usize = 120;
const BYTES_PER_KIB: f64 = 1024.;

#[derive(Clone, Copy, Default)]
struct Sample {
    frame: FrameSample,
    api_kib_per_second: f64,
    artwork_kib_per_second: f64,
}

#[derive(Default)]
struct CacheResources {
    artwork_disk_bytes: Option<u64>,
    dedicated_gpu_bytes: Option<u64>,
}

impl CacheResources {
    fn sample() -> Self {
        Self {
            artwork_disk_bytes: Store::profile_directory()
                .ok()
                .and_then(|path| artwork_disk_usage(&path).ok()),
            dedicated_gpu_bytes: gpu_memory::dedicated_bytes(),
        }
    }
}

pub(super) struct Diagnostics {
    frames: FrameHistory,
    refresh_interval: Option<Duration>,
    samples: VecDeque<Sample>,
    received_at_open: Received,
    last_received: Received,
    last_sample_at: Instant,
    sound: Option<String>,
    startup_action: Option<Duration>,
    content_opening: Option<ContentSample>,
    prefetch: PrefetchReport,
    resources: CacheResources,
    memory: Option<memory::Memory>,
}

impl Diagnostics {
    pub(super) fn open(parent: &Window, cx: &mut App) -> gpui::Result<WindowHandle<Self>> {
        Self::open_window(parent, true, cx)
    }

    fn open_window(
        parent: &Window,
        visible: bool,
        cx: &mut App,
    ) -> gpui::Result<WindowHandle<Self>> {
        let source = parent.window_handle();
        let display = parent.display(cx);
        let display_id = display.as_ref().map(|display| display.id());
        let refresh_interval = parent.refresh_interval(cx);
        let mut bounds = Bounds::centered(display_id, size(px(770.), px(630.)), cx);
        if let Some(display) = display {
            let screen = display.bounds();
            let margin = px(28.);
            if screen.size.width >= bounds.size.width + margin * 2. {
                bounds.origin.x = screen.right() - bounds.size.width - margin;
            }
        }
        cx.open_window(
            WindowOptions {
                titlebar: Some(TitlebarOptions {
                    title: Some("Mediadeka — Диагностика".into()),
                    ..Default::default()
                }),
                display_id,
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(620.), px(460.))),
                show: visible,
                focus: visible,
                ..Default::default()
            },
            move |window, cx| {
                if !visible {
                    window.resize(bounds.size);
                }
                cx.new(|cx| Self::new(source, refresh_interval, window, cx))
            },
        )
    }

    fn new(
        source: AnyWindowHandle,
        refresh_interval: Option<Duration>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let panel = window.window_handle();
        cx.spawn(async move |view, cx| {
            let executor = cx.background_executor().clone();
            let mut next_cache_sample = Instant::now();
            loop {
                executor.timer(SAMPLE_PERIOD).await;
                let reading = source.update(cx, |view, window, cx| {
                    let (sound, startup_action, content_opening, prefetch) = view
                        .downcast::<MainView>()
                        .ok()
                        .map_or_else(Default::default, |view| {
                            let view = view.read(cx);
                            (
                                playback_report(view, cx),
                                view.startup_first_action(),
                                view.last_content_opening(),
                                view.prefetch_report(),
                            )
                        });
                    (
                        window.refresh_interval(cx),
                        sound,
                        startup_action,
                        content_opening,
                        prefetch,
                    )
                });
                let Ok((refresh, sound, startup_action, content_opening, prefetch)) = reading
                else {
                    let _ = panel.update(cx, |_, window, _| window.remove_window());
                    break;
                };
                let resources = if Instant::now() >= next_cache_sample {
                    next_cache_sample = Instant::now() + CACHE_SAMPLE_PERIOD;
                    Some(executor.spawn(async { CacheResources::sample() }).await)
                } else {
                    None
                };
                if view
                    .update(cx, |view, cx| {
                        view.sample(refresh);
                        view.sound = sound;
                        view.startup_action = startup_action;
                        view.content_opening = content_opening;
                        view.prefetch = prefetch;
                        view.memory = memory::sample();
                        if let Some(resources) = resources {
                            view.resources = resources;
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let received = telemetry::received();
        Self {
            frames: FrameHistory::new(source.window_id()),
            refresh_interval,
            samples: VecDeque::with_capacity(HISTORY_POINTS),
            received_at_open: received,
            last_received: received,
            last_sample_at: Instant::now(),
            sound: None,
            startup_action: None,
            content_opening: None,
            prefetch: PrefetchReport::default(),
            resources: CacheResources::default(),
            memory: None,
        }
    }

    fn sample(&mut self, refresh: Option<Duration>) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_sample_at).as_secs_f64();
        let received = telemetry::received();
        let rate = |total: u64, previous: u64| {
            total.saturating_sub(previous) as f64 / (BYTES_PER_KIB * elapsed)
        };
        self.refresh_interval = refresh;
        if self.samples.len() == HISTORY_POINTS {
            self.samples.pop_front();
        }
        self.samples.push_back(Sample {
            frame: self.frames.sample(refresh),
            api_kib_per_second: rate(received.api_bytes, self.last_received.api_bytes),
            artwork_kib_per_second: rate(received.artwork_bytes, self.last_received.artwork_bytes),
        });
        self.last_received = received;
        self.last_sample_at = now;
    }

    #[cfg(feature = "capture")]
    pub(super) fn activity(&self) -> (bool, bool) {
        (
            self.samples.iter().any(|sample| sample.frame.fps.is_some()),
            self.last_received.api_bytes > self.received_at_open.api_bytes
                || self.last_received.artwork_bytes > self.received_at_open.artwork_bytes,
        )
    }
}

#[cfg(feature = "capture")]
impl MainView {
    pub fn open_sound_diagnostics_for_capture(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Result<AnyWindowHandle> {
        Diagnostics::open_window(window, false, cx).map(Into::into)
    }
}

impl Render for Diagnostics {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let current = self.samples.back().copied().unwrap_or_default();
        let refresh_hz = self
            .refresh_interval
            .map(|interval| 1. / interval.as_secs_f64());
        let budget_ms = self
            .refresh_interval
            .map(|interval| interval.as_secs_f64() * 1000.);
        let display = refresh_hz.map_or_else(
            || "Основное окно".into(),
            |hz| format!("Основное окно · {hz:.0} Гц"),
        );
        let budget = budget_ms.map_or_else(
            || "Интервал монитора неизвестен".into(),
            |budget| format!("Бюджет монитора {budget:.2} мс"),
        );
        let incoming_api = self
            .last_received
            .api_bytes
            .saturating_sub(self.received_at_open.api_bytes);
        let incoming_artwork = self
            .last_received
            .artwork_bytes
            .saturating_sub(self.received_at_open.artwork_bytes);
        let network = format!(
            "С открытия получено {} · запросы HTTP {} · объединённые чтения {}",
            byte_count(incoming_api.saturating_add(incoming_artwork)),
            self.last_received
                .api_requests
                .saturating_sub(self.received_at_open.api_requests),
            self.last_received
                .joined_reads
                .saturating_sub(self.received_at_open.joined_reads),
        );
        let history_seconds = SAMPLE_PERIOD.as_millis() * HISTORY_POINTS as u128 / 1_000;

        div()
            .id("diagnostics-panel")
            .size_full()
            .overflow_y_scroll()
            .bg(rgb(DIAGNOSTICS_BACKGROUND))
            .text_color(primary())
            .font_family(FONT_FAMILY)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(14.))
                    .px(px(22.))
                    .py(px(18.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(heading("Диагностика", 24.))
                            .child(muted(display)),
                    )
                    .children(self.sound.as_ref().map(|report| {
                        card()
                            .id("diagnostics-sound")
                            .accessibility_id("diagnostics-sound")
                            .role(Role::Group)
                            .aria_label(format!("Аудио · {report}"))
                            .gap(px(4.))
                            .child(heading("Аудио", 15.))
                            .child(muted(report.clone()))
                    }))
                    .children(self.startup_action.map(|time| muted(format!(
                        "Запуск → первое действие в рабочем окне · {} мс", time.as_millis()
                    ))))
                    .children(self.content_opening.map(|opening| {
                        let label = format!(
                            "Открытие {} → готовый кадр · {:.1} мс",
                            opening.label(), opening.elapsed.as_secs_f64() * 1000.
                        );
                        muted(label.clone()).id("diagnostics-content-opening")
                            .accessibility_id("diagnostics-content-opening")
                            .role(Role::Label).aria_label(label)
                    }))
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .child(stat(
                                "diagnostics-fps", "Отрисовка", current.frame.fps, "FPS",
                                "Кадры GPUI за секунду",
                            ))
                            .child(stat(
                                "diagnostics-cpu", "CPU кадра · P95", current.frame.cpu_p95_ms, "мс",
                                "95% кадров уложились в это время",
                            ))
                            .child(stat(
                                "diagnostics-stability", "Дольше бюджета", current.frame.over_budget_percent, "%",
                                budget,
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(10.))
                            .child(chart(
                                "Частота отрисовки", "FPS", refresh_hz, HISTORY_POINTS,
                                [Series::new("Кадры GPUI", DIAGNOSTICS_FRAME, self.samples.iter().map(|s| s.frame.fps))],
                            ))
                            .child(chart(
                                "Стабильность кадров", "мс", budget_ms, HISTORY_POINTS,
                                [
                                    Series::new("Интервал P95", DIAGNOSTICS_INTERVAL, self.samples.iter().map(|s| s.frame.interval_p95_ms)),
                                    Series::new("CPU P95", DIAGNOSTICS_FRAME, self.samples.iter().map(|s| s.frame.cpu_p95_ms)),
                                ],
                            )),
                    )
                    .child(chart(
                        "Входящий поток", "КиБ/с", None, HISTORY_POINTS,
                        [
                            Series::new("API", DIAGNOSTICS_API, self.samples.iter().map(|s| Some(s.api_kib_per_second))),
                            Series::new("Обложки", DIAGNOSTICS_ARTWORK, self.samples.iter().map(|s| Some(s.artwork_kib_per_second))),
                        ],
                    ))
                    .child(
                        div()
                            .id("diagnostics-network")
                            .accessibility_id("diagnostics-network")
                            .role(Role::Label)
                            .aria_label(network.clone())
                            .child(muted(network)),
                    )
                    .child(catalog_cache(&self.prefetch, &self.resources, self.memory))
                    .child(muted(format!("История {history_seconds} с · Показанные монитором кадры измеряются отдельно"))),
            )
    }
}

fn catalog_cache(
    prefetch: &PrefetchReport,
    resources: &CacheResources,
    memory: Option<memory::Memory>,
) -> impl IntoElement {
    let catalog = telemetry::catalog();
    let hits = catalog.fresh + catalog.stale;
    let total = hits + catalog.miss;
    let hit_rate = if total == 0 {
        "—".into()
    } else {
        format!("{:.0}%", hits as f64 / total as f64 * 100.)
    };
    let counts = format!(
        "Открытия из кэша {hit_rate} · свежие {} · устаревшие {} · без снимка {}",
        catalog.fresh, catalog.stale, catalog.miss,
    );
    let resident = format!(
        "В памяти {} стр. · {} · активные загрузки {}",
        catalog.resident_pages,
        byte_count(catalog.resident_bytes as u64),
        catalog.active,
    );
    let outcome = format!(
        "Ответы каталога с ошибкой {} · отменённые загрузки {}",
        catalog.failures, catalog.cancelled,
    );
    let sources = catalog
        .source_reads
        .into_iter()
        .filter(|(_, reads)| *reads != 0)
        .map(|(provider, reads)| format!("{} {reads}", provider.display_name()))
        .collect::<Vec<_>>();
    card()
        .id("diagnostics-catalog-cache")
        .accessibility_id("diagnostics-catalog-cache")
        .role(Role::Group)
        .aria_label("Кэш каталога")
        .gap(px(4.))
        .child(heading("Кэш каталога", 15.))
        .child(muted(counts))
        .child(muted(resident))
        .child(muted(outcome))
        .when(!sources.is_empty(), |card| {
            card.child(muted(format!(
                "Загрузки каталога · {}",
                sources.join(" · ")
            )))
        })
        .children(memory.map(|memory| {
            muted(format!(
                "RAM процесса · {} · частная память {}",
                byte_count(memory.working_set),
                byte_count(memory.private)
            ))
        }))
        .children(
            resources
                .artwork_disk_bytes
                .map(|bytes| muted(format!("Обложки на диске · {}", byte_count(bytes)))),
        )
        .child({
            let dedicated = resources
                .dedicated_gpu_bytes
                .map_or_else(|| "—".into(), byte_count);
            let label = format!("Выделенная GPU-память процесса · {dedicated}");
            muted(label.clone())
                .id("diagnostics-gpu-memory")
                .accessibility_id("diagnostics-gpu-memory")
                .role(Role::Label)
                .aria_label(label)
        })
        .child(muted(format!(
            "Предзагрузка · в очереди {} · приоритет {} · готовы {} · отменены {}",
            prefetch.waiting,
            prefetch.active.unwrap_or("—"),
            prefetch.completed,
            prefetch.cancelled,
        )))
        .child({
            let efficiency = format!(
                "Предзагрузка из сети · использовано {} из {} · ответы без использованного снимка {}",
                prefetch.useful,
                prefetch.network_completed,
                byte_count(prefetch.unused_response_bytes),
            );
            muted(efficiency.clone())
                .id("diagnostics-prefetch-efficiency")
                .accessibility_id("diagnostics-prefetch-efficiency")
                .role(Role::Label)
                .aria_label(efficiency)
        })
        .child({
            use telemetry::CatalogInvalidation::{AccountChanged, CacheCleared, CredentialRevoked};
            let events = catalog.invalidations;
            let latest = match events.latest {
                Some(CacheCleared) => "очистка",
                Some(AccountChanged) => "изменение аккаунта",
                Some(CredentialRevoked) => "отзыв доступа",
                None => "—",
            };
            let label = format!(
                "Сбросы каталога · очистка {} · аккаунт {} · отзыв {} · последний {latest}",
                events.cleared, events.account_changed, events.credential_revoked,
            );
            muted(label.clone())
                .id("diagnostics-catalog-invalidations")
                .accessibility_id("diagnostics-catalog-invalidations")
                .role(Role::Label)
                .aria_label(label)
        })
        .children(catalog.source_latency.map(|latency| {
            muted(format!(
                "Ответ источника · в среднем {} мс · P95 {} мс",
                latency.mean.as_millis(),
                latency.p95.as_millis(),
            ))
        }))
}

fn playback_report(view: &MainView, cx: &App) -> Option<String> {
    let (source, details) = view.player.read(cx).sound_report();
    let fields = [
        source.map(|source| source.label().to_owned()),
        details.quality.as_ref().map(|quality| quality.label()),
        details
            .time_to_sound_ms
            .map(|ms| format!("До звука {ms} мс")),
        details.reason.clone(),
    ];
    let report = fields.into_iter().flatten().collect::<Vec<_>>();
    (!report.is_empty()).then(|| report.join(" · "))
}

fn stat(
    id: &'static str,
    title: &'static str,
    value: Option<f64>,
    unit: &'static str,
    note: impl Into<gpui::SharedString>,
) -> impl IntoElement {
    let value = value.map_or_else(
        || "—".into(),
        |value| {
            if unit == "мс" {
                format!("{value:.2}")
            } else {
                format!("{value:.0}")
            }
        },
    );
    card()
        .id(id)
        .accessibility_id(id)
        .role(Role::Group)
        .aria_label(format!("{title}: {value} {unit}"))
        .flex_1()
        .min_w_0()
        .gap(px(5.))
        .child(muted(title))
        .child(
            div()
                .flex()
                .items_baseline()
                .gap(px(6.))
                .child(
                    div()
                        .font_family("Consolas")
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(27.))
                        .child(value),
                )
                .child(muted(unit)),
        )
        .child(muted(note))
}

pub(super) fn card() -> Div {
    div()
        .flex()
        .flex_col()
        .px(px(13.))
        .py(px(10.))
        .rounded(px(10.))
        .bg(rgb(DIAGNOSTICS_CARD))
        .border_1()
        .border_color(rgb(DIAGNOSTICS_CARD_BORDER))
}

pub(super) fn muted(text: impl Into<gpui::SharedString>) -> Div {
    div()
        .text_color(secondary())
        .text_size(px(12.))
        .child(text.into())
}

fn byte_count(bytes: u64) -> String {
    let kib = bytes as f64 / BYTES_PER_KIB;
    if kib >= BYTES_PER_KIB {
        format!("{:.2} МиБ", kib / BYTES_PER_KIB)
    } else if kib >= 1. {
        format!("{kib:.1} КиБ")
    } else {
        format!("{bytes} Б")
    }
}
