pub(in crate::ui) const FONT: &str = "Segoe UI Variable";

pub(in crate::ui) const CANVAS: u32 = 0x0e0d12;
pub(in crate::ui) const SIDEBAR: u32 = 0x17151d;
pub(in crate::ui) const SURFACE: u32 = 0x201e26;
pub(in crate::ui) const ELEVATED: u32 = 0x2b2832;
pub(in crate::ui) const HOVER: u32 = 0x37333e;
pub(in crate::ui) const PRIMARY: u32 = 0xf0edf4;
pub(in crate::ui) const SECONDARY: u32 = 0xb5afbe;
pub(in crate::ui) const MUTED: u32 = 0x9a94a3;
pub(in crate::ui) const ACCENT: u32 = 0xffb778;
pub(in crate::ui) const ACTIVE: u32 = 0x392b25;
pub(in crate::ui) const RULE: u32 = 0x3b3642;
pub(in crate::ui) const ERROR: u32 = 0xff9b9e;
pub(in crate::ui) const WARNING: u32 = 0xffcb92;
pub(in crate::ui) const SUCCESS: u32 = 0xa8d4b0;
pub(in crate::ui) const ARTIST_STAGE_MAX_WIDTH: f32 = 1320.;

pub(in crate::ui) mod text {
    pub const HERO_TITLE: f32 = 48.;
    pub const PAGE_TITLE: f32 = 32.;
    pub const PAGE_TITLE_COMPACT: f32 = 28.;
    pub const SECTION_TITLE: f32 = 22.;
    pub const CARD_TITLE: f32 = 15.;
    pub const TRACK_TITLE: f32 = 15.;
    pub const NAVIGATION: f32 = 14.;
    pub const BODY: f32 = 14.;
    pub const METADATA: f32 = 13.;
    pub const CAPTION: f32 = 12.;
    pub const UTILITY: f32 = 11.;
}

pub(in crate::ui) mod space {
    pub const XS: f32 = 4.;
    pub const SM: f32 = 8.;
    pub const MD: f32 = 12.;
    pub const LG: f32 = 16.;
    pub const XL: f32 = 24.;
    pub const SECTION: f32 = 32.;
    pub const BLOCK: f32 = 48.;
}

pub(in crate::ui) mod radius {
    pub const THUMB: f32 = 4.;
    pub const CONTROL: f32 = 8.;
    pub const INPUT: f32 = 10.;
    pub const POPOVER: f32 = 14.;
    pub const PILL: f32 = 999.;
}

pub(in crate::ui) mod size {
    pub const XS: f32 = 28.;
    pub const SM: f32 = 32.;
    pub const MD: f32 = 36.;
    pub const LG: f32 = 40.;
    pub const PLAY: f32 = 48.;
}

pub(in crate::ui) mod selection {
    pub const LIGHT: u32 = super::ACCENT;
    pub const TRACK_TINT: f32 = 0.04;
    pub const MARKER: f32 = 0.85;
}

pub(in crate::ui) mod atmosphere {
    pub const ARTIST: f32 = 0.85;
    pub const ALBUM: f32 = 0.75;
    pub const HOME: f32 = 0.65;
    pub const PLAYLIST: f32 = 0.50;
    pub const LIBRARY: f32 = 0.30;
    pub const SEARCH: f32 = 0.20;
    pub const SETTINGS: f32 = 0.15;
    pub const LYRICS: f32 = 0.28;
    pub const NOW_PLAYING: f32 = 0.64;
}

pub(in crate::ui) mod control {
    pub const FOCUS: u32 = super::PRIMARY;

    pub const SLIDER_LEVEL: u32 = super::ACCENT;
}

pub(in crate::ui) mod contrast {
    pub const CANVAS: u32 = 0x050507;
    pub const SURFACE: u32 = 0x101016;
    pub const ELEVATED: u32 = 0x22222d;
    pub const SELECTED: u32 = 0x303342;
    pub const RULE: u32 = 0x77758a;
    pub const PRIMARY: u32 = 0xffffff;
    pub const SECONDARY: u32 = 0xe4e2ed;
    pub const ACCENT: u32 = 0xf8e6bd;
    pub const SHELL_STROKE: f32 = 2.;
}

