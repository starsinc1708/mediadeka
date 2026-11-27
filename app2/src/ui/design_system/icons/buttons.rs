//! Editorial buttons that use the shared vector icon renderer.

use gpui::{App, Div, FontWeight, Stateful, accesskit::Toggled, div, prelude::*, px};

use super::{AnimatedGlyph, Glyph, GlyphState, animated_glyph, pulse};
use crate::ui::design_system::controls::{self, ControlState, ControlVariant};
use crate::ui::fluid_glass::tokens::{size, space, text};

const GLYPH_SHARE: f32 = 0.52;
/// Reserve room for the longest caption states ("Слушать", "Пауза",
/// "Продолжить", "Загрузка…") so neighbouring Hero commands never shift.
const CAPTIONED_WIDTH: f32 = 146.;

pub(in crate::ui) fn hero_play(
    id: &'static str,
    label: &'static str,
    playing: bool,
    size: f32,
    order: isize,
    state: GlyphState,
    cx: &App,
) -> Stateful<Div> {
    play_button(id, label, playing, size, order, state, true, cx)
}

pub(in crate::ui) fn hero_play_icon(
    id: &'static str,
    label: &'static str,
    playing: bool,
    size: f32,
    order: isize,
    state: GlyphState,
    cx: &App,
) -> Stateful<Div> {
    play_button(id, label, playing, size, order, state, false, cx)
}

#[allow(clippy::too_many_arguments)]
fn play_button(
    id: &'static str,
    label: &'static str,
    playing: bool,
    requested_size: f32,
    order: isize,
    state: GlyphState,
    show_label: bool,
    cx: &App,
) -> Stateful<Div> {
    let state = control_state(state, playing);
    let variant = if show_label {
        ControlVariant::DialogAction
    } else {
        ControlVariant::PrimaryCircle
    };
    let height = if show_label {
        requested_size.min(size::LG + space::XS)
    } else {
        requested_size
    };
    let side = (height * 0.5).clamp(18., size::PLAY * 0.5);
    let control = command_button(id, label, order, None, playing, state)
        .aria_toggled(Toggled::from(playing))
        .h(px(height))
        .justify_center()
        .when(!show_label, |control| {
            control.w(px(height)).tooltip(guise::tooltip(label))
        })
        .when(show_label, |control| {
            control
                .gap(px(space::SM))
                .px(px(space::LG))
                .w(px(CAPTIONED_WIDTH))
        });
    variant
        .paint(control, controls::material(cx), state)
        .child(glyph_content(
            id,
            Glyph::PlayPause,
            playing,
            side,
            state,
            cx,
        ))
        .when(show_label, |control| {
            control.child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(text::BODY))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label),
            )
        })
}

/// An icon command with an independent hit target and a settled-state indicator.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn ghost_icon_button(
    id: &'static str,
    label: &'static str,
    glyph: Glyph,
    target: bool,
    size: f32,
    tab_index: isize,
    enabled: bool,
    cx: &App,
) -> Stateful<Div> {
    let state = if enabled {
        GlyphState::Rest
    } else {
        GlyphState::Disabled
    };
    ghost_icon_button_with_state(id, label, glyph, target, size, tab_index, state, cx)
}

/// [`ghost_icon_button`] with explicit active/disabled/loading semantics.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn ghost_icon_button_with_state(
    id: &'static str,
    label: &'static str,
    glyph: Glyph,
    target: bool,
    size: f32,
    tab_index: isize,
    state: GlyphState,
    cx: &App,
) -> Stateful<Div> {
    let active = state.is_active() || (target && glyph.target_is_emphasis());
    let material = controls::material(cx);
    let variant = if glyph.is_impulse() {
        ControlVariant::TransportIcon
    } else {
        ControlVariant::GhostIcon
    };
    let state = control_state(state, active);
    let control = command_button(id, label, tab_index, Some(glyph), target, state)
        .size(px(size))
        .justify_center()
        .when(!(matches!(glyph, Glyph::More) && target), |control| {
            control.tooltip(guise::tooltip(label))
        });
    variant.paint(control, material, state).child(glyph_content(
        id,
        glyph,
        target,
        size * GLYPH_SHARE,
        state,
        cx,
    ))
}

/// A text action with an optional leading glyph. There is no permanent
/// container: the word sits on the surface and only gains a local wash while
/// hovered or pressed. Useful for the word next to a primary circle.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn text_action(
    id: &'static str,
    label: &'static str,
    glyph: Option<Glyph>,
    target: bool,
    text_size: f32,
    tab_index: isize,
    enabled: bool,
    cx: &App,
) -> Stateful<Div> {
    let state = if enabled {
        GlyphState::Rest
    } else {
        GlyphState::Disabled
    };
    text_action_with_state(id, label, glyph, target, text_size, tab_index, state, cx)
}

