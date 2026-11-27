//! One vocabulary of surfaces for every control.
//!
//! Small controls approximate the shell optics instead of paying for another
//! GPU pipeline: a two-stop directional gradient, a specular lift and a darker
//! floor stand in for refraction and blur. Blur, dispersion and the optical
//! edge stay on the single Fluid Glass shader for the large surfaces.

use gpui::{Background, Hsla, linear_color_stop, linear_gradient, rgb};

use crate::ui::fluid_glass::tokens as glass;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui) enum Kind {
    /// Large parts of the shell: navigation rail, player, right panel.
    OpticalGlass,
    /// Popovers, menus, dialogs and secondary controls.
    SmokedGlass,
    /// Primary actions: Listen, Play/Pause, confirmations.
    Porcelain,
    /// Secondary actions that only gain a surface on interaction.
    Ghost,
    /// Fields, sliders, segmented containers, progress troughs.
    Recessed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::ui) enum Interaction {
    Rest,
    Hover,
    Pressed,
}

/// Editorial action colours. The palette tints the atmosphere, not the
/// controls: a command stays neutral until its state changes.
pub(in crate::ui) mod action {
    /// The one contrast surface (primary circle, dialog confirmation).
    pub(in crate::ui) const SURFACE: u32 = 0xf0edf0;
    /// Glyph on the contrast surface.
    pub(in crate::ui) const GLYPH: u32 = 0x17151b;
    /// Resting secondary icon.
    pub(in crate::ui) const ICON: u32 = super::glass::SECONDARY;
    /// Hovered or focused secondary icon.
    pub(in crate::ui) const HOVER: u32 = 0xffffff;
    /// Local wash under a bare icon while hovered or pressed.
    pub(in crate::ui) const WASH: f32 = 0.10;
}

pub(in crate::ui) const SMOKED_BASE: u32 = glass::ELEVATED;

/// What the analytic renderer consumes for one material. Terms the shell
/// shader owns (blur, refraction, dispersion) are not repeated here.
#[derive(Clone, Copy)]
pub(in crate::ui) struct Parameters {
    pub opacity: f32,
    pub edge_intensity: f32,
    pub inner_shadow: f32,
}

#[derive(Clone, Copy)]
pub(in crate::ui) struct Style {
    /// Directional gradient: specular top, optical body, darker floor.
    pub face: Background,
    /// Single colour for surfaces that only take a flat fill.
    pub flat: Hsla,
    pub edge: Option<Hsla>,
    pub ink: Hsla,
}

#[derive(Clone, Copy)]
pub(in crate::ui) struct Material {
    kind: Kind,
    accent: Hsla,
    selected: bool,
    high_contrast: bool,
}

impl Kind {
    pub(in crate::ui) fn parameters(self) -> Parameters {
        let (opacity, edge, shadow) = match self {
            Self::OpticalGlass => (0.84, 0.55, 0.45),
            Self::SmokedGlass => (0.90, 0.34, 0.40),
            Self::Porcelain => (1.00, 0.60, 0.30),
            Self::Ghost => (0.0, 0.0, 0.0),
            Self::Recessed => (0.72, 0.16, 0.70),
        };
        Parameters {
            opacity,
            edge_intensity: edge,
            inner_shadow: shadow,
        }
    }
}

impl Material {
    pub(in crate::ui) fn new(kind: Kind, accent: Hsla) -> Self {
        Self {
            kind,
            accent,
            selected: false,
            high_contrast: false,
        }
    }

    pub(in crate::ui) fn high_contrast(self, high_contrast: bool) -> Self {
        Self {
            high_contrast,
            ..self
        }
    }

    pub(in crate::ui) fn contrast(self) -> bool {
        self.high_contrast
    }

    pub(in crate::ui) fn accent(self) -> Hsla {
        self.accent
    }

    pub(in crate::ui) fn kind(self, kind: Kind) -> Self {
        Self { kind, ..self }
    }

    pub(in crate::ui) fn selected(self, selected: bool) -> Self {
        Self { selected, ..self }
    }

    pub(in crate::ui) fn parameters(self) -> Parameters {
        self.kind.parameters()
    }

    /// The ring that follows keyboard focus: the atmosphere accent, lifted.
    pub(in crate::ui) fn focus(self) -> Hsla {
        shift(self.accent, 0.16)
    }

    pub(in crate::ui) fn style(self, interaction: Interaction) -> Style {
        let p = self.parameters();
        let hover = f32::from(u8::from(interaction == Interaction::Hover));
        let pressed = f32::from(u8::from(interaction == Interaction::Pressed));
        let selected = f32::from(u8::from(self.selected));
        match self.kind {
            Kind::Porcelain => self.porcelain(&p, hover, pressed, selected),
            Kind::OpticalGlass | Kind::SmokedGlass => self.smoked(&p, hover, pressed, selected),
            Kind::Ghost => self.ghost(&p, hover, pressed, selected),
            Kind::Recessed => self.recessed(&p, hover, pressed, selected),
        }
    }

    fn porcelain(self, p: &Parameters, hover: f32, pressed: f32, selected: f32) -> Style {
        // One neutral contrast surface. The palette never fills a command, so
        // the primary action reads the same on every cover.
        let ceramic = Hsla::from(rgb(action::SURFACE));
        let lift = 0.05 * hover + 0.03 * selected;
        let hovered = shift(ceramic, lift);
        let sunken = shift(ceramic, -0.055 - p.inner_shadow * 0.03);
        let face = if pressed > 0.5 {
            sunken
        } else if hover > 0.5 {
            hovered
        } else {
            ceramic
        };
        Style {
            face: face.into(),
            flat: face,
            edge: None,
            ink: Hsla::from(rgb(action::GLYPH)),
        }
    }

