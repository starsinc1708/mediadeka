use std::{cell::Cell, rc::Rc};

use gpui::{
    AccessibleAction, Bounds, ClickEvent, Context, Div, FontWeight, Hsla, KeyDownEvent,
    Orientation, Pixels, Role, Stateful,
    accesskit::{ActionData, Toggled},
    div, linear_color_stop, linear_gradient,
    prelude::*,
    px, relative, rgb,
};
use guise::{Icon, IconName};

use super::{
    ACCENT, BASE, CONTROL, Concept, LINE, MUTED, SELECTED, TEXT, button, button_base, glyph, sleeve,
};

const MAX_VOLUME: u32 = 100;

impl Concept {
    pub(super) fn transport(&self, compact: bool, cx: &Context<Self>) -> Div {
        let duration = self.duration_seconds();
        let current = self.track(self.current);
        div()
            .flex_none()
            .w_full()
            .h(px(if compact { 68. } else { 88. }))
            .px(px(if compact { 17. } else { 32. }))
            .flex()
            .items_center()
            .gap(px(if compact { 12. } else { 24. }))
            .bg(linear_gradient(
                180.,
                linear_color_stop(Hsla::from(rgb(LINE)).alpha(0.74), 0.),
                linear_color_stop(Hsla::from(rgb(CONTROL)).alpha(0.92), 1.),
            ))
            .border_t_1()
            .border_color(Hsla::from(rgb(LINE)).alpha(0.84))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(if compact { 8. } else { 12. }))
                    .child(sleeve(
                        self.releases[self.current.release].image.clone(),
                        if compact { 35. } else { 48. },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_size(px(if compact { 13. } else { 15. }))
                                    .child(current.title.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(rgb(MUTED))
                                    .child(current.artist.clone()),
                            ),
                    ),
            )
            .child(
                div()
                    .w(relative(if compact { 0.42 } else { 0.4 }))
                    .max_w(px(490.))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(if compact { 2. } else { 5. }))
                    .child(self.transport_buttons(compact, cx))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .child(timestamp(self.position_seconds))
                            .child(div().flex_1().min_w_0().child(range(
                                self,
                                Range {
                                    id: "concept-seek",
                                    name: "Позиция воспроизведения, секунды",
                                    max: duration,
                                    step: 5,
                                    order: 72,
                                    get: |view| view.position_seconds,
                                    set: |view, seconds| view.position_seconds = seconds,
                                },
                                cx,
                            )))
                            .child(timestamp(duration)),
                    ),
            )
            .child(self.transport_tools(compact, cx))
    }

    fn transport_buttons(&self, compact: bool, cx: &Context<Self>) -> Div {
        let has_previous = self.has_previous();
        let has_next = !self.following.is_empty();
        let hit_target = if compact { 30. } else { 36. };
        div()
            .flex()
            .items_center()
            .gap(px(if compact { 12. } else { 16. }))
            .child(
                button("concept-prev", "Предыдущий трек", 69)
                    .size(px(hit_target))
                    .justify_center()
                    .child(glyph(
                        IconName::SkipBack,
                        if has_previous { TEXT } else { MUTED },
                    ))
                    .when(!has_previous, |prev| {
                        prev.tab_stop(false)
                            .opacity(0.48)
                            .aria_description("Нет предыдущего трека")
                    })
                    .when(has_previous, |prev| {
                        prev.on_click(cx.listener(|view, _, _, cx| {
                            view.previous_track();
                            cx.notify();
                        }))
                    }),
            )
            .child(
                button_base(
                    "concept-pause",
                    if self.paused {
                        "Продолжить"
                    } else {
                        "Пауза"
                    },
                    70,
                )
                .size(px(if compact { 31. } else { 36. }))
                .justify_center()
                .rounded_full()
                .bg(rgb(TEXT))
                .hover(|style| style.bg(rgb(ACCENT)))
                .child(
                    Icon::new(if self.paused {
                        IconName::Play
                    } else {
                        IconName::Pause
                    })
                    .pixel_size(px(17.))
                    .color(Hsla::from(rgb(BASE))),
                )
                .on_click(cx.listener(|view, _, _, cx| {
                    view.paused = !view.paused;
                    cx.notify();
                })),
            )
            .child(
                button("concept-next", "Следующий трек", 71)
                    .size(px(hit_target))
                    .justify_center()
                    .child(glyph(
                        IconName::SkipForward,
                        if has_next { TEXT } else { MUTED },
                    ))
                    .when(!has_next, |next| {
                        next.tab_stop(false)
                            .opacity(0.48)
                            .aria_description("Очередь пуста")
                    })
                    .when(has_next, |next| {
                        next.on_click(cx.listener(|view, _, _, cx| {
                            view.next();
                            cx.notify();
                        }))
                    }),
            )
    }

    fn transport_tools(&self, compact: bool, cx: &Context<Self>) -> Div {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(if compact { 8. } else { 11. }))
            .child(glyph(
                if self.volume_percent == 0 {
                    IconName::VolumeX
                } else {
                    IconName::Volume2
                },
                MUTED,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(px(if compact { 68. } else { 97. }))
                    .child(range(
                        self,
                        Range {
                            id: "concept-volume",
                            name: "Громкость, проценты",
                            max: MAX_VOLUME,
                            step: 5,
                            order: 73,
                            get: |view| u32::from(view.volume_percent),
                            set: |view, volume| view.volume_percent = volume as u8,
                        },
                        cx,
                    )),
            )
            .child(
                button("concept-queue", "Очередь", 74)
                    .aria_toggled(if self.queue_open {
                        Toggled::True
                    } else {
                        Toggled::False
                    })
                    .flex_none()
                    .h(px(if compact { 30. } else { 34. }))
                    .px(px(if compact { 7. } else { 9. }))
                    .gap(px(6.))
                    .when(self.queue_open, |queue| queue.bg(rgb(SELECTED)))
                    .child(glyph(
                        IconName::ListMusic,
                        if self.queue_open { TEXT } else { MUTED },
                    ))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(MUTED))
                            .child(if compact {
                                self.following.len().to_string()
                            } else {
                                format!("Очередь {}", self.following.len())
                            }),
                    )
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.queue_open = !view.queue_open;
                        cx.notify();
                    })),
            )
    }
}

