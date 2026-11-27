use gpui::{
    App, Context, Div, Entity, FontWeight, IntoElement, Role, Toggled, WeakEntity, Window, div,
    prelude::*, px, rgb,
};
use guise::{ColorName, Size, Switch, TextInput, TextInputEvent, theme};

use crate::{
    ui::{
        MainView,
        components::{
            Activate, button,
            glass_control::{Material, Tone},
            slider::{Slider, slider_in_window},
        },
        fluid_glass::{AudioAvailability, Backdrop},
    },
    visual::{AudioChannel, AudioProfile, VisualSettings},
};

#[cfg(feature = "capture")]
const AUDIO_CONTROLS_SPAN: gpui::SpanId = 0x6d64617564696f63;

#[derive(Clone, Copy)]
pub(super) enum Percentage {
    Intensity,
    Speed,
    Opacity,
    Blur,
    Sensitivity,
}

impl Percentage {
    fn value(self, visual: VisualSettings) -> u8 {
        match self {
            Self::Intensity => (visual.intensity * 100.).round() as u8,
            Self::Speed => (visual.speed * 100.).round() as u8,
            Self::Opacity => visual.surface.opacity,
            Self::Blur => visual.surface.blur,
            Self::Sensitivity => visual.audio.sensitivity(),
        }
    }

    fn set(self, visual: &mut VisualSettings, value: u8) {
        match self {
            Self::Intensity => visual.intensity = f32::from(value) / 100.,
            Self::Speed => visual.speed = f32::from(value) / 100.,
            Self::Opacity => visual.surface.opacity = value.min(100),
            Self::Blur => visual.surface.blur = value.min(100),
            Self::Sensitivity => visual.audio.set_sensitivity(value),
        }
    }

    pub(super) fn row(
        self,
        visual: VisualSettings,
        enabled: bool,
        fluid: bool,
        cx: &Context<MainView>,
    ) -> impl IntoElement {
        let (group, id, input, label, order) = match self {
            Self::Intensity => (
                "appearance-intensity-group",
                "appearance-intensity-slider",
                "appearance-intensity-input",
                "Интенсивность",
                810,
            ),
            Self::Speed => (
                "appearance-speed-group",
                "appearance-speed-slider",
                "appearance-speed-input",
                "Скорость",
                815,
            ),
            Self::Opacity => (
                "appearance-opacity-group",
                "appearance-opacity-slider",
                "appearance-opacity-input",
                "Плотность стекла",
                820,
            ),
            Self::Blur => (
                "appearance-blur-group",
                "appearance-blur-slider",
                "appearance-blur-input",
                "Размытие преломления",
                825,
            ),
            Self::Sensitivity => (
                "appearance-audio-sensitivity-group",
                "appearance-audio-sensitivity",
                "appearance-audio-sensitivity-input",
                "Чувствительность",
                879,
            ),
        };
        let update = move |view: &mut MainView,
                           value: f64,
                           window: &mut Window,
                           cx: &mut Context<MainView>| {
            let mut visual = view.appearance.visual;
            self.set(&mut visual, value.round() as u8);
            view.update_visual(visual, window, cx);
        };
        let value = self.value(visual);
        div()
            .id(group)
            .accessibility_id(group)
            .role(Role::Group)
            .aria_label(label)
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme(cx).dimmed())
                    .child(label),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(slider_in_window(
                        id,
                        label,
                        order,
                        Slider {
                            value: f64::from(value),
                            min: 0.,
                            max: 100.,
                            step: 1.,
                            enabled,
                            playlist_presentation: false,
                            fluid,
                        },
                        update,
                        cx,
                    ))
                    .child(PrecisePercentage {
                        kind: self,
                        input,
                        label,
                        order: order + 1,
                        value,
                        enabled,
                        owner: cx.weak_entity(),
                    }),
            )
    }
}

#[derive(IntoElement)]
struct PrecisePercentage {
    kind: Percentage,
    input: &'static str,
    label: &'static str,
    order: isize,
    value: u8,
    enabled: bool,
    owner: WeakEntity<MainView>,
}

struct Editor {
    field: Entity<TextInput>,
    applied: u8,
    invalid: bool,
}

