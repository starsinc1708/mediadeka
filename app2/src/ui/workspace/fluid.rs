pub(super) mod page_motion;

use gpui::{Context, Div, StyleRefinement, Window, WindowControlArea, div, prelude::*, px, rgb};

use crate::{
    music::ObjectKind,
    state::HomeView,
    ui::{
        MainView,
        fluid_glass::{Ambient, Backdrop, Geometry, PageAtmosphere, ShellGeometry, tokens},
        listening::ListeningMode,
    },
};

use super::{WorkspaceLayout, routes::Route};

impl WorkspaceLayout {
    pub(super) fn fluid(window: &Window) -> Self {
        let shell = ShellGeometry::for_window(window);
        let geometry = shell.base();
        let compact = geometry.compact();
        let viewport_width = shell.viewport().width;
        Self {
            compact,
            sidebar_width: geometry.rail(),
            margin: geometry.margin(),
            width: viewport_width - 2. * geometry.inset(),
            content_width: f32::from(shell.content_bounds().size.width),
        }
    }
}

impl MainView {
    pub(super) fn has_full_fluid_navigation(
        &self,
        layout: WorkspaceLayout,
        window: &Window,
    ) -> bool {
        let minimum = if self.has_pins() {
            tokens::NAV_FULL_PINNED_MIN_HEIGHT
        } else {
            tokens::NAV_FULL_MIN_HEIGHT
        };
        !layout.compact && window.viewport_size().height >= px(minimum)
    }

    pub(super) fn fluid_workspace(
        &self,
        home: Option<&HomeView>,
        refreshing: bool,
        layout: WorkspaceLayout,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let geometry = Geometry::for_window(window);
        let high_contrast = self.fluid_high_contrast(cx);
        let page_panel = self.panel_replaces_page(window);
        let backdrop = self.fluid_backdrop.as_ref().map(|backdrop| {
            let route = self.workspace.routes.current();
            let atmosphere = if self.listening.mode() == Some(ListeningMode::Lyrics) {
                Some(PageAtmosphere::lyrics())
            } else {
                self.workspace_atmosphere(window, cx)
            };
            Backdrop::set_workspace(
                backdrop,
                self.docked_panel_surface(window),
                exposure(route, self.fluid_local_group_exposure()),
                atmosphere,
                cx,
            );
            backdrop
                .clone()
                .cached(StyleRefinement::default().absolute().inset_0())
        });
        div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(if high_contrast {
                tokens::contrast::CANVAS
            } else {
                tokens::CANVAS
            }))
            .p(px(geometry.inset()))
            .child(
                // The shell is inset from the window edge; the strip above the
                // toolbar is the titlebar users grab to move the window.
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(geometry.inset()))
                    .window_control_area(WindowControlArea::Drag),
            )
            .when(high_contrast && page_panel, |workspace| {
                // Keep the opaque fill behind the shell contour.
                workspace.child(
                    div()
                        .absolute()
                        .top(px(geometry.inset() + tokens::TITLEBAR))
                        .left(px(geometry.inset() + geometry.rail()))
                        .right(px(geometry.inset()))
                        .bottom(px(geometry.inset()
                            + geometry.player()
                            + geometry.content_bottom()))
                        .bg(rgb(tokens::contrast::SURFACE)),
                )
            })
            .children(backdrop)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .flex()
                    .child(self.music_navigation(home, layout, window, cx))
                    .child(
                        div()
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .pb(px(geometry.content_bottom()))
                            .child(self.music_toolbar(layout, window, cx))
                            .child(self.workspace_body(home, refreshing, layout, window, cx)),
                    ),
            )
            .child(
                div()
                    .relative()
                    .w_full()
                    .flex_none()
                    .child(self.player.clone()),
            )
            .when(!page_panel || self.now_playing_menu_open(), |workspace| {
                workspace.children(self.playlist_overlay(window, cx))
            })
    }

    fn workspace_atmosphere(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<PageAtmosphere<'_>> {
        let ambient = match self.workspace.routes.current() {
            Route::Home | Route::Station(_) => Ambient::Home,
            Route::Search { .. } => Ambient::Search,
            Route::History => Ambient::History,
            Route::Playlist(target) => {
                return Some(
                    PageAtmosphere::playlist(
                        &target.key,
                        self.playlist_atmosphere_artwork(&target.key, target.artwork.as_deref()),
                    )
                    .with_hero_light(self.playlist_hero_light(window, cx)),
                );
            }
            Route::Object(target) => match target.object.kind {
                ObjectKind::Station => Ambient::Home,
                _ => {
                    let content = self.adaptive_content(window);
                    let compact = content.compact() || window.viewport_size().height < px(560.);
                    let light = self
                        .object_hero_scroll(content.width, compact, cx)
                        .map_or(1., |hero| hero.hero_light());
                    return self
                        .object_atmosphere()
                        .map(|page| page.with_hero_light(light));
                }
            },
            Route::Library(super::LibraryTab::Tracks) => {
                return self
                    .local_group_atmosphere()
                    .map(|page| page.with_hero_light(self.local_album_hero_light(window, cx)));
            }
            Route::Library(_) | Route::LocalTrack(_) => Ambient::Library,
            Route::Settings => return Some(PageAtmosphere::settings()),
        };
        Some(PageAtmosphere::ambient(ambient))
    }
}

fn exposure(route: &Route, local_group: Option<f32>) -> f32 {
    use tokens::atmosphere;

    match route {
        Route::Home | Route::Station(_) => atmosphere::HOME,
        Route::Search { .. } => atmosphere::SEARCH,
        Route::Settings => atmosphere::SETTINGS,
        Route::Playlist(_) => atmosphere::PLAYLIST,
        Route::Object(target) => match target.object.kind {
            ObjectKind::Artist => atmosphere::ARTIST,
            ObjectKind::Album => atmosphere::ALBUM,
            ObjectKind::Playlist | ObjectKind::Collection => atmosphere::PLAYLIST,
            ObjectKind::Station => atmosphere::HOME,
            ObjectKind::Track => atmosphere::LIBRARY,
        },
        Route::Library(super::LibraryTab::Tracks) => local_group.unwrap_or(atmosphere::LIBRARY),
        Route::Library(_) | Route::History | Route::LocalTrack(_) => atmosphere::LIBRARY,
    }
}
