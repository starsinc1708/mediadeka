mod appearance;
mod assets;
mod autoplay;
mod components;
mod connections;
mod design_system;
mod diagnostics;
mod effects;
mod fluid_glass;
mod history;
mod home;
mod labels;
mod likes;
mod listening;
mod login;
mod lyrics;
mod music;
mod native;
mod panel;
mod player;
mod playlist;
mod settings;
mod soloist;
mod station;
mod theme;
mod welcome;
mod welcome_motion;
mod workspace;

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext, Application, Bounds, Context, Div, FocusHandle, IntoElement,
    KeyBinding, Role, ScrollHandle, SharedString, Stateful, Subscription, TextRenderingMode,
    TitlebarOptions, Window, WindowBounds, WindowControlArea, WindowHandle, WindowOptions, actions,
    div, point, prelude::*, px, rgb, size, svg,
};
use guise::{Icon, IconName};

use crate::{
    appearance::{Appearance, Shell, TrackColumnVisibility, TrackDensity},
    integrations::Integrations,
    state::{Intent, Screen, State},
};
use assets::provider_index;
use components::{Activate, button};
use diagnostics::Diagnostics;
use effects::Motion;
use history::HistoryUi;
use home::HomeUi;
use listening::{Listening, ListeningMode};
use login::Connection;
use lyrics::{FullscreenRequested, LyricsView};
use music::MusicUi;
use native::Command;
use panel::RightPanel;
use player::{
    EntityRequested, FavoriteRequested, ListeningChanged, ListeningRequested, LyricsRequested,
    PlayerBar, QueueRequested, Snapshot, TrackMenuRequested, TrackRequested,
};
use playlist::PlaylistUi;
use theme::{FONT_FAMILY, primary};
use welcome_motion::{ConnectionMotion, Flight};
use workspace::Workspace;

#[cfg(all(test, windows))]
pub(crate) use autoplay::acceptance::check_automatic_queue;
#[cfg(feature = "capture")]
pub use playlist::acceptance::{seed_playlist_capture, seed_playlist_source_capture};

const TITLE: &str = "Mediadeka";
const INITIAL_SIZE: (f32, f32) = (1100., 700.);
const MIN_SIZE: (f32, f32) = (800., 400.);
const TITLEBAR_HEIGHT: f32 = 41.;
/// How long the boot splash stays on screen before the shell takes over.
/// Long enough for the splash frame to be presented, short enough to be
/// invisible on a warm start.
const BOOT_SPLASH: u64 = 120;
const CHROME_BUTTON_WIDTH: f32 = 48.;
const CHROME_END_PADDING: f32 = 5.;

actions!(
    mediadeka,
    [
        Tab,
        ShiftTab,
        Escape,
        FocusGlobalSearch,
        FocusPageSearch,
        ToggleListening,
        MovePinUp,
        MovePinDown
    ]
);

/// Key context of a pinned sidebar item, so Alt+↑ and Alt+↓ act only there.
pub(in crate::ui) const PIN_KEY_CONTEXT: &str = "PinnedItem";

#[cfg(feature = "capture")]
pub struct CaptureLayout {
    pub width: f32,
    pub height: f32,
    pub reduced_motion: bool,
}

pub fn application() -> Application {
    gpui_platform::application().with_assets(assets::Assets)
}

pub fn open_main_window(cx: &mut App) -> gpui::Result<WindowHandle<MainView>> {
    open_window(cx, INITIAL_SIZE, true, None)
}

#[cfg(feature = "capture")]
pub fn open_capture_window(
    cx: &mut App,
    layout: CaptureLayout,
) -> gpui::Result<WindowHandle<MainView>> {
    open_window(
        cx,
        (layout.width, layout.height),
        false,
        Some(layout.reduced_motion),
    )
}

