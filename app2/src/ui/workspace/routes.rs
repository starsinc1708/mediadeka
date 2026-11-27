use std::time::Instant;

use crate::{
    music::{
        ObjectKind, ObjectRef,
        library::{EntryKey, LibraryGroup, LibrarySort, Scope},
    },
    playlist::{PlaylistKey, PlaylistTrack},
    providers::{HomeItem, ProviderId},
};
use gpui::{ListOffset, Pixels};
use serde::{Deserialize, Serialize};

const NAVIGATION_HISTORY_LIMIT: usize = 256;

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub(in crate::ui) enum Page {
    #[default]
    Home,
    Search,
    History,
    Tracks,
    Albums,
    Artists,
    Playlists,
    Playlist,
    Object,
    Station,
    Settings,
}

#[derive(Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub(in crate::ui) enum LibraryTab {
    Tracks,
    Albums,
    Artists,
    Playlists,
}

impl LibraryTab {
    pub(in crate::ui) const ALL: [Self; 4] =
        [Self::Tracks, Self::Albums, Self::Artists, Self::Playlists];

    pub(in crate::ui) const fn page(self) -> Page {
        match self {
            Self::Tracks => Page::Tracks,
            Self::Albums => Page::Albums,
            Self::Artists => Page::Artists,
            Self::Playlists => Page::Playlists,
        }
    }

    pub(in crate::ui) const fn id(self) -> &'static str {
        match self {
            Self::Tracks => "tracks",
            Self::Albums => "albums",
            Self::Artists => "artists",
            Self::Playlists => "playlists",
        }
    }

    pub(in crate::ui) const fn label(self) -> &'static str {
        match self {
            Self::Tracks => "Треки",
            Self::Albums => "Альбомы",
            Self::Artists => "Исполнители",
            Self::Playlists => "Плейлисты",
        }
    }
}

