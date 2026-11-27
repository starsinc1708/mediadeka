mod combined;

use gpui::{
    AnyElement, Context, Div, FontWeight, IntoElement, Role, Stateful, div, prelude::*, px, rgb,
};
use guise::{Icon, IconName};

use crate::providers::{
    Home, HomeItem, HomeSection, HomeSectionPage, HomeSectionStyle, HomeTab, ProviderId,
};
use crate::ui::theme::{
    SHELF_ARROW, SHELF_ARROW_ACTIVE, SHELF_ARROW_DISABLED, SHELF_ARROW_HOVER, SHELF_TAB,
    SHELF_TAB_ACTIVE, SHELF_TAB_ARTWORK, SHELF_TAB_BORDER, SHELF_TAB_HOVER, SHELF_TAB_SELECTED,
    SHELF_TAB_SELECTED_BORDER, navigation_accent as accent, secondary, surface_border,
};
use crate::ui::{
    components::paint_section_tab,
    design_system::controls::{ControlState, ControlVariant},
    home::shelf_motion::keep_shelf_moving,
};

use super::{
    Activate, FeedContext, ItemStyle, Layout, MainView, artwork, button, section, section_items,
    section_title, track_item,
};

const CARD_GAP: f32 = 12.;
const COMPACT_TABBED_CARD_WIDTH: f32 = 132.;
const SHELF_TITLE_HEIGHT: f32 = 32.;
const SHELF_GAP: f32 = 10.;
#[cfg(feature = "capture")]
const ACTIVITY_GAP: f32 = 9.;
const QUICK_TILE_HEIGHT: f32 = 54.;
const QUICK_ROW_GAP: f32 = 9.;
const SHELF_OVERSCAN: f32 = 340.;
const TAB_HEIGHT: f32 = 34.;
const TAB_GAP: f32 = 7.;
const TAB_MIN_WIDTH: f32 = 104.;
const TAB_MAX_WIDTH: f32 = 174.;

#[cfg(feature = "capture")]
pub type HomeShelfGeometryForCapture = (usize, String, f32, Vec<(usize, f32, f32)>);
#[cfg(feature = "capture")]
pub type SoundcloudHomeTopsForCapture = (Vec<(String, f32)>, Option<f32>, Vec<(String, f32)>);

struct PositionedShelf<'a> {
    shelf: Shelf<'a>,
    page: ShelfPage,
    tabs: TabLayout,
    top: f32,
    height: f32,
}

#[derive(Clone, Copy)]
pub(super) enum ShelfSurface {
    Home,
    SoundCloudHome,
    SoundCloudLibrary,
    Combined,
}

impl ShelfSurface {
    fn origin_slot(self) -> usize {
        usize::from(matches!(self, Self::SoundCloudLibrary))
    }

    fn shows(self, page: HomeSectionPage) -> bool {
        match self {
            Self::Home | Self::SoundCloudHome => page == HomeSectionPage::Home,
            Self::SoundCloudLibrary => page == HomeSectionPage::Library,
            Self::Combined => true,
        }
    }
}

impl MainView {
    fn shelf_origin(&self, surface: ShelfSurface) -> f32 {
        self.home.shelf_origins[surface.origin_slot()].get()
    }

    fn track_shelf_origin(&self, surface: ShelfSurface) -> impl gpui::IntoElement {
        self.track_home_section_origin(self.home.shelf_origins[surface.origin_slot()].clone())
    }

    #[cfg(feature = "capture")]
    pub fn home_shelf_origin_for_capture(&self) -> f32 {
        self.shelf_origin(ShelfSurface::Home)
    }