fn timestamp(seconds: u32) -> Div {
    div()
        .min_w(px(29.))
        .flex_none()
        .text_center()
        .text_size(px(11.))
        .text_color(rgb(MUTED))
        .child(format!("{}:{:02}", seconds / 60, seconds % 60))
}

#[derive(Clone, Copy)]
struct Range {
    id: &'static str,
    name: &'static str,
    max: u32,
    step: u32,
    order: isize,
    get: fn(&Concept) -> u32,
    set: fn(&mut Concept, u32),
}

fn range(view: &Concept, range: Range, cx: &Context<Concept>) -> Stateful<Div> {
    let value = (range.get)(view).min(range.max);
    let ratio = value as f32 / range.max.max(1) as f32;
    let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
    let pointer_bounds = bounds.clone();
    let entity = cx.weak_entity();
    let change = move |view: &mut Concept, target: u32, cx: &mut Context<Concept>| {
        (range.set)(view, target.min(range.max));
        cx.notify();
    };
    let increment = cx.listener(move |view, _: &(), _, cx| {
        change(view, (range.get)(view).saturating_add(range.step), cx);
    });
    let decrement = cx.listener(move |view, _: &(), _, cx| {
        change(view, (range.get)(view).saturating_sub(range.step), cx);
    });
    div()
        .on_children_prepainted(move |children, _, _| {
            if let Some(&track) = children.first() {
                bounds.set(track);
            }
        })
        .id(range.id)
        .accessibility_id(range.id)
        .role(Role::Slider)
        .aria_label(range.name)
        .aria_min_numeric_value(0.)
        .aria_max_numeric_value(f64::from(range.max))
        .aria_numeric_value(f64::from(value))
        .aria_numeric_value_step(f64::from(range.step))
        .aria_orientation(Orientation::Horizontal)
        .w_full()
        .h(px(18.))
        .px(px(4.))
        .flex()
        .items_center()
        .rounded(px(4.))
        .border_1()
        .border_color(gpui::transparent_black())
        .child(
            div()
                .relative()
                .w_full()
                .h(px(3.))
                .rounded_full()
                .bg(Hsla::from(rgb(MUTED)).alpha(0.38))
                .child(
                    div()
                        .h_full()
                        .w(relative(ratio))
                        .rounded_full()
                        .bg(rgb(TEXT)),
                )
                .when(range.max > 0, |track| {
                    track.child(
                        div()
                            .absolute()
                            .left(relative(ratio))
                            .top(px(-2.))
                            .ml(px(-3.5))
                            .size(px(7.))
                            .rounded_full()
                            .bg(rgb(TEXT)),
                    )
                }),
        )
        .when(range.max > 0, |slider| {
            slider
                .tab_index(range.order)
                .cursor_pointer()
                .focus_visible(|style| style.border_color(rgb(ACCENT)))
                .on_click(cx.listener(move |view, event: &ClickEvent, _, cx| {
                    if let Some(point) = event.mouse_position() {
                        let track = pointer_bounds.get();
                        if track.size.width > px(0.) {
                            let ratio = ((point.x - track.left()) / track.size.width).clamp(0., 1.);
                            change(view, (ratio * range.max as f32).round() as u32, cx);
                        }
                    }
                }))
                .on_key_down(cx.listener(move |view, event: &KeyDownEvent, _, cx| {
                    let value = (range.get)(view);
                    let target = match event.keystroke.key.as_str() {
                        "left" | "down" => value.saturating_sub(range.step),
                        "right" | "up" => value.saturating_add(range.step),
                        "home" => 0,
                        "end" => range.max,
                        _ => return,
                    };
                    change(view, target, cx);
                    cx.stop_propagation();
                }))
                .on_a11y_action(AccessibleAction::Increment, move |_, window, cx| {
                    increment(&(), window, cx)
                })
                .on_a11y_action(AccessibleAction::Decrement, move |_, window, cx| {
                    decrement(&(), window, cx)
                })
                .on_a11y_action(AccessibleAction::SetValue, move |data, _, cx| {
                    if let Some(ActionData::NumericValue(target)) = data
                        && target.is_finite()
                    {
                        let _ =
                            entity.update(cx, |view, cx| change(view, target.round() as u32, cx));
                    }
                })
        })
}
