use std::{f32::consts::TAU, time::Duration};

use gpui::{Div, Hsla, Role, Stateful, div, prelude::*, px, rgb};

use crate::providers::ProviderId;

use super::Layout;
use crate::ui::{
    fluid_glass::tokens,
    theme::{SKELETON_BLOCK, SKELETON_CARD, SKELETON_ROW},
};

const GAP: f32 = 12.;
const PULSE: Duration = Duration::from_millis(1400);

pub(super) fn loading(
    layout: Layout,
    provider: Option<ProviderId>,
    time: Option<Duration>,
) -> Stateful<Div> {
    // The shared, opacity-only pulse follows the pinned Sonora skeleton. All blocks use
    // the same clock so the shell stays still while the collection geometry comes in.
    let opacity = time.map_or(0.92, |time| {
        let phase = (time.as_secs_f32() / PULSE.as_secs_f32()).fract();
        0.77 + (1. - (phase * TAU).cos()) * 0.115
    });
    let block_color = if layout.glass.is_some() {
        tokens::ELEVATED
    } else {
        SKELETON_BLOCK
    };
    let block = |element: Div| element.bg(Hsla::from(rgb(block_color)).alpha(opacity));
    let title = |width| block(div()).w(px(width)).h(px(17.)).rounded(px(5.));
    let skeleton = div()
        .id("home-skeleton")
        .accessibility_id("home-skeleton")
        .role(Role::Status)
        .aria_label("Загружаем подборку")
        .w_full()
        .flex()
        .flex_col()
        .gap(px(if layout.compact { 20. } else { 27. }));
    skeleton
        .when(
            provider.is_none_or(|provider| provider == ProviderId::Spotify),
            |body| {
                body.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .child(title(132.))
                        .children((0..2).map(|_| {
                            div().w_full().flex().gap(px(GAP)).children(
                                (0..layout.quick_columns).map(|_| {
                                    div()
                                        .w(px(layout.quick_width))
                                        .h(px(54.))
                                        .flex_none()
                                        .flex()
                                        .items_center()
                                        .gap(px(9.))
                                        .px(px(6.))
                                        .rounded(px(8.))
                                        .when(layout.glass.is_none(), |row| {
                                            row.bg(rgb(SKELETON_ROW))
                                        })
                                        .child(block(div()).size(px(42.)).rounded(px(5.)))
                                        .child(block(div()).h(px(9.)).w(px(62.)).rounded(px(4.)))
                                }),
                            )
                        })),
                )
            },
        )
        .children((0..2).map(|shelf| {
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap(px(11.))
                .child(title(if shelf == 0 { 164. } else { 124. }))
                .child(
                    div()
                        .w_full()
                        .flex()
                        .gap(px(GAP))
                        .children((0..layout.columns).map(|index| {
                            div()
                                .w(px(layout.card_width))
                                .flex_none()
                                .flex()
                                .flex_col()
                                .rounded(px(9.))
                                .overflow_hidden()
                                .when(layout.glass.is_none(), |card| card.bg(rgb(SKELETON_CARD)))
                                .child(block(div()).w_full().aspect_ratio(1.))
                                .child(
                                    div()
                                        .h(px(50.))
                                        .px(px(8.))
                                        .flex()
                                        .flex_col()
                                        .justify_center()
                                        .gap(px(6.))
                                        .child(
                                            block(div())
                                                .w(px(layout.card_width
                                                    * (0.51 + (index % 3) as f32 * 0.1)))
                                                .h(px(9.))
                                                .rounded(px(4.)),
                                        )
                                        .child(
                                            block(div())
                                                .w(px(layout.card_width * 0.35))
                                                .h(px(7.))
                                                .rounded(px(3.)),
                                        ),
                                )
                        })),
                )
        }))
}
