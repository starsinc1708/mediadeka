//! Shared playback, selection and interaction states for editorial track rows.

use gpui::{App, Div, ElementId, Hsla, SharedString, Stateful, div, prelude::*, px, rgb};
use guise::{Icon, IconName};

use crate::{
    playback::Status,
    ui::{
        design_system::materials,
        fluid_glass::tokens::{self, contrast, selection},
    },
};

use super::{button, fluid_current_track_fill, selection_marker};

pub(in crate::ui) const RADIUS: f32 = 7.;
pub(in crate::ui) const HOVER_GROUP: &str = "editorial-track-row";

pub(in crate::ui) fn current_description(status: &Status) -> &'static str {
    match status {
        Status::Playing => "Текущий трек · воспроизводится",
        Status::Paused => "Текущий трек · пауза",
        Status::Loading => "Текущий трек · загружается",
        Status::Failed(_) => "Текущий трек · ошибка воспроизведения",
        _ => "Текущий трек",
    }
}

#[derive(Clone, Copy)]
pub(in crate::ui) struct Style {
    accent: Hsla,
    high_contrast: bool,
}

impl Style {
    pub(in crate::ui) fn new(cx: &App, high_contrast: bool) -> Self {
        Self {
            accent: if high_contrast {
                rgb(contrast::ACCENT).into()
            } else {
                materials::accent(cx)
            },
            high_contrast,
        }
    }

    pub(in crate::ui) fn title(self, current: bool) -> Hsla {
        if current {
            self.accent
        } else {
            rgb(if self.high_contrast {
                contrast::PRIMARY
            } else {
                tokens::PRIMARY
            })
            .into()
        }
    }

    pub(in crate::ui) fn fill(self) -> Hsla {
        if self.high_contrast {
            fluid_current_track_fill(true)
        } else {
            self.accent.alpha(selection::TRACK_TINT)
        }
    }

    pub(in crate::ui) fn secondary(self) -> Hsla {
        rgb(if self.high_contrast {
            contrast::SECONDARY
        } else {
            tokens::SECONDARY
        })
        .into()
    }

    pub(in crate::ui) fn marker(self) -> Div {
        selection_marker(self.accent)
    }

    pub(in crate::ui) fn surface<E: ParentElement + gpui::Styled>(
        self,
        row: E,
        current: bool,
    ) -> E {
        if current {
            row.relative()
                .rounded(px(RADIUS))
                .bg(self.fill())
                .child(self.marker())
        } else {
            row
        }
    }

    pub(in crate::ui) fn playback_marker(self, status: &Status) -> Icon {
        let glyph = match status {
            Status::Paused => IconName::Pause,
            Status::Loading => IconName::LoaderCircle,
            Status::Failed(_) => IconName::CircleAlert,
            _ => IconName::AudioLines,
        };
        let ink = if !self.high_contrast && matches!(status, Status::Failed(_)) {
            rgb(tokens::ERROR).into()
        } else {
            self.accent
        };
        Icon::new(glyph).pixel_size(px(14.)).color(ink)
    }

    pub(in crate::ui) fn cover_marker(self, status: &Status) -> Div {
        div()
            .absolute()
            .bottom_0()
            .right_0()
            .size(px(18.))
            .rounded(px(4.))
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(contrast::CANVAS))
            .child(self.playback_marker(status))
    }

    fn cover_play<E: gpui::Styled + ParentElement>(self, overlay: E, edge: f32) -> E {
        overlay
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(Hsla::from(rgb(contrast::CANVAS)).alpha(if self.high_contrast { 1. } else { 0.72 }))
            .child(
                Icon::new(IconName::Play)
                    .pixel_size(px((edge * 0.48).clamp(14., 20.)))
                    .color(Hsla::from(rgb(contrast::PRIMARY))),
            )
    }

    pub(in crate::ui) fn cover_play_preview(
        self,
        id: impl Into<ElementId>,
        artwork: Div,
        edge: f32,
    ) -> Div {
        artwork.relative().child(
            self.cover_play(div().id(id), edge)
                .opacity(0.)
                .group_hover(HOVER_GROUP, |overlay| overlay.opacity(1.)),
        )
    }

    /// Keeps its hitbox attached to the square cover, independent of the row.
    pub(in crate::ui) fn cover_play_button(
        self,
        id: impl Into<ElementId>,
        name: impl Into<SharedString>,
        order: isize,
        edge: f32,
    ) -> Stateful<Div> {
        let name = name.into();
        self.cover_play(button(id, name.clone(), order), edge)
            .opacity(0.)
            .group_hover(HOVER_GROUP, |control| control.opacity(1.))
            .hover(|control| control.opacity(1.))
            .focus_visible(|control| control.opacity(1.).border_color(rgb(contrast::PRIMARY)))
            .tooltip(guise::tooltip(name))
    }

    /// The current-track fill can live on a wrapper when a row has its own
    /// secondary commands. In that case only the row's hitbox receives hover.
    pub(in crate::ui) fn interaction(self, row: Stateful<Div>) -> Stateful<Div> {
        let (hover, press) = if self.high_contrast {
            (rgb(contrast::ELEVATED).into(), rgb(contrast::CANVAS).into())
        } else {
            let neutral = Hsla::from(rgb(0xffffff));
            (neutral.alpha(0.07), neutral.alpha(0.11))
        };
        let focus =
            Hsla::from(rgb(contrast::PRIMARY)).alpha(if self.high_contrast { 1. } else { 0.80 });
        row.rounded(px(RADIUS))
            .hover(move |row| row.bg(hover))
            .active(move |row| row.bg(press))
            .focus_visible(move |row| row.border_color(focus))
    }

    pub(in crate::ui) fn selected(self, row: Stateful<Div>) -> Stateful<Div> {
        let rule = if self.high_contrast {
            rgb(contrast::RULE).into()
        } else {
            Hsla::from(rgb(0xffffff)).alpha(0.22)
        };
        row.border_color(rule)
    }
}
