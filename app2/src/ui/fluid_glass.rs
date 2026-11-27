//! Shared geometry and materials for the Fluid Glass presentation.

#[cfg(feature = "capture")]
mod acceptance;
pub(super) mod artist_portrait;
pub(super) mod artwork;
mod audio;
mod clock;
mod contextual;
mod dock;
mod exposure;
mod geometry;
mod graphics;
pub(super) use graphics::studio::EFFECTS as DEBUG_EFFECTS;
#[cfg(feature = "capture")]
pub(super) use graphics::studio::{Pass as DebugPass, comparison_flags};
mod palette;
mod preview;
mod quality;
#[cfg(feature = "capture")]
pub(in crate::ui) use preview::quality_preview_label;
mod reactivity;
pub(super) mod tokens;
pub(super) mod track_change;
mod waveform;

use std::time::Duration;

use gpui::{
    Context, Div, Entity, IntoElement, Pixels, Point, Render, StyleRefinement, Styled,
    Subscription, Task, Window, div, prelude::*, px,
};

use super::{components::artwork as cover_cache, player::PlayerBar};
use crate::{
    appearance::GlassPalette,
    playback::{SoundSource, SoundStarted},
    visual::VisualSettings,
};
pub(super) use audio::Availability as AudioAvailability;
use audio::modulation::Modulation;
use clock::Clock;
use contextual::Contextual;
pub(super) use contextual::{Ambient, PageAtmosphere};
pub(super) use dock::{Dock, Surface as DockSurface};
use exposure::Exposure;
pub(super) use geometry::{Geometry, ShellGeometry};
use palette::{Cover, Field, Palette};
use quality::{Quality, QualitySelection};
use reactivity::Reactivity;
use track_change::{PALETTE_DELAY, RowLaunch, RowOrigin, TrackChange};

const PALETTE_TRANSITION: Duration = crate::ui::design_system::motion::MOTION.palette();

pub(super) fn effects_enabled() -> bool {
    !std::env::var("MEDIADEKA_GLASS_EFFECTS")
        .ok()
        .is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "off" | "none"))
}

pub(super) fn transition_motion_configured() -> bool {
    effects_enabled() && Quality::configured() != Some(Quality::Static)
}

#[derive(Clone, Copy)]
struct Sample {
    wall: Duration,
    active: Option<bool>,
    live_audio: bool,
    shader_phase: Option<Duration>,
}

#[derive(Clone, Copy, PartialEq)]
enum Scene {
    Workspace,
    Standalone { exposure: f32, player: bool },
}

pub(super) struct Backdrop {
    clock: Clock,
    frame: Option<Task<()>>,
    sample: Option<Sample>,
    effects: bool,
    quality: QualitySelection,
    theme: GlassPalette,
    cover: Cover,
    selected_track: Option<(u64, usize)>,
    track_change: Option<TrackChange>,
    row_launch: RowLaunch,
    pending_palette: Option<Palette>,
    immediate_palette: bool,
    from: Field,
    target: Palette,
    changed: Duration,
    dock: DockSurface,
    exposure: Exposure,
    contextual: Contextual,
    scene: Scene,
    process_audio_playing: bool,
    audio: Reactivity,
    audio_availability: AudioAvailability,
    last_pattern: contextual::PatternMix,
    last_field: Field,
    frame_clock: Option<std::time::Instant>,
    frame_ms: f32,
    #[cfg(feature = "capture")]
    hide_waveform_for_capture: bool,
    #[cfg(feature = "capture")]
    hide_row_launch_for_capture: bool,
    _track: Option<Subscription>,
    _sound: Option<Subscription>,
    _activation: Option<Subscription>,
    _visibility: Option<Subscription>,
}

impl gpui::EventEmitter<AudioAvailability> for Backdrop {}

