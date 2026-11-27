//! The playback queue, track details and lyrics in the right panel.
//!
//! The panel is a separate surface of the workspace. It never changes the
//! route and keeps its state in the profile, not in the page history.

mod queue;
mod queue_motion;
mod resize;
mod track;
#[cfg(feature = "capture")]
pub(super) use queue::queue_row;

use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt, AnyElement, Context, Div, FocusHandle, FontWeight, IntoElement,
    ListAlignment, ListState, Role, ScrollHandle, SharedString, Stateful, Task, Window, div,
    prelude::*, px, relative, rgb,
};
use guise::{Chip, Icon, IconName, Size, Variant};

use crate::{
    storage::{PanelState, PanelTab, Store},
    ui::{
        MainView,
        components::{Activate, button, music::control},
        design_system::{
            controls::{ControlState, ControlVariant, paint_page_tab, tab_selection_marker},
            materials::{self, Material},
            motion::{MOTION, out_quad},
        },
        fluid_glass::{
            Dock, DockSurface,
            tokens::{self, size, space, text},
        },
        theme::{SURFACE, playlist, playlist::metrics, surface_border},
        workspace::Page,
    },
};

const WIDTH: f32 = 320.;
/// From this window width the panel takes its own column beside the page.
/// In narrower Fluid Glass windows the panel shares the workspace surface.
const INLINE_MIN_WIDTH: f32 = 1440.;
const DOCK_MIN_CONTENT_WIDTH: f32 = 216.;
const DOCK_CONTENT_REVEAL_WIDTH: f32 = 44.;
const TAB_GROUP: isize = 590;
const ACTION_GLYPH_SIZE: f32 = 18.;
const FLUID_TAB_WIDTH: f32 = 72.;
/// The tab row is split into equal slots, so the underline position is a
/// fraction of the row and never depends on the label widths.
const TABS: f32 = 3.;

fn tab_slot(tab: PanelTab) -> f32 {
    match tab {
        PanelTab::Queue => 0.,
        PanelTab::Track => 1.,
        PanelTab::Lyrics => 2.,
    }
}

#[derive(Clone, Copy)]
struct PanelTabMotion {
    from: f32,
    started: Instant,
}

impl PanelTabMotion {
    fn position(self, tab: PanelTab, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started);
        let progress = out_quad(elapsed.as_secs_f32() / MOTION.normal().as_secs_f32());
        self.from + (tab_slot(tab) - self.from) * progress
    }
}

pub(super) fn inline(window: &Window) -> bool {
    f32::from(window.viewport_size().width) >= INLINE_MIN_WIDTH
}

fn panel_icon_action(
    id: &'static str,
    label: &'static str,
    glyph: IconName,
    order: isize,
    material: Material,
    enabled: bool,
) -> Stateful<Div> {
    let button = button(id, label, order)
        .size(px(size::SM))
        .flex_none()
        .justify_center()
        .child(Icon::new(glyph).pixel_size(px(ACTION_GLYPH_SIZE)));
    ControlVariant::GhostIcon
        .paint(
            button,
            material,
            ControlState {
                disabled: !enabled,
                ..ControlState::default()
            },
        )
        .when(!enabled, |button| {
            button
                .opacity(0.5)
                .aria_description("Недоступно")
                .tab_stop(false)
                .cursor_default()
        })
}

impl MainView {
    /// The width an open docked panel takes from the page.
    pub(super) fn docked_panel_width(&self, window: &Window) -> f32 {
        if self.uses_fluid_glass() {
            self.docked_panel_surface(window).width
        } else if self.panel.open && inline(window) {
            self.right_panel_width(window)
        } else {
            0.
        }
    }

    pub(super) fn docked_panel_surface(&self, window: &Window) -> DockSurface {
        if inline(window) {
            self.panel.dock.surface(self.right_panel_width(window))
        } else {
            DockSurface::default()
        }
    }

