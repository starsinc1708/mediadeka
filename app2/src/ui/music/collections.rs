#[cfg(feature = "capture")]
mod acceptance;
mod cards;
mod filters;
mod group;
mod header;
mod list;
mod motion;
#[cfg(all(test, windows, feature = "capture"))]
mod native_refresh;
mod views;
pub(super) use views::CollectionViews;

use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use gpui::{App, Context, Task, Window};

use crate::{
    music::{
        ObjectKind,
        library::{
            CollectionUpdate, Entry, EntryAction, EntryKey, LibraryGroup, LibraryIndex,
            LibraryResults, LikeChange, MatchLinks, Scope, load_collections, playlist_entry,
        },
        likes::LikeIdentity,
    },
    playback::{CollectionKey, CollectionPlan, QueueEntry},
    playlist::{PlaylistKey, PlaylistTrack},
    providers::{ProviderId, collections::CollectionKind},
    storage::Store,
    ui::{
        MainView,
        fluid_glass::PageAtmosphere,
        workspace::{LibraryLocation, LibraryTab, Page, PageState},
    },
};

use super::run_job;

pub(super) const LIBRARY_FILTER_ORDER: isize = 31;

pub(super) struct Source {
    pub(super) entries: Arc<[Entry]>,
}

#[derive(Default)]
pub(super) struct Library {
    pub(super) sources: BTreeMap<ProviderId, Source>,
    local: Arc<[Entry]>,
    matches: MatchLinks,
    changes: BTreeMap<EntryKey, Option<Arc<[Entry]>>>,
    downloads: HashSet<LikeIdentity>,
    download_refresh: Option<Task<()>>,
    index: LibraryIndex,
    results: LibraryResults,
    errors: BTreeMap<ProviderId, (String, bool)>,
    error: Option<String>,
    location: LibraryLocation,
    filters_open: bool,
    list: list::LibraryList,
}

impl LibraryTab {
    fn kind(self) -> CollectionKind {
        match self {
            Self::Tracks => CollectionKind::Liked,
            Self::Albums => CollectionKind::Albums,
            Self::Artists => CollectionKind::Artists,
            Self::Playlists => CollectionKind::Playlists,
        }
    }
}

impl Library {
    fn group_artwork(&self) -> Option<&str> {
        let first = *self.results.visible.first()?;
        self.index.rows[first]
            .entry(self.location.source.as_ref())
            .artwork
            .as_deref()
    }

    fn narrows_results(&self) -> bool {
        !self.location.query.trim().is_empty()
            || self.location.downloaded
            || self.location.source.is_some()
            || self.location.group.is_some()
    }

    fn shows_filter_controls(&self) -> bool {
        self.results.unfiltered > 0
            || self.narrows_results()
            || self.location.sort != crate::music::library::LibrarySort::default()
    }

    fn acknowledge_changes(&mut self, scope: &Scope, entries: &[Entry]) {
        if !self.changes.keys().any(|key| &key.0 == scope) {
            return;
        }
        let marks: std::collections::HashMap<_, _> = entries
            .iter()
            .map(|entry| (&entry.key, entry.liked))
            .collect();
        self.changes.retain(|key, change| {
            if &key.0 != scope {
                return true;
            }
            let actual = marks.get(key).copied();
            let expected = change.as_ref().map(|changed| changed[0].liked);
            actual != expected
        });
    }

    fn rebuild(&mut self, loading: bool) {
        let position = self.scroll();
        self.index = LibraryIndex::build_with_changes(
            std::iter::once(self.local.clone())
                .chain(self.sources.values().map(|source| source.entries.clone())),
            &self.matches,
            &self.changes,
        );
        self.index.apply_downloads(&self.downloads);
        self.project(position, loading);
    }

    pub(super) fn filter_changed(&mut self, query: String) -> bool {
        if self.location.query == query {
            return false;
        }
        self.change_filters(|location| location.query = query);
        true
    }

    fn local_album_plan(&self, selected: &PlaylistTrack) -> Option<(CollectionPlan, usize)> {
        let group @ LibraryGroup::Album { title, artist } = self.location.group.as_ref()? else {
            return None;
        };
        let entries: Vec<_> = self
            .local
            .iter()
            .filter_map(|entry| match &entry.action {
                EntryAction::Track(key @ PlaylistKey::Local(_), track)
                    if entry.downloaded && group.contains(track) =>
                {
                    Some((key, track))
                }
                _ => None,
            })
            .enumerate()
            .map(|(index, (playlist, track))| QueueEntry {
                playlist: playlist.clone(),
                playlist_revision: None,
                recommendations: Vec::new(),
                index,
                track: track.clone(),
            })
            .collect();
        let start = entries
            .iter()
            .position(|entry| entry.track.id == selected.id)?;
        let album = CollectionKey::LocalAlbum {
            title: title.clone(),
            artist: artist.clone(),
        };
        Some((
            CollectionPlan::from_entries(album, title.clone(), None, entries),
            start,
        ))
    }
}

