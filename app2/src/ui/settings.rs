mod appearance;
mod artwork_cache;
mod catalog_cache;
mod genius;
mod integrations;
#[cfg(feature = "capture")]
mod shader_debug;
mod sound_downloads;
mod visual;

use std::{cell::Cell, rc::Rc};

use gpui::{
    Context, Div, FontWeight, IntoElement, Pixels, Role, Stateful, Window, canvas, div, prelude::*,
    px, rgb,
};
use guise::{Button, Icon, IconName, Variant, theme};

use crate::{
    providers::{Profile, ProviderId},
    state::{ActiveSession, HomeView, Intent, Screen},
};

use super::{
    MainView,
    assets::{provider_index, provider_mark},
    components::{
        Activate, button, content_state::empty_state, glass_control::Tone,
        scrollbar::set_handle_offset,
    },
    design_system::controls::ControlVariant,
    fluid_glass::tokens::{self, radius, size, space, text},
    theme::{ELEVATED, SETTINGS_ERROR_BACKGROUND},
};
#[cfg(feature = "capture")]
pub(super) use appearance::menus::Samples as EditorialMenuSamples;
pub(super) use genius::GeniusSettings;
pub(super) use integrations::IntegrationControls;
#[cfg(feature = "capture")]
pub(super) use shader_debug::ShaderDebug;
pub(super) use sound_downloads::SoundDownloadSettings;

const SETTINGS_SECTIONS: [(&str, &str, isize); 4] = [
    ("settings-connections", "Подключения", 30),
    ("settings-library", "Библиотека и тексты", 699),
    ("settings-integrations", "Интеграции", 729),
    ("settings-appearance", "Оформление", 759),
];
type SectionOffset = Rc<Cell<Option<Pixels>>>;

impl MainView {
    #[cfg(feature = "capture")]
    pub fn enable_settings_diagnostics_for_capture(&mut self, cx: &mut Context<Self>) {
        self.settings_diagnostics = true;
        cx.notify();
    }

    pub(super) fn connections_settings(
        &self,
        home: &HomeView,
        busy: bool,
        compact: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let interactive = !busy && !self.is_settings_login() && !self.soloist.is_open();
        let glass = self.glass_controls(cx);
        let fluid = glass.is_some();
        let palette = theme(cx);
        let gap = px(match (fluid, compact) {
            (true, true) => space::LG,
            (true, false) => space::SECTION,
            (false, true) => 20.,
            (false, false) => 38.,
        });
        let anchors: [SectionOffset; 4] = std::array::from_fn(|_| SectionOffset::default());
        let section_size = match (fluid, compact) {
            (true, true) => 20.,
            (true, false) => text::SECTION_TITLE,
            (false, true) => 19.,
            (false, false) => 24.,
        };
        let title_size = match (fluid, compact) {
            (true, true) => tokens::text::PAGE_TITLE_COMPACT,
            (true, false) => tokens::text::PAGE_TITLE,
            (false, true) => 29.,
            (false, false) => 48.,
        };
        let title = div()
            .id("settings-title")
            .accessibility_id("settings-title")
            .role(Role::Heading)
            .aria_label("Настройки")
            .text_size(px(title_size))
            .when(fluid, |title| title.line_height(px(title_size * 1.12)))
            .font_weight(if fluid {
                FontWeight::SEMIBOLD
            } else {
                FontWeight(670.)
            })
            .child("Настройки");
        div()
            .w_full()
            .flex()
            .when(!fluid, |page| page.justify_center())
            .child(
                div()
                    .id("settings-page")
                    .accessibility_id("settings-page")
                    .role(Role::Group)
                    .aria_label("Настройки")
                    .w_full()
                    .max_w(px(if fluid { 1180. } else { 1080. }))
                    .flex()
                    .flex_col()
                    .gap(if fluid && !compact {
                        px(space::BLOCK)
                    } else {
                        gap
                    })
                    .child(
                        div()
                            .flex()
                            .gap(gap)
                            .when(!fluid, |header| header.flex_col())
                            .when(fluid, |header| {
                                header
                                    .flex_wrap()
                                    .items_end()
                                    .justify_between()
                                    .pb(px(if compact { 8. } else { 14. }))
                                    .border_b_1()
                                    .border_color(palette.border())
                            })
                            .child(title)
                            .child(self.settings_shortcuts(&anchors, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(gap)
                            .items_start()
                            .when(compact, |columns| columns.flex_col())
                            .child(self.connection_services(
                                home,
                                interactive,
                                compact,
                                self.settings_section_heading(0, section_size, &anchors[0], cx),
                                cx,
                            ))
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(if compact { 16. } else { 24. }))
                                    .when(!compact, |column| column.flex_1())
                                    .child(self.settings_section_heading(
                                        1,
                                        section_size,
                                        &anchors[1],
                                        cx,
                                    ))
                                    .child(self.sound_download_settings(interactive, cx))
                                    .child(artwork_cache::section(interactive, glass, cx))
                                    .child(catalog_cache::section(interactive, glass, cx))
                                    .child(self.like_settings(home, interactive, cx))
                                    .child(self.genius_settings(interactive, cx)),
                            ),
                    )
                    .child(
                        div()
                            .border_t_1()
                            .border_color(palette.border())
                            .pt(px(if compact { 14. } else { 24. }))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(self.settings_section_heading(2, section_size, &anchors[2], cx))
                            .child(self.listening_integration_settings(interactive, cx)),
                    )
                    .child(
                        div()
                            .border_t_1()
                            .border_color(palette.border())
                            .pt(px(if compact { 14. } else { 24. }))
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .child(self.settings_section_heading(3, section_size, &anchors[3], cx))
                            .child(self.appearance_settings(compact, window, cx)),
                    ),
            )
    }

