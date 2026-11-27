use gpui::{
    AccessibleAction, AnyElement, Context, Div, FontWeight, IntoElement, KeyDownEvent,
    ListAlignment, ListOffset, ListState, Pixels, Role, SharedString, Stateful, Window,
    accesskit::ActionData, div, list, prelude::*, px, rgb,
};
use guise::{Icon, IconName};
use std::ops::Range;

use crate::{
    appearance::TrackColumn,
    music::library::{EntryAction, LibraryRow, Scope},
    playback::{QueueEntry, can_resolve},
    storage::CollectionView,
    ui::{
        MainView,
        assets::{mark, provider_mark},
        components::{
            Activate, button,
            music::{TRACK_ROW_HEIGHT, cover, time_label},
            scroll_viewport::{Density, FluidScrollViewport, ReadAhead},
            scrollbar::{ScrollChrome, ScrollSurface, scrollbar_element, with_list_height_hints},
            track_row,
        },
        design_system::adaptive_layout::{AdaptiveContent, TrackColumns},
        fluid_glass::{ShellGeometry, tokens as fluid},
        music::{PlaybackAction, QUEUE_CONTROL, detail::sticky_hero::scroll_to_hero_top},
        playlist::popup::MenuKind,
        theme::{ELEVATED, secondary, surface_border},
        workspace::{LibraryLocation, LibraryScroll, MusicTarget, Page, Target},
    },
};

use super::super::prefetch::entry_target;
use super::{
    Library,
    cards::{Grid, collection_subtitle},
    motion::{Reflow, Surface},
};
use crate::ui::music::detail::hero_motion::{HeroMotion, cover_span, painted_fragment};

const HEADER_ITEMS: usize = 1;
pub(super) const COLUMN_HEADER_HEIGHT: f32 = 24.;
const WIDE_TABLE: f32 = 990.;
const LIKE_WIDTH: f32 = 74.;
const ARTIST_WIDTH: f32 = 144.;
const ALBUM_WIDTH: f32 = 170.;
const COVER_WIDTH: f32 = 32.;
const SOURCES_WIDTH: f32 = 122.;
const COMPACT_SOURCES_WIDTH: f32 = 106.;
const DATE_WIDTH: f32 = 80.;
const DURATION_WIDTH: f32 = 40.;
const ROW_INSET: f32 = 7.;
const BUTTON_BORDER: f32 = 1.;
const COLLECTION_LIST_HEIGHT: u16 = 52;
const COLLECTION_COMPACT_HEIGHT: u16 = 36;
const COLLECTION_LIST_COVER: f32 = 40.;
const COLLECTION_COMPACT_COVER: f32 = 28.;
const COLLECTION_LIST_WIDTH: f32 = 940.;
const COLLECTION_COMPACT_WIDTH: f32 = 850.;
const CLASSIC_ALBUM_EDGE: u16 = 140;
const COMPACT_GRID_SCALE: f32 = 0.66;
const COVER_RADIUS: f32 = 5.;

pub(super) struct LibraryList {
    state: ListState,
    pending: Option<LibraryScroll>,
    last_scroll: ListOffset,
    read_ahead: ReadAhead,
    layout: Layout,
    pub(super) reflow: Reflow,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Layout {
    Rows(u16),
    Grid(Grid),
}

impl Layout {
    fn columns(self) -> usize {
        match self {
            Self::Rows(_) => 1,
            Self::Grid(grid) => grid.columns,
        }
    }

    fn height(self) -> Pixels {
        px(match self {
            Self::Rows(height) => f32::from(height),
            Self::Grid(grid) => grid.row_height(),
        })
    }
}

impl Default for LibraryList {
    fn default() -> Self {
        Self {
            state: ListState::new(HEADER_ITEMS, ListAlignment::Top, px(180.)),
            pending: None,
            last_scroll: ListOffset::default(),
            read_ahead: ReadAhead::default(),
            layout: Layout::Rows(TRACK_ROW_HEIGHT as u16),
            reflow: Reflow::default(),
        }
    }
}

impl LibraryList {
    fn remember_scroll(&mut self, entries: usize) {
        self.last_scroll = self.state.logical_scroll_top();
        let top = self.last_scroll.item_ix.saturating_sub(HEADER_ITEMS) * self.layout.columns();
        // Seed direction from restored positions before classifying the next user move.
        self.read_ahead.seed(top.min(entries));
    }

