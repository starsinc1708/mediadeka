use gpui::Window;

use crate::ui::fluid_glass::tokens::{self, space};

const COMPACT_HEIGHT: f32 = 600.;
const COMPACT_WIDTH: f32 = 1100.;
const TRANSPORT_FRACTION: f32 = 0.34;
const TRANSPORT_MIN: f32 = 196.;
const TRANSPORT_MAX: f32 = 520.;
const METADATA_MAX: f32 = 360.;
const VOLUME_COMPACT: f32 = 64.;
const VOLUME_STANDARD: f32 = 96.;

#[derive(Clone, Copy)]
pub(super) struct Layout {
    pub compact: bool,
    pub transport_width: f32,
    pub labeled_metadata: bool,
}

impl Layout {
    pub(super) fn for_window(window: &Window) -> Self {
        let viewport = window.viewport_size();
        Self::resolve(viewport.width.into(), viewport.height.into())
    }

    fn resolve(width: f32, height: f32) -> Self {
        Self {
            compact: height < COMPACT_HEIGHT || width < COMPACT_WIDTH,
            transport_width: (width * TRANSPORT_FRACTION).clamp(TRANSPORT_MIN, TRANSPORT_MAX),
            labeled_metadata: width >= tokens::LISTENING_TOOL_MIN_WIDTH,
        }
    }

    pub(super) fn inset(self) -> f32 {
        if self.compact { space::LG } else { space::XL }
    }

    pub(super) fn gap(self) -> f32 {
        if self.compact { space::SM } else { space::XL }
    }

    pub(super) fn metadata_width(self) -> f32 {
        METADATA_MAX
    }

    pub(super) fn volume_width(self) -> f32 {
        if self.compact {
            VOLUME_COMPACT
        } else {
            VOLUME_STANDARD
        }
    }
}
