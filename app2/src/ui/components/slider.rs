use crate::ui::{
    components::glass_control::{Kind, Material},
    design_system::materials::{self, Interaction, action as editorial},
    fluid_glass::tokens as fluid,
    theme::{ACCENT, DIVIDER, PRIMARY, SLIDER_TRACK, fluid_high_contrast, focus_ring, playlist},
};
use gpui::{
    AccessibleAction, Background, Bounds, Context, Hsla, KeyDownEvent, Orientation, Pixels, Role,
    Window, accesskit::ActionData, div, prelude::*, px, relative, rgb,
};
use std::{cell::Cell, rc::Rc};

const FLUID_SLIDER_GROUP: &str = "fluid-slider";
const TRACK_HEIGHT: f32 = 4.;
const PREVIEW_HIT_HEIGHT: f32 = 22.;

#[derive(Clone, Copy)]
pub(in crate::ui) struct Slider {
    pub value: f64,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub enabled: bool,
    pub playlist_presentation: bool,
    pub fluid: bool,
}

pub(in crate::ui) struct SliderPreview<T: 'static> {
    pub fraction: Option<f32>,
    pub update: fn(&mut T, Option<f32>, &mut Context<T>),
}

pub(in crate::ui) fn slider<T: 'static>(
    id: &'static str,
    title: &'static str,
    order: isize,
    settings: Slider,
    action: fn(&mut T, f64, &mut Context<T>),
    cx: &Context<T>,
) -> impl IntoElement {
    slider_in_window(
        id,
        title,
        order,
        settings,
        move |view, value, _, cx| action(view, value, cx),
        cx,
    )
}

pub(in crate::ui) fn slider_with_preview<T: 'static>(
    id: &'static str,
    title: &'static str,
    order: isize,
    settings: Slider,
    action: fn(&mut T, f64, &mut Context<T>),
    preview: SliderPreview<T>,
    cx: &Context<T>,
) -> impl IntoElement {
    slider_view(
        id,
        title,
        order,
        settings,
        move |view, value, _, cx| action(view, value, cx),
        Some(preview),
        cx,
    )
}

pub(in crate::ui) fn slider_in_window<T: 'static>(
    id: &'static str,
    title: &'static str,
    order: isize,
    settings: Slider,
    action: impl Fn(&mut T, f64, &mut Window, &mut Context<T>) + Copy + 'static,
    cx: &Context<T>,
) -> impl IntoElement {
    slider_view(id, title, order, settings, action, None, cx)
}