    fn visible_rows(&self) -> Range<usize> {
        let viewport = self.state.viewport_bounds();
        let offset = self.state.logical_scroll_top();
        let items = self.state.item_count().max(HEADER_ITEMS);
        let first = offset.item_ix.clamp(HEADER_ITEMS, items);
        let row_height = self.layout.height();
        let count = ((viewport.size.height + offset.offset_in_item) / row_height).ceil() as usize;
        let end = first.saturating_add(count).min(items);
        let end = (first..end)
            .find(|&index| {
                self.state
                    .bounds_for_item(index)
                    .is_some_and(|bounds| bounds.top() >= viewport.bottom())
            })
            .unwrap_or(end);
        let columns = self.layout.columns();
        (first - HEADER_ITEMS) * columns..end.saturating_sub(HEADER_ITEMS) * columns
    }
}

impl Library {
    pub(super) fn change_filters(&mut self, change: impl FnOnce(&mut LibraryLocation)) {
        let removed = self.visible_row_spans();
        change(&mut self.location);
        self.list.read_ahead = ReadAhead::default();
        self.project(Default::default(), false);
        let mut removed = removed;
        for row in &self.results.visible {
            removed.remove(row);
        }
        let started = std::time::Instant::now();
        for span in removed.into_values() {
            self.list.reflow.exits.start(span, started);
        }
    }

    fn visible_row_spans(&self) -> std::collections::HashMap<usize, gpui::SpanId> {
        if !matches!(self.list.layout, Layout::Rows(_)) {
            return Default::default();
        }
        let state = &self.list.state;
        let viewport = state.viewport_bounds();
        let top = state.logical_scroll_top().item_ix.max(HEADER_ITEMS);
        self.results
            .visible
            .iter()
            .copied()
            .enumerate()
            .skip(top - HEADER_ITEMS)
            .take_while(|(index, _)| {
                state
                    .bounds_for_item(index + HEADER_ITEMS)
                    .is_some_and(|bounds| bounds.top() < viewport.bottom())
            })
            .map(|(_, index)| {
                let entry = self.index.rows[index].entry(self.location.source.as_ref());
                (index, cover_span(("library-row", &entry.key)))
            })
            .collect()
    }

    pub(super) fn group_scroll_offset(&self, expanded_height: Pixels) -> Pixels {
        let offset = self.list.state.logical_scroll_top();
        if offset.item_ix < HEADER_ITEMS {
            offset.offset_in_item
        } else {
            expanded_height
        }
    }

    pub(super) fn init(&mut self, location: LibraryLocation, cx: &Context<MainView>) {
        self.list.reflow = Reflow::default();
        self.list.pending = Some(location.scroll.clone());
        self.list.remember_scroll(self.results.visible.len());
        self.location = location;
        let view = cx.weak_entity();
        self.list.state.set_scroll_handler(move |_, window, cx| {
            let view = view.clone();
            // The list callback runs while GPUI holds its scroll state.
            // Read the updated anchor after that event has completed.
            window.defer(cx, move |window, cx| {
                let _ = view.update(cx, |view, cx| {
                    view.library_scrolled(window, cx);
                });
            });
        });
    }

    pub(super) fn scroll(&self) -> LibraryScroll {
        if let Some(pending) = &self.list.pending {
            return pending.clone();
        }
        let offset = self.list.state.logical_scroll_top();
        let row = offset
            .item_ix
            .checked_sub(HEADER_ITEMS)
            .and_then(|row| self.results.visible.get(row * self.list.layout.columns()))
            .map(|&row| {
                self.index.rows[row]
                    .entry(self.location.source.as_ref())
                    .key
                    .clone()
            });
        LibraryScroll { offset, row }
    }

    pub(super) fn project(&mut self, position: LibraryScroll, loading: bool) {
        let location = &self.location;
        let results = self.index.project(
            location.source.as_ref(),
            location.group.as_ref(),
            &location.query,
            location.downloaded,
            location.sort,
        );
        if self.results.visible != results.visible {
            self.list
                .reflow
                .begin(if matches!(self.list.layout, Layout::Grid(_)) {
                    Surface::Cover
                } else {
                    Surface::Row
                });
        }
        self.results = results;
        let previous = self.list.state.item_count();
        let rows = self.row_count();
        if previous != rows + HEADER_ITEMS {
            self.list.state.splice(HEADER_ITEMS..previous, rows);
            self.list
                .state
                .clone()
                .with_uniform_item_height(self.list.layout.height());
        }
        self.list.pending = None;
        self.apply_position(position, loading);
    }

    fn apply_position(&mut self, mut position: LibraryScroll, loading: bool) {
        if let Some(key) = &position.row {
            if let Some(index) = self.results.visible.iter().position(|&index| {
                self.index.rows[index]
                    .entries()
                    .any(|entry| &entry.key == key)
            }) {
                position.offset.item_ix = index / self.list.layout.columns() + HEADER_ITEMS;
            } else if loading {
                self.list.pending = Some(position);
                return;
            }
        } else if loading && position.offset.item_ix > self.row_count() {
            self.list.pending = Some(position);
            return;
        }
        position.offset.item_ix = position.offset.item_ix.min(self.row_count());
        self.list.state.scroll_to(position.offset);
        self.list.remember_scroll(self.results.visible.len());
    }

    fn row_count(&self) -> usize {
        self.results
            .visible
            .len()
            .div_ceil(self.list.layout.columns())
    }