    pub(super) fn prepare_panel_geometry(&mut self, window: &Window, cx: &Context<Self>) {
        if !self.uses_fluid_glass() {
            return;
        }
        #[cfg(feature = "capture")]
        let sample = self.panel.dock_sample;
        #[cfg(not(feature = "capture"))]
        let sample: Option<Duration> = None;
        let open = self.panel.open && inline(window);
        let animate = !self.reduced_motion
            && !self.fluid_high_contrast(cx)
            && inline(window)
            && (sample.is_some()
                || (self.capture_time.is_none()
                    && window.is_visible()
                    && window.is_window_active()));
        let now = sample.unwrap_or_else(|| self.panel.dock_clock.elapsed());
        if self.panel.dock.frame(open, now, animate) && sample.is_none() {
            window.request_animation_frame();
        }
    }

    #[cfg(feature = "capture")]
    pub fn sample_fluid_glass_dock_for_capture(
        &mut self,
        wall: Option<Duration>,
        cx: &mut Context<Self>,
    ) {
        self.panel.dock_sample = wall;
        cx.notify();
    }

    fn default_right_panel_width(&self) -> f32 {
        if self.uses_fluid_glass() {
            WIDTH
        } else if self.music_page() == Page::Playlist {
            metrics::PANEL_WIDTH
        } else {
            WIDTH
        }
    }

    fn right_panel_width(&self, window: &Window) -> f32 {
        let fallback = self.default_right_panel_width();
        if self.uses_fluid_glass() && inline(window) {
            self.panel.resize.width(window, fallback)
        } else {
            fallback
        }
    }
}

pub(super) struct RightPanel {
    pub(super) open: bool,
    pub(super) tab: PanelTab,
    compact_suspended: bool,
    inspected_track: Option<crate::playback::QueueEntry>,
    follow_playing_track: bool,
    tab_motion: Option<PanelTabMotion>,
    queue_list: ListState,
    queue_motion: std::rc::Rc<std::cell::RefCell<queue_motion::QueueMotion>>,
    track_scroll: ScrollHandle,
    inspector: track::InspectorTransition,
    resize: resize::Resize,
    dock: Dock,
    dock_clock: Instant,
    #[cfg(feature = "capture")]
    dock_sample: Option<Duration>,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    load: Option<Task<()>>,
    save: Option<Task<()>>,
    dirty: bool,
}

impl RightPanel {
    pub(super) fn new(cx: &mut Context<MainView>) -> Self {
        Self {
            open: false,
            tab: PanelTab::Queue,
            compact_suspended: false,
            inspected_track: None,
            follow_playing_track: false,
            tab_motion: None,
            queue_list: ListState::new(0, ListAlignment::Top, px(100.)),
            queue_motion: Default::default(),
            track_scroll: ScrollHandle::new(),
            inspector: track::InspectorTransition::default(),
            resize: resize::Resize::new(cx),
            dock: Dock::default(),
            dock_clock: Instant::now(),
            #[cfg(feature = "capture")]
            dock_sample: None,
            focus: cx.focus_handle(),
            return_focus: None,
            load: None,
            save: None,
            dirty: false,
        }
    }

    pub(super) fn update_queue_list(&self, count: usize) {
        let previous = self.queue_list.item_count();
        if count > previous {
            self.queue_list.splice(previous..previous, count - previous);
        } else if count < previous {
            self.queue_list.splice(count..previous, 0);
        }
        self.queue_list.remeasure();
    }

    pub(super) fn discard_return_focus(&mut self, removed: &FocusHandle) {
        if self.return_focus.as_ref() == Some(removed) {
            self.return_focus = None;
        }
    }

    fn state(&self, autoplay: bool) -> PanelState {
        PanelState {
            open: self.open,
            tab: self.tab,
            autoplay,
            width_dip: self.resize.preferred_width(),
        }
    }
}

