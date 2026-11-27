use gpui::{
    Bounds, Context, Div, FontWeight, IntoElement, Pixels, Role, Window, div, point, prelude::*,
    px, rgb, size,
};
use guise::{Icon, IconName, theme};

use crate::providers::ProviderId;

use super::{
    MainView,
    assets::{BRAND_MARKS, mark, provider_index},
    components::{Activate, button, fluid_selection_marker, glass_control::Tone},
    effects::{self, ACCENTS, MotionFrame, Transfer},
    fluid_glass::tokens,
    theme::{ELEVATED, SOUNDCLOUD_TILE, SURFACE},
    welcome_motion::ConnectionFrame,
};

const SPLIT_WIDTH: f32 = 900.;
const INTRO_SPLIT_WIDTH: f32 = 1100.;
const INTRO_SPLIT_HEIGHT: f32 = 460.;
const COLUMN_GAP: f32 = 40.;
const DESKTOP_COLUMN_GAP: f32 = 132.;
const DESKTOP_WIDTH: f32 = 1600.;
const STAGE_MARGIN: f32 = 20.;

pub(super) fn split_layout(width: Pixels) -> bool {
    width >= px(SPLIT_WIDTH)
}

fn compact_pane_height(provider: Option<ProviderId>) -> f32 {
    match provider {
        Some(ProviderId::YouTubeMusic) => 460.,
        Some(ProviderId::Jellyfin | ProviderId::SoundCloud) => 420.,
        Some(ProviderId::YandexMusic) => 323.,
        Some(ProviderId::Spotify) => 286.,
        None => 230.,
    }
}

#[derive(Clone, Copy)]
struct Metrics {
    spaciousness: f32,
}

impl Metrics {
    fn new(content_height: Pixels) -> Self {
        let minimum = super::MIN_SIZE.1 - super::TITLEBAR_HEIGHT;
        let standard = super::INITIAL_SIZE.1 - super::TITLEBAR_HEIGHT;
        Self {
            spaciousness: ((f32::from(content_height) - minimum) / (standard - minimum))
                .clamp(0., 1.),
        }
    }

    fn span(self, minimum: f32, standard: f32) -> Pixels {
        px(minimum + (standard - minimum) * self.spaciousness)
    }

    fn gap(self) -> Pixels {
        self.span(6., 16.)
    }

    fn brand_height(self) -> Pixels {
        self.span(33., 55.) + self.span(0., 2.) + self.span(11., 14.)
    }

    fn card_padding(self) -> Pixels {
        self.span(4., 11.)
    }

    fn provider_header(self) -> Pixels {
        self.span(26., 40.)
    }

    fn provider_height(self) -> Pixels {
        self.span(31., 47.)
    }

    fn row_height(self, index: usize) -> Pixels {
        self.card_padding()
            + self.provider_header()
            + self.gap()
            + self.provider_height() * (index as f32 + 0.5)
    }

    fn card_height(self) -> Pixels {
        self.row_height(ACCENTS.len() - 1) + self.provider_height() * 0.5 + self.card_padding()
    }

    fn rail_height(self, show_guest: bool) -> Pixels {
        self.brand_height() + self.gap() + self.entries_height(show_guest)
    }

    fn entries_height(self, show_guest: bool) -> Pixels {
        if show_guest {
            self.card_height() + self.gap() + self.guest_height()
        } else {
            self.card_height()
        }
    }

    fn guest_height(self) -> Pixels {
        self.span(44., 59.)
    }
}

