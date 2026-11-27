use std::time::Duration;

use gpui::Window;

use crate::visual::QualityChoice;

const HIGH_RESOLUTION_AREA: f32 = 2560. * 1440.;

pub(super) struct QualitySelection {
    fixed: Option<Quality>,
    software: bool,
}

impl QualitySelection {
    pub(super) fn configured(window: &Window, choice: QualityChoice) -> Self {
        // An explicit environment profile is an acceptance override; otherwise
        // the user's performance profile chooses the fixed quality.
        let fixed = Quality::configured().or_else(|| Quality::of(choice));
        Self {
            fixed,
            software: fixed.is_none()
                && window
                    .gpu_specs()
                    .is_some_and(|gpu| gpu.is_software_emulated),
        }
    }

    pub(super) fn effective(&self, window: &Window, reduced_motion: bool) -> Quality {
        let viewport = window.viewport_size();
        let scale = window.scale_factor();
        self.for_area(
            reduced_motion,
            f32::from(viewport.width) * f32::from(viewport.height) * scale * scale,
        )
    }

    fn for_area(&self, reduced_motion: bool, physical_area: f32) -> Quality {
        if reduced_motion {
            Quality::Static
        } else if let Some(fixed) = self.fixed {
            fixed
        } else if self.software || physical_area > HIGH_RESOLUTION_AREA {
            Quality::Eco
        } else {
            Quality::Balanced
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Quality {
    Ultra,
    High,
    Balanced,
    Eco,
    Static,
}

impl Quality {
    fn of(choice: QualityChoice) -> Option<Self> {
        match choice {
            QualityChoice::Auto => None,
            QualityChoice::Ultra => Some(Self::Ultra),
            QualityChoice::High => Some(Self::High),
            QualityChoice::Balanced => Some(Self::Balanced),
            QualityChoice::Eco => Some(Self::Eco),
            QualityChoice::Static => Some(Self::Static),
        }
    }

    /// A representative profile for the live settings preview. Auto uses High
    /// so the preview is sharp on a small card without pretending to be Ultra.
    pub(super) fn preview(choice: QualityChoice) -> Self {
        match choice {
            QualityChoice::Auto | QualityChoice::Ultra | QualityChoice::High => Self::High,
            QualityChoice::Balanced => Self::Balanced,
            QualityChoice::Eco => Self::Eco,
            QualityChoice::Static => Self::Static,
        }
    }

    pub(super) fn configured() -> Option<Self> {
        match std::env::var("MEDIADEKA_GLASS_QUALITY")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "ultra" => Some(Self::Ultra),
            "high" => Some(Self::High),
            "balanced" => Some(Self::Balanced),
            "eco" => Some(Self::Eco),
            "static" => Some(Self::Static),
            _ => None,
        }
    }

    pub(super) fn raster_scale(self) -> f32 {
        match self {
            Self::Ultra => 1.,
            Self::High => 0.75,
            Self::Balanced | Self::Static => 0.5,
            Self::Eco => 0.375,
        }
    }

    pub(super) fn frame_interval(self) -> Option<Duration> {
        let fps = self.frames_per_second();
        (fps > 0).then(|| Duration::from_micros(1_000_000u64.div_ceil(u64::from(fps))))
    }

    pub(super) fn frames_per_second(self) -> u32 {
        match self {
            Self::Ultra | Self::High => 60,
            Self::Balanced => 30,
            Self::Eco => 15,
            Self::Static => 0,
        }
    }

    pub(super) fn reacts_to_audio(self) -> bool {
        !matches!(self, Self::Eco | Self::Static)
    }

    pub(super) fn glitch(self) -> bool {
        !matches!(self, Self::Eco | Self::Static)
    }

    /// Two octaves carry the visible spectrum; a third only adds compile time
    /// to the single FXC-optimized pipeline without changing the art direction.
    pub(super) fn warp_octaves(self) -> u32 {
        match self {
            Self::Ultra | Self::High | Self::Balanced => 2,
            Self::Eco | Self::Static => 1,
        }
    }

    #[cfg(feature = "capture")]
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Ultra => "ultra",
            Self::High => "high",
            Self::Balanced => "balanced",
            Self::Eco => "eco",
            Self::Static => "static",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn follows_physical_pixel_budget_and_accessibility_preference() {
        let auto = QualitySelection {
            fixed: None,
            software: false,
        };
        assert_eq!(auto.for_area(false, 1920. * 1080.), Quality::Balanced);
        assert_eq!(auto.for_area(false, 3840. * 2160.), Quality::Eco);
        let software = QualitySelection {
            software: true,
            ..auto
        };
        assert_eq!(software.for_area(false, 1920. * 1080.), Quality::Eco);
        let ultra = QualitySelection {
            fixed: Some(Quality::Ultra),
            software: false,
        };
        assert_eq!(ultra.for_area(false, 3840. * 2160.), Quality::Ultra);
        assert_eq!(ultra.for_area(true, 3840. * 2160.), Quality::Static);
        assert_eq!(Quality::Static.frame_interval(), None);
    }
}
