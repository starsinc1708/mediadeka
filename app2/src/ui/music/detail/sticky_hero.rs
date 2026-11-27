use std::time::Instant;

use gpui::{
    AnyElement, App, Div, Hsla, IntoElement, Pixels, Role, SpanId, Window, div, linear_color_stop,
    linear_gradient, prelude::*, px, rgb,
};

use crate::ui::{
    components::{Activate, button, scrollbar::ScrollSurface},
    design_system::motion::{MOTION, out_cubic, smoothstep},
    fluid_glass::tokens,
};

use super::detail_style;

const COMPACT_HEIGHT: f32 = 68.;
const COMPACT_ARTWORK: f32 = 44.;
const COMPACT_TITLE: f32 = 17.;
const COMPACT_PLAY: f32 = 38.;
const MIN_TITLE_SPACE: f32 = 96.;

/// One visual Hero over a stable spacer in the scroll content. The visible
/// footprint follows the spacer until it reaches the height of the controller.
#[derive(Clone, Copy)]
pub(in crate::ui) struct CollapsingMediaHero {
    expanded_height: f32,
    expanded_cover: f32,
    expanded_title: f32,
    height: f32,
    progress: f32,
    reduced: bool,
}

pub(in crate::ui) struct HeroHeader<'a> {
    pub id: &'static str,
    pub title_id: &'static str,
    pub title: &'a str,
    pub subtitle: Option<AnyElement>,
    pub width: f32,
    pub margin: f32,
    /// Width of the controls including the play button and their spacing.
    pub action_width: f32,
    pub high_contrast: bool,
}

impl CollapsingMediaHero {
    pub(super) fn collapsed_height() -> Pixels {
        px(COMPACT_HEIGHT)
    }

    pub(in crate::ui) fn new(
        expanded_height: f32,
        expanded_cover: f32,
        expanded_title: f32,
        offset: Pixels,
        reduced: bool,
    ) -> Self {
        let expanded_height = expanded_height.max(COMPACT_HEIGHT + 1.);
        let distance = expanded_height - COMPACT_HEIGHT;
        let offset = f32::from(offset).clamp(0., distance);
        let progress = offset / distance;
        Self {
            expanded_height,
            expanded_cover,
            expanded_title,
            height: expanded_height - offset,
            progress,
            reduced,
        }
    }

    pub(in crate::ui) fn height(self) -> Pixels {
        px(self.height)
    }

    pub(in crate::ui) fn expanded_height(self) -> Pixels {
        px(self.expanded_height)
    }

    pub(in crate::ui) fn progress(self) -> f32 {
        self.progress
    }

    fn presentation(self) -> f32 {
        if self.reduced {
            // Keep the expanded composition in the remaining spacer until
            // the scroll reaches the pinned controller.
            if self.progress() == 1. { 1. } else { 0. }
        } else {
            smoothstep(self.progress())
        }
    }

    pub(in crate::ui) fn artwork_size(self) -> f32 {
        mix(self.expanded_cover, COMPACT_ARTWORK, self.presentation()).min(self.height - 16.)
    }

    pub(in crate::ui) fn artwork_radius(self) -> f32 {
        mix(10., 6., self.presentation())
    }

    pub(in crate::ui) fn play_size(self) -> f32 {
        let expanded = if self.expanded_cover < 120. { 44. } else { 56. };
        mix(expanded, COMPACT_PLAY, self.presentation())
    }

    pub(in crate::ui) fn hero_light(self) -> f32 {
        mix(1., 0.28, self.presentation())
    }