fn open_window(
    cx: &mut App,
    (width, height): (f32, f32),
    visible: bool,
    fixed_motion: Option<bool>,
) -> gpui::Result<WindowHandle<MainView>> {
    let opened = Instant::now();
    let requested_size = size(px(width), px(height));
    let bounds = Bounds::centered(None, requested_size, cx);
    cx.set_text_rendering_mode(TextRenderingMode::Grayscale);
    cx.set_reduce_motion(fixed_motion.unwrap_or_else(native::reduced_motion));
    cx.bind_keys([
        KeyBinding::new("tab", Tab, None),
        KeyBinding::new("shift-tab", ShiftTab, None),
        KeyBinding::new("escape", Escape, None),
        KeyBinding::new("ctrl-k", FocusGlobalSearch, None),
        KeyBinding::new("ctrl-f", FocusPageSearch, None),
        KeyBinding::new("ctrl-l", ToggleListening, None),
        KeyBinding::new("alt-up", MovePinUp, Some(PIN_KEY_CONTEXT)),
        KeyBinding::new("alt-down", MovePinDown, Some(PIN_KEY_CONTEXT)),
    ]);

    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some(TITLE.into()),
            appears_transparent: true,
            ..Default::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(MIN_SIZE.0), px(MIN_SIZE.1))),
        show: visible,
        focus: visible,
        ..Default::default()
    };
    let appearance = Appearance::startup();
    let fluid = appearance.shell == Shell::FluidGlass;
    theme::install(cx, fluid);
    appearance::install(cx, appearance);
    design_system::icons::install(cx);
    cx.open_window(options, move |window, cx| {
        if !visible {
            // Allocate a real D3D11 target for off-screen capture.
            window.resize(requested_size);
        }
        let view = cx.new(|cx| MainView::new(window, cx, visible, fixed_motion, fluid, appearance));
        if visible {
            view.update(cx, |view, cx| view.start_desktop(opened, window, cx));
        }
        view
    })
}

pub struct MainView {
    state: State,
    workspace: Workspace,
    fluid_backdrop: Option<gpui::Entity<fluid_glass::Backdrop>>,
    home: HomeUi,
    playlist: PlaylistUi,
    likes: likes::LikeUi,
    music: MusicUi,
    station: station::StationUi,
    autoplay: autoplay::AutoplayUi,
    history: HistoryUi,
    panel: RightPanel,
    lyrics: gpui::Entity<LyricsView>,
    listening: Listening,
    pins: workspace::pins::Pins,
    player: gpui::Entity<PlayerBar>,
    player_snapshot: Snapshot,
    _player_subscription: Subscription,
    _favorite_subscription: Subscription,
    _queue_subscription: Subscription,
    _track_subscription: Subscription,
    _lyrics_subscription: Subscription,
    _fullscreen_lyrics_subscription: Subscription,
    _listening_view_subscription: Subscription,
    _entity_subscription: Subscription,
    _track_menu_subscription: Subscription,
    _sound_subscription: Subscription,
    _listening_subscription: Subscription,
    _integrations_subscription: Subscription,
    _sound_cache_subscription: Subscription,
    _collection_progress_subscription: Subscription,
    _queue_resume_subscription: Subscription,
    _station_end_subscription: Subscription,
    soloist: soloist::SoloistUi,
    login: Option<Connection>,
    connection_worker: Option<connections::ConnectionWorker>,
    focus: FocusHandle,
    connection_focus: FocusHandle,
    provider_focus: [FocusHandle; crate::providers::ProviderId::ALL.len()],
    connection_motion: ConnectionMotion,
    connection_focus_pending: bool,
    settings_focus_pending: Option<crate::providers::ProviderId>,
    appearance: Appearance,
    track_density: TrackDensity,
    track_column_visibility: TrackColumnVisibility,
    appearance_error: Option<String>,
    appearance_notice: Option<String>,
    gallery_open: bool,
    appearance_quality_select: components::overlay::Select,
    #[cfg(feature = "capture")]
    settings_diagnostics: bool,
    #[cfg(feature = "capture")]
    shader_debug: settings::ShaderDebug,
    #[cfg(feature = "capture")]
    icon_gallery: [bool; design_system::icons::Glyph::ALL.len()],
    #[cfg(feature = "capture")]
    gallery_inputs: components::search_input::GalleryInputs,
    #[cfg(feature = "capture")]
    editorial_menu_samples: settings::EditorialMenuSamples,
    #[cfg(feature = "capture")]
    editorial_gallery_tab: usize,
    sound_download_settings: settings::SoundDownloadSettings,
    genius_settings: settings::GeniusSettings,
    integrations: gpui::Entity<Integrations>,
    integration_controls: settings::IntegrationControls,
    settings_sections: [FocusHandle; 4],
    scroll: ScrollHandle,
    motion: Motion,
    hovered: Option<usize>,
    started: Instant,
    /// The first painted frame is the splash; see [`MainView::splash`].
    boot_splash: bool,
    reduced_motion: bool,
    capture_time: Option<Duration>,
    diagnostics: Option<WindowHandle<Diagnostics>>,
    _visibility_subscription: Option<Subscription>,
}

