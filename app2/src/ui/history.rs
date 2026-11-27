//! The listening history page. Rows are read from the profile and replayed as
//! a one-entry queue; the journal is appended only when sound actually starts.

use std::ops::Range;

mod fluid;
mod jobs;

use crate::{
    playback::{QueueEntry, SoundDetails, SoundSource, can_resolve},
    playlist::{PlaylistKey, PlaylistTrack},
    providers::ProviderId,
    storage::{CollectionRecord, HISTORY_LIMIT, HistoryRecord, LocalTimestamp},
    ui::{
        MainView,
        assets::provider_mark,
        components::{
            Activate, button,
            music::{TRACK_ROW_HEIGHT, cover, fluid_cover},
            scroll_viewport::{Density, FluidScrollViewport},
            scrollbar::{ScrollChrome, ScrollSurface, scrollbar_element, with_list_height_hints},
            track_row,
        },
        fluid_glass::{Geometry, ShellGeometry, artwork::Seed, tokens},
        labels::readable_date_time,
        music::status,
        playlist::navigation::track_destination,
        theme::{ELEVATED, PRIMARY, SECONDARY, secondary, surface_border},
        workspace::{Page, PageState},
    },
};
use gpui::{
    AccessibleAction, AnyElement, Context, Div, ElementId, IntoElement, KeyDownEvent,
    ListAlignment, ListOffset, ListState, Role, Stateful, Task, Window, accesskit::ActionData, div,
    list, prelude::*, px, rgb,
};

#[cfg(feature = "capture")]
use crate::storage::Store;

const HEADER_ITEMS: usize = 1;
const FLUID_COVER: f32 = 33.;

pub(super) struct HistoryRow {
    id: i64,
    started: i64,
    pub(super) key: PlaylistKey,
    pub(super) track: PlaylistTrack,
    platform: Option<ProviderId>,
    source: SoundSource,
    details: SoundDetails,
    time: LocalTimestamp,
}

impl HistoryRow {
    fn from_record(record: HistoryRecord) -> Self {
        Self {
            id: record.id,
            started: record.started,
            key: record.key,
            track: record.track,
            platform: record.platform,
            source: record.source,
            details: record.details,
            time: record.time,
        }
    }

    fn date(&self) -> String {
        readable_date_time(
            self.time.year,
            self.time.month,
            self.time.day,
            self.time.hour,
            self.time.minute,
        )
    }

    fn source_label(&self) -> String {
        let mut label = self.source.label().to_owned();
        if let Some(quality) = &self.details.quality {
            label.push_str(&format!(" · {}", quality.label()));
        }
        label
    }