    pub(in crate::ui) fn render(
        self,
        props: HeroHeader<'_>,
        artwork: impl IntoElement,
        actions: impl IntoElement,
        extras: impl IntoElement,
        on_title: impl Fn(&(), &mut Window, &mut App) + 'static,
        _cx: &App,
    ) -> Div {
        let t = self.presentation();
        let height = self.height;
        let cover = self.artwork_size();
        let width = (props.width - 2. * props.margin).max(1.);
        let gap = mix(if width < 780. { 16. } else { 30. }, 12., t);
        let text_left = cover + gap;
        let mut title_size = mix(
            detail_style::title_size(self.expanded_title, props.title),
            COMPACT_TITLE,
            t,
        );
        let multiline = self.expanded_height >= 220. && props.title.chars().count() > 28;
        let mut extra_line = if multiline {
            1. - smoothstep((t - 0.20) / 0.42)
        } else {
            0.
        };
        let meta_size = mix(15., 12., t);
        if self.reduced && t == 0. {
            // The stacked typography must fit the clipped scroll footprint.
            let title_space = height - 2. * 4. - meta_size * 1.25 - 3.;
            if title_size * 1.08 * (1. + extra_line) > title_space {
                extra_line = 0.;
                title_size = title_size.min(title_space / 1.08);
            }
        }
        let title_lines = if extra_line > 0.65 { 2 } else { 1 };
        let title_height = title_size * 1.08 * (1. + extra_line);
        let play = self.play_size();
        let actions_right = (width - props.action_width - 4.).max(text_left);
        let actions_slide = smoothstep((t - 0.18) / 0.64);
        let title_block = title_height + meta_size * 1.25 + 3.;
        let max_stacked_title_y = (height - title_block - play - 12.).max(4.);
        let title_y = mix(
            (height * 0.19).clamp(4., max_stacked_title_y),
            (height - title_size * 1.08 - 17.) * 0.5,
            t,
        );
        let text_bottom = title_y + title_block;
        let actions_y = mix(
            (text_bottom + 8.).min(height - play - 4.),
            (height - play) * 0.5,
            smoothstep((t - 0.53) / 0.47),
        );
        let mut actions_left = mix(text_left, actions_right, actions_slide);
        if actions_y < text_bottom - 2. && actions_left < text_left + MIN_TITLE_SPACE {
            actions_left = actions_right;
        }
        let title_width = if actions_y >= text_bottom - 2. || props.action_width == 0. {
            width - text_left
        } else {
            actions_left - text_left - 8.
        };
        let ink = detail_style::Ink::new(true, props.high_contrast);
        let surface: Hsla = rgb(tokens::CANVAS).into();
        let extras_top = actions_y + play + 5.;
        let extras_alpha = 1. - smoothstep((t - 0.19) / 0.38);
        let show_extras = extras_alpha > 0.
            && extras_top + 28. < height
            && (!self.reduced || self.progress() == 0.);

        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(self.height())
            .child(
                div()
                    .id(props.id)
                    .accessibility_id(props.id)
                    .role(Role::Group)
                    .aria_label("Заголовок страницы")
                    .size_full()
                    .relative()
                    .when(props.high_contrast, |header| {
                        header.bg(rgb(tokens::contrast::CANVAS))
                    })
                    .when(!props.high_contrast, |header| {
                        header.child(
                            div()
                                .absolute()
                                .inset_0()
                                .fade_edges(px(14.), Pixels::ZERO)
                                .bg(linear_gradient(
                                    180.,
                                    linear_color_stop(surface.alpha(0.08 + 0.42 * t), 0.),
                                    linear_color_stop(surface.alpha(0.), 1.),
                                )),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .left(px(props.margin))
                            .top(px((height - cover) * 0.5))
                            .size(px(cover))
                            .group("compact-hero-cover")
                            .child(
                                div()
                                    .size_full()
                                    .relative()
                                    .when(t == 1. && !self.reduced, |artwork| {
                                        artwork
                                            .layer_scale(0.97)
                                            .group_hover("compact-hero-cover", |style| {
                                                style.layer_scale(1.)
                                            })
                                    })
                                    .child(artwork),
                            ),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(props.margin + text_left))
                            .top(px(title_y))
                            .w(px(title_width.max(1.)))
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                button(format!("{}-top", props.title_id), "К началу страницы", 28)
                                    .rounded(px(4.))
                                    .tooltip(guise::tooltip(props.title.to_owned()))
                                    .child(
                                        detail_style::title(
                                            super::super::heading(
                                                props.title_id,
                                                props.title.to_owned(),
                                            ),
                                            title_size,
                                            title_lines,
                                        )
                                        .min_w_0()
                                        .w_full()
                                        .text_color(ink.primary),
                                    )
                                    .on_activate(on_title),
                            )
                            .when_some(props.subtitle, |text, subtitle| {
                                text.child(
                                    div()
                                        .truncate()
                                        .text_size(px(meta_size))
                                        .text_color(ink.secondary)
                                        .child(subtitle),
                                )
                            }),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(props.margin + actions_left))
                            .top(px(actions_y.max(0.)))
                            .child(actions),
                    )
                    .when(show_extras, |header| {
                        header.child(
                            div()
                                .absolute()
                                .left(px(props.margin + text_left))
                                .right(px(props.margin))
                                .top(px(extras_top))
                                .bottom_0()
                                .overflow_hidden()
                                .opacity(extras_alpha)
                                .child(extras),
                        )
                    }),
            )
    }
}

fn mix(from: f32, to: f32, progress: f32) -> f32 {
    from + (to - from) * progress
}

/// A source revision blends inside the same square at the current scroll pose.
/// Its state is scoped to the page so a navigation uses the shared artwork move.
pub(in crate::ui) fn blend_hero_artwork<T: Clone + PartialEq + 'static>(
    scope: SpanId,
    revision: T,
    animate: bool,
    render: impl Fn(&T, bool) -> AnyElement + 'static,
) -> impl IntoElement {
    HeroArtworkBlend {
        scope,
        revision,
        animate,
        render,
    }
}