impl MainView {
    pub(super) fn welcome(
        &self,
        frame: MotionFrame,
        motion: ConnectionFrame,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let viewport = size(
            window.viewport_size().width,
            window.viewport_size().height - px(super::TITLEBAR_HEIGHT),
        );
        let fluid = self.uses_fluid_glass();
        let m = Metrics::new(viewport.height);
        let wide = split_layout(viewport.width);
        let width = f32::from(viewport.width);
        let intro_split = width >= INTRO_SPLIT_WIDTH && viewport.height >= px(INTRO_SPLIT_HEIGHT);
        let spacious = ((width - 800.) / 600.).clamp(0., 1.);
        let standard_width = super::INITIAL_SIZE.0;
        let desktop = ((width - standard_width) / (DESKTOP_WIDTH - standard_width)).clamp(0., 1.);
        let gap = px(COLUMN_GAP + (DESKTOP_COLUMN_GAP - COLUMN_GAP) * desktop);
        let rail_width = px(336. + 134. * spacious);
        let pane_width = if wide {
            px(430. + 70. * spacious).min(viewport.width - rail_width - gap - px(STAGE_MARGIN * 2.))
        } else {
            px(480.).min(viewport.width - px(STAGE_MARGIN * 2.))
        };
        let center = (viewport.width - rail_width) * 0.5;
        let left = (viewport.width - rail_width - gap - pane_width) * 0.5;
        let right = left + rail_width + gap;
        let rail_x = if wide {
            interpolate(
                if intro_split { right } else { center },
                left,
                motion.layout,
            )
        } else {
            center
        };
        let show_guest = !self.state.has_youtube_account();
        let rail_y = (viewport.height - m.rail_height(show_guest)) * 0.5;
        let card_y = rail_y + m.brand_height() + m.gap();
        let provider = self.login.as_ref().map(|entry| entry.provider());
        let has_pane = provider.is_some() || self.guest_status();
        let stage = div()
            .relative()
            .overflow_hidden()
            .w_full()
            .min_h_0()
            .flex_1()
            .when(intro_split, |stage| {
                let intro_y = (viewport.height - px(140.)) * 0.5;
                stage.child(
                    div()
                        .absolute()
                        .left(left)
                        .top(interpolate(intro_y, rail_y, motion.layout))
                        .w(rail_width)
                        .child(brand(m, Some(1. - motion.layout), fluid, cx)),
                )
            });
        let stage = stage.when(wide || !has_pane, |stage| {
            stage.child(
                div()
                    .absolute()
                    .left(rail_x)
                    .top(if intro_split { card_y } else { rail_y })
                    .w(rail_width)
                    .child(self.provider_rail(frame, m, show_guest, !intro_split, cx)),
            )
        });

        if !has_pane {
            return stage;
        }
        let flight = motion.flight;
        let pane_height = if wide {
            m.entries_height(show_guest)
        } else {
            px(compact_pane_height(provider)).min(viewport.height - px(STAGE_MARGIN * 2.))
        };
        let target = if wide {
            Bounds::new(point(right, card_y), size(pane_width, pane_height))
        } else {
            let top = (viewport.height - pane_height) * 0.5;
            Bounds::new(
                point((viewport.width - pane_width) * 0.5, top),
                size(pane_width, pane_height),
            )
        };
        let pane = if wide {
            Bounds::new(
                point(
                    target.left().max(rail_x + rail_width + px(12.)),
                    target.top(),
                ),
                target.size,
            )
        } else {
            Bounds::new(
                point(
                    interpolate(rail_x, target.left(), motion.layout),
                    interpolate(card_y, target.top(), motion.layout),
                ),
                size(
                    interpolate(rail_width, pane_width, motion.layout),
                    interpolate(m.card_height(), pane_height, motion.layout),
                ),
            )
        };
        let content = self.login.as_ref().map_or_else(
            || self.guest_connection_panel(!wide, cx).into_any_element(),
            |entry| {
                self.connection_panel(entry, !wide, pane.size.width, flight, cx)
                    .into_any_element()
            },
        );
        let stage = stage.when(!wide && !fluid, |stage| {
            stage.children(
                flight
                    .filter(|flight| flight.progress < 0.32)
                    .map(|flight| {
                        div()
                            .absolute()
                            .left(rail_x)
                            .top(card_y)
                            .w(rail_width)
                            .h(m.card_height())
                            .rounded(px(10.))
                            .bg(rgb(SURFACE))
                            .opacity((1. - flight.progress / 0.32).powi(2))
                    }),
            )
        });
        let stage = stage.child(
            div()
                .id("connection-anchor")
                .track_focus(&self.connection_focus)
                .absolute()
                .left(pane.left())
                .top(pane.top())
                .w(pane.size.width)
                .h(pane.size.height)
                .child(content),
        );
        if let Some(flight) = flight.filter(|_| !fluid) {
            let index = provider_index(flight.provider);
            let from = point(
                if wide {
                    rail_x + rail_width - px(14.)
                } else {
                    rail_x + rail_width * 0.5
                },
                card_y + m.row_height(index),
            );
            stage.child(effects::transfer_fog(Transfer {
                progress: flight.progress,
                assembly: flight.assembly,
                from,
                to: pane,
                provider: index,
                compact: !wide,
            }))
        } else {
            stage
        }
    }

