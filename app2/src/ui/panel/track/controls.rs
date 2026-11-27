use gpui::{AnyElement, Context, FontWeight, IntoElement, prelude::*, px, rgb};
use guise::{Button, Icon, IconName, Size, Variant};

use super::super::panel_icon_action;

use crate::{
    playback::QueueEntry,
    ui::{
        MainView,
        components::{Activate, button, music::control},
        design_system::controls::{ControlState, ControlVariant},
        fluid_glass::tokens::{size, space, text},
        playlist::actions::QueueAction,
        theme::playlist::{self, metrics},
    },
};

impl MainView {
    pub(super) fn inspector_play(
        &self,
        playlist_view: bool,
        playable: bool,
        playing: bool,
        index: usize,
        entry: Option<&QueueEntry>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let entry = entry.cloned();
        let play = cx.listener(move |view, _: &(), _, cx| {
            if playing {
                view.restart_current(cx);
            } else if let Some(entry) = &entry {
                view.player.update(cx, |bar, cx| {
                    bar.command(cx, |playback| playback.play(vec![entry.clone()], 0))
                });
            } else {
                view.playlist_queue(QueueAction::Play, Some(index), cx);
            }
        });
        let id = "playlist-inspector-play";
        let label = if playing {
            "Сначала"
        } else {
            "Слушать"
        };
        let glyph = Icon::new(IconName::Play).pixel_size(px(16.));
        if let Some(controls) = self.glass_controls(cx) {
            let button = button(id, label, 594)
                .h(px(size::SM))
                .px(px(space::MD))
                .gap(px(space::SM))
                .text_size(px(text::METADATA))
                .font_weight(FontWeight::SEMIBOLD)
                .when(playing && playable, |button| {
                    button.aria_description("Воспроизвести текущий трек сначала")
                })
                .child(glyph)
                .child(label);
            ControlVariant::DialogAction
                .paint(
                    button,
                    controls,
                    ControlState {
                        disabled: !playable,
                        ..Default::default()
                    },
                )
                .when(playable, |button| button.on_activate(play))
                .when(!playable, |button| {
                    button
                        .aria_description("Недоступно")
                        .tab_stop(false)
                        .cursor_default()
                })
                .into_any_element()
        } else {
            Button::new(id, label)
                .variant(Variant::Filled)
                .when(playlist_view, |button| {
                    button
                        .color(gpui::Hsla::from(rgb(playlist::SIGNAL)))
                        .size(Size::Xs)
                        .radius(Size::Sm)
                })
                .tab_order(594)
                .left_section(glyph)
                .disabled(!playable)
                .when(!playable, |button| {
                    button.accessible_description("Недоступно")
                })
                .when(playable, |button| {
                    button.on_click(move |_, window, cx| play(&(), window, cx))
                })
                .into_any_element()
        }
    }

    pub(super) fn inspector_enqueue(
        &self,
        playlist_view: bool,
        playable: bool,
        entry: Option<QueueEntry>,
        index: usize,
        cx: &Context<Self>,
    ) -> AnyElement {
        let enqueue = cx.listener(move |view, _: &(), _, cx| match entry.clone() {
            Some(entry) => view.enqueue_entry(entry, cx),
            None => view.playlist_queue(QueueAction::End, Some(index), cx),
        });
        let id = "playlist-inspector-enqueue";
        let label = "Добавить в очередь";
        if let Some(controls) = self.glass_controls(cx) {
            panel_icon_action(id, label, IconName::ListEnd, 596, controls, playable)
                .tooltip(guise::tooltip(label))
                .when(playable, |button| button.on_activate(enqueue))
                .into_any_element()
        } else {
            control(id, label, IconName::ListEnd, 596, playable, false, false)
                .when(playlist_view, |button| {
                    button
                        .pixel_size(px(metrics::CONTROL_SMALL))
                        .variant(Variant::Subtle)
                        .color(gpui::Hsla::from(rgb(playlist::INK_SECONDARY)))
                })
                .when(playable, |button| {
                    button.on_click(move |_, window, cx| enqueue(&(), window, cx))
                })
                .into_any_element()
        }
    }
}
