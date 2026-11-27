//! Pattern, optical and performance controls of the Fluid Glass settings.

mod audio_controls;

use gpui::{
    App, ClipboardItem, Context, Div, FontWeight, IntoElement, Role, SharedString, Stateful,
    Toggled, WeakEntity, Window, div, prelude::*, px,
};
use guise::{ColorName, Size, Switch, theme};

use crate::{
    storage::Store,
    ui::{
        MainView,
        components::{
            Activate, button,
            glass_control::Tone,
            overlay::{SelectChoice, SelectLabel},
        },
    },
    visual::{PatternId, PatternSelection, QualityChoice, VisualFlags, VisualSettings},
};

use audio_controls::Percentage;

#[derive(IntoElement)]
struct VisualControls {
    owner: WeakEntity<MainView>,
    compact: bool,
}

#[cfg(feature = "capture")]
struct CapturedVisualPreset(String);

#[cfg(feature = "capture")]
impl gpui::Global for CapturedVisualPreset {}

impl RenderOnce for VisualControls {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // The visual editor lays out after the shell and surrounding settings
        // sections return, so their Windows debug render stacks do not overlap.
        self.owner
            .update(cx, |view, cx| {
                view.render_visual_settings(self.compact, window, cx)
                    .into_any_element()
            })
            .unwrap_or_else(|_| gpui::Empty.into_any_element())
    }
}

impl MainView {
    #[cfg(feature = "capture")]
    pub fn copied_visual_preset_for_capture(&self, cx: &gpui::App) -> Option<String> {
        cx.try_global::<CapturedVisualPreset>()
            .map(|preset| preset.0.clone())
    }

    pub(super) fn visual_settings(&self, compact: bool, cx: &Context<Self>) -> impl IntoElement {
        VisualControls {
            owner: cx.weak_entity(),
            compact,
        }
    }