impl MainView {
    pub(in crate::ui) fn fluid_local_group_exposure(&self) -> Option<f32> {
        use crate::ui::fluid_glass::tokens::atmosphere;
        match self.music.collections.location.group.as_ref()? {
            LibraryGroup::Album { .. } => Some(atmosphere::ALBUM),
            LibraryGroup::Artist(_) => Some(atmosphere::ARTIST),
        }
    }

    pub(in crate::ui) fn local_group_atmosphere(&self) -> Option<PageAtmosphere<'_>> {
        let library = &self.music.collections;
        match library.location.group.as_ref()? {
            LibraryGroup::Album { title, artist } => Some(PageAtmosphere::local(
                ObjectKind::Album,
                &[title, artist],
                library.group_artwork(),
            )),
            LibraryGroup::Artist(name) => {
                Some(PageAtmosphere::local(ObjectKind::Artist, &[name], None))
            }
        }
    }

    pub(in crate::ui) fn library_location(&self, cx: &App) -> LibraryLocation {
        let library = &self.music.collections;
        LibraryLocation {
            query: self.music.filter.read(cx).text(),
            scroll: library.scroll(),
            ..library.location.clone()
        }
    }

    pub(super) fn restore_library(&mut self, tab: LibraryTab, cx: &mut Context<Self>) {
        let mut location = match self.workspace.routes.state() {
            PageState::Library(location) => location.clone(),
            _ => LibraryLocation::default(),
        };
        if let Some(Scope::Provider { provider, profile }) = &location.source
            && !self.library_profile_is_current(*provider, profile)
        {
            location.source = None;
            location.scroll = Default::default();
        }
        self.music.filter.update(cx, |field, cx| {
            field.set_text(&location.query, cx);
            field.set_placeholder("Поиск в медиатеке", cx);
        });
        self.music.collections.init(location, cx);
        if self.uses_fluid_glass()
            && let Some(kind) = tab.collection_kind()
        {
            self.music.collection_views.load(kind);
        }
        self.sync_library_local_playlists(cx);
        self.load_music_collections(tab, cx);
    }

    pub(in crate::ui) fn refresh_library(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self.music_page().library_tab() {
            self.music.request = None;
            self.load_music_collections(tab, cx);
        }
    }

    pub(in crate::ui) fn apply_library_like(&mut self, change: LikeChange) {
        if self.music_page() != Page::Tracks
            || matches!(&change.key.0, Scope::Provider { provider, profile }
                if !self.library_profile_is_current(*provider, profile))
        {
            return;
        }
        let loading = self.music.request.is_some();
        let library = &mut self.music.collections;
        if let Some(link) = change.link
            && !library.matches.choices.contains(&link)
        {
            library.matches.choices.push(link);
        }
        library
            .changes
            .insert(change.key, change.entry.map(|entry| Arc::from([entry])));
        library.rebuild(loading);
    }

    pub(in crate::ui) fn refresh_library_downloads(&mut self, cx: &mut Context<Self>) {
        if self.music_page() != Page::Tracks {
            return;
        }
        self.music.collections.download_refresh = Some(cx.spawn(async |view, cx| {
            let downloaded = cx
                .background_executor()
                .spawn(async { Store::open_default()?.cached_recordings() })
                .await;
            let _ = view.update(cx, |view, cx| {
                let library = &mut view.music.collections;
                library.download_refresh = None;
                match downloaded {
                    Ok(downloaded) => {
                        let position = library.scroll();
                        library.downloads = downloaded;
                        library.index.apply_downloads(&library.downloads);
                        library.project(position, view.music.request.is_some());
                    }
                    Err(error) => library.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
    }

    pub(in crate::ui) fn sync_library_local_playlists(&mut self, cx: &mut Context<Self>) {
        if self.music_page() == Page::Playlists {
            self.music.collections.local = self.playlist.local.iter().map(playlist_entry).collect();
            let loading = self.music.request.is_some();
            self.music.collections.rebuild(loading);
            cx.notify();
        }
    }

    fn load_music_collections(&mut self, tab: LibraryTab, cx: &mut Context<Self>) {
        if self.music.request.is_some() {
            return;
        }
        self.music.collections.error = None;
        self.music.request = Some(run_job(
            cx,
            move |cancelled, tx| {
                load_collections(tab.kind(), cancelled, |update| {
                    let _ = tx.try_send(update);
                })
                .map_err(|error| error.to_string())
            },
            move |view, update, cx| {
                if view.receive_library_update(tab, update, cx) {
                    view.music.collections.rebuild(true);
                }
                cx.notify();
            },
            |view, result, _| {
                let library = &mut view.music.collections;
                if let Err(error) = result {
                    library.error = Some(error);
                }
                library.finish_position();
            },
        ));
        cx.notify();
    }

    fn receive_library_update(
        &mut self,
        tab: LibraryTab,
        update: CollectionUpdate,
        cx: &mut Context<Self>,
    ) -> bool {
        match update {
            CollectionUpdate::Local(local) if tab != LibraryTab::Playlists => {
                let library = &mut self.music.collections;
                library.acknowledge_changes(&Scope::Local, &local);
                let changed = library.local.as_ref() != local.as_slice();
                if changed {
                    library.local = local.into();
                }
                changed
            }
            CollectionUpdate::Local(_) => false,
            CollectionUpdate::Links(mut links) => {
                let library = &mut self.music.collections;
                if !library.changes.is_empty() {
                    for choice in &library.matches.choices {
                        if !links.choices.contains(choice) {
                            links.choices.push(choice.clone());
                        }
                    }
                }
                let changed = library.matches != links;
                library.matches = links;
                changed
            }
            CollectionUpdate::Downloads(downloads) => {
                let library = &mut self.music.collections;
                let changed = library.downloads != downloads;
                library.downloads = downloads;
                changed
            }
            CollectionUpdate::Issue(message) => {
                self.music.collections.error = Some(message);
                false
            }
            CollectionUpdate::Data {
                provider,
                profile,
                entries,
            } if self.library_profile_is_current(provider, &profile) => {
                let library = &mut self.music.collections;
                library.errors.remove(&provider);
                library.acknowledge_changes(&Scope::Provider { provider, profile }, &entries);
                if library
                    .sources
                    .get(&provider)
                    .is_some_and(|source| source.entries == entries)
                {
                    return false;
                }
                library.sources.insert(provider, Source { entries });
                true
            }
            CollectionUpdate::Error {
                provider,
                profile,
                message,
                hide,
            } if self.library_profile_is_current(provider, &profile) => {
                let library = &mut self.music.collections;
                library.errors.insert(provider, (message, hide));
                if hide {
                    library.sources.remove(&provider);
                    library
                        .changes
                        .retain(|key, _| key.0.provider() != Some(provider));
                    if library.location.source.as_ref().and_then(Scope::provider) == Some(provider)
                    {
                        library.location.source = None;
                    }
                    self.forget_account_playlists(provider, Some(&profile), cx);
                }
                hide
            }
            _ => false,
        }
    }

    fn library_profile_is_current(&self, provider: ProviderId, profile: &str) -> bool {
        self.state.home_view().is_some_and(|(home, _)| {
            home.accounts
                .iter()
                .any(|account| account.provider == provider && account.profile.id == profile)
        })
    }

    pub(in crate::ui) fn reveal_library_filter(&mut self) -> bool {
        let glass = self.uses_fluid_glass();
        let library = &mut self.music.collections;
        if !library.shows_filter_controls() {
            return false;
        }
        if glass {
            library.filters_open = true;
        }
        library.reveal();
        true
    }

    pub(in crate::ui) fn open_library_group(
        &mut self,
        group: LibraryGroup,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.show_music_page(Page::Tracks, window, cx);
        let library = &mut self.music.collections;
        library.location.group = Some(group);
        library.location.source = Some(Scope::Local);
        library.project(Default::default(), self.music.request.is_some());
        self.reset_list_prefetch(cx);
        cx.notify();
    }

    fn play_local_album_track(&mut self, selected: &PlaylistTrack, cx: &mut Context<Self>) -> bool {
        let Some((plan, start)) = self.music.collections.local_album_plan(selected) else {
            return false;
        };
        self.ensure_playlist_favorites(&plan.entries[start].playlist, cx);
        let entries = plan.entries.clone();
        self.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| {
                playback.play_collection(entries, start, Arc::new(plan))
            });
        });
        true
    }
}
