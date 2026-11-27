//! The virtualized queue tab, with GPUI drag and keyboard reordering.

use std::{cell::RefCell, collections::HashMap, rc::Rc};

use gpui::{
    AnyElement, Context, Div, DragMoveEvent, FocusHandle, FontWeight, KeyDownEvent, ListState,
    Pixels, Role, Stateful, Window, div, list, prelude::*, px, rgb,
};
use guise::{Icon, IconName};

use super::{
    panel_icon_action,
    queue_motion::{Drag, HEADING_HEIGHT, QueueMotion, Row, queue_span, row_bounds, target_at},
};

use crate::{
    playback::QueueEntry,
    ui::{
        MainView,
        components::{
            Activate, button,
            content_state::empty_state,
            music::fluid_cover,
            scroll_viewport::{Density, FluidScrollViewport},
            scrollbar::ScrollSurface,
            track_row,
        },
        fluid_glass::{artwork::Seed, tokens as fluid},
        music::detail::hero_motion::painted_fragment,
        player::PlayerBar,
        playlist::artwork::track_cover,
        theme::{
            ACCENT, ELEVATED, PLAYER_PANEL_CURRENT, PLAYER_PANEL_HOVER, playlist,
            playlist::metrics, secondary,
        },
        workspace::Page,
    },
};

const QUEUE_ROW_HEIGHT: f32 = 31.;
const FLUID_QUEUE_ROW_HEIGHT: f32 = 48.;
const FLUID_QUEUE_COVER: f32 = 32.;
const DRAG_GRAB_Y: f32 = 14.;

impl MainView {
    #[cfg(feature = "capture")]
    pub fn queue_item_bounds_for_capture(
        &self,
        index: usize,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        self.panel.queue_list.bounds_for_item(index)
    }

    #[cfg(feature = "capture")]
    pub fn queue_order_for_capture(&self, cx: &gpui::App) -> Vec<(usize, String)> {
        let bar = self.player.read(cx);
        (0..bar.queue_len())
            .filter_map(|index| bar.queued(index))
            .map(|(id, entry)| (id, entry.track.title.clone()))
            .collect()
    }

    #[cfg(feature = "capture")]
    pub fn sample_queue_motion_for_capture(&self, ms: u64, cx: &mut Context<Self>) {
        self.panel
            .queue_motion
            .borrow_mut()
            .sample(std::time::Duration::from_millis(ms));
        cx.notify();
    }

    #[cfg(feature = "capture")]
    pub fn queue_drag_for_capture(&self) -> Option<(usize, Option<usize>)> {
        self.panel
            .queue_motion
            .borrow()
            .drag
            .map(|drag| (drag.row.id, drag.target))
    }

    #[cfg(feature = "capture")]
    pub fn append_playing_track_to_queue_for_capture(&self, cx: &mut Context<Self>) {
        self.player.update(cx, |bar, cx| {
            if let Some(entry) = bar.current().cloned() {
                bar.command(cx, |playback| playback.insert_next(vec![entry]));
            }
        });
    }

    #[cfg(feature = "capture")]
    pub fn queue_appearance_for_capture(&self, id: usize) -> f32 {
        let motion = self.panel.queue_motion.borrow();
        motion.appearance(id, motion.now(), self.reduced_motion)
    }

    #[cfg(feature = "capture")]
    pub fn queue_removal_for_capture(&self, id: usize, window: &Window) -> (f32, Option<f32>) {
        (
            self.panel
                .queue_motion
                .borrow()
                .exits
                .painted_alpha(queue_span(id)),
            window
                .rendered_span(queue_span(id))
                .map(|bounds| bounds.top().into()),
        )
    }

    #[cfg(feature = "capture")]
    pub fn queue_last_item_bottom_for_capture(&self) -> Option<gpui::Pixels> {
        let last = self.panel.queue_list.item_count().checked_sub(1)?;
        self.queue_item_bounds_for_capture(last)
            .map(|bounds| bounds.bottom())
    }

