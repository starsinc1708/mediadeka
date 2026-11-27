#[cfg(feature = "capture")]
pub(super) mod acceptance;
pub(super) mod actions;
pub(super) mod artwork;
mod favorites;
mod hero;
mod labels;
mod loading;
mod local;
pub(super) mod navigation;
mod page;
pub(super) mod popup;
mod scrollbar;
mod snapshot;
mod table;

use std::cell::Cell;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::{
    App, AppContext, Context, Entity, FocusHandle, ListAlignment, ListOffset, ListState, Pixels,
    Subscription, Task, px,
};
use guise::{Size, TextInput};

use crate::{
    playback::{QueueEntry, can_resolve},
    playlist::{Playlist, PlaylistKey, PlaylistTrack},
    providers::ProviderId,
    storage::LocalPlaylistSummary,
};

use super::{
    MainView,
    components::{overlay, search_input},
    connections::SourceFailure,
    workspace::{PlaylistLocation, Target},
};

pub(super) const SORT_TRIGGER_WIDTH: f32 = 168.;

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum Sort {
    #[default]
    Playlist,
    Title,
    Artist,
    Added,
}

impl Sort {
    pub(super) const ALL: [Self; 4] = [Self::Playlist, Self::Title, Self::Artist, Self::Added];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Playlist => "Порядок плейлиста",
            Self::Title => "По названию",
            Self::Artist => "По исполнителю",
            Self::Added => "Сначала новые",
        }
    }

    pub(super) fn id(self) -> &'static str {
        match self {
            Self::Playlist => "playlist",
            Self::Title => "title",
            Self::Artist => "artist",
            Self::Added => "added",
        }
    }
}

#[derive(Clone, Copy)]
struct ColumnsPreference {
    album: bool,
    added: bool,
}

impl Default for ColumnsPreference {
    fn default() -> Self {
        Self {
            album: true,
            added: true,
        }
    }
}

enum Content {
    Loading,
    Partial {
        snapshot: Arc<snapshot::Snapshot>,
        total: Option<usize>,
    },
    Loaded(Arc<snapshot::Snapshot>),
    Error(SourceFailure),
}

fn matches_playlist_account(
    key: &PlaylistKey,
    provider: ProviderId,
    profile: Option<&str>,
) -> bool {
    matches!(key, PlaylistKey::Source { provider: owner, profile: id, .. }
        if *owner == provider && profile.is_none_or(|profile| id.as_deref().is_none_or(|id| id == profile)))
}

