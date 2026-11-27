use std::{
    cell::RefCell,
    sync::{Arc, atomic::Ordering},
    time::Instant,
};

use gpui::{
    App, Context, FocusHandle, IntoElement, Role, SharedString, Window, div, prelude::*, px,
};
use guise::{Button, IconName, Variant};

use crate::{
    music::{
        ObjectKind, ObjectRef, PublicInput, ScopedPage, SearchInput,
        library::{Entry, EntryAction, EntryKey, Scope, search_saved},
        resolve_public_input,
        search_cache::{Dispatch, SearchBatch, search_catalogs},
    },
    playback::can_resolve,
    providers::{HomeItem, ProviderError, ProviderId},
    ui::{
        MainView,
        components::{
            Activate,
            content_state::{error_state, loading_state, refresh_indicator},
            section_tab, track_row,
        },
        design_system::controls::ControlVariant,
        fluid_glass::tokens,
        playlist::navigation::Destination,
        theme::surface_border,
        workspace::{MusicTarget, Page, Route, Target},
    },
};

use super::{
    FIRST_RESULT_TAB_INDEX, PlaybackAction, SEARCH_COVER, SearchArtwork, SearchRowStyle,
    detail::playlist_key, heading, prefetch::entry_target, row, run_job, status,
};

#[cfg(all(test, windows, feature = "capture"))]
mod native_soundcloud;

pub(super) struct Search {
    kind: ObjectKind,
    candidates: Vec<ResultRow>,
    saved: Vec<SavedRow>,
    direct: Option<(ObjectRef, String)>,
    unresolved: Option<UnresolvedLink>,
    errors: Vec<String>,
    submitted: bool,
    next_tab: isize,
}

struct UnresolvedLink {
    provider: ProviderId,
    url: String,
    error: ProviderError,
}

struct ResultRow {
    candidate: ScopedPage,
    focus: FocusHandle,
}

struct SavedRow {
    entry: Arc<Entry>,
    focus: FocusHandle,
    object: Option<ObjectRef>,
}

impl Default for Search {
    fn default() -> Self {
        Self {
            kind: ObjectKind::Track,
            candidates: Vec::new(),
            saved: Vec::new(),
            direct: None,
            unresolved: None,
            errors: Vec::new(),
            submitted: false,
            next_tab: super::FIRST_RESULT_TAB_INDEX,
        }
    }
}

impl Search {
    fn append(&mut self, pages: impl IntoIterator<Item = ScopedPage>, cx: &App) {
        for candidate in pages {
            if self.saved.iter().any(|saved| saved.matches(&candidate)) {
                continue;
            }
            if let Some(row) = self
                .candidates
                .iter_mut()
                .find(|row| same_result(&row.candidate, &candidate))
            {
                row.candidate = candidate;
            } else {
                let focus = self.row_focus(cx);
                self.candidates.push(ResultRow { candidate, focus });
            }
        }
    }

    fn accept_batch(&mut self, batch: SearchBatch, cx: &App) {
        if batch.complete {
            self.candidates.retain(|row| {
                let candidate = &row.candidate;
                candidate.page.reference.provider != batch.provider
                    || candidate.profile.as_deref() != Some(batch.profile.as_str())
                    || batch.pages.iter().any(|page| same_result(candidate, page))
            });
        }
        self.append(batch.pages.iter().cloned(), cx);
    }

    fn contains(&self, candidate: &ScopedPage) -> bool {
        self.candidates
            .iter()
            .any(|entry| same_result(&entry.candidate, candidate))
    }

    fn append_saved(&mut self, entries: Vec<Arc<Entry>>, cx: &App) {
        for entry in entries {
            let object = entry.object();
            let index = self.saved.iter().position(|row| row.entry.key == entry.key);
            let focus = match index {
                Some(index) => self.saved[index].focus.clone(),
                None => self.row_focus(cx),
            };
            let row = SavedRow {
                entry,
                focus,
                object,
            };
            self.candidates
                .retain(|candidate| !row.matches(&candidate.candidate));
            if let Some(index) = index {
                self.saved[index] = row;
            } else {
                self.saved.push(row);
            }
        }
    }

