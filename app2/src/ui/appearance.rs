//! The appearance active in this process.

use gpui::{App, Global};

use crate::{
    appearance::{Appearance, GlassPalette, TrackColumn},
    storage::{Store, StoreError},
    visual::VisualSettings,
};

impl super::MainView {
    pub(in crate::ui) fn toggle_track_column(
        &mut self,
        column: TrackColumn,
    ) -> Result<(), StoreError> {
        let visibility = self.track_column_visibility.toggle(column);
        Store::open_default()?.save_track_columns(visibility)?;
        self.track_column_visibility = visibility;
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct ActiveAppearance(Appearance);

impl Global for ActiveAppearance {}

pub(in crate::ui) fn install(cx: &mut App, appearance: Appearance) {
    cx.set_global(ActiveAppearance(appearance));
}

pub(in crate::ui) fn appearance(cx: &App) -> Appearance {
    cx.try_global::<ActiveAppearance>()
        .map_or_else(Appearance::default, |active| active.0)
}

pub(in crate::ui) fn palette(cx: &App) -> GlassPalette {
    appearance(cx).palette
}

pub(in crate::ui) fn visual(cx: &App) -> VisualSettings {
    appearance(cx).visual
}

pub(in crate::ui) fn set_palette(cx: &mut App, palette: GlassPalette) {
    let mut appearance = appearance(cx);
    appearance.palette = palette;
    cx.set_global(ActiveAppearance(appearance));
}

pub(in crate::ui) fn set_visual(cx: &mut App, visual: VisualSettings) {
    let mut appearance = appearance(cx);
    appearance.visual = visual;
    cx.set_global(ActiveAppearance(appearance));
}
