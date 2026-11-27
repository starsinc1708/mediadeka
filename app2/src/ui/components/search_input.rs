use std::time::{Duration, Instant};

use gpui::{App, Context, Entity, Hsla, IntoElement, Window, div, prelude::*, px};
#[cfg(feature = "capture")]
use gpui::{AppContext, Div};
use guise::{Icon, IconName, Size, TextInput, anim::AnimValue};

#[cfg(feature = "capture")]
use crate::ui::MainView;
use crate::ui::{
    design_system::{
        controls::{ControlState, ControlVariant},
        motion::{MOTION, smoothstep},
    },
    fluid_glass::tokens::{size, space, text},
};

use super::{Activate, button, glass_control::Material};

const FOCUS_DURATION: Duration = MOTION.interaction();
const ICON_CONTROL: f32 = size::XS;
#[cfg(feature = "capture")]
const GALLERY_GLOBAL: &str = "appearance-gallery-global-search";
#[cfg(feature = "capture")]
const GALLERY_LOCAL: &str = "appearance-gallery-local-search";

type Submit = Box<dyn Fn(&(), &mut Window, &mut App) + 'static>;

/// The same editing control serves the catalog, library and playlist.
pub(in crate::ui) fn editor(
    id: &'static str,
    placeholder: &'static str,
    tab_order: isize,
    classic_size: Size,
    fluid: bool,
    cx: &mut Context<TextInput>,
) -> TextInput {
    let field = TextInput::new(cx)
        .id(id)
        .placeholder(placeholder)
        .tab_order(tab_order);
    if fluid {
        field.size(Size::Sm).embedded()
    } else {
        field.size(classic_size).bare(true)
    }
}

#[derive(Clone, Copy)]
pub(in crate::ui) enum Scope {
    Global,
    Local,
}

#[derive(IntoElement)]
pub(in crate::ui) struct SearchInput {
    id: &'static str,
    field: Entity<TextInput>,
    style: Material,
    scope: Scope,
    clear_order: isize,
    compact: bool,
    submit: Option<Submit>,
}

impl SearchInput {
    pub(in crate::ui) fn new(
        id: &'static str,
        field: Entity<TextInput>,
        style: Material,
        scope: Scope,
        clear_order: isize,
        compact: bool,
    ) -> Self {
        Self {
            id,
            field,
            style,
            scope,
            clear_order,
            compact,
            submit: None,
        }
    }

    pub(in crate::ui) fn submit(
        mut self,
        handler: impl Fn(&(), &mut Window, &mut App) + 'static,
    ) -> Self {
        self.submit = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for SearchInput {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let variant = ControlVariant::SearchInput;
        let global = matches!(self.scope, Scope::Global);
        let (height, inset, glyph, shortcut) = if global {
            (42., space::LG, 20., "Ctrl+K")
        } else {
            (38., space::MD, 17., "Ctrl+F")
        };
        let field = self.field.read(cx);
        let focused = field.is_focused(window);
        let has_text = !field.is_empty();
        let immediate = cx.reduce_motion() || self.style.contrast();
        let state = window.use_keyed_state(
            ("editorial-search-motion", self.field.entity_id()),
            cx,
            |_, _| [Fade::at(false), Fade::at(has_text)],
        );
        let now = Instant::now();
        let [focus, clear] = state.update(cx, |[focus, clear], _| {
            [
                focus.retarget(focused, now, immediate),
                clear.retarget(has_text, now, immediate),
            ]
        });
        if (focus != value(focused) || clear != value(has_text)) && !immediate {
            window.request_animation_frame();
        }

        let rest = variant.resolve(self.style, ControlState::default());
        let focus_style = variant.resolve(
            self.style,
            ControlState {
                focused: true,
                ..Default::default()
            },
        );
        let bg = mix(rest.bg, focus_style.bg, focus);
        let ink = mix(rest.fg, focus_style.fg, focus);
        let edge = mix(rest.border, focus_style.border, focus);
        let editor_opacity = if immediate {
            1.
        } else if has_text {
            0.92 + 0.08 * clear
        } else {
            1. - 0.08 * focus
        };
        let search = Icon::new(IconName::Search).pixel_size(px(glyph)).color(ink);
        let leading = match self.submit {
            Some(submit) => {
                icon_control("home-catalog-search", "Начать поиск музыки", 11, self.style)
                    .child(search)
                    .on_activate(submit)
                    .into_any_element()
            }
            None => search.into_any_element(),
        };
        let clear_slot = div()
            .size(px(ICON_CONTROL))
            .flex_none()
            .when(has_text, |slot| {
                let field = self.field.clone();
                slot.child(
                    icon_control(
                        format!("{}-clear", self.id),
                        "Очистить поиск",
                        self.clear_order,
                        self.style,
                    )
                    .opacity(clear)
                    .child(Icon::new(IconName::X).pixel_size(px(16.)).color(ink))
                    .on_activate(move |_, window, cx| {
                        field.update(cx, |field, cx| {
                            field.clear(cx);
                            field.focus(window, cx);
                        });
                    }),
                )
            });
        div()
            .h(px(height))
            .w_full()
            .min_w_0()
            .px(px(inset))
            .gap(px(if self.compact { space::XS } else { space::SM }))
            .flex()
            .items_center()
            .rounded(px(variant.radius(global)))
            .border_1()
            .border_color(edge)
            .bg(bg)
            .child(div().flex_none().child(leading))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .opacity(editor_opacity)
                    .child(self.field),
            )
            .child(clear_slot)
            .when(!self.compact, |field| {
                field.child(
                    div()
                        .flex_none()
                        .text_size(px(text::UTILITY))
                        .text_color(ink)
                        .child(shortcut),
                )
            })
    }
}