    fn row_focus(&mut self, cx: &App) -> FocusHandle {
        let focus = cx.focus_handle().tab_index(self.next_tab).tab_stop(true);
        self.next_tab = self.next_tab.saturating_add(1);
        focus
    }
}

fn same_result(a: &ScopedPage, b: &ScopedPage) -> bool {
    a.page.reference == b.page.reference && a.profile == b.profile
}

impl SavedRow {
    fn matches(&self, candidate: &ScopedPage) -> bool {
        self.object.as_ref() == Some(&candidate.page.reference)
            && matches!(&self.entry.key.0, Scope::Provider { profile, .. }
                if Some(profile.as_str()) == candidate.profile.as_deref())
    }
}

enum ResultUpdate {
    Saved(Vec<Arc<Entry>>),
    SavedComplete(Vec<EntryKey>),
    Catalog(SearchBatch),
    Candidates(Vec<ScopedPage>),
    Error(String),
    Direct(ObjectRef, String),
    Unresolved(UnresolvedLink),
}

impl MainView {
    pub(in crate::ui) fn searches_connected_catalogs(&self) -> bool {
        self.state
            .home_view()
            .is_some_and(|(home, _)| !home.accounts.is_empty())
    }

    pub(in crate::ui) fn global_search_pending(&self) -> bool {
        self.music_page() == Page::Search && self.music.request.is_some()
    }

    /// Enter in the global field searches the catalogs for the current
    /// category, or `Track` outside the search page. Typing alone never
    /// changes the route.
    pub(in crate::ui) fn submit_global_search(&mut self, window: &Window, cx: &mut Context<Self>) {
        let query = self.workspace.search.text(cx).trim().to_owned();
        let kind = match self.workspace.routes.current() {
            Route::Search { kind, .. } => *kind,
            _ => ObjectKind::Track,
        };
        self.submit_catalog(kind, query, window, cx);
    }

    pub(in crate::ui) fn submit_catalog(
        &mut self,
        kind: ObjectKind,
        query: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(SearchInput::Url(url)) = SearchInput::new(kind, &query)
            && let Some(object) = ObjectRef::from_url(&url)
            && !crate::music::is_soundcloud_set(&object)
        {
            self.open_search_address(object, url, window, cx);
            return;
        }
        self.show_catalog_query(kind, query, window, cx);
    }

    fn show_catalog_query(
        &mut self,
        kind: ObjectKind,
        query: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if matches!(self.workspace.routes.current(),
            Route::Search { kind: current, query: text } if *current == kind && *text == query)
        {
            if self.music.request.is_none() && !query.trim().is_empty() {
                self.search_music(kind, query, cx);
            }
        } else {
            self.navigate(Route::Search { kind, query }, window, cx);
        }
    }

    pub(in crate::ui) fn catalog_search_accounts_changed(&mut self, cx: &mut Context<Self>) {
        if let Route::Search { kind, query } = self.workspace.routes.current().clone() {
            self.music.request = None;
            self.music.search = Search::default();
            self.search_music(kind, query, cx);
        }
    }

