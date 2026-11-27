#[cfg(feature = "capture")]
mod acceptance;
#[cfg(all(test, windows, feature = "capture"))]
mod alternate_snapshot_native;
mod collections;
mod credits;
pub(in crate::ui) mod detail;
pub(super) mod detail_style;
mod object_content;
mod prefetch;
mod relations;
mod search;
mod source_links;

pub(super) use prefetch::PrefetchReport;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::{
    AnyElement, AppContext, Context, Div, Entity, FocusHandle, FontWeight, IntoElement, Role,
    SharedString, Stateful, Subscription, Task, Window, div, prelude::*, px, rgb,
};
use guise::{Icon, IconName, Size, TextInput};

use crate::{
    music::{ObjectKind, ObjectRef, ScopedPage, library::EntryAction},
    playback::{QueueEntry, Status, can_resolve},
    playlist::{PlaylistKey, PlaylistTrack},
    providers::{HomeItem, ProviderId},
};

use super::{
    MainView,
    assets::provider_mark,
    components::{
        self, button,
        music::TRACK_ROW_HEIGHT,
        search_input::{self, Scope, SearchInput},
    },
    theme::{ELEVATED, SURFACE, secondary, surface_border},
    workspace::{LibraryTab, MusicTarget, Route, Target},
};

const TRACK_LYRICS_ORDER: isize = 41;

#[derive(Clone, Copy)]
enum PlaybackAction {
    Play,
    Shuffle,
    Append,
}

pub(super) struct MusicUi {
    pub(super) filter: Entity<TextInput>,
    library_tab_focus: [FocusHandle; LibraryTab::ALL.len()],
    library_filters_focus: FocusHandle,
    pub(super) track_lyrics_focus: FocusHandle,
    library_source_select: components::overlay::Select,
    library_sort_select: components::overlay::Select,
    object: Option<detail::Detail>,
    track_release: Option<detail::track_release::ReleaseTracks>,
    artist_discography: Option<ScopedPage>,
    collections: collections::Library,
    collection_views: collections::CollectionViews,
    search: search::Search,
    request: Option<Request>,
    prefetch: prefetch::CatalogPrefetch,
    relation: Option<relations::RelationRequest>,
    credits: credits::TrackCredits,
    _filter_subscription: Subscription,
}

impl MusicUi {
    pub(super) fn new(fluid: bool, cx: &mut Context<MainView>) -> Self {
        let filter = cx.new(|cx| {
            let field = search_input::editor(
                "music-filter",
                "Название, исполнитель или ссылка",
                30,
                Size::Sm,
                fluid,
                cx,
            );
            if fluid {
                field.short_placeholder("Поиск в медиатеке")
            } else {
                field
            }
        });
        let subscription = cx.observe(&filter, |view, filter, cx| {
            if view.music_page().library_tab().is_some()
                && view
                    .music
                    .collections
                    .filter_changed(filter.read(cx).text())
            {
                view.reset_list_prefetch(cx);
            }
            cx.notify();
        });
        Self {
            filter,
            library_tab_focus: LibraryTab::ALL.map(|tab| {
                cx.focus_handle()
                    .tab_index(20 + tab as isize)
                    .tab_stop(true)
            }),
            library_filters_focus: cx
                .focus_handle()
                .tab_index(collections::LIBRARY_FILTER_ORDER)
                .tab_stop(true),
            track_lyrics_focus: cx
                .focus_handle()
                .tab_index(TRACK_LYRICS_ORDER)
                .tab_stop(true),
            library_source_select: components::overlay::Select::new(32, cx),
            library_sort_select: components::overlay::Select::new(40, cx),
            object: None,
            track_release: None,
            artist_discography: None,
            collections: collections::Library::default(),
            collection_views: collections::CollectionViews::default(),
            search: search::Search::default(),
            request: None,
            prefetch: prefetch::CatalogPrefetch::default(),
            relation: None,
            credits: credits::TrackCredits::default(),
            _filter_subscription: subscription,
        }
    }
}

impl MainView {
    pub(in crate::ui) fn library_tab_focus(&self, tab: LibraryTab) -> &FocusHandle {
        &self.music.library_tab_focus[tab as usize]
    }

    pub(super) fn music_request_pending(&self) -> bool {
        self.music.request.is_some()
    }

    pub(super) fn music_object_ready(&self) -> bool {
        self.music
            .object
            .as_ref()
            .is_some_and(|detail| detail.data.is_some())
            || !self.music_request_pending()
    }

    pub(super) fn has_loaded_catalog_object(&self, target: &MusicTarget) -> bool {
        self.music
            .object
            .as_ref()
            .is_some_and(|detail| detail.target == *target && detail.data.is_some())
    }