/// [`text_action`] with explicit active/disabled/loading semantics.
#[allow(clippy::too_many_arguments)]
pub(in crate::ui) fn text_action_with_state(
    id: &'static str,
    label: &'static str,
    glyph: Option<Glyph>,
    target: bool,
    text_size: f32,
    tab_index: isize,
    state: GlyphState,
    cx: &App,
) -> Stateful<Div> {
    captioned_action(
        ControlVariant::TextAction,
        id,
        label,
        glyph,
        target,
        text_size,
        tab_index,
        state,
        cx,
    )
}

pub(in crate::ui) fn secondary_action(
    id: &'static str,
    label: &'static str,
    glyph: Option<Glyph>,
    target: bool,
    tab_index: isize,
    enabled: bool,
    cx: &App,
) -> Stateful<Div> {
    let state = if enabled {
        GlyphState::Rest
    } else {
        GlyphState::Disabled
    };
    captioned_action(
        ControlVariant::SecondaryButton,
        id,
        label,
        glyph,
        target,
        text::BODY,
        tab_index,
        state,
        cx,
    )
}

#[allow(clippy::too_many_arguments)]
fn captioned_action(
    variant: ControlVariant,
    id: &'static str,
    label: &'static str,
    glyph: Option<Glyph>,
    target: bool,
    text_size: f32,
    tab_index: isize,
    state: GlyphState,
    cx: &App,
) -> Stateful<Div> {
    let active = state.is_active() || (target && glyph.is_some_and(Glyph::target_is_emphasis));
    let state = control_state(state, active);
    let secondary = variant == ControlVariant::SecondaryButton;
    let control = command_button(id, label, tab_index, glyph, target, state)
        .gap(px(space::SM))
        .h(px(if secondary { size::MD } else { size::XS }))
        .px(px(if secondary { space::MD } else { space::SM }));
    variant
        .paint(control, controls::material(cx), state)
        .when_some(glyph, |control, glyph| {
            control.child(glyph_content(
                id,
                glyph,
                target,
                text_size.max(14.) + 2.,
                state,
                cx,
            ))
        })
        .child(
            div()
                .text_size(px(text_size))
                .font_weight(FontWeight::SEMIBOLD)
                .child(label),
        )
}

fn control_state(state: GlyphState, active: bool) -> ControlState {
    ControlState {
        active,
        disabled: !state.is_enabled(),
        loading: state.is_loading(),
        ..ControlState::default()
    }
}

fn command_button(
    id: &'static str,
    label: &'static str,
    order: isize,
    glyph: Option<Glyph>,
    target: bool,
    state: ControlState,
) -> Stateful<Div> {
    let enabled = !state.disabled && !state.loading;
    crate::ui::components::button(id, label, order)
        .group(id)
        .flex_none()
        .when(glyph.is_some_and(Glyph::announces_toggle), |control| {
            control.aria_toggled(Toggled::from(target))
        })
        .when(
            matches!(glyph, Some(Glyph::More | Glyph::ChevronDown)),
            |control| control.aria_expanded(target),
        )
        .when(!enabled, |control| {
            control
                .tab_stop(false)
                .cursor_default()
                .opacity(if state.disabled { 0.5 } else { 0.72 })
                .aria_description(if state.disabled {
                    "Недоступно"
                } else {
                    "Загрузка…"
                })
        })
        .when(enabled && glyph.is_some_and(Glyph::is_impulse), |control| {
            control.on_click(move |_, window, cx| pulse(window, cx, id))
        })
}

fn glyph_content(
    id: &'static str,
    glyph: Glyph,
    target: bool,
    side: f32,
    state: ControlState,
    cx: &App,
) -> Stateful<Div> {
    let animated = animated_glyph(id, glyph, target, side, None, cx);
    div()
        .id("command-glyph")
        .flex_none()
        .size(px(side))
        .when(
            !cx.reduce_motion()
                && !crate::ui::theme::fluid_high_contrast(cx)
                && !state.disabled
                && !state.loading,
            |glyph| {
                glyph
                    .group_hover(id, |style| style.layer_scale(1.04))
                    .group_active(id, |style| style.layer_scale(0.94))
            },
        )
        .child(AnimatedGlyph {
            active: state.active && glyph.wants_indicator(),
            loading: state.loading,
            ..animated
        })
}