    pub(super) fn search_music(&mut self, kind: ObjectKind, query: String, cx: &mut Context<Self>) {
        self.music.search.kind = kind;
        let Some(input) = SearchInput::new(kind, &query) else {
            return;
        };
        self.music.search.submitted = true;
        self.music.search.errors.clear();
        self.music.search.direct = None;
        self.music.search.unresolved = None;
        let has_accounts = self.searches_connected_catalogs();
        let submitted_at = Instant::now();
        self.music.request = Some(run_job(
            cx,
            move |cancelled, tx| match input {
                SearchInput::Text { kind, query } => {
                    let keys = RefCell::new(Vec::new());
                    match search_saved(kind, &query, cancelled, |entries| {
                        if !entries.is_empty() {
                            keys.borrow_mut()
                                .extend(entries.iter().map(|entry| entry.key.clone()));
                            let _ = tx.try_send(ResultUpdate::Saved(entries));
                        }
                    }) {
                        Ok(()) => {
                            let _ = tx.try_send(ResultUpdate::SavedComplete(keys.into_inner()));
                        }
                        Err(error) => {
                            let _ = tx.try_send(ResultUpdate::Error(error.to_string()));
                        }
                    }
                    if !has_accounts || cancelled.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    search_catalogs(
                        kind,
                        &query,
                        Dispatch::Submitted(submitted_at),
                        cancelled,
                        |result| {
                            let _ = tx.try_send(match result {
                                Ok(batch) => ResultUpdate::Catalog(batch),
                                Err(error) => ResultUpdate::Error(error),
                            });
                        },
                    )
                }
                SearchInput::Url(url) => resolve_public_input(&url, cancelled, |result| {
                    let update = match result {
                        PublicInput::Address(object, url) => ResultUpdate::Direct(object, url),
                        PublicInput::Candidates(pages) => ResultUpdate::Candidates(pages),
                        PublicInput::Unresolved {
                            provider,
                            url,
                            error,
                        } => ResultUpdate::Unresolved(UnresolvedLink {
                            provider,
                            url,
                            error,
                        }),
                    };
                    let _ = tx.try_send(update);
                }),
            },
            |view, update, cx| {
                let search = &mut view.music.search;
                match update {
                    ResultUpdate::Saved(entries) => search.append_saved(entries, cx),
                    ResultUpdate::SavedComplete(keys) => {
                        search.saved.retain(|row| keys.contains(&row.entry.key))
                    }
                    ResultUpdate::Catalog(batch) => search.accept_batch(batch, cx),
                    ResultUpdate::Candidates(pages) => search.append(pages, cx),
                    ResultUpdate::Error(error) => search.errors.push(error),
                    ResultUpdate::Direct(object, url) => search.direct = Some((object, url)),
                    ResultUpdate::Unresolved(link) => search.unresolved = Some(link),
                }
                cx.notify();
            },
            |view, result, _| {
                if let Err(error) = result {
                    view.music.search.errors.push(error);
                }
            },
        ));
        cx.notify();
    }

    pub(super) fn open_search_address(
        &mut self,
        object: ObjectRef,
        url: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let profile = self
            .connected_provider_profile(object.provider)
            .map(str::to_owned);
        if object.kind == ObjectKind::Playlist {
            let url = if object.provider == ProviderId::Jellyfin {
                Some(url)
            } else {
                crate::music::links::public_link(&url).map(|link| link.url)
            };
            let item = HomeItem {
                id: object.id.clone(),
                title: "Плейлист".into(),
                subtitle: String::new(),
                url,
                artwork: None,
            };
            self.show_playlist(
                Target::from_item(object.provider, profile.as_deref(), object.id, item),
                window,
                cx,
            );
        } else {
            self.open_music_target(
                MusicTarget {
                    object,
                    profile,
                    title: String::new(),
                    artwork: None,
                },
                window,
                cx,
            );
        }
    }