impl Backdrop {
    pub(super) fn new(
        player: &Entity<PlayerBar>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let theme = super::appearance::palette(cx);
        let visual = super::appearance::visual(cx);
        let effects = effects_enabled();
        let palette = theme.palette(None);
        let track = effects.then(|| {
            cx.observe(player, |view, player, cx| {
                let player = player.read(cx);
                let playing = renders_process_audio(player);
                let playback_changed = view.process_audio_playing != playing;
                view.process_audio_playing = playing;
                let selected = player.fluid_track_position();
                let track_changed = view.selected_track != selected;
                if track_changed {
                    view.selected_track = selected;
                    view.track_change = selected.map(|_| TrackChange::default());
                    view.row_launch.track_changed(selected);
                    view.contextual
                        .select_track(player.current_entry().map(|entry| entry.track.id.as_str()));
                }
                let cover_changed =
                    view.theme == GlassPalette::Auto && view.cover.select(player.current_artwork());
                if cover_changed {
                    view.pending_palette = None;
                }
                if !super::theme::fluid_high_contrast(cx)
                    && (playback_changed || cover_changed || track_changed)
                {
                    cx.notify();
                }
            })
        });
        let sound = effects.then(|| {
            cx.subscribe(player, |view, player, started: &SoundStarted, cx| {
                let player = player.read(cx);
                let current = player.current().is_some_and(|entry| {
                    entry.playlist == started.entry.playlist
                        && entry.index == started.entry.index
                        && entry.track.id == started.entry.track.id
                });
                if current
                    && view.scene == Scene::Workspace
                    && view.has_optics(cx)
                    && player.is_playing()
                    && view.row_launch.sound_started(player.fluid_track_position())
                {
                    cx.notify();
                }
            })
        });
        Self {
            clock: Clock::new(),
            frame: None,
            sample: None,
            effects,
            quality: QualitySelection::configured(window, visual.quality),
            theme,
            cover: Cover::empty(),
            selected_track: player.read(cx).fluid_track_position(),
            track_change: None,
            row_launch: RowLaunch::default(),
            pending_palette: None,
            immediate_palette: false,
            from: Field::uniform(palette),
            target: palette,
            changed: Duration::ZERO,
            dock: DockSurface::default(),
            exposure: Exposure::new(tokens::atmosphere::HOME),
            contextual: Contextual::default(),
            scene: Scene::Workspace,
            process_audio_playing: renders_process_audio(player.read(cx)),
            audio: Reactivity::default(),
            audio_availability: AudioAvailability::Inactive,
            last_pattern: contextual::PatternMix::default(),
            last_field: Field::uniform(palette),
            frame_clock: None,
            frame_ms: 0.,
            #[cfg(feature = "capture")]
            hide_waveform_for_capture: false,
            #[cfg(feature = "capture")]
            hide_row_launch_for_capture: false,
            _track: track,
            _sound: sound,
            _activation: effects
                .then(|| cx.observe_window_activation(window, Self::activity_changed)),
            _visibility: effects.then(|| {
                cx.observe_window_visibility(window, |view, _, window, cx| {
                    view.activity_changed(window, cx)
                })
            }),
        }
    }

    /// Applies pattern, effect and performance choices. The pattern and flags
    /// are read from the global each frame; the quality profile needs a new
    /// raster schedule and therefore a repaint.
    pub(super) fn set_visual(
        &mut self,
        visual: VisualSettings,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.quality = QualitySelection::configured(window, visual.quality);
        self.sample = None;
        cx.notify();
    }

    pub(super) fn audio_availability(&self) -> AudioAvailability {
        self.audio_availability
    }

    /// Selects a page atmosphere while keeping a standalone scene (fullscreen
    /// lyrics and now-playing keep the shell geometry of the scene).
    pub(super) fn set_page(&mut self, page: Option<PageAtmosphere<'_>>, cx: &mut Context<Self>) {
        if self.contextual.differs(page) {
            self.contextual.select(page);
            cx.notify();
        }
    }

