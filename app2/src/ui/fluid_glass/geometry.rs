//! One responsive contour for the native layout, optical shader and fallback.

use gpui::{Bounds, Pixels, Size, Window, point, px, size};

use super::tokens;

#[derive(Clone, Copy, Debug)]
pub(in crate::ui) struct Geometry {
    pub(super) inset: f32,
    pub(super) rail: f32,
    pub(super) player: f32,
    pub(super) outer_radius: f32,
    pub(super) inner_radius: f32,
    expansion: f32,
}

impl Geometry {
    pub(in crate::ui) fn for_window(window: &Window) -> Self {
        let viewport = window.viewport_size();
        Self::for_size(viewport.width.into(), viewport.height.into())
    }

    pub(in crate::ui) fn preview() -> Self {
        Self {
            inset: 0.,
            rail: 0.,
            player: 0.,
            outer_radius: 0.,
            inner_radius: 0.,
            expansion: 0.,
        }
    }

    pub(super) fn without_navigation(mut self, player: bool) -> Self {
        self.rail = 0.;
        if !player {
            self.player = 0.;
        }
        self
    }

    pub(super) fn for_size(width: f32, height: f32) -> Self {
        let expansion = growth(width, tokens::RAIL_EXPAND_START, tokens::RAIL_EXPAND_END);
        let depth = growth(height, tokens::SHELL_EXPAND_START, tokens::SHELL_EXPAND_END);
        Self {
            inset: mix(tokens::INSET_COMPACT, tokens::INSET, depth),
            rail: mix(tokens::RAIL_COMPACT, tokens::RAIL, expansion),
            player: mix(tokens::PLAYER_COMPACT, tokens::PLAYER, depth),
            outer_radius: mix(tokens::OUTER_RADIUS_COMPACT, tokens::OUTER_RADIUS, depth),
            inner_radius: mix(tokens::INNER_RADIUS_COMPACT, tokens::INNER_RADIUS, depth),
            expansion,
        }
    }

    pub(in crate::ui) fn inset(self) -> f32 {
        self.inset
    }

    pub(in crate::ui) fn rail(self) -> f32 {
        self.rail
    }

    pub(in crate::ui) fn player(self) -> f32 {
        self.player
    }

    pub(in crate::ui) fn player_cover(self) -> f32 {
        let height_progress =
            (self.player - tokens::PLAYER_COMPACT) / (tokens::PLAYER - tokens::PLAYER_COMPACT);
        let full_height =
            self.responsive_width(tokens::PLAYER_COVER_NARROW, tokens::PLAYER_COVER_WIDE);
        mix(tokens::PLAYER_COVER_COMPACT, full_height, height_progress).round()
    }

    pub(in crate::ui) fn compact(self) -> bool {
        self.rail < tokens::RAIL_LABEL_WIDTH
    }

    pub(in crate::ui) fn content_bottom(self) -> f32 {
        mix(
            tokens::CONTENT_BOTTOM_COMPACT,
            tokens::CONTENT_BOTTOM,
            self.expansion,
        )
    }

    pub(in crate::ui) fn margin(self) -> f32 {
        mix(
            tokens::CONTENT_PADDING_COMPACT,
            tokens::CONTENT_PADDING,
            self.expansion,
        )
    }

    /// Lateral dissolve beside the concave bends. Content margins are wider
    /// than this band, so text never enters it.
    pub(in crate::ui) fn edge_fade_side(self) -> f32 {
        mix(16., 24., self.expansion)
    }

    pub(in crate::ui) fn content_width(self, width: f32) -> f32 {
        width - 2. * self.inset - self.rail - 2. * self.margin()
    }

    pub(in crate::ui) fn search_max(self) -> f32 {
        mix(
            tokens::SEARCH_MAX_COMPACT,
            tokens::SEARCH_MAX,
            self.expansion,
        )
    }

    pub(in crate::ui) fn responsive_width(self, narrow: f32, wide: f32) -> f32 {
        mix(narrow, wide, self.expansion)
    }
}

/// Detailed bounds shared by layout, scrolling, scrollbars and tests.
///
/// `Geometry` remains the compact set of scalars sent to the shader. This type
/// adds the rectangles that used to be recomputed in several modules. The
/// exact shell mask and the decorative material feather stay separate: the
/// latter is only a width derived from the same contour.
#[derive(Clone, Copy, Debug)]
pub(in crate::ui) struct ShellGeometry {
    base: Geometry,
    viewport: Size<f32>,
    content: Bounds<Pixels>,
    safe_content: Bounds<Pixels>,
}

/// The extra room a scrollbar needs at each edge. The bottom inset clears the
/// concave elbow between the page and the player instead of ending inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::ui) struct ScrollInsets {
    pub top: Pixels,
    pub bottom: Pixels,
    pub right: Pixels,
}

impl ShellGeometry {
    /// Geometry for the workspace. `docked_width` is the width the open right
    /// panel takes from the page, or zero when the panel overlays the page.
    pub(in crate::ui) fn for_workspace(window: &Window, docked_width: f32) -> Self {
        let viewport = window.viewport_size();
        Self::for_size(
            size(f32::from(viewport.width), f32::from(viewport.height)),
            docked_width,
        )
    }

