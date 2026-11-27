use gpui::{
    AnyElement, Context, Div, FontWeight, IntoElement, Role, StyledText, accesskit::Toggled,
    canvas, div, point, prelude::*, px, rgb, size,
};
use guise::{Button, Icon, IconName, Size, Variant};

use crate::{
    playback::Status,
    playlist::PlaylistKey,
    ui::{
        assets::provider_mark,
        components::{
            Activate, button, entity_link, glass_control::Tone, music::compact_control,
            scrollbar::ScrollSurface,
        },
        design_system::{
            controls::{self, ControlState, ControlVariant},
            icons,
        },
        fluid_glass::artwork::Seed,
        labels::track_count,
        music::{
            detail::hero_motion::{HERO_ARTWORK_SPAN, cover_span, shared_artwork},
            detail::sticky_hero::{
                CollapsingMediaHero, HeroHeader, blend_hero_artwork, scroll_to_hero_top,
            },
            detail_style,
        },
        theme::{PRIMARY, playlist::*},
        workspace::pins::pin_of_target,
    },
};

use super::{
    Content, MainView, OpenPlaylist,
    actions::QueueAction,
    artwork::{collection_cover, playlist_cover, playlist_hero_cover, sleeve},
    labels::status_label,
    page::Layout,
    popup::MenuKind,
};

#[cfg(all(windows, test, feature = "capture"))]
mod scroll_effects;