impl Page {
    pub(in crate::ui) const fn library_tab(self) -> Option<LibraryTab> {
        match self {
            Self::Tracks => Some(LibraryTab::Tracks),
            Self::Albums => Some(LibraryTab::Albums),
            Self::Artists => Some(LibraryTab::Artists),
            Self::Playlists => Some(LibraryTab::Playlists),
            _ => None,
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(in crate::ui) struct Target {
    pub(in crate::ui) key: PlaylistKey,
    pub(in crate::ui) name: String,
    pub(in crate::ui) artwork: Option<String>,
    pub(in crate::ui) link: Option<String>,
}

impl Target {
    pub(in crate::ui) fn from_item(
        provider: ProviderId,
        profile: Option<&str>,
        id: String,
        item: HomeItem,
    ) -> Self {
        Self {
            key: PlaylistKey::Source {
                provider,
                profile: profile.map(str::to_owned),
                id,
            },
            name: item.title,
            artwork: item.artwork,
            link: item.url,
        }
    }

    pub(in crate::ui) fn local(id: i64, name: String) -> Self {
        Self {
            key: PlaylistKey::Local(id),
            name,
            artwork: None,
            link: None,
        }
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(in crate::ui) enum Route {
    Home,
    Search { kind: ObjectKind, query: String },
    History,
    Library(LibraryTab),
    Playlist(Target),
    Object(MusicTarget),
    LocalTrack(Box<LocalTrackTarget>),
    Station(MusicTarget),
    Settings,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(in crate::ui) struct MusicTarget {
    pub(in crate::ui) object: ObjectRef,
    pub(in crate::ui) profile: Option<String>,
    pub(in crate::ui) title: String,
    pub(in crate::ui) artwork: Option<String>,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub(in crate::ui) struct LocalTrackTarget {
    pub(in crate::ui) key: PlaylistKey,
    pub(in crate::ui) track: PlaylistTrack,
}

impl Route {
    pub(in crate::ui) fn page(&self) -> Page {
        match self {
            Self::Home => Page::Home,
            Self::Search { .. } => Page::Search,
            Self::History => Page::History,
            Self::Library(tab) => tab.page(),
            Self::Playlist(_) => Page::Playlist,
            Self::Object(_) | Self::LocalTrack(_) => Page::Object,
            Self::Station(_) => Page::Station,
            Self::Settings => Page::Settings,
        }
    }

    pub(in crate::ui) fn same_destination(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Object(a), Self::Object(b)) | (Self::Station(a), Self::Station(b)) => {
                a.object == b.object && a.profile == b.profile
            }
            (Self::Playlist(a), Self::Playlist(b)) => a.key == b.key,
            (Self::LocalTrack(a), Self::LocalTrack(b)) => {
                if a.track.media.is_some() && b.track.media.is_some() {
                    a.track.media == b.track.media
                } else {
                    a.key == b.key && a.track.id == b.track.id
                }
            }
            _ => self == other,
        }
    }
}

#[derive(Clone, Default)]
pub(in crate::ui) struct LibraryLocation {
    pub(in crate::ui) query: String,
    pub(in crate::ui) source: Option<Scope>,
    pub(in crate::ui) scroll: LibraryScroll,
    pub(in crate::ui) sort: LibrarySort,
    pub(in crate::ui) downloaded: bool,
    pub(in crate::ui) album_list: bool,
    pub(in crate::ui) group: Option<LibraryGroup>,
}

#[derive(Clone, Default)]
pub(in crate::ui) struct LibraryScroll {
    pub(in crate::ui) offset: ListOffset,
    pub(in crate::ui) row: Option<EntryKey>,
}

#[derive(Clone, Default)]
pub(in crate::ui) struct PlaylistLocation {
    pub(in crate::ui) query: String,
    pub(in crate::ui) offset: ListOffset,
    pub(in crate::ui) sort: crate::ui::playlist::Sort,
    pub(in crate::ui) selection: Option<(usize, String)>,
}

/// The single page state kept by a history entry. The variant follows the
/// route kind, so one entry never carries state for two different pages.
#[derive(Clone)]
pub(in crate::ui) enum PageState {
    Scroll(Pixels),
    History(ListOffset),
    Library(LibraryLocation),
    Playlist(PlaylistLocation),
}

impl PageState {
    fn for_route(route: &Route) -> Self {
        match route {
            Route::History => Self::History(ListOffset::default()),
            Route::Library(_) => Self::Library(LibraryLocation::default()),
            Route::Playlist(_) => Self::Playlist(PlaylistLocation::default()),
            _ => Self::Scroll(Pixels::ZERO),
        }
    }
}

struct Entry {
    route: Route,
    state: PageState,
    artwork_origin: Option<gpui::SpanId>,
}

impl Entry {
    fn new(route: Route) -> Self {
        let state = PageState::for_route(&route);
        Self {
            route,
            state,
            artwork_origin: None,
        }
    }
}

pub(in crate::ui) struct Routes {
    history: Vec<Entry>,
    position: usize,
    selection: (u64, Instant),
}

impl Default for Routes {
    fn default() -> Self {
        Self::starting_at(Route::Home)
    }
}

impl Routes {
    pub(super) fn starting_at(route: Route) -> Self {
        Self {
            history: vec![Entry::new(route)],
            position: 0,
            selection: (0, Instant::now()),
        }
    }

    pub(super) fn selection(&self) -> (u64, Instant) {
        self.selection
    }

    fn select(&mut self, position: usize) {
        self.position = position;
        self.selection = (self.selection.0.wrapping_add(1), Instant::now());
    }

    pub(in crate::ui) fn current(&self) -> &Route {
        &self.history[self.position].route
    }

    pub(in crate::ui) fn state(&self) -> &PageState {
        &self.history[self.position].state
    }

    pub(in crate::ui) fn remember(&mut self, state: PageState) {
        self.history[self.position].state = state;
    }

    pub(in crate::ui) fn push(&mut self, route: Route) {
        if !self.current().same_destination(&route) {
            self.history.truncate(self.position + 1);
            self.history.push(Entry::new(route));
            if self.history.len() > NAVIGATION_HISTORY_LIMIT {
                self.history.remove(0);
            }
            self.select(self.history.len() - 1);
        }
    }

    pub(in crate::ui) fn can_step(&self, forward: bool) -> bool {
        if forward {
            self.position + 1 < self.history.len()
        } else {
            self.position > 0
        }
    }

    pub(super) fn record_artwork_arrival(
        &mut self,
        previous: &Route,
        source: gpui::SpanId,
    ) -> bool {
        if self.current().same_destination(previous) {
            return false;
        }
        self.history[self.position].artwork_origin = Some(source);
        true
    }

    pub(super) fn artwork_origin_for_step(&self, forward: bool) -> Option<gpui::SpanId> {
        let destination = if forward {
            self.position + 1
        } else {
            self.position
        };
        self.history.get(destination)?.artwork_origin
    }

    pub(in crate::ui) fn step(&mut self, forward: bool) {
        if self.can_step(forward) {
            if forward {
                self.select(self.position + 1);
            } else {
                self.select(self.position - 1);
            }
        }
    }

    pub(in crate::ui) fn remove_local(&mut self, id: i64) {
        let matches = |entry: &Entry| matches!(&entry.route, Route::Playlist(target) if target.key == PlaylistKey::Local(id));
        let current_deleted = matches(&self.history[self.position]);
        let mut retained_before = self.history[..self.position]
            .iter()
            .filter(|route| !matches(route))
            .count();
        self.history.retain(|route| !matches(route));
        if current_deleted {
            let fallback = Route::Library(LibraryTab::Playlists);
            if self.history.get(retained_before).map(|entry| &entry.route) != Some(&fallback) {
                if retained_before > 0 && self.history[retained_before - 1].route == fallback {
                    retained_before -= 1;
                } else {
                    self.history.insert(retained_before, Entry::new(fallback));
                }
            }
        }
        if current_deleted {
            self.select(retained_before);
        } else {
            self.position = retained_before;
        }
    }

    pub(in crate::ui) fn rename_local(&mut self, id: i64, name: &str) {
        for entry in &mut self.history {
            if let Route::Playlist(target) = &mut entry.route
                && target.key == PlaylistKey::Local(id)
            {
                target.name = name.to_owned();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_navigation_entries_are_evicted_without_losing_the_current_route() {
        let mut routes = Routes::default();
        for index in 0..(NAVIGATION_HISTORY_LIMIT + 5) {
            routes.push(Route::Search {
                kind: ObjectKind::Track,
                query: index.to_string(),
            });
        }
        assert!(matches!(routes.current(), Route::Search { query, .. } if query == "260"));
        let mut steps = 0;
        while routes.can_step(false) {
            routes.step(false);
            steps += 1;
        }
        assert_eq!(steps, NAVIGATION_HISTORY_LIMIT - 1);
        assert!(matches!(routes.current(), Route::Search { query, .. } if query == "5"));
    }

    #[test]
    fn artwork_origin_survives_replacing_an_entry_in_full_history() {
        let mut routes = Routes::default();
        for index in 0..NAVIGATION_HISTORY_LIMIT {
            routes.push(Route::Search {
                kind: ObjectKind::Track,
                query: index.to_string(),
            });
        }
        let previous = routes.current().clone();
        let source = crate::ui::workspace::HERO_ARTWORK_SPAN;
        assert!(!routes.record_artwork_arrival(&previous, source));
        routes.push(Route::History);
        assert_eq!(routes.position, NAVIGATION_HISTORY_LIMIT - 1);
        assert!(routes.record_artwork_arrival(&previous, source));
        assert_eq!(routes.artwork_origin_for_step(false), Some(source));
        routes.step(false);
        assert_eq!(routes.artwork_origin_for_step(true), Some(source));
    }

    #[test]
    fn library_filters_and_scroll_belong_to_history_entries() {
        let mut routes = Routes::default();
        routes.push(Route::Library(LibraryTab::Tracks));
        routes.remember(PageState::Library(LibraryLocation {
            query: "First Filter".into(),
            source: Some(Scope::Provider {
                provider: ProviderId::Spotify,
                profile: "first-profile".into(),
            }),
            scroll: LibraryScroll {
                offset: ListOffset {
                    item_ix: 42,
                    offset_in_item: gpui::px(6.),
                },
                row: None,
            },
            sort: LibrarySort::Artist,
            downloaded: true,
            ..Default::default()
        }));
        routes.push(Route::Library(LibraryTab::Albums));
        routes.remember(PageState::Library(LibraryLocation {
            query: "Saved albums".into(),
            album_list: true,
            ..Default::default()
        }));
        routes.push(Route::Library(LibraryTab::Tracks));
        assert!(matches!(routes.state(), PageState::Library(place) if place.query.is_empty()));
        routes.remember(PageState::Library(LibraryLocation {
            query: "Second Filter".into(),
            ..Default::default()
        }));
        routes.step(false);
        assert!(
            matches!(routes.state(), PageState::Library(place) if place.query == "Saved albums" && place.album_list)
        );
        routes.step(false);
        let PageState::Library(restored) = routes.state() else {
            panic!("the first Liked entry lost its state");
        };
        assert_eq!(restored.query, "First Filter");
        assert!(
            matches!(restored.source.as_ref(), Some(Scope::Provider { profile, .. }) if profile == "first-profile")
        );
        assert_eq!(restored.sort, LibrarySort::Artist);
        assert!(restored.downloaded);
        assert_eq!(restored.scroll.offset.item_ix, 42);
        assert_eq!(restored.scroll.offset.offset_in_item, gpui::px(6.));
        routes.step(true);
        routes.step(true);
        let PageState::Library(restored) = routes.state() else {
            panic!("the second Liked entry lost its state");
        };
        assert_eq!(restored.query, "Second Filter");
    }

    #[test]
    fn each_page_keeps_the_state_kind_of_its_route() {
        let mut routes = Routes::default();
        routes.remember(PageState::Scroll(gpui::px(240.)));
        assert!(matches!(routes.state(), PageState::Scroll(offset) if *offset == gpui::px(240.)));

        routes.push(Route::Search {
            kind: ObjectKind::Album,
            query: "найденное".into(),
        });
        routes.remember(PageState::Scroll(gpui::px(96.)));
        assert!(matches!(routes.state(), PageState::Scroll(offset) if *offset == gpui::px(96.)));

        routes.push(Route::Playlist(Target::local(7, "Плейлист".into())));
        routes.remember(PageState::Playlist(PlaylistLocation {
            query: "строка".into(),
            offset: ListOffset {
                item_ix: 12,
                offset_in_item: gpui::px(3.),
            },
            sort: crate::ui::playlist::Sort::Artist,
            selection: Some((13, "row-13".into())),
        }));
        let PageState::Playlist(place) = routes.state() else {
            panic!("the playlist entry lost its query");
        };
        assert_eq!(place.query, "строка");
        assert_eq!(place.offset.item_ix, 12);
        assert!(place.sort == crate::ui::playlist::Sort::Artist);
        assert_eq!(place.selection, Some((13, "row-13".into())));

        routes.push(Route::Object(MusicTarget {
            object: ObjectRef {
                provider: ProviderId::Spotify,
                kind: ObjectKind::Track,
                id: "track".into(),
            },
            profile: None,
            title: "Трек".into(),
            artwork: None,
        }));
        routes.remember(PageState::Scroll(gpui::px(48.)));
        assert!(matches!(routes.state(), PageState::Scroll(offset) if *offset == gpui::px(48.)));
    }

    #[test]
    fn the_history_page_keeps_its_scroll_position() {
        let mut routes = Routes::default();
        routes.push(Route::History);
        routes.remember(PageState::History(ListOffset {
            item_ix: 12,
            offset_in_item: gpui::px(4.),
        }));
        routes.push(Route::Library(LibraryTab::Playlists));
        routes.step(false);
        let PageState::History(offset) = routes.state() else {
            panic!("the history entry lost its scroll position");
        };
        assert_eq!(offset.item_ix, 12);
        assert_eq!(offset.offset_in_item, gpui::px(4.));
        routes.push(Route::Library(LibraryTab::Playlists));
        routes.step(false);
        assert!(matches!(routes.state(), PageState::History(offset) if offset.item_ix == 12));
    }

    #[test]
    fn repeated_push_keeps_a_single_entry_and_its_state() {
        let mut routes = Routes::default();
        routes.push(Route::Library(LibraryTab::Playlists));
        routes.remember(PageState::Library(LibraryLocation {
            query: "Фильтр".into(),
            ..Default::default()
        }));
        routes.push(Route::Library(LibraryTab::Playlists));
        assert!(matches!(routes.state(), PageState::Library(place) if place.query == "Фильтр"));
        routes.step(false);
        assert!(routes.current() == &Route::Home);
        assert!(matches!(routes.state(), PageState::Scroll(offset) if *offset == Pixels::ZERO));
    }

    #[test]
    fn removing_a_local_playlist_returns_to_the_playlist_library() {
        let mut routes = Routes::default();
        routes.push(Route::Library(LibraryTab::Playlists));
        routes.remember(PageState::Library(LibraryLocation {
            query: "Локальный".into(),
            ..Default::default()
        }));
        routes.push(Route::Playlist(Target::local(4, "Открытый".into())));
        routes.remember(PageState::Playlist(PlaylistLocation {
            query: "фильтр".into(),
            offset: ListOffset {
                item_ix: 5,
                offset_in_item: gpui::px(1.),
            },
            ..Default::default()
        }));
        routes.remove_local(4);
        assert!(routes.current() == &Route::Library(LibraryTab::Playlists));
        assert!(matches!(routes.state(), PageState::Library(place) if place.query == "Локальный"));
        routes.step(false);
        assert!(routes.current() == &Route::Home);
    }

    #[test]
    fn renaming_a_local_playlist_updates_every_history_entry() {
        let mut routes = Routes::default();
        routes.push(Route::Playlist(Target::local(9, "Старое".into())));
        routes.push(Route::Library(LibraryTab::Playlists));
        routes.push(Route::Playlist(Target::local(9, "Старое".into())));
        routes.rename_local(9, "Новое");
        let names = |routes: &Routes| match routes.current() {
            Route::Playlist(target) => target.name.clone(),
            _ => String::new(),
        };
        assert_eq!(names(&routes), "Новое");
        routes.step(false);
        routes.step(false);
        assert_eq!(names(&routes), "Новое");
    }
}