    pub(super) fn queue_section(&self, cx: &Context<Self>) -> AnyElement {
        let player = self.player.read(cx);
        let entries = player.queue_len();
        let clear = player.has_queued_next().then(|| {
            let revision = player.queue_revision();
            let label = "Очистить предстоящие";
            let id = "queue-clear-upcoming";
            let action = if let Some(material) = self.glass_controls(cx) {
                panel_icon_action(id, label, IconName::ListX, 610, material, true)
            } else {
                button(id, label, 610)
                    .size(px(fluid::size::XS))
                    .flex_none()
                    .justify_center()
                    .hover(|style| style.bg(rgb(ELEVATED)))
                    .child(Icon::new(IconName::ListX).pixel_size(px(16.)))
            };
            action
                .aria_description("Текущий трек и прослушанное сохранятся")
                .tooltip(guise::tooltip(label))
                .on_activate(cx.listener(move |view, _, window, cx| {
                    if !cx.has_active_drag()
                        && view
                            .player
                            .update(cx, |player, cx| player.clear_upcoming(revision, cx))
                    {
                        view.focus_panel(window, cx);
                    }
                }))
        });
        let on_playlist = self.music_page() == Page::Playlist;
        let glass = self.uses_fluid_glass();
        let mut body = div()
            .id("right-panel-queue-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .px(px(if glass {
                fluid::space::MD
            } else if on_playlist {
                metrics::GAP
            } else {
                10.
            }))
            .pb(px(if glass {
                fluid::space::SM
            } else if on_playlist {
                metrics::PAD
            } else {
                10.
            }))
            .child(
                div()
                    .flex()
                    .items_start()
                    .gap(px(fluid::space::XS))
                    .child(div().flex_1().min_w_0().child(self.autoplay_control(cx)))
                    .children(clear),
            )
            .children(self.queue_recommendation_controls(cx));
        let leaving = !self.reduced_motion
            && !cx.reduce_motion()
            && !self.panel.queue_motion.borrow().exits.is_empty();
        if entries == 0 && !leaving {
            body = body.child(empty_state(
                "right-panel-queue-empty",
                "Очередь пуста.",
                Some("Добавьте трек из медиатеки или поиска."),
                IconName::ListMusic,
                cx,
            ));
        } else {
            body = body.child(self.queue_rows(entries, on_playlist, cx));
        }
        body.into_any_element()
    }

    fn queue_rows(&self, count: usize, on_playlist: bool, cx: &Context<Self>) -> Stateful<Div> {
        let bar = self.player.downgrade();
        let player = self.player.read(cx);
        let current_index = player.current_index();
        let revision = player.queue_revision();
        let reduced = self.reduced_motion || cx.reduce_motion();
        let motion = self.panel.queue_motion.clone();
        let now = motion.borrow().now();
        let list_state = self.panel.queue_list.clone();
        let panel_focus = self.panel.focus.clone();
        let row_focuses = Rc::new(RefCell::new(HashMap::new()));
        let style = self
            .uses_fluid_glass()
            .then(|| track_row::Style::new(cx, self.fluid_high_contrast(cx)));
        motion
            .borrow_mut()
            .prepare(now, cx.has_active_drag(), revision, current_index);
        motion.borrow_mut().observe_entries(
            player.queue_replacement_revision(),
            player.next_queue_entry_id(),
            now,
        );
        let line = motion.borrow().drag.and_then(|drag| {
            let target = drag.target?;
            let bounds = row_bounds(&list_state, target, current_index)?;
            let viewport = list_state.viewport_bounds();
            let y = if target > drag.row.index {
                bounds.bottom() - drag.height
            } else {
                bounds.top()
            };
            (viewport.top() <= y && y < viewport.bottom()).then_some(y - viewport.top())
        });
        let outro = {
            let motion = motion.borrow();
            (!motion.exits.is_empty()).then(|| {
                motion
                    .exits
                    .layer_at(!reduced, motion.sample_time())
                    .into_any_element()
            })
        };
        div()
            .id("player-queue-list")
            .accessibility_id("player-queue-list")
            .role(Role::ListBox)
            .aria_label("Треки в очереди")
            .aria_size_of_set(count)
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .overflow_hidden()
            .on_drag_move({
                let motion = motion.clone();
                let list = list_state.clone();
                move |event: &DragMoveEvent<Row>, window, cx| {
                    let row = *event.drag(cx);
                    let mut motion = motion.borrow_mut();
                    if let Some(drag) = motion.drag.as_mut().filter(|drag| drag.row.id == row.id) {
                        drag.target = target_at(&list, row, event.event.position);
                        window.refresh();
                    }
                }
            })
            .on_drop({
                let motion = motion.clone();
                let list = list_state.clone();
                let bar = bar.clone();
                move |row: &Row, window, cx| {
                    let Some(drag) = motion.borrow_mut().drag.take() else {
                        return;
                    };
                    let pointer = window.mouse_position();
                    let Some(target) = target_at(&list, *row, pointer) else {
                        return;
                    };
                    if bar
                        .update(cx, |bar, cx| {
                            bar.move_upcoming(row.revision, row.id, row.index, target, cx)
                        })
                        .unwrap_or(false)
                    {
                        let mut motion = motion.borrow_mut();
                        let now = motion.now();
                        motion.commit(drag, target, &list, pointer.y, now);
                    }
                }
            })
            .child(FluidScrollViewport::new(
                list(list_state.clone(), move |index, window, cx| {
                    bar.update(cx, |bar, cx| {
                        let Some((id, entry)) = bar.queued(index) else {
                            return div().into_any_element();
                        };
                        let current = current_index == Some(index);
                        let data = Row {
                            id,
                            index,
                            revision,
                            current: current_index,
                        };
                        let (offset, moving) = motion.borrow_mut().offset(data, now, reduced);
                        let appearance = motion.borrow().appearance(id, now, reduced);
                        if (moving || appearance < 1.)
                            && window.is_visible()
                            && window.is_window_active()
                        {
                            window.request_animation_frame();
                        }
                        let lifted = motion.borrow().drag.is_some_and(|drag| drag.row.id == id);
                        let row = render_queue_row(
                            entry,
                            (id, index),
                            count,
                            current,
                            on_playlist,
                            style,
                            Some(cx),
                        )
                        .on_activate(cx.listener(move |bar, _, _, cx| {
                            bar.command(cx, |playback| playback.play_queued(index))
                        }))
                        .relative()
                        .top(offset - px(8. * (1. - appearance)))
                        .opacity(0.30 + 0.70 * appearance)
                        .when(lifted, |row| row.opacity(0.))
                        .when(style.is_some(), |row| {
                            let state =
                                window.use_keyed_state(("queue-row-focus", id), cx, |_, cx| {
                                    cx.focus_handle().tab_stop(true)
                                });
                            let handle = state.read(cx).clone().tab_index(620 + index as isize);
                            row_focuses.borrow_mut().insert(id, handle.clone());
                            row.track_focus(&handle)
                        })
                        .when(style.is_some() && data.can_move(), |row| {
                            let deletion = DeleteUpcoming {
                                motion: motion.clone(),
                                list: list_state.clone(),
                                panel_focus: panel_focus.clone(),
                                row_focuses: row_focuses.clone(),
                                animated: !reduced,
                            };
                            let motion = motion.clone();
                            let list = list_state.clone();
                            let preview = DragPreview {
                                title: entry.track.title.clone(),
                                artist: entry.track.artist.clone(),
                                artwork: entry.track.artwork.clone(),
                                seed: Seed::queue_track(&entry.playlist, &entry.track),
                                width: Pixels::ZERO,
                                height: px(FLUID_QUEUE_ROW_HEIGHT),
                                shift: Pixels::ZERO,
                                style: style.expect("Fluid queue row"),
                                reduced,
                            };
                            row.aria_keyshortcuts("Alt+ArrowUp Alt+ArrowDown Delete")
                                .on_drag(data, move |row, offset, window, cx| {
                                    let Some(bounds) = row_bounds(&list, row.index, row.current)
                                    else {
                                        return cx.new(|_| preview.clone());
                                    };
                                    motion.borrow_mut().drag = Some(Drag {
                                        row: *row,
                                        height: bounds.size.height,
                                        cursor_offset: px(DRAG_GRAB_Y),
                                        target: target_at(&list, *row, window.mouse_position()),
                                    });
                                    cx.new(|_| DragPreview {
                                        width: bounds.size.width,
                                        height: bounds.size.height,
                                        shift: offset.y - px(DRAG_GRAB_Y),
                                        ..preview.clone()
                                    })
                                })
                                .on_key_down(cx.listener(
                                    move |bar, event: &KeyDownEvent, window, cx| {
                                        let key = &event.keystroke;
                                        if key.key == "delete"
                                            && !key.modifiers.modified()
                                            && !event.is_held
                                        {
                                            deletion.apply(bar, data, window, cx);
                                            return;
                                        }
                                        if !key.modifiers.alt {
                                            return;
                                        }
                                        let target = match key.key.as_str() {
                                            "up" => index.checked_sub(1),
                                            "down" => Some(index + 1),
                                            _ => None,
                                        };
                                        if let Some(target) = target
                                            && bar.move_upcoming(revision, id, index, target, cx)
                                        {
                                            window.prevent_default();
                                            cx.stop_propagation();
                                        }
                                    },
                                ))
                        });
                        let row = if style.is_some() && !reduced && data.can_move() {
                            painted_fragment(row, queue_span(id), None)
                        } else {
                            row.into_any_element()
                        };
                        let heading = if style.is_none() {
                            None
                        } else if current {
                            Some(("queue-now-playing", "Сейчас играет"))
                        } else if current_index.is_some_and(|current| index == current + 1) {
                            Some(("queue-up-next", "Далее"))
                        } else {
                            None
                        };
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .children(heading.map(|(id, label)| {
                                div()
                                    .id(id)
                                    .accessibility_id(id)
                                    .role(Role::Heading)
                                    .aria_label(label)
                                    .px(px(9.))
                                    .h(px(HEADING_HEIGHT))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .text_size(px(fluid::text::CAPTION))
                                    .text_color(
                                        style.map_or_else(secondary, track_row::Style::secondary),
                                    )
                                    .child(label)
                            }))
                            .child(row)
                            .into_any_element()
                    })
                    .unwrap_or_else(|_| div().into_any_element())
                })
                .size_full(),
                ScrollSurface::VirtualList(self.panel.queue_list.clone()),
                self.uses_fluid_glass(),
                Density::Panel,
            ))
            .children(outro)
            .when_some(line, |list, top| {
                list.child(
                    div()
                        .absolute()
                        .top(top)
                        .left(px(8.))
                        .right(px(8.))
                        .h(px(2.))
                        .rounded(px(1.))
                        .bg(style.map_or_else(|| rgb(ACCENT).into(), |style| style.title(true))),
                )
            })
    }
}

