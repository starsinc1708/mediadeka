use std::time::Instant;

use gpui::{
    App, Context, Div, FontWeight, Hsla, IntoElement, Role, ScrollWheelEvent, Stateful, Window,
    div, point, prelude::*, px, rgb,
};
use guise::IconName;

use crate::{
    music::lyrics::{Line, Source},
    ui::{
        components::{
            Activate, button,
            content_state::{empty_state, error_state, loading_state, refresh_indicator},
            fluid_current_track_fill,
            music::compact_control,
            overlay::{SelectChoice, SelectLabel},
            scroll_viewport::{Density, FluidScrollViewport},
            scrollbar::ScrollSurface,
        },
        design_system::{
            icons::{self, Glyph},
            materials::{self, Kind, Material},
        },
        fluid_glass::{ShellGeometry, tokens as fluid},
        panel,
        theme::{ACCENT, ACTIVE, primary, secondary},
    },
};

use super::{FullscreenRequested, LyricsView, follow::FollowScroll, karaoke};

const LYRICS_CONTENT_WIDTH: f32 = 820.;
const MODES: [Option<Source>; 7] = [
    None,
    Some(Source::Local),
    Some(Source::YandexMusic),
    Some(Source::Lrclib),
    Some(Source::Spotify),
    Some(Source::YouTubeMusic),
    Some(Source::Genius),
];

impl Render for LyricsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.schedule_frame(window, cx);
        div()
            .id("playback-lyrics")
            .accessibility_id("playback-lyrics")
            .role(Role::Group)
            .aria_label("Текст песни")
            .track_focus(&self.focus)
            .tab_group()
            .tab_index(595)
            .tab_stop(false)
            .w_full()
            .min_h_0()
            .min_w_0()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .when(self.fullscreen, |view| view.pt(px(12.)))
            .child(self.header(window, cx))
            .child(self.lyric_scroll(window, cx))
            .when(self.has_picker_footer(), |view| {
                view.child(self.picker_footer(window, cx))
            })
    }
}

impl LyricsView {
    pub(super) fn accent(&self, cx: &App) -> u32 {
        if !self.fluid {
            ACCENT
        } else if crate::ui::theme::fluid_high_contrast(cx) {
            fluid::contrast::ACCENT
        } else {
            fluid::selection::LIGHT
        }
    }

    fn header(&self, window: &Window, cx: &Context<Self>) -> Div {
        let show_track = !self.fullscreen
            || (!self.fluid && window.viewport_size().width < px(crate::ui::listening::WIDE_TEXT));
        let player_track = self.entry.as_ref().map(|entry| &entry.track);
        div()
            .flex_none()
            .w_full()
            .max_w(px(LYRICS_CONTENT_WIDTH))
            .px(px(if self.fullscreen { 30. } else { 12. }))
            .pb(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .children(player_track.filter(|_| show_track).map(|track| {
                        div()
                            .text_size(px(if self.fullscreen { 22. } else { 14. }))
                            .line_height(px(if self.fullscreen { 28. } else { 19. }))
                            .font_weight(FontWeight::SEMIBOLD)
                            .truncate()
                            .child(track.title.clone())
                    }))
                    .children(player_track.filter(|_| show_track).and_then(|track| {
                        (!track.artist.is_empty()).then(|| {
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(secondary())
                                .child(track.artist.clone())
                        })
                    })),
            )
            .when(
                player_track.is_some() && !self.has_picker_footer(),
                |header| header.child(self.source_picker(window, cx)),
            )
            .when(
                self.current_text().is_some() && self.load.is_some(),
                |header| header.child(refresh_indicator("lyrics-refresh", "Обновляем текст", cx)),
            )
            .when(!self.fullscreen, |header| {
                const ID: &str = "lyrics-fullscreen-toggle";
                const LABEL: &str = "Текст во весь экран";
                let expand = cx.listener(|_, _: &(), _, cx| cx.emit(FullscreenRequested));
                let action = if self.fluid {
                    icons::ghost_icon_button(
                        ID,
                        LABEL,
                        Glyph::Fullscreen,
                        false,
                        fluid::size::SM,
                        561,
                        true,
                        cx,
                    )
                    .on_activate(expand)
                    .into_any_element()
                } else {
                    compact_control(ID, LABEL, IconName::Maximize2, 561, true, self.fullscreen)
                        .on_click(move |_, window, cx| expand(&(), window, cx))
                        .into_any_element()
                };
                header.child(action)
            })
    }

