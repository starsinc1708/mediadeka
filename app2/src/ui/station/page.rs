#[cfg(feature = "capture")]
use gpui::Window;
use gpui::{AnyElement, Context, IntoElement, Role, div, prelude::*, px};
use guise::{Button, Variant};

use crate::{playback::Status, playlist::PlaylistKey, providers::ProviderId};

#[cfg(feature = "capture")]
use super::my_wave_target;
use super::{MY_WAVE, MusicTarget, Route};
use crate::ui::{
    MainView,
    assets::provider_mark,
    components::{
        Activate,
        content_state::error_state,
        music::{cover, fluid_cover},
    },
    design_system::{
        controls::ControlVariant,
        icons::{self, Glyph, GlyphState},
    },
    fluid_glass::{
        artwork::Seed,
        tokens::{radius, size, space, text},
    },
    music::{heading, status},
    theme::secondary,
    workspace::pins::pin_of_object,
};

impl MainView {
    #[cfg(feature = "capture")]
    pub fn show_station_for_capture(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.open_music_target(my_wave_target(None), window, cx);
    }

    #[cfg(feature = "capture")]
    pub fn play_station_for_capture(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.open_yandex_wave(None, window, cx);
    }

    #[cfg(feature = "capture")]
    pub fn station_semantics_for_capture(&self) -> serde_json::Value {
        let Route::Station(target) = self.workspace.routes.current() else {
            return serde_json::Value::Null;
        };
        let key = self.station_key(target);
        serde_json::json!({
            "title": station_title(target),
            "provider": target.object.provider.as_str(),
            "pin_available": pin_of_object(target).is_some(),
            "error": self.station.failure.as_ref().filter(|(failed, _)| failed == &key).map(|(_, error)| &error.message),
        })
    }

