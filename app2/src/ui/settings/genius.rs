use gpui::{AppContext as _, Context, Div, Entity, Task, div, prelude::*, px};
use guise::{Button, Size, TextInput, Variant, theme};

use crate::{
    music::lyrics::genius,
    storage::Store,
    ui::{MainView, components::heading, design_system::controls::ControlVariant, music::status},
};

pub(in crate::ui) struct GeniusSettings {
    token: Entity<TextInput>,
    connected: Option<bool>,
    error: Option<String>,
    task: Option<Task<()>>,
}

enum Work {
    Load,
    Connect(String),
    Disconnect,
}

impl GeniusSettings {
    pub(in crate::ui) fn new(cx: &mut Context<MainView>) -> Self {
        let token = cx.new(|cx| {
            TextInput::new(cx)
                .id("settings-genius-token")
                .placeholder("Client Access Token")
                .password(true)
                .max_length(2_000)
                .tab_order(720)
        });
        Self {
            token,
            connected: None,
            error: None,
            task: None,
        }
    }
}

impl Work {
    fn run(self) -> Result<bool, String> {
        let store = Store::open_default().map_err(|error| error.to_string())?;
        match self {
            Self::Load => store.genius_connected().map_err(|error| error.to_string()),
            Self::Connect(token) => {
                genius::connect(&store, &token)?;
                Ok(true)
            }
            Self::Disconnect => {
                store
                    .set_genius_token(None)
                    .map_err(|error| error.to_string())?;
                Ok(false)
            }
        }
    }
}

impl MainView {
    pub(in crate::ui) fn load_genius_settings(&mut self, cx: &mut Context<Self>) {
        if self.genius_settings.connected.is_none() {
            self.run_genius_settings(Work::Load, cx);
        }
    }

    fn submit_genius_settings(&mut self, cx: &mut Context<Self>) {
        let token = self.genius_settings.token.read(cx).text();
        if token.trim().is_empty() {
            self.genius_settings.error = Some("Введите Client Access Token Genius.".into());
            cx.notify();
            return;
        }
        self.genius_settings
            .token
            .update(cx, |input, cx| input.set_text("", cx));
        self.run_genius_settings(Work::Connect(token), cx);
    }

    fn run_genius_settings(&mut self, work: Work, cx: &mut Context<Self>) {
        let settings = &mut self.genius_settings;
        if settings.task.is_some() {
            return;
        }
        let changed = !matches!(work, Work::Load);
        settings.error = None;
        settings.task = Some(cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work.run() })
                .await;
            let _ = view.update(cx, |view, cx| {
                let settings = &mut view.genius_settings;
                settings.task = None;
                match result {
                    Ok(connected) => {
                        settings.connected = Some(connected);
                        if changed {
                            view.genius_settings_changed(cx);
                        }
                    }
                    Err(error) => settings.error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn genius_settings(&self, interactive: bool, cx: &Context<Self>) -> Div {
        let settings = &self.genius_settings;
        let busy = settings.task.is_some();
        let glass = self.glass_controls(cx);
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(heading("Genius", 18.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme(cx).dimmed())
                    .child(if settings.connected == Some(true) {
                        "Подключён · пояснения к тексту и сведения об авторах"
                    } else {
                        "Пояснения к тексту и сведения об авторах"
                    }),
            )
            .when_some(settings.connected, |group, connected| {
                group
                    .child(div().w_full().max_w(px(360.)).child(settings.token.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .child(
                                Button::new(
                                    "settings-genius-connect",
                                    if connected {
                                        "Заменить токен"
                                    } else {
                                        "Подключить"
                                    },
                                )
                                .size(Size::Xs)
                                .variant(Variant::Default)
                                .when_some(glass, |button, material| {
                                    ControlVariant::SecondaryButton.text_button(button, material)
                                })
                                .tab_order(721)
                                .disabled(!interactive || busy)
                                .on_click(cx.listener(
                                    |view, _, _, cx| {
                                        view.submit_genius_settings(cx);
                                    },
                                )),
                            )
                            .when(connected, |actions| {
                                actions.child(
                                    Button::new("settings-genius-disconnect", "Отключить")
                                        .size(Size::Xs)
                                        .variant(Variant::Default)
                                        .when_some(glass, |button, material| {
                                            ControlVariant::TextAction.text_button(button, material)
                                        })
                                        .tab_order(722)
                                        .disabled(!interactive || busy)
                                        .on_click(cx.listener(|view, _, _, cx| {
                                            view.genius_settings.token.update(cx, |input, cx| {
                                                input.set_text("", cx);
                                            });
                                            view.run_genius_settings(Work::Disconnect, cx);
                                        })),
                                )
                            }),
                    )
            })
            .when(busy, |group| {
                group.child(status(
                    "settings-genius-loading",
                    "Проверяем подключение Genius…",
                ))
            })
            .when_some(settings.error.as_ref(), |group, error| {
                group
                    .child(status("settings-genius-error", error.clone()))
                    .when(settings.connected.is_none(), |group| {
                        group.child(
                            Button::new("settings-genius-retry", "Повторить")
                                .size(Size::Xs)
                                .variant(Variant::Default)
                                .when_some(glass, |button, material| {
                                    ControlVariant::TextAction.text_button(button, material)
                                })
                                .tab_order(720)
                                .disabled(!interactive || busy)
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.load_genius_settings(cx);
                                })),
                        )
                    })
            })
    }
}
