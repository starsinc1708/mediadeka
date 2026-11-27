use std::time::Duration;

use gpui::{
    App, Bounds, Context, Div, Entity, IntoElement, MouseButton, MouseDownEvent, Stateful,
    StyleRefinement, Subscription, WeakEntity, Window, div, prelude::*, px, size,
    transparent_black,
};
use guise::{ActionIcon, IconName};

use super::{ListeningRequested, PlayerBar, TrackMenuRequested, layout::Layout};
use crate::ui::{
    components::{Activate, button, music::compact_control},
    design_system::{
        controls::{self, ControlState, ControlVariant},
        icons::{self, Glyph},
        motion::MOTION,
    },
    fluid_glass::tokens,
    theme::playlist::metrics,
};

// Match the large play button in the transport row.
const HEIGHT: f32 = 34.;
pub(super) const LISTENING_COVER_ORDER: isize = 599;
const LISTENING_TOOL_ORDER: isize = 612;
const LISTENING_TOOLTIP: &str = "Полноэкранное прослушивание · Ctrl+L";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::ui) enum ControlKind {
    Tool,
    Selected,
    Primary,
    Transport,
}

impl ControlKind {
    pub(super) fn toggle(selected: bool) -> Self {
        if selected { Self::Selected } else { Self::Tool }
    }

    fn variant(self) -> ControlVariant {
        match self {
            Self::Primary => ControlVariant::PrimaryCircle,
            Self::Transport => ControlVariant::TransportIcon,
            Self::Tool | Self::Selected => ControlVariant::GhostIcon,
        }
    }

    pub fn dimensions(self, compact: bool) -> (f32, f32) {
        match (self, compact) {
            (Self::Primary, false) => (48., 24.),
            (Self::Primary, true) => (34., 18.),
            (Self::Transport, false) => (40., 22.),
            (Self::Transport, true) => (30., 18.),
            (_, false) => (36., 20.),
            (_, true) => (26., 16.),
        }
    }

    pub fn editorial_style(
        self,
        control: ActionIcon,
        compact: bool,
        motion: bool,
        cx: &App,
    ) -> ActionIcon {
        let (size, glyph) = self.dimensions(compact);
        let selected = self == Self::Selected;
        let control = control
            .pixel_size(px(size))
            .glyph_size(px(glyph))
            .radius(if matches!(self, Self::Tool | Self::Selected) {
                guise::Size::Md
            } else {
                guise::Size::Xl
            })
            .when(selected, |control| control.toggled(true));
        let material = controls::material(cx);
        let state = ControlState {
            active: selected,
            ..ControlState::default()
        };
        let surface = self.variant().surface(material, state);
        let halo = !matches!(self, Self::Primary);
        // An active transport command keeps its accent glyph and dot, not a
        // filled square. The hover/press wash lives in the glyph halo.
        let surface = if selected {
            guise::Surface {
                bg: transparent_black(),
                ..surface
            }
        } else {
            surface
        };
        let hovered = self.variant().resolve(
            material,
            ControlState {
                hovered: true,
                ..state
            },
        );
        let pressed = self
            .variant()
            .resolve(
                material,
                ControlState {
                    pressed: true,
                    ..state
                },
            )
            .bg;
        let timing = |duration| {
            if motion && !material.contrast() {
                duration
            } else {
                Duration::ZERO
            }
        };
        control
            .surface_style(surface)
            .pointer_feedback(
                timing(MOTION.fast()),
                timing(MOTION.instant()),
                hovered.fg,
                pressed,
            )
            // The transport is softer than the shared icon scale: the glyph
            // compresses a little instead of visibly shrinking the command.
            .pointer_scale(1.02, 0.96)
            .glyph_halo(halo)
    }
}

