use gpui::{App, Div, div, prelude::*, px};
use guise::{Button, Variant};

use crate::ui::{
    components::{
        artwork::{ArtworkClearStatus, artwork_clear_status, clear_artwork_cache},
        glass_control::Material,
        heading,
    },
    design_system::controls::ControlVariant,
    music::status,
};

pub(super) fn section(interactive: bool, glass: Option<Material>, cx: &App) -> Div {
    let label = match artwork_clear_status(cx) {
        ArtworkClearStatus::Idle => None,
        ArtworkClearStatus::Clearing => Some("Очищаем кэш обложек…"),
        ArtworkClearStatus::Cleared => Some("Кэш обложек очищен."),
        ArtworkClearStatus::Failed => Some("Не удалось полностью очистить кэш обложек."),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(heading("Обложки", 18.))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .child(
                    Button::new("settings-clear-artwork", "Очистить кэш обложек")
                        .variant(Variant::Default)
                        .when_some(glass, |button, material| {
                            ControlVariant::SecondaryButton.text_button(button, material)
                        })
                        .tab_order(708)
                        .disabled(!interactive)
                        .on_click(|_, _, cx| clear_artwork_cache(cx)),
                )
                .when_some(label, |row, label| {
                    row.child(status("settings-clear-artwork-status", label).text_size(px(13.)))
                }),
        )
}
