mod layout;

use std::time::Instant;

use gpui::{
    Context, Div, FontWeight, Role, Stateful, Window, WindowControlArea, accesskit::Toggled, div,
    prelude::*, px, rgb,
};
use guise::Icon;

use crate::{
    state::HomeView,
    ui::{
        MainView,
        assets::mark,
        components::{Activate, button, fluid_selection_marker},
        design_system::controls::{ControlState, ControlVariant},
        fluid_glass::tokens::{self as fluid, radius, space, text},
        theme::{
            ACTIVE, ELEVATED, SIDEBAR_BACKGROUND, navigation_accent, playlist, primary, secondary,
            surface_border,
        },
    },
};

use super::{Page, Workspace, WorkspaceLayout};
pub(super) use layout::SidebarLayout;
use layout::{Destination, PRIMARY, SETTINGS};
use playlist::metrics::{
    CONTROL, CONTROL_SMALL, COVER_GAP, GAP, GLYPH, NAV_COMPACT, NAV_INSET, NAV_LABEL_GAP, NAV_WIDTH,
};

const FULL_NAV_TOP_PADDING: f32 = 14.;
const FULL_NAV_LINK_MARGIN: f32 = 20.;

pub(super) struct SidebarMotion {
    pub(super) from: f32,
    to: f32,
    started: Instant,
    layout: SidebarLayout,
    #[cfg(feature = "capture")]
    sample: Option<f32>,
}

impl SidebarMotion {
    fn progress(&self, now: Instant) -> f32 {
        #[cfg(feature = "capture")]
        if let Some(sample) = self.sample {
            return sample;
        }
        (now.duration_since(self.started).as_secs_f32() / self.layout.duration().as_secs_f32())
            .min(1.)
    }

    fn position(&self, fraction: f32) -> f32 {
        self.from + (self.to - self.from) * (1. - (1. - fraction).powi(3))
    }
}

impl Workspace {
    pub(super) fn select_page(&mut self, page: Page, immediate: bool, high_contrast: bool) {
        self.transitions.select(self.nav_at, page, immediate);
        if immediate || high_contrast {
            self.nav_motion = None;
        } else if self.nav_at != page {
            let now = Instant::now();
            let layout = self.nav_layout.get();
            self.nav_motion = layout.selection_offset(page).and_then(|to| {
                let from = self
                    .nav_visible
                    .get()
                    .or_else(|| layout.selection_offset(self.nav_at))?;
                (from != to).then_some(SidebarMotion {
                    from,
                    to,
                    started: now,
                    layout,
                    #[cfg(feature = "capture")]
                    sample: None,
                })
            });
        }
        self.nav_at = page;
    }
}

impl MainView {
    #[cfg(feature = "capture")]
    pub fn sample_sidebar_motion_for_capture(
        &mut self,
        progress: Option<f32>,
        cx: &mut Context<Self>,
    ) {
        if let Some(motion) = self.workspace.nav_motion.as_mut() {
            motion.sample = progress.map(|progress| progress.clamp(0., 1.));
            cx.notify();
        }
    }

    #[cfg(feature = "capture")]
    pub fn sidebar_motion_targets_for_capture(
        &self,
        window: &Window,
    ) -> [(gpui::Point<gpui::Pixels>, f32); 2] {
        let layout = self.workspace.nav_layout.get();
        let glass = layout != SidebarLayout::Classic;
        let workspace = self.workspace_layout(window);
        let inset = if glass {
            crate::ui::fluid_glass::Geometry::for_window(window).inset()
        } else {
            0.
        };
        let mut top = inset + layout.titlebar_height();
        if layout == SidebarLayout::FluidFull {
            top += FULL_NAV_TOP_PADDING + FULL_NAV_LINK_MARGIN;
        }
        [Page::Tracks, Page::Playlists].map(|page| {
            let offset = layout.selection_offset(page).expect("library link");
            let center = gpui::point(
                px(inset + workspace.sidebar_width / 2.),
                px(top + offset + layout.row() / 2.),
            );
            (center, offset)
        })
    }