    pub(super) fn set_workspace(
        view: &Entity<Self>,
        dock: DockSurface,
        exposure: f32,
        page: Option<PageAtmosphere<'_>>,
        cx: &mut gpui::App,
    ) {
        let current = view.read(cx);
        let resize_dock = current.dock != dock;
        let grade_scene = current.exposure.target() != exposure;
        let change_context = current.contextual.differs(page);
        if resize_dock || grade_scene || current.scene != Scene::Workspace || change_context {
            view.update(cx, |view, cx| {
                view.scene = Scene::Workspace;
                if change_context {
                    view.contextual.select(page);
                }
                if resize_dock {
                    view.dock = dock;
                }
                if grade_scene {
                    view.exposure.select(exposure);
                }
                cx.notify();
            });
        }
    }

    /// Applies a palette chosen in the settings immediately. Auto starts from
    /// Solar and takes the current cover on the next paint, where the artwork
    /// cache is available; the cache must not be read from an event handler.
    pub(super) fn set_palette(
        &mut self,
        palette: GlassPalette,
        artwork: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        if self.theme == palette {
            return;
        }
        self.theme = palette;
        self.cover = Cover::empty();
        self.contextual.clear_light();
        self.pending_palette = None;
        self.immediate_palette = palette == GlassPalette::Auto;
        if palette == GlassPalette::Auto
            && let Some(url) = artwork
        {
            self.cover.select(Some(url));
        }
        let target = palette.palette(None);
        self.from = Field::uniform(target);
        self.target = target;
        cx.notify();
    }

    fn set_standalone(view: &Entity<Self>, exposure: f32, player: bool, cx: &mut gpui::App) {
        let scene = Scene::Standalone { exposure, player };
        if view.read(cx).scene != scene {
            view.update(cx, |view, cx| {
                view.scene = scene;
                view.row_launch = RowLaunch::default();
                cx.notify();
            });
        }
    }

    fn palette_at(&self, now: Duration, animated: bool) -> Field {
        if !animated {
            return Field::uniform(self.target);
        }
        let progress =
            now.saturating_sub(self.changed).as_secs_f32() / PALETTE_TRANSITION.as_secs_f32();
        self.from.toward(self.target, progress)
    }

    fn has_optics(&self, cx: &gpui::App) -> bool {
        self.effects
            && !super::theme::fluid_high_contrast(cx)
            && !super::appearance::visual(cx).surface.neutral
    }

    fn set_high_contrast(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.track_change = None;
        self.row_launch = RowLaunch::default();
        if enabled {
            self.suspend();
        }
        cx.notify();
    }

    fn activity_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !can_present(window) {
            self.suspend();
        }
        cx.notify();
    }

    fn suspend(&mut self) -> bool {
        let running = self.clock.pause();
        self.audio.suspend();
        let row_visible = self.row_launch.cancel();
        self.frame.take().is_some() || running || row_visible
    }

    #[cfg(feature = "capture")]
    fn sample(&mut self, sample: Sample, cx: &mut Context<Self>) {
        self.sample = Some(sample);
        cx.notify();
    }
}

fn renders_process_audio(player: &PlayerBar) -> bool {
    player.is_playing()
        && player
            .current_sound_source()
            .is_some_and(|source| source != SoundSource::Soloist)
}

fn can_present(window: &Window) -> bool {
    window.is_window_active() && window.is_visible()
}

