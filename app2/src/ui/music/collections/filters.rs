use gpui::{
    AnyElement, App, Context, Div, ElementId, FontWeight, IntoElement, Role, SharedString,
    Stateful, Toggled, Window, div, prelude::*, px, rgb,
};
use guise::{Button, Chip, Icon, IconName, Size, Variant};

use crate::{
    music::library::{LibrarySort, Scope},
    providers::ProviderId,
    ui::{
        MainView,
        components::{
            Activate, button,
            glass_control::{Material, Tone},
            overlay::{SelectChoice, SelectLabel},
        },
        design_system::controls::{ControlState, ControlVariant},
        fluid_glass::tokens::{size, space, text},
        theme::{ACCENT, ELEVATED, SURFACE, primary, secondary, surface_border},
        workspace::LibraryLocation,
    },
};

use super::LIBRARY_FILTER_ORDER;

const SORT_OPTIONS: [(LibrarySort, &str, &str); 3] = [
    (
        LibrarySort::Recent,
        "library-sort-recent",
        "Недавно добавленные",
    ),
    (LibrarySort::Title, "library-sort-title", "Название"),
    (LibrarySort::Artist, "library-sort-artist", "Исполнитель"),
];

impl MainView {
    pub(in crate::ui) fn dismiss_library_filters(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.music.library_source_select.close(window, cx)
            || self.music.library_sort_select.close(window, cx)
        {
            cx.notify();
            return true;
        }
        let library = &mut self.music.collections;
        if !library.filters_open {
            return false;
        }
        library.filters_open = false;
        library.reveal();
        window.focus(&self.music.library_filters_focus, cx);
        cx.notify();
        true
    }

    pub(super) fn library_filters_trigger(
        &self,
        compact: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let library = &self.music.collections;
        let controls = self.glass_controls(cx);
        let icon_only = compact && controls.is_some();
        let applied = applied_filters(&library.location);
        let count = applied.iter().flatten().count();
        let label = if count > 0 {
            format!("Фильтры · {count}")
        } else {
            "Фильтры".into()
        };
        let trigger = button("library-filters", label.clone(), LIBRARY_FILTER_ORDER)
            .track_focus(&self.music.library_filters_focus)
            .h(px(size::SM))
            .flex_none()
            .px(px(space::SM))
            .gap(px(space::SM))
            .rounded(px(6.))
            .text_size(px(text::METADATA))
            .child(
                Icon::new(if icon_only && count > 0 {
                    IconName::ListFilter
                } else {
                    IconName::SlidersHorizontal
                })
                .pixel_size(px(16.)),
            )
            .when(icon_only, |trigger| {
                trigger
                    .w(px(size::SM))
                    .justify_center()
                    .px_0()
                    .tooltip(guise::tooltip(label.clone()))
            })
            .when(!icon_only, |trigger| {
                trigger.child(label).child(
                    Icon::new(if library.filters_open {
                        IconName::ChevronUp
                    } else {
                        IconName::ChevronDown
                    })
                    .pixel_size(px(12.)),
                )
            });
        if let Some(controls) = controls {
            ControlVariant::SecondaryButton.paint(
                trigger,
                controls,
                ControlState {
                    active: count > 0 || library.filters_open,
                    expanded: library.filters_open,
                    ..Default::default()
                },
            )
        } else {
            trigger
                .text_color(if count > 0 {
                    rgb(ACCENT).into()
                } else {
                    secondary()
                })
                .border_color(surface_border())
                .when(library.filters_open, |trigger| trigger.bg(rgb(ELEVATED)))
                .hover(|style| style.bg(rgb(ELEVATED)).text_color(primary()))
        }
        .on_toggle(
            library.filters_open,
            cx.listener(|view, _, window, cx| {
                if !view.dismiss_library_filters(window, cx) {
                    view.music.collections.filters_open = true;
                    cx.notify();
                }
            }),
        )
    }

    pub(super) fn library_applied_filters(&self) -> Option<String> {
        let labels = applied_filters(&self.music.collections.location);
        let result = labels.into_iter().flatten().collect::<Vec<_>>().join(" · ");
        (!result.is_empty()).then_some(result)
    }

