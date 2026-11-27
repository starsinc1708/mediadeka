use gpui::{App, Context, Div, Pixels, Role, Window, accesskit::Live, div, prelude::*, px};
use guise::{Button, IconName, Size, Variant};

use crate::{
    music::library::Scope,
    ui::{
        MainView,
        assets::provider_mark,
        components::{
            Activate,
            content_state::{empty_state, error_state, loading_state, refresh_indicator},
            section_tab,
        },
        design_system::controls::ControlVariant,
        fluid_glass::tokens::text,
        labels::{counted, track_count},
        music::{heading, search_box, status},
        theme::{secondary, surface_border},
        workspace::{LibraryTab, Page},
    },
};

use super::list::{COLUMN_HEADER_HEIGHT, columns, date_label};

impl MainView {
    pub(super) fn library_header(
        &self,
        width: f32,
        wide: bool,
        show_tabs: bool,
        group_hero_height: Option<Pixels>,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let glass = self.uses_fluid_glass();
        let controls = self.glass_controls(cx);
        let library = &self.music.collections;
        let local_group = library.location.group.as_ref().filter(|_| glass);
        let tab = self
            .music_page()
            .library_tab()
            .unwrap_or(LibraryTab::Tracks);
        let tracks = tab == LibraryTab::Tracks;
        let albums = tab == LibraryTab::Albums;
        let list = self.shows_library_rows();
        let filter_controls = library.shows_filter_controls();
        let show_filter_row = !glass
            || library.filters_open
            || !library.location.query.is_empty()
            || library.location.group.is_some()
            || self.library_applied_filters().is_some();
        let count = library.results.visible.len();
        let refreshing = count > 0 && self.music_request_pending();
        let count_label = match tab {
            LibraryTab::Tracks => track_count(count),
            LibraryTab::Albums => counted(count, "альбом", "альбома", "альбомов"),
            LibraryTab::Artists => counted(count, "исполнитель", "исполнителя", "исполнителей"),
            LibraryTab::Playlists => counted(count, "плейлист", "плейлиста", "плейлистов"),
        };
        let summary = || {
            format!(
                "{} · {} скачано · Первое добавление {}",
                counted(
                    library.results.visible_sources,
                    "площадка",
                    "площадки",
                    "площадок"
                ),
                library.results.downloaded,
                date_label(library.results.first_added),
            )
        };
        let heading_id = if tracks {
            "music-liked-title"
        } else {
            "library-title"
        };
        let title = if !show_tabs && !tracks {
            tab.label()
        } else {
            "Медиатека"
        };
        let title_size = match (glass, wide) {
            (true, true) => text::PAGE_TITLE,
            (true, false) => text::PAGE_TITLE_COMPACT,
            (false, true) => 36.,
            (false, false) => 25.,
        };
        div()
            .flex()
            .flex_col()
            .w_full()
            .pt(px(if group_hero_height.is_some() {
                0.
            } else if wide && glass {
                28.
            } else if wide {
                14.
            } else {
                7.
            }))
            .gap(px(if wide { 8. } else { 7. }))
            .when_some(group_hero_height, |header, height| {
                // Keep the virtual first item's height stable as the visible
                // hero shortens above the scrolling layer.
                header.child(div().h(height).flex_none())
            })
            .when(local_group.is_none(), |header| {
                header.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_w_0()
                                .flex_wrap()
                                .items_center()
                                .gap(px(12.))
                                .child(heading(heading_id, title).text_size(px(title_size)))
                                .child(
                                    status("library-counts", count_label)
                                        .text_size(px(text::CAPTION)),
                                )
                                .when(glass && filter_controls, |title| {
                                    title
                                        .child(div().flex_1())
                                        .child(self.library_filters_trigger(!wide, cx))
                                }),
                        )
                        .child(refresh_slot(refreshing, cx)),
                )
            })
            .when(show_tabs && local_group.is_none(), |header| {
                header.child(
                    div()
                        .id("library-tabs")
                        .accessibility_id("library-tabs")
                        .role(Role::TabList)
                        .aria_label("Разделы медиатеки")
                        .flex()
                        .flex_wrap()
                        .gap(px(4.))
                        .border_b_1()
                        .border_color(surface_border())
                        .children(
                            LibraryTab::ALL
                                .into_iter()
                                .enumerate()
                                .map(|(index, page)| {
                                    section_tab(
                                        format!("library-tab-{}", page.id()),
                                        page.label(),
                                        page == tab,
                                        20 + index as isize,
                                        controls,
                                    )
                                    .track_focus(self.library_tab_focus(page))
                                    .on_activate(cx.listener(move |view, _, window, cx| {
                                        view.show_library_tab(page, window, cx);
                                    }))
                                }),
                        ),
                )
            })
            .when(filter_controls && show_filter_row, |header| {
                header.child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(px(8.))
                        .when(
                            !glass || library.filters_open || !library.location.query.is_empty(),
                            |row| {
                                row.child(
                                    div()
                                        .w(px(if wide { 330. } else { 236. }))
                                        .max_w_full()
                                        .child(search_box(
                                            self.music.filter.clone(),
                                            controls,
                                            !wide,
                                        )),
                                )
                            },
                        )
                        .when(!glass || local_group.is_some(), |row| {
                            row.child(self.library_filters_trigger(!wide, cx))
                        })
                        .children(self.library_applied_filters().map(|labels| {
                            status("library-applied-filters", labels)
                                .min_w_0()
                                .text_size(px(12.))
                        }))
                        .when_some(library.location.group.as_ref(), |row, group| {
                            row.child(
                                Button::new(
                                    "library-group",
                                    if glass {
                                        "Все локальные треки".to_owned()
                                    } else {
                                        format!("{} ×", group.title())
                                    },
                                )
                                .size(Size::Xs)
                                .tab_order(46)
                                .on_click(cx.listener(
                                    |view, _, _, cx| {
                                        view.change_library_filters(
                                            |location| location.group = None,
                                            cx,
                                        );
                                    },
                                )),
                            )
                        })
                        .when(local_group.is_some(), |row| {
                            row.child(refresh_slot(refreshing, cx))
                        }),
                )
            })
            .when(
                library.filters_open && library.shows_filter_controls(),
                |header| {
                    header.child(self.library_filters_panel(
                        summary(),
                        albums && !glass,
                        list,
                        window,
                        cx,
                    ))
                },
            )
            .when_some(tab.collection_kind().filter(|_| glass), |header, kind| {
                header.child(self.collection_view_controls(kind, cx))
            })
            .when(tab == LibraryTab::Playlists, |header| {
                header.child(self.local_playlist_actions(cx))
            })
            .children(self.collection_status(tab, cx))
            .when(
                !self.music_request_pending() && count == 0 && library.error.is_none(),
                |header| header.child(self.library_empty(tab, cx)),
            )
            .when(count > 0 && list, |header| {
                if group_hero_height.is_some() {
                    header.child(div().h(px(COLUMN_HEADER_HEIGHT)).flex_none())
                } else {
                    header.child(
                        columns(self.library_table(width, wide, tracks), tracks, glass)
                            .border_b_1()
                            .border_color(surface_border()),
                    )
                }
            })
    }

    fn library_empty(&self, tab: LibraryTab, cx: &Context<Self>) -> Div {
        let id = if tab == LibraryTab::Tracks {
            "liked-empty"
        } else {
            "library-empty"
        };
        if self.music.collections.narrows_results() {
            return div()
                .when(
                    self.uses_fluid_glass() && !self.reduced_motion && !cx.reduce_motion(),
                    |state| state.opacity(self.music.collections.list.reflow.empty_opacity()),
                )
                .child(empty_state(
                    id,
                    "Ничего не найдено",
                    Some("Измените запрос или сбросьте фильтры."),
                    IconName::Search,
                    cx,
                ));
        }
        let (title, explanation, icon) = match tab {
            LibraryTab::Tracks => (
                "Нет сохранённых треков",
                "Подключите площадку или откройте файлы с компьютера.",
                IconName::Music,
            ),
            LibraryTab::Albums => (
                "Нет сохранённых альбомов",
                "Здесь появятся альбомы подключённых площадок.",
                IconName::Disc3,
            ),
            LibraryTab::Artists => (
                "Нет сохранённых исполнителей",
                "Здесь появятся исполнители подключённых площадок.",
                IconName::Users,
            ),
            LibraryTab::Playlists => (
                "Нет плейлистов",
                "Создайте плейлист или импортируйте файлы, папку или M3U.",
                IconName::ListMusic,
            ),
        };
        div().child(
            empty_state(id, title, Some(explanation), icon, cx).child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap(px(8.))
                    .when(tab != LibraryTab::Playlists, |actions| {
                        actions.child(
                            Button::new("library-empty-import", "Перейти к импорту")
                                .variant(Variant::Light)
                                .size(Size::Sm)
                                .tab_order(47)
                                .when_some(self.glass_controls(cx), |button, material| {
                                    ControlVariant::TextAction.text_button(button, material)
                                })
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.show_music_page(Page::Playlists, window, cx);
                                })),
                        )
                    })
                    .child(
                        Button::new("library-empty-connect", "Подключить площадку")
                            .variant(Variant::Default)
                            .size(Size::Sm)
                            .tab_order(49)
                            .when_some(self.glass_controls(cx), |button, material| {
                                ControlVariant::SecondaryButton.text_button(button, material)
                            })
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.show_music_page(Page::Settings, window, cx);
                            })),
                    ),
            ),
        )
    }

    pub(super) fn toggle_album_view(&mut self, cx: &mut Context<Self>) {
        let location = &mut self.music.collections.location;
        location.album_list = !location.album_list;
        cx.notify();
    }

    fn collection_status(&self, tab: LibraryTab, cx: &Context<Self>) -> Option<Div> {
        let library = &self.music.collections;
        let controls = self.glass_controls(cx);
        let loading = self.music.request.is_some();
        let cold_loading = loading && library.results.visible.is_empty();
        let errors = || {
            library.errors.iter().filter(|&(&provider, _)| match &library.location.source {
            None => true,
            Some(scope) => matches!(scope, Scope::Provider { provider: selected, .. } if provider == *selected),
        })
        };
        if !cold_loading && library.error.is_none() && errors().next().is_none() {
            return None;
        }
        let reconnect = errors()
            .filter_map(|(&provider, (_, revoked))| revoked.then_some(provider.display_name()))
            .collect::<Vec<_>>();
        Some(
            div()
                .flex()
                .flex_none()
                .flex_col()
                .gap(px(5.))
                .when(cold_loading, |group| {
                    group.child(loading_state(
                        "collection-loading",
                        "Загружаем коллекции…",
                        cx,
                    ))
                })
                .when_some(library.error.as_ref(), |group, error| {
                    if library.results.visible.is_empty() {
                        group.child(error_state("collection-error", error.clone(), None, cx))
                    } else {
                        group.child(status("collection-error", error.clone()).text_size(px(12.)))
                    }
                })
                .when(!reconnect.is_empty(), |group| {
                    group.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(7.))
                            .child(
                                status(
                                    "collection-reconnect",
                                    format!("Нужен повторный вход · {}", reconnect.join(", ")),
                                )
                                .text_size(px(12.)),
                            )
                            .child(
                                Button::new("collection-settings", "Подключения")
                                    .variant(Variant::Subtle)
                                    .tab_order(48)
                                    .size(Size::Xs)
                                    .when_some(controls, |button, material| {
                                        ControlVariant::TextAction.text_button(button, material)
                                    })
                                    .on_click(cx.listener(|view, _, window, cx| {
                                        view.show_music_page(Page::Settings, window, cx)
                                    })),
                            ),
                    )
                })
                .children(errors().filter(|(_, (_, revoked))| !revoked).map(
                    |(&provider, (error, _))| {
                        let name = provider.display_name();
                        let message = if error.starts_with(name) {
                            error.clone()
                        } else {
                            format!("{name} · {error}")
                        };
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(7.))
                            .text_size(px(12.))
                            .text_color(secondary())
                            .child(provider_mark(provider, 14.))
                            .child(message)
                    },
                ))
                .when(
                    !loading
                        && (library.error.is_some() || errors().any(|(_, (_, revoked))| !revoked)),
                    |group| {
                        group.child(
                            div().flex().child(
                                Button::new("collection-retry", "Повторить обновление")
                                    .variant(Variant::Default)
                                    .tab_order(47)
                                    .size(Size::Xs)
                                    .when_some(controls, |button, material| {
                                        ControlVariant::TextAction.text_button(button, material)
                                    })
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        view.load_music_collections(tab, cx)
                                    })),
                            ),
                        )
                    },
                ),
        )
    }
}

fn refresh_slot(refreshing: bool, cx: &App) -> Div {
    div().size(px(24.)).flex_none().when(refreshing, |slot| {
        slot.child(
            refresh_indicator("collection-loading", "Обновляем коллекции…", cx)
                .a11y_synthetic_children(|builder| {
                    builder.parent_node().set_live(Live::Polite);
                }),
        )
    })
}