    fn settings_shortcuts(
        &self,
        anchors: &[SectionOffset; 4],
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let glass = self.glass_controls(cx);
        let palette = theme(cx);
        div()
            .id("settings-shortcuts")
            .accessibility_id("settings-shortcuts")
            .role(Role::Navigation)
            .aria_label("Разделы настроек")
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .when(glass.is_none(), |links| {
                links
                    .pb(px(10.))
                    .border_b_1()
                    .border_color(palette.border())
            })
            .children(
                SETTINGS_SECTIONS
                    .into_iter()
                    .enumerate()
                    .map(|(index, (id, title, _))| {
                        let anchor = anchors[index].clone();
                        let focus = self.settings_sections[index].clone();
                        button(
                            format!("{id}-shortcut"),
                            format!("Перейти к разделу «{title}»"),
                            20 + index as isize,
                        )
                        .h(px(if glass.is_some() { size::SM } else { 30. }))
                        .px(px(space::SM))
                        .gap(px(space::XS))
                        .rounded(px(radius::CONTROL))
                        .text_size(px(text::METADATA))
                        .when_some(glass, |control, glass| glass.paint(control, Tone::Quiet))
                        .when(glass.is_none(), |control| {
                            control.hover(|style| style.bg(rgb(ELEVATED)))
                        })
                        .child(title)
                        .child(
                            Icon::new(IconName::ArrowDown)
                                .pixel_size(px(text::METADATA))
                                .color(palette.dimmed().hsla()),
                        )
                        .on_activate(cx.listener(
                            move |view, _, window, cx| {
                                let Some(offset) = anchor.get() else {
                                    return;
                                };
                                view.stop_workspace_scrolling();
                                set_handle_offset(&view.scroll, offset);
                                window.focus(&focus, cx);
                                cx.notify();
                            },
                        ))
                    }),
            )
    }

    fn settings_section_heading(
        &self,
        index: usize,
        size: f32,
        anchor: &SectionOffset,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let (id, title, order) = SETTINGS_SECTIONS[index];
        let anchor = anchor.clone();
        let scroll = self.scroll.clone();
        let focus_ink = theme(cx).primary();
        div()
            .id(id)
            .accessibility_id(id)
            .role(Role::Heading)
            .aria_label(title)
            .track_focus(&self.settings_sections[index])
            .tab_index(order)
            .tab_stop(false)
            .relative()
            .focus_visible(move |style| style.text_color(focus_ink))
            .text_size(px(size))
            .font_weight(FontWeight::SEMIBOLD)
            .child(
                canvas(
                    move |bounds, _, _| {
                        anchor.set(Some(
                            bounds.top() - scroll.bounds().top() - scroll.offset().y,
                        ));
                    },
                    |_, (), _, _| {},
                )
                .absolute()
                .top_0()
                .left_0(),
            )
            .child(title)
    }