impl MainView {
    pub(super) fn playlist_hero(
        &self,
        open: &OpenPlaylist,
        layout: Layout,
        collapsing: Option<CollapsingMediaHero>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let playlist = open.playlist();
        let title_text =
            playlist.map_or(open.target.name.as_str(), |playlist| playlist.name.as_str());
        let art = collection_cover(open);
        let controls = self.glass_controls(cx);
        let glass = controls.is_some();
        let high_contrast = self.fluid_high_contrast(cx);
        let ink = detail_style::Ink::new(glass, high_contrast);
        let provider = open.target.key.provider();
        let local = match open.target.key {
            PlaylistKey::Local(id) => Some(id),
            _ => None,
        };
        let fallback = Seed::collection(&open.target.key);
        let candidates = self.playlist_artwork_candidates(&open.target.key);
        let add_files = local.filter(|_| open.is_empty_local());
        let playback = &self.player_snapshot;
        let current = playback.key.as_ref() == Some(&open.target.key);
        let status = current.then_some(&playback.status);
        let has_audio = open.visible_audio;
        let playing = matches!(status, Some(Status::Playing));
        let paused = matches!(status, Some(Status::Paused));
        let loading = matches!(status, Some(Status::Loading));
        let needs_connection =
            matches!(&open.content, Content::Error(error) if error.needs_sign_in);
        let play_label = if playing {
            "Пауза"
        } else if loading {
            "Загрузка…"
        } else if glass && paused {
            "Продолжить"
        } else if has_audio {
            "Слушать"
        } else {
            "Воспроизведение недоступно"
        };
        let play_icon = if playing {
            IconName::Pause
        } else if loading {
            IconName::LoaderCircle
        } else {
            IconName::Play
        };
        let play_enabled = (has_audio || playing) && !loading;
        let play = Button::new("playlist-play", play_label)
            .variant(Variant::Filled)
            .color(gpui::Hsla::from(rgb(SIGNAL)))
            .radius(Size::Sm)
            .size(if layout.compact || layout.cover_column {
                Size::Sm
            } else {
                Size::Md
            })
            .tab_order(30)
            .left_section(Icon::new(play_icon).pixel_size(px(16.)))
            .disabled(!play_enabled)
            .when(!play_enabled, |button| {
                button.accessible_description("Недоступно")
            })
            .when(play_enabled, |button| {
                button.on_click(cx.listener(|view, _, _, cx| view.activate_playlist(cx)))
            });
        let play = if glass {
            let state = if loading {
                icons::GlyphState::Loading
            } else if play_enabled {
                icons::GlyphState::Rest
            } else {
                icons::GlyphState::Disabled
            };
            let render_play = if collapsing.is_some() {
                icons::hero_play_icon
            } else {
                icons::hero_play
            };
            let edge = collapsing.map_or_else(
                || {
                    if layout.compact || layout.cover_column {
                        44.
                    } else {
                        56.
                    }
                },
                |hero| hero.play_size(),
            );
            render_play("playlist-play", play_label, playing, edge, 30, state, cx)
                .when(play_enabled, |button| {
                    button.on_activate(cx.listener(|view, _, _, cx| view.activate_playlist(cx)))
                })
                .into_any_element()
        } else {
            play.into_any_element()
        };
        let shuffle = compact_control(
            "playlist-shuffle",
            "Перемешать и слушать",
            IconName::Shuffle,
            31,
            has_audio,
            false,
        )
        .when_some(controls, |shuffle, material| {
            shuffle.surface_style(ControlVariant::SecondaryButton.surface(
                material,
                ControlState {
                    disabled: !has_audio,
                    ..Default::default()
                },
            ))
        })
        .when(collapsing.is_some(), |shuffle| {
            shuffle.pixel_size(px(metrics::CONTROL))
        });
        let shuffle = if has_audio {
            shuffle.on_click(
                cx.listener(|view, _, _, cx| view.playlist_queue(QueueAction::Shuffle, None, cx)),
            )
        } else {
            shuffle
        };
        let removed = self.playlist.projected_removed_track(open);
        let removed_duration = playlist
            .and_then(|playlist| playlist.tracks.get(removed?))
            .and_then(|track| track.duration)
            .map_or(0, u64::from);
        let duration = open
            .duration()
            .map(|duration| duration.saturating_sub(removed_duration));
        let count = playlist.map_or(0, |playlist| {
            playlist.tracks.len() - usize::from(removed.is_some())
        });
        let progress = open.load_progress();
        let stage = layout.fluid_hero.is_some();
        let meta = playlist
            .filter(|_| collapsing.is_none() || progress.is_none())
            .map(|_| {
                let mut meta = Vec::new();
                if collapsing.is_none()
                    && let Some(provider) = provider
                {
                    meta.push(provider.display_name().to_owned());
                }
                meta.push(progress.as_ref().map_or_else(
                    || track_count(count),
                    |loaded| format!("Загружено треков: {loaded}"),
                ));
                if let Some(seconds) = duration.filter(|_| count > 0) {
                    meta.push(if stage {
                        detail_style::duration_label(seconds)
                    } else if seconds >= 3600 {
                        format!("{} ч {} мин", seconds / 3600, seconds / 60 % 60)
                    } else {
                        format!("{} мин {} с", seconds / 60, seconds % 60)
                    });
                }
                meta.join(if stage { " · " } else { "  ·  " })
            });
        let stage_byline = self.playlist_owner_name(open);
        let owner = super::navigation::owner(open);
        let motion = self.music_hero_motion();
        let ledger = (layout.fluid_ledger && playlist.is_some()).then(|| {
            let source = if local.is_some() {
                "Локальные файлы".to_owned()
            } else {
                provider
                    .map(|provider| provider.display_name().to_owned())
                    .unwrap_or_else(|| "Плейлист".to_owned())
            };
            detail_style::catalogue(
                vec![
                    ("ИСТОЧНИК", source),
                    progress.as_ref().map_or_else(
                        || ("ТРЕКИ", count.to_string()),
                        |loaded| ("ЗАГРУЖЕНО", loaded.clone()),
                    ),
                    (
                        "ДЛИТЕЛЬНОСТЬ",
                        duration
                            .filter(|_| count > 0)
                            .map(detail_style::duration_label)
                            .unwrap_or_else(|| "—".to_owned()),
                    ),
                ],
                ink,
            )
        });
        let title = playlist_title(title_text, layout, provider.filter(|_| !glass), ink.primary);
        let pin = pin_of_target(&open.target).with_title(title_text);
        let pinned = self.is_pinned(&pin.id);
        let pin_status = self.pin_status(&pin.id);
        let pin_label = if pinned {
            "Открепить"
        } else {
            "Закрепить"
        };
        let pin_toggle = button("pin-toggle", pin_label, 42)
            .when_some(pin_status, |pin, status| pin.aria_description(status))
            .aria_toggled(if pinned {
                Toggled::True
            } else {
                Toggled::False
            })
            .h(px(if layout.compact { 30. } else { 34. }))
            .px(px(8.))
            .gap(px(7.))
            .rounded(px(RADIUS))
            .border_color(rgb(RULE))
            .text_color(rgb(INK_SECONDARY))
            .when(collapsing.is_some(), |pin| {
                pin.size(px(metrics::CONTROL))
                    .px(px(0.))
                    .justify_center()
                    .flex_none()
            })
            .when(!glass, |pin| pin.hover(|style| style.bg(rgb(HOVER))))
            .when_some(controls, |pin, material| {
                material.paint(pin, if pinned { Tone::Selected } else { Tone::Quiet })
            })
            .child(if glass {
                icons::animated_glyph(
                    "playlist-pin-icon",
                    icons::Glyph::Pin,
                    pinned,
                    18.,
                    None,
                    cx,
                )
                .into_any_element()
            } else {
                Icon::new(if pinned {
                    IconName::PinOff
                } else {
                    IconName::Pin
                })
                .pixel_size(px(16.))
                .color(gpui::Hsla::from(rgb(if pinned {
                    SIGNAL
                } else {
                    INK_SECONDARY
                })))
                .into_any_element()
            })
            .when(layout.cover_column || collapsing.is_some(), |pin| {
                pin.tooltip(guise::tooltip(pin_status.unwrap_or(pin_label)))
            })
            .when(!layout.cover_column && collapsing.is_none(), |pin| {
                pin.child(div().text_size(px(13.)).child(pin_label))
            })
            .on_activate(cx.listener(move |view, _, _, cx| {
                view.toggle_pin(pin.clone(), cx);
            }));
        let more = self.playlist_menu_trigger(
            "playlist-more",
            "Действия плейлиста",
            IconName::Ellipsis,
            32,
            MenuKind::Playlist,
            cx,
        );
        if let Some(hero) = collapsing {
            const GAP: f32 = 8.;
            let show_play = !needs_connection || playing || loading;
            let action_width = 2. * metrics::CONTROL
                + GAP
                + if show_play {
                    hero.play_size() + metrics::CONTROL + 2. * GAP
                } else {
                    0.
                };
            let secondary = vec![
                shuffle.into_any_element(),
                pin_toggle.into_any_element(),
                more.into_any_element(),
            ];
            let actions = div()
                .flex()
                .items_center()
                .gap(px(GAP))
                .when(show_play, |row| row.child(play));
            let actions = if let Some(material) = controls {
                actions.child(controls::action_cluster(
                    "playlist-hero-secondary-actions",
                    material,
                    true,
                    secondary,
                ))
            } else {
                actions.children(secondary)
            };
            let edge = hero.artwork_size();
            let radius = hero.artwork_radius();
            let art = blend_hero_artwork(
                cover_span(("playlist-hero", fallback)),
                (art.map(str::to_owned), candidates.to_vec()),
                !self.reduced_motion && !high_contrast,
                move |(url, candidates), _| {
                    playlist_hero_cover(url.as_deref(), candidates, fallback, edge, radius)
                        .into_any_element()
                },
            );
            let art = shared_artwork(
                div()
                    .id("playlist-hero-artwork")
                    .accessibility_id("playlist-hero-artwork")
                    .role(Role::Image)
                    .aria_label("Обложка плейлиста")
                    .relative()
                    .size(px(edge))
                    .flex_none()
                    .child(art),
                HERO_ARTWORK_SPAN,
                motion,
            );
            let kind = if local.is_some() {
                "Локальный плейлист"
            } else {
                "Плейлист"
            };
            let byline = stage_byline.or_else(|| provider.map(|provider| provider.display_name()));
            let subtitle = if let Some((name, destination)) = stage_byline.zip(owner) {
                div()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .children(progress.as_ref().map(|loaded| {
                        let label = format!("Загружено {loaded}");
                        div()
                            .id("playlist-load-progress")
                            .accessibility_id("playlist-load-progress")
                            .role(Role::Status)
                            .aria_label(label.clone())
                            .flex_none()
                            .child(format!("{label} ·"))
                    }))
                    .child(entity_link::text(
                        div().min_w_0().truncate(),
                        "playlist-owner",
                        name,
                        Some(destination),
                        29,
                        cx,
                    ))
                    .when(progress.is_none(), |line| {
                        line.child(div().flex_none().child(format!("· {kind}")))
                    })
                    .into_any_element()
            } else if let Some(loaded) = &progress {
                let status = format!("Загружено {loaded}");
                let label =
                    byline.map_or_else(|| status.clone(), |owner| format!("{status} · {owner}"));
                div()
                    .id("playlist-load-progress")
                    .accessibility_id("playlist-load-progress")
                    .role(Role::Status)
                    .aria_label(label.clone())
                    .child(label)
                    .into_any_element()
            } else {
                let label =
                    byline.map_or_else(|| kind.to_owned(), |owner| format!("{owner} · {kind}"));
                div().child(label).into_any_element()
            };
            let subtitle = div()
                .flex()
                .items_center()
                .min_w_0()
                .gap(px(8.))
                .children(provider.map(|provider| {
                    div()
                        .id("playlist-source-logo")
                        .accessibility_id("playlist-source-logo")
                        .role(Role::Image)
                        .aria_label(provider.display_name())
                        .flex_none()
                        .tooltip(guise::tooltip(provider.display_name()))
                        .child(provider_mark(provider, 14.))
                }))
                .child(div().flex_1().min_w_0().truncate().child(subtitle));
            let facts = div().children(meta);
            let surface = ScrollSurface::VirtualList(self.playlist.list.clone());
            return hero
                .render(
                    HeroHeader {
                        id: "playlist-media-hero",
                        title_id: "playlist-title",
                        title: title_text,
                        subtitle: Some(subtitle.into_any_element()),
                        width: layout.width,
                        margin: layout.margin,
                        action_width,
                        high_contrast,
                    },
                    art,
                    actions,
                    facts,
                    move |_, window, cx| scroll_to_hero_top(surface.clone(), window, cx),
                    cx,
                )
                .into_any_element();
        }
        div()
            .when(!layout.cover_column, |hero| {
                hero.pt(px(if layout.compact { 6. } else { 24. }))
                    .pb(px(if layout.compact { 5. } else { 26. }))
            })
            .when_some(layout.fluid_hero, |hero, metrics| {
                hero.py(px(metrics.vertical_inset))
            })
            .w_full()
            .relative()
            .flex()
            .when(layout.compact || glass, |hero| hero.items_center())
            .when(!layout.compact && !layout.cover_column && !glass, |hero| {
                hero.items_end()
            })
            .gap(px(if layout.compact { 14. } else { 32. }))
            .when_some(layout.fluid_hero, |hero, metrics| hero.gap(px(metrics.gap)))
            .when(layout.cover_column, |hero| {
                hero.flex_col().items_start().gap(px(22.))
            })
            .child(shared_artwork(
                div()
                    .id("playlist-hero-artwork")
                    .accessibility_id("playlist-hero-artwork")
                    .role(Role::Image)
                    .aria_label("Обложка плейлиста")
                    .relative()
                    .flex_none()
                    .child(if let Some(metrics) = layout.fluid_hero {
                        playlist_cover(
                            art,
                            candidates,
                            fallback,
                            metrics.cover,
                            (metrics.cover * 0.035).clamp(8., 16.),
                        )
                    } else {
                        sleeve(art, title_text, layout.cover)
                    }),
                HERO_ARTWORK_SPAN,
                motion,
            ))
            .child(
                div()
                    .when_some(motion, |details, motion| details.opacity(motion.opacity()))
                    .when(!layout.cover_column, |details| details.flex_1())
                    .when(layout.cover_column, |details| details.w_full())
                    .when(layout.fluid_ledger, |details| details.max_w(px(730.)))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(if layout.compact { 2. } else { 8. }))
                    .when(glass, |details| {
                        details
                            .gap(px(if layout.compact { 5. } else { 10. }))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .text_size(px(if layout.compact { 10. } else { 11. }))
                                    .line_height(px(if layout.compact { 12. } else { 14. }))
                                    .font_weight(FontWeight::MEDIUM)
                                    .letter_spacing(if layout.compact { 0.12 } else { 0.18 })
                                    .text_color(ink.secondary)
                                    .when(!layout.compact, |row| {
                                        row.child(
                                            div()
                                                .w(px(20.))
                                                .h(px(1.))
                                                .flex_none()
                                                .bg(ink.secondary.opacity(0.55)),
                                        )
                                    })
                                    .children(provider.map(|provider| {
                                        div()
                                            .id("playlist-source-logo")
                                            .accessibility_id("playlist-source-logo")
                                            .role(Role::Image)
                                            .aria_label(provider.display_name())
                                            .child(provider_mark(
                                                provider,
                                                if layout.compact { 12. } else { 14. },
                                            ))
                                    }))
                                    .child(
                                        (if local.is_some() {
                                            "Локальный плейлист"
                                        } else {
                                            "Плейлист"
                                        })
                                        .to_uppercase(),
                                    ),
                            )
                    })
                    .child(div().max_w(px(820.)).child(title))
                    .when_some(
                        stage_byline.filter(|_| stage || owner.is_some()),
                        |details, name| {
                            details.child(entity_link::text(
                                div()
                                    .text_size(px(if layout.compact { 12. } else { 16. }))
                                    .line_height(px(if layout.compact { 16. } else { 22. }))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(ink.primary.opacity(if layout.compact {
                                        0.82
                                    } else {
                                        0.90
                                    }))
                                    .line_clamp(1)
                                    .text_ellipsis(),
                                "playlist-owner",
                                name,
                                owner,
                                29,
                                cx,
                            ))
                        },
                    )
                    .when(!layout.fluid_ledger, move |details| {
                        details.children(meta.map(|meta| {
                            div()
                                .text_size(px(if layout.compact { 12. } else { 14. }))
                                .text_color(ink.secondary)
                                .child(meta)
                        }))
                    })
                    .child({
                        let actions = div()
                            .pt(px(if stage {
                                if layout.compact { 2. } else { 12. }
                            } else if layout.compact {
                                1.
                            } else {
                                5.
                            }))
                            .flex()
                            .when(layout.cover_column || glass, |actions| actions.flex_wrap())
                            .items_center()
                            .gap(px(8.))
                            .when_some(add_files, |actions, id| {
                                actions.child(
                                    Button::new("playlist-add-files", "Добавить файлы")
                                        .variant(Variant::Filled)
                                        .color(gpui::Hsla::from(rgb(SIGNAL)))
                                        .radius(Size::Sm)
                                        .size(Size::Sm)
                                        .tab_order(30)
                                        .left_section(Icon::new(IconName::Plus).pixel_size(px(16.)))
                                        .when_some(controls, |button, material| {
                                            ControlVariant::DialogAction.dialog_button(
                                                button.left_section(detail_style::primary_icon(
                                                    IconName::Plus,
                                                )),
                                                material,
                                            )
                                        })
                                        .disabled(self.playlist.local_busy)
                                        .on_click(cx.listener(move |view, _, _, cx| {
                                            view.add_files_to_playlist(id, cx)
                                        })),
                                )
                            })
                            .when(
                                add_files.is_none() && (!needs_connection || playing || loading),
                                |actions| actions.child(play),
                            );
                        if add_files.is_none() {
                            let secondary = vec![
                                shuffle.into_any_element(),
                                pin_toggle.into_any_element(),
                                more.into_any_element(),
                            ];
                            match controls {
                                Some(material) => actions.child(controls::action_cluster(
                                    "playlist-hero-secondary-actions",
                                    material,
                                    layout.compact,
                                    secondary,
                                )),
                                None => actions.children(secondary),
                            }
                        } else {
                            actions.child(pin_toggle).child(more)
                        }
                    })
                    .children(self.playlist_hero_notices(open, cx)),
            )
            .children(ledger)
            .into_any_element()
    }

    pub(super) fn playlist_hero_notices(
        &self,
        open: &OpenPlaylist,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let playlist = open.playlist();
        let play_status = &self.player_snapshot.status;
        let description = self.playlist.notice.as_deref().or_else(|| {
            if self.player_snapshot.key.as_ref() == Some(&open.target.key)
                && let Status::Failed(error) = play_status
            {
                Some(error.as_str())
            } else if open
                .snapshot()
                .is_some_and(|snapshot| !snapshot.playlist.tracks.is_empty() && !snapshot.has_audio)
            {
                Some("Источник не предоставил доступ к аудио для этих треков.")
            } else {
                None
            }
        });
        let import = match open.target.key {
            PlaylistKey::Local(id) => self.playlist_operation_status(Some(id), cx),
            _ => None,
        };
        let favorite_error =
            playlist.is_some() && self.playlist.favorites.read_failed(&open.target.key);
        if description.is_none() && import.is_none() && !favorite_error {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(description.map(|text| status_label("playlist-notice", text.to_owned())))
                .children(import)
                .when(favorite_error, |notices| {
                    let key = open.target.key.clone();
                    notices.child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(px(8.))
                            .child(status_label(
                                "playlist-favorites-error",
                                "Не удалось прочитать избранное.",
                            ))
                            .child(
                                div()
                                    .flex_none()
                                    .debug_selector(|| "playlist-favorites-retry".to_owned())
                                    .child(
                                        Button::new("playlist-favorites-retry", "Повторить")
                                            .variant(Variant::Default)
                                            .tab_order(34)
                                            .accessible_name("Повторить чтение избранного")
                                            .on_click(cx.listener(move |view, _, _, cx| {
                                                view.retry_playlist_favorites(&key, cx);
                                            })),
                                    ),
                            ),
                    )
                }),
        )
    }
}