    pub(in crate::ui) fn catalog_search_view(
        &self,
        compact: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let search = &self.music.search;
        let pending = self.music.request.is_some();
        let glass = self.uses_fluid_glass();
        let row_style = track_row::Style::new(cx, self.fluid_high_contrast(cx));
        let controls = self.glass_controls(cx);
        let title_size = if compact {
            tokens::text::PAGE_TITLE_COMPACT
        } else {
            tokens::text::PAGE_TITLE
        };
        let query = match self.workspace.routes.current() {
            Route::Search { query, .. } => query.clone(),
            _ => String::new(),
        };
        let catalogs = self.searches_connected_catalogs();
        let has_results = !search.saved.is_empty() || !search.candidates.is_empty();
        let link = matches!(
            SearchInput::new(search.kind, &query),
            Some(SearchInput::Url(_))
        );
        div()
            .id("music-catalog-search")
            .accessibility_id("music-catalog-search")
            .role(gpui::Role::Group)
            .aria_label("Поиск")
            .flex()
            .flex_col()
            .max_w(px(1040.))
            .gap(px(8.))
            .child(div().flex().items_center().gap(px(tokens::space::SM))
                .child(heading("music-search-title", "Поиск").pb(px(5.)).when(glass, |title| title.text_size(px(title_size))))
                .when(glass, |heading| {
                    heading.child(div().w(px(tokens::size::XS)).flex_none().children(
                        (pending && has_results).then(|| refresh_indicator("music-search-refreshing", "Обновляем результаты…", cx)),
                    ))
                }))
            .when(!catalogs && search.unresolved.is_none() && (!glass || query.trim().is_empty() || search.saved.is_empty()), |page| {
                page.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(status(
                            "music-search-guest-scope",
                            "По названиям ищем в медиатеке. Публичные ссылки тоже можно открыть.",
                        ))
                        .child(
                            Button::new("music-search-connect", "Подключить площадку")
                                .variant(Variant::Default)
                                .tab_order(23)
                                .when_some(controls, |button, material| {
                                    ControlVariant::TextAction.text_button(button, material)
                                })
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.show_music_page(Page::Settings, window, cx);
                                })),
                        ),
                )
            })
            .child(
                div()
                    .id("catalog-search-categories")
                    .accessibility_id("catalog-search-categories")
                    .role(Role::TabList)
                    .aria_label("Категории поиска")
                    .flex()
                    .flex_wrap()
                    .gap(px(4.))
                    .border_b_1()
                    .border_color(surface_border())
                    .children(
                        [
                            (ObjectKind::Track, "Треки"),
                            (ObjectKind::Album, "Альбомы и синглы"),
                            (ObjectKind::Artist, "Артисты"),
                            (ObjectKind::Playlist, "Плейлисты"),
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(index, (kind, name))| {
                            section_tab(
                                ("catalog-search-kind", index),
                                name,
                                search.kind == kind,
                                24 + index as isize,
                                controls,
                            )
                            .on_activate(cx.listener({
                                let query = query.clone();
                                move |view, _, window, cx| {
                                    if view.music.search.kind != kind {
                                        view.show_catalog_query(kind, query.clone(), window, cx);
                                    }
                                }
                            }))
                        }),
                    ),
            )
            .when(query.trim().is_empty(), |page| {
                page.child(status(
                    "music-search-prompt",
                    "Введите название или вставьте музыкальную ссылку.",
                ))
            })
            .when(
                !pending
                    && search.submitted
                    && search.candidates.is_empty()
                    && search.saved.is_empty()
                    && search.direct.is_none()
                    && search.unresolved.is_none()
                    && search.errors.is_empty(),
                |page| {
                    page.child(crate::ui::components::content_state::empty_state(
                        "music-search-empty",
                        if !catalogs && !link {
                            "В медиатеке ничего не найдено. Подключите площадку для поиска в её каталоге."
                        } else {
                            "Подходящих записей не найдено."
                        },
                        Some("Проверьте название или попробуйте другой запрос."),
                        IconName::Search,
                        cx,
                    ))
                },
            )
            .when_some(search.direct.clone(), |page, (object, url)| {
                page.child(
                    div().flex().child(
                        Button::new(
                            "music-search-open-url",
                            format!("Открыть в Mediadeka · {}", object.provider.display_name()),
                        )
                        .variant(Variant::Light)
                        .tab_order(42)
                        .when_some(controls, |button, material| {
                            ControlVariant::SecondaryButton.text_button(button, material)
                        })
                        .on_click(cx.listener(
                            move |view, _, window, cx| {
                                if view.music_page() == Page::Search
                                    && view.music.search.direct.as_ref().is_some_and(
                                        |(current, address)| current == &object && address == &url,
                                    )
                                {
                                    view.open_search_address(
                                        object.clone(),
                                        url.clone(),
                                        window,
                                        cx,
                                    );
                                }
                            },
                        )),
                    ),
                )
            })
            .when_some(search.unresolved.as_ref(), |page, link| {
                page.child(self.unresolved_search_link(link, cx))
            })
            .children(search.saved.iter().enumerate().map(|(index, saved)| {
                let entry = &saved.entry;
                let current = glass && self.music_entry_is_current(&entry.action, cx);
                let subtitle = [entry.subtitle.as_str(), entry.key.0.label()]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ");
                let playable = glass && matches!(&entry.action,
                    EntryAction::Track(key, track) if can_resolve(key, track));
                let play = playable.then(|| {
                    let entry = entry.clone();
                    row_style.cover_play_button(
                        format!("music-search-saved-play-{index}"),
                        format!("Слушать · {}", entry.title),
                        FIRST_RESULT_TAB_INDEX + index as isize,
                        SEARCH_COVER,
                    ).on_activate_at(cx.listener(move |view, origin, window, cx| {
                        cx.stop_propagation();
                        if view.music_page() != Page::Search
                            || !view.music.search.saved.get(index)
                                .is_some_and(|current| Arc::ptr_eq(&current.entry, &entry))
                        {
                            return;
                        }
                        if let EntryAction::Track(key, track) = &entry.action {
                            view.play_from_fluid_row(*origin, window, cx, |view, _, cx| {
                                view.play_library_track(key, track, cx);
                            });
                        }
                    }))
                });
                let result = row(
                    ("music-search-saved", index),
                    index,
                    entry.title.clone(),
                    subtitle,
                    SearchArtwork {
                        url: entry.artwork.as_deref(),
                        seed: crate::ui::fluid_glass::artwork::Seed::entry(&entry.action),
                        play,
                    },
                    entry.key.0.provider(),
                    if glass { SearchRowStyle::Fluid { current: current.then_some(&self.player_snapshot.status), row_style } } else { SearchRowStyle::Classic },
                )
                .relative()
                .children(self.prefetch_visibility_probe(entry_target(entry)))
                .track_focus(&saved.focus)
                .on_activate_at(cx.listener({
                    let entry = entry.clone();
                    move |view, origin, window, cx| {
                        if view.music_page() == Page::Search
                            && view
                                .music
                                .search
                                .saved
                                .get(index)
                                .is_some_and(|current| Arc::ptr_eq(&current.entry, &entry))
                        {
                            view.play_from_fluid_row(*origin, window, cx, |view, window, cx| {
                                view.activate_library_entry(
                                    &entry.action,
                                    &entry.title,
                                    entry.artwork.as_deref(),
                                    window,
                                    cx,
                                )
                            });
                        }
                    }
                }));
                let action = self.music_queue_button(
                    format!("studio-search-queue-{index}"), 500 + index as isize,
                    &entry.action, cx,
                );
                if current || action.is_some() {
                    let result = div().flex().items_center()
                        .child(result.flex_1().min_w_0())
                        .children(action);
                    row_style.surface(result, current).into_any_element()
                } else {
                    result.into_any_element()
                }
            }))
            .children(search.candidates.iter().enumerate().map(|(index, entry)| {
                let candidate = &entry.candidate;
                let page = &candidate.page;
                let provider = page.reference.provider;
                let current = glass.then(|| {
                    self.player.read(cx).catalogue_track_status(
                        &page.reference,
                        candidate.profile.as_deref(),
                    )
                }).flatten();
                let order = search.saved.len() + index;
                let play = if glass && page.reference.kind == ObjectKind::Track {
                    let key = playlist_key(candidate);
                    page.tracks.first().filter(|track| can_resolve(&key, track)).map(|track| {
                        let candidate = candidate.clone();
                        let track = track.clone();
                        row_style.cover_play_button(
                            format!("{}-play", candidate_id(&page.reference)),
                            format!("Слушать · {}", page.title),
                            FIRST_RESULT_TAB_INDEX + order as isize,
                            SEARCH_COVER,
                        ).on_activate_at(cx.listener(move |view, origin, window, cx| {
                            cx.stop_propagation();
                            if view.music_page() == Page::Search && view.music.search.contains(&candidate) {
                                view.play_from_fluid_row(*origin, window, cx, |view, _, cx| {
                                    view.play_music_tracks([(key.clone(), &track)], PlaybackAction::Play, cx);
                                });
                            }
                        }))
                    })
                } else {
                    None
                };
                let result = row(
                    candidate_id(&page.reference),
                    order,
                    page.title.clone(),
                    candidate_label(page, provider),
                    SearchArtwork { url: page.artwork.as_deref(), seed: None, play },
                    Some(provider),
                    if glass { SearchRowStyle::Fluid { current, row_style } } else { SearchRowStyle::Classic },
                )
                .relative()
                .children(self.prefetch_visibility_probe([Destination::Object(candidate_target(candidate))]))
                .track_focus(&entry.focus)
                .on_activate(cx.listener({
                    let candidate = candidate.clone();
                    move |view, _, window, cx| {
                        if view.music_page() == Page::Search
                            && view.music.search.contains(&candidate)
                        {
                            view.open_candidate(&candidate, window, cx);
                        }
                    }
                }));
                row_style.surface(result, current.is_some())
            }))
            .when(pending && search.saved.is_empty() && search.candidates.is_empty() && search.direct.is_none() && search.unresolved.is_none(), |page| {
                let label = if link {
                        "Открываем ссылку…"
                    } else if catalogs {
                        "Ищем в медиатеке и подключённых каталогах…"
                    } else {
                        "Ищем в медиатеке…"
                    };
                if glass {
                    page.child(loading_state("music-search-loading", label, cx))
                } else {
                    page.child(status("music-search-loading", label))
                }
            })
            .children(search.errors.iter().enumerate().map(|(index, error)| {
                if glass {
                    error_state(format!("music-search-error-{index}"), error.clone(), None, cx).into_any_element()
                } else {
                    div().child(error.clone()).into_any_element()
                }
            }))
    }

    fn unresolved_search_link(
        &self,
        link: &UnresolvedLink,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let action = if link.error.needs_sign_in() {
            self.provider_recovery_action("music-search-source-connect", link.provider, 42, cx)
        } else {
            let url = link.url.clone();
            Button::new("music-search-source-retry", "Повторить")
                .tab_order(42)
                .on_click(cx.listener(move |view, _, _, cx| {
                    if view.music_page() == Page::Search
                        && view
                            .music
                            .search
                            .unresolved
                            .as_ref()
                            .is_some_and(|link| link.url == url)
                    {
                        view.focus_workspace_page();
                        view.search_music(view.music.search.kind, url.clone(), cx);
                    }
                }))
        };
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(status(
                "music-search-source-error",
                link.error.message.clone(),
            ))
            .child(
                action
                    .variant(Variant::Light)
                    .when_some(self.glass_controls(cx), |button, material| {
                        ControlVariant::TextAction.text_button(button, material)
                    }),
            )
    }

    fn open_candidate(&mut self, candidate: &ScopedPage, window: &Window, cx: &mut Context<Self>) {
        let page = &candidate.page;
        let object = page.reference.clone();
        if object.kind == ObjectKind::Playlist {
            let item = HomeItem {
                id: object.id.clone(),
                title: page.title.clone(),
                subtitle: String::new(),
                url: page.url.clone(),
                artwork: page.artwork.clone(),
            };
            self.show_playlist(
                Target::from_item(
                    object.provider,
                    candidate.profile.as_deref(),
                    object.id,
                    item,
                ),
                window,
                cx,
            );
        } else {
            self.open_music_target(candidate_target(candidate), window, cx);
        }
    }
}