#[derive(IntoElement)]
struct HeroArtworkBlend<T: Clone + PartialEq + 'static, F: Fn(&T, bool) -> AnyElement + 'static> {
    scope: SpanId,
    revision: T,
    animate: bool,
    render: F,
}

struct ArtworkRevision<T> {
    current: T,
    prior: Option<(T, Instant)>,
}

impl<T, F> RenderOnce for HeroArtworkBlend<T, F>
where
    T: Clone + PartialEq + 'static,
    F: Fn(&T, bool) -> AnyElement + 'static,
{
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let animate = self.animate && !cx.reduce_motion();
        let state = window.use_keyed_state(
            ("hero-artwork-revision", self.scope as usize),
            cx,
            |_, _| ArtworkRevision {
                current: self.revision.clone(),
                prior: None,
            },
        );
        let blend = state.update(cx, |state, _| {
            if state.current != self.revision {
                let last = std::mem::replace(&mut state.current, self.revision.clone());
                state.prior = animate.then(|| (last, Instant::now()));
            }
            let blend = state.prior.as_ref().and_then(|(last, start)| {
                let progress = start.elapsed().as_secs_f32() / MOTION.normal().as_secs_f32();
                (animate && progress < 1.).then(|| (last.clone(), smoothstep(progress)))
            });
            if blend.is_none() {
                state.prior = None;
            }
            blend
        });
        let alpha = blend.as_ref().map_or(1., |(_, alpha)| *alpha);
        if blend.is_some() && window.is_visible() && window.is_window_active() {
            window.request_animation_frame();
        }
        div()
            .absolute()
            .inset_0()
            .children(blend.map(|(last, alpha)| {
                div()
                    .absolute()
                    .inset_0()
                    .opacity(1. - alpha)
                    .child((self.render)(&last, true))
            }))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .opacity(alpha)
                    .child((self.render)(&self.revision, false)),
            )
    }
}

pub(in crate::ui) fn scroll_to_hero_top(surface: ScrollSurface, window: &mut Window, cx: &mut App) {
    let start = surface.offset();
    if start <= px(0.) {
        return;
    }
    if cx.reduce_motion() || crate::ui::theme::fluid_high_contrast(cx) {
        surface.set_offset(px(0.));
        window.refresh();
    } else {
        return_to_top(surface, start, start, Instant::now(), window);
    }
}

fn return_to_top(
    surface: ScrollSurface,
    start: Pixels,
    expected: Pixels,
    since: Instant,
    window: &mut Window,
) {
    window.on_next_frame(move |window, _| {
        if (surface.offset() - expected).abs() > px(1.) {
            return;
        }
        let phase = since.elapsed().as_secs_f32() / MOTION.normal().as_secs_f32();
        surface.set_offset(start * (1. - out_cubic(phase)));
        window.refresh();
        if phase < 1. {
            let expected = surface.offset();
            return_to_top(surface, start, expected, since, window);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scroll_pose_keeps_a_constant_spacer_and_reverses_at_the_same_offset() {
        let pose = |offset| CollapsingMediaHero::new(328., 280., 56., px(offset), false);
        let samples = [0., 65., 130., 195., 260., 420.].map(pose);
        assert!(
            samples
                .iter()
                .all(|hero| hero.expanded_height() == px(328.))
        );
        assert!(
            samples
                .windows(2)
                .all(|pair| pair[0].artwork_size() >= pair[1].artwork_size())
        );
        assert_eq!(samples[0].artwork_size(), 280.);
        assert_eq!(samples[2].height(), px(198.));
        assert_eq!(samples[2].artwork_size(), pose(130.).artwork_size());
        assert_eq!(samples[5].height(), px(COMPACT_HEIGHT));
        assert_eq!(samples[5].artwork_size(), COMPACT_ARTWORK);
        assert!((samples[5].hero_light() - 0.28).abs() < f32::EPSILON);
    }

    #[test]
    fn reduced_motion_pins_the_compact_controls_when_the_spacer_is_consumed() {
        let pose = |offset| CollapsingMediaHero::new(328., 280., 56., px(offset), true);
        let midway = pose(100.);
        assert_eq!(midway.height(), px(228.));
        assert_eq!(midway.artwork_size(), 212.);
        assert_eq!(midway.play_size(), 56.);
        assert!(pose(259.).artwork_size() > COMPACT_ARTWORK);
        let compact = pose(260.);
        assert_eq!(compact.height(), px(COMPACT_HEIGHT));
        assert_eq!(compact.artwork_size(), COMPACT_ARTWORK);
        assert_eq!(compact.play_size(), COMPACT_PLAY);
        assert_eq!(pose(100.).artwork_size(), midway.artwork_size());
    }
}
