use std::f32::consts::{PI, TAU};

use gpui::Rgba;

const ACHROMATIC: f32 = 0.001;

#[derive(Clone, Copy)]
pub(super) struct Oklch {
    pub(super) lightness: f32,
    pub(super) chroma: f32,
    pub(super) hue: f32,
}

impl Oklch {
    pub(super) fn from_rgb(rgb: Rgba) -> Self {
        // Björn Ottosson's D65 matrices for linear sRGB and Oklab.
        let [r, g, b] = [rgb.r, rgb.g, rgb.b].map(to_linear);
        let l = (0.412_221_46 * r + 0.536_332_55 * g + 0.051_445_995 * b).cbrt();
        let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
        let s = (0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b).cbrt();
        let a = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
        let b = 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s;
        Self {
            lightness: 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s,
            chroma: a.hypot(b),
            hue: b.atan2(a).rem_euclid(TAU),
        }
    }

    pub(super) fn hue_distance(self, other: Self) -> f32 {
        shortest_arc(self.hue, other.hue).abs()
    }

    pub(super) fn to_rgb(self) -> Rgba {
        let (sin, cos) = self.hue.sin_cos();
        let linear = |chroma: f32| {
            let a = chroma * cos;
            let b = chroma * sin;
            let l = (self.lightness + 0.396_337_78 * a + 0.215_803_76 * b).powi(3);
            let m = (self.lightness - 0.105_561_346 * a - 0.063_854_17 * b).powi(3);
            let s = (self.lightness - 0.089_484_18 * a - 1.291_485_5 * b).powi(3);
            [
                4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
                -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
                -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
            ]
        };
        let in_gamut = |channels: [f32; 3]| channels.into_iter().all(|v| (0. ..=1.).contains(&v));
        let mut channels = linear(self.chroma);
        if !in_gamut(channels) {
            // Reduce chroma at constant lightness and hue to fit the display.
            let (mut low, mut high) = (0., self.chroma);
            for _ in 0..12 {
                let middle = (low + high) * 0.5;
                if in_gamut(linear(middle)) {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            channels = linear(low);
        }
        from_linear(channels)
    }
}

pub(super) fn interpolate(from: Rgba, to: Rgba, amount: f32) -> Rgba {
    if from == to {
        return from;
    }
    let from = Oklch::from_rgb(from);
    let to = Oklch::from_rgb(to);
    let hue = if from.chroma < ACHROMATIC {
        to.hue
    } else if to.chroma < ACHROMATIC {
        from.hue
    } else {
        from.hue + shortest_arc(from.hue, to.hue) * amount
    };
    Oklch {
        lightness: mix(from.lightness, to.lightness, amount),
        chroma: mix(from.chroma, to.chroma, amount),
        hue,
    }
    .to_rgb()
}

pub(super) fn blend_light(from: Rgba, to: Rgba, amount: f32) -> Rgba {
    let channel = |from, to| mix(to_linear(from), to_linear(to), amount);
    from_linear([
        channel(from.r, to.r),
        channel(from.g, to.g),
        channel(from.b, to.b),
    ])
}

fn shortest_arc(from: f32, to: f32) -> f32 {
    (to - from + PI).rem_euclid(TAU) - PI
}

fn mix(from: f32, to: f32, amount: f32) -> f32 {
    from * (1. - amount) + to * amount
}

fn to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn from_linear([r, g, b]: [f32; 3]) -> Rgba {
    let srgb = |value: f32| {
        let value = value.clamp(0., 1.);
        if value <= 0.0031308 {
            value * 12.92
        } else {
            1.055 * value.powf(1. / 2.4) - 0.055
        }
    };
    Rgba {
        r: srgb(r),
        g: srgb(g),
        b: srgb(b),
        a: 1.,
    }
}

#[cfg(test)]
mod tests {
    use gpui::rgb;

    use super::*;

    #[test]
    fn srgb_round_trip_and_hue_preserving_gamut_mapping() {
        let color = rgb(0x9344cd);
        let original = Oklch::from_rgb(color);
        let round_trip = original.to_rgb();
        assert!((color.r - round_trip.r).abs() < 0.001);
        assert!((color.g - round_trip.g).abs() < 0.001);
        assert!((color.b - round_trip.b).abs() < 0.001);

        let saturated = Oklch {
            lightness: 0.76,
            chroma: 0.8,
            ..original
        };
        let displayable = Oklch::from_rgb(saturated.to_rgb());
        assert!(displayable.chroma < saturated.chroma);
        assert!((displayable.lightness - saturated.lightness).abs() < 0.001);
        assert!(displayable.hue_distance(original) < 0.01);
    }
}