impl PlayerBar {
    pub(super) fn place_listening_control(&mut self, window: &Window) -> bool {
        let in_tools = self.fluid_chrome()
            && window.viewport_size().width >= px(tokens::LISTENING_TOOL_MIN_WIDTH);
        let order = if in_tools {
            LISTENING_TOOL_ORDER
        } else {
            LISTENING_COVER_ORDER
        };
        if self.listening_focus.tab_index != order {
            self.listening_focus = self.listening_focus.clone().tab_index(order);
        }
        in_tools
    }

    pub(super) fn listening_cover(
        &self,
        image: Div,
        edge: f32,
        in_tools: bool,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let cover = if in_tools {
            // The dedicated tool is the focusable command at this width.
            div()
                .id("player-cover-shortcut")
                .flex()
                .items_center()
                .cursor_pointer()
                .on_click(cx.listener(|_, _, _, cx| cx.emit(ListeningRequested)))
        } else {
            button(
                "player-listening",
                if self.listening_presentation {
                    "Выйти из полноэкранного прослушивания"
                } else {
                    "Сейчас играет · открыть на весь экран"
                },
                LISTENING_COVER_ORDER,
            )
            .track_focus(&self.listening_focus)
            .on_activate(cx.listener(|_, _, _, cx| cx.emit(ListeningRequested)))
        };
        cover
            .size(px(edge + 2.))
            .flex_none()
            .rounded(px(7.))
            .justify_center()
            .tooltip(guise::tooltip(LISTENING_TOOLTIP))
            .when(
                !self.listening_presentation && self.current().is_some(),
                |cover| {
                    cover.on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|_, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            cx.emit(TrackMenuRequested(Bounds::new(
                                event.position,
                                size(px(1.), px(1.)),
                            )));
                        }),
                    )
                },
            )
            .child(image)
    }

    pub(super) fn listening_tool(&self, cx: &Context<Self>) -> ActionIcon {
        compact_control(
            "player-listening",
            "Полноэкранное прослушивание",
            IconName::Maximize2,
            LISTENING_TOOL_ORDER,
            true,
            false,
        )
        .track_focus(&self.listening_focus)
        .on_click(cx.listener(|_, _, _, cx| cx.emit(ListeningRequested)))
    }

    pub(super) fn control_glyph(
        &self,
        button: ActionIcon,
        id: &'static str,
        glyph: Glyph,
        target: bool,
        side: f32,
        cx: &App,
    ) -> ActionIcon {
        button.when(self.fluid_chrome(), |button| {
            button.glyph_content(icons::animated_glyph(id, glyph, target, side, None, cx))
        })
    }

    pub(super) fn control_feedback(
        &self,
        control: ActionIcon,
        kind: ControlKind,
        compact: bool,
        cx: &App,
    ) -> ActionIcon {
        if !self.fluid_chrome() {
            return control;
        }
        kind.editorial_style(control, compact, self.fluid_motion, cx)
    }
}

pub(super) struct Controls {
    player: WeakEntity<PlayerBar>,
    _transport: Subscription,
}

impl Controls {
    pub(super) fn new(player: &Entity<PlayerBar>, cx: &mut Context<Self>) -> Self {
        Self {
            player: player.downgrade(),
            _transport: cx.observe(player, |_, _, cx| cx.notify()),
        }
    }

    pub(super) fn style(playlist: bool, fluid_compact: Option<bool>) -> StyleRefinement {
        StyleRefinement::default()
            .w_full()
            .h(px(if let Some(compact) = fluid_compact {
                ControlKind::Primary.dimensions(compact).0
            } else if playlist {
                metrics::CONTROL_SMALL
            } else {
                HEIGHT
            }))
            .flex_none()
    }
}

impl Render for Controls {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.player
            .update(cx, |player, cx| {
                let compact = if player.fluid_chrome() {
                    Layout::for_window(window).compact
                } else {
                    window.viewport_size().height < px(600.)
                };
                player.transport_controls(compact, cx)
            })
            .unwrap_or_else(|_| div())
    }
}
