use std::time::Duration;

use guise::IconName;

use crate::ui::{
    fluid_glass::tokens as fluid,
    theme::playlist::metrics::{CONTROL, CONTROL_SMALL, GAP_SMALL},
};

use super::super::{LibraryTab, Page, TOOLBAR_HEIGHT};

#[derive(Clone, Copy)]
pub(super) struct Destination {
    pub(super) page: Page,
    pub(super) id: &'static str,
    pub(super) label: &'static str,
    pub(super) glyph: IconName,
}

impl Destination {
    const fn new(page: Page, id: &'static str, label: &'static str, glyph: IconName) -> Self {
        Self {
            page,
            id,
            label,
            glyph,
        }
    }

    const fn library(tab: LibraryTab, id: &'static str, glyph: IconName) -> Self {
        Self::new(tab.page(), id, tab.label(), glyph)
    }
}

pub(super) const PRIMARY: [Destination; 3] = [
    Destination::new(Page::Home, "home-page", "Главная", IconName::House),
    Destination::new(Page::Search, "home-search-page", "Поиск", IconName::Search),
    Destination::new(Page::History, "home-history", "История", IconName::History),
];
const PLAYLISTS: Destination =
    Destination::library(LibraryTab::Playlists, "home-playlists", IconName::ListMusic);
pub(super) const SETTINGS: Destination = Destination::new(
    Page::Settings,
    "home-settings",
    "Настройки",
    IconName::Settings,
);
const CLASSIC_LIBRARY: [Destination; 2] = [
    Destination::new(Page::Tracks, "home-liked", "Медиатека", IconName::Library),
    PLAYLISTS,
];
const COMPACT_LIBRARY: [Destination; 2] = [
    Destination {
        id: "studio-library",
        ..CLASSIC_LIBRARY[0]
    },
    PLAYLISTS,
];
const FULL_LIBRARY: [Destination; 4] = [
    Destination::library(LibraryTab::Tracks, "studio-library", IconName::Music2),
    Destination::library(LibraryTab::Albums, "studio-library-albums", IconName::Disc3),
    Destination::library(
        LibraryTab::Artists,
        "studio-library-artists",
        IconName::Users,
    ),
    PLAYLISTS,
];

#[derive(Clone, Copy, Eq, PartialEq)]
pub(in crate::ui::workspace) enum SidebarLayout {
    Classic,
    FluidDense,
    FluidShort,
    FluidFull,
}

impl SidebarLayout {
    pub(super) fn library(self) -> &'static [Destination] {
        match self {
            Self::Classic => &CLASSIC_LIBRARY,
            Self::FluidDense | Self::FluidShort => &COMPACT_LIBRARY,
            Self::FluidFull => &FULL_LIBRARY,
        }
    }

    pub(super) fn current(self, page: Page) -> Page {
        match page {
            Page::Albums | Page::Artists if self != Self::FluidFull => Page::Tracks,
            Page::Playlist => Page::Playlists,
            page => page,
        }
    }

    pub(super) fn titlebar_height(self) -> f32 {
        if self == Self::Classic {
            TOOLBAR_HEIGHT
        } else {
            fluid::TITLEBAR
        }
    }

    pub(super) fn row(self) -> f32 {
        match self {
            Self::FluidFull => fluid::NAV_ROW,
            Self::FluidDense => fluid::NAV_DENSE_ROW,
            Self::Classic | Self::FluidShort => CONTROL,
        }
    }

    pub(super) fn gap(self) -> f32 {
        match self {
            Self::FluidFull => fluid::NAV_GAP,
            Self::FluidDense => fluid::NAV_DENSE_GAP,
            Self::Classic | Self::FluidShort => GAP_SMALL,
        }
    }

    pub(super) fn separator_height(self) -> f32 {
        match self {
            Self::FluidDense | Self::FluidShort => fluid::NAV_SHORT_SEPARATOR,
            Self::Classic | Self::FluidFull => CONTROL_SMALL,
        }
    }

    pub(super) fn selection_offset(self, page: Page) -> Option<f32> {
        let page = self.current(page);
        let stride = self.row() + self.gap();
        if let Some(index) = PRIMARY.iter().position(|link| link.page == page) {
            return Some(index as f32 * stride);
        }
        let index = self.library().iter().position(|link| link.page == page)?;
        Some((PRIMARY.len() + index) as f32 * stride + self.separator_height() + self.gap())
    }

    pub(super) fn duration(self) -> Duration {
        if self == Self::Classic {
            Duration::from_millis(160)
        } else {
            fluid::NAV_DURATION
        }
    }
}
