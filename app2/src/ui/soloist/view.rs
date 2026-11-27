use gpui::{
    AnyElement, App, Context, Div, FontWeight, IntoElement, KeyDownEvent, Role, SharedString,
    Window, div, hsla, prelude::*, px, rgb,
};
use guise::{Button, Icon, IconName, Variant};

use crate::{
    providers::ProviderId,
    soloist::{PairProgress, Readiness},
};

use super::{Kind, MainView, Work};
use crate::ui::{
    assets::provider_mark,
    components::{Activate, button, heading, overlay},
    design_system::{
        controls::{ControlState, ControlVariant},
        materials::Material,
    },
    theme::{
        ACCENT, ELEVATED, PRIMARY, SECONDARY, SETTINGS_ERROR_BACKGROUND, SETTINGS_ROW_DIVIDER,
        SURFACE, secondary, surface_border,
    },
};

const DASHBOARD: &str = "https://developer.spotify.com/dashboard/soloist";

impl MainView {
    pub(in crate::ui) fn soloist_settings(
        &self,
        interactive: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let status = match (
            &self.soloist.readiness,
            self.soloist.active(),
            self.spotify_account(),
        ) {
            (_, Some(Kind::Check), _) => "Проверяем подключение…".to_owned(),
            (_, Some(Kind::Install), _) => "Устанавливаем…".to_owned(),
            (_, Some(Kind::Pair), _) => "Сопряжение…".to_owned(),
            (Some(Readiness::Ready { version }), _, Some(_)) => format!("Подключён · {version}"),
            (Some(Readiness::Installed { version, .. } | Readiness::Ready { version }), _, _) => {
                format!("Установлен · {version}")
            }
            _ if self.soloist.issue.is_some() => "Проверка не удалась".to_owned(),
            _ => "Установка и сопряжение".to_owned(),
        };
        let control = Button::new("settings-soloist", "Настроить")
            .variant(Variant::Default)
            .tab_order(40)
            .track_focus(&self.soloist.settings_focus)
            .accessible_name("Настроить Spotify Soloist")
            .size(guise::Size::Xs);
        let control = if interactive {
            control.on_click(cx.listener(|view, _, window, cx| view.open_soloist(None, window, cx)))
        } else {
            control.disabled(true)
        };
        div()
            .id("settings-soloist-row")
            .accessibility_id("settings-soloist-row")
            .role(Role::Group)
            .aria_label("Spotify Soloist")
            .min_h(px(52.))
            .px(px(13.))
            .gap(px(12.))
            .border_t_1()
            .border_color(rgb(SETTINGS_ROW_DIVIDER))
            .flex()
            .items_center()
            .child(
                div()
                    .size(px(25.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .items_center()
                    .child(
                        Icon::new(IconName::Headphones)
                            .pixel_size(px(17.))
                            .color(gpui::Hsla::from(rgb(ACCENT))),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Spotify Soloist"),
                    )
                    .child(
                        div()
                            .id("settings-soloist-status")
                            .accessibility_id("settings-soloist-status")
                            .role(Role::Status)
                            .aria_label(status.clone())
                            .text_size(px(12.))
                            .text_color(rgb(SECONDARY))
                            .truncate()
                            .child(status),
                    ),
            )
            .child(control)
    }

    pub(in crate::ui) fn soloist_overlay(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let dialog = self.soloist.dialog.as_ref()?;
        let material = self.glass_controls(cx);
        let playback = dialog.opened_for.is_some();
        let active = self.soloist.active();
        let ready = matches!(self.soloist.readiness, Some(Readiness::Ready { .. }));
        let installed = matches!(self.soloist.readiness, Some(Readiness::Installed { .. }));
        let step = if ready || active == Some(Kind::Pair) {
            2
        } else if installed {
            1
        } else {
            0
        };
        let body_height = (window.viewport_size().height - px(158.)).max(px(145.));
        let close_label = if active.is_some_and(|kind| kind != Kind::Check) {
            "Отменить"
        } else {
            "Закрыть"
        };
        let issue = self.soloist.issue.as_deref().or({
            if let Some(Readiness::Installed { detail, .. }) = &self.soloist.readiness {
                detail.as_deref()
            } else {
                None
            }
        });
        let dialog_title = "Spotify Soloist";
        let close = button("soloist-close", "Закрыть настройку Spotify Soloist", 708)
            .track_focus(&self.soloist.close_focus)
            .size(px(32.))
            .justify_center()
            .rounded(px(7.))
            .when(material.is_none(), |close| {
                close.hover(|style| style.bg(rgb(ELEVATED)))
            })
            .when_some(material, |close, material| {
                ControlVariant::GhostIcon.paint(close, material, ControlState::default())
            })
            .child(Icon::new(IconName::X).pixel_size(px(16.)))
            .on_activate(cx.listener(|view, _, window, cx| {
                view.close_soloist(window, cx);
            }));
        let body = div()
            .id("soloist-body")
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .max_h(body_height)
            .gap(px(10.))
            .px(px(2.))
            .py(px(3.))
            .child(progress(step, material))
            .when(playback && !ready, |body| {
                body.child(text("Чтобы воспроизвести Spotify, подключите Soloist."))
            })
            .children(issue.map(alert))
            .child(self.soloist_step(material));
        let footer = div()
            .mt(px(12.))
            .flex()
            .w_full()
            .items_center()
            .gap(px(8.))
            .child(dialog_action(
                "soloist-cancel",
                close_label,
                715,
                false,
                material,
                cx.listener(|view, _, window, cx| {
                    view.close_soloist(window, cx);
                }),
            ))
            .child(div().flex_1())
            .children(self.soloist_next(playback, material, cx));
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.72))
                .occlude()
                .child(
                    div()
                        .id("soloist-dialog")
                        .accessibility_id("soloist-dialog")
                        .role(Role::Dialog)
                        .aria_label("Настройка Spotify Soloist")
                        .track_focus(&self.soloist.focus)
                        .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                            if event.keystroke.key == "escape" {
                                view.close_soloist(window, cx);
                                cx.stop_propagation();
                            } else if event.keystroke.key == "enter"
                                && view.soloist.focus.is_focused(window)
                            {
                                view.submit_soloist(window, cx);
                                cx.stop_propagation();
                            }
                        }))
                        .w(px(548.))
                        .max_w_full()
                        .flex()
                        .flex_col()
                        .occlude()
                        .px(px(19.))
                        .py(px(14.))
                        .rounded(px(13.))
                        .border_1()
                        .when(material.is_none(), |dialog| {
                            dialog.border_color(surface_border()).bg(rgb(SURFACE))
                        })
                        .when_some(material, overlay::menu_surface)
                        .shadow_lg()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(10.))
                                .mb(px(10.))
                                .child(provider_mark(ProviderId::Spotify, 25.))
                                .child(div().flex_1().min_w_0().child(heading(dialog_title, 23.)))
                                .child(close),
                        )
                        .child(body)
                        .child(footer),
                ),
        )
    }

    fn soloist_step(&self, material: Option<Material>) -> Div {
        let mut content = div().flex().flex_col().gap(px(8.));
        match self.soloist.active() {
            Some(Kind::Check) => return content.child(status("Проверяем установку и сопряжение…")),
            Some(Kind::Install) => {
                return content.child(status("Загружаем и проверяем сборку Spotify…"));
            }
            Some(Kind::Pair) => {
                let state = self
                    .soloist
                    .operation
                    .as_ref()
                    .and_then(|operation| operation.progress.as_ref());
                let description = match state {
                    Some(PairProgress::Waiting { .. }) => "Ожидаем выбор устройства в Spotify…",
                    Some(PairProgress::Verifying) => "Проверяем аккаунт и запуск Soloist…",
                    Some(PairProgress::Starting) | None => "Запускаем сопряжение…",
                };
                content = content.child(status(description)).child(pair_steps());
                if let Some(PairProgress::Waiting { port }) = state {
                    content = content.child(text(format!("Если устройство не появилось, проверьте доступ Spotify и Mediadeka к локальной сети (TCP {port}).")));
                }
                return content;
            }
            None => {}
        }
        match &self.soloist.readiness {
            Some(Readiness::Ready { version }) => {
                content.child(status("Soloist подключён"))
                    .child(text(format!("Версия {version}. При следующем запуске Mediadeka запустит Soloist в фоне.")))
            }
            Some(Readiness::Installed { .. }) if self.spotify_account().is_some() => {
                content.child(text("Откройте панель Spotify Soloist и вставьте свой ключ. Затем выберите «Mediadeka Soloist» среди устройств Spotify."))
                    .child(div().flex().child(
                        dialog_action("soloist-dashboard", "Открыть панель Spotify", 709, false, material, |_, _, cx| {
                            cx.open_url(DASHBOARD);
                        }),
                    ))
                    .child(self.soloist.key.clone())
            }
            Some(Readiness::Installed { .. }) => {
                content.child(text("Soloist установлен. Войдите в Spotify в настройках Mediadeka, затем вернитесь сюда для сопряжения с тем же аккаунтом."))
            }
            _ => content.child(text("Mediadeka загрузит Soloist с сервера Spotify и установит его в текущий профиль Windows. После установки мастер поможет подключить аккаунт."))
        }
    }

    fn soloist_next(
        &self,
        playback: bool,
        material: Option<Material>,
        cx: &Context<Self>,
    ) -> Option<Div> {
        if self.soloist.active().is_some() {
            return None;
        }
        let (id, label) = match self.soloist.readiness {
            Some(Readiness::Installed { .. }) if self.spotify_account().is_some() => {
                ("soloist-pair", "Начать сопряжение")
            }
            Some(Readiness::Installed { .. }) => ("soloist-spotify-signin", "Войти в Spotify"),
            Some(Readiness::Ready { .. }) if playback => ("soloist-retry-play", "Повторить запуск"),
            Some(Readiness::Ready { .. }) => ("soloist-done", "Готово"),
            _ => ("soloist-install", "Установить Soloist"),
        };
        let retry = matches!(
            self.soloist.readiness,
            Some(Readiness::Installed {
                detail: Some(_),
                ..
            })
        ) || self.soloist.issue.is_some() && self.soloist.readiness.is_some();
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .when(retry, |row| {
                    row.child(dialog_action(
                        "soloist-recheck",
                        "Проверить",
                        714,
                        false,
                        material,
                        cx.listener(|view, _, _, cx| {
                            view.run_soloist(
                                Work::Check {
                                    account: view.spotify_account(),
                                },
                                cx,
                            );
                        }),
                    ))
                })
                .child(dialog_action(
                    id,
                    label,
                    716,
                    true,
                    material,
                    cx.listener(|view, _, window, cx| view.submit_soloist(window, cx)),
                )),
        )
    }
}