impl Render for Backdrop {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(previous) = self.frame_clock.replace(std::time::Instant::now()) {
            let elapsed = previous.elapsed().as_secs_f32() * 1000.;
            // Ignore idle gaps; the meter is the cadence while the material runs.
            if elapsed > 0.1 && elapsed < 250. {
                self.frame_ms = if self.frame_ms > 0. {
                    self.frame_ms * 0.8 + elapsed * 0.2
                } else {
                    elapsed
                };
            }
        }
        let quality = self.quality.effective(window, cx.reduce_motion());
        let optics = self.has_optics(cx);
        let interval = if optics && !matches!(self.scene, Scene::Standalone { player: false, .. }) {
            quality.frame_interval()
        } else {
            None
        };
        if interval.is_none() {
            self.frame_ms = 0.;
        }
        let motion = interval.is_some();
        let active = motion
            && self
                .sample
                .and_then(|sample| sample.active)
                .unwrap_or_else(|| can_present(window));
        // Only a presented window schedules frames. Freezing the clock keeps
        // the shader and palette on the last phase while the timer is stopped.
        if !active || self.sample.is_some() {
            self.frame.take();
        } else if self.frame.is_none()
            && let Some(interval) = interval
        {
            self.frame = Some(cx.spawn(async move |view, cx| {
                cx.background_executor().timer(interval).await;
                let _ = view.update(cx, |view, cx| {
                    view.frame = None;
                    cx.notify();
                });
            }));
        }
        let now = match self.sample {
            Some(sample) if motion && (sample.active.is_none() || sample.live_audio) => sample.wall,
            sample => self.clock.phase(sample.map(|sample| sample.wall), active),
        };
        let settings = super::appearance::visual(cx);
        let features = self.audio.update(
            motion
                && quality.reacts_to_audio()
                && settings.audio.enabled
                && self.sample.is_none_or(|sample| sample.live_audio),
            active,
            self.process_audio_playing,
        );
        let availability = self.audio.availability();
        if self.audio_availability != availability {
            self.audio_availability = availability;
            cx.emit(availability);
        }
        let audio = Modulation::new(settings.audio, settings.effects, features);
        let cover_revision = self
            .cover
            .url()
            .and_then(|cover| cover_cache::cover_revision(cover, cx));
        if optics
            && let Some(colors) = self.cover.resolve(cover_revision, |cover| {
                cover_cache::cover_colors(cover, window, cx)
            })
        {
            let palette = self.theme.palette(colors);
            if self.immediate_palette {
                self.from = Field::uniform(palette);
                self.target = palette;
                self.immediate_palette = false;
            } else {
                self.pending_palette = Some(palette);
            }
        }
        if optics && self.theme == GlassPalette::Auto && self.scene == Scene::Workspace {
            let revision = self
                .contextual
                .artwork_url()
                .and_then(|cover| cover_cache::cover_revision(cover, cx));
            self.contextual.resolve(revision, |cover| {
                cover_cache::cover_colors(cover, window, cx)
            });
        }
        let transition_visible =
            active || (motion && self.sample.is_some_and(|s| s.active.is_none()));
        let row_flight = if transition_visible && quality.glitch() {
            self.row_launch.sample(now, window.viewport_size())
        } else {
            if !motion || !quality.glitch() {
                self.row_launch = RowLaunch::default();
            }
            None
        };
        #[cfg(feature = "capture")]
        let row_flight = row_flight.filter(|_| !self.hide_row_launch_for_capture);
        let track_age = if transition_visible {
            self.track_change.as_mut().map(|change| change.age(now))
        } else {
            if !motion || (self.sample.is_none() && !can_present(window)) {
                self.track_change = None;
            }
            None
        };
        if track_age.is_none_or(|age| age >= PALETTE_DELAY)
            && let Some(target) = self.pending_palette.take()
            && target != self.target
        {
            self.from = self.palette_at(now, motion);
            self.target = target;
            self.changed = now;
        }
        let pulse = track_age.map_or(0., TrackChange::pulse);
        if track_age.is_some_and(|age| age >= PALETTE_DELAY) {
            self.track_change = None;
        }
        let shader_phase = self
            .sample
            .and_then(|sample| sample.shader_phase)
            .filter(|_| motion)
            .unwrap_or(now);
        let mut geometry = Geometry::for_window(window);
        let (dock, exposure) = match self.scene {
            Scene::Workspace => (
                self.dock,
                self.exposure.sample(now, motion && self.sample.is_none()),
            ),
            Scene::Standalone { exposure, player } => {
                geometry = geometry.without_navigation(player);
                (DockSurface::default(), exposure)
            }
        };
        let palette = self.palette_at(now, motion);
        if palette == Field::uniform(self.target) {
            self.from = palette;
        }
        let mut palette = palette;
        if self.scene == Scene::Workspace && self.theme == GlassPalette::Auto {
            // The current track owns the player. The page starts from the
            // stable Auto palette and the route can supply its own artwork.
            palette.upper = GlassPalette::Auto.palette(None);
        }
        let (pattern, palette) = self.contextual.sample(
            now,
            motion,
            palette,
            settings,
            self.scene != Scene::Workspace,
        );
        self.last_pattern = pattern;
        self.last_field = palette;
        let trace = self.audio.waveform();
        #[cfg(feature = "capture")]
        let trace = trace.filter(|_| !self.hide_waveform_for_capture);
        div().size_full().child(graphics::layer(
            geometry,
            dock,
            shader_phase.as_secs_f32(),
            motion,
            palette,
            if optics {
                graphics::Material::Procedural(quality)
            } else {
                graphics::Material::Flat {
                    high_contrast: super::theme::fluid_high_contrast(cx),
                }
            },
            graphics::Reactivity {
                audio,
                pulse,
                exposure,
                artwork_placement: self.contextual.artwork_placement(),
                hero_light: self.contextual.hero_light(),
                trace,
                pattern,
                flags: settings.effects,
                surface: settings.surface,
                row_flight,
            },
        ))
    }
}