    pub(super) fn open_music_route(&mut self, cx: &mut Context<Self>) {
        super::design_system::icons::forget_motion(cx, "music-object-play");
        self.music.request = None;
        self.music.relation = None;
        self.music.object = None;
        self.music.track_release = None;
        self.music.collections = collections::Library::default();
        self.music.search = search::Search::default();
        let route = self.workspace.routes.current().clone();
        self.music.prefetch.route_changed(&route, cx);
        match route {
            Route::Object(target) => self.load_music_object(target, cx),
            Route::Search { kind, query } => self.search_music(kind, query, cx),
            Route::Library(tab) => self.restore_library(tab, cx),
            _ => {}
        }
        self.refresh_genius_credits(cx);
        self.refresh_track_release(cx);
    }

    pub(in crate::ui) fn open_music_item(
        &mut self,
        provider: ProviderId,
        profile: Option<&str>,
        item: HomeItem,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let Some(object) = ObjectRef::from_item(provider, &item) else {
            return;
        };
        if object.kind == ObjectKind::Playlist {
            self.show_playlist(
                Target::from_item(provider, profile, object.id, item),
                window,
                cx,
            );
        } else {
            self.open_music_target(
                MusicTarget {
                    object,
                    profile: profile.map(str::to_owned),
                    title: item.title,
                    artwork: item.artwork,
                },
                window,
                cx,
            );
        }
    }

    pub(in crate::ui) fn open_music_target(
        &mut self,
        target: MusicTarget,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let route = if target.object.kind == ObjectKind::Station {
            Route::Station(target)
        } else {
            Route::Object(target)
        };
        self.navigate(route, window, cx);
    }

    fn play_music_tracks<'a>(
        &mut self,
        entries: impl IntoIterator<Item = (PlaylistKey, &'a PlaylistTrack)>,
        action: PlaybackAction,
        cx: &mut Context<Self>,
    ) {
        let entries = entries
            .into_iter()
            .enumerate()
            .filter(|(_, (playlist, track))| can_resolve(playlist, track))
            .map(|(index, (playlist, track))| QueueEntry {
                playlist,
                playlist_revision: None,
                recommendations: Vec::new(),
                index,
                track: track.clone(),
            })
            .collect::<Vec<_>>();
        if entries.is_empty() {
            return;
        }
        self.ensure_playlist_favorites(&entries[0].playlist, cx);
        self.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| match action {
                PlaybackAction::Play => playback.play(entries, 0),
                PlaybackAction::Shuffle => playback.play_shuffled(entries),
                PlaybackAction::Append => playback.append(entries),
            })
        });
    }

    fn music_queue_button(
        &self,
        id: String,
        order: isize,
        action: &EntryAction,
        cx: &Context<Self>,
    ) -> Option<Stateful<Div>> {
        let EntryAction::Track(key, track) = action else {
            return None;
        };
        if !self.uses_fluid_glass() || !can_resolve(key, track) {
            return None;
        }
        use super::{components::Activate, fluid_glass::tokens};
        let label = format!("Добавить в очередь · {}", track.title);
        let (key, track) = (key.clone(), track.clone());
        Some(
            button(SharedString::from(id), label, order)
                .size(px(QUEUE_CONTROL))
                .flex_none()
                .justify_center()
                .rounded(px(8.))
                .text_color(rgb(tokens::SECONDARY))
                .hover(|style| {
                    style
                        .bg(rgb(tokens::ELEVATED))
                        .text_color(rgb(tokens::PRIMARY))
                })
                .tooltip(guise::tooltip("Добавить в очередь"))
                .child(Icon::new(IconName::ListPlus).pixel_size(px(16.)))
                .on_activate(cx.listener(move |view, _, _, cx| {
                    view.play_music_tracks([(key.clone(), &track)], PlaybackAction::Append, cx);
                    view.player.update(cx, |bar, cx| {
                        bar.show_notice("Добавлено в очередь".into(), cx)
                    });
                })),
        )
    }

    fn music_entry_is_current(&self, entry: &EntryAction, cx: &gpui::App) -> bool {
        let EntryAction::Track(key, track) = entry else {
            return false;
        };
        self.player.read(cx).recording_status(key, track).is_some()
    }
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

fn run_job<T: Send + 'static>(
    cx: &mut Context<MainView>,
    work: impl FnOnce(&AtomicBool, async_channel::Sender<T>) -> Result<(), String> + Send + 'static,
    receive: impl Fn(&mut MainView, T, &mut Context<MainView>) + 'static,
    finish: impl FnOnce(&mut MainView, Result<(), String>, &mut Context<MainView>) + 'static,
) -> Request {
    let cancelled = Arc::new(AtomicBool::new(false));
    let check = Arc::clone(&cancelled);
    let task = cx.spawn(async move |view, cx| {
        let (tx, rx) = async_channel::unbounded();
        let work_flag = Arc::clone(&check);
        let worker = cx
            .background_executor()
            .spawn(async move { work(&work_flag, tx) });
        while let Ok(update) = rx.recv().await {
            if check.load(Ordering::Relaxed)
                || view
                    .update(cx, |view, cx| receive(view, update, cx))
                    .is_err()
            {
                return;
            }
        }
        let result = worker.await;
        if !check.load(Ordering::Relaxed) {
            let _ = view.update(cx, |view, cx| {
                view.music.request.take();
                finish(view, result, cx);
                cx.notify();
            });
        }
    });
    Request {
        cancelled,
        _task: task,
    }
}

