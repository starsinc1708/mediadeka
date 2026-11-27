mod accounts;
mod content_timing;
mod fluid;
mod navigation;
pub(super) mod pins;
mod routes;
mod scrolling;
mod search;
mod startup;
mod toolbar;

use std::cell::Cell;

use gpui::{
    AnyElement, Context, Div, FocusHandle, IntoElement, Pixels, Role, SpanId, Window, div, point,
    prelude::*, px, rgb,
};

use crate::state::{HomeContent, HomeView, Intent, Screen};

use super::{
    MainView,
    components::scrollbar::scroll_offset,
    design_system::adaptive_layout::{AdaptiveContent, LayoutBand},
    fluid_glass::ShellGeometry,
    music::detail::hero_motion::{HERO_ARTWORK_SPAN, HeroMotion},
    theme::{WINDOW, playlist::metrics},
};
pub(super) use content_timing::ContentSample;
use navigation::sidebar_width;
use routes::Routes;
pub(super) use routes::{
    LibraryLocation, LibraryScroll, LibraryTab, LocalTrackTarget, MusicTarget, Page, PageState,
    PlaylistLocation, Route, Target,
};

const RAIL_EXPAND_START: f32 = 1040.;
const RAIL_EXPAND_END: f32 = 1230.;
const RAIL_LABEL_WIDTH: f32 = 164.;
const TOOLBAR_HEIGHT: f32 = metrics::CONTROL + 2. * metrics::GAP;

#[derive(Clone, Copy)]
pub(super) struct WorkspaceLayout {
    pub(super) compact: bool,
    sidebar_width: f32,
    margin: f32,
    pub(super) content_width: f32,
    pub(super) width: f32,
}

impl WorkspaceLayout {
    pub(super) fn new(width: f32) -> Self {
        // The rail and both margins grow more slowly than the available width.
        let progress =
            ((width - RAIL_EXPAND_START) / (RAIL_EXPAND_END - RAIL_EXPAND_START)).clamp(0., 1.);
        let sidebar_width = sidebar_width(progress);
        let margin = 17. + 12. * progress;
        Self {
            compact: sidebar_width < RAIL_LABEL_WIDTH,
            sidebar_width,
            margin,
            width,
            content_width: width - sidebar_width - margin * 2.,
        }
    }
}

pub(super) struct Workspace {
    pub(super) routes: Routes,
    pub(in crate::ui) search: search::SearchBar,
    scrolling: scrolling::WorkspaceScrolling,
    nav_at: Page,
    nav_motion: Option<navigation::SidebarMotion>,
    nav_layout: Cell<navigation::SidebarLayout>,
    nav_visible: Cell<Option<f32>>,
    transitions: fluid::page_motion::TransitionCoordinator,
    content_band: Cell<Option<LayoutBand>>,
    mounted: bool,
    page_focus: FocusHandle,
    pending_focus: Option<FocusHandle>,
    /// A restored scroll position waits here until its page has data.
    pending_scroll: Option<Pixels>,
    startup: startup::Startup,
    content_timing: content_timing::ContentTiming,
}

impl Workspace {
    pub(super) fn new(window: &Window, cx: &mut Context<MainView>, fluid: bool) -> Self {
        Self {
            routes: if fluid {
                Routes::starting_at(Route::Library(LibraryTab::Tracks))
            } else {
                Routes::default()
            },
            search: search::SearchBar::new(window, fluid, cx),
            scrolling: scrolling::WorkspaceScrolling::default(),
            nav_at: Page::Home,
            nav_motion: None,
            nav_layout: Cell::new(navigation::SidebarLayout::Classic),
            nav_visible: Cell::new(None),
            transitions: fluid::page_motion::TransitionCoordinator::new(fluid),
            content_band: Cell::new(None),
            mounted: false,
            page_focus: cx.focus_handle(),
            pending_focus: None,
            pending_scroll: None,
            startup: startup::Startup::default(),
            content_timing: content_timing::ContentTiming::default(),
        }
    }

    pub(super) fn on_settings(&self) -> bool {
        self.routes.current().page() == Page::Settings
    }

    pub(super) fn take_pending_scroll(&mut self) -> Option<Pixels> {
        self.pending_scroll.take()
    }
}