impl MainView {
    /// Read the saved openness and tab once at startup.
    pub(super) fn load_saved_panel(&mut self, cx: &mut Context<Self>) {
        if self.panel.load.is_some() {
            return;
        }
        let task = cx.spawn(async move |view, cx| {
            let state = cx
                .background_executor()
                .spawn(async { Store::open_default().and_then(|store| store.panel_state()) })
                .await;
            let _ = view.update(cx, |view, cx| {
                view.panel.load = None;
                match state {
                    Ok(state) => {
                        if !view.panel.dirty {
                            view.panel.open = state.open;
                            view.panel.tab = state.tab;
                        }
                        view.panel.resize.restore_width(state.width_dip);
                        view.autoplay.restore(state.autoplay);
                        view.autoplay_player_changed(cx);
                        view.sync_panel_buttons(cx);
                        cx.notify();
                    }
                    Err(error) => {
                        eprintln!("Не удалось прочитать состояние панели Mediadeka: {error}");
                    }
                }
                if view.panel.dirty {
                    view.persist_panel(cx);
                }
            });
        });
        self.panel.load = Some(task);
    }

    pub(super) fn panel_is_open_on_queue(&self) -> bool {
        self.panel.open && self.panel.tab == PanelTab::Queue
    }

    pub(super) fn panel_replaces_page(&self, window: &Window) -> bool {
        self.panel.open
            && !inline(window)
            && !self.panel.compact_suspended
            && (self.uses_fluid_glass() || self.panel_is_open_on_queue())
    }

    pub(super) fn suspend_panel_for_entity(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.panel_replaces_page(window) {
            self.panel.compact_suspended = true;
            self.panel.return_focus = None;
            self.focus_workspace_page();
            cx.notify();
        }
    }

    pub(super) fn remember_inspector_subject(&mut self) {
        if self.panel.open
            && self.panel.tab == PanelTab::Track
            && !self.panel.follow_playing_track
            && let Some((key, revision, index, track)) = self.selected_playlist_track()
        {
            self.panel.inspected_track = Some(crate::playback::QueueEntry {
                playlist: key.clone(),
                playlist_revision: revision,
                index,
                track: track.clone(),
                recommendations: Vec::new(),
            });
        }
    }

    pub(super) fn clear_inspector_subject(&mut self) {
        self.panel.inspected_track = None;
    }

    pub(in crate::ui) fn panel_follows_playing_track(&self) -> bool {
        self.panel.follow_playing_track
    }

    fn panel_shows_playing_track(&self) -> bool {
        self.panel.open
            && self.panel.tab == PanelTab::Track
            && (self.panel.follow_playing_track
                || self.panel.inspected_track.is_none() && self.selected_playlist_track().is_none())
    }

    pub(super) fn leave_panel_page(&mut self, cx: &mut Context<Self>) {
        self.panel.return_focus = None;
        self.set_panel_open(false, cx);
        self.sync_panel_buttons(cx);
    }

    pub(super) fn panel_contains_focus(&self, window: &Window, cx: &Context<Self>) -> bool {
        self.panel.open && self.panel.focus.contains_focused(window, cx)
    }

