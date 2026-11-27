mod oklch;

use gpui::{Rgba, rgb};

use crate::{appearance::GlassPalette, ui::components::artwork::ArtworkColors};

use oklch::{Oklch, blend_light, interpolate};

// Background, then four light stops. The shader receives the four light
// stops as RGB and the background as the canvas; the fifth stop is the
// reflection highlight. Palettes never select a pattern or a shader.
const SOLAR: [u32; 5] = [0x09090d, 0x44121c, 0xa92d32, 0xf15c35, 0xffb578];
const ABYSS: [u32; 5] = [0x070c14, 0x102c46, 0x165b88, 0x239ab4, 0xa0e7e7];
const ULTRAVIOLET: [u32; 5] = [0x0b0813, 0x301545, 0x653398, 0xb054d1, 0xe9b7ff];
const MONOCHROME: [u32; 5] = [0x09090b, 0x25262b, 0x51535c, 0x9699a4, 0xe3e4ea];
const EMBER: [u32; 5] = [0x0b0909, 0x310c0c, 0x8c1a20, 0xce4234, 0xef8057];
const ACID: [u32; 5] = [0x090e0c, 0x132a21, 0x41634a, 0x9ac75a, 0xe1f3aa];
const GLACIER: [u32; 5] = [0x0b1118, 0x253f57, 0x507d9d, 0x8eb9cb, 0xd9f0ee];
const ROSEWOOD: [u32; 5] = [0x110c12, 0x452236, 0x8c4262, 0xc77a93, 0xf2c3cf];

const MIN_COLOR_SHARE: f32 = 0.045;
const MIN_COLOR_CHROMA: f32 = 0.045;
const DISTINCT_HUE: f32 = 0.58;

pub(super) enum Cover {
    Pending(Option<String>),
    Settled(Option<String>, Option<[u8; 32]>),
}

impl Cover {
    pub(super) fn empty() -> Self {
        Self::Settled(None, None)
    }

    pub(super) fn url(&self) -> Option<&str> {
        match self {
            Self::Pending(current) | Self::Settled(current, _) => current.as_deref(),
        }
    }

    pub(super) fn select(&mut self, url: Option<&str>) -> bool {
        if self.url() == url {
            return false;
        }
        *self = Self::Pending(url.map(str::to_owned));
        true
    }

    /// The outer option means a lookup is complete, even if no colors are available.
    pub(super) fn resolve(
        &mut self,
        revision: Option<[u8; 32]>,
        cached_colors: impl FnOnce(&str) -> Option<Option<ArtworkColors>>,
    ) -> Option<Option<ArtworkColors>> {
        if let Self::Settled(source @ Some(_), known) = self
            && revision.is_some()
            && revision != *known
        {
            *self = Self::Pending(source.take());
        }
        let Self::Pending(source) = self else {
            return None;
        };
        let result = match source {
            Some(url) => cached_colors(url)?,
            None => None,
        };
        *self = Self::Settled(source.take(), revision);
        Some(result)
    }

    #[cfg(feature = "capture")]
    pub(super) fn settled_for(&self, url: &str) -> bool {
        matches!(self, Self::Settled(Some(current), _) if current == url)
    }
}

impl GlassPalette {
    pub(super) fn palette(self, artwork: Option<ArtworkColors>) -> Palette {
        if self == Self::Auto {
            return artwork.map_or_else(
                || Self::Solar.palette(None),
                |artwork| Palette::from_swatches(artwork.swatches()),
            );
        }

        let stops = match self {
            Self::Auto | Self::Solar => SOLAR,
            Self::Abyss => ABYSS,
            Self::Ultraviolet => ULTRAVIOLET,
            Self::Monochrome => MONOCHROME,
            Self::Ember => EMBER,
            Self::Acid => ACID,
            Self::Glacier => GLACIER,
            Self::Rosewood => ROSEWOOD,
        };
        fixed_palette(stops)
    }

    fn for_hue(color: Oklch) -> Self {
        let degrees = color.hue.to_degrees();
        if (82. ..272.).contains(&degrees) {
            Self::Abyss
        } else if (272. ..345.).contains(&degrees) {
            Self::Ultraviolet
        } else {
            Self::Solar
        }
    }

    /// The brightest stop of the active palette. Component materials tint from
    /// it, so controls follow the chosen atmosphere without reading artwork.
    pub(in crate::ui) fn accent(self) -> Rgba {
        if self == Self::Auto {
            rgb(super::tokens::ACCENT)
        } else {
            self.palette(None).colors[3]
        }
    }
}

