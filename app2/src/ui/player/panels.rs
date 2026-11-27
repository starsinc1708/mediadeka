use super::PlayerBar;
use crate::{
    playback::Playback,
    ui::{
        components::{
            Activate, button,
            glass_control::Material,
            overlay::{self, MenuLayout},
        },
        design_system::materials,
        theme::{DIVIDER, ELEVATED, PLAYER_PANEL_CURRENT, PLAYER_PANEL_HOVER, fluid_high_contrast},
    },
};
use gpui::{Context, Div, FontWeight, Role, Stateful, Window, div, prelude::*, px, rgb};

const PANEL_ROW_HEIGHT: f32 = 31.;

impl PlayerBar {
    pub(in crate::ui) fn dismiss_devices(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.devices_open {
            return false;
        }
        self.devices_open = false;
        if let Some(focus) = self.devices_return.take() {
            window.focus(&focus, cx);
        }
        cx.notify();
        true
    }

    pub(super) fn show_devices(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.devices_open {
            self.dismiss_devices(window, cx);
        } else {
            self.devices_return = window.focused(cx);
            self.dismiss_sound_popover(window, cx);
            self.devices_open = true;
            window.focus(&self.devices_focus, cx);
            self.command(cx, Playback::request_devices);
            cx.notify();
        }
    }

    pub(super) fn devices_panel(&self, window: &Window, cx: &Context<Self>) -> Option<Div> {
        if !self.devices_open {
            return None;
        }
        let material = self.fluid_chrome().then(|| {
            materials::Material::new(materials::Kind::SmokedGlass, materials::accent(cx))
                .high_contrast(fluid_high_contrast(cx))
        });
        let layout = MenuLayout::new(window, 310., material.is_none().then_some(PANEL_ROW_HEIGHT));
        let empty = self.playback.devices().is_empty();
        let panel = layout
            .frame("player-panel", "Устройство вывода", material)
            .role(Role::Group)
            .track_focus(&self.devices_focus)
            .tab_index(611)
            .tab_group()
            .tab_stop(false)
            .when(material.is_none(), |panel| {
                panel.border_color(rgb(DIVIDER)).bg(rgb(ELEVATED))
            })
            .child(
                div()
                    .h(px(layout.item_height))
                    .flex_none()
                    .flex()
                    .items_center()
                    .px(px(8.))
                    .text_size(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Устройство вывода"),
            )
            .when(empty, |panel| {
                panel.child(
                    div()
                        .h(px(layout.item_height))
                        .flex()
                        .items_center()
                        .px(px(8.))
                        .text_size(px(13.))
                        .child(
                            self.playback
                                .error()
                                .unwrap_or("Читаем доступные устройства…")
                                .to_owned(),
                        ),
                )
            })
            .child(self.device_rows(layout, material, cx));
        Some(div().child(layout.place(
            self.devices_anchor.get(),
            true,
            (1 + self.playback.devices().len().max(1), 0),
            panel,
            self.fluid_motion && !self.reduced_motion && !fluid_high_contrast(cx),
            cx.listener(|bar, _, window, cx| {
                bar.dismiss_devices(window, cx);
                cx.stop_propagation();
            }),
        )))
    }

    fn device_rows(
        &self,
        layout: MenuLayout,
        material: Option<Material>,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id("player-device-items")
            .role(Role::ListBox)
            .aria_label("Устройства вывода")
            .flex_none()
            .flex()
            .flex_col()
            .children(
                self.playback
                    .devices()
                    .iter()
                    .enumerate()
                    .map(|(index, device)| {
                        let id = device.id.clone();
                        let title = device.label.clone();
                        let selected = id == self.playback.audio_device();
                        let row = button(
                            format!("player-device-{id}"),
                            title.clone(),
                            620 + index as isize,
                        )
                        .role(Role::ListBoxOption)
                        .aria_selected(selected)
                        .when(selected, |row| {
                            row.aria_description("Текущее устройство вывода")
                        });
                        let row = if let Some(material) = material {
                            overlay::menu_item(row, layout, material, false)
                        } else {
                            row.w_full()
                                .h(px(PANEL_ROW_HEIGHT))
                                .px(px(8.))
                                .gap(px(8.))
                                .rounded(px(6.))
                                .text_size(px(13.))
                                .when(selected, |row| row.bg(rgb(PLAYER_PANEL_CURRENT)))
                                .hover(|row| row.bg(rgb(PLAYER_PANEL_HOVER)))
                        };
                        overlay::menu_value(row, title.into(), selected).on_activate(cx.listener(
                            move |bar, _, _, cx| {
                                bar.command(cx, |playback| playback.select_device(id.clone()))
                            },
                        ))
                    }),
            )
    }
}