    fn set_layout(&mut self, layout: Layout, loading: bool) {
        if layout == self.list.layout {
            return;
        }
        let mut position = self.scroll();
        self.list.reflow.begin(Surface::Cover);
        if layout.height() != self.list.layout.height() {
            position.offset.offset_in_item = Pixels::ZERO;
        }
        self.list.layout = layout;
        self.list.state.reset(self.row_count() + HEADER_ITEMS);
        self.list
            .state
            .clone()
            .with_uniform_item_height(layout.height());
        self.list.pending = None;
        self.apply_position(position, loading);
    }

    pub(super) fn finish_position(&mut self) {
        if let Some(position) = self.list.pending.take() {
            self.apply_position(position, false);
        }
    }

    pub(super) fn reveal(&mut self) {
        self.list.pending = None;
        self.list.state.scroll_to(ListOffset::default());
        self.list.remember_scroll(self.results.visible.len());
    }
}

impl MainView {
    fn library_scrolled(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.music_page().library_tab().is_none() {
            return;
        }
        let library = &mut self.music.collections;
        let offset = library.list.state.logical_scroll_top();
        let previous = std::mem::replace(&mut library.list.last_scroll, offset);
        if previous.item_ix == offset.item_ix && previous.offset_in_item == offset.offset_in_item {
            return;
        }
        library.list.pending = None;
        if library.location.group.is_some() {
            cx.notify();
        }
        let visible = library.list.visible_rows();
        let start = visible.start;
        let next = library
            .list
            .read_ahead
            .range(visible, library.results.visible.len());
        let backwards = next.end <= start;
        let mut entries: Vec<_> = library.results.visible[next]
            .iter()
            .map(|&index| library.index.rows[index].entry(library.location.source.as_ref()))
            .collect();
        if backwards {
            entries.reverse();
        }
        let covers = entries
            .iter()
            .filter_map(|entry| entry.artwork.clone())
            .collect();
        let targets: Vec<_> = entries.into_iter().filter_map(entry_target).collect();
        self.prefetch_list_window(covers, targets, window.scale_factor(), cx);
    }

    pub(in crate::ui) fn local_album_hero_light(&self, window: &Window, cx: &Context<Self>) -> f32 {
        let width = self.adaptive_content(window).width;
        let compact = width < WIDE_TABLE || window.viewport_size().height < px(560.);
        self.local_group_hero(width, compact, cx)
            .map_or(1., |hero| hero.hero_light())
    }

    pub(super) fn collection_motion(
        &self,
        span: gpui::SpanId,
        surface: Surface,
        window: &Window,
        cx: &Context<Self>,
    ) -> (Option<HeroMotion>, f32) {
        if !self.uses_fluid_glass() || self.reduced_motion || cx.reduce_motion() {
            return (None, 1.);
        }
        self.music.collections.list.reflow.at(span, surface, window)
    }

    #[cfg(feature = "capture")]
    pub fn sample_collection_motion_for_capture(
        &mut self,
        elapsed_ms: Option<u64>,
        cx: &mut Context<Self>,
    ) {
        self.music.collections.list.reflow.sample =
            elapsed_ms.map(std::time::Duration::from_millis);
        cx.notify();
    }

    #[cfg(feature = "capture")]
    pub fn library_departure_alpha_for_capture(&self) -> f32 {
        self.music.collections.list.reflow.exits.strongest_alpha()
    }

    pub(super) fn shows_library_rows(&self) -> bool {
        matches!(self.music.collections.list.layout, Layout::Rows(_))
    }

    pub(super) fn library_table(&self, width: f32, wide: bool, tracks: bool) -> Table {
        let glass = self.uses_fluid_glass();
        if !glass {
            return Table {
                width,
                cover: COVER_WIDTH,
                columns: TrackColumns {
                    artist: wide.then_some(ARTIST_WIDTH),
                    album: wide.then_some(ALBUM_WIDTH),
                    date: Some(DATE_WIDTH),
                    source: Some(if wide {
                        SOURCES_WIDTH
                    } else {
                        COMPACT_SOURCES_WIDTH
                    }),
                    duration: DURATION_WIDTH,
                },
            };
        }
        let display = self
            .music_page()
            .library_tab()
            .and_then(|tab| tab.collection_kind())
            .map(|kind| self.music.collection_views.get(kind).view);
        let (max, cover) = match display {
            Some(CollectionView::Compact) => (COLLECTION_COMPACT_WIDTH, COLLECTION_COMPACT_COVER),
            Some(_) => (COLLECTION_LIST_WIDTH, COLLECTION_LIST_COVER),
            None => (width, self.track_density.cover()),
        };
        let width = AdaptiveContent::resolve(width.min(max), None).table_width();
        let columns = if tracks {
            TrackColumns::library(width, self.track_column_visibility)
        } else {
            TrackColumns {
                artist: None,
                album: None,
                source: (width >= 420.).then_some(72.),
                date: (width >= 720.).then_some(110.),
                duration: DURATION_WIDTH,
            }
        };
        Table {
            width,
            cover,
            columns,
        }
    }

