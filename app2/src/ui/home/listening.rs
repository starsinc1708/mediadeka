use std::sync::Arc;

use gpui::{
    AnyElement, Context, Div, ElementId, FontWeight, Role, SharedString, Stateful, Window, div,
    prelude::*, px, relative, rgb,
};

use crate::{
    music::library::LibraryGroup,
    playback::{CollectionKey, can_resolve},
    playlist::{Media, PlaylistKey},
    providers::ProviderId,
    storage::{CollectionRecord, Store},
    ui::{
        assets::provider_mark,
        components::{
            entity_link,
            glass_control::Tone,
            music::{cover, fluid_cover, time_label},
            track_row,
        },
        fluid_glass::{artwork::Seed, tokens},
        playlist::navigation::{self, Destination},
        theme::{
            HOME_ITEM_HOVER, HOME_ROW_BACKGROUND, navigation_accent, secondary, surface_border,
        },
        workspace::Target,
    },
};

use super::{Activate, FeedSource, Layout, MainView, button, content::section_title, fluid};

pub(super) struct Listening {
    pub spotlight: Option<Stateful<Div>>,
    pub continuation: Option<Stateful<Div>>,
    pub sections: Vec<Stateful<Div>>,
}

impl MainView {
    pub(super) fn home_listening(
        &self,
        visible: &[&FeedSource],
        layout: Layout,
        cx: &Context<Self>,
    ) -> Listening {
        let local = self.home.filter.is_none();
        let in_scope = |key| source_visible(key, visible, local);
        let mut sections = Vec::new();
        let unfinished: Vec<_> = self
            .history
            .collections
            .iter()
            .filter(|record| record.progress.resumable() && in_scope(record.key.connected_source()))
            .take(layout.quick_columns)
            .collect();
        let mut additional = unfinished.iter().copied().enumerate().peekable();
        let featured_collection = layout.glass.and_then(|_| additional.next());
        let mut spotlight = featured_collection
            .map(|(index, record)| self.continue_tile(index, record, layout, true, cx));
        let continuation = (additional.peek().is_some()).then(|| {
            let stacked = layout.continuation_rail() && spotlight.is_some();
            let title = if stacked {
                "Другие подборки"
            } else {
                "Продолжить прослушивание"
            };
            section("home-continue", "Продолжить прослушивание")
                .child(section_title(title))
                .child(tiles().when(stacked, |rail| rail.flex_col()).children(
                    additional.map(|(index, record)| {
                        self.continue_tile(index, record, layout, false, cx)
                    }),
                ))
        });

        let mut seen = Vec::new();
        let mut recent = self
            .history
            .rows()
            .iter()
            .filter(|row| {
                if !in_scope(row.key.connected_source()) || !can_resolve(&row.key, &row.track) {
                    return false;
                }
                let identity = match &row.track.media {
                    Some(media @ Media::File(_)) => (None, Some(media), None),
                    Some(media) => (row.key.connected_source(), Some(media), None),
                    None => (
                        row.key.connected_source(),
                        None,
                        Some(row.track.id.as_str()),
                    ),
                };
                if seen.contains(&identity) {
                    return false;
                }
                seen.push(identity);
                true
            })
            .take(layout.quick_columns)
            .enumerate()
            .peekable();
        if layout.glass.is_some() && spotlight.is_none() {
            spotlight = recent
                .next()
                .map(|(index, row)| self.recent_track_tile(index, row, layout, true, cx));
        }
        let mut collections = self
            .history
            .collections
            .iter()
            .filter(|record| {
                in_scope(record.key.connected_source())
                    && !unfinished
                        .iter()
                        .any(|displayed| displayed.key == record.key)
            })
            .take(layout.quick_columns)
            .enumerate()
            .peekable();
        if recent.peek().is_some() || collections.peek().is_some() {
            sections.push(
                section("home-recent", "Недавно прослушанное")
                    .child(section_title("Недавно прослушанное"))
                    .child(
                        tiles()
                            .children(recent.map(|(index, row)| {
                                self.recent_track_tile(index, row, layout, false, cx)
                            }))
                            .children(collections.map(|(index, record)| {
                                let key = record.key.clone();
                                let id = format!("home-recent-collection-{index}");
                                let target = collection_destination(record);
                                let visibility =
                                    self.prefetch_visibility_probe(target.iter().cloned());
                                let open = target.clone();
                                let control = tile(
                                    id.clone(),
                                    format!("Открыть · {}", record.title),
                                    layout,
                                    collection_tile(record, layout),
                                )
                                .relative()
                                .children(visibility)
                                .on_activate(cx.listener(move |view, _, window, cx| {
                                    view.open_recent_collection(&key, open.clone(), window, cx);
                                }));
                                entity_link::hint(control, &id, 38, target, cx)
                            })),
                    ),
            );
        }
        Listening {
            spotlight,
            continuation,
            sections,
        }
    }