pub(in crate::ui) fn heading(id: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .accessibility_id(id)
        .role(Role::Heading)
        .aria_label(label.clone())
        .text_size(px(32.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

pub(in crate::ui) fn status(id: &'static str, label: impl Into<SharedString>) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .accessibility_id(id)
        .role(Role::Status)
        .aria_label(label.clone())
        .text_color(secondary())
        .child(label)
}

fn search_box(
    field: Entity<TextInput>,
    controls: Option<components::glass_control::Material>,
    compact: bool,
) -> AnyElement {
    if let Some(material) = controls {
        return SearchInput::new("music-filter", field, material, Scope::Local, 31, compact)
            .into_any_element();
    }
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .min_w_0()
        .h(px(38.))
        .px(px(10.))
        .rounded(px(6.))
        .bg(rgb(SURFACE))
        .border_1()
        .border_color(surface_border())
        .child(
            Icon::new(IconName::Search)
                .pixel_size(px(16.))
                .color(secondary()),
        )
        .child(div().flex_1().min_w_0().child(field))
        .into_any_element()
}

const FIRST_RESULT_TAB_INDEX: isize = 100;
const QUEUE_CONTROL: f32 = 32.;
const SEARCH_COVER: f32 = 34.;

#[derive(Clone, Copy)]
enum SearchRowStyle<'a> {
    Classic,
    Fluid {
        current: Option<&'a Status>,
        row_style: components::track_row::Style,
    },
}

struct SearchArtwork<'a> {
    url: Option<&'a str>,
    seed: Option<super::fluid_glass::artwork::Seed>,
    play: Option<Stateful<Div>>,
}

#[cfg(feature = "capture")]
pub(super) fn gallery_track_row(
    track: &PlaylistTrack,
    row_style: components::track_row::Style,
) -> Stateful<Div> {
    row(
        "appearance-track-row",
        0,
        track.title.clone(),
        track.artist.clone(),
        SearchArtwork {
            url: track.artwork.as_deref(),
            seed: super::fluid_glass::artwork::Seed::track(track),
            play: None,
        },
        None,
        SearchRowStyle::Fluid {
            current: None,
            row_style,
        },
    )
    .tab_stop(false)
    .role(Role::ListItem)
    .cursor_default()
}

fn row(
    id: impl Into<gpui::ElementId>,
    order: usize,
    title: String,
    subtitle: String,
    artwork: SearchArtwork<'_>,
    provider: Option<ProviderId>,
    style: SearchRowStyle<'_>,
) -> Stateful<Div> {
    use super::{components::track_row, fluid_glass::tokens};
    let glass = matches!(style, SearchRowStyle::Fluid { .. });
    let control = button(
        id,
        format!("{title} · {subtitle}"),
        FIRST_RESULT_TAB_INDEX + order as isize,
    )
    .w_full()
    .h(px(TRACK_ROW_HEIGHT))
    .gap(px(10.))
    .px(px(9.))
    .rounded(px(6.));
    let control = match style {
        SearchRowStyle::Classic => control.hover(|row| row.bg(rgb(ELEVATED))),
        SearchRowStyle::Fluid { current, row_style } => row_style
            .interaction(control)
            .text_size(px(tokens::text::TRACK_TITLE))
            .text_color(row_style.title(current.is_some()))
            .font_weight(if current.is_some() {
                FontWeight::SEMIBOLD
            } else {
                FontWeight::MEDIUM
            })
            .when_some(current, |row, status| {
                row.aria_description(components::track_row::current_description(status))
            }),
    };
    control
        .when(artwork.play.is_some(), |row| {
            row.group(track_row::HOVER_GROUP)
        })
        .child(match style {
            SearchRowStyle::Fluid { current, row_style } => {
                components::music::fluid_cover(artwork.url, artwork.seed, SEARCH_COVER, 5.)
                    .children(current.map(|status| row_style.cover_marker(status)))
                    .children(artwork.play)
            }
            SearchRowStyle::Classic => {
                components::music::cover(artwork.url, SEARCH_COVER, 5., false)
            }
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(div().truncate().child(title))
                .when(!subtitle.is_empty(), |text| {
                    text.child(
                        div()
                            .truncate()
                            .text_size(px(tokens::text::CAPTION))
                            .when(glass, |label| label.font_weight(FontWeight::NORMAL))
                            .text_color(match style {
                                SearchRowStyle::Fluid { row_style, .. } => row_style.secondary(),
                                SearchRowStyle::Classic => secondary(),
                            })
                            .child(subtitle),
                    )
                }),
        )
        .children(provider.map(|provider| provider_mark(provider, 19.)))
}
