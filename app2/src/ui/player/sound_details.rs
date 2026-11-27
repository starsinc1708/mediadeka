use gpui::{
    AnyElement, App, Context, Div, FocusHandle, FontWeight, IntoElement, Role, Stateful, Window,
    div, prelude::*, px,
};
use guise::{Icon, IconName};

use crate::{
    playback::{AudioQuality, SoundDetails, SoundSource},
    ui::{
        Escape,
        assets::provider_mark,
        components::{
            Activate, button,
            overlay::{MenuLayout, TriggerBounds},
        },
        design_system::controls::{self, ControlState, ControlVariant},
        fluid_glass::tokens::{space, text},
        theme::{fluid_high_contrast, secondary},
    },
};

use super::PlayerBar;

const QUALITY_ORDER: isize = 599;
const QUALITY_LABEL: &str = "Качество звука";
const QUALITY_COMPACT_WIDTH: f32 = 128.;

pub(super) struct SoundPopover {
    open: bool,
    anchor: TriggerBounds,
    focus: FocusHandle,
    trigger: FocusHandle,
}

impl SoundPopover {
    pub(super) fn new(cx: &App) -> Self {
        Self {
            open: false,
            anchor: TriggerBounds::default(),
            focus: cx.focus_handle(),
            trigger: cx.focus_handle().tab_index(QUALITY_ORDER).tab_stop(true),
        }
    }
}

impl PlayerBar {
    pub(in crate::ui) fn sound_report(&self) -> (Option<SoundSource>, &SoundDetails) {
        (
            self.playback.sound_source().copied(),
            self.playback.sound_details(),
        )
    }