    pub(super) fn library_filters_panel(
        &self,
        summary: String,
        albums: bool,
        list: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let library = &self.music.collections;
        let controls = self.glass_controls(cx);
        let panel = div()
            .id("library-filter-options")
            .accessibility_id("library-filter-options")
            .role(Role::Group)
            .aria_label("Фильтры медиатеки")
            .w_full()
            .max_w(px(880.))
            .p(px(10.))
            .flex()
            .flex_col()
            .gap(px(7.))
            .rounded(px(7.))
            .border_1();
        let panel = if let Some(controls) = controls {
            controls.group(panel)
        } else {
            panel.border_color(surface_border()).bg(rgb(SURFACE))
        };
        panel
            .child(filter_row("Площадка").child(self.library_sources(window, cx)))
            .child(filter_row("Сортировка").child(self.library_sort(window, cx)))
            .child(
                filter_row("Список")
                    .child(filter_choice(
                        "library-downloaded",
                        "Только скачанные",
                        43,
                        library.location.downloaded,
                        controls,
                        cx.listener(|view, _, _, cx| {
                            view.change_library_filters(
                                |location| location.downloaded = !location.downloaded,
                                cx,
                            );
                        }),
                    ))
                    .when(albums, |row| {
                        row.child(filter_action(
                            "library-album-view",
                            if list {
                                "Показать плитки"
                            } else {
                                "Показать списком"
                            },
                            44,
                            Variant::Default,
                            controls,
                            cx.listener(|view, _, _, cx| view.toggle_album_view(cx)),
                        ))
                    })
                    .when(self.library_applied_filters().is_some(), |row| {
                        row.child(filter_action(
                            "library-reset-filters",
                            "Сбросить фильтры",
                            45,
                            Variant::Subtle,
                            controls,
                            cx.listener(|view, _, window, cx| {
                                view.change_library_filters(
                                    |location| {
                                        location.source = None;
                                        location.group = None;
                                        location.downloaded = false;
                                        location.sort = LibrarySort::default();
                                    },
                                    cx,
                                );
                                window.focus(&view.music.library_filters_focus, cx);
                            }),
                        ))
                    }),
            )
            .child(super::super::status("library-details", summary).text_size(px(12.)))
    }

    pub(super) fn change_library_filters(
        &mut self,
        change: impl FnOnce(&mut LibraryLocation),
        cx: &mut Context<Self>,
    ) {
        let library = &mut self.music.collections;
        library.change_filters(change);
        self.reset_list_prefetch(cx);
        cx.notify();
    }

    fn library_sources(&self, window: &Window, cx: &Context<Self>) -> Div {
        let library = &self.music.collections;
        let selected = &library.location.source;
        let accounts = self
            .state
            .home_view()
            .into_iter()
            .flat_map(|(home, _)| &home.accounts);
        let scopes: Vec<_> = [None, Some(Scope::Local)]
            .into_iter()
            .chain(accounts.map(|account| {
                Some(Scope::Provider {
                    provider: account.provider,
                    profile: account.profile.id.clone(),
                })
            }))
            .collect();
        let choices = scopes
            .iter()
            .map(|scope| {
                let provider = scope.as_ref().and_then(Scope::provider);
                let (suffix, count) = if scope.is_none() {
                    ("all", library.results.unfiltered)
                } else {
                    (
                        provider.map_or("local", ProviderId::as_str),
                        library
                            .results
                            .counts
                            .get(&provider)
                            .copied()
                            .unwrap_or_default(),
                    )
                };
                let label = scope.as_ref().map_or("Все", scope_label);
                SelectChoice::new(
                    format!("liked-source-{suffix}"),
                    format!("{label} · {count}"),
                    scope == selected,
                )
            })
            .collect();
        self.music.library_source_select.view(
            SelectLabel {
                trigger: "library-source-select",
                menu: "liked-sources",
                label: "Площадки медиатеки",
                value: selected.as_ref().map_or("Все", scope_label).into(),
                enabled: true,
            },
            choices,
            |view| &mut view.music.library_source_select,
            move |view, index, _, cx| {
                let scope = &scopes[index];
                if &view.music.collections.location.source != scope {
                    view.change_library_filters(
                        |location| {
                            location.source = scope.clone();
                            if location.source != Some(Scope::Local) {
                                location.group = None;
                            }
                        },
                        cx,
                    );
                }
            },
            self.glass_controls(cx),
            self.uses_fluid_glass() && !self.reduced_motion && !self.fluid_high_contrast(cx),
            window,
            cx,
        )
    }

