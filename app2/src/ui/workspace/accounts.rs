//! The connections card at the bottom of the workspace sidebar.

use gpui::{Context, Div, div, prelude::*, px, rgb};
use guise::{Icon, IconName};

use crate::{
    providers::{ErrorKind, ProviderId},
    state::{ActiveSession, HomeContent, HomeView, Issue},
    ui::{
        MainView,
        assets::provider_mark,
        components::{Activate, button},
        design_system::controls::{ControlState, ControlVariant},
        fluid_glass::tokens::{self as fluid, size, space, text},
        labels::{connection_count, counted},
        theme::{SOURCE_CHIP_BACKGROUND, SOURCE_ROW_HOVER, playlist, secondary},
    },
};

use super::Page;

const MARKS: usize = 3;

impl MainView {
    pub(super) fn accounts_card(
        &self,
        home: Option<&HomeView>,
        compact: bool,
        cx: &Context<Self>,
    ) -> Div {
        let Some(home) = home else {
            return div();
        };
        let accounts = home.accounts.as_slice();
        let glass = self.uses_fluid_glass();
        let guest = matches!(home.session, ActiveSession::Guest);
        let full_text = if guest {
            "Гостевой вход".to_owned()
        } else {
            connection_count(accounts.len())
        };
        let state = if glass { active_connection(home) } else { None };
        let name = if let Some((provider, status, _)) = state {
            format!(
                "Подключения · {full_text} · {}: {status}",
                provider.display_name()
            )
        } else {
            format!("Подключения · {full_text}")
        };
        let text = if guest {
            full_text
        } else {
            counted(accounts.len(), "сервис", "сервиса", "сервисов")
        };
        let card = button("home-accounts", name.clone(), 18)
            .h(px(if glass {
                size::SM
            } else {
                playlist::metrics::CONTROL
            }))
            .flex_none()
            .gap(px(space::XS))
            .px(px(if compact { 0. } else { space::SM }))
            .rounded(px(playlist::RADIUS))
            .when(!glass, |card| card.bg(rgb(SOURCE_CHIP_BACKGROUND)))
            .when(!glass, |card| {
                card.hover(|style| style.bg(rgb(SOURCE_ROW_HOVER)))
            })
            .when_some(self.glass_controls(cx), |card, material| {
                ControlVariant::TextAction.paint(card, material, ControlState::default())
            })
            .when(compact || state.is_some(), |card| {
                card.tooltip(guise::tooltip(name))
            })
            .when(compact, |card| card.justify_center())
            .child(
                Icon::new(IconName::Plug)
                    .pixel_size(px(playlist::metrics::GLYPH))
                    .color(secondary()),
            )
            .when(compact, |card| {
                card.children(state.map(|(_, _, color)| status_dot(color)))
            })
            .when(!compact, |card| {
                card.children(accounts.iter().take(MARKS).map(|account| {
                    div()
                        .relative()
                        .flex_none()
                        .child(provider_mark(account.provider, 16.))
                        .children(
                            state
                                .filter(|(provider, _, _)| *provider == account.provider)
                                .map(|(_, _, color)| {
                                    status_dot(color).absolute().right_0().bottom_0()
                                }),
                        )
                }))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(text::CAPTION))
                        .text_color(secondary())
                        .child(text),
                )
            })
            .on_activate(
                cx.listener(|this, _, window, cx| this.show_music_page(Page::Settings, window, cx)),
            );
        div().flex_none().child(card)
    }
}

fn active_connection(home: &HomeView) -> Option<(ProviderId, &'static str, u32)> {
    let ActiveSession::Account { provider, .. } = home.session else {
        return None;
    };
    let (status, color) = match &home.content {
        HomeContent::Loading => ("Синхронизация", fluid::WARNING),
        HomeContent::Fresh(_) => ("Подключено", fluid::SUCCESS),
        HomeContent::Partial { issue, .. }
        | HomeContent::Cached { issue, .. }
        | HomeContent::Empty(issue) => match issue {
            Issue::Provider(error) if error.needs_sign_in() => ("Требуется вход", fluid::ERROR),
            Issue::Provider(error)
                if matches!(error.kind, ErrorKind::Network | ErrorKind::Unavailable) =>
            {
                ("Недоступно", fluid::WARNING)
            }
            _ => ("Ошибка", fluid::ERROR),
        },
    };
    Some((provider, status, color))
}

fn status_dot(color: u32) -> Div {
    div().size(px(6.)).flex_none().rounded_full().bg(rgb(color))
}
