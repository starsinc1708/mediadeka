use std::borrow::Cow;

use gpui::{App, Global, Hsla, hsla, rgb};

pub(in crate::ui) const WINDOW: u32 = 0x11100f;
pub(in crate::ui) const SURFACE: u32 = 0x1e1b18;
pub(in crate::ui) const ELEVATED: u32 = 0x2a2520;
pub(in crate::ui) const ACCENT: u32 = 0xe4bb98;
pub(in crate::ui) const ACTIVE: u32 = 0x3b322d;
pub(in crate::ui) const PRIMARY: u32 = 0xf4f0eb;
pub(in crate::ui) const SECONDARY: u32 = 0xc3bab1;
pub(in crate::ui) const DIVIDER: u32 = 0x3a342f;
pub(in crate::ui) const FONT_FAMILY: &str = "Golos Text";

pub(in crate::ui) mod playlist {
    pub const PAPER: u32 = super::WINDOW;
    pub const PANEL: u32 = super::SURFACE;
    pub const POPOVER: u32 = 0x211e1b;
    pub const RULE: u32 = super::DIVIDER;
    pub const HOVER: u32 = super::ELEVATED;
    pub const SELECTED: u32 = super::ACTIVE;
    pub const SELECTION_EDGE: u32 = 0xc59d7d;
    pub const TRACK_LINE: u32 = 0x64574c;
    pub const PLAYING: u32 = 0x2c251f;
    pub const INK_SECONDARY: u32 = super::SECONDARY;
    pub const SIGNAL: u32 = super::ACCENT;
    pub const SLEEVE: u32 = super::ELEVATED;
    pub const SLEEVE_LABEL: u32 = super::PRIMARY;
    pub const DISPLAY_FACE: &str = super::FONT_FAMILY;
    pub const RADIUS: f32 = 6.;

    pub(in crate::ui) mod metrics {
        const REM: f32 = 14.;
        pub const PAD: f32 = 8.;
        pub const GAP_SMALL: f32 = REM / 4.;
        pub const GAP: f32 = REM / 2.;
        pub const GAP_WIDE: f32 = REM;
        pub const COVER_GAP: f32 = REM * 0.75;
        pub const CONTROL: f32 = 32.;
        pub const CONTROL_SMALL: f32 = 26.;
        pub const GLYPH: f32 = 16.;
        pub const ROW: f32 = 42.;
        pub const HEADER: f32 = 32.;
        pub const THUMB: f32 = 34.;
        pub const COVER: f32 = 140.;
        pub const LIST_ROW: f32 = 52.;
        pub const PLAYER: f32 = 76.;
        pub const PLAYER_COMPACT: f32 = ROW + 2. * PAD;
        pub const PLAYER_INSET: f32 = REM * 1.25;
        pub const SEEK_MAX: f32 = 560.;
        pub const VOLUME: f32 = 110.;
        pub const VOLUME_COMPACT: f32 = 72.;
        pub const NAV_WIDTH: f32 = 195.;
        pub const NAV_COMPACT: f32 = LIST_ROW + REM + 1.;
        pub const NAV_INSET: f32 = 12.;
        pub const NAV_LABEL_GAP: f32 = REM * 0.625;
        pub const PANEL_WIDTH: f32 = 254.;
    }
}

pub(in crate::ui) fn primary() -> Hsla {
    rgb(PRIMARY).into()
}

pub(in crate::ui) fn secondary() -> Hsla {
    rgb(SECONDARY).into()
}

pub(in crate::ui) fn focus_ring() -> Hsla {
    rgb(ACCENT).into()
}

pub(in crate::ui) fn surface_border() -> Hsla {
    rgb(DIVIDER).into()
}

pub(in crate::ui) fn navigation_accent() -> Hsla {
    rgb(ACCENT).into()
}

// Artwork
pub(in crate::ui) const ARTWORK_TINT: u32 = 0xa78364;
pub(in crate::ui) const COVER_LARGE_TINT: u32 = 0x4b3a2f;

// Scrollbar
pub(in crate::ui) const SCROLLBAR_TROUGH: u32 = ELEVATED;
pub(in crate::ui) const SCROLLBAR_THUMB: u32 = 0x7d6c5e;
pub(in crate::ui) const SCROLLBAR_THUMB_HOVER: u32 = 0xab9886;

