//! Editorial styles for small interactive surfaces.

use gpui::{AnyElement, App, Div, Hsla, Stateful, div, prelude::*, px, rgb, transparent_black};

use super::materials::{Interaction, Kind, Material, action};
use crate::ui::fluid_glass::tokens::{self, contrast};

pub(in crate::ui) fn material(cx: &App) -> Material {
    Material::new(Kind::Ghost, super::materials::accent(cx))
        .high_contrast(crate::ui::theme::fluid_high_contrast(cx))
}

pub(in crate::ui) fn switch_color(material: Material) -> guise::ColorValue {
    Hsla::from(rgb(if material.contrast() {
        contrast::ACCENT
    } else {
        action::SURFACE
    }))
    .into()
}

pub(in crate::ui) fn paint_page_tab(
    control: Stateful<Div>,
    material: Material,
    selected: bool,
) -> Stateful<Div> {
    ControlVariant::Tab
        .paint(control, material, ControlState::default())
        .rounded(px(tokens::radius::CONTROL))
        .when(selected, |tab| {
            tab.text_color(rgb(if material.contrast() {
                contrast::PRIMARY
            } else {
                tokens::PRIMARY
            }))
        })
}

pub(in crate::ui) fn tab_selection_marker(material: Material) -> Div {
    let accent = if material.contrast() {
        rgb(contrast::ACCENT).into()
    } else {
        material.accent()
    };
    div()
        .w(px(18.))
        .max_w_full()
        .h(px(2.))
        .rounded_full()
        .bg(accent)
}

/// A neutral shared surface for related secondary commands. Each child keeps
/// its own hover, focus and semantics; the cluster only supplies the backing
/// and the hairline dividers, so no command is duplicated.
pub(in crate::ui) fn action_cluster(
    id: &'static str,
    material: Material,
    compact: bool,
    actions: Vec<AnyElement>,
) -> Stateful<Div> {
    let hc = material.contrast();
    let fill = if hc {
        Hsla::from(rgb(contrast::SURFACE))
    } else {
        Hsla::from(rgb(tokens::SURFACE)).alpha(0.55)
    };
    let edge = Hsla::from(rgb(if hc { contrast::RULE } else { tokens::RULE })).alpha(if hc {
        1.
    } else {
        0.45
    });
    let divider = Hsla::from(rgb(if hc { contrast::RULE } else { tokens::RULE })).alpha(if hc {
        1.
    } else {
        0.55
    });
    let mut children = Vec::with_capacity(actions.len() * 2);
    for (index, action) in actions.into_iter().enumerate() {
        if index > 0 {
            children.push(
                div()
                    .w(px(1.))
                    .h(px(if compact { 14. } else { 16. }))
                    .flex_none()
                    .bg(divider)
                    .into_any_element(),
            );
        }
        children.push(action);
    }
    div()
        .id(id)
        .accessibility_id(id)
        .role(gpui::Role::Group)
        .aria_label("Дополнительные действия")
        .flex()
        .items_center()
        .flex_none()
        .rounded(px(if compact { 10. } else { 12. }))
        .bg(fill)
        .border_1()
        .border_color(edge)
        .p(px(2.))
        .children(children)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) enum ControlVariant {
    PrimaryCircle,
    SecondaryButton,
    TransportIcon,
    GhostIcon,
    TextAction,
    SearchInput,
    SelectTrigger,
    MenuItem,
    Tab,
    DialogAction,
}

#[derive(Clone, Copy, Default)]
pub(in crate::ui) struct ControlState {
    pub hovered: bool,
    pub pressed: bool,
    pub focused: bool,
    pub active: bool,
    pub disabled: bool,
    pub loading: bool,
    pub expanded: bool,
}

#[derive(Clone, Copy)]
pub(in crate::ui) struct ControlStyle {
    pub bg: Hsla,
    pub fg: Hsla,
    pub border: Hsla,
    pub radius: f32,
}

impl ControlVariant {
    pub fn dialog_button(self, button: guise::Button, material: Material) -> guise::Button {
        self.guise_button(button, material, true)
    }

    pub fn text_button(self, button: guise::Button, material: Material) -> guise::Button {
        self.guise_button(button, material, false)
    }

    fn guise_button(
        self,
        button: guise::Button,
        material: Material,
        filled: bool,
    ) -> guise::Button {
        let rest = self.resolve(material, ControlState::default());
        let hover = self.resolve(
            material,
            ControlState {
                hovered: true,
                ..ControlState::default()
            },
        );
        let press = self.resolve(
            material,
            ControlState {
                pressed: true,
                ..ControlState::default()
            },
        );
        let focus = self.resolve(
            material,
            ControlState {
                focused: true,
                ..ControlState::default()
            },
        );
        button
            .variant(if filled {
                guise::Variant::Filled
            } else {
                guise::Variant::Transparent
            })
            .color(if filled { rest.bg } else { rest.fg })
            .backgrounds(rest.bg.into(), hover.bg.into(), press.bg.into())
            .focus_outline(focus.border)
            .radius(guise::Size::Md)
    }