    fn recent_track_tile(
        &self,
        index: usize,
        row: &crate::ui::history::HistoryRow,
        layout: Layout,
        featured: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let key = row.key.clone();
        let track = row.track.clone();
        let current = layout
            .glass
            .and_then(|_| self.player.read(cx).recording_status(&key, &track));
        let visibility = self.prefetch_visibility_probe(
            navigation::track_target(&key, &track).map(Destination::Object),
        );
        let id = format!("home-recent-track-{index}");
        let label = format!("Слушать · {}", track.title);
        let content = Tile {
            title: &track.title,
            detail: &track.artist,
            artwork: track.artwork.as_deref(),
            fallback: Seed::track(&track),
            provider: key.provider(),
            label_elements: Some((
                entity_link::track(
                    div().truncate(),
                    &format!("{id}-title"),
                    &key,
                    &track,
                    38,
                    cx,
                ),
                entity_link::artist_names(div(), &format!("{id}-artist"), &key, &track, 38, cx)
                    .into_any_element(),
            )),
        };
        let control = if featured {
            fluid::spotlight(
                button(id, label, 38),
                layout,
                content,
                "Недавно слушали",
                "Слушать снова",
                None,
                false,
            )
        } else {
            tile(id, label, layout, content)
        };
        let row_style = track_row::Style::new(cx, self.fluid_high_contrast(cx));
        row_style
            .surface(control, current.is_some())
            .when_some(current, |tile, status| {
                tile.aria_description(track_row::current_description(status))
            })
            .relative()
            .children(visibility)
            .on_activate(cx.listener(move |view, _, _, cx| {
                view.play_history_entry(key.clone(), track.clone(), cx)
            }))
    }