// Diagnostics
pub(in crate::ui) const DIAGNOSTICS_BACKGROUND: u32 = 0x15131e;
pub(in crate::ui) const DIAGNOSTICS_CARD: u32 = 0x1e1b2a;
pub(in crate::ui) const DIAGNOSTICS_CARD_BORDER: u32 = 0x332c42;
pub(in crate::ui) const DIAGNOSTICS_GRID: u32 = 0x3c344d;
pub(in crate::ui) const DIAGNOSTICS_FRAME: u32 = 0xbb9bff;
pub(in crate::ui) const DIAGNOSTICS_INTERVAL: u32 = 0x6bd0cb;
pub(in crate::ui) const DIAGNOSTICS_API: u32 = 0x6bd0cb;
pub(in crate::ui) const DIAGNOSTICS_ARTWORK: u32 = 0xe9a975;
pub(in crate::ui) const DIAGNOSTICS_TARGET: u32 = 0xb8aacd;

// Home
pub(in crate::ui) const NOTICE_BACKGROUND: u32 = SURFACE;
pub(in crate::ui) const YANDEX_WAVE_BORDER: u32 = 0x5b4540;
pub(in crate::ui) const YANDEX_WAVE_BORDER_HOVER: u32 = 0xb98461;
pub(in crate::ui) const YANDEX_WAVE_START: u32 = 0x472e2a;
pub(in crate::ui) const YANDEX_WAVE_END: u32 = SURFACE;
pub(in crate::ui) const HOME_ITEM_HOVER: u32 = ELEVATED;
pub(in crate::ui) const HOME_ROW_BACKGROUND: u32 = SURFACE;
pub(in crate::ui) const HOME_COVER_PALETTE: [u32; 6] =
    [0x4b3528, 0x2e3c37, 0x47302e, 0x3b3540, 0x4c3d2a, 0x3c3833];
pub(in crate::ui) const HOME_COVER_SHADE: u32 = SURFACE;
pub(in crate::ui) const SHELF_ARROW_DISABLED: u32 = SURFACE;
pub(in crate::ui) const SHELF_ARROW: u32 = ELEVATED;
pub(in crate::ui) const SHELF_ARROW_HOVER: u32 = ACTIVE;
pub(in crate::ui) const SHELF_ARROW_ACTIVE: u32 = 0x554539;
pub(in crate::ui) const SHELF_TAB: u32 = SURFACE;
pub(in crate::ui) const SHELF_TAB_BORDER: u32 = DIVIDER;
pub(in crate::ui) const SHELF_TAB_HOVER: u32 = ELEVATED;
pub(in crate::ui) const SHELF_TAB_ACTIVE: u32 = 0x554539;
pub(in crate::ui) const SHELF_TAB_SELECTED: u32 = ACTIVE;
pub(in crate::ui) const SHELF_TAB_SELECTED_BORDER: u32 = 0x766153;
pub(in crate::ui) const SHELF_TAB_ARTWORK: u32 = 0x4b3a2f;
pub(in crate::ui) const SKELETON_BLOCK: u32 = ELEVATED;
pub(in crate::ui) const SKELETON_ROW: u32 = SURFACE;
pub(in crate::ui) const SKELETON_CARD: u32 = SURFACE;

// Login and settings
pub(in crate::ui) const SETTINGS_ERROR_BACKGROUND: u32 = 0x402638;
pub(in crate::ui) const SETTINGS_ROW_DIVIDER: u32 = DIVIDER;

pub(in crate::ui) fn icon_button_hover() -> Hsla {
    Hsla::from(rgb(SECONDARY)).alpha(0.12)
}

pub(in crate::ui) fn notice_surface(warning: bool) -> Hsla {
    if warning {
        hsla(0.075, 0.43, 0.64, 0.13)
    } else {
        rgb(SURFACE).into()
    }
}

pub(in crate::ui) fn notice_border(warning: bool) -> Hsla {
    if warning {
        hsla(0.075, 0.43, 0.72, 0.27)
    } else {
        surface_border()
    }
}

pub(in crate::ui) fn transparent() -> Hsla {
    hsla(0., 0., 0., 0.)
}

// Playlist menu
pub(in crate::ui) const PLAYLIST_MENU_SELECTED: u32 = ACTIVE;
pub(in crate::ui) const PLAYLIST_MENU_HOVER: u32 = ELEVATED;

// Welcome
pub(in crate::ui) const SOUNDCLOUD_TILE: u32 = 0xee611b;