    pub(super) fn feed(
        &self,
        collection: Option<&Home>,
        source: FeedContext<'_>,
        surface: ShelfSurface,
        layout: Layout,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let (id, title) = match surface {
            ShelfSurface::Home => ("home-feed", "Главная площадки"),
            ShelfSurface::SoundCloudHome => ("home-feed", "Главная SoundCloud"),
            ShelfSurface::SoundCloudLibrary => ("home-library-content", "Медиатека SoundCloud"),
            ShelfSurface::Combined => ("home-feed", "Подборки площадок"),
        };
        let body = section()
            .relative()
            .id(id)
            .accessibility_id(id)
            .role(Role::Group)
            .aria_label(title)
            .gap(px(shelf_gap(layout)))
            .child(self.track_shelf_origin(surface));
        let Some(home) = collection else {
            return body;
        };
        let mut shelves = self
            .positioned_shelves(home, source.provider, surface, layout)
            .peekable();
        if shelves.peek().is_none() {
            return body;
        }
        let scroll_top = -f32::from(self.scroll.offset().y) - self.shelf_origin(surface);
        let viewport_height = f32::from(self.scroll.bounds().size.height);
        body.children(shelves.map(|placed| {
            let PositionedShelf {
                shelf,
                page,
                tabs,
                top,
                height,
            } = placed;
            // Lightweight spacers preserve the same scroll extent as the visible shelves.
            let nearby = home.sections.is_empty()
                || viewport_height <= 0.
                || (top + height >= scroll_top - SHELF_OVERSCAN
                    && top <= scroll_top + viewport_height + SHELF_OVERSCAN);
            let child = if nearby {
                self.render_shelf(source, &shelf, page, tabs, cx)
            } else {
                div().w_full()
            };
            child.h(px(height)).flex_none()
        }))
    }