fn fixed_palette(stops: [u32; 5]) -> Palette {
    Palette {
        colors: [rgb(stops[1]), rgb(stops[2]), rgb(stops[3]), rgb(stops[4])],
        shadow: rgb(stops[0]),
        reflection: rgb(stops[4]),
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Palette {
    pub(super) colors: [Rgba; 4],
    pub(super) shadow: Rgba,
    pub(super) reflection: Rgba,
}

impl Palette {
    pub(super) fn page(artwork: ArtworkColors) -> Self {
        let colorful = colorful_swatches(artwork.swatches());
        let Some(&primary) = colorful.first() else {
            return GlassPalette::Monochrome.palette(None);
        };
        // A single-hue artwork retains its hue through the shadow and
        // highlights. A second or third hue is introduced only by the artwork.
        let colors = || colorful.iter().copied();
        let secondary = colors()
            .find(|color| color.hue_distance(primary) >= DISTINCT_HUE)
            .unwrap_or(primary);
        let third = colors()
            .find(|color| {
                color.hue_distance(primary) >= DISTINCT_HUE
                    && color.hue_distance(secondary) >= DISTINCT_HUE
            })
            .unwrap_or(secondary);
        let stop = |color: Oklch, lightness: f32, chroma: f32| {
            Oklch {
                lightness,
                chroma: color.chroma.min(chroma),
                ..color
            }
            .to_rgb()
        };
        let highlight = colors()
            .max_by(|a, b| a.lightness.total_cmp(&b.lightness))
            .unwrap_or(primary);
        Self {
            colors: [
                stop(primary, primary.lightness.clamp(0.54, 0.74), 0.19),
                stop(secondary, 0.48, 0.146),
                stop(third, 0.37, 0.113),
                stop(highlight, 0.78, 0.12),
            ],
            shadow: stop(primary, 0.154, 0.016),
            reflection: stop(highlight, 0.82, 0.075),
        }
    }

    fn from_swatches(swatches: &[(u32, f32)]) -> Self {
        let colorful = colorful_swatches(swatches);
        let Some(&primary) = colorful.first() else {
            return GlassPalette::Monochrome.palette(None);
        };
        let secondary = colorful
            .iter()
            .copied()
            .find(|color| color.hue_distance(primary) >= DISTINCT_HUE);
        let tertiary = colorful.iter().copied().find(|color| {
            color.hue_distance(primary) >= DISTINCT_HUE
                && secondary.is_some_and(|secondary| color.hue_distance(secondary) >= DISTINCT_HUE)
        });
        let highlight = colorful
            .iter()
            .copied()
            .max_by(|a, b| a.lightness.total_cmp(&b.lightness))
            .unwrap_or(primary);
        let mut palette = GlassPalette::for_hue(primary).palette(None);
        let enclosed_light = |color: Oklch| {
            Oklch {
                lightness: color.lightness.clamp(0.42, 0.72),
                chroma: color.chroma.min(0.19),
                ..color
            }
            .to_rgb()
        };
        for (slot, artwork, strength) in [
            (0, Some(primary), 0.62),
            (1, secondary, 0.85),
            (2, tertiary, 0.72),
        ] {
            if let Some(artwork) = artwork {
                palette.colors[slot] =
                    blend_light(palette.colors[slot], enclosed_light(artwork), strength);
            }
        }
        palette.reflection = reflection_from(palette.colors[0]);
        let reflection = Oklch {
            lightness: highlight.lightness.clamp(0.64, 0.79),
            chroma: highlight.chroma.min(0.095),
            ..highlight
        };
        palette.colors[3] = blend_light(palette.colors[3], reflection.to_rgb(), 0.48);
        let shadow_color = tertiary.or(secondary).unwrap_or(primary);
        let shadow = Oklch {
            lightness: 0.16,
            chroma: 0.019,
            ..shadow_color
        };
        palette.shadow = blend_light(palette.shadow, shadow.to_rgb(), 0.55);
        palette
    }

    pub(super) fn interpolate(self, target: Self, progress: f32) -> Self {
        match progress {
            progress if progress <= 0. => self,
            progress if progress >= 1. => target,
            progress => Self {
                colors: std::array::from_fn(|index| {
                    interpolate(self.colors[index], target.colors[index], progress)
                }),
                shadow: interpolate(self.shadow, target.shadow, progress),
                reflection: interpolate(self.reflection, target.reflection, progress),
            },
        }
    }
}

fn colorful_swatches(swatches: &[(u32, f32)]) -> Vec<Oklch> {
    swatches
        .iter()
        .filter(|(_, share)| *share >= MIN_COLOR_SHARE)
        .map(|(color, _)| Oklch::from_rgb(rgb(*color)))
        .filter(|color| color.chroma >= MIN_COLOR_CHROMA)
        .collect()
}

fn reflection_from(warm: Rgba) -> Rgba {
    let channel = |value| 0.62 + 0.38 * value;
    Rgba {
        r: channel(warm.r),
        g: channel(warm.g),
        b: channel(warm.b),
        a: 1.,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Field {
    pub(super) player: Palette,
    pub(super) upper: Palette,
}

impl Field {
    // At the leading edge of the light transfer, 22% of the transition
    // elapses before the upper canvas starts to change.
    const UPPER_DELAY: f32 = 0.22;

    pub(super) fn uniform(palette: Palette) -> Self {
        Self {
            player: palette,
            upper: palette,
        }
    }

    pub(super) fn toward(self, target: Palette, progress: f32) -> Self {
        if progress >= 1. || self == Self::uniform(target) {
            return Self::uniform(target);
        }
        let ease = |position: f32| {
            let position = position.clamp(0., 1.);
            position * position * (3. - 2. * position)
        };
        let active = 1. - Self::UPPER_DELAY;
        Self {
            player: self.player.interpolate(target, ease(progress / active)),
            upper: self
                .upper
                .interpolate(target, ease((progress - Self::UPPER_DELAY) / active)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_waits_for_the_current_result_and_settles_once() {
        let mut cover = Cover::empty();
        assert!(cover.select(Some("first")));
        assert_eq!(cover.resolve(None, |_| None), None);
        assert!(!cover.select(Some("first")));
        assert!(cover.select(Some("second")));
        assert_eq!(
            cover.resolve(Some([1; 32]), |url| {
                assert_eq!(url, "second");
                Some(None)
            }),
            Some(None)
        );
        assert_eq!(
            cover.resolve(Some([1; 32]), |_| panic!("settled cover read again")),
            None
        );
        assert_eq!(
            cover.resolve(None, |_| panic!("evicted original settled again")),
            None
        );
        assert_eq!(cover.resolve(Some([2; 32]), |_| None), None);
        assert_eq!(cover.resolve(Some([2; 32]), |_| Some(None)), Some(None));
        assert!(cover.select(None));
        assert_eq!(
            cover.resolve(None, |_| panic!("empty cover accessed the cache")),
            Some(None)
        );
        assert!(cover.select(Some("missing")));
        assert_eq!(cover.resolve(None, |_| Some(None)), Some(None));
        assert_eq!(
            cover.resolve(None, |_| panic!("failed cover read again")),
            None
        );
    }

    #[test]
    fn artwork_hue_bases_and_absent_or_monochrome_art() {
        for (color, family) in [
            (0xdd6641, GlassPalette::Solar),
            (0x31a5b8, GlassPalette::Abyss),
            (0x8d3dc5, GlassPalette::Ultraviolet),
        ] {
            assert_eq!(GlassPalette::for_hue(Oklch::from_rgb(rgb(color))), family);
            let auto = Palette::from_swatches(&[(color, 1.)]);
            let fixed = family.palette(None);
            assert_eq!(auto.colors[1..3], fixed.colors[1..3]);
            assert_ne!(auto.colors[0], fixed.colors[0]);
        }
        assert_eq!(
            Palette::from_swatches(&[(0x919399, 0.98), (0xff1010, 0.02)]),
            GlassPalette::Monochrome.palette(None)
        );
        assert_eq!(
            GlassPalette::Auto.palette(None),
            GlassPalette::Solar.palette(None)
        );
    }

    #[test]
    fn a_transition_keeps_its_endpoints_and_mixes_lightness_perceptually() {
        let dark = Palette {
            colors: [rgb(0); 4],
            shadow: rgb(0),
            reflection: rgb(0),
        };
        let light = Palette {
            colors: [rgb(0xffffff); 4],
            shadow: rgb(0xffffff),
            reflection: rgb(0xffffff),
        };
        assert_eq!(dark.interpolate(light, -1.), dark);
        assert_eq!(dark.interpolate(light, 2.), light);
        let half = dark.interpolate(light, 0.5);
        assert!((Oklch::from_rgb(half.colors[0]).lightness - 0.5).abs() < 0.001);
        assert_eq!(half.colors[0].a, 1.);
    }

    #[test]
    fn light_leads_from_the_player_and_retargeting_preserves_both_visible_colors() {
        let previous = GlassPalette::Solar.palette(None);
        let target = GlassPalette::Abyss.palette(None);
        let start = Field::uniform(previous);
        let early = start.toward(target, 0.15);
        assert_ne!(early.player, previous);
        assert_eq!(early.upper, previous);
        let middle = start.toward(target, 0.5);
        assert_ne!(middle.player, middle.upper);
        assert_ne!(middle.upper, previous);
        let redirected = GlassPalette::Ultraviolet.palette(None);
        assert_eq!(middle.toward(redirected, 0.), middle);
        assert_eq!(middle.toward(redirected, 1.), Field::uniform(redirected));
    }
}