    fn provider_rail(
        &self,
        frame: MotionFrame,
        m: Metrics,
        show_guest: bool,
        show_brand: bool,
        cx: &Context<Self>,
    ) -> Div {
        let fluid = self.uses_fluid_glass();
        let palette = theme(cx);
        let card = div()
            .w_full()
            .h(m.card_height())
            .flex_none()
            .flex()
            .flex_col()
            .py(m.card_padding())
            .child(
                div()
                    .flex()
                    .items_center()
                    .h(m.provider_header())
                    .mb(m.gap())
                    .border_b_1()
                    .border_color(palette.border())
                    .child(
                        div()
                            .id("welcome-services")
                            .accessibility_id("welcome-services")
                            .role(Role::Heading)
                            .aria_label("Подключите сервис")
                            .text_size(if fluid {
                                m.span(tokens::text::TRACK_TITLE, 24.)
                            } else {
                                m.span(17., 25.)
                            })
                            .line_height(m.span(21., 32.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Подключите сервис"),
                    ),
            )
            .child(
                div().flex().flex_col().children(
                    ProviderId::ALL
                        .into_iter()
                        .enumerate()
                        .map(|(index, provider)| self.provider_row(index, provider, m, frame, cx)),
                ),
            );

        let entries = div()
            .id("entry-options")
            .accessibility_id("entry-options")
            .role(Role::Group)
            .aria_label("Варианты входа")
            .w_full()
            .h(m.entries_height(show_guest))
            .flex()
            .flex_col()
            .gap(m.gap())
            .child(card)
            .when(show_guest, |entries| {
                entries.child(self.guest_button(m, cx))
            });

        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap(m.gap())
            .when(show_brand, |rail| rail.child(brand(m, None, fluid, cx)))
            .child(entries)
    }

    fn provider_row(
        &self,
        index: usize,
        provider: ProviderId,
        m: Metrics,
        frame: MotionFrame,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let selected = self
            .login
            .as_ref()
            .is_some_and(|entry| entry.provider() == provider);
        let hover = frame.hover(index);
        let glass = self.glass_controls(cx);
        let contrast = self.fluid_high_contrast(cx);
        let palette = theme(cx);
        button(
            provider.as_str(),
            provider.display_name(),
            index as isize + 1,
        )
        .track_focus(&self.provider_focus[index])
        .relative()
        .w_full()
        .h(m.provider_height())
        .px(m.span(8., 12.))
        .gap(m.span(11., 15.))
        .rounded(px(6.))
        .text_size(m.span(13.5, 16.5))
        .line_height(m.span(17., 21.))
        .when_some(glass, |row, glass| {
            glass
                .paint(
                    row,
                    if selected {
                        Tone::Selected
                    } else {
                        Tone::Quiet
                    },
                )
                .text_color(palette.text())
                .when(selected, |row| row.child(fluid_selection_marker(contrast)))
        })
        .when(glass.is_none(), |row| {
            row.bg(gpui::Hsla::from(rgb(ELEVATED)).alpha(if selected { 1. } else { hover }))
                .active(|row| row.bg(rgb(ELEVATED)))
        })
        .when(selected, |row| row.aria_description("Текущий сервис"))
        .child(
            div()
                .relative()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(m.span(19., 27.))
                .when(provider == ProviderId::SoundCloud, |tile| {
                    tile.rounded_full().bg(rgb(SOUNDCLOUD_TILE))
                })
                .child(mark(
                    BRAND_MARKS[index],
                    if provider == ProviderId::SoundCloud {
                        m.span(15., 22.)
                    } else {
                        m.span(19., 27.)
                    },
                )),
        )
        .child(
            div()
                .relative()
                .flex_1()
                .whitespace_nowrap()
                .child(provider.display_name()),
        )
        .child(
            div()
                .relative()
                .left(px(if glass.is_some() {
                    0.
                } else {
                    frame.shift(index)
                }))
                .child(
                    Icon::new(IconName::ChevronRight)
                        .pixel_size(m.span(13., 17.))
                        .color(palette.dimmed().hsla()),
                ),
        )
        .when(glass.is_none(), |row| {
            row.on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                let next = if *hovered {
                    Some(index)
                } else if this.hovered == Some(index) {
                    None
                } else {
                    return;
                };
                if this.hovered != next {
                    this.hovered = next;
                    cx.notify();
                }
            }))
        })
        .on_activate(
            cx.listener(move |this, _, window, cx| this.select_provider(provider, window, cx)),
        )
    }

    fn guest_button(&self, m: Metrics, cx: &Context<Self>) -> impl IntoElement {
        let glass = self.glass_controls(cx);
        let palette = theme(cx);
        button("guest", "Гостевой вход", ProviderId::ALL.len() as isize + 1)
            .aria_description("Публичные подборки без аккаунта")
            .rounded(px(8.))
            .w_full()
            .h(m.guest_height())
            .px(m.span(10., 16.))
            .gap(px(10.))
            .when_some(glass, |button, glass| glass.paint(button, Tone::Outlined))
            .when(glass.is_none(), |button| {
                button
                    .bg(rgb(SURFACE))
                    .hover(|style| style.bg(rgb(ELEVATED)))
                    .active(|style| style.bg(rgb(ELEVATED)))
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(1.))
                    .child(
                        div()
                            .text_size(m.span(13., 16.))
                            .line_height(m.span(16., 20.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Гостевой вход"),
                    )
                    .child(
                        div()
                            .text_size(m.span(11., 12.))
                            .line_height(m.span(14., 16.))
                            .text_color(palette.dimmed())
                            .child("Публичные подборки без аккаунта"),
                    ),
            )
            .child(
                div()
                    .size(m.span(25., 34.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(palette.primary().hsla())
                    .child(
                        Icon::new(IconName::ArrowRight)
                            .pixel_size(m.span(15., 19.))
                            .color(palette.body().hsla()),
                    ),
            )
            .on_activate(cx.listener(|this, _, _, cx| this.run(crate::state::Intent::Guest, cx)))
    }
}

fn interpolate(from: Pixels, to: Pixels, progress: f32) -> Pixels {
    from + (to - from) * progress
}

fn brand(m: Metrics, intro: Option<f32>, fluid: bool, cx: &Context<MainView>) -> Div {
    let palette = theme(cx);
    let visibility = intro.unwrap_or_default();
    let title = if fluid {
        m.span(26., 43.) + px(12. * visibility)
    } else {
        m.span(24., 41.) + px(7. * visibility)
    };
    div()
        .h(m.brand_height())
        .flex()
        .flex_col()
        .when(intro.is_none(), |brand| brand.items_center())
        .gap(m.span(0., 5.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(m.span(6., 11.))
                .child(mark("mediadeka.svg", m.span(33., 55.)))
                .child(
                    div()
                        .id("brand-name")
                        .accessibility_id("brand-name")
                        .role(Role::Heading)
                        .aria_label("Mediadeka")
                        .line_height(if fluid {
                            title + px(4.)
                        } else {
                            m.span(26., 44.)
                        })
                        .text_color(palette.text())
                        .text_size(title)
                        .font_weight(FontWeight(670.))
                        .child("Mediadeka"),
                ),
        )
        .when_some(
            intro.filter(|visibility| *visibility > 0.08),
            |brand, visibility| {
                brand.child(
                    div()
                        .opacity(visibility)
                        .text_size(px(16.))
                        .line_height(px(23.))
                        .text_color(palette.dimmed())
                        .child("Войдите в музыкальный сервис, чтобы открыть свою медиатеку."),
                )
            },
        )
}