    fn library_sort(&self, window: &Window, cx: &Context<Self>) -> Div {
        let selected = self.music.collections.location.sort;
        self.music.library_sort_select.view(
            SelectLabel {
                trigger: "library-sort-select",
                menu: "library-sort-options",
                label: "Сортировка",
                value: selected.label().into(),
                enabled: true,
            },
            SORT_OPTIONS
                .into_iter()
                .map(|(sort, id, label)| SelectChoice::new(id, label, selected == sort))
                .collect(),
            |view| &mut view.music.library_sort_select,
            |view, index, _, cx| {
                view.change_library_filters(|location| location.sort = SORT_OPTIONS[index].0, cx)
            },
            self.glass_controls(cx),
            self.uses_fluid_glass() && !self.reduced_motion && !self.fluid_high_contrast(cx),
            window,
            cx,
        )
    }
}

fn filter_choice(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    order: isize,
    selected: bool,
    controls: Option<Material>,
    change: impl Fn(&(), &mut Window, &mut App) + 'static,
) -> AnyElement {
    if let Some(controls) = controls {
        let label = label.into();
        let button = button(id, label.clone(), order)
            .role(Role::CheckBox)
            .aria_toggled(Toggled::from(selected))
            .h(px(24.))
            .px(px(10.))
            .gap(px(6.))
            .rounded(px(6.))
            .text_size(px(11.))
            .font_weight(FontWeight::MEDIUM);
        controls
            .paint(
                button,
                if selected {
                    Tone::Selected
                } else {
                    Tone::Quiet
                },
            )
            .children(selected.then_some("✓"))
            .child(label)
            .on_activate(change)
            .into_any_element()
    } else {
        Chip::new(id, label)
            .checked(selected)
            .size(Size::Xs)
            .tab_order(order)
            .on_change(move |_, window, cx| change(&(), window, cx))
            .into_any_element()
    }
}

fn filter_action(
    id: &'static str,
    label: &'static str,
    order: isize,
    variant: Variant,
    controls: Option<Material>,
    activate: impl Fn(&(), &mut Window, &mut App) + 'static,
) -> AnyElement {
    if let Some(controls) = controls {
        let button = button(id, label, order)
            .h(px(30.))
            .px(px(14.))
            .rounded(px(6.))
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD);
        let action = if variant == Variant::Subtle {
            ControlVariant::TextAction
        } else {
            ControlVariant::SecondaryButton
        };
        action
            .paint(button, controls, ControlState::default())
            .child(label)
            .on_activate(activate)
            .into_any_element()
    } else {
        Button::new(id, label)
            .size(Size::Xs)
            .variant(variant)
            .tab_order(order)
            .on_click(move |_, window, cx| activate(&(), window, cx))
            .into_any_element()
    }
}

fn filter_row(label: &'static str) -> Div {
    div().flex().items_start().gap(px(7.)).child(
        div()
            .w(px(80.))
            .flex_none()
            .pt(px(4.))
            .text_size(px(12.))
            .text_color(secondary())
            .child(label),
    )
}

fn applied_filters(location: &LibraryLocation) -> [Option<&str>; 3] {
    [
        location.source.as_ref().map(scope_label),
        location.downloaded.then_some("Скачанные"),
        (location.sort != LibrarySort::default()).then_some(location.sort.label()),
    ]
}

fn scope_label(scope: &Scope) -> &'static str {
    match scope {
        Scope::Local => "На компьютере",
        Scope::Provider { provider, .. } => provider.display_name(),
    }
}