impl super::MainView {
    pub(super) fn cancel_fluid_row_launch(&self, cx: &mut Context<Self>) {
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |view, cx| {
                if view.row_launch.cancel() {
                    cx.notify();
                }
            });
        }
    }

    pub(in crate::ui) fn play_from_fluid_row(
        &mut self,
        origin: Option<Point<Pixels>>,
        window: &Window,
        cx: &mut Context<Self>,
        play: impl FnOnce(&mut Self, &Window, &mut Context<Self>),
    ) {
        let request = origin
            .zip(self.fluid_backdrop.as_ref())
            .and_then(|(point, backdrop)| {
                let view = backdrop.read(cx);
                let viewport = window.viewport_size();
                let geometry = Geometry::for_window(window);
                let in_workspace = point.x >= px(geometry.inset() + geometry.rail())
                    && point.x < viewport.width - px(geometry.inset())
                    && point.y >= px(geometry.inset() + tokens::TITLEBAR)
                    && point.y < viewport.height - px(geometry.inset() + geometry.player());
                (in_workspace
                    && view.scene == Scene::Workspace
                    && view.has_optics(cx)
                    && view.quality.effective(window, cx.reduce_motion()).glitch())
                .then(|| {
                    (
                        backdrop.clone(),
                        self.player.read(cx).fluid_track_position(),
                        RowOrigin { point, viewport },
                    )
                })
            });
        play(self, window, cx);
        if let Some((backdrop, before, origin)) = request
            && let Some(after) = self.player.read(cx).fluid_track_position()
            && Some(after) != before
        {
            backdrop.update(cx, |view, _| view.row_launch.arm(after, origin));
        }
    }

    pub(super) fn fluid_scene_backdrop(
        &self,
        cx: &mut Context<Self>,
        exposure: f32,
        player_surface: bool,
        page: Option<PageAtmosphere<'_>>,
    ) -> Div {
        let backdrop = self.fluid_backdrop.as_ref().map(|backdrop| {
            Backdrop::set_standalone(backdrop, exposure, player_surface, cx);
            backdrop.update(cx, |view, cx| view.set_page(page, cx));
            backdrop
                .clone()
                .cached(StyleRefinement::default().size_full())
        });
        div().absolute().inset_0().children(backdrop)
    }

    pub(super) fn fluid_high_contrast(&self, cx: &gpui::App) -> bool {
        self.fluid_backdrop.is_some() && super::theme::fluid_high_contrast(cx)
    }

    pub(super) fn set_fluid_high_contrast(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let Some(backdrop) = &self.fluid_backdrop else {
            return;
        };
        if super::theme::fluid_high_contrast(cx) == enabled {
            return;
        }
        super::theme::set_fluid_contrast(cx, enabled);
        backdrop.update(cx, |backdrop, cx| backdrop.set_high_contrast(enabled, cx));
        cx.notify();
        cx.refresh_windows();
    }

    pub(super) fn suspend_fluid_backdrop(&self, cx: &mut Context<Self>) {
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |backdrop, cx| {
                if backdrop.suspend() {
                    cx.notify();
                }
            });
        }
    }
}
