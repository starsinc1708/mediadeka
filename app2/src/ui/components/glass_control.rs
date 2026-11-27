//! Component adapter: interaction states resolved through the design-system
//! materials instead of one universal tinted rectangle.

use gpui::{App, Div, Hsla, Stateful, prelude::*, rgb, transparent_black};

use crate::ui::{
    MainView,
    design_system::controls::{ControlState, ControlVariant},
    design_system::materials::{self, Interaction},
    fluid_glass::tokens,
};

pub(in crate::ui) use crate::ui::design_system::materials::{Kind, Material};

#[derive(Clone, Copy)]
pub(in crate::ui) enum Tone {
    Quiet,
    Outlined,
    Selected,
}

impl Tone {
    fn material(self, base: Material) -> Material {
        match self {
            // Secondary actions stay bare until the pointer or focus arrives.
            Self::Quiet => base.kind(Kind::Ghost),
            // Toolbars and secondary controls sit on smoked glass.
            Self::Outlined => base.kind(Kind::SmokedGlass),
            Self::Selected => base.kind(Kind::Ghost).selected(true),
        }
    }
}

impl MainView {
    pub(in crate::ui) fn glass_controls(&self, cx: &App) -> Option<Material> {
        self.uses_fluid_glass().then(|| {
            Material::new(Kind::SmokedGlass, materials::accent(cx))
                .high_contrast(self.fluid_high_contrast(cx))
        })
    }
}

impl Material {
    pub(in crate::ui) fn icon_surface(self, tone: Tone) -> guise::Surface {
        if !matches!(tone, Tone::Outlined) {
            return ControlVariant::GhostIcon.surface(
                self,
                ControlState {
                    active: matches!(tone, Tone::Selected),
                    ..ControlState::default()
                },
            );
        }
        let material = tone.material(self);
        if material.contrast() {
            let palette = material.contrast_palette(tone);
            return guise::Surface {
                bg: palette.rest,
                bg_hover: palette.hover,
                fg: palette.ink,
                border: (palette.edge.a > 0.).then_some(palette.edge),
            };
        }
        let rest = material.style(Interaction::Rest);
        let hover = material.style(Interaction::Hover);
        guise::Surface {
            bg: rest.flat,
            bg_hover: hover.flat,
            fg: rest.ink,
            border: rest.edge,
        }
    }

    pub(in crate::ui) fn paint(self, control: Stateful<Div>, tone: Tone) -> Stateful<Div> {
        self.paint_enabled(control, tone, true)
    }

    pub(in crate::ui) fn paint_enabled(
        self,
        control: Stateful<Div>,
        tone: Tone,
        enabled: bool,
    ) -> Stateful<Div> {
        let material = tone.material(self);
        let control = if !matches!(tone, Tone::Outlined) {
            ControlVariant::TextAction.paint(
                control,
                material,
                ControlState {
                    active: matches!(tone, Tone::Selected),
                    disabled: !enabled,
                    ..ControlState::default()
                },
            )
        } else if material.contrast() {
            let Contrast {
                rest,
                hover,
                pressed,
                edge,
                ink,
                focus,
            } = material.contrast_palette(tone);
            control
                .bg(rest)
                .text_color(ink)
                .border_color(edge)
                .hover(move |style| style.bg(hover))
                .active(move |style| style.bg(pressed))
                .focus_visible(move |style| style.border_color(focus))
        } else {
            let rest = material.style(Interaction::Rest);
            let hover = material.style(Interaction::Hover);
            let pressed = material.style(Interaction::Pressed);
            let focus = material.focus();
            control
                .bg(rest.face)
                .text_color(rest.ink)
                .border_color(rest.edge.unwrap_or_else(transparent_black))
                .hover(move |style| style.bg(hover.face))
                .active(move |style| style.bg(pressed.face))
                .focus_visible(move |style| style.border_color(focus))
        };
        control.when(!enabled, |control| {
            control
                .tab_stop(false)
                .cursor_default()
                .opacity(0.6)
                .aria_description("Недоступно")
        })
    }

    pub(in crate::ui) fn group<T: gpui::Styled>(self, surface: T) -> T {
        if self.contrast() {
            return surface
                .bg(rgb(tokens::contrast::SURFACE))
                .border_color(rgb(tokens::contrast::RULE));
        }
        let panel = Material::new(Kind::OpticalGlass, self.accent()).style(Interaction::Rest);
        surface
            .bg(panel.flat)
            .border_color(panel.edge.unwrap_or_else(transparent_black))
    }

    fn contrast_palette(self, tone: Tone) -> Contrast {
        use tokens::contrast;
        Contrast {
            rest: match tone {
                Tone::Quiet => transparent_black(),
                Tone::Outlined => rgb(contrast::SURFACE).into(),
                Tone::Selected => rgb(contrast::SELECTED).into(),
            },
            hover: rgb(contrast::ELEVATED).into(),
            pressed: rgb(contrast::CANVAS).into(),
            edge: match tone {
                Tone::Quiet => transparent_black(),
                Tone::Outlined => rgb(contrast::RULE).into(),
                Tone::Selected => rgb(contrast::ACCENT).into(),
            },
            ink: rgb(contrast::PRIMARY).into(),
            focus: rgb(contrast::PRIMARY).into(),
        }
    }
}

/// The high-contrast variant deliberately keeps flat, opaque surfaces.
#[derive(Clone, Copy)]
struct Contrast {
    rest: Hsla,
    hover: Hsla,
    pressed: Hsla,
    edge: Hsla,
    ink: Hsla,
    focus: Hsla,
}