    pub fn radius(self, global_search: bool) -> f32 {
        use tokens::radius;
        match self {
            Self::PrimaryCircle | Self::TransportIcon => radius::PILL,
            Self::SearchInput if global_search => 22.,
            Self::SearchInput => radius::INPUT,
            Self::Tab => radius::THUMB,
            Self::SecondaryButton
            | Self::GhostIcon
            | Self::TextAction
            | Self::SelectTrigger
            | Self::MenuItem
            | Self::DialogAction => radius::CONTROL,
        }
    }

    pub fn resolve(self, material: Material, state: ControlState) -> ControlStyle {
        let hc = material.contrast();
        let interactive = !state.disabled && !state.loading;
        let pressed = interactive && state.pressed;
        let hovered = interactive && state.hovered;
        let focus = rgb(if hc {
            contrast::PRIMARY
        } else {
            tokens::PRIMARY
        })
        .into();
        let muted = rgb(if hc { contrast::PRIMARY } else { action::ICON }).into();
        let accent = if hc {
            rgb(contrast::ACCENT).into()
        } else {
            material.accent()
        };
        let neutral = Hsla::from(rgb(action::HOVER));
        let menu_cursor = self == Self::MenuItem && state.active;
        let wash = |alpha| {
            if hc {
                rgb(if self == Self::MenuItem {
                    contrast::SELECTED
                } else {
                    contrast::ELEVATED
                })
                .into()
            } else {
                neutral.alpha(alpha)
            }
        };
        let (bg, fg, edge) = if matches!(self, Self::PrimaryCircle | Self::DialogAction) {
            let interaction = if pressed {
                Interaction::Pressed
            } else if hovered {
                Interaction::Hover
            } else {
                Interaction::Rest
            };
            let style = material.kind(Kind::Porcelain).style(interaction);
            (style.flat, style.ink, transparent_black())
        } else if self == Self::SearchInput {
            (
                if hc {
                    rgb(contrast::SURFACE).into()
                } else {
                    neutral.alpha(if state.focused { 0.07 } else { 0.045 })
                },
                if state.focused { focus } else { muted },
                if hc {
                    rgb(contrast::RULE).into()
                } else {
                    neutral.alpha(0.065)
                },
            )
        } else if self == Self::SecondaryButton {
            let bg = if hc {
                rgb(if state.active || pressed {
                    contrast::SELECTED
                } else if hovered {
                    contrast::ELEVATED
                } else {
                    contrast::SURFACE
                })
                .into()
            } else {
                let (color, alpha) = if pressed {
                    (tokens::SIDEBAR, 0.90)
                } else if state.active {
                    (tokens::ACTIVE, 0.90)
                } else if hovered {
                    (tokens::HOVER, 0.85)
                } else {
                    (tokens::SURFACE, 0.78)
                };
                Hsla::from(rgb(color)).alpha(alpha)
            };
            let edge = if hc {
                rgb(contrast::RULE).into()
            } else {
                transparent_black()
            };
            (bg, if state.active { accent } else { focus }, edge)
        } else {
            let bg = if pressed {
                wash(0.14)
            } else if hovered || menu_cursor || state.expanded {
                wash(0.07)
            } else if state.active && self == Self::GhostIcon {
                if hc {
                    rgb(contrast::SELECTED).into()
                } else {
                    accent.alpha(0.10)
                }
            } else {
                transparent_black()
            };
            let fg = if state.active && self != Self::MenuItem {
                accent
            } else if matches!(self, Self::MenuItem | Self::SelectTrigger)
                || hovered
                || state.focused
            {
                focus
            } else {
                muted
            };
            (bg, fg, transparent_black())
        };
        ControlStyle {
            bg,
            fg,
            border: if state.focused || hc && menu_cursor {
                focus.alpha(if hc { 1. } else { 0.65 })
            } else {
                edge
            },
            radius: self.radius(false),
        }
    }

    pub fn paint(
        self,
        control: Stateful<Div>,
        material: Material,
        state: ControlState,
    ) -> Stateful<Div> {
        let rest = self.resolve(material, state);
        let hover = self.resolve(
            material,
            ControlState {
                hovered: true,
                ..state
            },
        );
        let press = self.resolve(
            material,
            ControlState {
                pressed: true,
                ..state
            },
        );
        let focus = self.resolve(
            material,
            ControlState {
                focused: true,
                ..state
            },
        );
        control
            .rounded(px(rest.radius))
            .bg(rest.bg)
            .text_color(rest.fg)
            .border_color(rest.border)
            .hover(move |style| style.bg(hover.bg).text_color(hover.fg))
            .active(move |style| style.bg(press.bg).text_color(press.fg))
            .focus_visible(move |style| style.border_color(focus.border).text_color(focus.fg))
    }

    pub fn surface(self, material: Material, state: ControlState) -> guise::Surface {
        let rest = self.resolve(material, state);
        let hover = self.resolve(
            material,
            ControlState {
                hovered: true,
                ..state
            },
        );
        guise::Surface {
            bg: rest.bg,
            bg_hover: hover.bg,
            fg: rest.fg,
            border: (rest.border.a > 0.).then_some(rest.border),
        }
    }
}