impl MainView {
    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        visible: bool,
        fixed_motion: Option<bool>,
        fluid: bool,
        appearance: Appearance,
    ) -> Self {
        let reduced_motion = fixed_motion.unwrap_or_else(native::reduced_motion);
        let player = PlayerBar::new(reduced_motion, fluid, cx);
        let fluid_backdrop =
            fluid.then(|| cx.new(|cx| fluid_glass::Backdrop::new(&player, window, cx)));
        let integrations = cx.new(|cx| Integrations::new(visible, cx));
        let integrations_subscription = cx.observe(&integrations, |view, _, cx| {
            if view.music_page() == workspace::Page::Settings {
                cx.notify();
            }
        });
        let lyrics = LyricsView::new(player.clone(), fluid, cx);
        let favorite_subscription = cx.subscribe_in(
            &player,
            window,
            |view, _, event: &FavoriteRequested, window, cx| {
                view.like_gesture(event.subject.clone(), event.gesture, window, cx);
            },
        );
        let queue_subscription = cx.subscribe_in(
            &player,
            window,
            |view, _, _: &QueueRequested, window, cx| {
                view.toggle_queue_panel(window, cx);
            },
        );
        let lyrics_subscription = cx.subscribe_in(
            &player,
            window,
            |view, _, _: &LyricsRequested, window, cx| {
                view.toggle_lyrics_panel(window, cx);
            },
        );
        let track_subscription = cx.subscribe_in(
            &player,
            window,
            |view, _, _: &TrackRequested, window, cx| {
                view.toggle_playing_track_panel(window, cx);
            },
        );
        let fullscreen_lyrics_subscription = cx.subscribe_in(
            &lyrics,
            window,
            |view, _, _: &FullscreenRequested, window, cx| {
                let mode = (view.listening.mode() != Some(ListeningMode::Lyrics))
                    .then_some(ListeningMode::Lyrics);
                view.set_listening_mode(mode, window, cx);
            },
        );
        let listening_view_subscription = cx.subscribe_in(
            &player,
            window,
            |view, _, _: &ListeningRequested, window, cx| {
                view.toggle_listening(window, cx);
            },
        );
        let entity_subscription = cx.subscribe_in(
            &player,
            window,
            |view, _, event: &EntityRequested, window, cx| {
                view.follow_playlist_link(event.0.clone(), window, cx);
            },
        );
        let track_menu_subscription = cx.subscribe_in(
            &player,
            window,
            |view, _, event: &TrackMenuRequested, window, cx| {
                let bounds = components::overlay::TriggerBounds::default();
                bounds.set(event.0);
                view.show_playlist_menu(playlist::popup::MenuKind::NowPlaying, bounds, window, cx);
            },
        );
        let sound_subscription = cx.subscribe(
            &player,
            |view, _, event: &crate::playback::SoundStarted, cx| {
                view.integrations
                    .update(cx, |integrations, _| integrations.started(event));
                view.record_listening(event.clone(), cx);
                view.station_track_started(&event.entry, cx);
            },
        );
        let listening_subscription =
            cx.subscribe(&player, |view, _, event: &ListeningChanged, cx| {
                view.integrations
                    .update(cx, |integrations, _| integrations.playback_changed(event.0));
            });
        let sound_cache_subscription = cx.subscribe(
            &player,
            |view, _, _: &crate::playback::SoundCacheChanged, cx| {
                view.refresh_library_downloads(cx)
            },
        );
        let station_end_subscription = cx.subscribe(
            &player,
            |view, _, event: &crate::playback::StationTrackEnded, _| {
                view.station_track_ended(event)
            },
        );
        let collection_progress_subscription = cx.subscribe(
            &player,
            |view, _, event: &crate::playback::CollectionProgressUpdate, cx| {
                view.record_collection_listening(event.clone(), cx);
            },
        );
        let queue_resume_subscription = cx.subscribe(
            &player,
            |view, _, event: &crate::playback::QueueResumeUpdate, cx| {
                view.record_queue_resume(event.clone(), cx);
            },
        );
        let player_snapshot = player.read(cx).snapshot();
        let player_subscription = cx.observe_in(&player, window, |view, player, window, cx| {
            let state = player.read(cx).snapshot();
            if view.player_snapshot != state {
                if view.player_snapshot.queue_revision != state.queue_revision {
                    view.panel.update_queue_list(state.queue_len);
                }
                view.player_snapshot = state;
                view.refresh_genius_credits(cx);
                if !player.read(cx).dormant() {
                    view.playback_prefetch_changed(cx);
                }
                view.station_player_changed(cx);
                view.recommendation_player_changed(cx);
                view.autoplay_player_changed(cx);
                view.update_player_favorite(cx);
                if view.player_snapshot.soloist_setup_required {
                    view.open_soloist_on_playback(view.player_snapshot.clone(), window, cx);
                }
                cx.notify();
            }
        });
        let visibility_subscription = visible.then(|| {
            cx.observe_window_visibility(window, |view, visibility, _, cx| {
                let hidden = !visibility.is_visible();
                if hidden {
                    view.stop_workspace_scrolling();
                }
                view.motion.frame(
                    view.started.elapsed(),
                    view.hovered,
                    view.reduced_motion,
                    hidden,
                );
                cx.notify();
            })
        });
        let (track_density, track_column_visibility) = crate::storage::Store::open_default()
            .and_then(|store| store.track_display())
            .unwrap_or_default();
        let mut view = Self {
            state: State::default(),
            workspace: Workspace::new(window, cx, fluid),
            fluid_backdrop,
            home: HomeUi::new(cx),
            playlist: PlaylistUi::new(fluid, cx),
            likes: likes::LikeUi::default(),
            music: MusicUi::new(fluid, cx),
            station: station::StationUi::default(),
            autoplay: autoplay::AutoplayUi::default(),
            history: HistoryUi::default(),
            panel: RightPanel::new(cx),
            lyrics,
            listening: Listening::new(cx),
            pins: workspace::pins::Pins::new(cx),
            player,
            player_snapshot,
            _player_subscription: player_subscription,
            _favorite_subscription: favorite_subscription,
            _queue_subscription: queue_subscription,
            _track_subscription: track_subscription,
            _lyrics_subscription: lyrics_subscription,
            _fullscreen_lyrics_subscription: fullscreen_lyrics_subscription,
            _listening_view_subscription: listening_view_subscription,
            _entity_subscription: entity_subscription,
            _track_menu_subscription: track_menu_subscription,
            _sound_subscription: sound_subscription,
            _listening_subscription: listening_subscription,
            _integrations_subscription: integrations_subscription,
            _sound_cache_subscription: sound_cache_subscription,
            _collection_progress_subscription: collection_progress_subscription,
            _queue_resume_subscription: queue_resume_subscription,
            _station_end_subscription: station_end_subscription,
            soloist: soloist::SoloistUi::new(window, cx, visible),
            login: None,
            connection_worker: None,
            focus: cx.focus_handle(),
            connection_focus: cx.focus_handle(),
            provider_focus: std::array::from_fn(|index| {
                cx.focus_handle()
                    .tab_index(index as isize + 1)
                    .tab_stop(true)
            }),
            connection_motion: ConnectionMotion::default(),
            connection_focus_pending: false,
            settings_focus_pending: None,
            appearance,
            track_density,
            track_column_visibility,
            appearance_error: None,
            appearance_notice: None,
            gallery_open: false,
            appearance_quality_select: components::overlay::Select::new(830, cx),
            #[cfg(feature = "capture")]
            settings_diagnostics: false,
            #[cfg(feature = "capture")]
            shader_debug: settings::ShaderDebug::new(cx),
            #[cfg(feature = "capture")]
            icon_gallery: {
                let mut active = [false; design_system::icons::Glyph::ALL.len()];
                // Start the examples that only make sense in their alternate
                // state (repeat-one and muted) in that state.
                active[3] = true;
                active[10] = true;
                active
            },
            #[cfg(feature = "capture")]
            gallery_inputs: components::search_input::GalleryInputs::new(cx),
            #[cfg(feature = "capture")]
            editorial_menu_samples: settings::EditorialMenuSamples::new(cx),
            #[cfg(feature = "capture")]
            editorial_gallery_tab: 0,
            sound_download_settings: settings::SoundDownloadSettings::default(),
            genius_settings: settings::GeniusSettings::new(cx),
            integrations,
            integration_controls: settings::IntegrationControls::new(cx),
            settings_sections: std::array::from_fn(|_| cx.focus_handle()),
            scroll: ScrollHandle::new(),
            motion: Motion::default(),
            hovered: None,
            started: Instant::now(),
            boot_splash: visible,
            reduced_motion,
            capture_time: fixed_motion.map(|_| Duration::ZERO),
            diagnostics: None,
            _visibility_subscription: visibility_subscription,
        };
        if visible {
            if fluid {
                view.set_fluid_high_contrast(native::high_contrast(), cx);
            }
            // Two frames at 60 Hz: the splash is presented before the frame
            // that compiles the backdrop pipeline.
            cx.spawn_in(window, async move |view, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(BOOT_SPLASH))
                    .await;
                let _ = view.update_in(cx, |view, _window, cx| {
                    if view.boot_splash {
                        view.boot_splash = false;
                        cx.notify();
                    }
                });
            })
            .detach();
            let entity = cx.weak_entity();
            window.on_window_should_close(cx, move |window, cx| {
                entity
                    .update(cx, |view, cx| view.close_after_saved_route(window, cx))
                    .unwrap_or(true)
            });
            window.focus(&view.focus, cx);
            view.load_like_defaults(cx);
            view.load_saved_panel(cx);
            view.load_pins(cx);
            cx.spawn(async move |view, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if view
                        .update(cx, |view, cx| {
                            if view.uses_fluid_glass() {
                                view.set_fluid_high_contrast(native::high_contrast(), cx);
                            }
                            let reduced = native::reduced_motion();
                            if view.reduced_motion != reduced {
                                view.reduced_motion = reduced;
                                cx.set_reduce_motion(reduced);
                                view.player.update(cx, |player, cx| {
                                    player.set_reduced_motion(reduced, cx);
                                });
                                if reduced {
                                    view.connection_motion.settle();
                                }
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .detach();
        }
        view
    }

    #[cfg(feature = "capture")]
    pub fn sample_welcome(&mut self, time: Duration, hover: Option<usize>, cx: &mut Context<Self>) {
        self.capture_time = Some(time);
        self.hovered = hover.filter(|index| *index < effects::ACCENTS.len());
        cx.notify();
    }

    #[cfg(feature = "capture")]
    pub fn is_welcome_for_capture(&self) -> bool {
        self.is_welcome()
    }

    #[cfg(feature = "capture")]
    pub fn load_saved_state_for_capture(&mut self, cx: &mut Context<Self>) {
        self.load_saved_panel(cx);
        self.load_pins(cx);
    }

    fn is_welcome(&self) -> bool {
        self.login.is_none() && matches!(self.state.screen(), Screen::Welcome)
    }

    fn uses_fluid_glass(&self) -> bool {
        self.fluid_backdrop.is_some()
    }

    fn presentation_time(&self) -> Duration {
        self.capture_time.unwrap_or_else(|| self.started.elapsed())
    }

    fn open_connection_motion(&mut self, provider: crate::providers::ProviderId, window: &Window) {
        self.connection_motion.activate(
            self.presentation_time(),
            provider,
            !welcome::split_layout(window.viewport_size().width),
        );
        if self.reduced_motion {
            self.connection_motion.settle();
        }
    }

    fn set_connection_open(&mut self, open: bool) {
        if !open {
            self.connection_focus_pending = false;
        }
        self.connection_motion
            .set_open(self.presentation_time(), open);
    }

    #[cfg(feature = "capture")]
    pub fn select_connection_for_capture(
        &mut self,
        provider: crate::providers::ProviderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_provider(provider, window, cx);
    }

    #[cfg(feature = "capture")]
    pub fn submit_empty_jellyfin_for_capture(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.submit_connection(
            crate::providers::LoginInput::JellyfinPassword {
                server: String::new(),
                username: String::new(),
                password: String::new(),
            },
            window,
            cx,
        );
    }

    #[cfg(feature = "capture")]
    pub fn failed_connection_for_capture(&self) -> bool {
        matches!(self.state.screen(), Screen::Failed(_))
    }

    #[cfg(feature = "capture")]
    pub fn start_home_for_capture(
        &mut self,
        restore: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.run(
            if restore {
                Intent::Startup
            } else {
                Intent::Guest
            },
            cx,
        );
    }

    #[cfg(feature = "capture")]
    pub fn home_status_for_capture(&self) -> Option<&'static str> {
        use crate::state::HomeContent;
        let (home, busy) = self.state.home_view()?;
        if busy {
            return None;
        }
        Some(match home.content {
            HomeContent::Loading => return None,
            HomeContent::Fresh(_) => "fresh",
            HomeContent::Partial { .. } => "partial",
            HomeContent::Cached { .. } => "cached",
            HomeContent::Empty(_) => "empty",
        })
    }

    #[cfg(feature = "capture")]
    pub fn start_home_skeleton_for_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.state.begin(Intent::Startup);
        cx.notify();
    }

    #[cfg(feature = "capture")]
    pub fn home_scroll_offset_for_capture(&self) -> f32 {
        f32::from(self.scroll.offset().y)
    }

    #[cfg(feature = "capture")]
    pub fn home_scroll_extent_for_capture(&self) -> f32 {
        f32::from(self.scroll.max_offset().y)
    }

    #[cfg(feature = "capture")]
    pub fn set_home_scroll_offset_for_capture(&self, offset: f32, cx: &mut Context<Self>) {
        let mut position = self.scroll.offset();
        position.y = -px(offset).clamp(px(0.), self.scroll.max_offset().y);
        self.scroll.set_offset(position);
        cx.notify();
    }

    #[cfg(feature = "capture")]
    pub fn home_search_focused_for_capture(&self, window: &Window, cx: &App) -> bool {
        self.workspace.search.is_focused(window, cx)
    }

    #[cfg(feature = "capture")]
    pub fn diagnostics_window_for_capture(&self) -> Option<gpui::AnyWindowHandle> {
        self.diagnostics.map(Into::into)
    }

    #[cfg(feature = "capture")]
    pub fn diagnostics_activity_for_capture(&self, cx: &App) -> Option<(bool, bool)> {
        Some(self.diagnostics?.read(cx).ok()?.activity())
    }

    fn show_diagnostics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(existing) = self.diagnostics
            && existing
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        self.diagnostics = match Diagnostics::open(window, cx) {
            Ok(handle) => Some(handle),
            Err(error) => {
                eprintln!("Не удалось открыть диагностику Mediadeka: {error}");
                None
            }
        };
    }

    fn reset_scroll(&self) {
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    fn dismiss_global_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let query = match self.workspace.routes.current() {
            crate::ui::workspace::Route::Search { query, .. } => query.clone(),
            _ => String::new(),
        };
        self.workspace
            .search
            .dismiss(&query, &self.focus, window, cx);
    }
}

