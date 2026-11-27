use gpui::{
    AccessibleAction, AnyElement, ClickEvent, Context, Div, FontWeight, IntoElement, Role, Window,
    div, prelude::*, px, rgb,
};
use guise::{Icon, IconName};

use crate::ui::{
    components::{
        Activate, activation_position, button,
        music::{fluid_cover, time_label},
        overlay::{SelectChoice, SelectLabel},
        search_input::{Scope, SearchInput},
        track_row,
    },
    fluid_glass::{artwork::Seed, tokens::text},
    labels::readable_date,
    music::detail_style::Ink,
    theme::{focus_ring, playlist::*},
};
use crate::{
    playback::{Status, can_resolve},
    playlist::PlaylistTrack,
};

use super::{
    MainView, Sort,
    actions::QueueAction,
    artwork::track_cover,
    page::{Layout, PINNED_SEARCH_ROW, column_header_height},
    popup::MenuKind,
};

const INDEX_WIDTH: f32 = 28.;
const COVER_GAP: f32 = metrics::PAD;
const ACTION_WIDTH: f32 = metrics::CONTROL_SMALL * 2. + metrics::GAP_SMALL;

impl MainView {
    pub(super) fn playlist_toolbar(
        &self,
        layout: Layout,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let controls = self.glass_controls(cx);
        let ink = Ink::new(controls.is_some(), self.fluid_high_contrast(cx));
        let search = if let Some(material) = controls {
            SearchInput::new(
                "playlist-search",
                self.playlist.search.clone(),
                material,
                Scope::Local,
                35,
                layout.compact,
            )
            .into_any_element()
        } else {
            div()
                .h(px(if layout.compact { 30. } else { 34. }))
                .px(px(9.))
                .gap(px(8.))
                .flex()
                .items_center()
                .border_1()
                .border_color(ink.rule)
                .rounded(px(RADIUS))
                .bg(rgb(PANEL))
                .child(
                    Icon::new(IconName::Search)
                        .pixel_size(px(15.))
                        .color(ink.secondary),
                )
                .child(div().min_w_0().flex_1().child(self.playlist.search.clone()))
                .when(!layout.compact, |field| {
                    field.child(
                        div()
                            .flex_none()
                            .text_size(px(11.))
                            .text_color(ink.secondary)
                            .child("Ctrl+F"),
                    )
                })
                .into_any_element()
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .when(controls.is_some(), |toolbar| {
                        toolbar.h(px(PINNED_SEARCH_ROW))
                    })
                    .when(controls.is_none(), |toolbar| {
                        toolbar.pb(px(if layout.compact { 3. } else { 10. }))
                    })
                    .child(
                        div()
                            .w(px(if layout.compact { 228. } else { 300. }))
                            .min_w_0()
                            .child(search),
                    )
                    .child(div().min_w_0().flex_1())
                    .when(controls.is_some(), |toolbar| {
                        toolbar.child(
                            div().w(px(24.)).flex_none().children(
                                self.playlist
                                    .open
                                    .as_ref()
                                    .filter(|open| open.request.is_some())
                                    .map(|_| {
                                        crate::ui::components::content_state::refresh_indicator(
                                            "playlist-refreshing",
                                            "Обновляем плейлист…",
                                            cx,
                                        )
                                    }),
                            ),
                        )
                    })
                    .child(self.playlist_sort_select(window, cx))
                    .when(layout.album_available || controls.is_some(), |toolbar| {
                        toolbar.child(self.playlist_menu_trigger(
                            "playlist-columns",
                            "Настройки отображения",
                            IconName::SlidersHorizontal,
                            37,
                            MenuKind::Columns {
                                album: layout.album_available,
                                date: layout.date_available,
                            },
                            cx,
                        ))
                    }),
            )
            .child(
                layout
                    .cells()
                    .id("playlist-track-columns")
                    .accessibility_id("playlist-track-columns")
                    .role(Role::Group)
                    .aria_label("Колонки треков")
                    .h(px(column_header_height(layout.compact)))
                    .when(controls.is_none(), |header| header.bg(rgb(PANEL)))
                    .border_b_1()
                    .border_color(ink.rule)
                    .text_size(px(12.))
                    .text_color(ink.secondary)
                    .child(fixed(INDEX_WIDTH).text_right().child("#"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .pl(px(layout.row_cover + COVER_GAP))
                            .child("Название"),
                    )
                    .children(layout.artist.map(|width| fixed(width).child("Исполнитель")))
                    .children(layout.album.map(|width| fixed(width).child("Альбом")))
                    .children(
                        layout
                            .added
                            .map(|width| fixed(width).child("Дата добавления")),
                    )
                    .child(fixed(layout.duration).text_right().child(
                        if controls.is_none() && layout.duration > 60. {
                            "Длительность"
                        } else {
                            "Время"
                        },
                    ))
                    .child(fixed(ACTION_WIDTH)),
            )
    }

    fn playlist_sort_select(&self, window: &Window, cx: &Context<Self>) -> Div {
        let choices = Sort::ALL
            .into_iter()
            .map(|sort| {
                SelectChoice::new(
                    format!("playlist-sort-{}", sort.id()),
                    sort.label(),
                    self.playlist.sort == sort,
                )
            })
            .collect();
        self.playlist.sort_select.view(
            SelectLabel {
                trigger: "playlist-sort",
                menu: "playlist-sort-options",
                label: "Сортировка",
                value: self.playlist.sort.label().into(),
                enabled: true,
            },
            choices,
            |view| &mut view.playlist.sort_select,
            |view, index, _, cx| {
                let sort = Sort::ALL[index];
                if view.playlist.sort != sort {
                    view.playlist.sort = sort;
                    view.playlist.project(true);
                    view.reset_list_prefetch(cx);
                }
            },
            self.glass_controls(cx),
            self.uses_fluid_glass() && !self.reduced_motion && !self.fluid_high_contrast(cx),
            window,
            cx,
        )
    }

    pub(super) fn playlist_row(
        &self,
        index: usize,
        track: &PlaylistTrack,
        layout: Layout,
        active: bool,
        selected: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(open) = &self.playlist.open else {
            return div().into_any_element();
        };
        let available = can_resolve(&open.target.key, track);
        let mut title = track.title.clone();
        if !track.artist.trim().is_empty() {
            title.push_str(" · ");
            title.push_str(&track.artist);
        }
        if !available {
            title.push_str(" · аудио недоступно");
        }
        let paused = active && matches!(self.player_snapshot.status, Status::Paused);
        let controls = self.glass_controls(cx);
        let glass = controls.is_some();
        let high_contrast = self.fluid_high_contrast(cx);
        let row_style = track_row::Style::new(cx, high_contrast);
        let ink = Ink::new(glass, high_contrast);
        let key = &open.target.key;
        let related = super::navigation::row_prefetch_targets(key, track);
        let art_span = crate::ui::panel::playlist_track_artwork_span(key, index);
        let order = 100 + index as isize * 3;
        let title_id = format!("playlist-track-title-{index}");
        let artist_id = format!("playlist-track-artist-{index}");
        let artwork = if glass {
            let cover = fluid_cover(
                track.artwork.as_deref(),
                Seed::track(track),
                layout.row_cover,
                5.,
            )
            .when(available, |cover| {
                cover.child(
                    row_style
                        .cover_play_button(
                            format!("playlist-cover-play-{index}"),
                            format!("Слушать · {}", track.title),
                            order,
                            layout.row_cover,
                        )
                        .on_activate_at(cx.listener(move |view, origin, window, cx| {
                            cx.stop_propagation();
                            view.select_track(index, window, cx);
                            view.play_from_fluid_row(*origin, window, cx, |view, _, cx| {
                                view.playlist_queue(QueueAction::Play, Some(index), cx)
                            });
                        })),
                )
            })
            .when(self.inspector_is_moving(art_span), |art| art.opacity(0.));
            crate::ui::music::detail::hero_motion::shared_artwork(cover, art_span, None)
        } else {
            track_cover(track.artwork.as_deref(), metrics::THUMB).into_any_element()
        };
        let activate = cx.listener(move |view, _: &(), window, cx| {
            view.select_track(index, window, cx);
            if available {
                view.playlist_queue(QueueAction::Play, Some(index), cx);
            }
        });
        let row = button(("playlist-row", index), title, 100 + index as isize * 3)
            .accessibility_id(format!("playlist-row-{index}"))
            .role(Role::ListItem)
            .aria_selected(selected)
            .when(active, |row| {
                row.aria_description(track_row::current_description(&self.player_snapshot.status))
            })
            .w_full()
            .h(px(layout.row_height))
            .relative()
            .when(glass && available, |row| row.group(track_row::HOVER_GROUP))
            .children(self.prefetch_visibility_probe(related))
            .px(px(metrics::PAD - 1.))
            .gap(px(metrics::PAD))
            .justify_start()
            .border_color(gpui::transparent_black())
            .when(!selected, |row| {
                row.child(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .h(px(1.))
                        .bg(ink
                            .rule
                            .opacity(if glass && !high_contrast { 0.40 } else { 1. })),
                )
            })
            .when(!glass, |row| {
                row.when(active, |row| row.bg(rgb(PLAYING)))
                    .when(selected && !active, |row| row.bg(rgb(SELECTED)))
                    .when(selected, |row| row.border_color(rgb(SELECTION_EDGE)))
                    .focus_visible(|style| style.border_color(focus_ring()))
                    .hover(move |style| {
                        style.bg(rgb(if active {
                            PLAYING
                        } else if selected {
                            SELECTED
                        } else {
                            HOVER
                        }))
                    })
            })
            .when(glass, |row| {
                row_style
                    .surface(row_style.interaction(row), active)
                    .when(selected, |row| row_style.selected(row))
            })
            .on_click(cx.listener(move |view, event: &ClickEvent, window, cx| {
                view.select_track(index, window, cx);
                if available
                    && (matches!(event, ClickEvent::Keyboard(_)) || event.click_count() > 1)
                {
                    let origin = activation_position(event);
                    view.play_from_fluid_row(Some(origin), window, cx, |view, _, cx| {
                        view.playlist_queue(QueueAction::Play, Some(index), cx)
                    });
                }
            }))
            .on_a11y_action(AccessibleAction::Click, move |_, window, cx| {
                activate(&(), window, cx)
            })
            .child(
                fixed(INDEX_WIDTH)
                    .flex()
                    .items_center()
                    .justify_end()
                    .text_size(px(12.))
                    .text_color(ink.secondary)
                    .when(active, |cell| {
                        cell.child(if glass {
                            row_style.playback_marker(&self.player_snapshot.status)
                        } else {
                            Icon::new(if paused {
                                IconName::Pause
                            } else {
                                IconName::AudioLines
                            })
                            .pixel_size(px(metrics::GLYPH))
                            .color(gpui::Hsla::from(rgb(SIGNAL)))
                        })
                    })
                    .when(!active, |cell| cell.child((index + 1).to_string())),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(COVER_GAP))
                    .child(artwork)
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(crate::ui::components::entity_link::track(
                                div()
                                    .truncate()
                                    .text_size(px(if glass && !layout.compact {
                                        text::TRACK_TITLE
                                    } else {
                                        14.
                                    }))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if active {
                                        if glass {
                                            row_style.title(true)
                                        } else {
                                            rgb(SIGNAL).into()
                                        }
                                    } else if available {
                                        ink.primary
                                    } else {
                                        ink.secondary
                                    }),
                                &title_id,
                                key,
                                track,
                                order,
                                cx,
                            ))
                            .when(layout.artist.is_none(), |text| {
                                text.child(crate::ui::components::entity_link::artist_names(
                                    div().text_size(px(12.)).text_color(ink.secondary),
                                    &artist_id,
                                    key,
                                    track,
                                    order,
                                    cx,
                                ))
                            }),
                    ),
            )
            .children(layout.artist.map(|width| {
                crate::ui::components::entity_link::artist_names(
                    fixed(width).text_size(px(13.)).text_color(ink.secondary),
                    &artist_id,
                    key,
                    track,
                    order,
                    cx,
                )
            }))
            .children(layout.album.map(|width| {
                if let Some((name, target)) = super::navigation::album(key, track) {
                    crate::ui::components::entity_link::text(
                        fixed(width)
                            .truncate()
                            .text_size(px(12.))
                            .text_color(ink.secondary),
                        &format!("playlist-track-album-{index}"),
                        name,
                        Some(target),
                        order,
                        cx,
                    )
                } else {
                    cell(width, None, ink.secondary).into_any_element()
                }
            }))
            .children(layout.added.map(|width| {
                let added = track.added.as_deref().map(readable_date);
                cell(width, added.as_deref(), ink.secondary)
            }))
            .child(
                fixed(layout.duration)
                    .text_size(px(12.))
                    .text_color(ink.secondary)
                    .text_right()
                    .child(
                        track
                            .duration
                            .map_or_else(|| "—".into(), |time| time_label(f64::from(time))),
                    ),
            )
            .child(
                div()
                    .w(px(ACTION_WIDTH))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(metrics::GAP_SMALL))
                    .child(
                        self.like_control(
                            ("playlist-like", index),
                            101 + index as isize * 3,
                            key,
                            track,
                            cx,
                        )
                        .debug_selector(|| format!("playlist-like-{index}")),
                    )
                    .child(self.playlist_menu_trigger(
                        ("playlist-row-menu", index),
                        "Действия трека",
                        IconName::Ellipsis,
                        102 + index as isize * 3,
                        MenuKind::Track(index),
                        cx,
                    )),
            );
        row.into_any_element()
    }
}

fn cell(width: f32, value: Option<&str>, ink: gpui::Hsla) -> Div {
    fixed(width)
        .truncate()
        .text_size(px(12.))
        .text_color(ink)
        .child(value.unwrap_or("—").to_owned())
}

fn fixed(width: f32) -> Div {
    div().w(px(width)).min_w_0().flex_none()
}
