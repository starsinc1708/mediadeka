use gpui::{
    ClipboardItem, Context, Div, Entity, FontWeight, IntoElement, Pixels, Role, Stateful, Window,
    div, prelude::*, px, rgb,
};
use guise::{Button, Icon, IconName, Scrollbar, TextInput, Variant, theme};

use crate::{
    providers::{LoginInput, LoginPrompt, ProviderId},
    state::Screen,
    ui::{
        MainView,
        components::{
            Activate, button,
            glass_control::{Material, Tone},
        },
        design_system::controls::ControlVariant,
        effects::CONNECTION_RADIUS,
        theme::{ELEVATED, icon_button_hover, notice_border, notice_surface},
        welcome_motion::Flight,
    },
};

use super::{
    Connection, Form, Phase, actions_row, message, open_link, provider_heading, soundcloud_input,
    status_message, youtube_validation_error,
};

impl MainView {
    pub(in crate::ui) fn settings_login_overlay(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let entry = self.login.as_ref().filter(|_| self.is_settings_login())?;
        let viewport = window.viewport_size();
        let compact = viewport.height < px(500.);
        let width = px(536.).min(viewport.width - px(48.));
        let preferred_height = match entry.provider {
            ProviderId::Spotify | ProviderId::YandexMusic => 326.,
            ProviderId::SoundCloud if matches!(entry.phase, Phase::Setup) => 216.,
            ProviderId::SoundCloud => 326.,
            ProviderId::Jellyfin => 390.,
            ProviderId::YouTubeMusic => 462.,
        };
        let height = px(preferred_height).min(viewport.height - px(42.));
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .bg(theme(cx).body().alpha(0.72))
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .id("settings-login-modal")
                        .accessibility_id("settings-login-modal")
                        .role(Role::Dialog)
                        .aria_label(format!("Вход · {}", entry.provider.display_name()))
                        .track_focus(&self.connection_focus)
                        .w(width)
                        .h(height)
                        .child(self.connection_panel(entry, compact, width, None, cx)),
                ),
        )
    }

    pub(in crate::ui) fn connection_panel(
        &self,
        entry: &Connection,
        compact: bool,
        width: Pixels,
        flight: Option<Flight>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let modal = self.is_settings_login();
        let surface = flight.map_or(1., |flight| flight.surface);
        let glass = self.glass_controls(cx);
        let palette = theme(cx);
        let panel = div()
            .id("login-panel")
            .accessibility_id("login-panel")
            .role(Role::Group)
            .aria_label(entry.provider.display_name())
            .relative()
            .size_full()
            .min_h_0()
            .when(surface > 0., |panel| {
                panel.child(connection_surface(
                    width,
                    surface,
                    compact && !modal,
                    glass,
                    cx,
                ))
            });
        if flight.is_some_and(|flight| !flight.controls_visible()) {
            return panel;
        }
        let pending = matches!(entry.phase, Phase::Working(_) | Phase::ChooseYouTube { .. });
        let submit =
            match (&entry.form, pending) {
                (Some(Form::YouTube { .. }), false) => Some(actions_row().child(
                    primary_action("choose-youtube", "Продолжить", 23, glass).on_click(
                        cx.listener(|view, _, window, cx| view.submit_youtube(window, cx)),
                    ),
                )),
                (
                    Some(Form::Jellyfin {
                        server,
                        username,
                        password,
                    }),
                    false,
                ) => Some(jellyfin_actions(server, username, password, glass, cx)),
                _ => None,
            };
        let youtube_error = self.youtube_input_issue(entry, cx);
        let status = youtube_error
            .is_none()
            .then(|| self.connection_status(entry, cx))
            .flatten();
        let status_rows = usize::from(status.is_some());
        let side_by_side = width >= px(410.);
        let show_form = !matches!(entry.phase, Phase::ChooseYouTube { .. });
        let fields = if show_form {
            focused_rows(entry, status_rows, side_by_side)
        } else {
            Vec::new()
        };
        let revealed_field = entry.revealed_field.clone();
        let scroll = entry.scroll.clone();
        let content = div()
            .on_children_prepainted(move |_, window, cx| {
                let focused = fields
                    .iter()
                    .enumerate()
                    .find_map(|(field_index, (row, field))| {
                        field
                            .read(cx)
                            .is_focused(window)
                            .then_some((field_index, *row))
                    });
                if revealed_field.replace(focused) != focused
                    && let Some((_, row)) = focused
                {
                    scroll.scroll_to_item(row);
                    window.request_animation_frame();
                }
            })
            .id("login-panel-scroll")
            .min_h_0()
            .flex_1()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(if compact { 8. } else { 11. }))
            .pr(px(12.))
            .pb(px(1.))
            .overflow_y_scroll()
            .track_scroll(&entry.scroll)
            .children(status)
            .when(show_form, |body| {
                connection_form(
                    entry,
                    body,
                    side_by_side,
                    status_rows,
                    youtube_error,
                    glass,
                    cx,
                )
            });

        let controls = div()
            .size_full()
            .min_h_0()
            .p(px(if compact { 16. } else { 24. }))
            .flex()
            .flex_col()
            .gap(px(if compact { 10. } else { 15. }))
            .opacity(flight.map_or(1., |flight| flight.content))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .pb(px(if compact { 7. } else { 10. }))
                    .border_b_1()
                    .border_color(palette.border())
                    .when(compact && !modal, |header| {
                        header.child(self.connection_return(cx))
                    })
                    .child(provider_heading(entry.provider))
                    .when(modal, |header| {
                        header.justify_between().child(
                            button("settings-close-login", "Закрыть окно входа", 18)
                                .size(px(32.))
                                .flex_none()
                                .justify_center()
                                .rounded(px(6.))
                                .when_some(glass, |control, glass| {
                                    glass.paint(control, Tone::Quiet)
                                })
                                .when(glass.is_none(), |control| {
                                    control.hover(|style| style.bg(icon_button_hover()))
                                })
                                .child(
                                    Icon::new(IconName::X)
                                        .pixel_size(px(18.))
                                        .color(palette.text().hsla()),
                                )
                                .on_activate(cx.listener(|this, _, window, cx| {
                                    this.close_settings_connection(window, cx);
                                })),
                        )
                    }),
            )
            .child(
                div()
                    .relative()
                    .min_h_0()
                    .flex_1()
                    .w_full()
                    .flex()
                    .flex_col()
                    .child(content)
                    .child(Scrollbar::new("login-panel-scrollbar", &entry.scroll)),
            )
            .children(submit.map(|actions| actions.flex_none().justify_end().pt(px(2.))));

        panel.child(controls)
    }

    fn youtube_input_issue(&self, entry: &Connection, cx: &Context<Self>) -> Option<String> {
        let Form::YouTube {
            session, missing, ..
        } = entry.form.as_ref()?
        else {
            return None;
        };
        if *missing && session.read(cx).text().trim().is_empty() {
            return Some("Вставьте значение Cookie из YouTube Music.".into());
        }
        if matches!(entry.phase, Phase::Finished) {
            return youtube_validation_error(self.state.screen()).map(ToString::to_string);
        }
        None
    }

    fn connection_status(&self, entry: &Connection, cx: &Context<Self>) -> Option<Div> {
        let glass = self.glass_controls(cx);
        match &entry.phase {
            Phase::Setup => None,
            Phase::Working(prompt) => Some(waiting_banner(prompt.as_ref(), glass, cx)),
            Phase::ChooseYouTube { choices, .. } => Some(
                banner(false, glass, cx)
                    .child(status_message(
                        "login-choose-profile",
                        "Выберите профиль YouTube Music.".into(),
                        cx,
                    ))
                    .children(
                        choices
                            .profiles()
                            .iter()
                            .enumerate()
                            .map(|(index, profile)| {
                                let id = profile.id.clone();
                                Button::new(("youtube-profile", index), profile.name.clone())
                                    .variant(Variant::Light)
                                    .when_some(glass, |button, material| {
                                        ControlVariant::SecondaryButton
                                            .text_button(button, material)
                                    })
                                    .tab_order(index as isize + 24)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.choose_youtube(id.clone(), window, cx);
                                    }))
                            }),
                    ),
            ),
            Phase::Finished => {
                let (id, detail, warning) = match self.state.screen() {
                    Screen::Cancelled(_) => ("login-cancelled", "Вход отменён".into(), false),
                    Screen::Failed(failure) => ("login-error", failure.issue.to_string(), true),
                    _ => return None,
                };
                Some(
                    banner(warning, glass, cx)
                        .child(status_message(id, detail, cx))
                        .when(!warning || entry.form.is_none(), |banner| {
                            let label = if entry.form.is_none() {
                                "Повторить"
                            } else {
                                "Изменить данные"
                            };
                            banner.child(
                                actions_row().child(
                                    Button::new("retry", label)
                                        .variant(Variant::Light)
                                        .when_some(glass, |button, material| {
                                            ControlVariant::TextAction.text_button(button, material)
                                        })
                                        .tab_order(69)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.retry_connection(window, cx);
                                        })),
                                ),
                            )
                        }),
                )
            }
        }
    }
}