    fn has_picker_footer(&self) -> bool {
        self.fluid && !self.fullscreen && self.entry.is_some()
    }

    fn picker_footer(&self, window: &Window, cx: &Context<Self>) -> Div {
        div()
            .w_full()
            .max_w(px(LYRICS_CONTENT_WIDTH))
            .flex_none()
            .when(!panel::inline(window), |footer| {
                let shell = ShellGeometry::for_window(window);
                let content = shell.content_bounds();
                let safe = shell.safe_content_bounds();
                // The workspace already reserves part of the bottom inset.
                let bottom = content.bottom() - safe.bottom() - px(shell.base().content_bottom());
                footer
                    .px(safe.left() - content.left())
                    .pb(bottom.max(px(0.)))
            })
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(5.))
                    .border_t_1()
                    .border_color(crate::ui::theme::surface_border())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(secondary())
                            .child("Источник"),
                    )
                    .child(self.source_picker(window, cx)),
            )
    }

    fn source_picker(&self, window: &Window, cx: &Context<Self>) -> Div {
        let contrast = crate::ui::theme::fluid_high_contrast(cx);
        let material = self.fluid.then(|| {
            Material::new(Kind::SmokedGlass, materials::accent(cx)).high_contrast(contrast)
        });
        let plain_source = self.plain_source();
        let plain_selected = self.display_plain && plain_source.is_some();
        let plain_label = plain_source.map(|source| format!("{} · Обычный текст", source.label()));
        let synchronized_label =
            plain_source.map(|source| format!("{} · Синхронный текст", source.label()));
        let plain_index = MODES
            .iter()
            .position(|source| source.is_some() && *source == plain_source)
            .map(|index| index + 1);
        let mut choices: Vec<_> = MODES
            .into_iter()
            .enumerate()
            .map(|(index, source)| {
                SelectChoice::new(
                    format!("lyrics-source-{index}"),
                    if source.is_some() && source == plain_source {
                        synchronized_label.clone().unwrap_or_default()
                    } else {
                        source_label(source).to_owned()
                    },
                    !plain_selected && self.selection == source,
                )
            })
            .collect();
        if let (Some(index), Some(label)) = (plain_index, &plain_label) {
            choices.insert(
                index,
                SelectChoice::new("lyrics-source-plain", label.clone(), plain_selected),
            );
        }
        self.picker.view(
            SelectLabel {
                trigger: "lyrics-source",
                menu: "lyrics-sources",
                label: if plain_source.is_some() {
                    "Источник и версия текста"
                } else {
                    "Источник текста"
                },
                value: plain_label
                    .filter(|_| plain_selected)
                    .or_else(|| synchronized_label.filter(|_| self.selection == plain_source))
                    .unwrap_or_else(|| source_label(self.selection).to_owned())
                    .into(),
                enabled: true,
            },
            choices,
            |view| &mut view.picker,
            move |view, index, window, cx| {
                if Some(index) == plain_index {
                    view.choose_plain(window, cx);
                } else {
                    let source =
                        index - usize::from(plain_index.is_some_and(|plain| index > plain));
                    if let Some(source) = MODES.get(source) {
                        view.choose_source(*source, window, cx);
                    }
                }
            },
            material,
            self.fluid && !cx.reduce_motion() && !contrast,
            window,
            cx,
        )
    }

    fn lyric_scroll(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let text = self.current_text();
        let seekable = self.seekable;
        let compact = !self.fullscreen && window.viewport_size().height < px(500.);
        let state = self.lyrics_state(compact, cx);
        let row_size = if self.fullscreen {
            if window.viewport_size().height < px(500.) {
                22.
            } else {
                35.
            }
        } else {
            15.
        };
        let row_pad = if self.fullscreen { 12. } else { 7. };
        div()
            .id("lyrics-scroll")
            .accessibility_id("lyrics-scroll")
            .w_full()
            .max_w(px(LYRICS_CONTENT_WIDTH))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .relative()
            .px(px(if self.fullscreen { 22. } else { 9. }))
            .pb(px(if compact { 4. } else { 12. }))
            .when(!self.follow, |list| {
                const LABEL: &str = "К текущей строке";
                let action = if self.fluid {
                    icons::text_action(
                        "lyrics-follow",
                        LABEL,
                        None,
                        false,
                        fluid::text::CAPTION,
                        571,
                        true,
                        cx,
                    )
                    .self_start()
                } else {
                    button("lyrics-follow", LABEL, 571)
                        .w_full()
                        .h(px(25.))
                        .px(px(7.))
                        .text_size(px(11.))
                        .text_color(rgb(self.accent(cx)))
                        .child(LABEL)
                };
                list.child(action.on_activate(cx.listener(|view, _, window, cx| {
                    view.picker.focus_trigger(window, cx);
                    view.follow_current_line(cx);
                })))
            })
            .flex()
            .flex_col()
            .child(FluidScrollViewport::new(
                div()
                    .id("lyrics-body")
                    .w_full()
                    .min_h_0()
                    .min_w_0()
                    .flex_1()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .on_scroll_wheel(cx.listener(|view, _: &ScrollWheelEvent, _, cx| {
                        if view.follow && view.highlight.line.is_some() {
                            view.follow = false;
                            view.follow_scroll = FollowScroll::default();
                            cx.notify();
                        }
                    }))
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .children(
                        text.into_iter()
                            .flat_map(|text| text.lines.iter())
                            .enumerate()
                            .map(|(index, line)| {
                                self.lyric_line(index, line, seekable, row_size, row_pad, cx)
                            }),
                    )
                    .children(state.map(|state| div().my_auto().child(state)))
                    .children(
                        text.and_then(|text| text.attribution.as_ref())
                            .map(|label| {
                                div()
                                    .flex_none()
                                    .px(px(7.))
                                    .mt(px(6.))
                                    .text_size(px(11.))
                                    .text_color(secondary())
                                    .child(label.clone())
                            }),
                    )
                    .children(self.genius_footer(cx)),
                ScrollSurface::Area(self.scroll.clone()),
                self.fluid,
                Density::Panel,
            ))
    }

    fn lyrics_state(&self, compact: bool, cx: &Context<Self>) -> Option<Stateful<Div>> {
        const ID: &str = "lyrics-status";
        let text = self.current_text();
        if self.entry.is_none() {
            return Some(empty_state(
                ID,
                "Включите трек, чтобы увидеть текст",
                None,
                IconName::Captions,
                cx,
            ));
        }
        if text.is_some_and(|text| text.instrumental) {
            return Some(empty_state(
                ID,
                "Инструментальная запись",
                None,
                IconName::Music2,
                cx,
            ));
        }
        if text.is_some_and(|text| !text.lines.is_empty()) {
            return None;
        }
        if self.load.is_some() || self.genius.is_pending() {
            return Some(loading_state(ID, "Ищем текст…", cx));
        }
        let state = if self.failed {
            error_state(
                ID,
                "Не удалось загрузить текст",
                Some("Попробуйте повторить поиск или сменить источник."),
                cx,
            )
        } else {
            empty_state(
                ID,
                "Текст пока недоступен",
                Some(if compact {
                    "Для этой композиции текст не найден."
                } else {
                    "Для этой композиции не найдено синхронизированных или обычных текстов."
                }),
                IconName::Captions,
                cx,
            )
        };
        let material = self
            .fluid
            .then(|| crate::ui::design_system::controls::material(cx));
        let action = |id, label, order, glyph: IconName| {
            button(id, label, order)
                .h(px(if compact { 25. } else { 27. }))
                .px(px(6.))
                .gap(px(7.))
                .text_size(px(12.))
                .when_some(material, |button, material| {
                    crate::ui::design_system::controls::ControlVariant::TextAction.paint(
                        button,
                        material,
                        Default::default(),
                    )
                })
                .child(guise::Icon::new(glyph).pixel_size(px(14.)))
                .child(label)
        };
        Some(
            state
                .when(compact, |state| state.py(px(4.)).gap(px(4.)))
                .child(
                    action("lyrics-retry", "Повторить поиск", 569, IconName::RefreshCw)
                        .on_activate(cx.listener(|view, _, window, cx| {
                            view.picker.focus_trigger(window, cx);
                            view.retry_lyrics(cx);
                        })),
                )
                .child(
                    action(
                        "lyrics-change-source",
                        "Выбрать другой источник",
                        570,
                        IconName::Search,
                    )
                    .track_focus(&self.change_source_focus)
                    .on_activate(cx.listener(|view, _, window, cx| {
                        let selected = MODES
                            .iter()
                            .position(|&mode| mode == view.selection)
                            .unwrap_or(0);
                        view.picker.open_from(
                            selected,
                            Some(view.change_source_focus.clone()),
                            window,
                            cx,
                        );
                        cx.notify();
                    })),
                ),
        )
    }

    fn lyric_line(
        &self,
        index: usize,
        line: &Line,
        seekable: bool,
        size: f32,
        pad: f32,
        cx: &Context<Self>,
    ) -> Div {
        let active = self.highlight.line == Some(index);
        let label = line.text.clone();
        let reduced = self.reduced_motion(cx);
        let sweep = active.then(|| self.active_sweep(reduced)).flatten();
        let accent = rgb(self.accent(cx));
        let distance = self
            .highlight
            .line
            .map_or(0, |current| current.abs_diff(index));
        let depth = !self.fluid && self.fullscreen && self.follow && !active && !reduced;
        let contrast = self.fluid && crate::ui::theme::fluid_high_contrast(cx);
        let primary: Hsla = if contrast {
            rgb(fluid::contrast::PRIMARY).into()
        } else if self.fluid {
            rgb(fluid::PRIMARY).into()
        } else {
            primary()
        };
        let tone: Hsla = if contrast {
            rgb(fluid::contrast::SECONDARY).into()
        } else if self.fluid {
            rgb(fluid::SECONDARY).into()
        } else if depth {
            secondary()
                .blend(gpui::Hsla::from(rgb(0x766b62)).alpha((distance.min(5) as f32) * 0.073))
        } else {
            secondary()
        };
        let within_view = self.scroll.bounds_for_item(index).is_some_and(|bounds| {
            let mut onscreen = bounds;
            onscreen.origin += self.scroll.offset();
            onscreen.intersects(&self.scroll.bounds())
        });
        let emphasis = if self.fluid && (within_view || active) {
            self.emphasis.amount(index, Instant::now(), reduced)
        } else {
            f32::from(u8::from(active))
        };
        let content = div()
            .min_w_0()
            .when(
                self.fluid && self.fullscreen && emphasis > 0. && !reduced,
                |content| {
                    content
                        .layer_scale(1. + emphasis * 0.028)
                        .layer_scale_origin(point(0., 0.5))
                },
            )
            .child(karaoke::text(label.clone().into(), sweep, accent.into()));
        let style = |row: Div| {
            row.flex_1()
                .min_w_0()
                .px(px(pad))
                .py(px(pad * 0.75))
                .text_size(px(size))
                .line_height(px(size * 1.36))
                .font_weight(if line.time_ms.is_some() {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                })
        };
        let ink = |row: Stateful<Div>| {
            let contrast_mix = if line.words.is_empty() {
                emphasis
            } else if self.fluid {
                emphasis * 0.35
            } else {
                0.
            };
            row.text_color(tone.blend(primary.alpha(contrast_mix)))
                .when(depth && within_view && distance >= 2, |row| {
                    row.blur(px((distance.min(5) - 1) as f32 * 0.28))
                        .hover(|state| state.blur(px(0.)).text_color(secondary()))
                        .focus(|state| state.blur(px(0.)).text_color(secondary()))
                })
        };
        let row = if let Some(time_ms) = line.time_ms.filter(|_| seekable) {
            style(div())
                .child(
                    ink(button(format!("lyrics-line-{index}"), label, 572))
                        .w_full()
                        .flex_col()
                        .items_start()
                        .when(active, |row| row.aria_description("Текущая строка"))
                        .child(content)
                        .on_activate(
                            cx.listener(move |view, _, _, cx| view.seek_line(time_ms, cx)),
                        ),
                )
                .into_any_element()
        } else {
            ink(style(div())
                .id(("lyrics-line", index))
                .accessibility_id(format!("lyrics-line-{index}"))
                .role(Role::Label)
                .aria_value(label))
            .when(active, |row| row.aria_description("Текущая строка"))
            .child(content)
            .into_any_element()
        };
        div()
            .flex_none()
            .min_w_0()
            .w_full()
            .relative()
            .rounded(px(8.))
            .when(active && !self.fullscreen, |row| {
                if self.fluid {
                    row.bg(fluid_current_track_fill(contrast))
                } else {
                    row.bg(rgb(ACTIVE))
                }
            })
            .when(active && self.fullscreen, |row| {
                row.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(10.))
                        .bottom(px(10.))
                        .w(px(2.))
                        .rounded(px(1.))
                        .bg(accent),
                )
            })
            .flex()
            .flex_col()
            .child(
                div()
                    .w_full()
                    .min_w_0()
                    .flex()
                    .items_start()
                    .child(row)
                    .child(self.notes_for_line(index, cx)),
            )
            .children(self.annotation_body(index))
    }
}

fn source_label(source: Option<Source>) -> &'static str {
    source.map_or("Автоматически", Source::label)
}