fn icon_control(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    order: isize,
    material: Material,
) -> gpui::Stateful<gpui::Div> {
    ControlVariant::GhostIcon.paint(
        button(id, label, order)
            .size(px(ICON_CONTROL))
            .flex_none()
            .justify_center()
            .tooltip(guise::tooltip(label)),
        material,
        ControlState::default(),
    )
}

/// Editable samples, using the same fields and clear command as the workspace.
#[cfg(feature = "capture")]
pub(in crate::ui) struct GalleryInputs {
    global: Entity<TextInput>,
    local: Entity<TextInput>,
}

#[cfg(feature = "capture")]
impl GalleryInputs {
    pub(in crate::ui) fn new(cx: &mut Context<MainView>) -> Self {
        Self {
            global: cx.new(|cx| {
                editor(
                    GALLERY_GLOBAL,
                    "Поиск по всей музыке",
                    140,
                    Size::Xs,
                    true,
                    cx,
                )
                .short_placeholder("Поиск музыки")
            }),
            local: cx.new(|cx| {
                editor(GALLERY_LOCAL, "Поиск в плейлисте", 142, Size::Xs, true, cx)
                    .short_placeholder("Поиск")
            }),
        }
    }

    pub(in crate::ui) fn render(&self, material: Material, compact: bool) -> Div {
        div().flex().flex_wrap().gap(px(12.)).children(
            [
                (GALLERY_GLOBAL, &self.global, Scope::Global, 141, 302.),
                (GALLERY_LOCAL, &self.local, Scope::Local, 143, 244.),
            ]
            .map(|(id, field, scope, order, width)| {
                div().w(px(width)).max_w_full().child(SearchInput::new(
                    id,
                    field.clone(),
                    material,
                    scope,
                    order,
                    compact,
                ))
            }),
        )
    }
}

fn mix(from: Hsla, to: Hsla, amount: f32) -> Hsla {
    AnimValue::Color(from)
        .lerp(AnimValue::Color(to), amount)
        .color()
        .unwrap_or(to)
}

fn value(state: bool) -> f32 {
    if state { 1. } else { 0. }
}

struct Fade {
    from: f32,
    target: bool,
    start: Instant,
}

impl Fade {
    fn at(target: bool) -> Self {
        Self {
            from: value(target),
            target,
            start: Instant::now(),
        }
    }

    fn retarget(&mut self, target: bool, now: Instant, immediate: bool) -> f32 {
        if immediate {
            *self = Self::at(target);
            return self.from;
        }
        let elapsed = now.saturating_duration_since(self.start);
        let end = value(self.target);
        let current = if elapsed >= FOCUS_DURATION {
            end
        } else {
            let phase = smoothstep(elapsed.as_secs_f32() / FOCUS_DURATION.as_secs_f32());
            self.from + (end - self.from) * phase
        };
        if target != self.target {
            *self = Self {
                from: current,
                target,
                start: now,
            };
        }
        current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reversing_focus_keeps_the_current_ink_and_settles_after_160_ms() {
        let mut fade = Fade::at(false);
        let start = Instant::now();
        let midpoint = start + FOCUS_DURATION / 2;
        assert_eq!(fade.retarget(true, start, false), 0.);
        assert_eq!(fade.retarget(false, midpoint, false), 0.5);
        assert_eq!(
            fade.retarget(false, midpoint + FOCUS_DURATION / 2, false),
            0.25
        );
        assert_eq!(fade.retarget(false, midpoint + FOCUS_DURATION, false), 0.);
        assert_eq!(fade.retarget(true, midpoint + FOCUS_DURATION, true), 1.);
    }
}
