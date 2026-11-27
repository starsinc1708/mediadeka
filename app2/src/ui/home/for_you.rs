use std::collections::HashSet;

use gpui::{
    Context, Div, FontWeight, Role, Stateful, div, linear_color_stop, linear_gradient, prelude::*,
    px, relative, rgb,
};
use guise::{Button, Icon, IconName, Size, Variant};

use crate::{
    music::{ObjectKind, ObjectRef},
    providers::ProviderId,
    state::HomeView,
    storage::PanelTab,
    ui::{
        components::glass_control::Tone,
        design_system::{
            controls::{ControlState, ControlVariant},
            icons::{Glyph, animated_glyph},
        },
        fluid_glass::tokens::{SECONDARY, contrast, size, space, text},
        theme::{
            YANDEX_WAVE_BORDER, YANDEX_WAVE_BORDER_HOVER, YANDEX_WAVE_END, YANDEX_WAVE_START,
            secondary,
        },
        workspace::MusicTarget,
    },
};

use super::{
    Activate, FeedSource, Layout, MainView, button,
    content::section_title,
    listening::{Tile, section, tile, tile_base, tiles},
    open_track,
};

const PER_SOURCE: usize = 2;
pub(super) const QUEUE_ORDER: isize = 38;
const QUEUE: &str = "home-for-you-queue";

