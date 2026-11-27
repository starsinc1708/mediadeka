use gpui::{
    AppContext as _, Context, Div, Entity, FocusHandle, Role, Stateful, Toggled, Window, div,
    prelude::*, px,
};
use guise::{Button, ColorName, Size, Switch, TextInput, Variant, theme};

use crate::{
    integrations::{AccountStatus, Preferences, Service},
    ui::{
        MainView,
        components::{
            Activate, button,
            glass_control::{Material, Tone},
        },
        design_system::controls::ControlVariant,
        music::status,
    },
};

pub(in crate::ui) struct IntegrationControls {
    discord_id: Entity<TextInput>,
    lastfm_key: Entity<TextInput>,
    lastfm_secret: Entity<TextInput>,
    listenbrainz_token: Entity<TextInput>,
    connect_focus: [FocusHandle; Service::ALL.len()],
    prompt: Option<Service>,
}

type PreferenceToggle = fn(&mut Preferences) -> &mut bool;
const LINK_TAB_ORDERS: [isize; Service::ALL.len()] = [750, 760];

impl IntegrationControls {
    pub(in crate::ui) fn new(cx: &mut Context<MainView>) -> Self {
        let lastfm = LINK_TAB_ORDERS[Service::LastFm as usize];
        let listenbrainz = LINK_TAB_ORDERS[Service::ListenBrainz as usize];
        Self {
            discord_id: cx.new(|cx| {
                TextInput::new(cx)
                    .id("settings-discord-application-id")
                    .placeholder("ID приложения Discord")
                    .size(Size::Sm)
                    .max_length(20)
                    .tab_order(731)
            }),
            lastfm_key: secret_field("settings-lastfm-key", "Ключ API Last.fm", lastfm, cx),
            lastfm_secret: secret_field(
                "settings-lastfm-secret",
                "Секрет API Last.fm",
                lastfm + 1,
                cx,
            ),
            listenbrainz_token: secret_field(
                "settings-listenbrainz-token",
                "Токен ListenBrainz",
                listenbrainz,
                cx,
            ),
            connect_focus: LINK_TAB_ORDERS
                .map(|order| cx.focus_handle().tab_index(order + 2).tab_stop(true)),
            prompt: None,
        }
    }
}

fn secret_field(
    id: &'static str,
    name: &'static str,
    order: isize,
    cx: &mut Context<MainView>,
) -> Entity<TextInput> {
    cx.new(|cx| {
        TextInput::new(cx)
            .id(id)
            .accessible_name(name)
            .label(name)
            .size(Size::Sm)
            .password(true)
            .max_length(2_000)
            .tab_order(order)
    })
}

impl MainView {
    pub(super) fn listening_integration_settings(
        &self,
        interactive: bool,
        cx: &Context<Self>,
    ) -> Div {
        let integrations = self.integrations.read(cx);
        let preferences = integrations.preferences();
        let can_change_preferences = interactive && !integrations.preferences_busy();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(self.discord_settings(preferences, can_change_preferences, cx))
            .child(self.scrobbler_settings(Service::LastFm, interactive, cx))
            .child(self.scrobbler_settings(Service::ListenBrainz, interactive, cx))
            .when_some(integrations.preferences_error(), |section, error| {
                section.child(status("settings-integrations-error", error.to_owned()))
            })
    }