    pub(in crate::ui) fn set_library_layout(&mut self, width: f32) {
        let tab = self.music_page().library_tab();
        let display = self
            .uses_fluid_glass()
            .then(|| tab.and_then(|tab| tab.collection_kind()))
            .flatten()
            .map(|kind| self.music.collection_views.get(kind));
        let layout = if let Some(display) = display {
            match display.view {
                CollectionView::Grid => {
                    let edge = f32::from(display.card_edge);
                    let edge = if AdaptiveContent::resolve(width, None).compact() {
                        (edge * COMPACT_GRID_SCALE).round() as u16
                    } else {
                        display.card_edge
                    };
                    Layout::Grid(Grid::new(width, edge))
                }
                CollectionView::List => Layout::Rows(COLLECTION_LIST_HEIGHT),
                CollectionView::Compact => Layout::Rows(COLLECTION_COMPACT_HEIGHT),
            }
        } else if self.music_page() == Page::Albums && !self.music.collections.location.album_list {
            Layout::Grid(Grid::new(width, CLASSIC_ALBUM_EDGE))
        } else if self.uses_fluid_glass() {
            Layout::Rows(self.track_density.row_height() as u16)
        } else {
            Layout::Rows(TRACK_ROW_HEIGHT as u16)
        };
        let loading = self.music.request.is_some();
        self.music.collections.set_layout(layout, loading);
    }

    pub(in crate::ui) fn library_view(
        &self,
        margin: f32,
        width: f32,
        show_tabs: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let page = self.music_page();
        let layout = self.music.collections.list.layout;
        let width = if self.uses_fluid_glass() {
            self.adaptive_content(window).width
        } else {
            width - 2. * margin
        };
        let state = &self.music.collections.list.state;
        let view = cx.weak_entity();
        let surface = ScrollSurface::VirtualList(state.clone());
        let chrome = ScrollChrome::new(self.uses_fluid_glass(), window);
        let id = if page == Page::Tracks {
            "music-liked-page"
        } else {
            "music-library-page"
        };
        let title = page.library_tab().map_or("Медиатека", |tab| tab.label());
        let short = window.viewport_size().height < px(560.);
        let group_hero = self.local_group_hero(width, width < WIDE_TABLE || short, cx);
        let group_hero_height = group_hero.as_ref().map(|hero| hero.expanded_height());
        let has_group_columns = group_hero.is_some()
            && matches!(layout, Layout::Rows(_))
            && !self.music.collections.results.visible.is_empty();
        let natural_column_top = has_group_columns.then(|| group_column_top(state)).flatten();
        let visible_hero_height = group_hero
            .as_ref()
            .map_or(Pixels::ZERO, |hero| hero.height());
        let columns_top = natural_column_top.unwrap_or(visible_hero_height);
        let stuck_columns = has_group_columns && columns_top <= visible_hero_height;
        let header_cutoff = visible_hero_height
            + px(if stuck_columns {
                COLUMN_HEADER_HEIGHT
            } else {
                0.
            });
        let shell = ShellGeometry::for_workspace(window, self.docked_panel_width(window));
        let bottom_padding = shell.scroll_content_padding();
        let scrolling = FluidScrollViewport::new(
            div()
                .relative()
                .size_full()
                .when(
                    !self.music.collections.list.reflow.exits.is_empty(),
                    |body| {
                        body.child(
                            self.music
                                .collections
                                .list
                                .reflow
                                .paint_exits(self.uses_fluid_glass() && !self.reduced_motion),
                        )
                    },
                )
                .child(
                    list(state.clone(), move |index, window, cx| {
                        view.update(cx, |view, cx| {
                            view.library_item(
                                index,
                                (margin, width),
                                show_tabs,
                                group_hero_height,
                                window,
                                cx,
                            )
                        })
                        .unwrap_or_else(|_| div().into_any_element())
                    })
                    .size_full()
                    .pb(px(bottom_padding)),
                ),
            surface.clone(),
            self.uses_fluid_glass(),
            Density::Rows(layout.height()),
        );
        let scrolling = if group_hero.is_some() {
            scrolling.under_header(header_cutoff)
        } else {
            scrolling
        };
        let scrolling = div()
            .size_full()
            .child(scrolling)
            .when(has_group_columns, |body| {
                let state = state.clone();
                body.on_children_prepainted(move |_, window, _| {
                    // Filters can resize the virtual prefix. Re-align the column
                    // overlay after the list measures that real item again.
                    if group_column_top(&state) != natural_column_top {
                        window.on_next_frame(|window, _| window.refresh());
                    }
                })
            });
        with_list_height_hints(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_hidden()
                .child(scrolling)
                .children(group_hero.and_then(|hero| {
                    self.local_group_header(hero, width + 2. * margin, margin, cx)
                }))
                .when(has_group_columns, |viewport| {
                    viewport.child(
                        div()
                            .id("library-group-columns")
                            .accessibility_id("library-group-columns")
                            .role(Role::Group)
                            .aria_label("Столбцы треков")
                            .absolute()
                            .top(columns_top.max(visible_hero_height))
                            .left(px(margin))
                            .right(px(margin))
                            .child(
                                columns(
                                    self.library_table(width, width >= WIDE_TABLE, true),
                                    true,
                                    true,
                                )
                                .border_b_1()
                                .border_color(surface_border()),
                            ),
                    )
                })
                .child(self.library_scrollbar(&surface, page, chrome, cx))
                .child(self.observed_scrollbar_guide(
                    surface,
                    chrome,
                    Some(Self::library_scrolled),
                    cx,
                )),
            state,
            layout.height(),
        )
        .id(id)
        .accessibility_id(id)
        .role(Role::List)
        .aria_label(title)
    }