    fn for_size(viewport: Size<f32>, docked_width: f32) -> Self {
        let width = viewport.width;
        let height = viewport.height;
        let base = Geometry::for_size(width, height);
        let left = base.inset + base.rail;
        let top = base.inset + tokens::TITLEBAR;
        let right = (width - base.inset - docked_width.max(0.)).max(left);
        let bottom = (height - base.inset - base.player).max(top);
        let content = Bounds::new(
            point(px(left), px(top)),
            size(px(right - left), px(bottom - top)),
        );
        let safe_bottom = (base.inner_radius * 0.38).clamp(12., 28.);
        let safe = Bounds::new(
            point(content.left() + px(8.), content.top() + px(8.)),
            size(
                (content.size.width - px(16.)).max(px(0.)),
                (content.size.height - px(8.) - px(safe_bottom)).max(px(0.)),
            ),
        );
        Self {
            base,
            viewport,
            content,
            safe_content: safe,
        }
    }

    /// Geometry without a docked panel, used by scrollbars and standalone
    /// screens that still need the same safe insets.
    pub(in crate::ui) fn for_window(window: &Window) -> Self {
        Self::for_workspace(window, 0.)
    }

    pub(in crate::ui) fn base(self) -> Geometry {
        self.base
    }

    pub(in crate::ui) fn content_bounds(self) -> Bounds<Pixels> {
        self.content
    }

    pub(in crate::ui) fn safe_content_bounds(self) -> Bounds<Pixels> {
        self.safe_content
    }

    pub(in crate::ui) fn viewport(self) -> Size<f32> {
        self.viewport
    }

    /// The decorative band where glass density bleeds into the page. It is
    /// wider in a broad window and narrows with the shell itself.
    pub(in crate::ui) fn material_feather(self) -> f32 {
        (self.base.inner_radius * 1.6).clamp(40., 100.)
    }

    /// Safe distance from the bottom contour for an overlay scrollbar.
    pub(in crate::ui) fn scrollbar_insets(self) -> ScrollInsets {
        ScrollInsets {
            top: px(12.),
            bottom: px((self.material_feather() * 0.45 + 10.).clamp(20., 48.)),
            right: px(8.),
        }
    }

    /// Bottom padding for scroll content so the final row can be scrolled
    /// clear of the recursive elbow rather than stopping under it.
    pub(in crate::ui) fn scroll_content_padding(self) -> f32 {
        (self.base.inner_radius * 0.35 + 12.).clamp(16., 36.)
    }
}

fn growth(value: f32, start: f32, end: f32) -> f32 {
    crate::ui::design_system::motion::smoothstep((value - start) / (end - start))
}

fn mix(start: f32, end: f32, progress: f32) -> f32 {
    start + (end - start) * progress
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn broadening_the_window_also_broadens_the_music_area() {
        let narrow = Geometry::for_size(800., 400.);
        let broad = Geometry::for_size(1920., 1080.);
        assert_eq!((narrow.inset, narrow.rail, narrow.player), (8., 72., 62.));
        assert_eq!((broad.inset, broad.rail, broad.player), (20., 248., 100.));
        assert_eq!(narrow.player_cover(), 37.);
        assert_eq!(Geometry::for_size(800., 1080.).player_cover(), 54.);
        assert_eq!(broad.player_cover(), 68.);
        assert!(narrow.compact() && !broad.compact());
        for width in 801..=1920 {
            let width = width as f32;
            let before = Geometry::for_size(width - 1., 720.);
            let after = Geometry::for_size(width, 720.);
            assert!(before.rail <= after.rail);
            assert!(before.content_width(width - 1.) < after.content_width(width));
        }
    }

    #[test]
    fn shell_bounds_leave_the_player_and_rail_out_of_the_page() {
        let broad = ShellGeometry::for_size(size(1920., 1080.), 0.);
        assert_eq!(
            (
                f32::from(broad.content_bounds().left()),
                f32::from(broad.content_bounds().top()),
                f32::from(broad.content_bounds().right()),
                f32::from(broad.content_bounds().bottom()),
            ),
            (268., 68., 1900., 960.)
        );
        let docked = ShellGeometry::for_size(size(1920., 1080.), 320.);
        assert_eq!(f32::from(docked.content_bounds().right()), 1580.);
        assert!(docked.safe_content_bounds().right() <= docked.content_bounds().right());
        assert_eq!(docked.material_feather(), 89.6);
        assert!(docked.scrollbar_insets().bottom > px(20.));
        assert!(docked.scroll_content_padding() > 20.);
    }

    #[test]
    fn compact_shell_keeps_the_same_safe_contour() {
        let compact = ShellGeometry::for_size(size(800., 400.), 0.);
        assert_eq!(compact.viewport(), size(800., 400.));
        assert_eq!(
            f32::from(compact.safe_content_bounds().left()),
            f32::from(compact.content_bounds().left()) + 8.
        );
        assert!((40. ..=100.).contains(&compact.material_feather()));
        assert!(compact.scrollbar_insets().bottom >= px(20.));
    }

    #[test]
    fn adding_height_also_grows_the_area_above_the_player() {
        let available = |height: f32| {
            let geometry = Geometry::for_size(1360., height);
            height - 2. * geometry.inset - geometry.player - geometry.content_bottom()
        };
        for height in 401..=1080 {
            let height = height as f32;
            assert!(available(height - 1.) < available(height));
        }
    }
}