impl RenderOnce for PrecisePercentage {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let editor = window.use_keyed_state(self.input, cx, |window, cx| {
            let field = cx.new(|cx| {
                TextInput::new(cx)
                    .id(self.input)
                    .accessible_name(format!("{} в процентах", self.label))
                    .tab_order(self.order)
                    .size(Size::Xs)
                    .value(&self.value.to_string())
            });
            cx.subscribe_in(
                &field,
                window,
                move |editor: &mut Editor, _, event, window, cx| {
                    let TextInputEvent::Submit(text) = event else {
                        return;
                    };
                    let value = text.trim().parse::<u8>().ok().filter(|&value| value <= 100);
                    editor.invalid = value.is_none();
                    if let Some(value) = value {
                        editor.applied = value;
                        editor
                            .field
                            .update(cx, |field, cx| field.set_text(&value.to_string(), cx));
                        let _ = self.owner.update(cx, |view, cx| {
                            let mut visual = view.appearance.visual;
                            self.kind.set(&mut visual, value);
                            view.update_visual(visual, window, cx);
                        });
                    }
                    cx.notify();
                },
            )
            .detach();
            Editor {
                field,
                applied: self.value,
                invalid: false,
            }
        });
        let state = editor.read(cx);
        let field = state.field.clone();
        let invalid = state.invalid && state.applied == self.value;
        if state.applied != self.value {
            editor.update(cx, |state, cx| {
                state.applied = self.value;
                state.invalid = false;
                state
                    .field
                    .update(cx, |field, cx| field.set_text(&self.value.to_string(), cx));
            });
        }
        div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(12.))
            .when(self.enabled, |row| row.child(div().w(px(57.)).child(field)))
            .when(!self.enabled, |row| row.child(self.value.to_string()))
            .child("%")
            .when(invalid, |row| {
                row.child(
                    div()
                        .id("percentage-invalid")
                        .role(Role::Alert)
                        .child("0–100"),
                )
            })
    }
}

#[derive(IntoElement)]
struct SignalStatus {
    backdrop: Option<Entity<Backdrop>>,
    enabled: bool,
}

impl RenderOnce for SignalStatus {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let availability = if !self.enabled {
            None
        } else if let Some(backdrop) = self.backdrop {
            let initial = backdrop.read(cx).audio_availability();
            let status = window.use_keyed_state(
                ("appearance-audio-status", backdrop.entity_id()),
                cx,
                |_, cx| {
                    cx.subscribe(&backdrop, |state, _, &availability, cx| {
                        *state = availability;
                        cx.notify();
                    })
                    .detach();
                    initial
                },
            );
            Some(*status.read(cx))
        } else {
            Some(AudioAvailability::Unavailable)
        };
        let status = availability.map_or("Реакция на музыку выключена", AudioAvailability::label);
        let ink = if availability == Some(AudioAvailability::Unavailable)
            && !crate::ui::theme::fluid_high_contrast(cx)
        {
            rgb(crate::ui::fluid_glass::tokens::WARNING).into()
        } else {
            theme(cx).dimmed().hsla()
        };
        div()
            .id("appearance-audio-status")
            .accessibility_id("appearance-audio-status")
            .role(Role::Status)
            .aria_label(status)
            .text_size(px(12.))
            .text_color(ink)
            .child(status)
    }
}

impl MainView {
    #[cfg(feature = "capture")]
    pub fn reveal_audio_controls_for_capture(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(bounds) = window.rendered_span(AUDIO_CONTROLS_SPAN) else {
            return false;
        };
        let scroll = bounds.top() - px(210.) - self.scroll.offset().y;
        self.set_home_scroll_offset_for_capture(f32::from(scroll), cx);
        true
    }

    pub(super) fn audio_controls(
        &self,
        glass: Option<Material>,
        interactive: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        AudioControls {
            glass,
            interactive,
            owner: cx.weak_entity(),
        }
    }