    pub(super) fn music_navigation(
        &self,
        home: Option<&HomeView>,
        layout: WorkspaceLayout,
        window: &Window,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let compact = layout.compact;
        let glass = self.uses_fluid_glass();
        let sidebar = if !glass {
            SidebarLayout::Classic
        } else if self.has_full_fluid_navigation(layout, window) {
            SidebarLayout::FluidFull
        } else if self.has_pins()
            && window.viewport_size().height < px(fluid::NAV_DENSE_PINNED_MAX_HEIGHT)
        {
            SidebarLayout::FluidDense
        } else {
            SidebarLayout::FluidShort
        };
        let full = sidebar == SidebarLayout::FluidFull;
        let dense = sidebar == SidebarLayout::FluidDense;
        self.workspace.nav_layout.set(sidebar);
        let outer_inset = if glass && !compact {
            space::LG
        } else if compact {
            GAP
        } else {
            COVER_GAP
        };
        let brand_name = if glass {
            !compact
        } else {
            layout.sidebar_width >= NAV_WIDTH - 2. * GAP
        };
        // Keep compact icons near the existing control column as the glass
        // widens. Their neighbouring space can then fit the full labels.
        let trailing_inset = if glass && compact {
            outer_inset + layout.sidebar_width - fluid::RAIL_COMPACT
        } else {
            outer_inset
        };
        let brand = div()
            .h(px(sidebar.titlebar_height()))
            .flex_none()
            .flex()
            .items_center()
            .when(!brand_name, |brand| brand.justify_center())
            .when(brand_name, |brand| brand.px(px(GAP)).gap(px(NAV_LABEL_GAP)))
            .window_control_area(WindowControlArea::Drag)
            .child(mark("mediadeka.svg", px(CONTROL)))
            .when(brand_name, |brand| {
                brand.child(
                    div()
                        .text_size(px(20.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Mediadeka"),
                )
            });
        let pinned = self.pinned_group(compact, !dense, cx);
        div()
            .id("home-navigation")
            .accessibility_id("home-navigation")
            .role(Role::Navigation)
            .aria_label("Навигация")
            .w(px(layout.sidebar_width))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .pl(px(outer_inset))
            .pr(px(trailing_inset))
            .when(!glass, |nav| {
                nav.border_r_1()
                    .border_color(surface_border())
                    .bg(rgb(SIDEBAR_BACKGROUND))
            })
            .when(full, |nav| nav.pt(px(FULL_NAV_TOP_PADDING)))
            .child(brand)
            .child(
                div()
                    .relative()
                    .flex_none()
                    .flex()
                    .flex_col()
                    .gap(px(sidebar.gap()))
                    .when(full, |links| links.mt(px(FULL_NAV_LINK_MARGIN)))
                    .children(self.sidebar_highlight(sidebar, window, cx))
                    .children(PRIMARY.into_iter().enumerate().map(|(index, link)| {
                        self.music_nav_link(link, 12 + index as isize, compact, sidebar, cx)
                    }))
                    .when(full, |links| {
                        links.child(section_label("Медиатека")).child(
                            div()
                                .id("studio-library-links")
                                .role(Role::Group)
                                .aria_label("Медиатека")
                                .tab_index(15)
                                .tab_group()
                                .tab_stop(false)
                                .flex()
                                .flex_col()
                                .gap(px(sidebar.gap()))
                                .children(sidebar.library().iter().copied().enumerate().map(
                                    |(index, link)| {
                                        self.music_nav_link(
                                            link,
                                            index as isize,
                                            compact,
                                            sidebar,
                                            cx,
                                        )
                                    },
                                )),
                        )
                    })
                    .when(!full, |links| {
                        links.child(library_separator(compact, sidebar)).children(
                            sidebar
                                .library()
                                .iter()
                                .copied()
                                .enumerate()
                                .map(|(index, link)| {
                                    self.music_nav_link(
                                        link,
                                        15 + index as isize,
                                        compact,
                                        sidebar,
                                        cx,
                                    )
                                }),
                        )
                    }),
            )
            .when_some(pinned, |nav, pinned| nav.child(pinned))
            .when(!self.has_pins(), |nav| nav.child(div().flex_1().min_h_0()))
            .when(full && home.is_some(), |nav| {
                nav.child(section_label("Подключения"))
            })
            .child(self.accounts_card(home, compact, cx))
            .child(div().h(px(GAP)).flex_none())
            .child(self.music_nav_link(SETTINGS, 19, compact, sidebar, cx))
            .pb(px(if dense { GAP } else { outer_inset }))
    }

    fn sidebar_highlight(
        &self,
        layout: SidebarLayout,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<Div> {
        if self.pinned_current().is_some() {
            self.workspace.nav_visible.set(None);
            return None;
        }
        let Some(end) = layout.selection_offset(self.music_page()) else {
            self.workspace.nav_visible.set(None);
            return None;
        };
        let now = Instant::now();
        let high_contrast = self.fluid_high_contrast(cx);
        let position = self
            .workspace
            .nav_motion
            .as_ref()
            .filter(|motion| {
                !self.reduced_motion
                    && !high_contrast
                    && !crate::ui::native::minimized(window)
                    && motion.layout == layout
                    && motion.to == end
            })
            .and_then(|motion| {
                let progress = motion.progress(now);
                if progress >= 1. {
                    return None;
                }
                window.request_animation_frame();
                Some(motion.position(progress))
            })
            .unwrap_or(end);
        self.workspace.nav_visible.set(Some(position));
        Some(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px(position))
                .h(px(layout.row()))
                .rounded(px(if layout == SidebarLayout::Classic {
                    playlist::RADIUS
                } else {
                    radius::CONTROL
                }))
                .when(layout == SidebarLayout::Classic, |highlight| {
                    highlight.bg(rgb(ELEVATED))
                })
                .when(layout != SidebarLayout::Classic, |highlight| {
                    highlight
                        .bg(fluid_highlight(high_contrast))
                        .when(high_contrast, |highlight| {
                            highlight
                                .border_1()
                                .border_color(rgb(fluid::contrast::ACCENT))
                        })
                        .child(fluid_selection_marker(high_contrast))
                }),
        )
    }

    fn music_nav_link(
        &self,
        destination: Destination,
        order: isize,
        compact: bool,
        layout: SidebarLayout,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let Destination {
            page,
            id,
            label,
            glyph,
        } = destination;
        let current = layout.current(self.music_page());
        let glass = layout != SidebarLayout::Classic;
        let high_contrast = self.fluid_high_contrast(cx);
        let pinned_current = self.pinned_current().is_some();
        let selected = current == page && !(pinned_current && page == Page::Playlists);
        let link = button(id, label, order)
            .aria_toggled(if selected {
                Toggled::True
            } else {
                Toggled::False
            })
            .when(selected, |link| link.aria_description("Текущий раздел"))
            .when(layout == SidebarLayout::FluidFull, |link| {
                link.when_some(page.library_tab(), |link, tab| {
                    link.track_focus(self.library_tab_focus(tab))
                })
            })
            .h(px(layout.row()))
            .flex_none()
            .gap(px(NAV_LABEL_GAP))
            .rounded(px(if glass {
                radius::CONTROL
            } else {
                playlist::RADIUS
            }))
            .when(compact, |link| {
                link.justify_center().tooltip(guise::tooltip(label))
            })
            .when(!compact, |link| link.px(px(NAV_INSET)))
            .when(!glass && selected && page == Page::Settings, |link| {
                link.bg(rgb(ACTIVE))
            })
            .when(glass && selected && page == Page::Settings, |link| {
                link.relative()
                    .bg(fluid_highlight(high_contrast))
                    .child(fluid_selection_marker(high_contrast))
            })
            .text_color(match (glass, selected) {
                (true, true) if high_contrast => rgb(fluid::contrast::PRIMARY).into(),
                (true, false) if high_contrast => rgb(fluid::contrast::SECONDARY).into(),
                (true, true) => rgb(fluid::PRIMARY).into(),
                (true, false) => rgb(fluid::SECONDARY).into(),
                (false, true) => primary(),
                (false, false) => secondary(),
            })
            .text_size(px(if glass { text::NAVIGATION } else { 14. }))
            .font_weight(if selected {
                FontWeight::SEMIBOLD
            } else if glass {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            });
        let link = link
            .child(
                Icon::new(glyph)
                    .pixel_size(px(if glass { 18. } else { GLYPH }))
                    .color(match (glass, selected) {
                        (true, true) if high_contrast => rgb(fluid::contrast::ACCENT).into(),
                        (true, false) if high_contrast => rgb(fluid::contrast::SECONDARY).into(),
                        (true, true) => rgb(fluid::ACCENT).into(),
                        (true, false) => rgb(fluid::SECONDARY).into(),
                        (false, true) => navigation_accent(),
                        (false, false) => secondary(),
                    }),
            )
            .when(!compact, |link| link.child(div().truncate().child(label)));
        let link = if let Some(material) = self.glass_controls(cx) {
            let style = |hovered, pressed, focused| {
                ControlVariant::TextAction.resolve(
                    material,
                    ControlState {
                        hovered,
                        pressed,
                        focused,
                        ..ControlState::default()
                    },
                )
            };
            let hover = style(true, false, false);
            let press = style(false, true, false);
            let focus = style(false, false, true);
            link.hover(move |style| style.bg(hover.bg))
                .active(move |style| style.bg(press.bg))
                .focus_visible(move |style| style.border_color(focus.border))
        } else {
            link.hover(|style| style.bg(rgb(ELEVATED)))
        };
        link.on_activate(
            cx.listener(move |this, _, window, cx| this.show_music_page(page, window, cx)),
        )
    }
}

fn fluid_highlight(high_contrast: bool) -> gpui::Hsla {
    if high_contrast {
        rgb(fluid::contrast::SELECTED).into()
    } else {
        gpui::Hsla::from(rgb(fluid::ACTIVE)).alpha(0.72)
    }
}

fn section_label(label: &'static str) -> Div {
    div()
        .h(px(CONTROL_SMALL))
        .flex_none()
        .flex()
        .items_center()
        .px(px(NAV_INSET))
        .text_size(px(text::CAPTION))
        .text_color(rgb(fluid::SECONDARY))
        .child(label)
}

fn library_separator(compact: bool, layout: SidebarLayout) -> Div {
    div()
        .h(px(layout.separator_height()))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .px(px(NAV_INSET))
        .when(!compact || layout == SidebarLayout::Classic, |separator| {
            separator.child(
                div()
                    .h(px(1.))
                    .bg(if layout == SidebarLayout::Classic {
                        surface_border()
                    } else {
                        rgb(fluid::RULE).into()
                    })
                    .when(compact, |line| line.w(px(24.)))
                    .when(!compact, |line| line.w_full()),
            )
        })
}

pub(super) fn sidebar_width(progress: f32) -> f32 {
    NAV_COMPACT + (NAV_WIDTH - NAV_COMPACT) * progress
}
