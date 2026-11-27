use gpui::{Context, Div, Task, div, prelude::*, px};
use guise::{Button, Chip, Size, Variant, theme};

#[cfg(feature = "capture")]
use crate::providers::ProviderId;
use crate::{
    providers::downloads::PROVIDERS,
    storage::{Store, StoreError},
    ui::{MainView, assets::provider_mark, components::heading, music::status},
};

#[derive(Default)]
pub(in crate::ui) struct SoundDownloadSettings {
    enabled: Option<[bool; PROVIDERS.len()]>,
    error: Option<String>,
    task: Option<Task<()>>,
    #[cfg(feature = "capture")]
    capture_origin: std::rc::Rc<std::cell::Cell<gpui::Pixels>>,
}

impl MainView {
    pub(in crate::ui) fn load_sound_download_settings(&mut self, cx: &mut Context<Self>) {
        if self.sound_download_settings.enabled.is_none() {
            self.update_sound_download_settings(None, cx);
        }
    }

    fn update_sound_download_settings(&mut self, changed: Option<usize>, cx: &mut Context<Self>) {
        let settings = &mut self.sound_download_settings;
        if settings.task.is_some() {
            return;
        }
        settings.error = None;
        let previous = settings.enabled;
        settings.task = Some(cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = Store::open_default()?;
                    if let Some((index, mut enabled)) = changed.zip(previous) {
                        enabled[index] = !enabled[index];
                        store.set_sound_download_enabled(PROVIDERS[index], enabled[index])?;
                        return Ok(enabled);
                    }
                    let mut enabled = [false; PROVIDERS.len()];
                    for (index, provider) in PROVIDERS.into_iter().enumerate() {
                        enabled[index] = store.sound_download_enabled(provider)?;
                    }
                    Ok::<_, StoreError>(enabled)
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                let settings = &mut view.sound_download_settings;
                settings.task = None;
                match result {
                    Ok(enabled) => settings.enabled = Some(enabled),
                    Err(error) => settings.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    pub(super) fn sound_download_settings(&self, interactive: bool, cx: &Context<Self>) -> Div {
        let settings = &self.sound_download_settings;
        let section = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .child(heading("Загрузка оригиналов", 18.))
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(theme(cx).dimmed())
                    .child("Из подключённых аккаунтов, когда нет полного прямого потока."),
            )
            .when_some(settings.enabled, |group, enabled| {
                group.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.))
                        .when(!interactive || settings.task.is_some(), |row| {
                            row.opacity(0.6)
                        })
                        .children(PROVIDERS.into_iter().zip(enabled).enumerate().map(
                            |(index, (provider, checked))| {
                                Chip::new(
                                    format!("settings-audio-download-{}", provider.as_str()),
                                    provider.display_name(),
                                )
                                .accessible_name(format!(
                                    "Загрузка оригиналов · {}",
                                    provider.display_name()
                                ))
                                .left_section(provider_mark(provider, 14.))
                                .size(Size::Sm)
                                .tab_order(700 + index as isize)
                                .checked(checked)
                                .when(interactive, |chip| {
                                    chip.on_change(cx.listener(move |view, _, _, cx| {
                                        view.update_sound_download_settings(Some(index), cx);
                                    }))
                                })
                            },
                        )),
                )
            });
        #[cfg(feature = "capture")]
        let section = {
            let origin = settings.capture_origin.clone();
            let scroll = self.scroll.clone();
            section.on_children_prepainted(move |children, _, _| {
                if let Some(heading) = children.first() {
                    origin.set(heading.origin.y - scroll.offset().y);
                }
            })
        };
        section
            .when(
                settings.enabled.is_none() && settings.error.is_none(),
                |group| {
                    group.child(status(
                        "settings-sound-download-loading",
                        "Читаем настройки…",
                    ))
                },
            )
            .when_some(settings.error.as_ref(), |group, error| {
                group
                    .child(status("settings-sound-download-error", error.clone()))
                    .when(settings.enabled.is_none(), |group| {
                        group.child(
                            Button::new("settings-sound-download-retry", "Повторить")
                                .variant(Variant::Default)
                                .tab_order(700)
                                .disabled(!interactive)
                                .on_click(cx.listener(|view, _, _, cx| {
                                    view.load_sound_download_settings(cx);
                                })),
                        )
                    })
            })
    }
}

#[cfg(feature = "capture")]
impl MainView {
    pub fn sound_download_enabled_for_capture(&self, provider: ProviderId) -> Option<bool> {
        let settings = &self.sound_download_settings;
        if settings.task.is_some() {
            return None;
        }
        let index = PROVIDERS.iter().position(|&id| id == provider)?;
        Some(settings.enabled?[index])
    }

    pub fn toggle_sound_download_for_capture(
        &mut self,
        provider: ProviderId,
        cx: &mut Context<Self>,
    ) {
        if let Some(index) = PROVIDERS.iter().position(|&id| id == provider) {
            self.update_sound_download_settings(Some(index), cx);
        }
    }

    pub fn reveal_sound_downloads_for_capture(&self, cx: &mut Context<Self>) {
        let top = self.sound_download_settings.capture_origin.get();
        let offset = self.scroll.bounds().origin.y + px(10.) - top;
        self.scroll.set_offset(gpui::point(
            px(0.),
            offset.clamp(-self.scroll.max_offset().y, px(0.)),
        ));
        cx.notify();
    }
}
