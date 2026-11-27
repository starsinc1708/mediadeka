use super::{
    Content, MainView, OpenPlaylist, SourceFailure, artwork::collection_cover, labels::status_label,
};
use crate::ui::components::{
    artwork::{HeroBackdrop, hero_backdrop},
    content_state::{empty_state, error_state, loading_state},
    scroll_viewport::{Density, FluidScrollViewport},
    scrollbar::{ScrollChrome, ScrollSurface, with_list_height_hints},
};
use crate::ui::design_system::adaptive_layout::{AdaptiveContent, TrackColumns};
use crate::ui::design_system::controls::ControlVariant;
use crate::ui::fluid_glass::ShellGeometry;
use crate::ui::music::detail::sticky_hero::CollapsingMediaHero;
use crate::ui::music::detail_style::HeroSize;
use crate::ui::theme::playlist::{INK_SECONDARY, PAPER, metrics};
use crate::{
    appearance::TrackDensity,
    playlist::PlaylistKey,
    storage::{Store, StoreError},
};
use gpui::{
    AnyElement, Context, Div, IntoElement, Pixels, Role, Window, div, list, prelude::*, px, rgb,
};
use guise::{Button, IconName, Size, Variant};

pub(super) const PREFIX_ROWS: usize = 2;
pub(super) const TOOLBAR_ITEM: usize = 1;
pub(super) const ROW_HEIGHT: f32 = metrics::ROW;
const COVER_COLUMN_WIDTH: f32 = 288.;
const COLUMN_GAP: f32 = 48.;
const DESKTOP_INSET_EXTRA: f32 = 16.;
const DESKTOP_TOP: f32 = 32.;
const BACKDROP_HEIGHT: f32 = 540.;
const COVER_COLUMN_MIN_WIDTH: f32 = 1700.;
const COMPACT_HEIGHT: f32 = 560.;
pub(super) const PINNED_SEARCH_ROW: f32 = 42.;

fn cover_column(window: &Window, glass: bool) -> bool {
    let viewport = window.viewport_size();
    !glass && viewport.width >= px(COVER_COLUMN_MIN_WIDTH) && viewport.height >= px(COMPACT_HEIGHT)
}

fn compact_workspace(window: &Window, adaptive: Option<AdaptiveContent>) -> bool {
    window.viewport_size().height < px(COMPACT_HEIGHT)
        || adaptive.is_some_and(|layout| layout.compact())
}

pub(super) fn column_header_height(compact: bool) -> f32 {
    if compact { 25. } else { metrics::HEADER }
}

fn pinned_toolbar_height(compact: bool) -> Pixels {
    px(PINNED_SEARCH_ROW + column_header_height(compact))
}

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub(super) margin: f32,
    pub(super) width: f32,
    pub(super) cover: f32,
    pub(super) title_size: f32,
    pub(super) artist: Option<f32>,
    pub(super) album: Option<f32>,
    pub(super) added: Option<f32>,
    pub(super) album_available: bool,
    pub(super) date_available: bool,
    pub(super) duration: f32,
    pub(super) compact: bool,
    pub(super) cover_column: bool,
    pub(super) fluid_hero: Option<HeroSize>,
    pub(super) fluid_ledger: bool,
    pub(super) row_height: f32,
    pub(super) row_cover: f32,
    table_width: Option<f32>,
}