    pub(in crate::ui) fn station_view(&self, cx: &Context<Self>) -> AnyElement {
        let Route::Station(target) = self.workspace.routes.current() else {
            return div().into_any_element();
        };
        let key = self.station_key(target);
        let title = station_title(target);
        let active = self
            .station
            .active
            .as_ref()
            .filter(|active| active.key == key);
        let loading = self
            .station
            .starting
            .as_ref()
            .is_some_and(|starting| starting.key == key)
            || active.is_some() && self.player_snapshot.status == Status::Loading;
        let playing = active.is_some() && self.player_snapshot.status == Status::Playing;
        let paused = active.is_some() && self.player_snapshot.status == Status::Paused;
        let failure = self
            .station
            .failure
            .as_ref()
            .filter(|(failed, _)| failed == &key)
            .map(|(_, error)| error);
        let provider = target.object.provider;
        let controls = self.glass_controls(cx);
        let needs_sign_in = failure.is_some_and(|failure| failure.needs_sign_in)
            || matches!(
                key,
                PlaylistKey::Station {
                    provider: ProviderId::YandexMusic,
                    profile: None,
                    ..
                }
            );
        let button_label = if loading && controls.is_none() {
            "Подключаем станцию…"
        } else if playing {
            "Пауза"
        } else if paused {
            "Продолжить"
        } else if failure.is_some() {
            "Повторить"
        } else {
            "Слушать"
        };
        let play_target = target.clone();
        let mut pin_target = target.clone();
        pin_target.title = title.clone();
        let pin = pin_of_object(&pin_target);
        let message = failure.map(|error| error.message.as_str()).or_else(|| {
            needs_sign_in.then_some("Для запуска станции подключите аккаунт площадки.")
        });
        div()
            .id("station-page")
            .accessibility_id("station-page")
            .role(Role::Group)
            .aria_label(title.clone())
            .pt(px(space::MD))
            .flex()
            .flex_col()
            .gap(px(space::LG))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(space::LG))
                    .when(controls.is_some(), |header| {
                        header.child(fluid_cover(
                            target.artwork.as_deref(),
                            Some(Seed::object(&target.object)),
                            88.,
                            radius::CONTROL,
                        ))
                    })
                    .when(controls.is_none(), |header| {
                        header.children(
                            target
                                .artwork
                                .as_deref()
                                .map(|url| cover(Some(url), 88., 6., false)),
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(space::SM))
                                    .text_size(px(text::METADATA))
                                    .text_color(secondary())
                                    .child(provider_mark(target.object.provider, 18.))
                                    .child(target.object.provider.display_name()),
                            )
                            .child(
                                heading("station-title", title)
                                    .text_size(px(if controls.is_some() {
                                        text::PAGE_TITLE
                                    } else {
                                        36.
                                    }))
                                    .line_clamp(2),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(space::SM))
                    .children((!needs_sign_in || loading || playing || paused).then(|| {
                        let activate = cx.listener(move |view, _: &(), _, cx| {
                            if playing || paused {
                                view.player.update(cx, |bar, cx| {
                                    bar.command(cx, |playback| playback.toggle_pause())
                                });
                            } else {
                                view.start_station(play_target.clone(), cx);
                            }
                        });
                        if controls.is_some() {
                            icons::hero_play(
                                "station-play",
                                button_label,
                                playing,
                                size::LG,
                                32,
                                if loading {
                                    GlyphState::Loading
                                } else {
                                    GlyphState::Rest
                                },
                                cx,
                            )
                            .when(!loading, |button| button.on_activate(activate))
                            .into_any_element()
                        } else {
                            Button::new("station-play", button_label)
                                .variant(Variant::Filled)
                                .tab_order(32)
                                .disabled(loading)
                                .when(!loading, |button| {
                                    button.on_click(move |_, window, cx| activate(&(), window, cx))
                                })
                                .into_any_element()
                        }
                    }))
                    .children((needs_sign_in && !loading).then(|| {
                        let button = self
                            .provider_recovery_action("station-connect", provider, 35, cx)
                            .variant(Variant::Filled);
                        button.when_some(controls, |button, material| {
                            ControlVariant::DialogAction.dialog_button(button, material)
                        })
                    }))
                    .children(pin.map(|pin| {
                        let pinned = self.is_pinned(&pin.id);
                        let label = if pinned {
                            "Открепить"
                        } else {
                            "Закрепить"
                        };
                        let notice = self.pin_status(&pin.id);
                        if controls.is_some() {
                            icons::secondary_action(
                                "station-pin",
                                label,
                                Some(Glyph::Pin),
                                pinned,
                                33,
                                true,
                                cx,
                            )
                            .when_some(notice, |button, status| button.aria_description(status))
                            .on_activate(
                                cx.listener(move |view, _, _, cx| view.toggle_pin(pin.clone(), cx)),
                            )
                            .into_any_element()
                        } else {
                            Button::new("station-pin", label)
                                .variant(Variant::Subtle)
                                .tab_order(33)
                                .when_some(notice, |button, status| {
                                    button.accessible_description(status)
                                })
                                .on_click(cx.listener(move |view, _, _, cx| {
                                    view.toggle_pin(pin.clone(), cx)
                                }))
                                .into_any_element()
                        }
                    }))
                    .when(
                        active.is_some_and(|active| active.radio.is_some()),
                        |toolbar| {
                            toolbar.child(
                                Button::new("station-dislike", "Меньше такого")
                                    .variant(Variant::Subtle)
                                    .tab_order(34)
                                    .on_click(
                                        cx.listener(|view, _, _, cx| {
                                            view.dislike_station_track(cx)
                                        }),
                                    ),
                            )
                        },
                    ),
            )
            .children(message.map(|message| {
                if controls.is_some() && failure.is_some() {
                    error_state("station-status", message.to_owned(), None, cx)
                } else {
                    status("station-status", message.to_owned())
                }
            }))
            .into_any_element()
    }
}

fn station_title(target: &MusicTarget) -> String {
    if !target.title.trim().is_empty() {
        target.title.clone()
    } else if target.object.provider == ProviderId::YandexMusic && target.object.id == MY_WAVE {
        "Моя волна".into()
    } else {
        format!("Станция · {}", target.object.provider.display_name())
    }
}