impl MainView {
    pub(super) fn restore_workspace_focus(
        &mut self,
        visible: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let was_visible = std::mem::replace(&mut self.workspace.mounted, visible);
        let pending = self.workspace.pending_focus.take();
        if !visible || self.is_settings_login() || self.soloist.is_open() {
            return;
        }
        let panel_replaces_page = self.panel_replaces_page(window);
        if panel_replaces_page {
            if !was_visible || self.workspace.page_focus.contains_focused(window, cx) {
                self.focus_panel(window, cx);
            }
        } else if let Some(focus) = pending {
            window.focus(&focus, cx);
        } else if !was_visible {
            window.focus(&self.workspace.page_focus, cx);
        }
    }

    #[cfg(feature = "capture")]
    pub fn workspace_layout_for_capture(&self, window: &Window) -> (f32, f32, f32) {
        let layout = self.workspace_layout(window);
        (
            layout.sidebar_width,
            layout.content_width,
            f32::from(self.scroll.bounds().size.width),
        )
    }

    #[cfg(feature = "capture")]
    pub fn sidebar_motion_for_capture(&self) -> (Option<f32>, Option<f32>) {
        (
            self.workspace.nav_visible.get(),
            self.workspace.nav_motion.as_ref().map(|motion| motion.from),
        )
    }