    fn connection_services(
        &self,
        home: &HomeView,
        interactive: bool,
        compact: bool,
        heading: Stateful<Div>,
        cx: &Context<Self>,
    ) -> Div {
        let glass = self.glass_controls(cx);
        div()
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(8.))
            .when(!compact, |column| column.flex_1())
            .child(heading)
            .when(glass.is_some() && home.accounts.is_empty(), |column| {
                column.child(empty_state(
                    "settings-accounts-empty",
                    "Аккаунты не подключены",
                    None,
                    IconName::Plug,
                    cx,
                ))
            })
            .when_some(home.switch_issue.as_ref(), |body, issue| {
                body.child(
                    div()
                        .id("settings-connection-error")
                        .accessibility_id("settings-connection-error")
                        .role(Role::Alert)
                        .px(px(12.))
                        .py(px(9.))
                        .rounded(px(6.))
                        .bg(rgb(SETTINGS_ERROR_BACKGROUND))
                        .when_some(glass, |alert, glass| {
                            let ink = if glass.contrast() {
                                tokens::contrast::PRIMARY
                            } else {
                                tokens::ERROR
                            };
                            glass.group(alert.border_1()).text_color(rgb(ink))
                        })
                        .text_size(px(text::METADATA))
                        .child(issue.to_string()),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .children(ProviderId::ALL.into_iter().enumerate().flat_map(
                        |(index, provider)| {
                            let profile = home
                                .accounts
                                .iter()
                                .find(|account| account.provider == provider)
                                .map(|account| &account.profile);
                            let row = self
                                .service_row(index, provider, profile, interactive, compact, cx)
                                .into_any_element();
                            let soloist = (provider == ProviderId::Spotify)
                                .then(|| self.soloist_settings(interactive, cx).into_any_element());
                            [Some(row), soloist].into_iter().flatten()
                        },
                    )),
            )
    }

    fn service_row(
        &self,
        index: usize,
        provider: ProviderId,
        profile: Option<&Profile>,
        interactive: bool,
        compact: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let connected = profile.is_some();
        let palette = theme(cx);
        let glass = self.glass_controls(cx);
        let fluid = glass.is_some();
        let is_primary = matches!(self.state.home_view().map(|(view, _)| &view.session),
            Some(ActiveSession::Account { provider: current, .. }) if *current == provider);
        let operation = if connected { "sign-out" } else { "sign-in" };
        let label = if connected {
            "Выйти"
        } else {
            "Войти"
        };
        let name = provider.display_name();
        let id = format!("settings-{operation}-{}", provider.as_str());
        let control = Button::new(id, label)
            .variant(if connected {
                Variant::Subtle
            } else if fluid {
                Variant::Default
            } else {
                Variant::Light
            })
            .when_some(glass, |button, material| {
                ControlVariant::TextAction.text_button(button, material)
            })
            .tab_order(31 + index as isize)
            .accessible_name(format!("{label} · {name}"))
            .size(guise::Size::Xs)
            .full_width(true);
        div()
            .id(("settings-provider", index))
            .accessibility_id(format!("settings-provider-{}", provider.as_str()))
            .role(Role::Group)
            .aria_label(name)
            .min_h(px(if compact {
                45.
            } else if fluid {
                62.
            } else {
                56.
            }))
            .flex()
            .gap(px(12.))
            .items_center()
            .border_t_1()
            .border_color(palette.border())
            .child(provider_mark(provider, if compact { 20. } else { 24. }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(if fluid { text::CARD_TITLE } else { 14. }))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(palette.text())
                            .child(name),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(if fluid { text::CAPTION } else { 12. }))
                            .text_color(if is_primary {
                                palette.primary()
                            } else {
                                palette.dimmed()
                            })
                            .child(match profile {
                                Some(profile) if is_primary => {
                                    format!("{} · основной", profile.name)
                                }
                                Some(profile) => profile.name.clone(),
                                None => "Не подключён".to_owned(),
                            }),
                    ),
            )
            .children(profile.filter(|_| !is_primary).map(|profile| {
                let profile_id = profile.id.clone();
                Button::new(
                    format!("settings-primary-{}", provider.as_str()),
                    "Сделать основной",
                )
                .variant(Variant::Subtle)
                .when_some(glass, |button, material| {
                    ControlVariant::TextAction.text_button(button, material)
                })
                .accessible_name(format!("Сделать основной · {name}"))
                .size(guise::Size::Xs)
                .tab_order(36 + index as isize)
                .disabled(!interactive)
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.run_account(provider, profile_id.clone(), cx)
                }))
            }))
            .child(div().w(px(86.)).flex_none().child(if interactive {
                control
                    .track_focus(&self.provider_focus[provider_index(provider)])
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if connected {
                            this.run(Intent::Disconnect(provider), cx);
                        } else {
                            this.open_settings_connection(provider, window, cx);
                        }
                    }))
            } else {
                control.disabled(true)
            }))
    }

    pub(super) fn disconnecting(&self) -> bool {
        matches!(self.state.screen(), Screen::Pending(Intent::Disconnect(_)))
    }

    pub(super) fn tab_focus(&mut self, reverse: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.listening.mode().is_some() {
            if reverse {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            return;
        }
        let modal = self
            .soloist
            .modal_focus()
            .or_else(|| self.is_settings_login().then_some(&self.connection_focus))
            .or_else(|| self.like_menu_focus())
            .cloned();
        if modal.is_none() && self.cycle_playlist_menu(reverse, window, cx) {
            return;
        }
        let mut first_outside = None;
        loop {
            if reverse {
                window.focus_prev(cx);
            } else {
                window.focus_next(cx);
            }
            let Some(modal) = &modal else {
                return;
            };
            if modal.contains_focused(window, cx) {
                return;
            }
            let focused = window.focused(cx);
            if focused.is_none() || focused == first_outside {
                window.focus(modal, cx);
                return;
            }
            if first_outside.is_none() {
                first_outside = focused;
            }
        }
    }
}
