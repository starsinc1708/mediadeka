//! Shell and Fluid Glass palette controls inside the profile settings.

#[cfg(feature = "capture")]
mod gallery;
#[cfg(feature = "capture")]
pub(super) mod menus;
#[cfg(feature = "capture")]
mod rows;
#[cfg(feature = "capture")]
mod transport;

use gpui::{Context, Div, FontWeight, Role, Toggled, Window, div, prelude::*, px, rgb};
use guise::{Chip, Size, theme};

use crate::{
    appearance::{GlassPalette, Shell, TrackColumn, TrackDensity},
    storage::Store,
    ui::{
        MainView,
        components::{Activate, button, glass_control::Tone},
        fluid_glass::tokens,
        theme::{ELEVATED, primary, secondary},
    },
};

const PALETTE_ORDER: isize = 760;

impl MainView {
    pub(super) fn appearance_settings(
        &self,
        compact: bool,
        _window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let active = if self.uses_fluid_glass() {
            Shell::FluidGlass
        } else {
            Shell::Classic
        };
        let pending = self.appearance.shell != active;
        let glass = self.glass_controls(cx);
        let palette = theme(cx);
        let interactive = !self.is_settings_login() && !self.soloist.is_open();
        let row_gap = px(if compact { 12. } else { 18. });
        let control = |id: &'static str, label: &'static str, selected: bool, order: isize| {
            let choice = button(id, label, order)
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
                .text_size(px(13.))
                .font_weight(if selected {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::MEDIUM
                })
                .child(label);
            match glass {
                Some(glass) => glass.paint_enabled(
                    choice,
                    if selected {
                        Tone::Selected
                    } else {
                        Tone::Quiet
                    },
                    interactive,
                ),
                None => choice
                    .when(selected, |choice| choice.bg(rgb(ELEVATED)))
                    .text_color(if selected { primary() } else { secondary() })
                    .when(interactive, |choice| {
                        choice.hover(|style| style.bg(rgb(ELEVATED)).text_color(primary()))
                    })
                    .when(!interactive, |choice| choice.opacity(0.6).tab_stop(false)),
            }
        };
        let mut shell_row = div()
            .id("appearance-shell-group")
            .accessibility_id("appearance-shell-group")
            .role(Role::RadioGroup)
            .aria_label("Оболочка")
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .child(
                div()
                    .flex_none()
                    .w(px(if compact { 118. } else { 150. }))
                    .text_size(px(12.))
                    .text_color(palette.dimmed())
                    .child("Оболочка"),
            );
        for shell in Shell::ALL {
            let selected = self.appearance.shell == shell;
            let (id, label) = match shell {
                Shell::FluidGlass => ("appearance-shell-fluid", "Fluid Glass"),
                Shell::Classic => ("appearance-shell-classic", "Классическая"),
            };
            let order = match shell {
                Shell::FluidGlass => 750,
                Shell::Classic => 751,
            };
            shell_row = shell_row.child(
                control(id, label, selected, order)
                    .on_activate(cx.listener(move |view, _, _, cx| view.select_shell(shell, cx))),
            );
        }
        let mut palette_row = div()
            .id("appearance-palette-group")
            .accessibility_id("appearance-palette-group")
            .role(Role::RadioGroup)
            .aria_label("Палитра Fluid Glass")
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(
                div()
                    .flex_none()
                    .w(px(if compact { 118. } else { 150. }))
                    .text_size(px(12.))
                    .text_color(palette.dimmed())
                    .child("Палитра Fluid Glass"),
            );
        for (index, choice) in GlassPalette::ALL.into_iter().enumerate() {
            let selected = self.appearance.palette == choice;
            let id = match choice {
                GlassPalette::Auto => "appearance-palette-auto",
                GlassPalette::Solar => "appearance-palette-solar",
                GlassPalette::Abyss => "appearance-palette-abyss",
                GlassPalette::Ultraviolet => "appearance-palette-ultraviolet",
                GlassPalette::Monochrome => "appearance-palette-monochrome",
                GlassPalette::Ember => "appearance-palette-ember",
                GlassPalette::Acid => "appearance-palette-acid",
                GlassPalette::Glacier => "appearance-palette-glacier",
                GlassPalette::Rosewood => "appearance-palette-rosewood",
            };
            palette_row = palette_row.child(
                control(id, choice.label(), selected, PALETTE_ORDER + index as isize).on_activate(
                    cx.listener(move |view, _, _, cx| view.select_glass_palette(choice, cx)),
                ),
            );
        }
        let mut section = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(row_gap)
            .child(div().text_size(px(13.)).text_color(palette.dimmed()).child(
                "Fluid Glass меняет палитру сразу. Смена оболочки применяется после перезапуска.",
            ))
            .child(shell_row)
            .when(pending, |section| {
                section.child(
                    div()
                        .id("appearance-restart-notice")
                        .accessibility_id("appearance-restart-notice")
                        .role(Role::Alert)
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(180.))
                                .text_size(px(13.))
                                .text_color(secondary())
                                .child("Чтобы применить оболочку, перезапустите Mediadeka."),
                        )
                        .child(
                            match glass {
                                Some(glass) => glass.paint_enabled(
                                    button("appearance-restart", "Перезапустить Mediadeka", 752)
                                        .h(px(32.))
                                        .px(px(14.))
                                        .rounded(px(9.))
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child("Перезапустить"),
                                    Tone::Outlined,
                                    interactive,
                                ),
                                None => {
                                    button("appearance-restart", "Перезапустить Mediadeka", 752)
                                        .h(px(32.))
                                        .px(px(14.))
                                        .rounded(px(5.))
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .bg(rgb(ELEVATED))
                                        .text_color(primary())
                                        .child("Перезапустить")
                                }
                            }
                            .on_activate(cx.listener(
                                |view, _, window, cx| view.restart_application(window, cx),
                            )),
                        ),
                )
            })
            .child(palette_row)
            .when(self.uses_fluid_glass(), |section| {
                let section = section
                    .child(self.track_density_control(cx))
                    .child(self.track_columns_control(cx))
                    .child(
                        div()
                            .border_t_1()
                            .border_color(palette.border())
                            .pt(px(if compact { 14. } else { 20. }))
                            .child(self.visual_settings(compact, cx)),
                    );
                #[cfg(feature = "capture")]
                let section = section.when(self.settings_diagnostics, |section| {
                    section.child(
                        div()
                            .border_t_1()
                            .border_color(palette.border())
                            .pt(px(if compact { 14. } else { 20. }))
                            .flex()
                            .flex_col()
                            .gap(px(12.))
                            .child(self.icon_gallery_section(compact, cx))
                            .child(self.editorial_menu_samples.render(glass, _window, cx)),
                    )
                });
                section
            })
            .when(!self.uses_fluid_glass(), |section| {
                section.child(
                    div()
                        .border_t_1()
                        .border_color(palette.border())
                        .pt(px(if compact { 14. } else { 20. }))
                        .text_size(px(13.))
                        .text_color(palette.dimmed())
                        .child(
                            "Паттерны, оптика и профили производительности доступны в оболочке Fluid Glass.",
                        ),
                )
            })
            .when_some(self.appearance_error.as_ref(), |section, error| {
                section.child(
                    div()
                        .id("appearance-error")
                        .accessibility_id("appearance-error")
                        .role(Role::Alert)
                        .text_size(px(13.))
                        .text_color(rgb(tokens::contrast::ACCENT))
                        .child(error.clone()),
                )
            });
        if self.uses_fluid_glass() && !self.fluid_high_contrast(cx) {
            section = section.child(div().text_size(px(12.)).text_color(palette.dimmed()).child(
                format!(
                    "Действует: {}. Авто берёт цвета текущей обложки.",
                    self.appearance.palette.label()
                ),
            ));
        }
        section
    }

    fn track_density_control(&self, cx: &Context<Self>) -> Div {
        let glass = self.glass_controls(cx);
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(secondary())
                    .child("Плотность треков"),
            )
            .children(
                TrackDensity::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, density)| {
                        super::super::components::section_tab(
                            format!("appearance-density-{}", density.as_str()),
                            density.label(),
                            self.track_density == density,
                            PALETTE_ORDER + 25 + index as isize,
                            glass,
                        )
                        .on_activate(cx.listener(move |view, _, _, cx| {
                            match view.choose_track_density(density) {
                                Ok(()) => view.appearance_error = None,
                                Err(error) => view.appearance_error = Some(error.to_string()),
                            }
                            cx.notify();
                        }))
                    }),
            )
    }

    fn track_columns_control(&self, cx: &Context<Self>) -> Div {
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(10.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(secondary())
                    .child("Колонки треков"),
            )
            .children(
                TrackColumn::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, column)| {
                        let label = match column {
                            TrackColumn::Artist => "Исполнитель",
                            TrackColumn::Album => "Альбом",
                            TrackColumn::Date => "Дата",
                            TrackColumn::Source => "Площадка",
                        };
                        Chip::new(format!("appearance-column-{}", column.as_str()), label)
                            .size(Size::Sm)
                            .tab_order(PALETTE_ORDER + 28 + index as isize)
                            .checked(self.track_column_visibility.shows(column))
                            .on_change(cx.listener(move |view, _, _, cx| {
                                view.appearance_error = view
                                    .toggle_track_column(column)
                                    .err()
                                    .map(|error| error.to_string());
                                cx.notify();
                            }))
                    }),
            )
    }

    fn select_shell(&mut self, shell: Shell, cx: &mut Context<Self>) {
        if self.appearance.shell == shell {
            return;
        }
        self.appearance.shell = shell;
        self.persist_shell();
        cx.notify();
    }

    fn select_glass_palette(&mut self, palette: GlassPalette, cx: &mut Context<Self>) {
        if self.appearance.palette == palette {
            return;
        }
        self.appearance.palette = palette;
        crate::ui::appearance::set_palette(cx, palette);
        if let Some(backdrop) = &self.fluid_backdrop {
            let artwork = self.player.read(cx).current_artwork().map(str::to_owned);
            backdrop.update(cx, |backdrop, cx| {
                backdrop.set_palette(palette, artwork.as_deref(), cx)
            });
        }
        self.persist_palette();
        cx.notify();
    }

    fn persist_shell(&mut self) {
        match Store::open_default()
            .and_then(|store| store.save_shell(self.appearance.shell, self.appearance.palette))
        {
            Ok(()) => self.appearance_error = None,
            Err(error) => {
                self.appearance_error = Some(format!("Не удалось сохранить оформление: {error}"))
            }
        }
    }

    fn persist_palette(&mut self) {
        match Store::open_default().and_then(|store| {
            store.save_glass_palette(self.appearance.palette, self.appearance.shell)
        }) {
            Ok(()) => self.appearance_error = None,
            Err(error) => {
                self.appearance_error = Some(format!("Не удалось сохранить оформление: {error}"))
            }
        }
    }

    fn restart_application(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let persisted = Store::open_default().and_then(|store| {
            store.save_shell(self.appearance.shell, self.appearance.palette)?;
            store.save_glass_palette(self.appearance.palette, self.appearance.shell)
        });
        if let Err(error) = persisted {
            self.appearance_error = Some(format!("Не удалось сохранить оформление: {error}"));
            cx.notify();
            return;
        }
        self.appearance_error = None;
        // The in-app choice replaces the test/acceptance environment override.
        let result = std::env::current_exe().and_then(|executable| {
            let mut command = std::process::Command::new(executable);
            command
                .env_remove("MEDIADEKA_GUI")
                .env_remove("MEDIADEKA_GLASS_THEME")
                .env_remove("MEDIADEKA_GLASS_EFFECTS")
                .env_remove("MEDIADEKA_GLASS_QUALITY")
                .env_remove("MEDIADEKA_GLASS_PATTERN")
                .env_remove("MEDIADEKA_GLASS_INTENSITY")
                .env_remove("MEDIADEKA_GLASS_SPEED");
            command.spawn()
        });
        match result {
            Ok(_) => cx.quit(),
            Err(error) => {
                self.appearance_error =
                    Some(format!("Не удалось перезапустить Mediadeka: {error}"));
                cx.notify();
            }
        }
    }
}