impl Layout {
    /// Fluid Glass uses the current content width. Classic retains column
    /// availability based on `columns_width`, the undocked page width.
    fn new(width: f32, columns_width: f32, margin: f32, window: &Window, view: &MainView) -> Self {
        let preferences = view.playlist.columns;
        let glass = view.uses_fluid_glass();
        let cover_column = cover_column(window, glass);
        let adaptive = glass.then(|| view.adaptive_content(window));
        let compact = compact_workspace(window, adaptive);
        let column_inset = if cover_column {
            COVER_COLUMN_WIDTH + COLUMN_GAP + 2. * DESKTOP_INSET_EXTRA
        } else {
            0.
        };
        let width = width - column_inset;
        let columns_width = columns_width - column_inset;
        let columns =
            adaptive.map(|layout| TrackColumns::playlist(layout.table_width(), Default::default()));
        let album_available =
            columns.map_or(columns_width >= 760., |columns| columns.album.is_some());
        let date_available =
            columns.map_or(columns_width >= 935., |columns| columns.date.is_some());
        let columns = columns.map(|columns| columns.with_visibility(view.track_column_visibility));
        let track_count = view
            .playlist
            .open
            .as_ref()
            .and_then(OpenPlaylist::playlist)
            .map_or(0, |playlist| playlist.tracks.len());
        let fluid_hero =
            adaptive.map(|layout| HeroSize::playlist(layout.width, compact, track_count));
        let fluid_ledger = adaptive.is_some_and(|layout| !compact && layout.width >= 1280.);
        let artist = columns.map_or_else(
            || (columns_width >= 810.).then_some((width * 0.20).clamp(114., 300.)),
            |columns| columns.artist,
        );
        let album = columns.map_or(Some((width * 0.21).clamp(126., 320.)), |columns| {
            columns.album
        });
        let added = columns.map_or(Some(if width >= 1100. { 140. } else { 112. }), |columns| {
            columns.date
        });
        Self {
            margin: if cover_column { 0. } else { margin },
            width,
            cover: if let Some(hero) = fluid_hero {
                hero.cover
            } else if cover_column {
                COVER_COLUMN_WIDTH
            } else if compact {
                84.
            } else if width >= 1000. {
                192.
            } else {
                136.
            },
            title_size: if let Some(hero) = fluid_hero {
                hero.title
            } else if cover_column {
                38.
            } else if compact {
                23.
            } else if width >= 1000. {
                46.
            } else {
                32.
            },
            artist,
            album: album.filter(|_| album_available && (glass || preferences.album)),
            added: added.filter(|_| date_available && (glass || preferences.added)),
            album_available,
            date_available,
            duration: columns.map_or(if width >= 920. { 87. } else { 57. }, |columns| {
                columns.duration
            }),
            compact,
            cover_column,
            fluid_hero,
            fluid_ledger,
            row_height: if glass {
                view.track_density.row_height()
            } else {
                ROW_HEIGHT
            },
            row_cover: if glass {
                view.track_density.cover()
            } else {
                metrics::THUMB
            },
            table_width: adaptive.map(|layout| layout.table_width()),
        }
    }

    fn inset(self, element: impl IntoElement) -> Div {
        div().w_full().px(px(self.margin)).child(element)
    }

    fn table_inset(self, element: impl IntoElement) -> Div {
        self.inset(
            div()
                .w_full()
                .when_some(self.table_width, |table, width| table.max_w(px(width)))
                .child(element),
        )
    }

    pub(super) fn cells(self) -> Div {
        div()
            .w_full()
            .flex()
            .items_center()
            .gap(px(metrics::PAD))
            .px(px(metrics::PAD))
    }
}

impl OpenPlaylist {
    pub(super) fn is_empty_local(&self) -> bool {
        matches!(self.target.key, PlaylistKey::Local(_))
            && self
                .playlist()
                .is_some_and(|playlist| playlist.tracks.is_empty())
    }

    fn shows_track_toolbar(&self) -> bool {
        self.snapshot().is_some() && !self.is_empty_local()
    }
}

impl MainView {
    pub(in crate::ui) fn choose_track_density(
        &mut self,
        density: TrackDensity,
    ) -> Result<(), StoreError> {
        if self.track_density != density {
            Store::open_default()?.save_track_density(density)?;
            self.track_density = density;
            self.playlist.list.remeasure();
        }
        Ok(())
    }

    pub(in crate::ui) fn focus_playlist_filter(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .playlist
            .open
            .as_ref()
            .is_some_and(OpenPlaylist::shows_track_toolbar)
        {
            if let Some(offset) = self.playlist.filter_anchor.get() {
                self.playlist.list.scroll_to(gpui::ListOffset {
                    item_ix: 0,
                    offset_in_item: offset,
                });
            } else if !self.uses_fluid_glass() && !cover_column(window, false) {
                self.playlist.list.scroll_to_reveal_item(TOOLBAR_ITEM);
            }
            self.playlist
                .search
                .update(cx, |field, cx| field.focus(window, cx));
            cx.notify();
        }
    }