struct Request {
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for Request {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

struct OpenPlaylist {
    target: Target,
    content: Content,
    request: Option<Request>,
    refresh_error: Option<SourceFailure>,
    /// One slot per original occurrence. Sorting and filtering only change this projection.
    visible: Vec<usize>,
    selected: Option<usize>,
    visible_audio: bool,
}

impl OpenPlaylist {
    fn queue_entry(&self, index: usize) -> Option<QueueEntry> {
        let snapshot = self.snapshot()?;
        let track = snapshot.playlist.tracks.get(index)?;
        can_resolve(&self.target.key, track).then(|| QueueEntry {
            playlist: self.target.key.clone(),
            playlist_revision: snapshot.revision,
            index,
            track: track.clone(),
            recommendations: Vec::new(),
        })
    }

    fn playlist(&self) -> Option<&Playlist> {
        self.snapshot().map(|snapshot| &snapshot.playlist)
    }

    fn snapshot(&self) -> Option<&Arc<snapshot::Snapshot>> {
        match &self.content {
            Content::Loaded(snapshot) | Content::Partial { snapshot, .. } => Some(snapshot),
            _ => None,
        }
    }

    fn complete_snapshot(&self) -> Option<&Arc<snapshot::Snapshot>> {
        if let Content::Loaded(snapshot) = &self.content {
            Some(snapshot)
        } else {
            None
        }
    }

    fn load_progress(&self) -> Option<String> {
        let Content::Partial { snapshot, total } = &self.content else {
            return None;
        };
        let loaded = snapshot.playlist.tracks.len();
        Some(total.map_or_else(
            || loaded.to_string(),
            |total| format!("{loaded} из {total}"),
        ))
    }

    fn duration(&self) -> Option<u64> {
        self.complete_snapshot()?.duration
    }

    fn project(&mut self, query: &str, sort: Sort, removed: Option<usize>) {
        let (Content::Loaded(snapshot) | Content::Partial { snapshot, .. }) = &self.content else {
            return;
        };
        let tokens = &snapshot.tokens;
        let tracks = &snapshot.playlist.tracks;
        self.visible.clear();
        let terms = query.split_whitespace().collect::<Vec<_>>();
        self.visible
            .extend(tokens.iter().enumerate().filter_map(|(index, text)| {
                (removed != Some(index)
                    && terms.iter().all(|term| {
                        text.title.contains(term)
                            || text.artist.contains(term)
                            || text.album.contains(term)
                    }))
                .then_some(index)
            }));
        match sort {
            Sort::Playlist => {}
            Sort::Title => self
                .visible
                .sort_by(|&a, &b| tokens[a].title.cmp(&tokens[b].title)),
            Sort::Artist => self
                .visible
                .sort_by(|&a, &b| tokens[a].artist.cmp(&tokens[b].artist)),
            Sort::Added => self
                .visible
                .sort_by(|&a, &b| tracks[b].added.cmp(&tracks[a].added)),
        }
        self.visible_audio = if terms.is_empty() && removed.is_none() {
            snapshot.has_audio
        } else {
            snapshot.has_audio
                && self
                    .visible
                    .iter()
                    .any(|&index| can_resolve(&self.target.key, &tracks[index]))
        };
    }
}

pub(super) struct PlaylistUi {
    pub(super) search: Entity<TextInput>,
    editor: Entity<TextInput>,
    list: ListState,
    open: Option<OpenPlaylist>,
    snapshots: snapshot::Cache,
    menu: Option<popup::Menu>,
    menu_focus: FocusHandle,
    sort_select: overlay::Select,
    query: String,
    // Row projections can replace the entire virtual list while its pinned
    // filter is focused. Keep the compact hero above the new first result.
    filter_anchor: Cell<Option<Pixels>>,
    /// A restored row position waits here until the playlist has data.
    pending_offset: Option<ListOffset>,
    pending_selection: Option<(usize, String)>,
    sort: Sort,
    columns: ColumnsPreference,
    pub(in crate::ui) favorites: favorites::Marks,
    pub(in crate::ui) local: Vec<LocalPlaylistSummary>,
    local_task: Option<Task<()>>,
    import: Option<local::Import>,
    pending_local_edit: Option<local::PendingEdit>,
    edit: Option<actions::Edit>,
    edit_cancel_focus: FocusHandle,
    edit_return_focus: Option<FocusHandle>,
    local_busy: bool,
    notice: Option<String>,
    _search_subscription: Subscription,
}

impl MainView {
    /// The playlist row shown in the track tab, when one is selected.
    pub(in crate::ui) fn selected_playlist_track(
        &self,
    ) -> Option<(&PlaylistKey, Option<u64>, usize, &PlaylistTrack)> {
        let open = self.playlist.open.as_ref()?;
        let index = open.selected?;
        let snapshot = open.snapshot()?;
        let track = snapshot.playlist.tracks.get(index)?;
        Some((&open.target.key, snapshot.revision, index, track))
    }
}

impl PlaylistUi {
    pub(super) fn has_loaded_content(&self, target: &Target) -> bool {
        self.open.as_ref().is_some_and(|open| {
            open.target.key == target.key
                && match &open.content {
                    Content::Partial { .. } => !open.visible.is_empty(),
                    Content::Loaded(_) => true,
                    Content::Loading | Content::Error(_) => false,
                }
        })
    }

    pub(super) fn location(&self, cx: &App) -> PlaylistLocation {
        PlaylistLocation {
            query: self.search.read(cx).text(),
            offset: self.list.logical_scroll_top(),
            sort: self.sort,
            selection: self
                .open
                .as_ref()
                .and_then(|open| {
                    let index = open.selected?;
                    open.snapshot()?
                        .playlist
                        .tracks
                        .get(index)
                        .map(|track| (index, track.id.clone()))
                })
                .or_else(|| self.pending_selection.clone()),
        }
    }

    pub(super) fn new(fluid: bool, cx: &mut Context<MainView>) -> Self {
        let search = cx.new(|cx| {
            let field = search_input::editor(
                "playlist-search",
                "Поиск в плейлисте",
                35,
                Size::Xs,
                fluid,
                cx,
            );
            if fluid {
                field.short_placeholder("Поиск")
            } else {
                field
            }
        });
        let subscription = cx.observe(&search, |view, field, cx| {
            let query = field.read(cx).text().trim().to_lowercase();
            let ui = &mut view.playlist;
            if ui.query != query {
                ui.query = query;
                ui.project(true);
                view.reset_list_prefetch(cx);
                cx.notify();
            }
        });
        let list = ListState::new(0, ListAlignment::Top, px(200.));
        Self {
            search,
            editor: cx.new(|cx| {
                TextInput::new(cx)
                    .id("playlist-name")
                    .placeholder("Название плейлиста")
                    .accessible_name("Название плейлиста")
                    .tab_order(26)
            }),
            list,
            open: None,
            snapshots: snapshot::Cache::default(),
            menu: None,
            menu_focus: cx.focus_handle(),
            sort_select: overlay::Select::new(36, cx)
                .with_menu_width(235.)
                .with_trigger_width(SORT_TRIGGER_WIDTH),
            query: String::new(),
            filter_anchor: Cell::new(None),
            pending_offset: None,
            pending_selection: None,
            sort: Sort::default(),
            columns: ColumnsPreference::default(),
            favorites: favorites::Marks::default(),
            local: Vec::new(),
            local_task: None,
            import: None,
            pending_local_edit: None,
            edit: None,
            edit_cancel_focus: cx.focus_handle(),
            edit_return_focus: None,
            local_busy: false,
            notice: None,
            _search_subscription: subscription,
        }
    }

    fn project(&mut self, preserve_hero_scroll: bool) {
        let removed = self
            .open
            .as_ref()
            .and_then(|open| self.projected_removed_track(open));
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let previous = self.list.logical_scroll_top();
        open.project(&self.query, self.sort, removed);
        self.list.reset_with_uniform_height(
            open.visible.len() + page::PREFIX_ROWS + usize::from(open.visible.is_empty()),
            px(page::ROW_HEIGHT),
        );
        let restored = self.pending_offset.filter(|offset| {
            open.complete_snapshot().is_some()
                || (open.snapshot().is_some()
                    && offset.item_ix < open.visible.len() + page::PREFIX_ROWS)
        });
        if let Some(offset) = restored {
            self.pending_offset = None;
            self.list.scroll_to(offset);
        } else if preserve_hero_scroll {
            if let Some(offset) = self.filter_anchor.get().filter(|_| previous.item_ix > 0) {
                self.list.scroll_to(ListOffset {
                    item_ix: 0,
                    offset_in_item: offset,
                });
            } else if previous.item_ix < page::PREFIX_ROWS {
                self.list.scroll_to(previous);
            }
        } else if previous.item_ix > 0 || previous.offset_in_item > px(0.) {
            // A refreshed snapshot keeps the row the reader is looking at.
            self.list.scroll_to(previous);
        }
    }
}