pub(super) const SOLAR_ORANGE: u32 = 0xff5429;
pub(super) const HOT_MAGENTA: u32 = 0xe52865;
pub(super) const DEEP_BLUE: u32 = 0x254dc4;
pub(super) const ELECTRIC_CYAN: u32 = 0x30b9dd;
pub(super) const WARM_LIGHT: u32 = 0xffd0a3;

pub(in crate::ui) const INSET: f32 = 20.;
pub(in crate::ui) const INSET_COMPACT: f32 = 8.;
pub(in crate::ui) const RAIL: f32 = 248.;
pub(in crate::ui) const RAIL_COMPACT: f32 = 72.;
pub(super) const RAIL_EXPAND_START: f32 = 940.;
pub(super) const RAIL_EXPAND_END: f32 = 1340.;
pub(super) const RAIL_LABEL_WIDTH: f32 = 184.;
pub(in crate::ui) const PLAYER: f32 = 100.;
pub(in crate::ui) const PLAYER_COMPACT: f32 = 62.;
pub(super) const PLAYER_COVER_COMPACT: f32 = 37.;
pub(super) const PLAYER_COVER_NARROW: f32 = 54.;
pub(super) const PLAYER_COVER_WIDE: f32 = 68.;
pub(in crate::ui) const LISTENING_TOOL_MIN_WIDTH: f32 = 1120.;
pub(super) const SHELL_EXPAND_START: f32 = 460.;
pub(super) const SHELL_EXPAND_END: f32 = 600.;
pub(super) const OUTER_RADIUS: f32 = 24.;
pub(super) const OUTER_RADIUS_COMPACT: f32 = 17.;
pub(super) const INNER_RADIUS: f32 = 56.;
pub(super) const INNER_RADIUS_COMPACT: f32 = 24.;
pub(super) const SEARCH_MAX: f32 = 550.;
pub(super) const SEARCH_MAX_COMPACT: f32 = 224.;
pub(super) const WAVEFORM_MIN_WIDTH: f32 = 1280.;
pub(super) const WAVEFORM_MIN_HEIGHT: f32 = 600.;
pub(super) const WAVEFORM_MAX_WIDTH: f32 = 144.;
pub(super) const WAVEFORM_HEIGHT: f32 = 28.;
pub(super) const WAVEFORM_CENTER_GAP: f32 = 136.;
pub(super) const WAVEFORM_CENTER_FROM_BOTTOM: f32 = 0.60;
pub(in crate::ui) const TITLEBAR: f32 = 48.;
// Leave room for the fixed navigation rows above the 100 DIP player.
pub(in crate::ui) const NAV_FULL_MIN_HEIGHT: f32 = 720.;
pub(in crate::ui) const NAV_FULL_PINNED_MIN_HEIGHT: f32 = 760.;
pub(in crate::ui) const NAV_DENSE_PINNED_MAX_HEIGHT: f32 = 480.;
pub(in crate::ui) const NAV_SHORT_SEPARATOR: f32 = 14.;
pub(in crate::ui) const NAV_ROW: f32 = 40.;
pub(in crate::ui) const NAV_GAP: f32 = 6.;
pub(in crate::ui) const NAV_DENSE_ROW: f32 = 28.;
pub(in crate::ui) const NAV_DENSE_GAP: f32 = 2.;
pub(in crate::ui) const NAV_DURATION: std::time::Duration =
    crate::ui::design_system::motion::MOTION.normal();
pub(in crate::ui) const PAGE_DURATION: std::time::Duration =
    crate::ui::design_system::motion::MOTION.page();
pub(in crate::ui) const PAGE_OFFSET: f32 = 6.;
pub(in crate::ui) const CONTENT_PADDING: f32 = 40.;
pub(in crate::ui) const CONTENT_PADDING_COMPACT: f32 = 16.;
pub(super) const CONTENT_BOTTOM: f32 = 12.;
pub(super) const CONTENT_BOTTOM_COMPACT: f32 = 4.;
