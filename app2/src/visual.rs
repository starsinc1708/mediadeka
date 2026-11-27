//! Independent visual layers of the Fluid Glass engine.
//!
//! A palette chooses colours, a pattern chooses geometry, the optical and
//! post-processing flags choose the material, and a quality choice chooses the
//! render profile. None of these implies a separate shader.

use serde::{Deserialize, Serialize};

mod audio;
pub use audio::{AudioChannel, AudioProfile, AudioResponse};

/// Algorithm version kept stable so an object keeps its automatic pattern.
pub const PATTERN_SCHEMA: u32 = 1;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PatternId {
    #[default]
    ThermalFlow,
    FlutedGlass,
    Interference,
    SignalDistortion,
    OpticalCaustics,
    LightLeak,
    GemSmoke,
    Swirl,
    LiquidMetal,
}

impl PatternId {
    pub const ALL: [Self; 9] = [
        Self::ThermalFlow,
        Self::FlutedGlass,
        Self::Interference,
        Self::SignalDistortion,
        Self::OpticalCaustics,
        Self::LightLeak,
        Self::GemSmoke,
        Self::Swirl,
        Self::LiquidMetal,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ThermalFlow => "thermal-flow",
            Self::FlutedGlass => "fluted-glass",
            Self::Interference => "interference",
            Self::SignalDistortion => "signal-distortion",
            Self::OpticalCaustics => "optical-caustics",
            Self::LightLeak => "light-leak",
            Self::GemSmoke => "gem-smoke",
            Self::Swirl => "swirl",
            Self::LiquidMetal => "liquid-metal",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|pattern| pattern.as_str() == value)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::ThermalFlow => "Thermal Flow",
            Self::FlutedGlass => "Fluted Glass",
            Self::Interference => "Interference",
            Self::SignalDistortion => "Signal Distortion",
            Self::OpticalCaustics => "Optical Caustics",
            Self::LightLeak => "Light Leak",
            Self::GemSmoke => "Gem Smoke",
            Self::Swirl => "Swirl",
            Self::LiquidMetal => "Liquid Metal",
        }
    }

    /// Index in the packed GPU word. Must match `patterns/registry.wgsl`.
    pub const fn shader_id(self) -> u32 {
        match self {
            Self::ThermalFlow => 0,
            Self::FlutedGlass => 1,
            Self::Interference => 2,
            Self::SignalDistortion => 3,
            Self::OpticalCaustics => 4,
            Self::LightLeak => 5,
            Self::GemSmoke => 6,
            Self::Swirl => 7,
            Self::LiquidMetal => 8,
        }
    }

    /// Per-pattern artistic balance. The user's intensity multiplies this gain
    /// so a softer motif and a bright one share one control without one
    /// drowning the other.
    pub const fn gain(self) -> f32 {
        match self {
            Self::ThermalFlow => 1.15,
            Self::FlutedGlass => 1.0,
            Self::Interference => 0.95,
            Self::SignalDistortion => 0.95,
            Self::OpticalCaustics => 1.0,
            Self::LightLeak => 1.10,
            Self::GemSmoke => 0.90,
            Self::Swirl => 1.0,
            Self::LiquidMetal => 1.15,
        }
    }

    /// Artistic defaults. User intensity and speed replace two fields.
    pub const fn defaults(self) -> PatternParams {
        let (scale, speed, rotation, distortion, softness, complexity) = match self {
            Self::ThermalFlow => (1.0, 0.35, 0.0, 0.55, 0.62, 0.72),
            Self::FlutedGlass => (1.2, 0.15, 0.0, 0.18, 0.48, 0.42),
            Self::Interference => (1.05, 0.25, 0.0, 0.16, 0.30, 0.62),
            Self::SignalDistortion => (1.0, 0.40, 0.0, 0.52, 0.42, 0.50),
            Self::OpticalCaustics => (1.1, 0.28, 0.18, 0.42, 0.55, 0.72),
            Self::LightLeak => (0.9, 0.10, 0.08, 0.24, 0.80, 0.48),
            Self::GemSmoke => (1.0, 0.18, 0.28, 0.62, 0.68, 0.82),
            Self::Swirl => (1.1, 0.30, 0.0, 0.60, 0.52, 0.64),
            Self::LiquidMetal => (1.0, 0.22, 0.14, 0.46, 0.36, 0.82),
        };
        PatternParams {
            seed: 0,
            scale,
            speed,
            rotation,
            intensity: 0.65,
            distortion,
            softness,
            complexity,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PatternSelection {
    #[default]
    Auto,
    Fixed(PatternId),
}

impl PatternSelection {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Fixed(pattern) => pattern.as_str(),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        if value == "auto" {
            return Some(Self::Auto);
        }
        PatternId::parse(value).map(Self::Fixed)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Авто по странице",
            Self::Fixed(pattern) => pattern.label(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PatternParams {
    pub seed: u32,
    pub scale: f32,
    pub speed: f32,
    pub rotation: f32,
    pub intensity: f32,
    pub distortion: f32,
    pub softness: f32,
    pub complexity: f32,
}

impl PatternParams {
    /// User controls and the page seed shape the pattern without changing its
    /// mathematical identity.
    pub fn for_plan(pattern: PatternId, seed: u32, intensity: f32, speed: f32) -> Self {
        let base = pattern.defaults();
        let jitter = splitmix64(
            u64::from(seed) ^ (u64::from(pattern.shader_id()) << 8) ^ u64::from(PATTERN_SCHEMA),
        );
        let unit = |shift: u32| ((jitter >> shift) & 0xFF) as f32 / 255.0;
        // Fluted ribs are an architectural upright; other patterns may take a
        // full random orientation without losing their identity.
        let rotation = if pattern == PatternId::FlutedGlass {
            (base.rotation + (unit(16) - 0.5) * 0.10).rem_euclid(std::f32::consts::TAU)
        } else {
            (base.rotation + unit(16) * std::f32::consts::TAU).rem_euclid(std::f32::consts::TAU)
        };
        Self {
            seed: (jitter & 0xFF) as u32,
            scale: (base.scale * (0.90 + 0.20 * unit(0))).clamp(0.35, 2.6),
            speed: (speed * (0.75 + 0.35 * unit(8))).clamp(0.0, 1.0),
            rotation,
            intensity: (intensity * pattern.gain()).clamp(0.0, 1.0),
            distortion: (base.distortion * (0.82 + 0.30 * unit(24))).clamp(0.0, 1.0),
            softness: (base.softness * (0.86 + 0.24 * unit(32))).clamp(0.0, 1.0),
            complexity: (base.complexity * (0.84 + 0.28 * unit(40))).clamp(0.0, 1.0),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VisualFlags(u8);

impl VisualFlags {
    pub const FLUTED: u8 = 1 << 0;
    pub const REFRACTION: u8 = 1 << 1;
    pub const DISPERSION: u8 = 1 << 2;
    pub const GRAIN: u8 = 1 << 3;
    pub const BLOOM: u8 = 1 << 4;
    pub const GLITCH: u8 = 1 << 5;
    pub const VIGNETTE: u8 = 1 << 6;

    pub const ALL: Self = Self(0b0111_1111);
    pub const NONE: Self = Self(0);

    pub const fn from_bits(bits: u8) -> Self {
        Self(bits & Self::ALL.0)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, flag: u8) -> bool {
        self.0 & flag != 0
    }

    pub fn set(&mut self, flag: u8, enabled: bool) {
        if enabled {
            self.0 |= flag;
        } else {
            self.0 &= !flag;
        }
    }

    pub const fn enabled(self, flag: u8) -> bool {
        self.contains(flag)
    }
}

impl std::ops::BitOr for VisualFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitAnd for VisualFlags {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl Default for VisualFlags {
    fn default() -> Self {
        Self::ALL
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum QualityChoice {
    #[default]
    Auto,
    Ultra,
    High,
    Balanced,
    Eco,
    Static,
}

impl QualityChoice {
    pub const ALL: [Self; 6] = [
        Self::Auto,
        Self::Balanced,
        Self::High,
        Self::Ultra,
        Self::Eco,
        Self::Static,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ultra => "ultra",
            Self::High => "high",
            Self::Balanced => "balanced",
            Self::Eco => "eco",
            Self::Static => "static",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|quality| quality.as_str() == value)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Авто",
            Self::Ultra => "Ultra",
            Self::High => "High",
            Self::Balanced => "Balanced",
            Self::Eco => "Eco",
            Self::Static => "Static",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VisualSettings {
    pub pattern: PatternSelection,
    pub intensity: f32,
    pub speed: f32,
    pub effects: VisualFlags,
    pub quality: QualityChoice,
    pub audio: AudioResponse,
    pub surface: SurfaceAppearance,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceAppearance {
    pub opacity: u8,
    pub blur: u8,
    pub neutral: bool,
}

impl SurfaceAppearance {
    const fn standard() -> Self {
        Self {
            opacity: 50,
            blur: 50,
            neutral: false,
        }
    }
}

impl Default for SurfaceAppearance {
    fn default() -> Self {
        Self::standard()
    }
}

impl VisualSettings {
    /// Usable from `const` contexts such as the classic test appearance.
    pub const fn standard() -> Self {
        Self {
            pattern: PatternSelection::Auto,
            intensity: 0.65,
            speed: 0.35,
            effects: VisualFlags::ALL,
            quality: QualityChoice::Auto,
            audio: AudioResponse::standard(),
            surface: SurfaceAppearance::standard(),
        }
    }
}

impl VisualSettings {
    /// Test and acceptance overrides, mirroring `MEDIADEKA_GLASS_THEME`.
    pub fn with_env(mut self) -> Self {
        let var = |name: &str| std::env::var(name).ok();
        if let Some(pattern) =
            var("MEDIADEKA_GLASS_PATTERN").and_then(|value| PatternSelection::parse(value.trim()))
        {
            self.pattern = pattern;
        }
        let number = |name: &str| {
            var(name)
                .and_then(|value| value.trim().parse::<f32>().ok())
                .map(|value| (value / 100.).clamp(0., 1.))
        };
        if let Some(intensity) = number("MEDIADEKA_GLASS_INTENSITY") {
            self.intensity = intensity;
        }
        if let Some(speed) = number("MEDIADEKA_GLASS_SPEED") {
            self.speed = speed;
        }
        if let Some(effects) = var("MEDIADEKA_GLASS_EFFECTS") {
            self.effects = match effects.trim().to_ascii_lowercase().as_str() {
                "none" | "off" => VisualFlags::NONE,
                "all" | "on" => VisualFlags::ALL,
                names => {
                    let mut flags = VisualFlags::NONE;
                    for name in names.split(',').map(str::trim) {
                        let flag = match name {
                            "fluted" => VisualFlags::FLUTED,
                            "refraction" => VisualFlags::REFRACTION,
                            "dispersion" => VisualFlags::DISPERSION,
                            "grain" => VisualFlags::GRAIN,
                            "bloom" => VisualFlags::BLOOM,
                            "glitch" => VisualFlags::GLITCH,
                            "vignette" => VisualFlags::VIGNETTE,
                            _ => continue,
                        };
                        flags.set(flag, true);
                    }
                    flags
                }
            };
        }
        if let Some(quality) =
            var("MEDIADEKA_GLASS_QUALITY").and_then(|value| QualityChoice::parse(value.trim()))
        {
            self.quality = quality;
        }
        self
    }
}

impl Default for VisualSettings {
    fn default() -> Self {
        Self::standard()
    }
}

/// The automatic pattern plan of a page: a stable seed and an ordered set of
/// candidates. The same seed and schema always choose the same pattern.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PatternPlan {
    pub seed: u32,
    pub allowed: &'static [PatternId],
}

impl PatternPlan {
    pub const fn new(seed: u32, allowed: &'static [PatternId]) -> Self {
        Self { seed, allowed }
    }

    pub fn selected(self) -> Option<PatternId> {
        if self.allowed.is_empty() {
            return None;
        }
        let index = splitmix64(u64::from(self.seed) ^ u64::from(PATTERN_SCHEMA)) as usize
            % self.allowed.len();
        Some(self.allowed[index])
    }
}

fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automatic_selection_is_stable_and_uses_the_first_candidate() {
        let plan = PatternPlan::new(17, &[PatternId::Swirl, PatternId::ThermalFlow]);
        assert_eq!(plan.selected(), plan.selected());
        for seed in 0..256 {
            let plan = PatternPlan::new(seed, &[PatternId::Swirl, PatternId::ThermalFlow]);
            assert!(matches!(
                plan.selected(),
                Some(PatternId::Swirl | PatternId::ThermalFlow)
            ));
        }
        assert_eq!(PatternPlan::new(0, &[]).selected(), None);
    }

    #[test]
    fn every_pattern_has_a_independent_identity_and_round_trips() {
        for pattern in PatternId::ALL {
            assert_eq!(PatternId::parse(pattern.as_str()), Some(pattern));
            assert!(PatternSelection::parse(pattern.as_str()).is_some());
            assert_eq!(pattern.defaults().intensity, 0.65);
            assert!((0.8..=1.2).contains(&pattern.gain()));
        }
        assert_eq!(
            PatternSelection::parse("auto"),
            Some(PatternSelection::Auto)
        );
        assert_eq!(PatternSelection::parse("unknown"), None);
    }

    #[test]
    fn fluted_ribs_stay_upright_across_page_seeds() {
        for seed in 0..128 {
            let params = PatternParams::for_plan(PatternId::FlutedGlass, seed, 0.65, 0.35);
            let offset = params.rotation.min(std::f32::consts::TAU - params.rotation);
            assert!(
                offset <= 0.06,
                "seed {seed} rotated fluted ribs by {offset}"
            );
        }
    }

    #[test]
    fn palette_changes_do_not_enter_pattern_parameters() {
        let first = PatternParams::for_plan(PatternId::GemSmoke, 42, 0.6, 0.4);
        let second = PatternParams::for_plan(PatternId::GemSmoke, 42, 0.6, 0.4);
        assert_eq!(first, second);
        assert_ne!(
            first,
            PatternParams::for_plan(PatternId::GemSmoke, 43, 0.6, 0.4)
        );
    }

    #[test]
    fn flags_can_be_disabled_independently() {
        let mut flags = VisualFlags::ALL;
        for flag in [
            VisualFlags::FLUTED,
            VisualFlags::REFRACTION,
            VisualFlags::DISPERSION,
            VisualFlags::GRAIN,
            VisualFlags::BLOOM,
            VisualFlags::GLITCH,
            VisualFlags::VIGNETTE,
        ] {
            flags.set(flag, false);
            assert!(!flags.enabled(flag));
            flags.set(flag, true);
            assert!(flags.enabled(flag));
        }
        assert_eq!(VisualFlags::from_bits(0xFF).bits(), VisualFlags::ALL.bits());
    }
}