    fn positioned_shelves<'a>(
        &self,
        home: &'a Home,
        provider: ProviderId,
        surface: ShelfSurface,
        layout: Layout,
    ) -> impl Iterator<Item = PositionedShelf<'a>> {
        let flat = (matches!(surface, ShelfSurface::Home | ShelfSurface::Combined)
            && home.sections.is_empty()
            && !home.items.is_empty())
        .then_some(Shelf {
            index: 0,
            title: &home.title,
            first: 0,
            items: &home.items,
            active_tab: None,
            tabs: &[],
            quick: false,
            style: HomeSectionStyle::Cards,
        });
        let grouped = home
            .sections
            .iter()
            .enumerate()
            .filter(move |(_, group)| surface.shows(group.page))
            .filter_map(move |(index, group)| {
                Shelf::new(
                    home,
                    index,
                    group,
                    self.home.shelf_tabs.get(&(provider, index)).copied(),
                )
            });
        flat.into_iter().chain(grouped).scan(0., move |top, shelf| {
            let page = self.shelf_page(provider, &shelf, layout);
            let tabs = TabLayout::new(layout.content_width, shelf.valid_tabs().count());
            let height = shelf.height(page, tabs);
            let positioned = PositionedShelf {
                shelf,
                page,
                tabs,
                top: *top,
                height,
            };
            *top += height + shelf_gap(layout);
            Some(positioned)
        })
    }

    #[cfg(feature = "capture")]
    pub fn home_shelf_geometry_for_capture(
        &self,
        viewport: gpui::Size<gpui::Pixels>,
    ) -> Vec<HomeShelfGeometryForCapture> {
        use crate::ui::workspace::{Page, WorkspaceLayout};
        if self.music_page() != Page::Home {
            return Vec::new();
        }
        let surface = if self.soundcloud_active() {
            ShelfSurface::SoundCloudHome
        } else {
            ShelfSurface::Home
        };
        let Some((home, provider)) = self
            .home_snapshot_for_capture()
            .zip(self.home_provider_for_capture())
        else {
            return Vec::new();
        };
        self.positioned_shelves(
            home,
            provider,
            surface,
            Layout::new(
                WorkspaceLayout::new(f32::from(viewport.width)),
                viewport.height,
                None,
            ),
        )
        .map(|placed| {
            let tab_centers = placed
                .shelf
                .valid_tabs()
                .enumerate()
                .map(|(position, (index, _))| {
                    let column = position % placed.tabs.columns;
                    let row = position / placed.tabs.columns;
                    let x = column as f32 * (placed.tabs.width + TAB_GAP) + placed.tabs.width / 2.;
                    let y = SHELF_TITLE_HEIGHT
                        + SHELF_GAP
                        + row as f32 * (TAB_HEIGHT + TAB_GAP)
                        + TAB_HEIGHT / 2.;
                    (index, x, y)
                })
                .collect();
            (
                placed.shelf.index,
                placed.shelf.title.to_owned(),
                placed.top,
                tab_centers,
            )
        })
        .collect()
    }

    /// Absolute shelf offsets of the SoundCloud home, where recommendations,
    /// the subscription feed and the library shelves share one page.
    #[cfg(feature = "capture")]
    pub fn soundcloud_home_tops_for_capture(
        &self,
        viewport: gpui::Size<gpui::Pixels>,
    ) -> SoundcloudHomeTopsForCapture {
        use crate::ui::components::music::TRACK_ROW_HEIGHT;
        use crate::ui::workspace::WorkspaceLayout;
        let Some(home) = self.home_snapshot_for_capture() else {
            return (Vec::new(), None, Vec::new());
        };
        let layout = Layout::new(
            WorkspaceLayout::new(f32::from(viewport.width)),
            viewport.height,
            None,
        );
        let gap = shelf_gap(layout);
        let tops = |surface| {
            let mut tops = Vec::new();
            let mut end = 0f32;
            for placed in self.positioned_shelves(home, ProviderId::SoundCloud, surface, layout) {
                tops.push((placed.shelf.title.to_owned(), placed.top));
                end = placed.top + placed.height;
            }
            (tops, end)
        };
        let (home_tops, home_end) = tops(ShelfSurface::SoundCloudHome);
        let activity_rows = if home.sections.is_empty() {
            home.items.len()
        } else {
            home.sections
                .iter()
                .filter(|section| section.page == HomeSectionPage::Activity)
                .filter_map(|section| section_items(home, section))
                .flatten()
                .count()
        };
        let activity = super::soundcloud_activity_visible(home).then(|| {
            let height =
                SHELF_TITLE_HEIGHT + activity_rows as f32 * (TRACK_ROW_HEIGHT + ACTIVITY_GAP);
            (home_end + gap, height)
        });
        let library_start = home_end + gap + activity.map_or(0., |(_, height)| height + gap);
        let (mut library_tops, _) = tops(ShelfSurface::SoundCloudLibrary);
        for (_, top) in &mut library_tops {
            *top += library_start;
        }
        (home_tops, activity.map(|(top, _)| top), library_tops)
    }

    #[cfg(feature = "capture")]
    pub fn home_shelf_selected_tab_for_capture(&self, shelf: usize) -> Option<usize> {
        let home = self.home_snapshot_for_capture()?;
        let group = home.sections.get(shelf)?;
        Shelf::new(
            home,
            shelf,
            group,
            self.home
                .shelf_tabs
                .get(&(self.home_provider_for_capture()?, shelf))
                .copied(),
        )?
        .active_tab
    }

    #[cfg(feature = "capture")]
    fn home_snapshot_for_capture(&self) -> Option<&Home> {
        use crate::state::HomeContent;
        match &self.state.home_view()?.0.content {
            HomeContent::Fresh(snapshot)
            | HomeContent::Partial { snapshot, .. }
            | HomeContent::Cached { snapshot, .. } => Some(&snapshot.home),
            HomeContent::Loading | HomeContent::Empty(_) => None,
        }
    }

    #[cfg(feature = "capture")]
    fn home_provider_for_capture(&self) -> Option<ProviderId> {
        self.state
            .home_view()
            .map(|(home, _)| super::super::FeedSource::active(home).provider)
    }

    fn shelf_page(&self, provider: ProviderId, shelf: &Shelf<'_>, layout: Layout) -> ShelfPage {
        let (columns, card_width, style) = if shelf.quick {
            (layout.quick_columns, layout.quick_width, ItemStyle::Quick)
        } else {
            let (columns, card_width, style) = match shelf.style {
                HomeSectionStyle::Cards => (layout.columns, layout.card_width, ItemStyle::Card),
                HomeSectionStyle::Artists => (layout.columns, layout.card_width, ItemStyle::Artist),
                HomeSectionStyle::Releases => {
                    let columns = if layout.columns >= 7 { 4 } else { 3 };
                    (
                        columns,
                        tile_width(layout, columns, 278.),
                        ItemStyle::Release,
                    )
                }
                HomeSectionStyle::Collections => {
                    let columns = if layout.compact { 4 } else { 5 };
                    (
                        columns,
                        tile_width(layout, columns, 157.),
                        ItemStyle::Collection,
                    )
                }
            };
            if layout.short {
                (layout.columns, layout.card_width, style)
            } else if layout.compact && !shelf.tabs.is_empty() {
                let columns = (((layout.content_width + CARD_GAP)
                    / (COMPACT_TABBED_CARD_WIDTH + CARD_GAP))
                    .ceil() as usize)
                    .max(1);
                (
                    columns,
                    tile_width(layout, columns, COMPACT_TABBED_CARD_WIDTH),
                    style,
                )
            } else {
                (columns, card_width, style)
            }
        };
        let size = if shelf.quick { columns * 2 } else { columns };
        let items = shelf.item_count();
        let count = items.div_ceil(size);
        let position = self
            .home
            .shelf_pages
            .get(&(provider, shelf.index))
            .copied()
            .unwrap_or(0)
            .min(count.saturating_sub(1));
        ShelfPage {
            glass: layout.glass,
            width: columns as f32 * card_width + columns.saturating_sub(1) as f32 * CARD_GAP,
            position,
            count,
            size,
            columns,
            card_width,
            style,
            visible: (items - position * size).min(size),
        }
    }

    fn render_shelf(
        &self,
        source: FeedContext<'_>,
        shelf: &Shelf<'_>,
        page: ShelfPage,
        tabs: TabLayout,
        cx: &Context<Self>,
    ) -> Div {
        let selected = page.position;
        let position =
            self.shelf_turn_position((source.provider, shelf.index), (page.size, page.count), cx);
        let rows = if let Some(position) = position {
            let first = position.floor() as usize;
            let last = (position.ceil() as usize).min(page.count.saturating_sub(1));
            div()
                .relative()
                .w(px(page.width))
                .min_w_0()
                .h(px(shelf.rows_height(page)))
                .flex_none()
                .overflow_hidden()
                .children((first..=last).map(|index| {
                    div()
                        .absolute()
                        .top_0()
                        .left(px((index as f32 - position) * (page.width + CARD_GAP)))
                        .w_full()
                        .child(self.render_shelf_rows(
                            source,
                            shelf,
                            page,
                            (index, index == selected),
                            cx,
                        ))
                }))
                .on_children_prepainted(|bounds, window, _| {
                    if bounds
                        .iter()
                        .any(|bounds| window.content_mask().bounds.intersects(bounds))
                    {
                        keep_shelf_moving(window);
                    }
                })
                .into_any_element()
        } else {
            self.render_shelf_rows(source, shelf, page, (selected, true), cx)
        };
        section()
            .gap(px(SHELF_GAP))
            .child(self.shelf_heading(source, shelf, page, cx))
            .when(tabs.rows > 0, |body| {
                body.child(shelf_tabs(source, shelf, tabs, page.glass, cx))
            })
            .child(rows)
    }

    fn render_shelf_rows(
        &self,
        source: FeedContext<'_>,
        shelf: &Shelf<'_>,
        page: ShelfPage,
        (position, interactive): (usize, bool),
        cx: &Context<Self>,
    ) -> AnyElement {
        let cards = shelf.items().skip(position * page.size).take(page.size);
        let card = |index, item| card(self, source, index, item, (page, interactive), cx);
        if shelf.quick {
            let cards: Vec<_> = cards.collect();
            let id = if !interactive {
                format!("{}-{position}", source.id("shelf-outgoing", shelf.index))
            } else if source.combined {
                source.id("home-quick-picks", shelf.index)
            } else {
                "home-quick-picks".into()
            };
            section()
                .gap(px(QUICK_ROW_GAP))
                .id(id.clone())
                .when(interactive, |rows| {
                    rows.accessibility_id(id)
                        .role(Role::Group)
                        .aria_label("Быстрый выбор")
                })
                .children(cards.chunks(page.columns).map(|row| {
                    div()
                        .flex()
                        .gap(px(CARD_GAP))
                        .children(row.iter().map(|&(index, item)| card(index, item)))
                }))
                .into_any_element()
        } else {
            div()
                .flex()
                .gap(px(CARD_GAP))
                .children(cards.map(|(index, item)| card(index, item)))
                .into_any_element()
        }
    }

    fn shelf_heading(
        &self,
        source: FeedContext<'_>,
        shelf: &Shelf<'_>,
        page: ShelfPage,
        cx: &Context<Self>,
    ) -> Div {
        let id = shelf.index;
        div()
            .h(px(SHELF_TITLE_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(8.))
            .child(
                section_title(shelf.title.to_owned())
                    .id(source.id("feed-section", id))
                    .accessibility_id(source.id("home-section", id))
                    .role(Role::Heading)
                    .aria_label(shelf.title.to_owned()),
            )
            .when(page.count > 1, |header| {
                header.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(self.shelf_arrow(source, id, page, false, cx))
                        .child(self.shelf_arrow(source, id, page, true, cx)),
                )
            })
    }

    fn shelf_arrow(
        &self,
        source: FeedContext<'_>,
        shelf: usize,
        page: ShelfPage,
        next: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let glyph = |color| {
            Icon::new(if next {
                IconName::ChevronRight
            } else {
                IconName::ChevronLeft
            })
            .pixel_size(px(17.))
            .color(color)
        };
        let glass = page.glass;
        let enabled = if next {
            page.position + 1 < page.count
        } else {
            page.position > 0
        };
        let id = if next { "shelf-next" } else { "shelf-previous" };
        let description = if next {
            "Следующие записи"
        } else {
            "Предыдущие записи"
        };
        let key = (source.provider, shelf);
        let control = button(source.id(id, shelf), description, 28 + (next as isize))
            .size(px(32.))
            .flex_none()
            .justify_center()
            .rounded(px(9.));
        let control = if !enabled {
            control
                .tab_stop(false)
                .cursor_default()
                .aria_description("Недоступно")
                .a11y_synthetic_children(|tree| tree.parent_node().set_disabled())
                .on_any_mouse_down(|_, window, _| window.prevent_default())
                .when(glass.is_none(), |arrow| arrow.bg(rgb(SHELF_ARROW_DISABLED)))
                .child(glyph(secondary().alpha(0.42)))
        } else if let Some(material) = glass {
            let variant = ControlVariant::GhostIcon;
            let state = ControlState::default();
            variant
                .paint(control, material, state)
                .child(glyph(variant.resolve(material, state).fg))
        } else {
            control
                .border_color(surface_border())
                .bg(rgb(SHELF_ARROW))
                .hover(|style| style.bg(rgb(SHELF_ARROW_HOVER)))
                .active(|style| style.bg(rgb(SHELF_ARROW_ACTIVE)))
                .child(glyph(accent()))
        };
        let control = control.when(enabled, |arrow| {
            arrow.on_activate_at(cx.listener(
                move |this, origin: &Option<gpui::Point<gpui::Pixels>>, window, cx| {
                    let pointer = origin.is_some() && !window.last_input_was_keyboard();
                    this.turn_shelf(key, (page.size, page.count), next, pointer, cx);
                },
            ))
        });
        #[cfg(feature = "capture")]
        return crate::ui::music::detail::hero_motion::shared_artwork(
            control,
            crate::ui::home::shelf_motion::arrow_span(shelf, next),
            None,
        );
        #[cfg(not(feature = "capture"))]
        control.into_any_element()
    }

    #[cfg(feature = "capture")]
    pub fn home_shelf_arrow_bounds_for_capture(
        &self,
        window: &gpui::Window,
        shelf: usize,
        next: bool,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        window.rendered_span(crate::ui::home::shelf_motion::arrow_span(shelf, next))
    }
}