    fn discord_settings(
        &self,
        preferences: &Preferences,
        can_change_preferences: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let has_application_id = !preferences.discord_application_id.is_empty();
        let glass = self.glass_controls(cx);
        service_group("settings-discord", "Discord Rich Presence", cx)
            .child(self.integration_switch(
                ("settings-discord-enabled", "Discord Rich Presence", 730),
                preferences.discord_enabled,
                can_change_preferences && (preferences.discord_enabled || has_application_id),
                |preferences| &mut preferences.discord_enabled,
                cx,
            ))
            .when(has_application_id, |row| {
                row.child(status(
                    "settings-discord-saved-id",
                    format!("ID приложения · {}", preferences.discord_application_id),
                ))
            })
            .when(!has_application_id, |row| {
                row.child(status(
                    "settings-discord-missing-id",
                    "Сохраните ID приложения Discord, чтобы включить статус.",
                ))
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(150.))
                            .max_w(px(310.))
                            .child(self.integration_controls.discord_id.clone()),
                    )
                    .child(
                        action(
                            "settings-discord-save",
                            "Сохранить ID",
                            732,
                            can_change_preferences,
                            glass,
                            ControlVariant::SecondaryButton,
                        )
                        .on_click(cx.listener(|view, _, _, cx| view.save_discord_id(cx))),
                    ),
            )
            .child(self.integration_switch(
                ("settings-discord-paused", "Показывать статус на паузе", 733),
                preferences.discord_paused,
                can_change_preferences,
                |preferences| &mut preferences.discord_paused,
                cx,
            ))
            .child(self.integration_switch(
                ("settings-discord-private", "Скрывать детали записи", 734),
                preferences.discord_private,
                can_change_preferences,
                |preferences| &mut preferences.discord_private,
                cx,
            ))
    }

    fn scrobbler_settings(
        &self,
        service: Service,
        interactive: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let integrations = self.integrations.read(cx);
        let glass = self.glass_controls(cx);
        let preferences = integrations.preferences();
        let order = LINK_TAB_ORDERS[service as usize];
        let (prefix, name, enabled, change): (_, _, _, PreferenceToggle) = match service {
            Service::LastFm => (
                "settings-lastfm",
                "Last.fm",
                preferences.lastfm_enabled,
                |preferences| &mut preferences.lastfm_enabled,
            ),
            Service::ListenBrainz => (
                "settings-listenbrainz",
                "ListenBrainz",
                preferences.listenbrainz_enabled,
                |preferences| &mut preferences.listenbrainz_enabled,
            ),
        };
        let account = integrations.account(service);
        let connected = integrations.is_connected(service);
        let pending = matches!(account, AccountStatus::Loading | AccountStatus::Connecting);
        let prompt = self.integration_controls.prompt == Some(service) && !pending;
        let account_detail = match account {
            AccountStatus::Loading => "Обрабатываем подключение…".into(),
            AccountStatus::Disconnected => "Аккаунт не подключён".into(),
            AccountStatus::Connecting => match service {
                Service::LastFm => "Подтвердите подключение Last.fm в браузере…".into(),
                Service::ListenBrainz => "Проверяем токен ListenBrainz…".into(),
            },
            AccountStatus::Connected(name) if !name.is_empty() => format!("Аккаунт · {name}"),
            AccountStatus::Connected(_) => "Аккаунт подключён".into(),
            AccountStatus::Failed(error) => error.clone(),
        };
        let sending = match (enabled, connected) {
            (true, true) => "Отправка истории включена",
            (true, false) => "Отправка истории включится после подключения",
            (false, _) => "Отправка истории выключена",
        };
        let detail = format!("{account_detail} · {sending}");

        service_group(prefix, name, cx)
            .child(self.integration_switch(
                (
                    format!("{prefix}-enabled"),
                    format!("Скробблинг · {name}"),
                    order - 2,
                ),
                enabled,
                interactive && !integrations.preferences_busy(),
                change,
                cx,
            ))
            .child(
                div()
                    .id(format!("{prefix}-status"))
                    .accessibility_id(format!("{prefix}-status"))
                    .role(Role::Status)
                    .aria_label(detail.clone())
                    .text_color(theme(cx).dimmed())
                    .child(detail),
            )
            .when(prompt, |section| {
                section.child(self.integration_link_form(service))
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(8.))
                    .when(!pending, |actions| {
                        actions.child(
                            action(
                                format!("{prefix}-connect"),
                                if prompt || !connected {
                                    "Подключить"
                                } else {
                                    "Переподключить"
                                },
                                order + 2,
                                interactive,
                                glass,
                                ControlVariant::SecondaryButton,
                            )
                            .accessible_name(format!("Подключение · {name}"))
                            .when(interactive, |button| {
                                button.track_focus(
                                    &self.integration_controls.connect_focus[service as usize],
                                )
                            })
                            .on_click(cx.listener(
                                move |view, _, window, cx| {
                                    if view.integration_controls.prompt == Some(service) {
                                        view.submit_integration_link(service, cx);
                                    } else {
                                        view.open_integration_link(service, window, cx);
                                    }
                                },
                            )),
                        )
                    })
                    .when(connected && !pending && !prompt, |actions| {
                        actions.child(
                            action(
                                format!("{prefix}-disconnect"),
                                "Удалить подключение",
                                order + 3,
                                interactive,
                                glass,
                                ControlVariant::TextAction,
                            )
                            .accessible_name(format!("Удалить подключение · {name}"))
                            .on_click(cx.listener(
                                move |view, _, _, cx| {
                                    view.integrations
                                        .update(cx, |state, cx| state.disconnect(service, cx));
                                },
                            )),
                        )
                    })
                    .when(
                        prompt || matches!(account, AccountStatus::Connecting),
                        |actions| {
                            actions.child(
                                action(
                                    format!("{prefix}-cancel"),
                                    "Отмена",
                                    order + 3,
                                    interactive,
                                    glass,
                                    ControlVariant::TextAction,
                                )
                                .accessible_name(format!("Отменить подключение · {name}"))
                                .on_click(cx.listener(
                                    move |view, _, window, cx| {
                                        if view.integration_controls.prompt == Some(service) {
                                            view.clear_integration_secrets(cx);
                                        } else {
                                            view.integrations
                                                .update(cx, |state, cx| state.cancel(service, cx));
                                        }
                                        view.integration_controls.connect_focus[service as usize]
                                            .focus(window, cx);
                                    },
                                )),
                            )
                        },
                    ),
            )
    }

    fn integration_link_form(&self, service: Service) -> Div {
        let controls = &self.integration_controls;
        let fields = match service {
            Service::LastFm => [Some(&controls.lastfm_key), Some(&controls.lastfm_secret)],
            Service::ListenBrainz => [Some(&controls.listenbrainz_token), None],
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .max_w(px(310.))
            .children(fields.into_iter().flatten().cloned())
    }

    fn open_integration_link(
        &mut self,
        service: Service,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.clear_integration_secrets(cx);
        self.integration_controls.prompt = Some(service);
        let field = match service {
            Service::LastFm => &self.integration_controls.lastfm_key,
            Service::ListenBrainz => &self.integration_controls.listenbrainz_token,
        };
        field.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    pub(in crate::ui) fn clear_integration_secrets(&mut self, cx: &mut Context<Self>) {
        if self.integration_controls.prompt.take().is_none() {
            return;
        }
        for field in [
            &self.integration_controls.lastfm_key,
            &self.integration_controls.lastfm_secret,
            &self.integration_controls.listenbrainz_token,
        ] {
            field.update(cx, |input, cx| input.set_text("", cx));
        }
        cx.notify();
    }

    fn submit_integration_link(&mut self, service: Service, cx: &mut Context<Self>) {
        let controls = &self.integration_controls;
        match service {
            Service::LastFm => {
                let key = controls.lastfm_key.read(cx).text();
                let secret = controls.lastfm_secret.read(cx).text();
                self.clear_integration_secrets(cx);
                self.integrations
                    .update(cx, |state, cx| state.connect_lastfm(key, secret, cx));
            }
            Service::ListenBrainz => {
                let token = controls.listenbrainz_token.read(cx).text();
                self.clear_integration_secrets(cx);
                self.integrations
                    .update(cx, |state, cx| state.connect_listenbrainz(token, cx));
            }
        }
    }

    fn save_discord_id(&mut self, cx: &mut Context<Self>) {
        let id = self.integration_controls.discord_id.read(cx).text();
        let id = id.trim();
        if id.is_empty() {
            return;
        }
        self.integration_controls
            .discord_id
            .update(cx, |input, cx| input.set_text("", cx));
        self.integrations.update(cx, |state, cx| {
            let mut preferences = state.preferences().clone();
            preferences.discord_application_id = id.to_owned();
            state.set_preferences(preferences, cx);
        });
    }

    fn integration_switch(
        &self,
        (id, name, order): (
            impl Into<gpui::ElementId>,
            impl Into<gpui::SharedString>,
            isize,
        ),
        enabled: bool,
        interactive: bool,
        selection: PreferenceToggle,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let name = name.into();
        let glass = self.glass_controls(cx);
        button(id, name.clone(), order)
            .role(Role::Switch)
            .tab_stop(interactive)
            .aria_toggled(if enabled {
                Toggled::True
            } else {
                Toggled::False
            })
            .min_h(px(30.))
            .w_full()
            .max_w(px(320.))
            .justify_between()
            .gap(px(12.))
            .px(px(3.))
            .rounded(px(5.))
            .when_some(glass, |control, glass| {
                glass.paint_enabled(control, Tone::Quiet, interactive)
            })
            .child(name)
            .child(
                Switch::new("appearance")
                    .size(Size::Sm)
                    .color(glass.map_or_else(
                        || ColorName::Orange.into(),
                        crate::ui::design_system::controls::switch_color,
                    ))
                    .checked(enabled)
                    .disabled(!interactive),
            )
            .when(interactive, |control| {
                control.on_activate(cx.listener(move |view, _, _, cx| {
                    view.integrations.update(cx, |state, cx| {
                        let mut preferences = state.preferences().clone();
                        let selected = selection(&mut preferences);
                        *selected = !*selected;
                        state.set_preferences(preferences, cx);
                    });
                }))
            })
    }
}

#[cfg(feature = "capture")]
impl MainView {
    pub fn load_listening_integrations_for_capture(&self, cx: &mut Context<Self>) {
        self.integrations
            .update(cx, |state, cx| state.load_for_capture(cx));
    }

    pub fn reveal_listening_integrations_for_capture(
        &self,
        from_bottom: f32,
        cx: &mut Context<Self>,
    ) {
        self.scroll.set_offset(gpui::point(
            px(0.),
            (-self.scroll.max_offset().y + px(from_bottom)).min(px(0.)),
        ));
        cx.notify();
    }

    pub fn listening_integrations_busy_for_capture(&self, cx: &Context<Self>) -> bool {
        self.integrations.read(cx).preferences_busy()
    }

    pub fn enter_discord_id_for_capture(&self, id: &str, cx: &mut Context<Self>) {
        self.integration_controls
            .discord_id
            .update(cx, |field, cx| field.set_text(id, cx));
    }

    pub fn enter_scrobble_drafts_for_capture(&self, text: &str, cx: &mut Context<Self>) {
        let controls = &self.integration_controls;
        let fields = match controls.prompt {
            Some(Service::LastFm) => [Some(&controls.lastfm_key), Some(&controls.lastfm_secret)],
            Some(Service::ListenBrainz) => [Some(&controls.listenbrainz_token), None],
            None => return,
        };
        for field in fields.into_iter().flatten() {
            field.update(cx, |field, cx| field.set_text(text, cx));
        }
    }
}

fn service_group(id: &'static str, name: &'static str, cx: &Context<MainView>) -> Stateful<Div> {
    div()
        .id(id)
        .accessibility_id(id)
        .role(Role::Group)
        .aria_label(name)
        .flex()
        .flex_col()
        .gap(px(6.))
        .py(px(10.))
        .border_t_1()
        .border_color(theme(cx).border())
        .text_size(px(13.))
}

fn action(
    id: impl Into<gpui::ElementId>,
    name: &'static str,
    order: isize,
    interactive: bool,
    glass: Option<Material>,
    variant: ControlVariant,
) -> Button {
    Button::new(id, name)
        .tab_order(order)
        .variant(Variant::Default)
        .when_some(glass, |button, material| {
            variant.text_button(button, material)
        })
        .size(Size::Xs)
        .disabled(!interactive)
}
