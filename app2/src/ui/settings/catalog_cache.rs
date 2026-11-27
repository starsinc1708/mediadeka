use gpui::{Context, Div, Global, Task, div, prelude::*, px};
use guise::{Button, Variant};

use crate::{
    music::clear_catalog_cache,
    ui::{
        MainView,
        components::{glass_control::Material, heading},
        design_system::controls::ControlVariant,
        music::status,
    },
};

#[cfg(all(test, feature = "capture"))]
mod tests;

#[derive(Clone, Copy, Default)]
enum Phase {
    #[default]
    Idle,
    Clearing,
    Cleared,
    Failed,
}

struct Operation {
    phase: Phase,
    _task: Option<Task<()>>,
}

impl Global for Operation {}

pub(super) fn section(interactive: bool, glass: Option<Material>, cx: &Context<MainView>) -> Div {
    let phase = cx
        .try_global::<Operation>()
        .map_or(Phase::Idle, |operation| operation.phase);
    let hint = match phase {
        Phase::Idle => None,
        Phase::Clearing => Some("Очищаем снимки каталога…"),
        Phase::Cleared => Some("Снимки каталога очищены."),
        Phase::Failed => Some("Не удалось очистить снимки каталога."),
    };
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(heading("Снимки каталога", 18.))
        .child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.))
                .child(
                    Button::new(
                        "settings-clear-catalog",
                        "Очистить снимки страниц и плейлистов",
                    )
                    .variant(Variant::Default)
                    .when_some(glass, |button, material| {
                        ControlVariant::SecondaryButton.text_button(button, material)
                    })
                    .tab_order(709)
                    .disabled(!interactive || matches!(phase, Phase::Clearing))
                    .on_click(cx.listener(|view, _, _, cx| request(view, cx))),
                )
                .when_some(hint, |row, hint| {
                    row.child(status("settings-clear-catalog-status", hint).text_size(px(13.)))
                }),
        )
}

fn request(view: &mut MainView, cx: &mut Context<MainView>) {
    if cx
        .try_global::<Operation>()
        .is_some_and(|operation| matches!(operation.phase, Phase::Clearing))
    {
        return;
    }
    view.clear_music_prefetch();
    let task = cx.spawn(async move |view, cx| {
        let result = cx
            .background_executor()
            .spawn(async { clear_catalog_cache() })
            .await;
        let phase = match result {
            Ok(generation) => {
                let _ = view.update(cx, |view, _| {
                    view.retire_catalog_playlists(generation);
                    view.forget_artist_discography();
                });
                Phase::Cleared
            }
            Err(_) => Phase::Failed,
        };
        cx.update(|cx| {
            cx.set_global(Operation { phase, _task: None });
            cx.refresh_windows();
        });
    });
    cx.set_global(Operation {
        phase: Phase::Clearing,
        _task: Some(task),
    });
    cx.notify();
}
