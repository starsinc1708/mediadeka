//! The workspace toolbar: history, global catalog search and window controls.

use gpui::{
    AnyElement, Context, Div, IntoElement, Role, Stateful, Window, WindowControlArea, deferred,
    div, prelude::*, px, relative, rgb,
};
use guise::{Icon, IconName, Size, Variant};

use crate::ui::{
    MainView,
    components::{
        Activate, button,
        content_state::refresh_indicator,
        glass_control::Material,
        music::control,
        overlay::menu_surface,
        search_input::{Scope, SearchInput},
    },
    design_system::controls::{ControlState, ControlVariant},
    fluid_glass::{
        Geometry,
        tokens::{self as fluid, radius, size, space, text},
    },
    music::status,
    theme::{CATALOG_SEARCH_BACKGROUND, playlist::metrics, secondary, surface_border},
    window_controls,
};

use super::{TOOLBAR_HEIGHT, WorkspaceLayout};

impl MainView {
    pub(super) fn music_toolbar(
        &self,
        layout: WorkspaceLayout,
        window: &Window,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let WorkspaceLayout {
            compact, margin, ..
        } = layout;
        let glass = self.uses_fluid_glass();
        let material = self.glass_controls(cx);
        let (search_width, gap) = if glass {
            let geometry = Geometry::for_window(window);
            (
                geometry.search_max(),
                geometry.responsive_width(space::XS, space::MD),
            )
        } else if compact {
            (224., metrics::GAP)
        } else {
            (440., 11.)
        };
        let search = if let Some(material) = material {
            SearchInput::new(
                "home-search",
                self.workspace.search.field(),
                material,
                Scope::Global,
                12,
                compact,
            )
            .submit(cx.listener(|view, _, window, cx| view.submit_toolbar_search(window, cx)))
            .into_any_element()
        } else {
            let submit = control(
                "home-catalog-search",
                "Начать поиск музыки",
                IconName::Search,
                11,
                true,
                false,
                false,
            )
            .size(Size::Md)
            .radius(Size::Sm)
            .variant(Variant::Subtle)
            .color(secondary())
            .on_click(cx.listener(|view, _, window, cx| {
                view.submit_toolbar_search(window, cx);
            }));
            div()
                .h(px(metrics::CONTROL))
                .flex()
                .gap(px(3.))
                .items_center()
                .pl(px(5.))
                .pr(px(2.))
                .rounded(px(6.))
                .border_1()
                .border_color(surface_border())
                .bg(rgb(CATALOG_SEARCH_BACKGROUND))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(self.workspace.search.field()),
                )
                .child(submit)
                .into_any_element()
        };
        div()
            .id("workspace-toolbar")
            .accessibility_id("workspace-toolbar")
            .role(Role::Toolbar)
            .aria_label("Панель навигации")
            .h(px(if glass {
                fluid::TITLEBAR
            } else {
                TOOLBAR_HEIGHT
            }))
            .flex_none()
            .flex()
            .items_center()
            .pl(px(margin))
            .gap(px(gap))
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(if glass { space::XS } else { gap }))
                    .child(self.history_button(false, cx))
                    .child(self.history_button(true, cx)),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w(px(if compact { 120. } else { 200. }))
                    .max_w(px(search_width))
                    .when(glass, |search| {
                        search.capture_key_down(cx.listener(Self::on_search_suggestion_key))
                    })
                    .child(search)
                    .children(
                        material.and_then(|material| self.search_suggestions(window, material, cx)),
                    ),
            )
            .when(glass, |toolbar| {
                toolbar.child(div().size(px(24.)).flex_none().when(
                    self.global_search_pending(),
                    |slot| {
                        slot.child(refresh_indicator(
                            "workspace-search-progress",
                            "Идёт поиск",
                            cx,
                        ))
                    },
                ))
            })
            .when(!glass, |toolbar| {
                toolbar.child(
                    div()
                        .flex_none()
                        .text_size(px(11.))
                        .text_color(secondary())
                        .child("Ctrl+K"),
                )
            })
            .when(!glass, |toolbar| {
                toolbar.child(
                    status(
                        "workspace-search-scope",
                        if self.searches_connected_catalogs() {
                            if compact {
                                "Каталоги"
                            } else {
                                "Медиатека и подключённые каталоги"
                            }
                        } else {
                            "Медиатека и ссылки"
                        },
                    )
                    .text_size(px(11.))
                    .flex_none(),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(space::MD))
                    .h_full()
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(window_controls(window, cx))
    }

    fn history_button(&self, forward: bool, cx: &Context<Self>) -> guise::ActionIcon {
        let enabled = self.workspace.routes.can_step(forward)
            && !self.is_settings_login()
            && !self.disconnecting();
        let (id, title, glyph, order) = if forward {
            ("workspace-forward", "Вперёд", IconName::ChevronRight, 9)
        } else {
            ("workspace-back", "Назад", IconName::ChevronLeft, 8)
        };
        let control = control(id, title, glyph, order, enabled, false, false)
            .size(Size::Md)
            .radius(Size::Sm)
            .variant(Variant::Subtle)
            .color(secondary())
            .when_some(self.glass_controls(cx), |control, material| {
                control
                    .pixel_size(px(size::SM))
                    .glyph_size(px(18.))
                    .surface_style(ControlVariant::GhostIcon.surface(
                        material,
                        ControlState {
                            disabled: !enabled,
                            ..Default::default()
                        },
                    ))
            });
        if enabled {
            control.on_click(cx.listener(move |view, _, window, cx| {
                view.step_workspace_route(forward, window, cx);
            }))
        } else {
            control
        }
    }

    fn search_suggestions(
        &self,
        window: &Window,
        material: Material,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let search = &self.workspace.search;
        let suggestions = search.suggestions(window, cx);
        if suggestions.is_empty() {
            return None;
        }
        let popup = div()
            .id("workspace-search-suggestions")
            .accessibility_id("workspace-search-suggestions")
            .role(Role::ListBox)
            .aria_label("Недавние запросы")
            .aria_size_of_set(suggestions.len())
            .track_focus(&search.suggestions_focus)
            .tab_stop(false)
            .absolute()
            .top(relative(1.))
            .mt(px(space::XS))
            .w_full()
            .p(px(space::XS))
            .border_1()
            .rounded(px(radius::POPOVER))
            .occlude()
            .flex()
            .flex_col()
            .shadow_lg()
            .child(
                div()
                    .px(px(space::SM))
                    .py(px(space::XS))
                    .text_size(px(text::CAPTION))
                    .child("Недавние запросы"),
            )
            .children(suggestions.into_iter().enumerate().map(|(index, query)| {
                let query = query.to_owned();
                let selected = search.highlighted == Some(index);
                ControlVariant::MenuItem
                    .paint(
                        button(("workspace-search-suggestion", index), query.clone(), 11)
                            .role(Role::ListBoxOption)
                            .aria_selected(selected)
                            .when(selected, |item| item.aria_active_descendant())
                            .tab_stop(false)
                            .h(px(size::SM))
                            .gap(px(space::SM))
                            .px(px(space::SM))
                            .text_size(px(text::METADATA)),
                        material,
                        ControlState {
                            active: selected,
                            ..Default::default()
                        },
                    )
                    .child(Icon::new(IconName::History).pixel_size(px(16.)))
                    .child(div().min_w_0().truncate().child(query.clone()))
                    .on_activate(cx.listener(move |view, _, window, cx| {
                        view.choose_search_suggestion(&query, window, cx);
                    }))
            }));
        Some(
            deferred(menu_surface(popup, material))
                .priority(1)
                .into_any_element(),
        )
    }
}