    fn library_scrollbar(
        &self,
        surface: &ScrollSurface,
        page: Page,
        chrome: ScrollChrome,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let view = cx.weak_entity();
        let id = if page == Page::Tracks {
            "liked-scrollbar"
        } else {
            "library-scrollbar"
        };
        scrollbar_element(id, "Прокрутка медиатеки", surface, chrome)
            .tab_index(47)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                let surface = ScrollSurface::VirtualList(view.music.collections.list.state.clone());
                if let Some(target) = surface.offset_for_key(
                    &event.keystroke.key,
                    view.music.collections.list.layout.height(),
                ) {
                    view.set_scrollbar_offset(
                        &surface,
                        target,
                        Some(Self::library_scrolled),
                        window,
                        cx,
                    );
                    cx.stop_propagation();
                }
            }))
            .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
                if let Some(ActionData::NumericValue(value)) = data
                    && value.is_finite()
                {
                    let _ = view.update(cx, |view, cx| {
                        let surface =
                            ScrollSurface::VirtualList(view.music.collections.list.state.clone());
                        view.set_scrollbar_offset(
                            &surface,
                            px(*value as f32),
                            Some(Self::library_scrolled),
                            window,
                            cx,
                        );
                    });
                }
            })
    }

    pub(super) fn scroll_library_hero_to_top(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let list = &mut self.music.collections.list;
        list.pending = None;
        scroll_to_hero_top(ScrollSurface::VirtualList(list.state.clone()), window, cx);
        cx.notify();
    }

    fn library_item(
        &self,
        index: usize,
        (margin, width): (f32, f32),
        show_tabs: bool,
        group_hero_height: Option<Pixels>,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let wide = width >= WIDE_TABLE;
        let item = if let Some(index) = index.checked_sub(HEADER_ITEMS) {
            match self.music.collections.list.layout {
                Layout::Rows(_) => self.library_row(index, width, wide, window, cx),
                Layout::Grid(grid) => self
                    .collection_tiles(index, grid, window, cx)
                    .into_any_element(),
            }
        } else {
            self.library_header(width, wide, show_tabs, group_hero_height, window, cx)
                .into_any_element()
        };
        div().w_full().px(px(margin)).child(item).into_any_element()
    }

    fn library_row(
        &self,
        index: usize,
        width: f32,
        wide: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        let glass = self.uses_fluid_glass();
        let library = &self.music.collections;
        let row = &library.index.rows[library.results.visible[index]];
        let entry = row.entry(library.location.source.as_ref());
        let artwork_span = cover_span(("library", &entry.key));
        let row_span = cover_span(("library-row", &entry.key));
        if !library.list.reflow.exits.is_empty() {
            library.list.reflow.exits.forget(row_span);
        }
        let (row_motion, row_opacity) = self.collection_motion(row_span, Surface::Row, window, cx);
        let liked = entry.liked.then(|| entry.key.0.provider());
        let track = match &entry.action {
            EntryAction::Track(key, track) => Some((key, track)),
            _ => None,
        };
        let table = self.library_table(width, wide, track.is_some());
        let cols = table.columns;
        let inline_source = glass
            && cols.source.is_none()
            && self.track_column_visibility.shows(TrackColumn::Source);
        let downloaded = track.is_some() && row.downloaded;
        let marks = row
            .scopes()
            .map(Scope::label)
            .collect::<Vec<_>>()
            .join(", ");
        let album = track
            .and_then(|(_, track)| track.album.as_deref())
            .unwrap_or("");
        let duration = track
            .and_then(|(_, track)| track.duration)
            .map(|duration| time_label(f64::from(duration)))
            .unwrap_or_else(|| "—".into());
        let added = date_label(row.added);
        let subtitle = collection_subtitle(entry);
        let inline_attribution = [
            if track.is_some() && cols.artist.is_some() {
                ""
            } else {
                &subtitle
            },
            if track.is_some() && cols.album.is_none() {
                album
            } else {
                ""
            },
        ]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" · ");
        let description = [
            entry.title.as_str(),
            &subtitle,
            album,
            &marks,
            &added,
            &duration,
            if downloaded { "Скачано" } else { "" },
        ]
        .into_iter()
        .filter(|text| !text.is_empty() && *text != "—")
        .collect::<Vec<_>>()
        .join(" · ");
        let local_id = match &entry.action {
            EntryAction::LocalPlaylist { id, .. } => format!("local-playlist-{id}"),
            _ => format!("library-row-{index}"),
        };
        let order = 100 + index as isize * if glass { 4 } else { 3 };
        let play_download = downloaded && library.location.downloaded;
        let activation = if play_download {
            // A filtered row may display another platform's metadata. Its
            // downloaded member can start without opening a provider page.
            row.entry(None).action.clone()
        } else {
            entry.action.clone()
        };
        let title = entry.title.clone();
        let artwork = entry.artwork.clone();
        let current = glass && self.music_entry_is_current(&activation, cx);
        let high_contrast = self.fluid_high_contrast(cx);
        let row_style = track_row::Style::new(cx, high_contrast);
        let playable =
            matches!(&activation, EntryAction::Track(key, track) if can_resolve(key, track));
        let description = if playable {
            format!("Слушать · {description}")
        } else {
            description
        };
        let queue = self.music_queue_button(
            format!("studio-library-queue-{index}"),
            order + 3,
            &activation,
            cx,
        );
        use crate::ui::{components::entity_link, playlist::navigation};
        let title_style = div().min_w_0().truncate().when(glass, |title| {
            title
                .text_size(px(fluid::text::TRACK_TITLE))
                .text_color(row_style.title(current))
                .font_weight(if current {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
        });
        let edge = if glass { table.cover } else { COVER_WIDTH };
        let cover = if glass {
            self.collection_cover(entry, edge, COVER_RADIUS)
        } else {
            cover(artwork.as_deref(), edge, COVER_RADIUS, false)
        };
        let cover = cover
            .when(current, |cover| {
                cover.child(row_style.cover_marker(&self.player_snapshot.status))
            })
            .when(playable, |cover| {
                row_style.cover_play_preview(("library-cover-play", index), cover, edge)
            });
        let cover = if glass {
            painted_fragment(
                cover,
                artwork_span,
                self.collection_motion(artwork_span, Surface::Cover, window, cx)
                    .0,
            )
        } else {
            cover.into_any_element()
        };
        let album_link = |node: Div, key, track| {
            if let Some((name, target)) = navigation::album(key, track) {
                entity_link::text(
                    node,
                    &format!("library-track-album-{index}"),
                    name,
                    Some(target),
                    order,
                    cx,
                )
            } else {
                node.into_any_element()
            }
        };
        let row_button = button(
            SharedString::from(local_id),
            description,
            order + if glass { 0 } else { 2 },
        )
        .flex_1()
        .min_w_0()
        .h_full()
        .px(px(ROW_INSET))
        .gap(px(8.))
        .rounded(px(6.))
        .when(glass, |button| row_style.interaction(button))
        .when(current, |button| {
            button.aria_description(track_row::current_description(&self.player_snapshot.status))
        })
        .when(!glass, |button| {
            button.hover(|style| style.bg(rgb(ELEVATED)))
        })
        .when(playable, |button| button.cursor_pointer())
        .when(play_download && !current, |button| {
            button.aria_description("Слушать без сети")
        })
        .child(cover)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .child(title_style.child(entry.title.clone()))
                        .when(downloaded, |text| {
                            text.child(
                                div()
                                    .id(("library-download-mark", index))
                                    .flex_none()
                                    .tooltip(guise::tooltip("Скачано"))
                                    .child(
                                        Icon::new(IconName::Download)
                                            .pixel_size(px(12.))
                                            .color(secondary()),
                                    ),
                            )
                        }),
                )
                .when(!inline_attribution.is_empty(), |text| {
                    let body = div()
                        .text_size(px(if glass { fluid::text::CAPTION } else { 11. }))
                        .text_color(secondary())
                        .truncate();
                    text.child(if let Some((key, track)) = track {
                        body.flex()
                            .gap(px(7.))
                            .min_w_0()
                            .when(cols.artist.is_none(), |body| {
                                body.child(entity_link::artist_names(
                                    div(),
                                    &format!("library-track-artist-{index}"),
                                    key,
                                    track,
                                    order,
                                    cx,
                                ))
                            })
                            .when(cols.album.is_none() && !album.is_empty(), |body| {
                                body.child(album_link(div().truncate().min_w_0(), key, track))
                            })
                    } else {
                        body.child(inline_attribution)
                    })
                }),
        )
        .when_some(track, |button, (key, track)| {
            let size = if glass { fluid::text::BODY } else { 12. };
            button
                .children(cols.artist.map(|width| {
                    entity_link::artist_names(
                        column(width, "", size),
                        &format!("library-track-artist-{index}"),
                        key,
                        track,
                        order,
                        cx,
                    )
                }))
                .children(
                    cols.album
                        .map(|width| album_link(column(width, "", size), key, track)),
                )
        })
        .children(
            cols.source
                .map(|width| source_marks(row).flex_none().w(px(width)).overflow_hidden()),
        )
        .when(inline_source, |button| {
            button.children(track.map(|(key, _)| {
                let (mark, label) = match key.provider() {
                    Some(provider) => (
                        provider_mark(provider, 14.).into_any_element(),
                        provider.display_name(),
                    ),
                    None => (
                        mark("mediadeka.svg", px(14.)).into_any_element(),
                        "Локальные файлы",
                    ),
                };
                div()
                    .id(("library-inline-source", index))
                    .flex_none()
                    .tooltip(guise::tooltip(label))
                    .child(mark)
            }))
        })
        .children(
            cols.date
                .map(|width| column(width, added, fluid::text::CAPTION)),
        )
        .when(track.is_some(), |button| {
            button.child(
                column(cols.duration, duration, fluid::text::CAPTION)
                    .when(glass, |cell| cell.text_right()),
            )
        })
        .on_activate_at(cx.listener(move |view, origin, window, cx| {
            if let EntryAction::Track(key, track) = &activation {
                if playable {
                    view.play_from_fluid_row(*origin, window, cx, |view, _, cx| {
                        view.play_library_track(key, track, cx);
                    });
                }
            } else {
                view.navigate_from_artwork(artwork_span, window, cx, |view, window, cx| {
                    view.activate_library_entry(
                        &activation,
                        &title,
                        artwork.as_deref(),
                        window,
                        cx,
                    );
                });
            }
        }));
        let likes = track.map(|(key, track)| {
            div()
                .flex()
                .items_center()
                .w(px(LIKE_WIDTH))
                .flex_none()
                .child(self.like_control_with_mark(
                    SharedString::from(format!("library-like-{index}")),
                    order + if glass { 1 } else { 0 },
                    key,
                    track,
                    liked.as_slice(),
                    cx,
                ))
                .child(self.playlist_menu_trigger(
                    SharedString::from(format!("library-track-menu-{index}")),
                    format!("Действия с треком · {}", track.title),
                    IconName::Ellipsis,
                    order + if glass { 2 } else { 1 },
                    MenuKind::LibraryTrack(index),
                    cx,
                ))
        });
        let (before, after) = if glass { (None, likes) } else { (likes, None) };
        let row = div()
            .relative()
            .when(playable, |row| row.group(track_row::HOVER_GROUP))
            .flex()
            .items_center()
            .h(library.list.layout.height())
            .w_full()
            .opacity(row_opacity)
            .when(glass, |row| row.max_w(px(table.width)))
            .border_b_1()
            .border_color(if glass {
                gpui::Hsla::from(rgb(fluid::RULE)).alpha(0.40)
            } else {
                surface_border().alpha(0.55)
            })
            .children(before)
            .child(row_button)
            .children(after)
            .when(glass && track.is_some(), |row| {
                row.child(div().w(px(QUEUE_CONTROL)).flex_none().children(queue))
            })
            .children(self.prefetch_visibility_probe(entry_target(entry)));
        let row = row_style.surface(row, current);
        if glass {
            painted_fragment(row, row_span, row_motion)
        } else {
            row.into_any_element()
        }
    }

    pub(in crate::ui) fn library_recording(&self, index: usize) -> Option<QueueEntry> {
        let library = &self.music.collections;
        let row = library
            .index
            .rows
            .get(*library.results.visible.get(index)?)?;
        let entry = row.entry(library.location.source.as_ref());
        let EntryAction::Track(key, track) = &entry.action else {
            return None;
        };
        Some(super::super::detail::object_entry(key, index, track))
    }

    pub(in crate::ui::music) fn activate_library_entry(
        &mut self,
        action: &EntryAction,
        title: &str,
        artwork: Option<&str>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            EntryAction::Track(key, track) => {
                if let Some(target) = crate::ui::playlist::navigation::track_target(key, track) {
                    self.open_music_target(target, window, cx);
                } else {
                    self.play_library_track(key, track, cx);
                }
            }
            EntryAction::Object {
                profile,
                object,
                item,
            } if object.kind == crate::music::ObjectKind::Playlist => {
                self.open_music_item(object.provider, Some(profile), item.clone(), window, cx);
            }
            EntryAction::Object {
                profile, object, ..
            } => self.open_music_target(
                MusicTarget {
                    object: object.clone(),
                    profile: Some(profile.clone()),
                    title: title.to_owned(),
                    artwork: artwork.map(str::to_owned),
                },
                window,
                cx,
            ),
            EntryAction::LocalPlaylist { id, .. } => {
                self.show_playlist(Target::local(*id, title.to_owned()), window, cx);
            }
            EntryAction::LocalGroup(group) => self.open_library_group(group.clone(), window, cx),
        }
    }

    pub(in crate::ui::music) fn play_library_track(
        &mut self,
        key: &crate::playlist::PlaylistKey,
        track: &crate::playlist::PlaylistTrack,
        cx: &mut Context<Self>,
    ) {
        if !self.play_local_album_track(track, cx) {
            self.play_music_tracks([(key.clone(), track)], PlaybackAction::Play, cx);
        }
    }
}