    pub(super) fn focus_panel(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.panel.focus, cx);
    }

    pub(super) fn toggle_queue_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle_queue_panel_with_return_focus(None, window, cx);
    }

    pub(super) fn toggle_queue_panel_from_home(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.toggle_queue_panel_with_return_focus(window.focused(cx), window, cx);
    }

    fn toggle_queue_panel_with_return_focus(
        &mut self,
        return_focus: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let leaving_listening = self.listening.mode().is_some();
        if leaving_listening {
            self.set_listening_mode(None, window, cx);
        }
        if self.panel_is_open_on_queue()
            && (inline(window) || !self.panel.compact_suspended)
            && !leaving_listening
        {
            self.close_panel(window, cx);
        } else {
            self.dismiss_playlist_overlay(window, cx);
            self.open_panel(PanelTab::Queue, true, window, cx);
            self.panel.return_focus =
                Some(return_focus.unwrap_or_else(|| self.player.read(cx).queue_focus().clone()));
        }
    }

    pub(super) fn toggle_lyrics_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.lyrics
            .update(cx, |lyrics, cx| lyrics.set_inspection(None, cx));
        if let Some(mode) = self.listening.mode() {
            use super::listening::ListeningMode::{Cover, Lyrics};
            self.set_listening_mode(
                Some(if mode == Lyrics { Cover } else { Lyrics }),
                window,
                cx,
            );
        } else if self.panel.open
            && self.panel.tab == PanelTab::Lyrics
            && (inline(window) || !self.panel.compact_suspended)
        {
            self.close_panel(window, cx);
        } else {
            self.open_panel(PanelTab::Lyrics, true, window, cx);
        }
    }

    pub(super) fn toggle_playing_track_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let leaving_listening = self.listening.mode().is_some();
        if leaving_listening {
            self.set_listening_mode(None, window, cx);
        }
        if !leaving_listening
            && self.panel_shows_playing_track()
            && (inline(window) || !self.panel.compact_suspended)
        {
            self.close_panel(window, cx);
        } else {
            self.dismiss_playlist_overlay(window, cx);
            self.panel.follow_playing_track = true;
            self.panel.inspector = track::InspectorTransition::default();
            self.open_panel(PanelTab::Track, true, window, cx);
        }
    }

    pub(in crate::ui) fn show_track_lyrics(
        &mut self,
        entry: crate::playback::QueueEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.lyrics
            .update(cx, |lyrics, cx| lyrics.set_inspection(Some(entry), cx));
        self.open_panel(PanelTab::Lyrics, true, window, cx);
        self.panel.return_focus = Some(self.music.track_lyrics_focus.clone());
    }

    pub(in crate::ui) fn open_panel(
        &mut self,
        tab: PanelTab,
        move_focus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if move_focus {
            self.panel.return_focus = window.focused(cx);
        }
        self.set_panel_tab(tab, cx);
        self.set_panel_open(true, cx);
        if move_focus {
            self.focus_panel(window, cx);
        }
        self.sync_panel_buttons(cx);
        cx.notify();
    }

    pub(super) fn close_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.panel.open {
            return false;
        }
        let panel_had_focus = self.panel_contains_focus(window, cx);
        self.set_panel_open(false, cx);
        if let Some(focus) = self.panel.return_focus.take() {
            window.focus(&focus, cx);
        } else if panel_had_focus {
            window.focus(&self.focus, cx);
        }
        self.sync_panel_buttons(cx);
        cx.notify();
        true
    }

    /// A selected playlist row appears in the track tab. A wide window also
    /// opens the panel; a narrow one leaves the page in place.
    pub(super) fn reveal_track_tab(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.panel.open && self.panel.tab == PanelTab::Lyrics {
            return;
        }
        self.set_panel_tab(PanelTab::Track, cx);
        if !self.panel.open && inline(window) {
            self.set_panel_open(true, cx);
        }
        self.sync_panel_buttons(cx);
        cx.notify();
    }

    fn set_panel_tab(&mut self, tab: PanelTab, cx: &mut Context<Self>) {
        if self.panel.tab != tab {
            if tab != PanelTab::Track {
                self.panel.inspector = track::InspectorTransition::default();
            }
            let now = Instant::now();
            let from = self.panel.tab_motion.map_or_else(
                || tab_slot(self.panel.tab),
                |motion| motion.position(self.panel.tab, now),
            );
            self.panel.tab_motion = Some(PanelTabMotion { from, started: now });
            self.panel.tab = tab;
            self.persist_panel(cx);
        }
    }

    fn set_panel_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.panel.compact_suspended = false;
        if !open {
            self.clear_inspector_subject();
            self.panel.follow_playing_track = false;
            self.panel.tab_motion = None;
            self.lyrics
                .update(cx, |lyrics, cx| lyrics.set_inspection(None, cx));
        }
        if self.panel.open != open {
            self.panel.open = open;
            if !open {
                self.panel.resize.stop_drag();
                self.panel.inspector = track::InspectorTransition::default();
            }
            self.persist_panel(cx);
        }
    }

    pub(super) fn persist_panel(&mut self, cx: &mut Context<Self>) {
        self.panel.dirty = true;
        if self.panel.load.is_some() || self.panel.save.is_some() {
            return;
        }
        let task = cx.spawn(async move |view, cx| {
            loop {
                let state = match view.update(cx, |view, _| {
                    if !view.panel.dirty {
                        return None;
                    }
                    view.panel.dirty = false;
                    Some(view.panel.state(view.autoplay.enabled()))
                }) {
                    Ok(Some(state)) => state,
                    _ => break,
                };
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        Store::open_default().and_then(|store| store.save_panel_state(&state))
                    })
                    .await;
                if let Err(error) = result {
                    eprintln!("Не удалось сохранить состояние панели Mediadeka: {error}");
                }
            }
            let _ = view.update(cx, |view, _| view.panel.save = None);
        });
        self.panel.save = Some(task);
    }

    pub(super) fn sync_panel_buttons(&mut self, cx: &mut Context<Self>) {
        let immersive = self.listening.mode();
        let queue = immersive.is_none() && self.panel_is_open_on_queue();
        let inspector = immersive.is_none() && self.panel_shows_playing_track();
        let panel_lyrics = self.panel.open && self.panel.tab == PanelTab::Lyrics;
        let lyrics_active = match immersive {
            Some(mode) => mode == super::listening::ListeningMode::Lyrics,
            None => panel_lyrics,
        };
        self.player.update(cx, |bar, cx| {
            bar.set_panel_activity(queue, lyrics_active, cx);
            bar.set_inspector_active(inspector, cx);
        });
        let lyrics_visible = self.listening.lyrics_requested() || panel_lyrics;
        self.lyrics.update(cx, |view, cx| {
            view.set_visibility(lyrics_visible, lyrics_active, cx)
        });
    }

    pub(super) fn right_panel(&self, window: &Window, cx: &Context<Self>) -> Option<AnyElement> {
        let inline = inline(window);
        let glass = self.uses_fluid_glass();
        let dock_width = self.docked_panel_width(window);
        let dock_wrapper = || {
            div()
                .relative()
                .h_full()
                .w(px(dock_width))
                .flex_none()
                .overflow_hidden()
        };
        if !self.panel.open || (!inline && self.panel.compact_suspended) {
            return (glass && dock_width > 0.).then(|| dock_wrapper().into_any_element());
        }
        let page_panel = self.panel_replaces_page(window);
        let on_playlist = self.music_page() == Page::Playlist;
        let animate = !self.reduced_motion && !cx.reduce_motion() && !self.fluid_high_contrast(cx);
        let now = Instant::now();
        let motion = self.panel.tab_motion.filter(|motion| {
            animate && now.saturating_duration_since(motion.started) < MOTION.normal()
        });
        let tab_position = motion.map_or_else(
            || tab_slot(self.panel.tab),
            |motion| motion.position(self.panel.tab, now),
        );
        let body_opacity = motion.map_or(1., |motion| {
            let progress = now.saturating_duration_since(motion.started).as_secs_f32()
                / MOTION.fast().as_secs_f32();
            0.90 + 0.10 * out_quad(progress)
        });
        if motion.is_some() && window.is_visible() && window.is_window_active() {
            window.request_animation_frame();
        }
        if glass && !inline {
            self.reset_unavailable_panel_resize(window, cx);
        }
        let body = match self.panel.tab {
            PanelTab::Queue => self.queue_section(cx),
            PanelTab::Track => self.track_section(window, cx),
            PanelTab::Lyrics => self.lyrics.clone().into_any_element(),
        };
        // A short crossfade on the tab change; the shell itself does not move.
        let body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .opacity(body_opacity)
            .child(body);
        let panel = div()
            .id("right-panel")
            .accessibility_id("right-panel")
            .role(Role::Group)
            .aria_label(if page_panel && !glass {
                "Очередь и сведения о треке"
            } else {
                "Правая панель"
            })
            .track_focus(&self.panel.focus)
            .tab_index(TAB_GROUP)
            .tab_group()
            .tab_stop(false)
            .flex_none()
            .when(page_panel, |panel| panel.w_full())
            .when(page_panel && glass, |panel| {
                panel.min_w_0().overflow_hidden()
            })
            .when(!page_panel, |panel| {
                panel.w(px(self.right_panel_width(window)))
            })
            .h_full()
            .flex()
            .flex_col()
            .bg(rgb(SURFACE))
            .border_color(surface_border())
            .when(on_playlist, |panel| {
                panel
                    .bg(rgb(playlist::PANEL))
                    .border_color(rgb(playlist::RULE))
            })
            .when(glass, |panel| {
                panel
                    .bg(gpui::transparent_black())
                    .border_color(rgb(tokens::RULE))
            })
            .when(!page_panel && !glass, |panel| panel.border_l_1())
            .when(!inline && !page_panel, |panel| {
                panel
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .occlude()
                    .shadow_lg()
            })
            .child(self.panel_header(tab_position, cx))
            .child(body)
            .when(glass && inline, |panel| {
                panel.relative().child(self.resize_handle(window, cx))
            });
        if glass && inline {
            let content_alpha =
                out_quad((dock_width - DOCK_MIN_CONTENT_WIDTH) / DOCK_CONTENT_REVEAL_WIDTH);
            return Some(
                dock_wrapper()
                    .child(
                        panel
                            .absolute()
                            .top_0()
                            .right_0()
                            .w(px(dock_width.max(DOCK_MIN_CONTENT_WIDTH)))
                            .opacity(content_alpha),
                    )
                    .into_any_element(),
            );
        }
        let panel = if !animate || on_playlist || page_panel {
            panel.into_any_element()
        } else {
            panel
                .with_animation(
                    "right-panel-entry",
                    Animation::new(MOTION.fast()),
                    |panel, value| panel.opacity(0.62 + 0.38 * value),
                )
                .into_any_element()
        };
        Some(panel)
    }

    fn panel_header(&self, tab_position: f32, cx: &Context<Self>) -> Div {
        let glass = self.uses_fluid_glass();
        let on_playlist = !glass && self.music_page() == Page::Playlist;
        div()
            .flex_none()
            .flex()
            .items_center()
            .when(on_playlist, |header| {
                header
                    .h(px(metrics::HEADER))
                    .gap(px(metrics::GAP))
                    .px(px(metrics::GAP))
                    .border_b_1()
                    .border_color(rgb(playlist::RULE))
            })
            .when(glass, |header| {
                let divider = if self.fluid_high_contrast(cx) {
                    gpui::Hsla::from(rgb(tokens::contrast::RULE))
                } else {
                    gpui::Hsla::from(rgb(tokens::RULE)).alpha(0.55)
                };
                header
                    .h(px(size::LG + space::XS))
                    .gap(px(space::SM))
                    .px(px(space::MD))
                    .justify_between()
                    .border_b_1()
                    .border_color(divider)
            })
            .when(!glass && !on_playlist, |header| {
                header.gap(px(6.)).px(px(10.)).pt(px(10.)).pb(px(6.))
            })
            .child(
                div()
                    .id("right-panel-tabs")
                    .accessibility_id("right-panel-tabs")
                    .role(Role::TabList)
                    .aria_label("Разделы панели")
                    .relative()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .when(glass, |tabs| {
                        tabs.h(px(size::SM)).max_w(px(FLUID_TAB_WIDTH * TABS))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.panel_tab(
                                PanelTab::Queue,
                                "right-panel-queue",
                                "Очередь",
                                591,
                                cx,
                            ))
                            .child(self.panel_tab(
                                PanelTab::Track,
                                "right-panel-track",
                                "Трек",
                                592,
                                cx,
                            ))
                            .child(self.panel_tab(
                                PanelTab::Lyrics,
                                "right-panel-lyrics",
                                "Текст",
                                593,
                                cx,
                            )),
                    )
                    .child(self.panel_underline(tab_position, cx)),
            )
            .child(self.panel_close(on_playlist, cx))
    }

    /// One underline for the whole tab row: it travels between the labels
    /// instead of every tab carrying its own container or bar.
    fn panel_underline(&self, tab_position: f32, cx: &Context<Self>) -> Div {
        let slot = div()
            .absolute()
            .bottom_0()
            .left(relative(tab_position / TABS))
            .w(relative(1. / TABS));
        if let Some(material) = self.glass_controls(cx) {
            slot.bottom(px(1.))
                .flex()
                .justify_center()
                .child(tab_selection_marker(material))
        } else {
            slot.px(px(space::MD)).child(
                div()
                    .h(px(2.))
                    .rounded_full()
                    .w_full()
                    .bg(materials::accent(cx)),
            )
        }
    }

    fn panel_close(&self, on_playlist: bool, cx: &Context<Self>) -> AnyElement {
        let close = cx.listener(|view, _event: &(), window, cx| {
            view.close_panel(window, cx);
        });
        if let Some(material) = self.glass_controls(cx) {
            panel_icon_action(
                "right-panel-close",
                "Закрыть панель",
                IconName::X,
                594,
                material,
                true,
            )
            .tooltip(guise::tooltip("Закрыть панель · Esc"))
            .on_activate(close)
            .into_any_element()
        } else {
            control(
                "right-panel-close",
                "Закрыть панель",
                IconName::X,
                594,
                true,
                false,
                false,
            )
            .when(on_playlist, |close| {
                close
                    .pixel_size(px(metrics::CONTROL_SMALL))
                    .variant(Variant::Subtle)
                    .color(gpui::Hsla::from(rgb(playlist::INK_SECONDARY)))
            })
            .on_click(move |_, window, cx| close(&(), window, cx))
            .into_any_element()
        }
    }

    fn panel_tab(
        &self,
        tab: PanelTab,
        id: &'static str,
        label: &'static str,
        order: isize,
        cx: &Context<Self>,
    ) -> AnyElement {
        let selected = self.panel.tab == tab;
        let on_playlist = self.music_page() == Page::Playlist;
        let material = self.glass_controls(cx);
        if material.is_some() || on_playlist {
            let tab_button = button(id, label, order)
                .role(Role::Tab)
                .aria_selected(selected)
                .flex_1()
                .min_w_0()
                .h_full()
                .justify_center()
                .text_size(px(text::METADATA))
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                });
            let tab_button = if let Some(material) = material {
                paint_page_tab(tab_button, material, selected)
            } else {
                tab_button
                    .text_color(rgb(if selected {
                        playlist::SLEEVE_LABEL
                    } else {
                        playlist::INK_SECONDARY
                    }))
                    .hover(|style| style.text_color(rgb(playlist::SLEEVE_LABEL)))
            };
            tab_button
                .child(label)
                .on_activate(cx.listener(move |view, _, _, cx| view.activate_panel_tab(tab, cx)))
                .into_any_element()
        } else {
            Chip::new(SharedString::from(id), label)
                .as_tab()
                .checked(selected)
                .size(Size::Sm)
                .tab_order(order)
                .on_change(cx.listener(move |view, _, _, cx| view.activate_panel_tab(tab, cx)))
                .into_any_element()
        }
    }

    fn activate_panel_tab(&mut self, tab: PanelTab, cx: &mut Context<Self>) {
        if self.panel.tab != tab {
            self.set_panel_tab(tab, cx);
            self.sync_panel_buttons(cx);
            cx.notify();
        }
    }
}

pub(in crate::ui) use track::playlist_track_artwork_span;

#[cfg(feature = "capture")]
impl MainView {
    pub fn show_panel_for_capture(
        &mut self,
        tab: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = match tab {
            "queue" => PanelTab::Queue,
            "track" => PanelTab::Track,
            "lyrics" => PanelTab::Lyrics,
            _ => return,
        };
        self.open_panel(tab, false, window, cx);
    }

    pub fn panel_load_pending_for_capture(&self) -> bool {
        self.panel.load.is_some()
    }

    pub fn panel_state_for_capture(&self) -> (bool, &'static str) {
        (
            self.panel.open,
            match self.panel.tab {
                PanelTab::Queue => "queue",
                PanelTab::Track => "track",
                PanelTab::Lyrics => "lyrics",
            },
        )
    }
}