fn shelf_gap(layout: Layout) -> f32 {
    layout.section_gap()
}

fn tile_width(layout: Layout, columns: usize, max_width: f32) -> f32 {
    ((layout.content_width - CARD_GAP * (columns - 1) as f32) / columns as f32).min(max_width)
}

fn card(
    view: &MainView,
    source: FeedContext<'_>,
    index: usize,
    item: &HomeItem,
    (page, interactive): (ShelfPage, bool),
    cx: &Context<MainView>,
) -> Div {
    div()
        .w(px(page.card_width))
        .min_w_0()
        .flex_none()
        .child(track_item(
            view,
            source,
            index,
            item,
            (page.style, page.glass),
            interactive,
            cx,
        ))
}

fn shelf_tabs(
    source: FeedContext<'_>,
    shelf: &Shelf<'_>,
    layout: TabLayout,
    glass: Option<super::Material>,
    cx: &Context<MainView>,
) -> Stateful<Div> {
    let tabs: Vec<_> = shelf.valid_tabs().collect();
    section()
        .id(source.id("shelf-tabs", shelf.index))
        .role(Role::TabList)
        .aria_label(format!("Фильтры · {}", shelf.title))
        .gap(px(TAB_GAP))
        .flex_none()
        .children(tabs.chunks(layout.columns).map(|row| {
            div().flex().gap(px(TAB_GAP)).children(
                row.iter().map(|&(index, tab)| {
                    shelf_tab(source, shelf, index, tab, layout.width, glass, cx)
                }),
            )
        }))
}