    fn render_visual_settings(
        &self,
        compact: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let visual = self.appearance.visual;
        let palette = theme(cx);
        let glass = self.glass_controls(cx);
        let interactive = !self.is_settings_login() && !self.soloist.is_open();
        let material_editable = interactive && !visual.surface.neutral;
        let label_column = |label: &'static str| {
            div()
                .flex_none()
                .w(px(if compact { 118. } else { 150. }))
                .text_size(px(12.))
                .text_color(palette.dimmed())
                .child(label)
        };
        let choice = |id: String, label: SharedString, selected: bool, order: isize| {
            let control = button(id, label.clone(), order)
                .role(Role::RadioButton)
                .aria_selected(selected)
                .aria_toggled(if selected {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .h(px(32.))
                .px(px(12.))
                .rounded(px(if glass.is_some() { 9. } else { 5. }))
                .text_size(px(12.5))
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .child(label)
                .tab_stop(interactive);
            match glass {
                Some(glass) => glass.paint_enabled(
                    control,
                    if selected {
                        Tone::Selected
                    } else {
                        Tone::Quiet
                    },
                    interactive,
                ),
                None => control,
            }
        };

        let mut pattern_row = div()
            .id("appearance-pattern-group")
            .accessibility_id("appearance-pattern-group")
            .role(Role::RadioGroup)
            .aria_label("Фоновый паттерн")
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(label_column("Фоновый паттерн"));
        let current_pattern = self.appearance.visual.pattern;
        for (index, selection) in std::iter::once(PatternSelection::Auto)
            .chain(PatternId::ALL.into_iter().map(PatternSelection::Fixed))
            .enumerate()
        {
            let selected = current_pattern == selection;
            let id = format!("appearance-pattern-{}", selection.as_str());
            let label: SharedString = if selection == PatternSelection::Auto {
                "Авто".into()
            } else {
                selection.label().into()
            };
            let order = 800 + index as isize;
            pattern_row = pattern_row.child(choice(id, label, selected, order).on_activate(
                cx.listener(move |view, _, window, cx| {
                    let mut visual = view.appearance.visual;
                    visual.pattern = selection;
                    view.update_visual(visual, window, cx);
                }),
            ));
        }

        let quality_row = div()
            .id("appearance-quality-group")
            .accessibility_id("appearance-quality-group")
            .role(Role::Group)
            .aria_label("Профиль производительности")
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(label_column("Производительность"))
            .child(
                self.appearance_quality_select.view(
                    SelectLabel {
                        trigger: "appearance-quality-select",
                        menu: "appearance-quality-options",
                        label: "Профиль производительности",
                        value: visual.quality.label().into(),
                        enabled: interactive,
                    },
                    QualityChoice::ALL
                        .into_iter()
                        .map(|quality| {
                            SelectChoice::new(
                                format!("appearance-quality-{}", quality.as_str()),
                                quality.label(),
                                visual.quality == quality,
                            )
                        })
                        .collect(),
                    |view| &mut view.appearance_quality_select,
                    |view, index, window, cx| {
                        let mut visual = view.appearance.visual;
                        visual.quality = QualityChoice::ALL[index];
                        view.update_visual(visual, window, cx);
                    },
                    glass,
                    !self.reduced_motion && !self.fluid_high_contrast(cx),
                    window,
                    cx,
                ),
            );

        let mut effects_row = div()
            .id("appearance-effects-group")
            .accessibility_id("appearance-effects-group")
            .role(Role::Group)
            .aria_label("Эффекты")
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(label_column("Эффекты"));
        for (index, (flag, label)) in crate::ui::fluid_glass::DEBUG_EFFECTS
            .into_iter()
            .enumerate()
            .filter(|&(_, (flag, _))| {
                matches!(
                    flag,
                    VisualFlags::REFRACTION | VisualFlags::DISPERSION | VisualFlags::BLOOM
                )
            })
        {
            let missing_refraction =
                flag == VisualFlags::DISPERSION && !visual.effects.enabled(VisualFlags::REFRACTION);
            let editable = interactive && !missing_refraction;
            let enabled = visual.effects.enabled(flag) && !missing_refraction;
            let id = format!("appearance-effect-{flag}");
            let order = 850 + index as isize;
            let control = button(id, label, order)
                .role(Role::Switch)
                .tab_stop(editable)
                .when(!editable, |control| {
                    control.a11y_synthetic_children(|tree| tree.parent_node().set_disabled())
                })
                .aria_toggled(if enabled {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .h(px(34.))
                .gap(px(10.))
                .px(px(11.))
                .rounded(px(if glass.is_some() { 9. } else { 5. }))
                .text_size(px(12.5))
                .child(label)
                .child(
                    Switch::new(format!("appearance-visual-{flag}"))
                        .size(Size::Sm)
                        .color(glass.map_or_else(
                            || ColorName::Orange.into(),
                            crate::ui::design_system::controls::switch_color,
                        ))
                        .checked(enabled)
                        .disabled(!editable),
                )
                .when(editable, |control| {
                    control.on_activate(cx.listener(move |view, _, window, cx| {
                        let mut visual = view.appearance.visual;
                        visual.effects.set(flag, !visual.effects.enabled(flag));
                        view.update_visual(visual, window, cx);
                    }))
                });
            let control = match glass {
                Some(glass) => glass.paint_enabled(control, Tone::Quiet, editable),
                None => control,
            };
            effects_row = effects_row.child(control.when(missing_refraction, |control| {
                control.aria_description("Доступна при включённом преломлении")
            }));
        }

        let gallery_open = self.gallery_open;
        let mut gallery = div()
            .id("appearance-gallery")
            .accessibility_id("appearance-gallery")
            .role(Role::Group)
            .aria_label("Галерея шейдеров")
            .flex()
            .flex_col()
            .gap(px(10.));
        if gallery_open {
            let mut grid = div()
                .id("appearance-gallery-grid")
                .accessibility_id("appearance-gallery-grid")
                .role(Role::Group)
                .aria_label("Паттерны библиотеки")
                .flex()
                .flex_wrap()
                .gap(px(10.));
            for (index, pattern) in PatternId::ALL.into_iter().enumerate() {
                let selected = visual.pattern == PatternSelection::Fixed(pattern);
                let thumbnail = self.fluid_gallery_thumb(pattern, visual, 7.5, cx);
                let card = button(
                    format!("appearance-gallery-{}", pattern.as_str()),
                    pattern.label(),
                    900 + index as isize,
                )
                .role(Role::Button)
                .aria_label(pattern.label())
                .aria_selected(selected)
                .w(px(190.))
                .flex()
                .flex_col()
                .items_stretch()
                .gap(px(6.))
                .p(px(6.))
                .rounded(px(10.))
                .child(thumbnail)
                .child(
                    div()
                        .text_size(px(11.5))
                        .font_weight(if selected {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .child(pattern.label()),
                )
                .on_activate(cx.listener(move |view, _, window, cx| {
                    if !interactive {
                        return;
                    }
                    let mut visual = view.appearance.visual;
                    visual.pattern = PatternSelection::Fixed(pattern);
                    view.update_visual(visual, window, cx);
                }));
                let card = match glass {
                    Some(glass) => glass.paint_enabled(
                        card,
                        if selected {
                            Tone::Selected
                        } else {
                            Tone::Quiet
                        },
                        interactive,
                    ),
                    None => card,
                };
                grid = grid.child(card);
            }
            #[cfg(feature = "capture")]
            if self.settings_diagnostics {
                let interval = self.fluid_frame_interval(cx);
                gallery =
                    gallery.child(div().text_size(px(12.)).text_color(palette.dimmed()).child(
                        format!(
                            "Превью {} · интервал перерисовки {interval:.1} мс",
                            crate::ui::fluid_glass::quality_preview_label(visual.quality),
                        ),
                    ));
            }
            gallery = gallery.child(grid);
            #[cfg(feature = "capture")]
            if self.settings_diagnostics {
                gallery = gallery.child(self.shader_debug_studio(visual, interactive, window, cx));
            }
        }
        let preview = self.fluid_glass_preview(visual, cx);
        let pattern_panel = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(pattern_row)
            .child(Percentage::Intensity.row(visual, material_editable, glass.is_some(), cx))
            .child(Percentage::Speed.row(visual, material_editable, glass.is_some(), cx))
            .when(
                material_editable || (interactive && visual.audio.enabled),
                |panel| {
                    panel.child(
                        div()
                            .text_size(px(12.))
                            .text_color(palette.dimmed())
                            .child("Точный ввод — от 0 до 100. Применяется по Enter."),
                    )
                },
            )
            .child(quality_row);
        let neutral = visual.surface.neutral;
        let surface_controls = div()
            .flex()
            .gap(px(12.))
            .when(compact, |row| row.flex_col())
            .child(div().flex_1().min_w_0().child(Percentage::Opacity.row(
                visual,
                material_editable,
                glass.is_some(),
                cx,
            )))
            .child(div().flex_1().min_w_0().child(Percentage::Blur.row(
                visual,
                material_editable && visual.effects.enabled(VisualFlags::REFRACTION),
                glass.is_some(),
                cx,
            )));
        let neutral_control = button("appearance-neutral-background", "Нейтральный фон", 830)
            .role(Role::Switch)
            .aria_toggled(Toggled::from(neutral))
            .h(px(36.))
            .px(px(12.))
            .gap(px(8.))
            .tab_stop(interactive)
            .child("Нейтральный фон")
            .child(
                Switch::new("appearance-neutral-background-state")
                    .size(Size::Sm)
                    .color(glass.map_or_else(
                        || ColorName::Orange.into(),
                        crate::ui::design_system::controls::switch_color,
                    ))
                    .checked(neutral)
                    .disabled(!interactive),
            )
            .when_some(glass, |control, glass| {
                glass.paint_enabled(control, Tone::Quiet, interactive)
            })
            .when(interactive, |control| {
                control.on_activate(cx.listener(move |view, _, window, cx| {
                    let mut visual = view.appearance.visual;
                    visual.surface.neutral = !neutral;
                    view.update_visual(visual, window, cx);
                }))
            });

        div()
            .id("appearance-visual")
            .accessibility_id("appearance-visual")
            .role(Role::Group)
            .aria_label("Паттерн, оптика и производительность")
            .flex()
            .flex_col()
            .gap(px(14.))
            .child(
                div()
                    .flex()
                    .gap(px(18.))
                    .when(compact, |row| row.flex_col())
                    .child(
                        div()
                            .w_full()
                            .when(!compact, |card| card.w(px(360.)).flex_none())
                            .child(preview),
                    )
                    .child(pattern_panel),
            )
            .child(surface_controls)
            .child(div().flex().child(neutral_control))
            .child(effects_row)
            .child(self.audio_controls(glass, interactive, cx))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        match glass {
                            Some(glass) => glass.paint_enabled(
                                button("appearance-copy-preset", "Скопировать пресет", 870)
                                    .h(px(32.))
                                    .px(px(14.))
                                    .rounded(px(9.))
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Скопировать пресет JSON"),
                                Tone::Outlined,
                                interactive,
                            ),
                            None => button("appearance-copy-preset", "Скопировать пресет", 870)
                                .h(px(32.))
                                .px(px(14.))
                                .rounded(px(5.))
                                .child("Скопировать пресет JSON"),
                        }
                        .on_activate(cx.listener(|view, _, _, cx| view.copy_visual_preset(cx))),
                    )
                    .when_some(self.appearance_notice.as_ref(), |row, notice| {
                        row.child(
                            div()
                                .id("appearance-notice")
                                .accessibility_id("appearance-notice")
                                .role(Role::Alert)
                                .text_size(px(12.))
                                .text_color(if glass.is_some_and(|material| !material.contrast()) {
                                    gpui::Hsla::from(gpui::rgb(
                                        crate::ui::fluid_glass::tokens::SUCCESS,
                                    ))
                                } else {
                                    palette.dimmed().hsla()
                                })
                                .child(notice.clone()),
                        )
                    })
                    .child(
                        match glass {
                            Some(glass) => glass.paint_enabled(
                                button("appearance-gallery-toggle", "Галерея шейдеров", 871)
                                    .role(Role::Button)
                                    .h(px(32.))
                                    .px(px(14.))
                                    .rounded(px(9.))
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(if gallery_open {
                                        "Скрыть галерею"
                                    } else {
                                        "Галерея шейдеров"
                                    }),
                                Tone::Outlined,
                                interactive,
                            ),
                            None => button("appearance-gallery-toggle", "Галерея шейдеров", 871)
                                .h(px(32.))
                                .px(px(14.))
                                .rounded(px(5.))
                                .child(if gallery_open {
                                    "Скрыть галерею"
                                } else {
                                    "Галерея шейдеров"
                                }),
                        }
                        .on_toggle(
                            gallery_open,
                            cx.listener(|view, _, _, cx| {
                                view.gallery_open = !view.gallery_open;
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .when(gallery_open, |section| section.child(gallery))
    }

    fn update_visual(
        &mut self,
        visual: VisualSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.appearance.visual == visual {
            return;
        }
        self.appearance.visual = visual;
        self.appearance_notice = None;
        crate::ui::appearance::set_visual(cx, visual);
        if let Some(backdrop) = &self.fluid_backdrop {
            backdrop.update(cx, |backdrop, cx| backdrop.set_visual(visual, window, cx));
        }
        self.persist_visual();
        cx.notify();
    }

    fn persist_visual(&mut self) {
        // The active shell and palette become the initial row values when the
        // profile has none. On an existing row only the visual columns change.
        match Store::open_default().and_then(|store| {
            store.save_visual(
                self.appearance.visual,
                self.appearance.shell,
                self.appearance.palette,
            )
        }) {
            Ok(()) => self.appearance_error = None,
            Err(error) => {
                self.appearance_error = Some(format!("Не удалось сохранить оформление: {error}"))
            }
        }
    }

    fn copy_visual_preset(&mut self, cx: &mut Context<Self>) {
        let visual = self.appearance.visual;
        let preset = serde_json::json!({
            "schema": crate::visual::PATTERN_SCHEMA,
            "palette": self.appearance.palette.as_str(),
            "pattern": visual.pattern.as_str(),
            "intensity": (visual.intensity * 100.).round() as u32,
            "speed": (visual.speed * 100.).round() as u32,
            "surface": { "opacity": visual.surface.opacity, "blur": visual.surface.blur, "neutral": visual.surface.neutral },
            "effects": {
                "fluted": visual.effects.enabled(VisualFlags::FLUTED),
                "refraction": visual.effects.enabled(VisualFlags::REFRACTION),
                "dispersion": visual.effects.enabled(VisualFlags::DISPERSION),
                "grain": visual.effects.enabled(VisualFlags::GRAIN),
                "bloom": visual.effects.enabled(VisualFlags::BLOOM),
                "glitch": visual.effects.enabled(VisualFlags::GLITCH),
                "vignette": visual.effects.enabled(VisualFlags::VIGNETTE),
            },
            "quality": visual.quality.as_str(),
            "audio_response": visual.audio,
        });
        let text = serde_json::to_string_pretty(&preset).unwrap_or_default();
        self.appearance_notice = Some("Пресет скопирован в буфер обмена.".to_owned());
        cx.notify();
        #[cfg(feature = "capture")]
        if self.capture_time.is_some() {
            // The native capture window records the copy action's payload
            // without reading or replacing the desktop clipboard.
            cx.set_global(CapturedVisualPreset(text));
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
}