    fn continue_tile(
        &self,
        index: usize,
        record: &CollectionRecord,
        layout: Layout,
        featured: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let fraction = record.progress.fraction().clamp(0., 1.);
        let progress = format!("{}%", (fraction * 100.).floor() as u32);
        let position = time_label(record.progress.position_ms as f64 / 1000.);
        let key = record.key.clone();
        let mut content = collection_tile(record, layout);
        let detail = if featured {
            let track = record.progress.index + 1;
            match &key {
                CollectionKey::LocalAlbum { artist, .. } if !artist.is_empty() => {
                    format!("{artist} · трек {track}, {position} · {progress}")
                }
                _ => format!("{} · трек {track}, {position} · {progress}", content.detail),
            }
        } else {
            format!("{} · {progress}", content.detail)
        };
        content.detail = &detail;
        let id = format!("home-continue-{index}");
        let label = format!("Продолжить · {} · {detail}", record.title);
        let target = collection_destination(record);
        let visibility = self.prefetch_visibility_probe(target.iter().cloned());
        let route_key = key.clone();
        let route_title = record.title.clone();
        let route = target.clone();
        let view = cx.weak_entity();
        let title_id = format!("{id}-title");
        content.label_elements = Some((
            entity_link::hint(
                entity_link::activate(
                    div().id(title_id.clone()).child(route_title.clone()),
                    format!("Открыть · {route_title}"),
                    38,
                    move |window, cx| {
                        let _ = view.update(cx, |view, cx| {
                            view.open_recent_collection(&route_key, route.clone(), window, cx);
                        });
                    },
                ),
                &title_id,
                38,
                target,
                cx,
            ),
            div().truncate().child(detail.clone()).into_any_element(),
        ));
        let color = if self.fluid_high_contrast(cx) {
            rgb(tokens::contrast::ACCENT).into()
        } else if layout.glass.is_some() {
            rgb(tokens::control::SLIDER_LEVEL).into()
        } else {
            navigation_accent()
        };
        let meter = div()
            .id(format!("home-continue-progress-{index}"))
            .accessibility_id(format!("home-continue-progress-{index}"))
            .role(Role::ProgressIndicator)
            .aria_label("Прогресс прослушивания")
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(100.)
            .aria_numeric_value(f64::from(fraction) * 100.)
            .when(!featured, |bar| {
                bar.absolute().bottom(px(5.)).left(px(52.)).right(px(9.))
            })
            .when(featured, |bar| bar.w_full().max_w(px(240.)))
            .h(px(3.))
            .rounded_full()
            .bg(surface_border())
            .child(
                div()
                    .h_full()
                    .w(relative(fraction))
                    .rounded_full()
                    .bg(color),
            );
        let control = if featured {
            fluid::spotlight(
                button(id, label, 38),
                layout,
                content,
                "Продолжить прослушивание",
                "Продолжить",
                Some(meter),
                self.home.resume.is_some(),
            )
        } else {
            tile(id, label, layout, content).relative().child(meter)
        };
        control
            .relative()
            .children(visibility)
            .when(self.home.resume.is_some(), |control| {
                control
                    .aria_description("Восстанавливаем позицию прослушивания")
                    .cursor_default()
            })
            .on_activate(cx.listener(move |view, _, _, cx| {
                view.resume_home_collection(key.clone(), cx);
            }))
    }

    fn open_recent_collection(
        &mut self,
        key: &CollectionKey,
        destination: Option<Destination>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(destination) = destination {
            self.follow_playlist_link(destination, window, cx);
        } else if let CollectionKey::LocalAlbum { title, artist } = key {
            self.open_library_group(
                LibraryGroup::Album {
                    title: title.clone(),
                    artist: artist.clone(),
                },
                window,
                cx,
            );
        }
    }

    fn resume_home_collection(&mut self, key: CollectionKey, cx: &mut Context<Self>) {
        if self.home.resume.is_some() {
            return;
        }
        let playing_before = self.player_snapshot.clone();
        self.home.resume = Some(cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { Store::open_default()?.collection_listening(key) })
                .await;
            let _ = view.update(cx, |view, cx| {
                view.home.resume = None;
                cx.notify();
                if view.player_snapshot != playing_before {
                    return;
                }
                match result {
                    Ok(Some((plan, progress))) => {
                        let Some((home, _)) = view.state.home_view() else {
                            return;
                        };
                        if plan
                            .key
                            .connected_source()
                            .is_some_and(|(provider, profile)| {
                                !FeedSource::available(home).iter().any(|source| {
                                    source.provider == provider
                                        && source.profile.as_deref() == profile
                                })
                            })
                        {
                            return;
                        }
                        match &plan.key {
                            CollectionKey::Playlist(key) => view.ensure_playlist_favorites(key, cx),
                            CollectionKey::LocalAlbum { .. } => {
                                if let Some(entry) = progress
                                    .resume_index(&plan)
                                    .and_then(|index| plan.entries.get(index))
                                {
                                    view.ensure_playlist_favorites(&entry.playlist, cx);
                                }
                            }
                        }
                        view.player.update(cx, |bar, cx| {
                            bar.command(cx, |playback| {
                                playback.resume_collection(Arc::new(plan), progress)
                            });
                        });
                    }
                    result => {
                        let detail = result
                            .err()
                            .map_or_else(String::new, |error| format!(" {error}"));
                        view.player.update(cx, |bar, cx| {
                            bar.show_notice(format!("Не удалось продолжить подборку.{detail}"), cx)
                        });
                    }
                }
            });
        }));
        cx.notify();
    }
}

fn source_visible(
    source: Option<(ProviderId, Option<&str>)>,
    sources: &[&FeedSource],
    local: bool,
) -> bool {
    source.map_or(local, |(provider, profile)| {
        sources
            .iter()
            .any(|source| source.provider == provider && source.profile.as_deref() == profile)
    })
}

