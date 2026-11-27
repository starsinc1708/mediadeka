use gpui::{
    AnyElement, Context, Div, FontWeight, IntoElement, Pixels, Point, Role, Window,
    accesskit::Toggled, div, prelude::*, px, rgb,
};
use guise::{Icon, IconName};

use crate::{
    appearance::TrackColumn,
    music::{ArtistDetails, MusicPage, ObjectKind, ObjectRef, ScopedPage},
    playback::can_resolve,
    playlist::{PlaylistKey, PlaylistTrack},
    ui::{
        MainView,
        components::{
            Activate, button, entity_link,
            glass_control::Tone,
            music::{card_title, cover, fluid_cover, time_label},
            paint_section_tab, track_row,
        },
        design_system::adaptive_layout::TrackColumns,
        fluid_glass::{artwork::Seed, tokens::text},
        playlist::{navigation::Destination, popup::MenuKind},
        theme::{ACTIVE, ELEVATED, SURFACE, secondary, surface_border},
        workspace::MusicTarget,
    },
};

use super::{
    PlaybackAction,
    detail::{Detail, object_entry},
    detail_style::{self, Ink},
    heading,
};

mod artist_track;

const ARTIST_PREVIEW_TRACKS: usize = 5;
const ARTIST_TRACK_LIST_MAX_WIDTH: f32 = 800.;
const TRACK_MENU_WIDTH: f32 = 43.;
const HERO_COLUMNS_HEIGHT: f32 = 33.;
const TABLE_COVER: f32 = 30.;
const TABLE_GAP: f32 = 9.;

struct TrackRow<'a> {
    index: usize,
    key: &'a PlaylistKey,
    track: &'a PlaylistTrack,
    compact: bool,
    artwork: bool,
    artist_presentation: bool,
    animate_artist_mark: bool,
    playable: bool,
    active: bool,
}

#[derive(Clone, Copy)]
struct TrackTable {
    columns: Option<TrackColumns>,
    row_height: Option<f32>,
    artist_inline: bool,
}

impl TrackTable {
    fn artist_column(self) -> Option<Div> {
        let column = div().flex_none().min_w_0();
        match self.columns {
            Some(columns) => columns.artist.map(|width| column.w(px(width))),
            None if !self.artist_inline => Some(column.w_1_4()),
            None => None,
        }
    }

    fn time_column(self) -> Div {
        div()
            .w(px(self.columns.map_or(56., |columns| columns.duration)))
            .flex_none()
            .text_right()
    }
}

impl MainView {
    pub(in crate::ui) fn object_hero_columns_height(&self) -> Pixels {
        if self
            .displayed_music_object()
            .and_then(|detail| detail.data.as_ref())
            .is_some_and(|data| {
                data.page.reference.kind != ObjectKind::Artist && !data.page.tracks.is_empty()
            })
        {
            px(HERO_COLUMNS_HEIGHT)
        } else {
            Pixels::ZERO
        }
    }

    pub(in crate::ui) fn object_hero_columns(&self, width: f32, cx: &Context<Self>) -> Option<Div> {
        let data = self.displayed_music_object()?.data.as_ref()?;
        if self.object_hero_columns_height() == Pixels::ZERO {
            return None;
        }
        let key = super::detail::playlist_key(data);
        let radio = data.page.tracks.iter().enumerate().any(|(index, track)| {
            can_resolve(&key, track)
                && self.can_start_track_radio(&object_entry(&key, index, track))
        });
        let table = TrackTable {
            columns: Some(TrackColumns::playlist(width, self.track_column_visibility)),
            row_height: None,
            artist_inline: false,
        };
        Some(
            div().child(
                div()
                    .id("music-object-columns")
                    .accessibility_id("music-object-columns")
                    .role(Role::Group)
                    .aria_label("Колонки треков")
                    .child(object_columns(
                        table,
                        false,
                        true,
                        radio,
                        Ink::new(true, self.fluid_high_contrast(cx)),
                    )),
            ),
        )
    }