    pub(in crate::ui) fn playlist_viewport(
        &self,
        width: f32,
        columns_width: f32,
        margin: f32,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let layout = Layout::new(width, columns_width, margin, window, self);
        let open = self.playlist.open.as_ref();
        let show_toolbar =
            layout.cover_column && open.is_some_and(OpenPlaylist::shows_track_toolbar);
        let collapsing =
            layout.fluid_hero.is_some() && open.is_some_and(OpenPlaylist::shows_track_toolbar);
        div()
            .flex()
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .when(layout.fluid_hero.is_none(), |workspace| {
                workspace.bg(rgb(PAPER))
            })
            .when_some(open.filter(|_| layout.cover_column), |workspace, open| {
                workspace
                    .px(px(margin + DESKTOP_INSET_EXTRA))
                    .pt(px(DESKTOP_TOP))
                    .gap(px(COLUMN_GAP))
                    .when_some(collection_cover(open), |workspace, url| {
                        workspace.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .right_0()
                                .h(px(BACKDROP_HEIGHT))
                                .max_h(gpui::relative(1.))
                                .child(hero_backdrop(
                                    Some(url),
                                    None,
                                    rgb(PAPER).into(),
                                    HeroBackdrop::Collection,
                                )),
                        )
                    })
                    .child(
                        div()
                            .id("playlist-collection")
                            .accessibility_id("playlist-collection")
                            .role(Role::Group)
                            .aria_label("Плейлист")
                            .w(px(COVER_COLUMN_WIDTH))
                            .flex_none()
                            .max_h(gpui::relative(1.))
                            .self_start()
                            .overflow_y_scroll()
                            .child(self.playlist_hero(open, layout, None, cx)),
                    )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .when(layout.cover_column || collapsing, |column| {
                        column.children(self.local_editor(cx).map(|editor| editor.pb(px(12.))))
                    })
                    .when_some(open.filter(|_| collapsing), |column, open| {
                        column.children(
                            self.playlist_hero_notices(open, cx)
                                .map(|notices| layout.inset(notices)),
                        )
                    })
                    .when(show_toolbar, |column| {
                        column.child(
                            div()
                                .flex_none()
                                .child(self.playlist_toolbar(layout, window, cx)),
                        )
                    })
                    .child(self.playlist_table(layout, window, cx)),
            )
    }

    fn playlist_table(
        &self,
        layout: Layout,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let view = cx.weak_entity();
        let state = &self.playlist.list;
        let hero = self.playlist_media_hero(layout.fluid_hero, cx);
        let toolbar_height = pinned_toolbar_height(layout.compact);
        let top_inset = hero.map_or(Pixels::ZERO, |hero| hero.height() + toolbar_height);
        self.playlist.filter_anchor.set(
            hero.filter(|_| state.logical_scroll_top().item_ix > 0)
                .map(|hero| hero.expanded_height() - hero.height()),
        );
        let surface = ScrollSurface::VirtualList(state.clone());
        let chrome = ScrollChrome::new(self.uses_fluid_glass(), window);
        let shell = ShellGeometry::for_workspace(window, self.docked_panel_width(window));
        let mut bottom_padding = shell.scroll_content_padding();
        if self.uses_fluid_glass()
            && !self.playlist.query.is_empty()
            && let Some(open) = self.playlist.open.as_ref()
        {
            // A short result set must not pull the hidden hero back above the
            // focused filter. Reserve the rest of the viewport below the rows.
            let rows_height = layout.row_height * open.visible.len().max(1) as f32;
            let viewport_height = f32::from(state.viewport_bounds().size.height);
            bottom_padding = bottom_padding.max(viewport_height - rows_height);
        }
        with_list_height_hints(div(), state, px(layout.row_height))
            .id("playlist-viewport")
            .accessibility_id("playlist-viewport")
            .role(Role::List)
            .aria_label("Треки плейлиста")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .relative()
            .w_full()
            .overflow_hidden()
            .child(
                FluidScrollViewport::new(
                    list(state.clone(), move |index, window, cx| {
                        view.update(cx, |view, cx| {
                            view.playlist_item(index, layout, hero, window, cx)
                        })
                        .unwrap_or_else(|_| div().into_any_element())
                    })
                    .size_full()
                    .pb(px(bottom_padding)),
                    surface.clone(),
                    self.uses_fluid_glass(),
                    Density::Rows(px(layout.row_height)),
                )
                .under_header(top_inset),
            )
            .child(
                self.prefetch_after_list_layout(
                    state.clone(),
                    PREFIX_ROWS,
                    self.playlist
                        .open
                        .as_ref()
                        .map_or(0, |open| open.visible.len()),
                    top_inset,
                    Self::prefetch_playlist_window,
                    cx,
                ),
            )
            .children(
                hero.zip(self.playlist.open.as_ref())
                    .map(|(hero, open)| self.playlist_hero(open, layout, Some(hero), cx)),
            )
            .children(hero.map(|hero| {
                div()
                    .absolute()
                    .top(hero.height())
                    .left_0()
                    .right_0()
                    .h(toolbar_height)
                    .child(layout.table_inset(self.playlist_toolbar(layout, window, cx)))
            }))
            .child(super::scrollbar::scrollbar(cx, &surface, chrome).tab_index(38))
            .child(self.scrollbar_guide(surface, chrome, cx))
    }