fn dialog_action(
    id: &'static str,
    label: &'static str,
    order: isize,
    primary: bool,
    material: Option<Material>,
    activate: impl Fn(&(), &mut Window, &mut App) + 'static,
) -> AnyElement {
    if let Some(material) = material {
        let variant = if primary {
            ControlVariant::DialogAction
        } else {
            ControlVariant::TextAction
        };
        variant
            .paint(
                button(id, label, order)
                    .h(px(34.))
                    .px(px(11.))
                    .text_size(px(13.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(label),
                material,
                ControlState::default(),
            )
            .on_activate(activate)
            .into_any_element()
    } else {
        Button::new(id, label)
            .variant(if primary {
                Variant::Light
            } else {
                Variant::Default
            })
            .tab_order(order)
            .size(guise::Size::Xs)
            .on_click(move |_, window, cx| activate(&(), window, cx))
            .into_any_element()
    }
}

fn progress(step: usize, material: Option<Material>) -> Div {
    div().flex().gap(px(6.)).children(
        ["Установка", "Ключ", "Сопряжение"]
            .into_iter()
            .enumerate()
            .map(|(index, title)| {
                let current = index == step;
                let ink = material.map_or_else(
                    || rgb(if current { PRIMARY } else { SECONDARY }).into(),
                    |material| {
                        ControlVariant::Tab
                            .resolve(
                                material,
                                ControlState {
                                    active: current,
                                    ..ControlState::default()
                                },
                            )
                            .fg
                    },
                );
                div()
                    .flex_1()
                    .min_w_0()
                    .rounded(px(6.))
                    .px(px(8.))
                    .py(px(5.))
                    .text_size(px(12.))
                    .text_center()
                    .when(current, |step| step.font_weight(FontWeight::SEMIBOLD))
                    .text_color(ink)
                    .child(title)
            }),
    )
}

fn pair_steps() -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(3.))
        .child(text("1. Откройте Spotify с тем же аккаунтом."))
        .child(text("2. Запустите трек и откройте список устройств."))
        .child(text("3. Выберите «Mediadeka Soloist»."))
}

fn text(message: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(13.))
        .line_height(px(19.))
        .text_color(secondary())
        .child(message.into())
}

fn status(message: &'static str) -> impl IntoElement {
    text(message)
        .id("soloist-progress")
        .accessibility_id("soloist-progress")
        .role(Role::Status)
        .aria_label(message)
}

fn alert(message: &str) -> impl IntoElement {
    text(message.to_owned())
        .id("soloist-error")
        .accessibility_id("soloist-error")
        .role(Role::Alert)
        .aria_label(message.to_owned())
        .px(px(8.))
        .py(px(6.))
        .rounded(px(7.))
        .bg(rgb(SETTINGS_ERROR_BACKGROUND))
}