fn collection_destination(record: &CollectionRecord) -> Option<Destination> {
    let CollectionKey::Playlist(key) = &record.key else {
        return None;
    };
    if let Some(Destination::Object(mut target)) = navigation::collection(key) {
        target.title.clone_from(&record.title);
        target.artwork.clone_from(&record.artwork);
        return Some(Destination::Object(target));
    }
    Some(Destination::Playlist(Target {
        key: key.clone(),
        name: record.title.clone(),
        artwork: record.artwork.clone(),
        link: None,
    }))
}

fn collection_tile(record: &CollectionRecord, layout: Layout) -> Tile<'_> {
    Tile {
        title: &record.title,
        detail: if record.key.is_album() {
            "Альбом"
        } else {
            "Плейлист"
        },
        artwork: record.artwork.as_deref(),
        fallback: layout.glass.and_then(|_| match &record.key {
            CollectionKey::Playlist(PlaylistKey::Local(id)) => Some(Seed::playlist(*id)),
            CollectionKey::LocalAlbum { title, artist } => Seed::entry(
                &crate::music::library::EntryAction::LocalGroup(LibraryGroup::Album {
                    title: title.clone(),
                    artist: artist.clone(),
                }),
            ),
            _ => None,
        }),
        provider: record.key.connected_source().map(|(provider, _)| provider),
        label_elements: None,
    }
}

pub(super) fn section(id: &'static str, title: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .accessibility_id(id)
        .role(Role::Group)
        .aria_label(title)
        .w_full()
        .flex()
        .flex_col()
        .gap(px(8.))
}

pub(super) fn tiles() -> Div {
    div().w_full().flex().flex_wrap().gap(px(9.))
}

pub(super) struct Tile<'a> {
    pub title: &'a str,
    pub detail: &'a str,
    pub artwork: Option<&'a str>,
    pub fallback: Option<Seed>,
    pub provider: Option<ProviderId>,
    pub label_elements: Option<(AnyElement, AnyElement)>,
}

impl Tile<'_> {
    pub(super) fn labels(&mut self) -> (AnyElement, AnyElement) {
        self.label_elements.take().unwrap_or_else(|| {
            (
                div()
                    .truncate()
                    .child(self.title.to_owned())
                    .into_any_element(),
                div()
                    .truncate()
                    .child(self.detail.to_owned())
                    .into_any_element(),
            )
        })
    }
}

pub(super) fn tile(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    layout: Layout,
    content: Tile<'_>,
) -> Stateful<Div> {
    let tile = tile_base(id, label, layout, content);
    if let Some(material) = layout.glass {
        material.paint(tile, Tone::Quiet)
    } else {
        tile.hover(|style| style.bg(rgb(HOME_ITEM_HOVER)))
    }
}

pub(super) fn tile_base(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    layout: Layout,
    mut content: Tile<'_>,
) -> Stateful<Div> {
    let (title_label, detail_label) = content.labels();
    button(id, label, 38)
        .w(px(layout.quick_width))
        .h(px(55.))
        .flex_none()
        .min_w_0()
        .gap(px(8.))
        .px(px(7.))
        .rounded(px(8.))
        .when(layout.glass.is_none(), |tile| {
            tile.bg(rgb(HOME_ROW_BACKGROUND))
        })
        .child(if layout.glass.is_some() {
            fluid_cover(content.artwork, content.fallback, 36., 5.)
        } else {
            cover(content.artwork, 36., 5., false)
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .when(layout.glass.is_some(), |title| {
                            title.text_color(rgb(tokens::PRIMARY))
                        })
                        .text_size(px(if layout.glass.is_some() {
                            tokens::text::BODY
                        } else {
                            13.
                        }))
                        .child(title_label),
                )
                .child(
                    div()
                        .truncate()
                        .text_size(px(11.))
                        .text_color(secondary())
                        .child(detail_label),
                ),
        )
        .children(
            content
                .provider
                .map(|provider| provider_mark(provider, 14.)),
        )
}