fn slider_view<T: 'static>(
    id: &'static str,
    title: &'static str,
    order: isize,
    settings: Slider,
    action: impl Fn(&mut T, f64, &mut Window, &mut Context<T>) + Copy + 'static,
    preview: Option<SliderPreview<T>>,
    cx: &Context<T>,
) -> impl IntoElement {
    let high_contrast = settings.fluid && fluid_high_contrast(cx);
    let accent = materials::accent(cx);
    let preview_color = if high_contrast {
        Hsla::from(rgb(fluid::contrast::ACCENT))
    } else {
        accent
    };
    let state_accent = if high_contrast {
        Hsla::from(rgb(fluid::contrast::ACCENT))
    } else {
        Hsla::from(rgb(editorial::SURFACE))
    };
    let (track_color, inactive_color, fill, thumb): (Hsla, Hsla, Background, Hsla) =
        if high_contrast {
            (
                Hsla::from(rgb(fluid::contrast::RULE)),
                Hsla::from(rgb(fluid::contrast::ELEVATED)),
                rgb(fluid::contrast::ACCENT).into(),
                Hsla::from(rgb(fluid::contrast::PRIMARY)),
            )
        } else if settings.fluid {
            (
                Hsla::from(rgb(editorial::HOVER)).alpha(0.16),
                Hsla::from(rgb(editorial::HOVER)).alpha(0.10),
                rgb(editorial::SURFACE).into(),
                Hsla::from(rgb(editorial::SURFACE)),
            )
        } else if settings.playlist_presentation {
            (
                Hsla::from(rgb(playlist::TRACK_LINE)),
                Hsla::from(rgb(playlist::RULE)),
                rgb(playlist::SIGNAL).into(),
                Hsla::from(rgb(PRIMARY)),
            )
        } else {
            (
                Hsla::from(rgb(SLIDER_TRACK)),
                Hsla::from(rgb(DIVIDER)),
                rgb(ACCENT).into(),
                Hsla::from(rgb(PRIMARY)),
            )
        };
    let thumb_size = if settings.fluid { 10. } else { 8. };
    let span = settings.max - settings.min;
    let ratio = if span > 0. {
        ((settings.value - settings.min) / span).clamp(0., 1.) as f32
    } else {
        0.
    };
    let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
    let click_bounds = bounds.clone();
    let preview_bounds = bounds.clone();
    let set = cx.weak_entity();
    let increment = cx.listener(move |bar, _: &(), window, cx| {
        action(
            bar,
            (settings.value + settings.step).min(settings.max),
            window,
            cx,
        );
    });
    let decrement = cx.listener(move |bar, _: &(), window, cx| {
        action(
            bar,
            (settings.value - settings.step).max(settings.min),
            window,
            cx,
        );
    });
    let track = div()
        .on_children_prepainted(move |children, _, _| {
            if let Some(&child) = children.first() {
                bounds.set(child);
            }
        })
        .id(id)
        .accessibility_id(id)
        .role(Role::Slider)
        .aria_label(title)
        .aria_min_numeric_value(settings.min)
        .aria_max_numeric_value(settings.max)
        .aria_numeric_value(settings.value)
        .aria_numeric_value_step(settings.step)
        .aria_orientation(Orientation::Horizontal)
        .when(!settings.enabled, |track| {
            track.aria_description("Недоступно")
        })
        .relative()
        .h(px(if preview.is_some() {
            PREVIEW_HIT_HEIGHT
        } else {
            17.
        }))
        .w_full()
        .min_w(px(14.))
        .px(px(4.))
        .border_2()
        .border_color(gpui::transparent_black())
        .rounded(px(4.))
        .flex()
        .items_center()
        .when(settings.enabled && settings.fluid, |track| {
            track.group(FLUID_SLIDER_GROUP).active(move |style| {
                style.bg(if high_contrast {
                    rgb(fluid::contrast::SURFACE).into()
                } else {
                    Material::new(Kind::Recessed, accent)
                        .style(Interaction::Hover)
                        .flat
                })
            })
        })
        .child(
            div()
                .w_full()
                .h(px(TRACK_HEIGHT))
                .relative()
                .rounded_full()
                .bg(if settings.enabled {
                    track_color
                } else {
                    inactive_color
                })
                .when(settings.enabled, |track| {
                    track
                        .child(div().w(relative(ratio)).h_full().bg(fill).rounded_full())
                        .child(
                            div()
                                .absolute()
                                .top(px((TRACK_HEIGHT - thumb_size) * 0.5))
                                .left(relative(ratio))
                                .ml(px(-thumb_size * 0.5))
                                .size(px(thumb_size))
                                .rounded_full()
                                .bg(thumb)
                                .when(settings.fluid, |thumb| {
                                    thumb
                                        .border_1()
                                        .border_color(state_accent)
                                        .group_hover(FLUID_SLIDER_GROUP, move |style| {
                                            style.bg(state_accent)
                                        })
                                }),
                        )
                        .children(preview.as_ref().and_then(|preview| preview.fraction).map(
                            |fraction| {
                                div()
                                    .absolute()
                                    .top(px(-2.))
                                    .left(relative(fraction))
                                    .ml(px(-1.))
                                    .w(px(2.))
                                    .h(px(TRACK_HEIGHT + 4.))
                                    .rounded_full()
                                    .bg(preview_color)
                            },
                        ))
                }),
        )
        .when(settings.enabled, |track| {
            track
                .tab_index(order)
                .cursor_pointer()
                .focus_visible(move |track| {
                    track.border_color(if high_contrast {
                        rgb(fluid::contrast::ACCENT).into()
                    } else if settings.fluid {
                        rgb(fluid::control::FOCUS).into()
                    } else {
                        focus_ring()
                    })
                })
                .on_click(
                    cx.listener(move |bar, event: &gpui::ClickEvent, window, cx| {
                        if let Some(fraction) = event
                            .mouse_position()
                            .and_then(|point| fraction_at(click_bounds.get(), point.x))
                        {
                            action(bar, settings.min + f64::from(fraction) * span, window, cx);
                        }
                    }),
                )
                .when_some(preview.filter(|_| span > 0.), |track, preview| {
                    let update = preview.update;
                    track
                        .on_mouse_move(cx.listener(
                            move |bar, event: &gpui::MouseMoveEvent, _, cx| {
                                update(
                                    bar,
                                    fraction_at(preview_bounds.get(), event.position.x),
                                    cx,
                                );
                            },
                        ))
                        .on_hover(cx.listener(move |bar, hovered: &bool, _, cx| {
                            if !hovered {
                                update(bar, None, cx);
                            }
                        }))
                })
                .on_key_down(cx.listener(move |bar, event: &KeyDownEvent, window, cx| {
                    let goal = match event.keystroke.key.as_str() {
                        "left" | "down" => settings.value - settings.step,
                        "right" | "up" => settings.value + settings.step,
                        "home" => settings.min,
                        "end" => settings.max,
                        _ => return,
                    };
                    action(bar, goal.clamp(settings.min, settings.max), window, cx);
                    cx.stop_propagation();
                }))
                .on_a11y_action(AccessibleAction::Increment, move |_, window, cx| {
                    increment(&(), window, cx)
                })
                .on_a11y_action(AccessibleAction::Decrement, move |_, window, cx| {
                    decrement(&(), window, cx)
                })
                .on_a11y_action(AccessibleAction::SetValue, move |data, window, cx| {
                    if let Some(ActionData::NumericValue(value)) = data
                        && value.is_finite()
                    {
                        let _ = set.update(cx, |bar, cx| {
                            action(bar, value.clamp(settings.min, settings.max), window, cx);
                        });
                    }
                })
        });
    div().min_w_0().flex_1().child(track)
}

fn fraction_at(bounds: Bounds<Pixels>, x: Pixels) -> Option<f32> {
    (bounds.size.width > px(0.)).then(|| ((x - bounds.left()) / bounds.size.width).clamp(0., 1.))
}
