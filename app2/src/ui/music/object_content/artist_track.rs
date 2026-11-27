//! The loose track list on an artist's Fluid Glass page.

use std::cell::Cell;

use crate::playback::Status;
use gpui::{
    App, Bounds, Context, Div, Entity, FocusHandle, FontWeight, Hsla, IntoElement, ScrollHandle,
    Subscription, Window, canvas, div, fill, point, prelude::*, px, size,
};

use crate::ui::{
    MainView,
    components::{
        entity_link,
        scrollbar::{scroll_offset, set_handle_offset},
        track_row,
    },
    fluid_glass::tokens::text,
    player::{PlayerBar, PositionChanged},
};

use super::{Ink, TrackRow};

const MARK_PERIOD_SECONDS: f64 = 0.95;
const FOCUS_MARGIN: f32 = 6.;

#[derive(IntoElement)]
pub(super) struct RevealFocusedRow {
    pub index: usize,
    pub row: Div,
    pub scroll: ScrollHandle,
}

struct RowFocus {
    scope: FocusHandle,
    entered: Cell<bool>,
}

impl RenderOnce for RevealFocusedRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state =
            window.use_keyed_state(("artist-track-focus-state", self.index), cx, |_, cx| {
                RowFocus {
                    scope: cx.focus_handle(),
                    entered: Cell::new(false),
                }
            });
        let scope = state.read(cx).scope.clone();
        let scroll = self.scroll;
        let reveal = scope.clone();
        div()
            .child(self.row)
            .on_children_prepainted(move |children, window, cx| {
                let focused = reveal.contains_focused(window, cx);
                let previous = state.read(cx).entered.replace(focused);
                let Some(bounds) = children.first().filter(|_| focused && !previous) else {
                    return;
                };
                let viewport = scroll.bounds();
                let top = viewport.top() + px(FOCUS_MARGIN);
                let bottom = viewport.bottom() - px(FOCUS_MARGIN);
                let distance = if bounds.top() < top {
                    bounds.top() - top
                } else if bounds.bottom() > bottom {
                    bounds.bottom() - bottom
                } else {
                    return;
                };
                let offset = scroll_offset(&scroll);
                let scroll = scroll.clone();
                let reveal = reveal.clone();
                window.on_next_frame(move |window, cx| {
                    if reveal.contains_focused(window, cx)
                        && scroll_offset(&scroll) == offset
                        && set_handle_offset(&scroll, offset + distance)
                    {
                        window.refresh();
                    }
                });
            })
            .id(("artist-track-focus-scope", self.index))
            .track_focus(&scope)
    }
}

pub(super) fn body(
    row: &TrackRow<'_>,
    duration: String,
    ink: Ink,
    status: &Status,
    (style, player): (track_row::Style, &Entity<PlayerBar>),
    artwork: Option<Div>,
    cx: &Context<MainView>,
) -> Div {
    let track = row.track;
    let order = 100 + row.index as isize * 2;
    div()
        .w_full()
        .h(px(if row.compact { 42. } else { 55. }))
        .flex()
        .items_center()
        .gap(px(if row.compact { 9. } else { 13. }))
        .px(px(if row.compact { 7. } else { 11. }))
        .child(
            div()
                .w(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(text::CAPTION))
                .text_color(ink.secondary)
                .when(!row.active, |cell| cell.child((row.index + 1).to_string()))
                .when(row.active && !matches!(status, Status::Playing), |cell| {
                    cell.child(style.playback_marker(status))
                })
                .when(row.active && matches!(status, Status::Playing), |cell| {
                    cell.child(play_mark(
                        ink.primary,
                        row.animate_artist_mark,
                        player.clone(),
                    ))
                }),
        )
        .children(artwork)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(entity_link::track(
                    div()
                        .truncate()
                        .text_color(if row.playable || row.active {
                            ink.primary
                        } else {
                            ink.secondary
                        })
                        .text_size(px(if row.compact {
                            text::BODY
                        } else {
                            text::TRACK_TITLE
                        }))
                        .font_weight(FontWeight::MEDIUM),
                    &format!("music-track-title-{}", row.index),
                    row.key,
                    track,
                    order,
                    cx,
                ))
                .when(!track.artist.is_empty(), |name| {
                    name.child(entity_link::artist_names(
                        div().text_size(px(text::CAPTION)).text_color(ink.secondary),
                        &format!("music-track-artist-{}", row.index),
                        row.key,
                        track,
                        order,
                        cx,
                    ))
                }),
        )
        .child(
            div()
                .w(px(43.))
                .flex_none()
                .text_right()
                .text_color(ink.secondary)
                .text_size(px(text::CAPTION))
                .child(duration),
        )
}

fn play_mark(ink: Hsla, motion: bool, player: Entity<PlayerBar>) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            if !motion || cx.reduce_motion() {
                return 0.;
            }
            if window.is_visible() && window.content_mask().bounds.intersects(&bounds) {
                // Reuse the transport's sampled position. Scrolling out of view or changing the
                // active row drops this subscription with the remaining element state.
                window.use_state(cx, |_, cx| {
                    cx.subscribe(
                        &player,
                        |_: &mut Subscription, _, _: &PositionChanged, cx| {
                            cx.notify();
                        },
                    )
                });
            }
            (player.read(cx).position() / MARK_PERIOD_SECONDS).rem_euclid(1.) as f32
        },
        move |bounds, phase, window, _| {
            let phase = phase * std::f32::consts::TAU;
            for (index, offset) in [0., 1.8, 3.7].into_iter().enumerate() {
                let height = px(8. + 4. * (phase + offset).sin());
                let bar = Bounds::new(
                    point(
                        bounds.left() + px(index as f32 * 4.),
                        bounds.bottom() - height,
                    ),
                    size(px(2.), height),
                );
                window.paint_quad(fill(bar, ink));
            }
        },
    )
    .w(px(10.))
    .h(px(12.))
}