    fn accessible_name(&self) -> String {
        let platform = self.platform.map(ProviderId::display_name).unwrap_or("");
        [self.track.title.as_str(), &self.track.artist, platform]
            .into_iter()
            .filter(|part| !part.is_empty())
            .chain([self.source_label().as_str(), self.date().as_str()])
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

pub(super) struct HistoryUi {
    list: ListState,
    rows: Vec<HistoryRow>,
    loading: bool,
    error: Option<String>,
    task: Option<Task<()>>,
    loaded: bool,
    /// A restored position waits here until the rows have data.
    pending: Option<ListOffset>,
    pub(super) collections: Vec<CollectionRecord>,
    journal: jobs::Journal,
}

impl Default for HistoryUi {
    fn default() -> Self {
        Self {
            list: ListState::new(HEADER_ITEMS, ListAlignment::Top, px(220.)),
            rows: Vec::new(),
            loading: false,
            error: None,
            task: None,
            loaded: false,
            pending: None,
            collections: Vec::new(),
            journal: jobs::Journal::default(),
        }
    }
}

impl HistoryUi {
    pub(super) fn rows(&self) -> &[HistoryRow] {
        &self.rows
    }

    pub(super) fn needs_load(&self) -> bool {
        !self.loaded && !self.loading
    }

    pub(super) fn position(&self) -> ListOffset {
        self.list.logical_scroll_top()
    }

    pub(super) fn apply(&mut self, records: Vec<HistoryRecord>) {
        if self.loaded {
            for record in records.into_iter().rev() {
                let position = self.rows.binary_search_by(|row| {
                    (record.started, record.id).cmp(&(row.started, row.id))
                });
                if let Err(position) = position
                    && position < HISTORY_LIMIT
                {
                    self.rows.insert(position, HistoryRow::from_record(record));
                    let item = position + HEADER_ITEMS;
                    self.list.splice(item..item, 1);
                }
            }
            let previous = self.rows.len();
            if previous > HISTORY_LIMIT {
                self.rows.truncate(HISTORY_LIMIT);
                self.list
                    .splice(HISTORY_LIMIT + HEADER_ITEMS..previous + HEADER_ITEMS, 0);
            }
            self.error = None;
            return;
        }
        self.loaded = true;
        self.rows = records.into_iter().map(HistoryRow::from_record).collect();
        let previous = self.list.item_count();
        let count = self.rows.len() + HEADER_ITEMS;
        if previous != count {
            self.list.splice(HEADER_ITEMS..previous, self.rows.len());
            self.list
                .clone()
                .with_uniform_item_height(px(TRACK_ROW_HEIGHT));
        }
        self.error = None;
    }

    fn latest_id(&self) -> i64 {
        self.rows.iter().map(|row| row.id).max().unwrap_or_default()
    }

    fn settle(&mut self) {
        if let Some(offset) = self.pending.take() {
            self.list.scroll_to(offset);
        }
    }
}

impl MainView {
    pub(in crate::ui) fn open_history_route(&mut self, cx: &mut Context<Self>) {
        let offset = match self.workspace.routes.state() {
            PageState::History(offset) => Some(*offset),
            _ => None,
        };
        self.history.loading = false;
        self.history.error = None;
        self.history.pending =
            offset.filter(|offset| offset.item_ix > 0 || offset.offset_in_item > px(0.));
        self.load_history(cx);
    }

    pub(super) fn history_position(&self) -> ListOffset {
        self.history.position()
    }

    pub(super) fn history_loading(&self) -> bool {
        self.history.loading
    }

    pub(in crate::ui) fn history_view(
        &self,
        margin: f32,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let state = &self.history.list;
        let view = cx.weak_entity();
        let surface = ScrollSurface::VirtualList(state.clone());
        let chrome = ScrollChrome::new(self.uses_fluid_glass(), window);
        let shell = ShellGeometry::for_workspace(window, self.docked_panel_width(window));
        let bottom_padding = shell.scroll_content_padding();
        let current = self.uses_fluid_glass().then_some(()).and_then(|()| {
            let player = self.player.read(cx);
            self.history
                .rows()
                .iter()
                .find(|row| player.recording_status(&row.key, &row.track).is_some())
                .map(|row| row.id)
        });
        with_list_height_hints(
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_hidden()
                .child(FluidScrollViewport::new(
                    list(state.clone(), move |index, window, cx| {
                        view.update(cx, |view, cx| {
                            view.history_item(index, margin, current, window, cx)
                        })
                        .unwrap_or_else(|_| div().into_any_element())
                    })
                    .size_full()
                    .pb(px(bottom_padding)),
                    surface.clone(),
                    self.uses_fluid_glass(),
                    Density::Rows(px(TRACK_ROW_HEIGHT)),
                ))
                .child(self.prefetch_after_list_layout(
                    state.clone(),
                    HEADER_ITEMS,
                    self.history.rows.len(),
                    gpui::Pixels::ZERO,
                    Self::prefetch_history_window,
                    cx,
                ))
                .child(self.history_scrollbar(&surface, chrome, cx))
                .child(self.scrollbar_guide(surface, chrome, cx)),
            state,
            px(TRACK_ROW_HEIGHT),
        )
        .id("history-list")
        .accessibility_id("history-list")
        .role(Role::List)
        .aria_label("Прослушанные записи")
    }

    fn prefetch_history_window(
        &mut self,
        rows: Range<usize>,
        backwards: bool,
        scale: f32,
        cx: &mut Context<Self>,
    ) {
        let mut rows = self
            .history
            .rows
            .get(rows)
            .unwrap_or_default()
            .iter()
            .collect::<Vec<_>>();
        if backwards {
            rows.reverse();
        }
        let covers = rows
            .iter()
            .filter_map(|row| row.track.artwork.clone())
            .collect();
        let targets = rows
            .into_iter()
            .filter_map(|row| track_destination(&row.key, &row.track))
            .collect::<Vec<_>>();
        self.prefetch_list_window(covers, targets, scale, cx);
    }

    fn history_scrollbar(
        &self,
        surface: &ScrollSurface,
        chrome: ScrollChrome,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let set = cx.weak_entity();
        scrollbar_element("history-scrollbar", "Прокрутка истории", surface, chrome)
            .tab_index(48)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
                let surface = ScrollSurface::VirtualList(view.history.list.clone());
                let Some(target) =
                    surface.offset_for_key(&event.keystroke.key, px(TRACK_ROW_HEIGHT))
                else {
                    return;
                };
                if surface.set_offset(target) {
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .on_a11y_action(AccessibleAction::SetValue, move |data, _, cx| {
                if let Some(ActionData::NumericValue(value)) = data
                    && value.is_finite()
                {
                    let _ = set.update(cx, |view, cx| {
                        let surface = ScrollSurface::VirtualList(view.history.list.clone());
                        if surface.set_offset(px(*value as f32)) {
                            cx.notify();
                        }
                    });
                }
            })
    }

    fn history_item(
        &self,
        index: usize,
        margin: f32,
        current: Option<i64>,
        window: &gpui::Window,
        cx: &Context<Self>,
    ) -> AnyElement {
        if self.uses_fluid_glass() {
            let layout = Geometry::for_window(window);
            let width = layout.content_width(window.viewport_size().width.into())
                - self.docked_panel_width(window);
            let compact = layout.compact() || window.viewport_size().height < px(500.);
            return self.fluid_history_item(index, margin, compact, width, current, cx);
        }
        let item = if index == 0 {
            self.history_header().into_any_element()
        } else {
            self.history_row(index - HEADER_ITEMS, None, cx)
                .into_any_element()
        };
        div()
            .w_full()
            .px(px(margin))
            .child(
                div()
                    .max_w(px(1100.))
                    .when(index >= HEADER_ITEMS, |row| {
                        row.border_b_1().border_color(surface_border().alpha(0.55))
                    })
                    .child(item),
            )
            .into_any_element()
    }

    fn history_header(&self) -> Div {
        div()
            .w_full()
            .pt(px(12.))
            .pb(px(12.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(
                div()
                    .id("history-title")
                    .accessibility_id("history-title")
                    .role(Role::Heading)
                    .aria_label("История")
                    .text_size(px(32.))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("История"),
            )
            .children(
                self.history
                    .error
                    .as_ref()
                    .map(|error| status("history-error", error.clone())),
            )
            .when(
                self.history.rows.is_empty()
                    && !self.history.loading
                    && self.history.error.is_none(),
                |page| {
                    page.child(status(
                        "history-empty",
                        "Здесь появятся прослушанные записи.",
                    ))
                },
            )
    }

    fn history_row(
        &self,
        index: usize,
        glass: Option<fluid::RowStyle>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        use crate::ui::{
            components::entity_link,
            music::detail::hero_motion::{cover_span, shared_artwork},
        };
        let row = &self.history.rows[index];
        let cover_plays = glass.is_some() && can_resolve(&row.key, &row.track);
        let mut name = row.accessible_name();
        if cover_plays {
            name.insert_str(0, "Слушать снова · ");
        }
        let track = row.track.clone();
        let key = row.key.clone();
        let platform = row.platform;
        let high_contrast = self.fluid_high_contrast(cx);
        let row_style = track_row::Style::new(cx, high_contrast);
        let accessible_id = format!("history-item-{}", row.id);
        let title_id = format!("history-track-{}", row.id);
        let artist_id = format!("history-artist-{}", row.id);
        let artwork_id = format!("history-artwork-{}", row.id);
        let order = 100 + index as isize;
        let transition_span = glass.map(|_| cover_span(("history-track", row.id)));
        let artwork = if let Some(span) = transition_span {
            let artwork = fluid_cover(
                track.artwork.as_deref(),
                Seed::track(&track),
                FLUID_COVER,
                4.,
            )
            .when(glass.is_some_and(|glass| glass.current), |artwork| {
                artwork.child(row_style.cover_marker(&self.player_snapshot.status))
            })
            .when(cover_plays, |artwork| {
                let key = key.clone();
                let track = track.clone();
                artwork.child(
                    row_style
                        .cover_play_button(
                            format!("history-cover-play-{}", row.id),
                            format!("Слушать снова · {}", track.title),
                            order,
                            FLUID_COVER,
                        )
                        .on_activate(cx.listener(move |view, _, _, cx| {
                            cx.stop_propagation();
                            view.play_history_entry(key.clone(), track.clone(), cx);
                        })),
                )
            });
            shared_artwork(artwork, span, self.music_hero_motion())
        } else {
            cover(track.artwork.as_deref(), 30., 4., false).into_any_element()
        };
        let target = track_destination(&key, &track);
        let visibility = self.prefetch_visibility_probe(target.iter().cloned());
        let artwork = if let Some(target) = target.filter(|_| !cover_plays) {
            entity_link::artwork(
                artwork,
                &artwork_id,
                &track.title,
                target,
                transition_span,
                order,
                cx,
            )
        } else {
            artwork
        };
        let element_id = if glass.is_some() {
            ElementId::from(accessible_id.clone())
        } else {
            ElementId::from(("history-item", row.id as usize))
        };
        let control = button(element_id, name, 100 + index as isize)
            .accessibility_id(accessible_id)
            .relative()
            .w_full()
            .h(px(TRACK_ROW_HEIGHT))
            .gap(px(10.))
            .px(px(9.))
            .rounded(px(6.));
        let control = if let Some(glass) = glass {
            row_style
                .surface(row_style.interaction(control), glass.current)
                .when(cover_plays, |row| row.group(track_row::HOVER_GROUP))
                .when(glass.current, |row| {
                    row.aria_description(track_row::current_description(
                        &self.player_snapshot.status,
                    ))
                })
        } else {
            control
                .role(Role::ListItem)
                .hover(|row| row.bg(rgb(ELEVATED)))
        };
        let secondary = if glass.is_some() {
            fluid::ink_secondary(high_contrast)
        } else {
            secondary()
        };
        control
            .children(visibility)
            .child(artwork)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(crate::ui::components::entity_link::track(
                        div()
                            .truncate()
                            .text_color(glass.map_or_else(
                                || rgb(PRIMARY).into(),
                                |glass| row_style.title(glass.current),
                            ))
                            .when(glass.is_some(), |title| {
                                title
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_size(px(tokens::text::BODY))
                            }),
                        &title_id,
                        &key,
                        &track,
                        100 + index as isize,
                        cx,
                    ))
                    .when(!track.artist.trim().is_empty(), |text| {
                        text.child(crate::ui::components::entity_link::artist_names(
                            div().text_size(px(12.)).text_color(secondary),
                            &artist_id,
                            &key,
                            &track,
                            100 + index as isize,
                            cx,
                        ))
                    }),
            )
            .children(platform.map(|provider| provider_mark(provider, 19.)))
            .when(glass.is_none_or(|glass| glass.show_source), |row_control| {
                row_control.child(
                    div()
                        .flex_none()
                        .when(glass.is_some(), |source| {
                            source.w(px(160.)).truncate().text_right()
                        })
                        .text_size(px(12.))
                        .text_color(if glass.is_some() {
                            secondary
                        } else {
                            rgb(SECONDARY).into()
                        })
                        .child(row.source_label()),
                )
            })
            .child(
                div()
                    .flex_none()
                    .when(glass.is_some(), |date| date.w(px(50.)).text_right())
                    .text_size(px(12.))
                    .text_color(if glass.is_some() {
                        secondary
                    } else {
                        rgb(SECONDARY).into()
                    })
                    .child(if glass.is_some() {
                        format!("{:02}:{:02}", row.time.hour, row.time.minute)
                    } else {
                        row.date()
                    }),
            )
            .on_activate(cx.listener(move |view, _, _, cx| {
                view.play_history_entry(key.clone(), track.clone(), cx)
            }))
    }

    pub(super) fn play_history_entry(
        &mut self,
        key: PlaylistKey,
        track: PlaylistTrack,
        cx: &mut Context<Self>,
    ) {
        let entry = QueueEntry {
            playlist: key,
            playlist_revision: None,
            recommendations: Vec::new(),
            index: 0,
            track,
        };
        self.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| playback.play(vec![entry], 0))
        });
    }

    #[cfg(feature = "capture")]
    pub fn open_history_for_capture(&mut self, window: &gpui::Window, cx: &mut Context<Self>) {
        self.navigate(crate::ui::workspace::Route::History, window, cx);
    }

    #[cfg(feature = "capture")]
    pub fn history_ready_for_capture(&self) -> bool {
        !self.history.loading
    }

    #[cfg(feature = "capture")]
    pub fn history_cover_bounds_for_capture(
        &self,
        id: i64,
        window: &Window,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        use crate::ui::music::detail::hero_motion::cover_span;

        window.rendered_span(cover_span(("history-track", id)))
    }

    #[cfg(feature = "capture")]
    pub fn history_rows_for_capture(&self) -> Vec<(String, String, String, String)> {
        self.history
            .rows
            .iter()
            .map(|row| {
                (
                    row.track.title.clone(),
                    row.track.artist.clone(),
                    row.platform
                        .map(|provider| provider.display_name().to_owned())
                        .unwrap_or_default(),
                    row.source_label(),
                )
            })
            .collect()
    }
}

#[cfg(feature = "capture")]
impl MainView {
    /// Seed the listening history of the isolated capture profile.
    pub fn seed_history_for_capture(&mut self, local_playlist: i64) -> gpui::Result<()> {
        use crate::playlist::Media;

        let store = Store::open_default()?;
        store.use_guest(|| Some(()))?;
        let local_media = store
            .local_playlist(local_playlist)?
            .and_then(|playlist| playlist.tracks.into_iter().next())
            .and_then(|track| track.media)
            .ok_or_else(|| {
                std::io::Error::other("Не найден локальный файл для проверки истории.")
            })?;
        for (index, title) in ["Первый", "Второй", "Третий"].into_iter().enumerate()
        {
            let track = PlaylistTrack {
                id: format!("local-{index}"),
                title: title.to_owned(),
                artist: "Mediadeka".to_owned(),
                artists: Vec::new(),
                artist_credits: Vec::new(),
                album_ref: None,
                album: Some("Сценарий панели".to_owned()),
                artwork: None,
                added: Some("2026-11-24".to_owned()),
                duration: None,
                media: Some(local_media.clone()),
            };
            store.record_history(
                &PlaylistKey::Local(local_playlist),
                &track,
                SoundSource::LocalFile,
            )?;
        }
        let track = PlaylistTrack {
            id: "spotify-track".into(),
            title: "Поток".into(),
            artist: "Площадка".into(),
            artists: vec!["Площадка".into()],
            artist_credits: Vec::new(),
            album_ref: None,
            album: None,
            artwork: None,
            added: None,
            duration: Some(200),
            media: Some(Media::Provider {
                provider: ProviderId::Spotify,
                id: "spotify-track".into(),
            }),
        };
        store.record_history(
            &PlaylistKey::Source {
                provider: ProviderId::Spotify,
                profile: Some("capture".into()),
                id: "playlist".into(),
            },
            &track,
            SoundSource::Stream(ProviderId::Spotify),
        )?;
        Ok(())
    }
}