impl MainView {
    pub(super) fn home_for_you(
        &self,
        visible: &[&FeedSource],
        active: &FeedSource,
        home: &HomeView,
        refreshing: bool,
        layout: Layout,
        cx: &Context<Self>,
    ) -> Option<Stateful<Div>> {
        let wave = visible
            .iter()
            .copied()
            .find(|source| source.provider == ProviderId::YandexMusic);
        let recommendations =
            self.home_recommendation_controls(visible, active, home, refreshing, cx);
        let mut candidates =
            visible
                .iter()
                .flat_map(|source| {
                    let feed = self.home.feeds.get(source, active, home, refreshing);
                    let mut seen = HashSet::new();
                    feed.snapshot()
                        .into_iter()
                        .flat_map(move |snapshot| {
                            snapshot.home.items.iter().enumerate().filter_map(
                                move |(index, item)| {
                                    let object = ObjectRef::from_item(source.provider, item)?;
                                    (matches!(object.kind, ObjectKind::Station)
                                        || object.kind == ObjectKind::Playlist
                                            && snapshot.home.personal_mix(source.provider, index))
                                    .then_some((*source, item, object))
                                },
                            )
                        })
                        .filter(move |(_, _, object)| seen.insert(object.clone()))
                        .take(PER_SOURCE)
                })
                .peekable();
        let status = self.home.mix.status(layout.glass.is_some(), cx);
        if recommendations.is_none()
            && wave.is_none()
            && candidates.peek().is_none()
            && status.is_none()
        {
            return None;
        }
        let bar = self.player.read(cx);
        let next = bar
            .current_index()
            .and_then(|index| bar.entries().nth(index + 1));
        let queue_count = bar.queue_len();
        let queue_label = format!("Очередь · {queue_count}");
        let wave = wave.map(|source| {
            let profile = source.profile.clone();
            let wave = tile_base(
                "home-yandex-wave",
                "Запустить Мою волну в Mediadeka",
                layout,
                Tile {
                    title: "Моя волна",
                    detail: "Слушать станцию",
                    artwork: None,
                    fallback: None,
                    provider: Some(source.provider),
                    label_elements: None,
                },
            );
            if let Some(material) = layout.glass {
                material.paint(wave, Tone::Outlined)
            } else {
                wave.border_color(rgb(YANDEX_WAVE_BORDER))
                    .bg(linear_gradient(
                        112.,
                        linear_color_stop(rgb(YANDEX_WAVE_START), 0.),
                        linear_color_stop(rgb(YANDEX_WAVE_END), 1.),
                    ))
                    .hover(|style| style.border_color(rgb(YANDEX_WAVE_BORDER_HOVER)))
            }
            .on_activate(cx.listener(move |view, _, window, cx| {
                view.open_yandex_wave(profile.clone(), window, cx);
                view.open_panel(PanelTab::Queue, false, window, cx);
            }))
        });
        let cards =
            candidates
                .enumerate()
                .map(|(index, (source, item, object))| {
                    let source = source.clone();
                    let item = item.clone();
                    let kind = object.kind;
                    let target = MusicTarget {
                        object,
                        profile: source.profile.clone(),
                        title: item.title.clone(),
                        artwork: item.artwork.clone(),
                    };
                    tile(
                        format!("home-for-you-{index}"),
                        format!("{} · {}", kind.label(), item.title),
                        layout,
                        Tile {
                            title: &item.title,
                            detail: if kind == ObjectKind::Station {
                                "Слушать станцию"
                            } else {
                                "Персональный микс"
                            },
                            artwork: item.artwork.as_deref(),
                            fallback: None,
                            provider: Some(source.provider),
                            label_elements: None,
                        },
                    )
                    .on_activate(cx.listener(move |view, _, window, cx| {
                        if kind == ObjectKind::Station {
                            if view.state.home_view().is_some_and(|(home, _)| {
                                FeedSource::available(home).contains(&source)
                            }) {
                                view.open_and_start_station(target.clone(), window, cx);
                                view.open_panel(PanelTab::Queue, false, window, cx);
                            }
                        } else {
                            open_track(&source, item.clone(), view, window, cx);
                        }
                    }))
                });
        let preview = next.map(|entry| format!("Далее · {}", entry.track.title));
        let queue_control = if let Some(material) = layout.glass {
            let open = self.panel_is_open_on_queue();
            let detail = rgb(if material.contrast() {
                contrast::SECONDARY
            } else {
                SECONDARY
            });
            let control = button(QUEUE, queue_label, QUEUE_ORDER)
                .track_focus(&self.home.queue_focus)
                .h(px(size::SM))
                .flex_none()
                .gap(px(space::SM))
                .px(px(space::SM))
                .text_size(px(text::METADATA))
                .child(animated_glyph(QUEUE, Glyph::Queue, open, 16., None, cx))
                .child(div().font_weight(FontWeight::SEMIBOLD).child("Очередь"))
                .child(
                    div()
                        .min_w(px(space::LG))
                        .text_right()
                        .text_color(detail)
                        .child(queue_count.to_string()),
                );
            ControlVariant::TextAction
                .paint(
                    control,
                    material,
                    ControlState {
                        active: open,
                        expanded: open,
                        ..ControlState::default()
                    },
                )
                .on_toggle(
                    open,
                    cx.listener(|view, _, window, cx| {
                        if !view.panel_is_open_on_queue() {
                            window.focus(&view.home.queue_focus, cx);
                        }
                        view.toggle_queue_panel_from_home(window, cx);
                    }),
                )
                .into_any_element()
        } else {
            Button::new(QUEUE, queue_label)
                .variant(Variant::Default)
                .size(Size::Xs)
                .left_section(
                    Icon::new(IconName::ListMusic)
                        .pixel_size(px(14.))
                        .into_any_element(),
                )
                .tab_order(QUEUE_ORDER)
                .on_click(cx.listener(|view, _, window, cx| {
                    view.toggle_queue_panel(window, cx);
                }))
                .into_any_element()
        };
        let group = section("home-for-you", "Для вас");
        let title = if layout.glass.is_some() {
            super::fluid::section_heading("Для вас")
        } else {
            section_title("Для вас").into_any_element()
        };
        #[cfg(feature = "capture")]
        let group = group
            .relative()
            .child(self.track_home_section_origin(self.home.for_you_origin.clone()));
        Some(
            group
                .child(
                    div()
                        .flex()
                        .w_full()
                        .gap(px(12.))
                        .items_center()
                        .child(div().flex_1().min_w_0().child(title))
                        .children(preview.map(|title| {
                            queue_preview(title).when_some(layout.glass, |preview, material| {
                                preview.max_w(relative(0.42)).text_color(rgb(
                                    if material.contrast() {
                                        contrast::SECONDARY
                                    } else {
                                        SECONDARY
                                    },
                                ))
                            })
                        }))
                        .child(queue_control),
                )
                .children(recommendations)
                .children(status)
                .child(tiles().children(wave).children(cards)),
        )
    }
}

fn queue_preview(title: String) -> Stateful<Div> {
    div()
        .id("home-for-you-next")
        .accessibility_id("home-for-you-next")
        .role(Role::Label)
        .aria_value(title.clone())
        .min_w_0()
        .truncate()
        .text_size(px(12.))
        .text_color(secondary())
        .child(title)
}
