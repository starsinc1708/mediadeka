//! The platform filters above the home feed.

use gpui::{
    AnyElement, Context, Div, FontWeight, IntoElement, Role, SharedString, Stateful, div,
    prelude::*, px, rgb,
};
use guise::{Button, Chip, Icon, IconName, Size, Variant};

use crate::{
    providers::ProviderId,
    state::{ActiveSession, HomeView},
    ui::{
        assets::provider_mark,
        components::{Activate, button, paint_section_tab, section_tab},
        design_system::{
            controls::{ControlState, ControlVariant},
            icons::{Glyph, animated_glyph},
        },
        fluid_glass::tokens::{SECONDARY, contrast, size, space, text},
        theme::{ELEVATED, SURFACE, secondary, surface_border},
    },
};

use super::{FeedSource, MainView, Page};

impl MainView {
    pub(super) fn source_row(
        &self,
        home: &HomeView,
        active: &FeedSource,
        sources: &[FeedSource],
        refreshing: bool,
        heading: Option<Stateful<Div>>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let open = self.home.sources_open;
        let scope = self.home.filter.map_or("Все", ProviderId::display_name);
        let material = self.glass_controls(cx);
        let show_refresh = material.is_some()
            && sources.iter().any(|source| {
                self.home
                    .filter
                    .is_none_or(|provider| source.provider == provider)
                    && {
                        let feed = self.home.feeds.get(source, active, home, refreshing);
                        feed.refreshing && feed.snapshot().is_some()
                    }
            });
        const TOGGLE: &str = "home-source-toggle";
        let toggle = button(TOGGLE, format!("Площадки · {scope}"), 20).self_start();
        let toggle = if let Some(material) = material {
            let detail = rgb(if material.contrast() {
                contrast::SECONDARY
            } else {
                SECONDARY
            });
            ControlVariant::SelectTrigger
                .paint(
                    toggle,
                    material,
                    ControlState {
                        expanded: open,
                        ..ControlState::default()
                    },
                )
                .h(px(size::SM))
                .px(px(space::SM))
                .gap(px(space::SM))
                .text_size(px(text::METADATA))
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Площадки"))
                .child(div().text_color(detail).child(scope))
                .child(animated_glyph(
                    TOGGLE,
                    Glyph::ChevronDown,
                    open,
                    14.,
                    None,
                    cx,
                ))
        } else {
            toggle
                .h(px(34.))
                .px(px(9.))
                .gap(px(8.))
                .rounded(px(6.))
                .bg(rgb(SURFACE))
                .border_color(surface_border())
                .hover(|style| style.bg(rgb(ELEVATED)))
                .text_size(px(13.))
                .child("Площадки")
                .child(div().text_color(secondary()).child(scope))
                .child(
                    Icon::new(if open {
                        IconName::ChevronUp
                    } else {
                        IconName::ChevronDown
                    })
                    .pixel_size(px(14.))
                    .color(secondary()),
                )
        };
        let toggle = toggle.on_toggle(
            open,
            cx.listener(|view, _, _, cx| {
                view.home.sources_open = !view.home.sources_open;
                cx.notify();
            }),
        );
        div()
            .id("home-sources")
            .accessibility_id("home-sources")
            .role(Role::Group)
            .aria_label("Площадки")
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(if let Some(heading) = heading {
                div()
                    .w_full()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap(px(12.))
                    .child(heading)
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .when(show_refresh, |row| {
                                row.child(crate::ui::components::content_state::refresh_indicator(
                                    "home-refreshing",
                                    "Обновляем подборку…",
                                    cx,
                                ))
                            })
                            .child(toggle.flex_none()),
                    )
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(toggle)
                    .when(show_refresh, |row| {
                        row.child(crate::ui::components::content_state::refresh_indicator(
                            "home-refreshing",
                            "Обновляем подборку…",
                            cx,
                        ))
                    })
                    .into_any_element()
            })
            .when(open, |group| {
                group.child(self.source_options(home, active, sources, refreshing, cx))
            })
    }

    fn source_options(
        &self,
        home: &HomeView,
        active: &FeedSource,
        sources: &[FeedSource],
        refreshing: bool,
        cx: &Context<Self>,
    ) -> Div {
        let available = ProviderId::ALL
            .into_iter()
            .filter_map(|provider| sources.iter().find(|source| source.provider == provider));
        let first_connect = 22 + available.clone().count() as isize;
        let can_refresh = sources.iter().any(|source| {
            self.home
                .filter
                .is_none_or(|provider| source.provider == provider)
                && !self
                    .home
                    .feeds
                    .get(source, active, home, refreshing)
                    .refreshing
        });
        let glass = self.glass_controls(cx);
        let all_selected = self.home.filter.is_none();
        let all = if glass.is_some() {
            section_tab("home-source-all", "Все", all_selected, 21, glass)
                .text_size(px(text::METADATA))
                .on_activate(cx.listener(|view, _, _, cx| view.filter_home(None, cx)))
                .into_any_element()
        } else {
            Chip::new("home-source-all", "Все")
                .as_tab()
                .checked(all_selected)
                .size(Size::Sm)
                .tab_order(21)
                .on_change(cx.listener(|view, _, _, cx| view.filter_home(None, cx)))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .id("home-source-filters")
                    .accessibility_id("home-source-filters")
                    .role(Role::TabList)
                    .aria_label("Фильтр площадок")
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(all)
                    .children(available.enumerate().map(|(index, source)| {
                        self.source_chip(home, active, source, refreshing, 22 + index as isize, cx)
                    })),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .children(
                        ProviderId::ALL
                            .into_iter()
                            .filter(|provider| {
                                !sources.iter().any(|source| source.provider == *provider)
                            })
                            .enumerate()
                            .map(|(index, provider)| {
                                let id = SharedString::from(format!(
                                    "home-connect-{}",
                                    provider.as_str()
                                ));
                                let label = format!("{} · Нужен вход", provider.display_name());
                                Button::new(id, label)
                                    .variant(Variant::Subtle)
                                    .left_section(provider_mark(provider, 16.).into_any_element())
                                    .size(Size::Xs)
                                    .tab_order(first_connect + index as isize)
                                    .when_some(glass, |button, material| {
                                        ControlVariant::TextAction.text_button(button, material)
                                    })
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        view.open_provider_recovery(provider, window, cx);
                                    }))
                            }),
                    )
                    .child(
                        Button::new("refresh", "Обновить подборки")
                            .variant(Variant::Subtle)
                            .size(Size::Xs)
                            .tab_order(27)
                            .disabled(!can_refresh)
                            .when_some(glass, |button, material| {
                                ControlVariant::TextAction.text_button(button, material)
                            })
                            .on_click(cx.listener(|view, _, _, cx| view.refresh_visible_home(cx))),
                    )
                    .child(
                        Button::new("home-add-account", "Подключения")
                            .variant(Variant::Subtle)
                            .size(Size::Xs)
                            .tab_order(28)
                            .when_some(glass, |button, material| {
                                ControlVariant::TextAction.text_button(button, material)
                            })
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.show_music_page(Page::Settings, window, cx);
                            })),
                    ),
            )
    }

    fn source_chip(
        &self,
        home: &HomeView,
        active: &FeedSource,
        source: &FeedSource,
        refreshing: bool,
        order: isize,
        cx: &Context<Self>,
    ) -> AnyElement {
        let provider = source.provider;
        let status = if source.profile.is_none() {
            "Гостевая витрина"
        } else if source == active
            && matches!(&home.session, ActiveSession::Account { capabilities, .. } if !capabilities.home)
        {
            "Подборка недоступна"
        } else {
            self.home
                .feeds
                .get(source, active, home, refreshing)
                .status()
        };
        let profile_name = source.profile.as_ref().and_then(|profile| {
            home.accounts
                .iter()
                .find(|account| account.provider == provider && &account.profile.id == profile)
                .map(|account| account.profile.name.as_str())
        });
        let id = if source.profile.is_some() {
            format!("home-source-account-{}", provider.as_str())
        } else {
            "home-source-guest".to_owned()
        };
        let selected = self.home.filter == Some(provider);
        let label = format!("{} · {status}", provider.display_name());
        let name = profile_name.map_or(label.clone(), |name| format!("{label} · {name}"));
        if let Some(material) = self.glass_controls(cx) {
            paint_section_tab(
                button(id, name, order)
                    .role(Role::Tab)
                    .aria_selected(selected)
                    .h(px(size::SM))
                    .gap(px(space::XS))
                    .px(px(space::SM))
                    .text_size(px(text::METADATA)),
                material,
                selected,
            )
            .child(provider_mark(provider, 16.))
            .child(label)
            .on_activate(cx.listener(move |view, _, _, cx| view.toggle_home_filter(provider, cx)))
            .into_any_element()
        } else {
            Chip::new(SharedString::from(id), label)
                .as_tab()
                .checked(selected)
                .accessible_name(name)
                .size(Size::Sm)
                .tab_order(order)
                .left_section(provider_mark(provider, 16.).into_any_element())
                .on_change(cx.listener(move |view, _, _, cx| view.toggle_home_filter(provider, cx)))
                .into_any_element()
        }
    }

    pub(super) fn toggle_home_filter(&mut self, provider: ProviderId, cx: &mut Context<Self>) {
        self.filter_home((self.home.filter != Some(provider)).then_some(provider), cx);
    }

    pub(super) fn filter_home(&mut self, provider: Option<ProviderId>, cx: &mut Context<Self>) {
        if self.home.filter != provider {
            self.home.filter = provider;
            self.home.shelf_turns.clear();
            self.home.mix.clear();
            self.stop_workspace_scrolling();
            self.reset_scroll();
            cx.notify();
        }
    }

    fn refresh_visible_home(&mut self, cx: &mut Context<Self>) {
        if let Some((home, _)) = self.state.home_view() {
            for source in FeedSource::available(home) {
                if self
                    .home
                    .filter
                    .is_none_or(|provider| source.provider == provider)
                {
                    self.refresh_home_source(source, cx);
                }
            }
        }
    }
}