pub(super) fn date_label(date: Option<i64>) -> String {
    date.and_then(|value| chrono::DateTime::from_timestamp(value, 0))
        .map(|value| value.format("%d.%m.%Y").to_string())
        .unwrap_or_else(|| "—".into())
}

fn column(width: f32, text: impl Into<SharedString>, size: f32) -> Div {
    div()
        .w(px(width))
        .flex_none()
        .truncate()
        .text_size(px(size))
        .text_color(secondary())
        .child(text.into())
}

pub(super) fn source_marks(row: &LibraryRow) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.))
        .children(row.scopes().map(|source| match source {
            Scope::Local => mark("mediadeka.svg", px(14.)).into_any_element(),
            Scope::Provider { provider, .. } => provider_mark(*provider, 14.).into_any_element(),
        }))
}

pub(super) struct Table {
    width: f32,
    cover: f32,
    columns: TrackColumns,
}

pub(super) fn columns(table: Table, tracks: bool, glass: bool) -> Div {
    let cols = table.columns;
    let inset = ROW_INSET + if glass { BUTTON_BORDER } else { 0. };
    let action_width = if tracks && glass {
        LIKE_WIDTH + QUEUE_CONTROL
    } else {
        0.
    };
    let caption = fluid::text::CAPTION;
    div()
        .h(px(COLUMN_HEADER_HEIGHT))
        .w_full()
        .when(glass, |row| row.max_w(px(table.width)))
        .flex()
        .items_center()
        .gap(px(8.))
        .pl(px(inset))
        .pr(px(inset + action_width))
        .when(tracks && !glass, |row| {
            row.child(div().w(px(LIKE_WIDTH)).flex_none())
        })
        .child(div().w(px(table.cover)).flex_none())
        .child(
            div()
                .min_w_0()
                .flex_1()
                .text_size(px(caption))
                .text_color(secondary())
                .child("Название"),
        )
        .when(tracks, |row| {
            row.children(
                cols.artist
                    .map(|width| column(width, "Исполнитель", caption)),
            )
            .children(cols.album.map(|width| column(width, "Альбом", caption)))
        })
        .children(cols.source.map(|width| column(width, "Площадки", caption)))
        .children(cols.date.map(|width| column(width, "Добавлено", caption)))
        .when(tracks, |row| {
            row.child(column(cols.duration, "Время", caption).when(glass, |cell| cell.text_right()))
        })
}

