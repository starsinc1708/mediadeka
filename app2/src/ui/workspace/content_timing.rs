use std::time::{Duration, Instant};

use gpui::{Context, IntoElement, Styled, Window, canvas, px};

use super::{MainView, Route};

#[cfg(all(test, windows, feature = "capture"))]
mod tests;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) struct ContentSample {
    pub elapsed: Duration,
    kind: ContentKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ContentKind {
    Catalog,
    Playlist,
}

impl ContentSample {
    pub(in crate::ui) fn label(self) -> &'static str {
        match self.kind {
            ContentKind::Catalog => "объекта каталога",
            ContentKind::Playlist => "плейлиста",
        }
    }
}

#[derive(Default)]
pub(super) struct ContentTiming {
    last: Option<(u64, ContentSample)>,
}

impl MainView {
    pub(in crate::ui) fn last_content_opening(&self) -> Option<ContentSample> {
        self.workspace.content_timing.last.map(|(_, sample)| sample)
    }

    #[cfg(feature = "capture")]
    pub fn route_content_ms_for_capture(&self) -> Option<f64> {
        let (selection, _) = self.workspace.routes.selection();
        self.workspace
            .content_timing
            .last
            .filter(|(id, _)| *id == selection)
            .map(|(_, sample)| sample.elapsed.as_secs_f64() * 1000.)
    }

    pub(in crate::ui) fn route_paint_marker(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        let selection = self.workspace.routes.selection();
        if self
            .workspace
            .content_timing
            .last
            .is_some_and(|(id, _)| id == selection.0)
            || self.panel_replaces_page(window)
            || crate::ui::native::minimized(window)
        {
            return None;
        }
        let kind = match self.workspace.routes.current() {
            Route::Object(target) if self.has_loaded_catalog_object(target) => ContentKind::Catalog,
            Route::Playlist(target) if self.playlist.has_loaded_content(target) => {
                ContentKind::Playlist
            }
            _ => return None,
        };
        let view = cx.weak_entity();
        Some(
            canvas(
                |_, _, _| (),
                move |_, (), window, cx| {
                    // The native window presents the painted scene within the same App update.
                    window.defer(cx, move |_, cx| {
                        let _ = view.update(cx, |view, _| {
                            view.finish_content_opening(selection, kind);
                        });
                    });
                },
            )
            .absolute()
            .size(px(1.)),
        )
    }

    fn finish_content_opening(&mut self, (id, began): (u64, Instant), kind: ContentKind) {
        if self.workspace.routes.selection().0 == id
            && self
                .workspace
                .content_timing
                .last
                .is_none_or(|(recorded, _)| recorded != id)
        {
            self.workspace.content_timing.last = Some((
                id,
                ContentSample {
                    kind,
                    elapsed: began.elapsed(),
                },
            ));
        }
    }
}