struct DeleteUpcoming {
    motion: Rc<RefCell<QueueMotion>>,
    list: ListState,
    panel_focus: FocusHandle,
    row_focuses: Rc<RefCell<HashMap<usize, FocusHandle>>>,
    animated: bool,
}

impl DeleteUpcoming {
    fn apply(
        &self,
        bar: &mut PlayerBar,
        row: Row,
        window: &mut Window,
        cx: &mut Context<PlayerBar>,
    ) {
        if cx.has_active_drag() {
            return;
        }
        let viewport = self.list.viewport_bounds();
        let visible = |index| {
            row_bounds(&self.list, index, row.current)
                .is_some_and(|bounds| bounds.intersects(&viewport))
        };
        let neighbor_focus = [Some(row.index + 1), row.index.checked_sub(1)]
            .into_iter()
            .flatten()
            .filter(|&index| visible(index))
            .filter_map(|index| bar.queued(index))
            .find_map(|(id, _)| self.row_focuses.borrow().get(&id).cloned());
        let outgoing = row_bounds(&self.list, row.index, row.current)
            .filter(|_| self.animated && window.rendered_span(queue_span(row.id)).is_some());
        let neighbors = if outgoing.is_some() {
            (row.index + 1..self.list.item_count())
                .take_while(|&index| visible(index))
                .filter_map(|index| bar.queued(index).map(|(id, _)| (index, id)))
                .collect()
        } else {
            Vec::new()
        };
        if !bar.remove_upcoming(row.revision, row.id, row.index, cx) {
            return;
        }
        if let Some(bounds) = outgoing {
            let mut motion = self.motion.borrow_mut();
            let now = motion.now();
            motion.remove(row.id, bounds.size.height, neighbors, now);
        }
        window.focus(neighbor_focus.as_ref().unwrap_or(&self.panel_focus), cx);
        window.prevent_default();
        cx.stop_propagation();
    }
}