// Workspace
pub(in crate::ui) const SIDEBAR_BACKGROUND: u32 = 0x171513;
pub(in crate::ui) const NAV_LINK_HOVER: u32 = ELEVATED;
pub(in crate::ui) const CATALOG_SEARCH_BACKGROUND: u32 = SURFACE;
pub(in crate::ui) const SOURCE_CHIP_BACKGROUND: u32 = SURFACE;
pub(in crate::ui) const SOURCE_ROW_HOVER: u32 = ELEVATED;
pub(in crate::ui) const SCROLL_MARKER_BACKGROUND: u32 = SURFACE;
pub(in crate::ui) const SCROLL_MARKER_BORDER: u32 = 0x8d7664;
pub(in crate::ui) const SCROLL_MARKER_DOT: u32 = PRIMARY;
pub(in crate::ui) const SCROLL_MARKER_ARROW: u32 = ACCENT;

// Player
pub(in crate::ui) const PLAYER_PANEL_CURRENT: u32 = ACTIVE;
pub(in crate::ui) const PLAYER_PANEL_HOVER: u32 = ELEVATED;
pub(in crate::ui) const SLIDER_TRACK: u32 = SECONDARY;

pub(in crate::ui) fn install(cx: &mut App, fluid: bool) {
    if let Err(error) = cx
        .text_system()
        .add_fonts(vec![Cow::Borrowed(include_bytes!(
            "../../assets/fonts/GolosText.ttf"
        ))])
    {
        eprintln!("Не удалось зарегистрировать {FONT_FAMILY}: {error}");
    }
    if fluid {
        set_fluid_contrast(cx, false);
        return;
    }
    let mut theme = guise::Theme::dark()
        .with_body(rgb(WINDOW))
        .with_surface(rgb(SURFACE))
        .with_surface_hover(rgb(ELEVATED))
        .with_primary(rgb(ACCENT))
        .with_text(rgb(PRIMARY))
        .with_dimmed(rgb(SECONDARY))
        .with_border(rgb(DIVIDER));
    theme.font_family = FONT_FAMILY.into();
    theme.init(cx);
}

struct FluidContrast(bool);

impl Global for FluidContrast {}

pub(in crate::ui) fn fluid_high_contrast(cx: &App) -> bool {
    cx.try_global::<FluidContrast>().is_some_and(|mode| mode.0)
}

pub(super) fn set_fluid_contrast(cx: &mut App, high_contrast: bool) {
    use super::{
        design_system::{
            controls::{ControlState, ControlVariant},
            materials::{Kind, Material},
        },
        fluid_glass::tokens::{self, contrast},
    };
    super::fluid_glass::artwork::configure(cx, high_contrast);
    cx.set_global(FluidContrast(high_contrast));
    let [body, surface, hover, accent, text, dimmed, border] = if high_contrast {
        [
            contrast::CANVAS,
            contrast::SURFACE,
            contrast::ELEVATED,
            contrast::ACCENT,
            contrast::PRIMARY,
            contrast::SECONDARY,
            contrast::RULE,
        ]
    } else {
        [
            tokens::CANVAS,
            tokens::SURFACE,
            tokens::HOVER,
            tokens::ACCENT,
            tokens::PRIMARY,
            tokens::SECONDARY,
            tokens::RULE,
        ]
    };
    let input = ControlVariant::SearchInput;
    let material = Material::new(Kind::Recessed, rgb(accent).into()).high_contrast(high_contrast);
    let (background, tooltip_border, foreground) =
        super::components::overlay::menu_colors(material);
    cx.set_global(guise::overlay::TooltipSurface {
        background,
        border: tooltip_border,
        foreground,
    });
    let idle = input.resolve(material, ControlState::default());
    let focused = input.resolve(
        material,
        ControlState {
            focused: true,
            ..Default::default()
        },
    );
    cx.set_global(guise::input::TextInputSurface {
        background: idle.bg,
        focused_background: focused.bg,
        border: idle.border,
        focused_border: focused.border,
        radius: idle.radius,
    });
    let mut theme = guise::Theme::dark()
        .with_body(rgb(body))
        .with_surface(rgb(surface))
        .with_surface_hover(rgb(hover))
        .with_primary(rgb(accent))
        .with_text(rgb(text))
        .with_dimmed(rgb(dimmed))
        .with_border(rgb(border));
    theme.font_family = tokens::FONT.into();
    theme.radius.xl = 24.;
    theme.init(cx);
}