fn group_column_top(state: &ListState) -> Option<Pixels> {
    state
        .bounds_for_item(0)
        .map(|bounds| bounds.bottom() - state.viewport_bounds().top() - px(COLUMN_HEADER_HEIGHT))
}

#[cfg(feature = "capture")]
impl MainView {
    #[cfg(test)]
    pub(in crate::ui) fn library_scrollbar_for_test(
        &self,
        window: &Window,
    ) -> Option<(gpui::Bounds<Pixels>, Pixels)> {
        ScrollSurface::VirtualList(self.music.collections.list.state.clone())
            .track_for_capture(ScrollChrome::new(self.uses_fluid_glass(), window))
    }

    pub fn first_library_row_for_capture(&self) -> Option<gpui::Point<Pixels>> {
        self.music
            .collections
            .list
            .state
            .bounds_for_item(HEADER_ITEMS)
            .map(|bounds| bounds.center())
    }

    pub fn reveal_album_rows_for_capture(&mut self, rows: bool, cx: &mut Context<Self>) {
        let library = &mut self.music.collections;
        library.reveal();
        if rows {
            library.list.state.scroll_to(ListOffset {
                item_ix: HEADER_ITEMS,
                offset_in_item: Pixels::ZERO,
            });
        }
        cx.notify();
    }