#[cfg(feature = "capture")]
impl MainView {
    pub fn login_scroll_for_capture(&self) -> Option<(f32, bool)> {
        let entry = self.login.as_ref()?;
        let active_fully_visible = entry.revealed_field.get().is_some_and(|(_, index)| {
            entry.scroll.bounds_for_item(index).is_some_and(|row| {
                let viewport = entry.scroll.bounds();
                let offset = entry.scroll.offset().y;
                row.top() + offset >= viewport.top() - px(1.)
                    && row.bottom() + offset <= viewport.bottom() + px(1.)
            })
        });
        Some((f32::from(entry.scroll.max_offset().y), active_fully_visible))
    }
}

fn connection_surface(
    width: Pixels,
    coverage: f32,
    compact: bool,
    glass: Option<Material>,
    cx: &Context<MainView>,
) -> Div {
    let visible_width = width * coverage;
    let inset = if compact {
        (width - visible_width) * 0.5
    } else {
        px(0.)
    };
    let surface = div()
        .absolute()
        .left(px(0.) - inset)
        .top_0()
        .w(width)
        .h_full()
        .rounded(px(CONNECTION_RADIUS))
        .border_1()
        .border_color(theme(cx).border())
        .bg(theme(cx).surface().hsla())
        .when_some(glass, |surface, glass| glass.group(surface));

    div()
        .absolute()
        .top_0()
        .left(inset)
        .w(visible_width)
        .h_full()
        .overflow_hidden()
        .child(surface)
}