impl MainView {
    /// The first painted frame. It deliberately draws no backdrop: the Fluid
    /// Glass pipeline compiles on the frame that first uses it and blocks the
    /// UI thread for seconds, so that frame must come after something is
    /// already visible.
    fn splash(&self) -> impl IntoElement {
        use crate::ui::fluid_glass::tokens;
        let (background, ink, muted) = if self.uses_fluid_glass() {
            (tokens::CANVAS, tokens::PRIMARY, tokens::SECONDARY)
        } else {
            (theme::WINDOW, theme::PRIMARY, theme::SECONDARY)
        };
        div()
            .id("boot-splash")
            .accessibility_id("boot-splash")
            .role(Role::Status)
            .aria_label("Mediadeka загружается")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(14.))
            .bg(rgb(background))
            .text_color(rgb(ink))
            .child(assets::mark("mediadeka.svg", px(56.)))
            .child(
                div()
                    .text_size(px(20.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child(TITLE),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(muted))
                    .child("Загрузка…"),
            )
    }
}

impl Render for MainView {
    #[allow(refining_impl_trait)]
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.boot_splash {
            return self.splash().into_any_element();
        }
        if let Some(provider) = self.settings_focus_pending.take() {
            window.focus(&self.provider_focus[provider_index(provider)], cx);
        }
        self.focus_soloist_if_pending(window, cx);
        let base = div()
            .id("mediadeka")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Tab, window, cx| this.tab_focus(false, window, cx)))
            .on_action(
                cx.listener(|this, _: &ShiftTab, window, cx| this.tab_focus(true, window, cx)),
            )
            .on_action(cx.listener(|this, _: &Escape, window, cx| {
                if this.dismiss_lyrics_picker(window, cx) {
                    return;
                }
                if this.listening.mode().is_some() {
                    this.set_listening_mode(None, window, cx);
                    return;
                }
                if this.workspace.search.is_focused(window, cx) {
                    this.dismiss_global_search(window, cx);
                    return;
                }
                if this.close_soloist(window, cx)
                    || this.close_like_menu(window, cx)
                    || this.dismiss_playlist_overlay(window, cx)
                    || (this.panel_contains_focus(window, cx) && this.close_panel(window, cx))
                    || this
                        .player
                        .update(cx, |bar, cx| bar.dismiss_devices(window, cx))
                {
                    return;
                }
                if this.dismiss_library_filters(window, cx) {
                    return;
                }
                if this.stop_workspace_scrolling() {
                    cx.notify();
                } else {
                    this.escape(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusGlobalSearch, window, cx| {
                if this.listening.mode().is_none() {
                    this.focus_global_search(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusPageSearch, window, cx| {
                if this.listening.mode().is_none() {
                    this.focus_page_search(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleListening, window, cx| {
                this.toggle_listening(window, cx);
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .text_color(if self.uses_fluid_glass() {
                rgb(fluid_glass::tokens::PRIMARY).into()
            } else {
                primary()
            })
            .font_family(if self.uses_fluid_glass() {
                fluid_glass::tokens::FONT
            } else {
                FONT_FAMILY
            });
        if let Some(mode) = self.listening.mode() {
            return base
                .child(self.listening_view(mode, window, cx))
                .into_any_element();
        }
        self.prepare_panel_geometry(window, cx);
        self.settle_pending_scroll();
        self.prepare_library_view(window);
        let startup = matches!(self.state.screen(), Screen::Pending(Intent::Startup));
        self.restore_workspace_focus(startup || self.state.home_view().is_some(), window, cx);
        let home = self.state.home_view();
        if home.is_some() || startup {
            let (view, busy) = home.map_or((None, true), |(view, busy)| (Some(view), busy));
            let skeleton =
                view.is_none_or(|view| matches!(view.content, crate::state::HomeContent::Loading));
            if skeleton
                && !self.reduced_motion
                && self.capture_time.is_none()
                && !native::minimized(window)
            {
                window.request_animation_frame();
            }
            let like_overlay = self.like_overlay(window, cx);
            let login_overlay = self.settings_login_overlay(window, cx);
            let soloist_overlay = self.soloist_overlay(window, cx);
            let content_timing =
                (like_overlay.is_none() && login_overlay.is_none() && soloist_overlay.is_none())
                    .then(|| self.route_paint_marker(window, cx))
                    .flatten();
            return base
                .child(self.music_workspace(view, busy, window, cx))
                .children(content_timing)
                .children(like_overlay)
                .children(login_overlay)
                .children(soloist_overlay)
                .into_any_element();
        }
        self.stop_workspace_scrolling();

        let welcome = self.is_welcome() || self.login.is_some() || self.guest_status();
        if !welcome || !self.uses_fluid_glass() {
            self.suspend_fluid_backdrop(cx);
        }
        let animated = welcome && !self.reduced_motion && !native::minimized(window);
        let now = self.presentation_time();
        let connection = self.connection_motion.at(now, self.reduced_motion);
        if connection.flight.is_none_or(Flight::controls_visible)
            && std::mem::take(&mut self.connection_focus_pending)
            && self.connection_focus.is_focused(window)
        {
            self.focus_connection(window, cx);
        }
        let frame = self.motion.frame(
            now,
            self.hovered.filter(|_| welcome),
            self.reduced_motion,
            !animated && !self.reduced_motion,
        );
        // Icon morphs are driven from this frame; a settled icon costs nothing.
        if self.capture_time.is_none() && design_system::icons::active(cx) {
            window.request_animation_frame();
        }
        if animated
            && self.capture_time.is_none()
            && (self.motion.is_active() || self.connection_motion.is_active(&connection))
        {
            window.request_animation_frame();
        }

        let content = if welcome {
            self.welcome(frame, connection, window, cx)
                .into_any_element()
        } else {
            self.scroll_area(self.screen(cx), gpui::Pixels::ZERO, window)
                .into_any_element()
        };
        base.bg(guise::theme(cx).body().hsla())
            .when(welcome && self.uses_fluid_glass(), |base| {
                base.child(self.fluid_scene_backdrop(
                    cx,
                    fluid_glass::tokens::atmosphere::HOME,
                    false,
                    None,
                ))
            })
            .child(chrome(window, cx))
            .child(content)
            .into_any_element()
    }
}

impl MainView {
    fn scroll_area(
        &self,
        content: impl IntoElement,
        header_height: gpui::Pixels,
        window: &Window,
    ) -> components::scroll_viewport::FluidScrollViewport {
        use components::{
            scroll_viewport::{Density, FluidScrollViewport},
            scrollbar::ScrollSurface,
        };
        let area = div()
            .id("screen-scroll")
            .relative()
            .w_full()
            .min_h_0()
            .flex_1()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(content);
        let geometry =
            fluid_glass::ShellGeometry::for_workspace(window, self.docked_panel_width(window));
        let safe_left =
            f32::from(geometry.safe_content_bounds().origin.x - geometry.content_bounds().origin.x);
        let side = px(geometry.base().edge_fade_side().max(safe_left + 8.));
        FluidScrollViewport::new(
            area,
            ScrollSurface::Area(self.scroll.clone()),
            self.uses_fluid_glass(),
            Density::Content,
        )
        .side_fade(side)
        .under_header(header_height)
    }
}

fn chrome(window: &Window, cx: &Context<MainView>) -> impl IntoElement {
    div()
        .relative()
        .h(px(TITLEBAR_HEIGHT))
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .child(
            div()
                .flex_1()
                .h_full()
                .window_control_area(WindowControlArea::Drag),
        )
        .child(window_controls(window, cx))
}

fn window_controls(window: &Window, cx: &Context<MainView>) -> Div {
    let (maximize_name, maximize_icon) = if window.is_maximized() {
        (
            "Восстановить",
            Icon::new(IconName::Copy)
                .pixel_size(px(17.))
                .color(primary()),
        )
    } else {
        (
            "Развернуть",
            Icon::new(IconName::Square)
                .pixel_size(px(17.))
                .color(primary()),
        )
    };
    div()
        .flex_none()
        .flex()
        .items_center()
        .child(
            chrome_button(
                "diagnostics",
                "Диагностика",
                svg()
                    .path("diagnostics.svg")
                    .size(px(17.))
                    .flex_none()
                    .text_color(primary()),
                99,
            )
            .on_activate(cx.listener(|view, _, window, cx| view.show_diagnostics(window, cx))),
        )
        .child(
            chrome_button(
                "minimize",
                "Свернуть",
                Icon::new(IconName::Minus)
                    .pixel_size(px(17.))
                    .color(primary()),
                100,
            )
            .on_activate(|_, window, cx| native::run(Command::Minimize, window, cx)),
        )
        .child(
            chrome_button("maximize", maximize_name, maximize_icon, 101)
                .on_activate(|_, window, cx| native::run(Command::ToggleMaximize, window, cx)),
        )
        .child(
            chrome_button(
                "close",
                "Закрыть",
                Icon::new(IconName::X).pixel_size(px(17.)).color(primary()),
                102,
            )
            .on_activate(|_, window, cx| native::run(Command::Close, window, cx)),
        )
        .pr(px(CHROME_END_PADDING))
}

fn chrome_button(
    id: &'static str,
    label: &'static str,
    glyph: impl IntoElement,
    order: isize,
) -> Stateful<Div> {
    let color = if id == "close" { 0xad3153 } else { 0x635675 };
    let hover = gpui::Hsla::from(gpui::rgb(color)).alpha(0.44);
    button(id, SharedString::from(label), order)
        .h(px(34.))
        .w(px(CHROME_BUTTON_WIDTH))
        .justify_center()
        .rounded(px(8.))
        .hover(move |style| style.bg(hover))
        .active(move |style| style.bg(hover.alpha(0.68)))
        .child(glyph)
}