    fn render_audio_controls(
        &self,
        glass: Option<Material>,
        interactive: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let settings = self.appearance.visual.audio;
        let active = interactive && settings.enabled;
        let master = toggle(
            "appearance-audio-enabled".into(),
            "Адаптировать окружение к музыке",
            settings.enabled,
            interactive,
            glass,
            875,
        )
        .when(interactive, |control| {
            control.on_activate(cx.listener(|view, _, window, cx| {
                let mut visual = view.appearance.visual;
                visual.audio.enabled = !visual.audio.enabled;
                view.update_visual(visual, window, cx);
            }))
        });
        let mut presets = div()
            .id("appearance-audio-profiles")
            .role(Role::RadioGroup)
            .aria_label("Режим реакции на музыку")
            .flex()
            .flex_wrap()
            .gap(px(5.));
        for (index, (profile, label)) in AudioProfile::ALL
            .into_iter()
            .zip(["Спокойный", "Сбалансированный", "Иммерсивный"])
            .enumerate()
        {
            let selected = settings.profile == profile;
            let item = button(
                format!("appearance-audio-{}", profile.as_str()),
                label,
                876 + index as isize,
            )
            .role(Role::RadioButton)
            .tab_stop(active)
            .aria_selected(selected)
            .aria_toggled(toggled(selected))
            .h(px(30.))
            .px(px(9.))
            .rounded(px(8.))
            .text_size(px(12.))
            .child(label)
            .when(active, |item| {
                item.on_activate(cx.listener(move |view, _, window, cx| {
                    let mut visual = view.appearance.visual;
                    visual.audio.profile = profile;
                    view.update_visual(visual, window, cx);
                }))
            });
            presets = presets.child(match glass {
                Some(glass) => glass.paint_enabled(
                    item,
                    if selected {
                        Tone::Selected
                    } else {
                        Tone::Quiet
                    },
                    active,
                ),
                None => item,
            });
        }
        let channels = [
            ("patterns", "Движение паттернов"),
            ("lighting", "Интенсивность освещения"),
            ("reflections", "Оптические отражения"),
            ("dispersion", "Хроматическая дисперсия"),
            ("pulses", "Световые импульсы"),
            ("glitch", "Цифровые искажения"),
        ];
        let mut controls = div().flex().flex_wrap().gap(px(6.));
        for (index, (channel, (id, label))) in
            AudioChannel::ALL.into_iter().zip(channels).enumerate()
        {
            let control = toggle(
                format!("appearance-audio-channel-{id}"),
                label,
                settings.channel_enabled(channel),
                active,
                glass,
                881 + index as isize,
            )
            .when(active, |control| {
                control.on_activate(cx.listener(move |view, _, window, cx| {
                    let mut visual = view.appearance.visual;
                    visual.audio.toggle_channel(channel);
                    view.update_visual(visual, window, cx);
                }))
            });
            controls = controls.child(control);
        }
        let section = div()
            .id("appearance-audio")
            .accessibility_id("appearance-audio")
            .role(Role::Group)
            .aria_label("Музыкальная реактивность")
            .flex()
            .flex_col()
            .items_start()
            .gap(px(8.))
            .pt(px(5.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(14.))
                    .child("Музыкальная реактивность"),
            )
            .child(master)
            .child(SignalStatus {
                backdrop: self.fluid_backdrop.clone(),
                enabled: settings.enabled && !self.appearance.visual.surface.neutral,
            })
            .child(presets)
            .child(
                div()
                    .w_full()
                    .max_w(px(320.))
                    .child(Percentage::Sensitivity.row(
                        self.appearance.visual,
                        active,
                        glass.is_some(),
                        cx,
                    )),
            )
            .child(controls);
        #[cfg(feature = "capture")]
        let section = section.relative().child(
            gpui::canvas(
                |_, _, _| (),
                |bounds, (), window, _| window.with_paint_span(AUDIO_CONTROLS_SPAN, bounds, |_| {}),
            )
            .absolute()
            .size_full(),
        );
        section
    }
}

#[derive(IntoElement)]
struct AudioControls {
    glass: Option<Material>,
    interactive: bool,
    owner: WeakEntity<MainView>,
}

impl RenderOnce for AudioControls {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        // Lay out the settings section after the containing shell's render
        // returns. The debug Windows executable has a 1 MiB main stack.
        self.owner
            .update(cx, |view, cx| {
                view.render_audio_controls(self.glass, self.interactive, cx)
                    .into_any_element()
            })
            .unwrap_or_else(|_| gpui::Empty.into_any_element())
    }
}

fn toggled(enabled: bool) -> Toggled {
    if enabled {
        Toggled::True
    } else {
        Toggled::False
    }
}

fn toggle(
    id: String,
    label: &'static str,
    enabled: bool,
    interactive: bool,
    glass: Option<Material>,
    order: isize,
) -> gpui::Stateful<Div> {
    let control = button(id.clone(), label, order)
        .role(Role::Switch)
        .tab_stop(interactive)
        .aria_toggled(toggled(enabled))
        .h(px(34.))
        .gap(px(8.))
        .px(px(9.))
        .rounded(px(8.))
        .text_size(px(12.))
        .child(label)
        .child(
            Switch::new(format!("{id}-switch"))
                .size(Size::Sm)
                .color(glass.map_or_else(
                    || ColorName::Orange.into(),
                    crate::ui::design_system::controls::switch_color,
                ))
                .checked(enabled)
                .disabled(!interactive),
        );
    match glass {
        Some(glass) => glass.paint_enabled(control, Tone::Quiet, interactive),
        None => control,
    }
}