fn candidate_target(candidate: &ScopedPage) -> MusicTarget {
    let page = &candidate.page;
    MusicTarget {
        object: page.reference.clone(),
        profile: candidate.profile.clone(),
        title: page.title.clone(),
        artwork: page.artwork.clone(),
    }
}

fn candidate_id(object: &ObjectRef) -> SharedString {
    format!(
        "music-catalog-result-{}-{}-{}",
        object.provider.as_str(),
        object.kind.path(),
        object.id,
    )
    .into()
}

fn candidate_label(page: &crate::music::MusicPage, provider: ProviderId) -> String {
    let mut details = page.artists.clone();
    details.extend(page.album.clone());
    details.extend(page.year.map(|year| year.to_string()));
    details.extend(page.version.clone());
    details.extend(
        page.duration
            .map(|ms| format!("{}:{:02}", ms / 60_000, ms / 1000 % 60)),
    );
    details.push(provider.display_name().into());
    details.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_row_identity_includes_provider_and_object_kind() {
        let primary = ObjectRef {
            provider: ProviderId::Spotify,
            kind: ObjectKind::Track,
            id: "123".into(),
        };
        let other_provider = ObjectRef {
            provider: ProviderId::YandexMusic,
            ..primary.clone()
        };
        let other_kind = ObjectRef {
            kind: ObjectKind::Album,
            ..primary.clone()
        };
        assert_ne!(candidate_id(&primary), candidate_id(&other_provider));
        assert_ne!(candidate_id(&primary), candidate_id(&other_kind));
        assert_eq!(candidate_id(&primary), candidate_id(&primary.clone()));
    }
}