    pub(super) fn object_tracks(
        &self,
        detail: &Detail,
        page: &MusicPage,
        key: &PlaylistKey,
        (compact, width): (bool, f32),
        pinned_header: Option<Pixels>,
        cx: &Context<Self>,
    ) -> Option<Div> {
        if page.tracks.is_empty() || page.reference.kind == ObjectKind::Track {
            return None;
        }
        let artist = page.reference.kind == ObjectKind::Artist;
        let pinned_columns = pinned_header.is_some();
        let release_selection = self.selected_release_index();
        let controls = self.glass_controls(cx);
        let glass = controls.is_some();
        let artist_presentation = artist && glass;
        let columns = glass.then(|| TrackColumns::playlist(width, self.track_column_visibility));
        let table = TrackTable {
            columns,
            row_height: glass.then(|| self.track_density.row_height()),
            artist_inline: compact
                && columns.and_then(|columns| columns.artist).is_none()
                && (!glass || self.track_column_visibility.shows(TrackColumn::Artist)),
        };
        let high_contrast = self.fluid_high_contrast(cx);
        let ink = Ink::new(glass, high_contrast);
        let row_style = track_row::Style::new(cx, high_contrast);
        let truncated = artist && page.tracks.len() > ARTIST_PREVIEW_TRACKS;
        let visible = if truncated && !detail.show_all_tracks {
            ARTIST_PREVIEW_TRACKS
        } else {
            page.tracks.len()
        };
        let rows = page
            .tracks
            .iter()
            .take(visible)
            .enumerate()
            .map(|(index, track)| {
                let playable = can_resolve(key, track);
                let radio = playable
                    && page.reference.kind != ObjectKind::Track
                    && self.can_start_track_radio(&object_entry(key, index, track));
                (index, track, playable, radio)
            })
            .collect::<Vec<_>>();
        let radio_column = rows.iter().any(|&(_, _, _, radio)| radio);
        let animate_artist_mark =
            !high_contrast && crate::ui::fluid_glass::transition_motion_configured();
        let player = self.player.read(cx);
        let toggle = truncated.then(|| {
            let label = if detail.show_all_tracks {
                "Свернуть треки"
            } else if artist_presentation {
                "Все треки"
            } else {
                "Показать все треки"
            };
            button("music-artist-show-tracks", label, 90)
                .gap(px(7.))
                .h(px(30.))
                .px(px(6.))
                .rounded(px(6.))
                .text_size(px(13.))
                .when(!glass, |toggle| {
                    toggle.hover(|style| style.bg(rgb(ELEVATED)))
                })
                .when_some(controls, |toggle, material| {
                    material.paint(toggle, Tone::Quiet)
                })
                .child(label)
                .child(
                    Icon::new(if detail.show_all_tracks {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    })
                    .pixel_size(px(14.))
                    .color(secondary()),
                )
                .on_toggle(
                    detail.show_all_tracks,
                    cx.listener(|view, _, _, cx| {
                        if let Some(detail) = view.music.object.as_mut() {
                            detail.show_all_tracks = !detail.show_all_tracks;
                            cx.notify();
                        }
                    }),
                )
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(if glass && compact { 4. } else { 9. }))
                .when(artist_presentation, |list| {
                    list.w_full().max_w(px(ARTIST_TRACK_LIST_MAX_WIDTH))
                })
                .when(!pinned_columns || artist, |list| {
                    list.child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(9.))
                            .child(
                                heading(
                                    "music-object-tracks",
                                    if artist_presentation || page.artist_details.is_some() {
                                        "Популярное"
                                    } else if artist {
                                        "Треки артиста"
                                    } else {
                                        "Треки"
                                    },
                                )
                                .text_size(px(
                                    if artist_presentation && !compact {
                                        24.
                                    } else {
                                        22.
                                    },
                                )),
                            )
                            .children(toggle),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .overflow_hidden()
                        .border_color(ink.rule)
                        .when(artist_presentation && !compact, |list| list.gap(px(3.)))
                        .when(!glass, |list| list.rounded(px(10.)).border_1())
                        .when(
                            !pinned_columns && !artist_presentation && !(glass && compact),
                            |list| {
                                list.child(object_columns(table, artist, glass, radio_column, ink))
                            },
                        )
                        .children(rows.into_iter().map(|(index, track, playable, radio)| {
                            let active = glass && player.recording_status(key, track).is_some();
                            let selected = release_selection == Some(index);
                            let row = div()
                                .flex()
                                .items_center()
                                .relative()
                                .when(!artist_presentation && (!glass || high_contrast), |row| {
                                    row.border_t_1().border_color(ink.rule)
                                })
                                .child(div().flex_1().min_w_0().child(self.object_track_row(
                                    TrackRow {
                                        index,
                                        key,
                                        track,
                                        compact,
                                        artwork: artist,
                                        artist_presentation,
                                        animate_artist_mark,
                                        playable,
                                        active,
                                    },
                                    row_style,
                                    table,
                                    cx,
                                )))
                                .when(radio_column, |row| {
                                    row.child(div().w(px(TRACK_MENU_WIDTH)).flex_none().children(
                                        radio.then(|| {
                                            self.playlist_menu_trigger(
                                                format!("music-track-menu-{index}"),
                                                format!("Действия трека «{}»", track.title),
                                                IconName::Ellipsis,
                                                101 + (index as isize) * 2,
                                                MenuKind::ObjectTrack(index),
                                                cx,
                                            )
                                        }),
                                    ))
                                });
                            let row = row_style.surface(row, active || selected);
                            if selected {
                                self.release_selected_row(
                                    row,
                                    key,
                                    track,
                                    index,
                                    pinned_header.unwrap_or(Pixels::ZERO),
                                    cx,
                                )
                                .into_any_element()
                            } else if artist_presentation {
                                artist_track::RevealFocusedRow {
                                    index,
                                    row,
                                    scroll: self.scroll.clone(),
                                }
                                .into_any_element()
                            } else {
                                row.into_any_element()
                            }
                        })),
                ),
        )
    }

