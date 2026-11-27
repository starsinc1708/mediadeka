use gpui::{
    App, ClipboardItem, Context, Div, FocusHandle, KeyDownEvent, Role, Window, div, prelude::*, px,
};
use guise::IconName;

use crate::ui::{
    Escape, MainView, ShiftTab, Tab,
    components::{
        Activate, button,
        overlay::{self, MenuLayout, MenuMotion, Select, SelectChoice, SelectLabel, TriggerBounds},
    },
    design_system::{
        controls::{ControlState, ControlVariant},
        materials::Material,
    },
};

const VALUES: [(&str, &str); 3] = [
    ("editorial-select-auto", "Автоматически"),
    ("editorial-select-local", "Локальный файл"),
    ("editorial-select-catalog", "Каталог"),
];
const UNAVAILABLE: (&str, &str) = ("editorial-select-unavailable", "Копировать ссылку");
const UNAVAILABLE_REASON: &str = "Ссылка недоступна";

pub(in crate::ui) struct Samples {
    select: Select,
    value: usize,
    action_bounds: TriggerBounds,
    action_focus: FocusHandle,
    menu_focus: FocusHandle,
    menu_open: bool,
    #[cfg(feature = "capture")]
    menu_sample: Option<std::time::Duration>,
}

impl Samples {
    pub(in crate::ui) fn new(cx: &App) -> Self {
        Self {
            select: Select::new(894, cx),
            value: 0,
            action_bounds: TriggerBounds::default(),
            action_focus: cx.focus_handle().tab_index(899).tab_stop(true),
            menu_focus: cx.focus_handle(),
            menu_open: false,
            #[cfg(feature = "capture")]
            menu_sample: None,
        }
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<MainView>) {
        self.menu_open = false;
        window.focus(&self.action_focus, cx);
        cx.notify();
    }

    pub(in crate::ui) fn render(
        &self,
        material: Option<Material>,
        window: &Window,
        cx: &Context<MainView>,
    ) -> Div {
        let Some(material) = material else {
            return div();
        };
        let motion = !cx.reduce_motion() && !material.contrast();
        let expanded = self.menu_open;
        let trigger = ControlVariant::GhostIcon
            .paint(
                button("editorial-menu-trigger", "Действия", 899)
                    .track_focus(&self.action_focus)
                    .justify_center()
                    .size(px(36.))
                    .child(overlay::menu_icon(IconName::Ellipsis)),
                material,
                ControlState::default(),
            )
            .when(!expanded, |trigger| {
                trigger.tooltip(guise::tooltip("Действия"))
            })
            .on_toggle(
                expanded,
                cx.listener(|view, _, window, cx| {
                    let samples = &mut view.editorial_menu_samples;
                    if samples.menu_open {
                        samples.close_menu(window, cx);
                    } else {
                        samples.menu_open = true;
                        window.focus(&samples.menu_focus, cx);
                        cx.notify();
                    }
                }),
            );
        let select = self.select.view(
            SelectLabel {
                trigger: "editorial-select",
                menu: "editorial-select-options",
                label: "Источник текста",
                value: VALUES[self.value].1.into(),
                enabled: true,
            },
            VALUES
                .into_iter()
                .enumerate()
                .map(|(index, (id, label))| SelectChoice::new(id, label, self.value == index))
                .chain(std::iter::once(
                    SelectChoice::new(UNAVAILABLE.0, UNAVAILABLE.1, false)
                        .separated()
                        .disabled(UNAVAILABLE_REASON),
                ))
                .collect(),
            |view| &mut view.editorial_menu_samples.select,
            |view, index, _, _| view.editorial_menu_samples.value = index,
            Some(material),
            motion,
            window,
            cx,
        );
        div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(8.))
            .child(select)
            .child(self.action_bounds.capture(trigger))
            .when(expanded, |gallery| {
                let layout = MenuLayout::new(window, 224., None);
                let copy =
                    |view: &mut MainView, window: &mut Window, cx: &mut Context<MainView>| {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            VALUES[view.editorial_menu_samples.value].1.into(),
                        ));
                        view.editorial_menu_samples.close_menu(window, cx);
                    };
                let command = overlay::menu_value(
                    overlay::menu_item(
                        button("editorial-menu-copy", "Копировать значение", 900)
                            .role(Role::MenuItem)
                            .aria_active_descendant(),
                        layout,
                        material,
                        true,
                    )
                    .child(overlay::menu_icon(IconName::Copy)),
                    "Копировать значение".into(),
                    false,
                )
                .on_activate(cx.listener(move |view, _, window, cx| copy(view, window, cx)));
                let panel = layout
                    .frame("editorial-menu", "Действия", Some(material))
                    .track_focus(&self.menu_focus)
                    .on_action(cx.listener(|view, _: &Escape, window, cx| {
                        view.editorial_menu_samples.close_menu(window, cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(|view, _: &Tab, window, cx| {
                        view.editorial_menu_samples.close_menu(window, cx);
                    }))
                    .on_action(cx.listener(|view, _: &ShiftTab, window, cx| {
                        view.editorial_menu_samples.close_menu(window, cx);
                    }))
                    .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
                        match event.keystroke.key.as_str() {
                            "enter"
                                if view.editorial_menu_samples.menu_focus.is_focused(window) =>
                            {
                                copy(view, window, cx);
                            }
                            _ => return,
                        }
                        cx.stop_propagation();
                    }))
                    .child(command);
                let menu_motion = MenuMotion::from(motion);
                #[cfg(feature = "capture")]
                let menu_motion = menu_motion.sampled(self.menu_sample);
                gallery.child(layout.place(
                    self.action_bounds.get(),
                    false,
                    (1, 0),
                    panel,
                    menu_motion,
                    cx.listener(|view, _, window, cx| {
                        view.editorial_menu_samples.close_menu(window, cx);
                        cx.stop_propagation();
                    }),
                ))
            })
    }
}

#[cfg(feature = "capture")]
impl MainView {
    pub fn editorial_menu_anchor_for_capture(&self) -> gpui::Bounds<gpui::Pixels> {
        self.editorial_menu_samples.action_bounds.get()
    }

    pub fn editorial_select_bounds_for_capture(
        &self,
    ) -> (
        gpui::Bounds<gpui::Pixels>,
        Option<gpui::Bounds<gpui::Pixels>>,
    ) {
        (
            self.editorial_menu_samples
                .select
                .trigger_bounds_for_capture(),
            self.editorial_menu_samples.select.menu_bounds_for_capture(),
        )
    }

    pub fn editorial_select_row_bounds_for_capture(
        &self,
        row: usize,
    ) -> Option<gpui::Bounds<gpui::Pixels>> {
        self.editorial_menu_samples
            .select
            .row_bounds_for_capture(row)
    }

    pub fn sample_editorial_menu_for_capture(
        &mut self,
        sample: Option<std::time::Duration>,
        cx: &mut Context<Self>,
    ) {
        self.editorial_menu_samples.menu_sample = sample;
        cx.notify();
    }
}