fn playlist_title(
    title: &str,
    presentation: Layout,
    provider: Option<crate::providers::ProviderId>,
    ink: gpui::Hsla,
) -> impl IntoElement {
    let font_size = presentation.title_size;
    let line_height = px(font_size * 1.03);
    let text = StyledText::new(title.to_owned());
    let layout = text.layout().clone();
    let end = title.len();
    let edge = px(24.);
    let gap = px(11.);
    div()
        .relative()
        .w_full()
        .min_w_0()
        .child(
            div()
                .id("playlist-title")
                .accessibility_id("playlist-title")
                .role(Role::Heading)
                .aria_label(title.to_owned())
                .w_full()
                .min_w_0()
                .whitespace_normal()
                .font_family(DISPLAY_FACE)
                .text_size(px(font_size))
                .line_height(line_height)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(PRIMARY))
                .when(provider.is_some(), |heading| heading.pr(edge + gap))
                .when(presentation.cover_column && provider.is_none(), |heading| {
                    heading
                        .line_clamp(4)
                        .text_ellipsis()
                        .tooltip(guise::tooltip(title.to_owned()))
                })
                .when_some(presentation.fluid_hero, |heading, metrics| {
                    let lines = if !presentation.compact && title.chars().count() > 70 {
                        3
                    } else {
                        2
                    };
                    detail_style::title(
                        heading,
                        detail_style::title_size(metrics.title, title),
                        lines,
                    )
                    .text_color(ink)
                    .tooltip(guise::tooltip(title.to_owned()))
                })
                .child(text),
        )
        .children(provider.map(|provider| {
            // The title has already been prepainted. Place the logo after the final glyph,
            // including when the last line is much shorter than the longest line.
            canvas(
                move |bounds, window, cx| {
                    let end = layout.position_for_index(end)?;
                    let origin = point(
                        (end.x + gap).min(bounds.right() - edge),
                        end.y + (line_height - edge) / 2.,
                    );
                    let mut logo = div()
                        .id("playlist-source-logo")
                        .accessibility_id("playlist-source-logo")
                        .role(Role::Image)
                        .aria_label(provider.display_name())
                        .size(edge)
                        .child(provider_mark(provider, f32::from(edge)))
                        .into_any_element();
                    logo.prepaint_as_root(origin, size(edge.into(), edge.into()), window, cx);
                    Some(logo)
                },
                |_, logo, window, cx| {
                    if let Some(mut logo) = logo {
                        logo.paint(window, cx);
                    }
                },
            )
            .absolute()
            .inset_0()
        }))
}