fn focused_rows(
    entry: &Connection,
    start: usize,
    side_by_side: bool,
) -> Vec<(usize, Entity<TextInput>)> {
    match &entry.form {
        Some(Form::YouTube { session, .. }) => vec![(start + 1, session.clone())],
        Some(Form::Jellyfin {
            server,
            username,
            password,
        }) => vec![
            (start, server.clone()),
            (start + 1, username.clone()),
            (start + if side_by_side { 1 } else { 2 }, password.clone()),
        ],
        None => Vec::new(),
    }
}

fn connection_form(
    entry: &Connection,
    form: Stateful<Div>,
    side_by_side: bool,
    status_rows: usize,
    youtube_error: Option<String>,
    glass: Option<Material>,
    cx: &Context<MainView>,
) -> Stateful<Div> {
    match &entry.form {
        Some(Form::YouTube {
            session, help_open, ..
        }) => form
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(message("1. Войдите на сайте.", cx))
                    .child(open_link(
                        "open-youtube",
                        "Открыть YouTube Music",
                        "https://music.youtube.com/".into(),
                        19,
                        glass,
                        ControlVariant::TextAction,
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(message("2. Вставьте данные своего сеанса.", cx))
                    .child(session.clone())
                    .when_some(youtube_error, |field, issue| {
                        field.child(
                            div()
                                .id("youtube-session-error")
                                .accessibility_id("youtube-session-error")
                                .role(Role::Alert)
                                .aria_label(issue.clone())
                                .text_size(px(13.))
                                .text_color(theme(cx).text())
                                .child(issue),
                        )
                    }),
            )
            .child(youtube_help(*help_open, status_rows + 2, cx)),
        Some(Form::Jellyfin {
            server,
            username,
            password,
        }) => form
            .child(server.clone())
            .when(side_by_side, |form| {
                form.child(
                    div()
                        .flex()
                        .gap(px(10.))
                        .child(div().flex_1().min_w_0().child(username.clone()))
                        .child(div().flex_1().min_w_0().child(password.clone())),
                )
            })
            .when(!side_by_side, |form| {
                form.children([username.clone(), password.clone()])
            }),
        None => browser_form(entry, form, glass, cx),
    }
}

fn primary_action(
    id: &'static str,
    label: &'static str,
    order: isize,
    glass: Option<Material>,
) -> Button {
    Button::new(id, label)
        .variant(Variant::Filled)
        .tab_order(order)
        .when_some(glass, |button, material| {
            ControlVariant::DialogAction.dialog_button(button, material)
        })
}

fn jellyfin_actions(
    server: &Entity<TextInput>,
    username: &Entity<TextInput>,
    password: &Entity<TextInput>,
    glass: Option<Material>,
    cx: &Context<MainView>,
) -> Div {
    actions_row()
        .child(
            primary_action("jellyfin-submit", "Войти", 23, glass).on_click({
                let (server, username, password) =
                    (server.clone(), username.clone(), password.clone());
                cx.listener(move |this, _, window, cx| {
                    this.submit_connection(
                        LoginInput::JellyfinPassword {
                            server: server.read(cx).text(),
                            username: username.read(cx).text(),
                            password: password.read(cx).text(),
                        },
                        window,
                        cx,
                    );
                })
            }),
        )
        .child(
            Button::new("jellyfin-quick", "Quick Connect")
                .variant(Variant::Subtle)
                .when_some(glass, |button, material| {
                    ControlVariant::TextAction.text_button(button, material)
                })
                .tab_order(24)
                .on_click({
                    let server = server.clone();
                    cx.listener(move |this, _, window, cx| {
                        this.submit_connection(
                            LoginInput::JellyfinQuickConnect {
                                server: server.read(cx).text(),
                            },
                            window,
                            cx,
                        );
                    })
                }),
        )
}

fn browser_form(
    entry: &Connection,
    form: Stateful<Div>,
    glass: Option<Material>,
    cx: &Context<MainView>,
) -> Stateful<Div> {
    let (text, label, input): (&str, &str, fn(&Window) -> LoginInput) = match entry.provider {
        ProviderId::Spotify => (
            "Подтвердите вход в Spotify в браузере.",
            "Войти в Spotify",
            |_| LoginInput::Spotify,
        ),
        ProviderId::YandexMusic => (
            "Получите код устройства и подтвердите вход на странице Яндекса.",
            "Получить код",
            |_| LoginInput::YandexDevice,
        ),
        ProviderId::SoundCloud => (
            "Войдите на странице SoundCloud в отдельном окне Mediadeka.",
            "Войти в SoundCloud",
            soundcloud_input,
        ),
        _ => return form,
    };
    if !matches!(entry.phase, Phase::Setup) {
        return form;
    }
    form.child(message(text, cx)).child(actions_row().child(
        primary_action("start-connection", label, 24, glass).on_click(cx.listener(
            move |this, _, window, cx| {
                this.submit_connection(input(window), window, cx);
            },
        )),
    ))
}

fn waiting_banner(
    prompt: Option<&LoginPrompt>,
    glass: Option<Material>,
    cx: &Context<MainView>,
) -> Div {
    let status = banner(false, glass, cx);
    match prompt {
        Some(LoginPrompt::Browser { url }) => status
            .child(status_message(
                "login-waiting",
                "Откройте страницу входа, подтвердите его и вернитесь в Mediadeka.".into(),
                cx,
            ))
            .child(actions_row().child(open_link(
                "authorize",
                "Открыть в браузере",
                url.clone(),
                24,
                glass,
                ControlVariant::DialogAction,
            ))),
        Some(LoginPrompt::Code { code, url }) => status
            .child(status_message(
                "login-waiting",
                "Введите этот код на странице подтверждения.".into(),
                cx,
            ))
            .child(
                div()
                    .id("device-code")
                    .accessibility_id("device-code")
                    .role(Role::Label)
                    .aria_value(code.clone())
                    .text_color(theme(cx).text())
                    .text_size(px(34.))
                    .font_weight(FontWeight(670.))
                    .child(code.clone()),
            )
            .child(
                actions_row()
                    .child(open_link(
                        "authorize",
                        "Открыть в браузере",
                        url.clone(),
                        24,
                        glass,
                        ControlVariant::DialogAction,
                    ))
                    .child(
                        Button::new("copy-code", "Скопировать код")
                            .variant(Variant::Default)
                            .when_some(glass, |button, material| {
                                ControlVariant::TextAction.text_button(button, material)
                            })
                            .tab_order(25)
                            .on_click({
                                let code = code.clone();
                                move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
                                }
                            }),
                    ),
            ),
        Some(LoginPrompt::Waiting { message }) => {
            status.child(status_message("login-waiting", message.clone(), cx))
        }
        None => status.child(status_message(
            "login-waiting",
            "Подключаемся к сервису…".into(),
            cx,
        )),
    }
}