    fn object_track_row(
        &self,
        row: TrackRow<'_>,
        row_style: track_row::Style,
        table: TrackTable,
        cx: &Context<Self>,
    ) -> AnyElement {
        let TrackRow {
            index,
            key,
            track,
            compact,
            artwork,
            artist_presentation,
            playable,
            active,
            ..
        } = row;
        let duration = track
            .duration
            .map(|seconds| time_label(f64::from(seconds)))
            .unwrap_or_else(|| "—".into());
        let name = format!("{} · {} · {duration}", track.title, track.artist);
        let controls = self.glass_controls(cx);
        let high_contrast = self.fluid_high_contrast(cx);
        let mut ink = Ink::new(controls.is_some(), high_contrast);
        if active {
            ink.primary = row_style.title(true);
        }
        let status = &self.player_snapshot.status;
        let order = 100 + index as isize * 2;
        let title_id = format!("music-track-title-{index}");
        let artist_id = format!("music-track-artist-{index}");
        let play = move |view: &mut Self,
                         origin: &Option<Point<Pixels>>,
                         window: &mut Window,
                         cx: &mut Context<Self>| {
            view.play_from_fluid_row(*origin, window, cx, |view, _, cx| {
                view.play_object(Some(index), PlaybackAction::Play, cx)
            });
        };
        let cover = if controls.is_some() {
            let edge = if artist_presentation {
                if compact { 32. } else { 42. }
            } else {
                TABLE_COVER
            };
            Some(
                fluid_cover(
                    track.artwork.as_deref(),
                    Seed::track(track),
                    edge,
                    if artist_presentation { 4. } else { 5. },
                )
                .when(playable, |cover| {
                    cover.child(
                        row_style
                            .cover_play_button(
                                format!("music-track-cover-play-{index}"),
                                format!("Слушать · {}", track.title),
                                order,
                                edge,
                            )
                            .on_activate_at(cx.listener(move |view, origin, window, cx| {
                                cx.stop_propagation();
                                play(view, origin, window, cx);
                            })),
                    )
                }),
            )
        } else {
            artwork.then(|| cover(track.artwork.as_deref(), TABLE_COVER, 5., false))
        };
        let body = if artist_presentation {
            artist_track::body(
                &row,
                duration,
                ink,
                status,
                (row_style, &self.player),
                cover,
                cx,
            )
        } else {
            track_columns()
                .h(px(table.row_height.unwrap_or(if artwork || compact {
                    39.
                } else {
                    43.
                })))
                .child(
                    div()
                        .w(px(28.))
                        .flex_none()
                        .text_color(ink.secondary)
                        .text_size(px(12.))
                        .when(!active, |cell| cell.child((index + 1).to_string()))
                        .when(active, |cell| cell.child(row_style.playback_marker(status))),
                )
                .children(cover)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(entity_link::track(
                            div()
                                .truncate()
                                .text_size(px(if controls.is_some() && !compact {
                                    text::TRACK_TITLE
                                } else {
                                    14.
                                }))
                                .font_weight(FontWeight::MEDIUM)
                                .when(controls.is_some(), |title| {
                                    title.text_color(if playable || active {
                                        ink.primary
                                    } else {
                                        ink.secondary
                                    })
                                }),
                            &title_id,
                            key,
                            track,
                            order,
                            cx,
                        ))
                        .when(table.artist_inline && !track.artist.is_empty(), |text| {
                            text.child(entity_link::artist_names(
                                div().text_color(ink.secondary).text_size(px(12.)),
                                &artist_id,
                                key,
                                track,
                                order,
                                cx,
                            ))
                        }),
                )
                .children(table.artist_column().map(|column| {
                    entity_link::artist_names(
                        column.text_color(ink.secondary).text_size(px(13.)),
                        &artist_id,
                        key,
                        track,
                        order,
                        cx,
                    )
                }))
                .child(
                    table
                        .time_column()
                        .text_color(ink.secondary)
                        .text_size(px(13.))
                        .child(duration),
                )
        };
        if playable {
            button(("music-track", index), name, 100 + (index as isize) * 2)
                .p_0()
                .w_full()
                .when(artist_presentation, |row| row.rounded(px(7.)))
                .when(!artist_presentation, |row| row.rounded_none())
                .when(controls.is_none(), |row| {
                    row.hover(|style| style.bg(rgb(ELEVATED)))
                        .active(|style| style.bg(rgb(ACTIVE)))
                })
                .when(controls.is_some(), |row| {
                    row_style.interaction(row).group(track_row::HOVER_GROUP)
                })
                .when(active, |row| {
                    row.aria_description(track_row::current_description(status))
                })
                .child(body)
                .on_activate_at(cx.listener(play))
                .into_any_element()
        } else {
            div()
                .id(("music-track", index))
                .accessibility_id(format!("music-track-{index}"))
                .role(Role::ListItem)
                .aria_label(name)
                .aria_description(if active {
                    track_row::current_description(status)
                } else {
                    "Аудио недоступно"
                })
                .child(body)
                .into_any_element()
        }
    }

    pub(super) fn object_releases(
        &self,
        detail: &Detail,
        data: &ScopedPage,
        compact: bool,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let page = &data.page;
        let provider = page.reference.provider;
        let controls = self.glass_controls(cx);
        let short_release = controls.is_some()
            && page.reference.kind == ObjectKind::Album
            && detail_style::ReleasePresentation::for_page(Some(page))
                != detail_style::ReleasePresentation::Album;
        let discography = self.music.artist_discography.as_ref().filter(|artist| {
            short_release
                && artist.profile == data.profile
                && artist.page.reference.provider == provider
                && artist.page.releases.iter().any(|release| {
                    ObjectRef::from_item(provider, release).as_ref() == Some(&page.reference)
                })
        });
        let (items, linked_releases) = if page.releases.is_empty() {
            (discography?.page.releases.as_slice(), true)
        } else {
            (page.releases.as_slice(), false)
        };
        if !items.iter().any(|item| {
            ObjectRef::from_item(provider, item).is_some_and(|object| {
                !linked_releases || object.kind == ObjectKind::Album && object != page.reference
            })
        }) {
            return None;
        }
        let edge = if compact { 112. } else { 144. };
        let title = if linked_releases {
            "Другие релизы артиста"
        } else if page.reference.kind == ObjectKind::Collection {
            "В подборке"
        } else {
            "Релизы"
        };
        let types = page
            .artist_details
            .as_ref()
            .map(|data| &data.release_is_single);
        let can_filter = types.is_some_and(|types| {
            types.values().any(|&single| single) && types.values().any(|&single| !single)
        });
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(11.))
                .pt(px(5.))
                .child(heading("music-object-releases", title).text_size(px(22.)))
                .when(can_filter, |releases| {
                    releases.child(
                        div().flex().gap(px(6.)).children(
                            [
                                (None, "Все"),
                                (Some(true), "Синглы"),
                                (Some(false), "Альбомы"),
                            ]
                            .into_iter()
                            .enumerate()
                            .map(|(index, (filter, label))| {
                                let selected = detail.singles_filter == filter;
                                button(("music-release-filter", index), label, 375 + index as isize)
                                    .h(px(29.))
                                    .px(px(11.))
                                    .rounded_full()
                                    .bg(rgb(if detail.singles_filter == filter {
                                        ACTIVE
                                    } else {
                                        SURFACE
                                    }))
                                    .text_size(px(12.))
                                    .aria_description(if detail.singles_filter == filter {
                                        "Выбран"
                                    } else {
                                        ""
                                    })
                                    .when(controls.is_none(), |tab| {
                                        tab.hover(|style| style.bg(rgb(ELEVATED)))
                                    })
                                    .when_some(controls, |tab, material| {
                                        paint_section_tab(tab, material, selected)
                                            .aria_toggled(if selected {
                                                Toggled::True
                                            } else {
                                                Toggled::False
                                            })
                                    })
                                    .child(label)
                                    .on_activate(cx.listener(move |view, _, _, cx| {
                                        if let Some(detail) = view.music.object.as_mut() {
                                            detail.singles_filter = filter;
                                            cx.notify();
                                        }
                                    }))
                            }),
                        ),
                    )
                })
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(if compact { 9. } else { 12. }))
                        .children(
                            items
                                .iter()
                                .enumerate()
                                .filter_map(|(index, item)| {
                                    if let Some(filter) = detail.singles_filter
                                        && types.and_then(|types| types.get(&item.id))
                                            != Some(&filter)
                                    {
                                        return None;
                                    }
                                    let object = ObjectRef::from_item(provider, item)?;
                                    if linked_releases
                                        && (object.kind != ObjectKind::Album
                                            || object == page.reference)
                                    {
                                        return None;
                                    }
                                    let album_origin =
                                        (controls.is_some() && item.artwork.is_some()).then(|| {
                                            super::detail::hero_motion::cover_span((
                                                "release", &object,
                                            ))
                                        });
                                    let item = item.clone();
                                    let profile = data.profile.clone();
                                    let intent = Destination::Object(MusicTarget {
                                        object: object.clone(),
                                        profile: profile.clone(),
                                        title: item.title.clone(),
                                        artwork: item.artwork.clone(),
                                    });
                                    let visibility =
                                        self.prefetch_visibility_probe([intent.clone()]);
                                    let order = 400 + index as isize;
                                    let radius = if object.kind == ObjectKind::Artist {
                                        edge / 2.
                                    } else {
                                        8.
                                    };
                                    let tile =
                                        button(("music-release", index), item.title.clone(), order)
                                        .relative()
                                        .w(px(edge + 16.))
                                        .flex_none()
                                        .flex_col()
                                        .items_start()
                                        .gap(px(5.))
                                        .p(px(7.))
                                        .rounded(px(12.))
                                        .when(controls.is_none(), |tile| {
                                            tile.hover(|style| style.bg(rgb(ELEVATED)))
                                                .active(|style| style.bg(rgb(ACTIVE)))
                                        })
                                        .when_some(controls, |tile, material| {
                                            material.paint(tile, Tone::Quiet)
                                        })
                                        .child(if controls.is_some() {
                                            let artwork = fluid_cover(
                                                item.artwork.as_deref(),
                                                Some(Seed::object(&object)),
                                                edge,
                                                8.,
                                            );
                                            match album_origin {
                                                Some(id) => super::detail::hero_motion::shared_artwork(
                                                    artwork, id, self.music_hero_motion(),
                                                ),
                                                None => artwork.into_any_element(),
                                            }
                                        } else {
                                            cover(item.artwork.as_deref(), edge, radius, true)
                                                .into_any_element()
                                        })
                                        .child(card_title(item.title.clone()))
                                        .child(
                                            div()
                                                .w_full()
                                                .truncate()
                                                .text_size(px(12.))
                                                .text_color(secondary())
                                                .child(item.subtitle.clone()),
                                        )
                                        .children(visibility)
                                        .on_activate(
                                            cx.listener(move |view, _, window, cx| {
                                                let open = |view: &mut Self, window: &gpui::Window, cx: &mut Context<Self>| {
                                                    view.open_music_item(
                                                        provider, profile.as_deref(), item.clone(), window, cx,
                                                    );
                                                };
                                                if let Some(id) = album_origin {
                                                    view.navigate_from_artwork(id, window, cx, open);
                                                } else {
                                                    open(view, window, cx);
                                                }
                                            }),
                                        );
                                    Some(entity_link::hint(
                                        tile,
                                        &format!("music-release-{index}"),
                                        order,
                                        Some(intent),
                                        cx,
                                    ))
                                })
                                .take(if linked_releases { 8 } else { usize::MAX }),
                        ),
                ),
        )
    }

    pub(super) fn related_artists(
        &self,
        data: &ScopedPage,
        compact: bool,
        feature: bool,
        cx: &Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        let source = data.page.reference.provider;
        let related = ArtistDetails::related_preview(
            &data.page.reference,
            data.page.artist_details.as_ref()?.related.iter().cloned(),
        );
        if related.is_empty() {
            return None;
        }
        let controls = self.glass_controls(cx);
        Some(
            div()
                .id("music-related-artists")
                .accessibility_id("music-related-artists")
                .role(Role::Group)
                .aria_label("Похожие исполнители")
                .flex()
                .flex_col()
                .flex_none()
                .gap(px(6.))
                .p(px(12.))
                .rounded(px(12.))
                .when(compact, |panel| panel.w_full())
                .when(!compact, |panel| panel.w(px(238.)))
                .bg(rgb(SURFACE))
                .border_1()
                .border_color(surface_border())
                .when(controls.is_some() && compact, |panel| {
                    panel
                        .p_0()
                        .rounded_none()
                        .bg(gpui::transparent_black())
                        .border_0()
                        .border_t_1()
                        .pt(px(12.))
                })
                .when(feature, |panel| {
                    panel
                        .w(px(252.))
                        .p_0()
                        .pl(px(20.))
                        .rounded_none()
                        .bg(gpui::transparent_black())
                        .border_0()
                        .border_l_1()
                })
                .child(heading("music-related-title", "Похожие исполнители").text_size(px(16.)))
                .children(related.into_iter().enumerate().filter_map(|(index, item)| {
                    let target = MusicTarget {
                        object: ObjectRef::from_item(source, &item)?,
                        profile: data.profile.clone(),
                        title: item.title.clone(),
                        artwork: item.artwork.clone(),
                    };
                    let intent = Destination::Object(target.clone());
                    let visibility = self.prefetch_visibility_probe([intent.clone()]);
                    let order = 350 + index as isize;
                    let id = super::detail::hero_motion::cover_span((
                        "related-artist",
                        &data.page.reference,
                        index,
                    ));
                    let artwork = if controls.is_some() {
                        fluid_cover(
                            item.artwork.as_deref(),
                            Some(Seed::object(&target.object)),
                            40.,
                            20.,
                        )
                    } else {
                        cover(item.artwork.as_deref(), 31., 15.5, false)
                    };
                    let row = button(("music-related-artist", index), item.title.clone(), order)
                        .relative()
                        .gap(px(8.))
                        .px(px(5.))
                        .h(px(if controls.is_some() { 48. } else { 41. }))
                        .rounded(px(7.))
                        .when(controls.is_none(), |row| {
                            row.hover(|style| style.bg(rgb(ELEVATED)))
                        })
                        .when_some(controls, |row, material| material.paint(row, Tone::Quiet))
                        .child(if controls.is_some() && item.artwork.is_some() {
                            super::detail::hero_motion::shared_artwork(
                                artwork,
                                id,
                                self.music_hero_motion(),
                            )
                        } else {
                            artwork.into_any_element()
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(px(13.))
                                .child(item.title.clone()),
                        )
                        .child(
                            Icon::new(IconName::ChevronRight)
                                .pixel_size(px(14.))
                                .color(secondary()),
                        )
                        .children(visibility)
                        .on_activate(cx.listener(move |view, _, window, cx| {
                            let open =
                                |view: &mut Self, window: &gpui::Window, cx: &mut Context<Self>| {
                                    view.open_music_target(target.clone(), window, cx);
                                };
                            if item.artwork.is_some() {
                                view.navigate_from_artwork(id, window, cx, open);
                            } else {
                                open(view, window, cx);
                            }
                        }));
                    Some(entity_link::hint(
                        row,
                        &format!("music-related-artist-{index}"),
                        order,
                        Some(intent),
                        cx,
                    ))
                })),
        )
    }

    pub(super) fn artist_biography(
        &self,
        page: &MusicPage,
        cx: &Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        let biography = page
            .artist_details
            .as_ref()?
            .biography
            .as_ref()
            .filter(|biography| !biography.trim().is_empty())?;
        let ink = Ink::new(self.uses_fluid_glass(), self.fluid_high_contrast(cx));
        Some(
            div()
                .id("music-artist-biography")
                .accessibility_id("music-artist-biography")
                .role(Role::Group)
                .aria_label("Об исполнителе")
                .flex()
                .flex_none()
                .flex_col()
                .gap(px(8.))
                .max_w(px(760.))
                .child(heading("music-artist-about-title", "Об исполнителе").text_size(px(16.)))
                .child(
                    div()
                        .id("music-artist-biography-text")
                        .accessibility_id("music-artist-biography-text")
                        .role(Role::Label)
                        .aria_label(biography.clone())
                        .text_size(px(14.))
                        .line_height(px(22.))
                        .text_color(ink.secondary)
                        .child(biography.clone()),
                ),
        )
    }
}

const TRACK_COLUMN_PADDING: f32 = 13.;

fn object_columns(table: TrackTable, artist: bool, glass: bool, radio: bool, ink: Ink) -> Div {
    track_columns()
        .when(!glass, |header| header.bg(rgb(SURFACE)))
        .when(radio, |header| {
            header.pr(px(TRACK_MENU_WIDTH + TRACK_COLUMN_PADDING))
        })
        .h(px(HERO_COLUMNS_HEIGHT))
        .text_color(ink.secondary)
        .text_size(px(12.))
        .child(div().w(px(28.)).flex_none().child("#"))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .when(glass || artist, |title| {
                    title.pl(px(TABLE_COVER + TABLE_GAP))
                })
                .child("Название"),
        )
        .children(
            table
                .artist_column()
                .map(|column| column.child("Исполнитель")),
        )
        .child(table.time_column().child("Время"))
}

fn track_columns() -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(TABLE_GAP))
        .px(px(TRACK_COLUMN_PADDING))
}