    pub(super) fn dismiss_sound_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sound_popover.open {
            self.sound_popover.open = false;
            window.focus(&self.sound_popover.trigger, cx);
            cx.notify();
        }
    }

    fn toggle_sound_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sound_popover.open {
            self.dismiss_sound_popover(window, cx);
        } else {
            self.dismiss_devices(window, cx);
            self.sound_popover.open = true;
            window.focus(&self.sound_popover.focus, cx);
            cx.notify();
        }
    }

    pub(super) fn sound_control(&self, labeled: bool, cx: &Context<Self>) -> Option<Div> {
        let source = *self.playback.sound_source()?;
        let quality = self.playback.audio_quality();
        let label = stream_summary(source, quality);
        let expanded = self.sound_popover.open;
        let state = ControlState {
            expanded,
            active: expanded,
            ..ControlState::default()
        };
        let trigger = ControlVariant::GhostIcon
            .paint(
                button("player-sound-details", label.clone(), QUALITY_ORDER),
                controls::material(cx),
                state,
            )
            .track_focus(&self.sound_popover.trigger)
            .aria_expanded(expanded)
            .aria_description(QUALITY_LABEL)
            .when(!expanded, |control| {
                control.tooltip(guise::tooltip(format!("{QUALITY_LABEL} · {label}")))
            })
            .h(px(space::XL))
            .max_w(px(QUALITY_COMPACT_WIDTH))
            .px(px(space::XS))
            .gap(px(space::XS))
            .child(sound_icon(source))
            .when(labeled, |trigger| {
                trigger.child(div().min_w_0().truncate().child(quality.map_or_else(
                    || source.label().to_owned(),
                    |quality| quality.compact_label(),
                )))
            })
            .on_activate(cx.listener(|bar, _, window, cx| bar.toggle_sound_popover(window, cx)));
        Some(self.sound_popover.anchor.capture(trigger))
    }

    pub(super) fn sound_popover_panel(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if !self.fluid_chrome() || !self.sound_popover.open {
            return None;
        }
        let material = controls::material(cx);
        let layout = MenuLayout::new(window, 290., None);
        let (source, details) = self.sound_report();
        let mut fields = vec![(
            "Источник",
            source.map_or("Определяется", SoundSource::label).to_owned(),
        )];
        if let Some(quality) = &details.quality {
            fields.push(("Кодек", quality.codec_label()));
            fields.push(("Частота дискретизации", quality.sample_rate_label()));
            if let Some(bitrate) = quality.bitrate_label() {
                fields.push(("Битрейт", bitrate));
            }
        }
        let items = fields.len() + 1;
        let panel = layout
            .frame("player-sound-popover", QUALITY_LABEL, Some(material))
            .role(Role::Dialog)
            .track_focus(&self.sound_popover.focus)
            .tab_stop(false)
            .tab_group()
            .on_action(cx.listener(|bar, _: &Escape, window, cx| {
                bar.dismiss_sound_popover(window, cx);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .flex()
                    .h(px(layout.item_height))
                    .px(px(space::SM))
                    .items_center()
                    .justify_between()
                    .text_size(px(text::BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(QUALITY_LABEL)
                    .child(
                        ControlVariant::GhostIcon
                            .paint(
                                button(
                                    "player-sound-close",
                                    "Закрыть сведения о звуке",
                                    QUALITY_ORDER + 1,
                                ),
                                material,
                                ControlState::default(),
                            )
                            .size(px(space::XL))
                            .justify_center()
                            .tooltip(guise::tooltip("Закрыть · Esc"))
                            .child(Icon::new(IconName::X).pixel_size(px(space::LG)))
                            .on_activate(cx.listener(|bar, _, window, cx| {
                                bar.dismiss_sound_popover(window, cx);
                            })),
                    ),
            )
            .children(fields.into_iter().map(|(field, value)| {
                div()
                    .flex()
                    .h(px(layout.item_height))
                    .items_center()
                    .justify_between()
                    .px(px(space::SM))
                    .gap(px(space::SM))
                    .text_size(px(text::CAPTION))
                    .child(div().text_color(secondary()).child(field))
                    .child(value)
            }));
        Some(
            layout
                .place(
                    self.sound_popover.anchor.get(),
                    false,
                    (items, 0),
                    panel,
                    self.fluid_motion && !self.reduced_motion && !fluid_high_contrast(cx),
                    cx.listener(|bar, _, window, cx| {
                        bar.dismiss_sound_popover(window, cx);
                        cx.stop_propagation();
                    }),
                )
                .into_any_element(),
        )
    }

    pub(in crate::ui) fn sound_details(&self, compact: bool) -> Option<Stateful<Div>> {
        let source = *self.playback.sound_source()?;
        let quality = self.playback.audio_quality();
        let label = stream_summary(source, quality);
        let text = if compact {
            quality.map_or_else(
                || source.label().to_owned(),
                |quality| quality.compact_label(),
            )
        } else {
            label.clone()
        };
        let id = if compact {
            "player-sound-details"
        } else {
            "track-panel-sound-details"
        };
        Some(
            div()
                .id(id)
                .accessibility_id(id)
                .role(Role::Label)
                .aria_label(label.clone())
                .tooltip(guise::tooltip(label))
                .flex()
                .min_w_0()
                .items_center()
                .gap(px(5.))
                .text_size(px(11.))
                .line_height(px(15.))
                .text_color(secondary())
                .child(div().flex_none().child(sound_icon(source)))
                .child(
                    div()
                        .min_w_0()
                        .when(compact, |text| text.truncate())
                        .child(text),
                ),
        )
    }
}

fn stream_summary(source: SoundSource, quality: Option<&AudioQuality>) -> String {
    quality.map_or_else(
        || source.label().to_owned(),
        |quality| format!("{} · {}", source.label(), quality.label()),
    )
}

fn sound_icon(source: SoundSource) -> AnyElement {
    if let Some(provider) = source.provider() {
        provider_mark(provider, 13.).into_any_element()
    } else {
        let name = match source {
            SoundSource::LocalFile => IconName::FileAudio,
            SoundSource::Link => IconName::Link,
            _ => IconName::Download,
        };
        Icon::new(name)
            .pixel_size(px(13.))
            .color(secondary())
            .into_any_element()
    }
}