fn youtube_help(expanded: bool, row: usize, cx: &Context<MainView>) -> Div {
    let palette = theme(cx);
    let hover = palette.surface_hover().hsla();
    div()
        .flex()
        .flex_col()
        .gap(px(5.))
        .child(
            button("youtube-help-toggle", "Как скопировать Cookie", 22)
                .self_start()
                .h(px(28.))
                .px(px(5.))
                .gap(px(6.))
                .text_color(palette.dimmed())
                .text_size(px(13.))
                .rounded(px(5.))
                .hover(move |style| style.bg(hover))
                .child("Как скопировать Cookie")
                .child(
                    Icon::new(if expanded {
                        IconName::ChevronUp
                    } else {
                        IconName::ChevronDown
                    })
                    .pixel_size(px(13.))
                    .color(palette.dimmed().hsla()),
                )
                .on_toggle(
                    expanded,
                    cx.listener(move |view, _, _, cx| {
                        if let Some(entry) = view.login.as_mut()
                            && let Some(Form::YouTube { help_open, .. }) = entry.form.as_mut()
                        {
                            *help_open = !*help_open;
                            if *help_open {
                                entry.scroll.scroll_to_item(row);
                            }
                            cx.notify();
                        }
                    }),
                ),
        )
        .when(expanded, |help| {
            help.child(
                div()
                    .text_size(px(13.))
                    .text_color(palette.dimmed())
                    .child("Cookie — заголовок браузера с данными о вашем входе."),
            )
            .child(youtube_steps(cx))
        })
}