fn shelf_tab(
    source: FeedContext<'_>,
    shelf: &Shelf<'_>,
    index: usize,
    tab: &HomeTab,
    width: f32,
    glass: Option<super::Material>,
    cx: &Context<MainView>,
) -> Stateful<Div> {
    let shelf_id = shelf.index;
    let selected = shelf.active_tab == Some(index);
    let id = format!("{}-tab-{index}", source.id("home-section", shelf_id));
    let key = (source.provider, shelf_id);
    let control = button(id, tab.title.clone(), 29)
        .role(Role::Tab)
        .aria_selected(selected)
        .w(px(width))
        .h(px(TAB_HEIGHT))
        .gap(px(6.))
        .px(px(8.))
        .flex_none();
    let control = if let Some(material) = glass {
        paint_section_tab(control, material, selected)
    } else {
        control
            .rounded(px(9.))
            .bg(rgb(if selected {
                SHELF_TAB_SELECTED
            } else {
                SHELF_TAB
            }))
            .border_color(rgb(if selected {
                SHELF_TAB_SELECTED_BORDER
            } else {
                SHELF_TAB_BORDER
            }))
            .hover(|style| style.bg(rgb(SHELF_TAB_HOVER)))
            .active(|style| style.bg(rgb(SHELF_TAB_ACTIVE)))
    };
    control
        .when_some(tab.artwork.as_deref(), |chip, url| {
            chip.child(
                div()
                    .relative()
                    .size(px(20.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .items_center()
                    .rounded_full()
                    .overflow_hidden()
                    .bg(rgb(SHELF_TAB_ARTWORK))
                    .child(
                        Icon::new(IconName::Headphones)
                            .pixel_size(px(12.))
                            .color(accent()),
                    )
                    .child(artwork::remote(url, true, false)),
            )
        })
        .child(
            div()
                .min_w_0()
                .truncate()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tab.title.clone()),
        )
        .on_activate(cx.listener(move |this, _, _, cx| {
            this.home.shelf_tabs.insert(key, index);
            this.home.shelf_pages.remove(&key);
            this.home.shelf_turns.remove(&key);
            cx.notify();
        }))
}

struct Shelf<'a> {
    index: usize,
    title: &'a str,
    first: usize,
    items: &'a [HomeItem],
    active_tab: Option<usize>,
    tabs: &'a [HomeTab],
    quick: bool,
    style: HomeSectionStyle,
}