    pub fn liked_viewport_for_capture(&self) -> gpui::Bounds<Pixels> {
        self.music.collections.list.state.viewport_bounds()
    }

    pub fn library_scroll_bounds_for_capture(&self) -> (gpui::Bounds<Pixels>, f32, f32) {
        let surface = ScrollSurface::VirtualList(self.music.collections.list.state.clone());
        (
            surface.viewport(),
            surface.offset().into(),
            surface.extent().into(),
        )
    }

    pub fn liked_audit_for_capture(&self) -> serde_json::Value {
        let library = &self.music.collections;
        let offset = library.list.state.logical_scroll_top();
        let bounds = library.list.state.viewport_bounds();
        serde_json::json!({
            "pending": self.music.request.is_some(),
            "sources_with_errors": library.errors.keys().map(|provider| provider.as_str()).collect::<Vec<_>>(),
            "tracks": library.results.visible.len(),
            "downloaded": library.results.downloaded,
            "first_added": library.results.first_added,
            "counts": library.results.counts.iter().map(|(source, count)|
                (source.map_or("local", crate::providers::ProviderId::as_str), *count)
            ).collect::<std::collections::BTreeMap<_, _>>(),
            "query": library.location.query,
            "source": library.location.source.as_ref().map(Scope::label),
            "only_downloaded": library.location.downloaded,
            "sort": library.location.sort.label(),
            "album_view": (self.music_page() == Page::Albums).then_some(match library.list.layout {
                Layout::Rows(_) => "list",
                Layout::Grid(_) => "tiles",
            }),
            "columns": library.list.layout.columns(),
            "first_item": offset.item_ix,
            "offset_in_item_dip": f32::from(offset.offset_in_item),
            "viewport_width_dip": f32::from(bounds.size.width),
            "viewport_height_dip": f32::from(bounds.size.height),
        })
    }
}