fn youtube_steps(cx: &Context<MainView>) -> Div {
    let steps = [
        (
            "youtube-help-1",
            "Откройте инструменты разработчика клавишей F12 на сайте YouTube Music.",
        ),
        (
            "youtube-help-2",
            "Выберите «Сеть» (Network), обновите страницу и откройте запрос browse или next.",
        ),
        (
            "youtube-help-3",
            "В разделе «Заголовки запроса» скопируйте значение Cookie целиком, включая SAPISID или __Secure-3PAPISID.",
        ),
    ];
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .text_size(px(13.))
        .line_height(px(18.))
        .text_color(theme(cx).dimmed())
        .children(
            steps
                .into_iter()
                .enumerate()
                .map(|(index, (id, instruction))| {
                    div()
                        .id(id)
                        .accessibility_id(id)
                        .role(Role::Label)
                        .aria_value(format!("{}. {instruction}", index + 1))
                        .flex()
                        .gap(px(8.))
                        .child(div().flex_none().child(format!("{}.", index + 1)))
                        .child(div().min_w_0().child(instruction))
                }),
        )
}

fn banner(warning: bool, glass: Option<Material>, cx: &Context<MainView>) -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(7.))
        .p(px(12.))
        .rounded(px(6.))
        .bg(if warning {
            notice_surface(true)
        } else {
            rgb(ELEVATED).into()
        })
        .when_some(glass, |banner, glass| glass.group(banner))
        .when(warning, |banner| {
            banner.border_l_2().border_color(if glass.is_some() {
                theme(cx).primary().hsla()
            } else {
                notice_border(true)
            })
        })
}