impl<'a> Shelf<'a> {
    fn new(
        home: &'a Home,
        index: usize,
        group: &'a HomeSection,
        tab: Option<usize>,
    ) -> Option<Self> {
        let items = section_items(home, group)?;
        if items.is_empty() {
            return None;
        }
        let mut shelf = Self {
            index,
            title: &group.title,
            first: group.start,
            items,
            active_tab: None,
            tabs: &group.tabs,
            quick: group.quick,
            style: group.style,
        };
        shelf.active_tab = tab
            .filter(|&index| shelf.tabs.get(index).is_some_and(|tab| shelf.supports(tab)))
            .or_else(|| shelf.valid_tabs().next().map(|(index, _)| index));
        Some(shelf)
    }

    fn supports(&self, tab: &HomeTab) -> bool {
        tab.indices.iter().any(|&index| self.item(index).is_some())
    }

    fn valid_tabs(&self) -> impl Iterator<Item = (usize, &'a HomeTab)> {
        self.tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| self.supports(tab))
    }

    fn item(&self, index: usize) -> Option<&'a HomeItem> {
        let position = index.checked_sub(self.first)?;
        self.items.get(position)
    }

    fn items(&self) -> impl Iterator<Item = (usize, &'a HomeItem)> {
        let tab = self.active_tab.and_then(|index| self.tabs.get(index));
        (0..tab.map_or(self.items.len(), |tab| tab.indices.len())).filter_map(move |position| {
            let index = tab.map_or_else(|| self.first + position, |tab| tab.indices[position]);
            self.item(index).map(|item| (index, item))
        })
    }

    fn item_count(&self) -> usize {
        if self.active_tab.is_some() {
            self.items().count()
        } else {
            self.items.len()
        }
    }

    fn height(&self, page: ShelfPage, tabs: TabLayout) -> f32 {
        SHELF_TITLE_HEIGHT + SHELF_GAP + self.rows_height(page) + tabs.height()
    }

    fn rows_height(&self, page: ShelfPage) -> f32 {
        if self.quick {
            let visible = if page.glass.is_some() && page.count > 1 {
                page.size
            } else {
                page.visible
            };
            let rows = visible.div_ceil(page.columns);
            rows as f32 * QUICK_TILE_HEIGHT + rows.saturating_sub(1) as f32 * QUICK_ROW_GAP
        } else {
            page.card_width + page.style.label_height()
        }
    }
}

#[derive(Clone, Copy)]
struct ShelfPage {
    glass: Option<super::Material>,
    width: f32,
    position: usize,
    count: usize,
    size: usize,
    columns: usize,
    card_width: f32,
    style: ItemStyle,
    visible: usize,
}

#[derive(Clone, Copy)]
struct TabLayout {
    columns: usize,
    rows: usize,
    width: f32,
}

impl TabLayout {
    fn new(available: f32, tabs: usize) -> Self {
        let columns = (((available + TAB_GAP) / (TAB_MIN_WIDTH + TAB_GAP)) as usize)
            .max(1)
            .min(tabs.max(1));
        let width =
            ((available - TAB_GAP * (columns - 1) as f32) / columns as f32).min(TAB_MAX_WIDTH);
        Self {
            columns,
            rows: tabs.div_ceil(columns),
            width,
        }
    }

    fn height(self) -> f32 {
        if self.rows == 0 {
            0.
        } else {
            SHELF_GAP + self.rows as f32 * TAB_HEIGHT + (self.rows - 1) as f32 * TAB_GAP
        }
    }
}