#[derive(Clone)]
struct DragPreview {
    title: String,
    artist: String,
    artwork: Option<String>,
    seed: Seed,
    width: Pixels,
    height: Pixels,
    shift: Pixels,
    style: track_row::Style,
    reduced: bool,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let scale = if self.reduced { 1. } else { 1.02 };
        let tile = div()
            .w(self.width * scale)
            .h(self.height * scale)
            .flex()
            .items_center()
            .gap(px(9.))
            .px(px(9.))
            .rounded(px(track_row::RADIUS))
            .relative()
            .top(self.shift)
            .border_1()
            .border_color(self.style.title(true).alpha(0.65))
            .bg(rgb(ELEVATED))
            .shadow_sm()
            .child(fluid_cover(
                self.artwork.as_deref(),
                Some(self.seed),
                FLUID_QUEUE_COVER,
                4.,
            ))
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(fluid::text::BODY))
                            .text_color(self.style.title(false))
                            .child(self.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(fluid::text::CAPTION))
                            .text_color(self.style.secondary())
                            .child(self.artist.clone()),
                    ),
            );
        div().child(tile)
    }
}

#[cfg(feature = "capture")]
pub(in crate::ui) fn queue_row(
    entry: &QueueEntry,
    index: usize,
    count: usize,
    current: bool,
    on_playlist: bool,
    fluid: Option<track_row::Style>,
) -> Stateful<Div> {
    render_queue_row(
        entry,
        (index, index),
        count,
        current,
        on_playlist,
        fluid,
        None,
    )
}

