use gpui::{Context, Div, div, prelude::*, px};
use guise::{Button, Variant};

use crate::{
    state::HomeView,
    storage::Store,
    ui::{MainView, components::heading},
};

use super::{
    LikeUi, Provider,
    view::{destination_icon, display_name, notice, slug},
};

impl MainView {
    pub(in crate::ui) fn load_like_defaults(&mut self, cx: &mut Context<Self>) {
        self.likes.default_local();
        let task = cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async { Store::open_default().and_then(|store| store.like_defaults()) })
                .await;
            let _ = view.update(cx, |view, cx| {
                view.likes.settings_task = None;
                match result {
                    Ok(defaults) => view.likes.defaults = defaults,
                    Err(error) => view.likes.settings_error = Some(error.to_string()),
                }
                view.update_player_favorite(cx);
                cx.notify();
            });
        });
        self.likes.settings_task = Some(task);
    }

    pub(in crate::ui) fn like_settings(
        &self,
        home: &HomeView,
        interactive: bool,
        cx: &Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .w_full()
            .child(heading("Куда сохранять лайки по умолчанию", 18.))
            .child(
                div().flex().flex_wrap().gap(px(6.)).children(
                    std::iter::once(None)
                        .chain(home.accounts.iter().map(|account| Some(account.provider)))
                        .enumerate()
                        .map(|(index, provider)| {
                            let name = display_name(provider);
                            let checked = self.likes.defaults.contains(&provider);
                            let has_other_active = self.likes.defaults.iter().any(|other| {
                                *other != provider
                                    && other.is_none_or(|id| {
                                        home.accounts.iter().any(|account| account.provider == id)
                                    })
                            });
                            let enabled = interactive
                                && self.likes.settings_task.is_none()
                                && (!checked || has_other_active);
                            let selection = if checked {
                                "Выбрано"
                            } else {
                                "Не выбрано"
                            };
                            Button::new(format!("like-preference-{}", slug(provider)), name)
                                .variant(if checked {
                                    Variant::Light
                                } else {
                                    Variant::Default
                                })
                                .tab_order(710 + index as isize)
                                .left_section(destination_icon(provider))
                                .accessible_name(format!("{name} · {selection}"))
                                .disabled(!enabled)
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.toggle_like_default(provider, cx)
                                }))
                        }),
                ),
            )
            .when_some(self.likes.settings_error.as_ref(), |group, error| {
                group.child(notice("like-settings-error", error.clone()))
            })
    }

    fn toggle_like_default(&mut self, provider: Provider, cx: &mut Context<Self>) {
        if self.likes.settings_task.is_some() {
            return;
        }
        let mut defaults = self.likes.defaults.clone();
        if let Some(index) = defaults.iter().position(|value| *value == provider) {
            defaults.remove(index);
        } else {
            defaults.push(provider);
        }
        if defaults.is_empty() {
            return;
        }
        self.likes.settings_error = None;
        let task = cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Store::open_default()?.set_like_defaults(&defaults)?;
                    Ok::<_, crate::storage::StoreError>(defaults)
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                view.likes.settings_task = None;
                match result {
                    Ok(defaults) => view.likes.defaults = defaults,
                    Err(error) => view.likes.settings_error = Some(error.to_string()),
                }
                view.update_player_favorite(cx);
                cx.notify();
            });
        });
        self.likes.settings_task = Some(task);
        cx.notify();
    }
}

impl LikeUi {
    fn default_local(&mut self) {
        if self.defaults.is_empty() {
            self.defaults.push(None);
        }
    }
}