    fn smoked(self, p: &Parameters, hover: f32, pressed: f32, selected: f32) -> Style {
        let base = Hsla::from(rgb(SMOKED_BASE));
        let alpha = (p.opacity + 0.03 * hover + 0.05 * pressed).min(1.);
        let top = shift(base, 0.035 + 0.02 * hover - 0.02 * pressed);
        let floor = shift(base, -0.012 * hover);
        Style {
            face: gradient(180., top, floor, alpha),
            flat: base.opacity(alpha),
            edge: Some(Hsla {
                h: base.h,
                s: base.s,
                l: 0.72,
                a: p.edge_intensity * 0.55 + 0.10 * hover,
            }),
            ink: if selected > 0.5 {
                self.accent
            } else {
                Hsla::from(rgb(glass::PRIMARY))
            },
        }
    }

    fn ghost(self, p: &Parameters, hover: f32, pressed: f32, selected: f32) -> Style {
        // Interaction paints a local wash; the resting state stays bare.
        let tint = Hsla::from(rgb(action::HOVER));
        let alpha = p.opacity + action::WASH * hover + 0.16 * pressed;
        Style {
            face: tint.opacity(alpha).into(),
            flat: tint.opacity(alpha),
            edge: None,
            ink: if selected > 0.5 {
                self.accent
            } else {
                Hsla::from(rgb(glass::SECONDARY))
            },
        }
    }

    fn recessed(self, p: &Parameters, hover: f32, pressed: f32, selected: f32) -> Style {
        let shell = Hsla::from(rgb(glass::CANVAS));
        let base = shell;
        let alpha = (p.opacity + 0.04 * hover + 0.04 * pressed).min(1.);
        // Dark at the top, lighter at the floor: the surface reads as sunk.
        let top = shift(base, -0.02 + 0.01 * hover);
        let floor = shift(base, 0.03 + 0.01 * hover + 0.01 * selected);
        Style {
            face: gradient(180., top, floor, alpha),
            flat: base.opacity(alpha),
            edge: Some(
                Hsla::from(rgb(action::HOVER)).opacity(p.edge_intensity * (0.6 + 0.4 * selected)),
            ),
            ink: Hsla::from(rgb(glass::PRIMARY)),
        }
    }
}

/// The atmosphere accent every control tints from: the brightest stop of the
/// palette active in this process. It never reads the artwork directly, so a
/// saturated cover cannot decide the text colour of a button.
pub(in crate::ui) fn accent(cx: &gpui::App) -> Hsla {
    Hsla::from(crate::ui::appearance::palette(cx).accent())
}

fn gradient(angle: f32, from: Hsla, to: Hsla, opacity: f32) -> Background {
    linear_gradient(
        angle,
        linear_color_stop(from.opacity(from.a * opacity), 0.),
        linear_color_stop(to.opacity(to.a * opacity), 1.),
    )
}

fn shift(color: Hsla, delta: f32) -> Hsla {
    Hsla {
        l: (color.l + delta).clamp(0., 1.),
        ..color
    }
}

/// Hue-aware mix along the shorter arc, so two palettes never meet through grey.
#[cfg(test)]
mod tests {
    use super::*;

    // One stop from each family: solar, abyss, monochrome, rosewood.
    const ACCENTS: [u32; 4] = [0xffb578, 0xa0e7e7, 0xe3e4ea, 0xc77a93];

    fn luminance(color: Hsla) -> f32 {
        let channel = |value: f32| {
            if value <= 0.03928 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        let color = gpui::Rgba::from(color);
        0.2126 * channel(color.r) + 0.7152 * channel(color.g) + 0.0722 * channel(color.b)
    }

    fn contrast(ink: Hsla, face: Hsla) -> f32 {
        let (ink, face) = (luminance(ink), luminance(face));
        (ink.max(face) + 0.05) / (ink.min(face) + 0.05)
    }

    #[test]
    fn porcelain_keeps_its_label_readable_on_every_palette() {
        for accent in ACCENTS.map(|color| Hsla::from(rgb(color))) {
            for interaction in [Interaction::Rest, Interaction::Hover, Interaction::Pressed] {
                let style = Material::new(Kind::Porcelain, accent).style(interaction);
                assert!(
                    contrast(style.ink, style.flat) >= 4.5,
                    "{interaction:?} porcelain contrast {}",
                    contrast(style.ink, style.flat)
                );
            }
        }
    }

    #[test]
    fn ghost_stays_bare_until_the_pointer_arrives() {
        let accent = Hsla::from(rgb(ACCENTS[0]));
        let material = Material::new(Kind::Ghost, accent);
        assert_eq!(material.style(Interaction::Rest).flat.a, 0.);
        assert!(material.style(Interaction::Hover).flat.a > 0.05);
        assert!(
            material.style(Interaction::Pressed).flat.a > material.style(Interaction::Hover).flat.a
        );
        assert!(material.style(Interaction::Rest).edge.is_none());
    }

    #[test]
    fn selection_changes_the_label_and_keeps_the_surface_neutral() {
        for accent in ACCENTS.map(|color| Hsla::from(rgb(color))) {
            let material = Material::new(Kind::SmokedGlass, accent);
            let rest = material.style(Interaction::Rest);
            let selected = material.selected(true).style(Interaction::Rest);
            assert_eq!(rest.flat, selected.flat);
            assert_eq!(rest.edge, selected.edge);
            assert_eq!(selected.ink, accent);
        }
    }
}