    #[cfg(feature = "capture")]
    pub fn show_settings_for_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_music_page(Page::Settings, window, cx);
    }

    #[cfg(feature = "capture")]
    pub fn is_settings_for_capture(&self) -> bool {
        self.workspace.on_settings()
    }

    pub(super) fn focus_global_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.soloist.is_open() || self.is_settings_login() || self.state.home_view().is_none() {
            return;
        }
        self.workspace.search.focus(window, cx);
        self.note_first_workspace_action();
    }

    pub(super) fn focus_page_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.soloist.is_open() || self.is_settings_login() {
            return;
        }
        if self.panel_replaces_page(window) {
            self.close_panel(window, cx);
        }
        match self.music_page() {
            Page::Playlist => self.focus_playlist_filter(window, cx),
            page if page.library_tab().is_some()
                && self.state.home_view().is_some()
                && self.reveal_library_filter() =>
            {
                self.music
                    .filter
                    .update(cx, |field, cx| field.focus(window, cx));
            }
            _ => {}
        }
    }

    pub(super) fn prepare_library_view(&mut self, window: &Window) {
        if self.music_page().library_tab().is_some() {
            let layout = self.workspace_layout(window);
            let width = if self.uses_fluid_glass() {
                self.adaptive_content(window).width
            } else {
                layout.content_width - self.docked_panel_width(window) - 2. * layout.margin
            };
            self.set_library_layout(width);
        }
    }

    fn workspace_layout(&self, window: &Window) -> WorkspaceLayout {
        if self.uses_fluid_glass() {
            WorkspaceLayout::fluid(window)
        } else {
            WorkspaceLayout::new(f32::from(window.viewport_size().width))
        }
    }

    pub(in crate::ui) fn adaptive_content(&self, window: &Window) -> AdaptiveContent {
        let layout = self.workspace_layout(window);
        let width = layout.content_width - self.docked_panel_width(window) - 2. * layout.margin;
        let content = AdaptiveContent::resolve(width, self.workspace.content_band.get());
        self.workspace.content_band.set(Some(content.band));
        content
    }

    #[cfg(feature = "capture")]
    pub fn adaptive_content_for_capture(&self, window: &Window) -> (f32, String) {
        let content = self.adaptive_content(window);
        (content.width, format!("{:?}", content.band))
    }

    pub(super) fn music_workspace(
        &self,
        home: Option<&HomeView>,
        refreshing: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let layout = self.workspace_layout(window);
        if self.uses_fluid_glass() {
            return self
                .fluid_workspace(home, refreshing, layout, window, cx)
                .into_any_element();
        }
        let panel_replaces_page = self.panel_replaces_page(window);
        div()
            .size_full()
            .relative()
            .flex()
            .bg(rgb(WINDOW))
            .child(self.music_navigation(home, layout, window, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.music_toolbar(layout, window, cx))
                    .child(self.workspace_body(home, refreshing, layout, window, cx))
                    .child(div().w_full().flex_none().child(self.player.clone())),
            )
            .when(
                !panel_replaces_page || self.now_playing_menu_open(),
                |workspace| workspace.children(self.playlist_overlay(window, cx)),
            )
            .into_any_element()
    }

    fn workspace_body(
        &self,
        home: Option<&HomeView>,
        refreshing: bool,
        layout: WorkspaceLayout,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let body = div()
            .relative()
            .w_full()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex();
        let panel = self.right_panel(window, cx);
        let page = self.music_page();
        let visibility = (matches!(
            page,
            Page::Home | Page::History | Page::Playlist | Page::Search | Page::Object
        ) || page.library_tab().is_some())
        .then(|| self.flush_prefetch_visibility(cx));
        if self.panel_replaces_page(window) {
            return body.children(panel).children(visibility);
        }
        let margin = layout.margin;
        let content_width = layout.content_width - self.docked_panel_width(window);
        let startup_loading = matches!(self.state.screen(), Screen::Pending(Intent::Startup))
            && home.is_some_and(|home| matches!(home.content, HomeContent::Loading));
        let content = match page {
            Page::Playlist => self
                .playlist_viewport(content_width, layout.content_width, margin, window, cx)
                .into_any_element(),
            page if page.library_tab().is_some() => self
                .library_view(
                    margin,
                    content_width,
                    !self.uses_fluid_glass() || !self.has_full_fluid_navigation(layout, window),
                    window,
                    cx,
                )
                .into_any_element(),
            Page::History => self.history_view(margin, window, cx).into_any_element(),
            _ => self
                .music_viewport(
                    home.filter(|_| page != Page::Home || !startup_loading),
                    layout,
                    refreshing,
                    window,
                    cx,
                )
                .into_any_element(),
        };
        let (page_offset, opacity) = self
            .workspace
            .transitions
            .presentation(window, self.reduced_motion || self.fluid_high_contrast(cx));
        body.child(
            div()
                .id("workspace-content")
                .accessibility_id("workspace-content")
                .role(Role::Group)
                .aria_label("Рабочая область")
                .track_focus(&self.workspace.page_focus)
                .tab_stop(false)
                .relative()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .flex()
                .flex_col()
                .when(page_offset > Pixels::ZERO, |page| {
                    page.top(page_offset).pixel_snap(false).opacity(opacity)
                })
                .child(content),
        )
        .children(panel)
        .children(visibility)
    }

    fn music_viewport(
        &self,
        home: Option<&HomeView>,
        layout: WorkspaceLayout,
        refreshing: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let page = self.music_page();
        let short = window.viewport_size().height < px(560.);
        let adaptive = self
            .uses_fluid_glass()
            .then(|| self.adaptive_content(window));
        let width = adaptive.map_or_else(
            || layout.content_width - self.docked_panel_width(window),
            |content| content.width,
        );
        let compact = adaptive.map_or(layout.compact, AdaptiveContent::compact);
        let shell = ShellGeometry::for_workspace(window, self.docked_panel_width(window));
        let bottom_padding = shell.scroll_content_padding();
        let hero = (page == Page::Object)
            .then(|| self.object_hero_scroll(width, compact || short, cx))
            .flatten();
        let header_height = hero.map_or(Pixels::ZERO, |hero| {
            hero.height() + self.object_hero_columns_height()
        });
        let content = match (page, home) {
            (Page::Object, _) => self
                .object_view(compact, width, short, hero, cx)
                .into_any_element(),
            (Page::Station, _) => self.station_view(cx),
            (Page::Search, _) => self.catalog_search_view(compact, cx).into_any_element(),
            (Page::Settings, Some(home)) => self
                .connections_settings(home, refreshing, compact, window, cx)
                .into_any_element(),
            _ => self
                .home_feed(home, layout, window, refreshing, cx)
                .into_any_element(),
        };
        let scroll_content = div()
            .flex()
            .flex_col()
            .px(px(layout.margin))
            .when(hero.is_none(), |body| {
                body.pt(px(if layout.compact { 8. } else { 18. }))
            })
            .pb(px(
                (if layout.compact { 24.0_f32 } else { 44.0 }).max(bottom_padding)
            ))
            .child(
                div()
                    .w_full()
                    .when_some(
                        adaptive.filter(|_| page == Page::Search),
                        |body, geometry| body.max_w(px(geometry.reading_width())),
                    )
                    .child(content),
            );
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(self.workspace_scrolling(scroll_content, header_height, window, cx))
            .when_some(hero, |viewport, hero| {
                viewport
                    .children(self.object_collapsing_header(
                        hero,
                        width + 2. * layout.margin,
                        layout.margin,
                        cx,
                    ))
                    .children(self.object_hero_columns(width, cx).map(|columns| {
                        columns
                            .absolute()
                            .top(hero.height())
                            .left(px(layout.margin))
                            .right(px(layout.margin))
                            .max_w(px(AdaptiveContent::resolve(width, None).table_width()))
                    }))
            })
    }

    #[cfg(feature = "capture")]
    pub(super) fn soundcloud_active(&self) -> bool {
        self.state.home_view().is_some_and(|(view, _)| {
            matches!(
                view.session,
                crate::state::ActiveSession::Account {
                    provider: crate::providers::ProviderId::SoundCloud,
                    ..
                }
            )
        })
    }

    pub(super) fn music_page(&self) -> Page {
        self.workspace.routes.current().page()
    }

    /// Record the state of the page being left in its history entry.
    pub(in crate::ui) fn remember_page_state(&mut self, cx: &Context<Self>) {
        let state = match self.workspace.routes.current().page() {
            Page::Tracks | Page::Albums | Page::Artists | Page::Playlists => {
                PageState::Library(self.library_location(cx))
            }
            Page::History => PageState::History(self.history_position()),
            Page::Playlist => PageState::Playlist(self.playlist.location(cx)),
            Page::Home | Page::Search | Page::Object | Page::Station => {
                PageState::Scroll(scroll_offset(&self.scroll))
            }
            Page::Settings => return,
        };
        self.workspace.routes.remember(state);
    }

    pub(super) fn show_music_page(&mut self, page: Page, window: &Window, cx: &mut Context<Self>) {
        if self.is_settings_login() || self.disconnecting() {
            return;
        }
        let route = match page {
            Page::Home => Route::Home,
            Page::Search => Route::Search {
                kind: match self.workspace.routes.current() {
                    Route::Search { kind, .. } => *kind,
                    _ => crate::music::ObjectKind::Track,
                },
                query: String::new(),
            },
            Page::History => Route::History,
            Page::Tracks => Route::Library(LibraryTab::Tracks),
            Page::Albums => Route::Library(LibraryTab::Albums),
            Page::Artists => Route::Library(LibraryTab::Artists),
            Page::Playlists => Route::Library(LibraryTab::Playlists),
            Page::Settings => Route::Settings,
            Page::Playlist | Page::Object | Page::Station => return,
        };
        self.navigate(route, window, cx);
    }

    pub(super) fn navigate(&mut self, route: Route, window: &Window, cx: &mut Context<Self>) {
        if self.workspace.startup.loading
            || self.workspace.routes.current().same_destination(&route)
        {
            return;
        }
        self.note_first_workspace_action();
        self.remember_page_state(cx);
        self.workspace.routes.push(route);
        self.show_route(window, cx);
    }

    pub(in crate::ui) fn navigate_from_artwork(
        &mut self,
        source: SpanId,
        window: &Window,
        cx: &mut Context<Self>,
        navigate: impl FnOnce(&mut Self, &Window, &mut Context<Self>),
    ) {
        let previous = self.workspace.routes.current().clone();
        navigate(self, window, cx);
        if self
            .workspace
            .routes
            .record_artwork_arrival(&previous, source)
        {
            self.begin_artwork_transition(source, HERO_ARTWORK_SPAN, window, cx);
        }
    }

    pub(super) fn step_workspace_route(
        &mut self,
        forward: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.workspace.routes.can_step(forward) {
            return;
        }
        let source = self.workspace.routes.artwork_origin_for_step(forward);
        self.note_first_workspace_action();
        self.remember_page_state(cx);
        self.workspace.routes.step(forward);
        self.show_route(window, cx);
        if window.last_input_was_keyboard() && !self.workspace.routes.can_step(forward) {
            self.focus_workspace_page();
        }
        if let Some(source) = source {
            let (from, to) = if forward {
                (source, HERO_ARTWORK_SPAN)
            } else {
                (HERO_ARTWORK_SPAN, source)
            };
            self.begin_artwork_transition(from, to, window, cx);
        }
    }

    fn begin_artwork_transition(
        &mut self,
        from: SpanId,
        to: SpanId,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if !self.reduced_motion
            && !self.fluid_high_contrast(cx)
            && self.workspace.transitions.permits_artwork(window, cx)
            && let Some(origin) = window.rendered_span(from)
        {
            self.workspace.transitions.artwork = Some(HeroMotion::start(origin, to));
            cx.notify();
        }
    }

    pub(in crate::ui) fn music_hero_motion(&self) -> Option<HeroMotion> {
        self.workspace.transitions.artwork
    }

    #[cfg(feature = "capture")]
    pub fn sample_hero_motion_for_capture(
        &mut self,
        elapsed: std::time::Duration,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Bounds<Pixels>> {
        let origin = self.workspace.transitions.artwork.as_mut()?.sample(elapsed);
        cx.notify();
        Some(origin)
    }

    #[cfg(feature = "capture")]
    pub fn rendered_hero_artwork_for_capture(
        &self,
        window: &Window,
    ) -> Option<gpui::Bounds<Pixels>> {
        window.rendered_span(HERO_ARTWORK_SPAN)
    }

    pub(super) fn show_route(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.remember_inspector_subject();
        self.panel
            .discard_return_focus(&self.music.track_lyrics_focus);
        let leaving_panel = self.panel_replaces_page(window);
        if leaving_panel
            || self.workspace.page_focus.contains_focused(window, cx)
            || window.focused(cx).is_none()
        {
            self.focus_workspace_page();
        }
        if leaving_panel {
            self.leave_panel_page(cx);
        }
        self.show_route_with(
            self.reduced_motion || (window.last_input_was_keyboard() && !self.uses_fluid_glass()),
            cx,
        );
    }

    /// Route changes that happen after a background operation reopen the
    /// current route without keyboard-driven navigation motion.
    pub(in crate::ui) fn show_current_route(&mut self, cx: &mut Context<Self>) {
        self.focus_workspace_page();
        self.show_route_with(self.reduced_motion, cx);
    }

    fn request_workspace_focus(&mut self, focus: FocusHandle) {
        self.workspace.pending_focus = Some(focus);
    }

    pub(super) fn focus_workspace_page(&mut self) {
        self.request_workspace_focus(self.workspace.page_focus.clone());
    }

    pub(in crate::ui) fn show_library_tab(
        &mut self,
        tab: LibraryTab,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.show_music_page(tab.page(), window, cx);
        if self.music_page() == tab.page() {
            self.request_workspace_focus(self.library_tab_focus(tab).clone());
            cx.notify();
        }
    }

    fn show_route_with(&mut self, immediate: bool, cx: &mut Context<Self>) {
        self.cancel_fluid_row_launch(cx);
        self.persist_workspace_route(cx);
        let page = self.music_page();
        self.player.update(cx, |player, cx| {
            player.set_playlist_presentation(page == Page::Playlist, cx)
        });
        let high_contrast = self.fluid_high_contrast(cx);
        self.workspace.select_page(page, immediate, high_contrast);
        self.stop_workspace_scrolling();
        self.home.prepare_page(page);
        if page == Page::Home {
            self.load_history(cx);
            self.sync_home_sources(cx);
        }
        if page == Page::Settings {
            self.load_sound_download_settings(cx);
            self.load_genius_settings(cx);
        } else {
            self.clear_integration_secrets(cx);
        }
        let route = self.workspace.routes.current().clone();
        self.station_route_changed(&route);
        self.workspace.search.show_route(&route, cx);
        self.restore_scroll_state();
        self.open_playlist_route(cx);
        self.open_music_route(cx);
        if self.music_page() == Page::History {
            self.open_history_route(cx);
        }
        cx.notify();
    }

    fn restore_scroll_state(&mut self) {
        let offset = match self.workspace.routes.state() {
            PageState::Scroll(offset) => *offset,
            _ => Pixels::ZERO,
        };
        // The new page starts at the top until its own position can be applied.
        self.workspace.pending_scroll = (offset > Pixels::ZERO).then_some(offset);
        self.scroll.set_offset(point(px(0.), px(0.)));
    }

    /// Apply a restored scroll position once the page that owns it has data.
    pub(super) fn settle_pending_scroll(&mut self) {
        if self.workspace.pending_scroll.is_none() || !self.route_content_ready() {
            return;
        }
        if let Some(offset) = self.workspace.take_pending_scroll() {
            self.scroll
                .set_offset(point(self.scroll.offset().x, -offset));
        }
    }

    fn route_content_ready(&self) -> bool {
        match self.music_page() {
            Page::Home => self
                .state
                .home_view()
                .is_some_and(|(view, _)| !matches!(view.content, HomeContent::Loading)),
            Page::History => !self.history_loading(),
            Page::Search => !self.music_request_pending(),
            Page::Object => self.music_object_ready(),
            _ => true,
        }
    }

    pub(super) fn show_playlist(
        &mut self,
        target: Target,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.navigate(Route::Playlist(target), window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanding_the_rail_never_steals_content_width() {
        for width in 800..=1500 {
            let at_width = WorkspaceLayout::new(width as f32);
            let next_width = WorkspaceLayout::new(width as f32 + 1.);
            assert!(next_width.content_width >= at_width.content_width);
        }
    }

    #[test]
    fn workspace_navigation_has_compact_and_expanded_widths() {
        for (width, expected, compact) in [(800., 67., true), (1500., 195., false)] {
            let layout = WorkspaceLayout::new(width);
            assert_eq!(layout.sidebar_width, expected);
            assert_eq!(layout.compact, compact);
        }
    }
}