fn render_queue_row(
    entry: &QueueEntry,
    (id, index): (usize, usize),
    count: usize,
    current: bool,
    on_playlist: bool,
    fluid: Option<track_row::Style>,
    cx: Option<&Context<PlayerBar>>,
) -> Stateful<Div> {
    use crate::ui::{components::entity_link, playlist::navigation};
    let title = &entry.track.title;
    let artist = ((fluid.is_some() || on_playlist) && !entry.track.artist.trim().is_empty())
        .then_some(entry.track.artist.as_str());
    let origins: Vec<_> = entry
        .recommendations
        .iter()
        .map(|origin| format!("{} · {}", origin.provider.display_name(), origin.playlist))
        .collect();
    let mut label = format!("{}. {title}", index + 1);
    if !origins.is_empty() {
        label.push_str(" · Из подборок · ");
        label.push_str(&origins.join(" / "));
    }
    let (selected, hover, meta) = if on_playlist {
        (
            playlist::PLAYING,
            playlist::HOVER,
            rgb(playlist::INK_SECONDARY).into(),
        )
    } else {
        (PLAYER_PANEL_CURRENT, PLAYER_PANEL_HOVER, secondary())
    };
    let description = match (current, artist) {
        (true, Some(artist)) => Some(format!("Текущий трек · {artist}")),
        (true, None) => Some("Текущий трек".into()),
        (false, artist) => artist.map(str::to_owned),
    };
    let tint = if on_playlist {
        playlist::SIGNAL
    } else {
        ACCENT
    };
    let marker = |ink| {
        div()
            .size(px(metrics::GLYPH))
            .flex_none()
            .when(current, |mark| {
                mark.child(
                    Icon::new(IconName::AudioLines)
                        .pixel_size(px(metrics::GLYPH))
                        .color(ink),
                )
            })
    };
    let row = button(("player-queue-entry", id), label, 620 + index as isize)
        .role(Role::ListBoxOption)
        .aria_selected(current)
        .aria_position_in_set(index)
        .aria_size_of_set(count)
        .w_full()
        .when_some(description, |row, description| {
            row.aria_description(description)
        });
    let row = if let Some(style) = fluid {
        let cover = fluid_cover(
            entry.track.artwork.as_deref(),
            Some(Seed::queue_track(&entry.playlist, &entry.track)),
            FLUID_QUEUE_COVER,
            4.,
        );
        let play = cx
            .filter(|_| crate::playback::can_resolve(&entry.playlist, &entry.track))
            .map(|cx| {
                style
                    .cover_play_button(
                        format!("queue-cover-play-{id}"),
                        format!("Слушать · {title}"),
                        620 + index as isize,
                        FLUID_QUEUE_COVER,
                    )
                    .on_activate(cx.listener(move |bar, _, _, cx| {
                        cx.stop_propagation();
                        bar.command(cx, |playback| playback.play_queued(index));
                    }))
            });
        style
            .surface(style.interaction(row), current)
            .relative()
            .min_h(px(FLUID_QUEUE_ROW_HEIGHT))
            .px(px(9.))
            .gap(px(9.))
            .when(play.is_some(), |row| row.group(track_row::HOVER_GROUP))
            .child(cover.children(play))
    } else {
        row.min_h(px(QUEUE_ROW_HEIGHT))
            .px(px(6.))
            .gap(px(7.))
            .rounded(px(playlist::RADIUS))
            .border_2()
            .when(current, |row| row.bg(rgb(selected)))
            .hover(move |row| row.bg(rgb(hover)))
            .when(on_playlist, |row| {
                row.min_h(px(metrics::LIST_ROW))
                    .px(px(metrics::PAD))
                    .gap(px(metrics::PAD))
                    .child(
                        track_cover(entry.track.artwork.as_deref(), metrics::THUMB)
                            .bg(rgb(playlist::PAPER)),
                    )
            })
            .when(!on_playlist, |row| row.child(marker(rgb(tint).into())))
    };
    let meta = fluid.map_or(meta, track_row::Style::secondary);
    let title_node = div()
        .truncate()
        .text_size(px(12.))
        .when_some(fluid, |title, style| {
            title
                .text_size(px(fluid::text::BODY))
                .text_color(style.title(current))
                .font_weight(if current {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
        });
    let order = 620 + index as isize;
    let title_node = if let Some(cx) = cx {
        entity_link::text_with(
            title_node,
            &format!("queue-track-{id}"),
            title,
            navigation::track_destination(&entry.playlist, &entry.track),
            order,
            PlayerBar::navigation_handler(cx),
        )
    } else {
        title_node.child(title.clone()).into_any_element()
    };
    let artist_node = artist.map(|artist| {
        let node = div().text_size(px(11.)).text_color(meta);
        if let Some(cx) = cx {
            let open = PlayerBar::navigation_handler(cx);
            entity_link::artist_list(
                node,
                &format!("queue-artist-{id}"),
                order,
                navigation::artists(&entry.playlist, &entry.track),
                |node, id, name, destination, order| {
                    entity_link::text_with(node, id, name, Some(destination), order, open.clone())
                },
            )
        } else {
            node.truncate().child(artist.to_owned())
        }
    });
    row.when(!origins.is_empty(), |row| row.py(px(4.)))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(title_node)
                .children(artist_node)
                .children(origins.into_iter().map(|origin| {
                    div()
                        .whitespace_normal()
                        .text_size(px(11.))
                        .text_color(meta)
                        .child(origin)
                })),
        )
        .when_some(fluid, |row, style| row.child(marker(style.title(true))))
        .when(fluid.is_none() && on_playlist && current, |row| {
            row.child(marker(rgb(tint).into()))
        })
}