    fn prefetch_playlist_window(
        &mut self,
        rows: std::ops::Range<usize>,
        backwards: bool,
        scale: f32,
        cx: &mut Context<Self>,
    ) {
        let (covers, targets) = self
            .playlist
            .open
            .as_ref()
            .and_then(|open| {
                let tracks = &open.playlist()?.tracks;
                let mut rows = open.visible.get(rows)?.to_vec();
                if backwards {
                    rows.reverse();
                }
                let entries = rows.into_iter().map(|index| &tracks[index]);
                let covers = entries
                    .clone()
                    .filter_map(|track| track.artwork.clone())
                    .collect();
                let targets = entries
                    .flat_map(|track| {
                        super::navigation::row_prefetch_targets(&open.target.key, track)
                    })
                    .collect::<Vec<_>>();
                Some((covers, targets))
            })
            .unwrap_or_default();
        self.prefetch_list_window(covers, targets, scale, cx);
    }

    pub(in crate::ui) fn playlist_hero_light(&self, window: &Window, cx: &Context<Self>) -> f32 {
        self.playlist_collapsing_hero(window, cx)
            .map_or(1., |hero| hero.hero_light())
    }

    pub(in crate::ui) fn playlist_collapsing_hero(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<CollapsingMediaHero> {
        let adaptive = self.adaptive_content(window);
        let compact = compact_workspace(window, Some(adaptive));
        let count = self.playlist.open.as_ref()?.playlist()?.tracks.len();
        self.playlist_media_hero(Some(HeroSize::playlist(adaptive.width, compact, count)), cx)
    }

    #[cfg(feature = "capture")]
    pub fn playlist_scroll_content_bounds_for_capture(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> gpui::Bounds<Pixels> {
        let mut bounds = self.playlist.list.viewport_bounds();
        if let Some(hero) = self.playlist_collapsing_hero(window, cx) {
            let compact = compact_workspace(window, Some(self.adaptive_content(window)));
            let inset = (hero.height() + pinned_toolbar_height(compact)).min(bounds.size.height);
            bounds.origin.y += inset;
            bounds.size.height -= inset;
        }
        bounds
    }

    fn playlist_media_hero(
        &self,
        metrics: Option<HeroSize>,
        cx: &Context<Self>,
    ) -> Option<CollapsingMediaHero> {
        let metrics = metrics?;
        if !self.uses_fluid_glass() || !self.playlist.open.as_ref()?.shows_track_toolbar() {
            return None;
        }
        let height = metrics.cover + 2. * metrics.vertical_inset;
        let offset = self.playlist.list.logical_scroll_top();
        Some(CollapsingMediaHero::new(
            height,
            metrics.cover,
            metrics.title,
            if offset.item_ix == 0 {
                offset.offset_in_item
            } else {
                px(height)
            },
            self.reduced_motion || self.fluid_high_contrast(cx),
        ))
    }

    fn playlist_item(
        &self,
        index: usize,
        layout: Layout,
        hero: Option<CollapsingMediaHero>,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(open) = &self.playlist.open else {
            return div().into_any_element();
        };
        if index == 0 {
            return if let Some(hero) = hero {
                div().h(hero.expanded_height()).into_any_element()
            } else if layout.cover_column {
                div().h(px(6.)).into_any_element()
            } else {
                layout
                    .inset(self.playlist_hero(open, layout, None, cx))
                    .into_any_element()
            };
        }
        if index == TOOLBAR_ITEM {
            return layout
                .table_inset(
                    self.playlist_intro(open, layout, hero.is_some(), window, cx)
                        .when(hero.is_some(), |intro| {
                            intro.pt(pinned_toolbar_height(layout.compact))
                        }),
                )
                .into_any_element();
        }
        let Some(snapshot) = open.snapshot() else {
            return div().into_any_element();
        };
        let playlist = &snapshot.playlist;
        let Some(&original) = open.visible.get(index - PREFIX_ROWS) else {
            if playlist.tracks.len() == 1 && self.playlist.projected_removed_track(open).is_some() {
                return div().into_any_element();
            }
            return layout
                .table_inset(empty_state(
                    "playlist-empty",
                    if matches!(open.content, Content::Partial { .. }) {
                        "Среди загруженных треков совпадений пока нет."
                    } else if playlist.tracks.is_empty() {
                        "В плейлисте пока нет треков."
                    } else {
                        "По запросу ничего не найдено."
                    },
                    None,
                    IconName::ListMusic,
                    cx,
                ))
                .into_any_element();
        };
        let track = &playlist.tracks[original];
        let player = &self.player_snapshot;
        let active = if !self.uses_fluid_glass() {
            player.highlights(&open.target.key, snapshot.revision, original)
        } else {
            let matching_recording = self
                .player
                .read(cx)
                .recording_status(&open.target.key, track)
                .is_some();
            let same_snapshot = player.key.as_ref() == Some(&open.target.key)
                && player.playlist_revision == snapshot.revision;
            matching_recording
                && (!same_snapshot
                    || player.identifies(&open.target.key, snapshot.revision, original))
        };
        let selected = open.selected == Some(original);
        layout
            .table_inset(self.playlist_row(original, track, layout, active, selected, cx))
            .into_any_element()
    }

    fn playlist_intro(
        &self,
        open: &OpenPlaylist,
        layout: Layout,
        pinned_toolbar: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        match &open.content {
            Content::Loading => {
                div().child(loading_state("playlist-loading", "Загружаем плейлист…", cx))
            }
            Content::Error(error) => self.playlist_error(
                "playlist-unavailable",
                "playlist-retry",
                &open.target.key,
                error,
                cx,
            ),
            Content::Loaded(_) | Content::Partial { .. } => div()
                .when(!layout.cover_column && !pinned_toolbar, |intro| {
                    intro.children(self.local_editor(cx).map(|editor| editor.pb(px(8.))))
                })
                .when(
                    !layout.cover_column && !pinned_toolbar && open.shows_track_toolbar(),
                    |intro| intro.child(self.playlist_toolbar(layout, window, cx)),
                )
                .when(
                    open.request.is_some()
                        && open.complete_snapshot().is_some()
                        && (!self.uses_fluid_glass() || !open.shows_track_toolbar()),
                    |intro| intro.child(status_label("playlist-refreshing", "Обновляем плейлист…")),
                )
                .when_some(open.refresh_error.as_ref(), |intro, error| {
                    intro.child(self.playlist_error(
                        "playlist-refresh-error",
                        "playlist-refresh-retry",
                        &open.target.key,
                        error,
                        cx,
                    ))
                }),
        }
    }

    fn playlist_error(
        &self,
        status_id: &'static str,
        button_id: &'static str,
        key: &PlaylistKey,
        error: &SourceFailure,
        cx: &Context<Self>,
    ) -> Div {
        let action = if let Some(provider) = key.provider().filter(|_| error.needs_sign_in) {
            self.provider_recovery_action("playlist-connect", provider, 34, cx)
                .variant(Variant::Light)
        } else {
            Button::new(button_id, "Повторить")
                .variant(Variant::Subtle)
                .color(gpui::Hsla::from(rgb(INK_SECONDARY)))
                .tab_order(34)
                .on_click(cx.listener(|view, _, _, cx| view.refresh_playlist(cx)))
        };
        let action = action
            .size(Size::Xs)
            .when_some(self.glass_controls(cx), |button, material| {
                ControlVariant::TextAction.text_button(button, material)
            });
        div().child(error_state(status_id, error.message.clone(), None, cx).child(action))
    }
}
