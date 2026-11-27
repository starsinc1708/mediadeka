use gpui::{Context, Window};

use crate::soloist::Readiness;

use super::{Kind, MainView, Page};

impl MainView {
    pub fn open_soloist_for_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.show_music_page(Page::Settings, window, cx);
        self.open_soloist(None, window, cx);
    }

    pub fn soloist_stage_for_capture(&self) -> Option<&'static str> {
        if !self.soloist.is_open() || self.soloist.active() == Some(Kind::Check) {
            return None;
        }
        Some(match &self.soloist.readiness {
            Some(Readiness::Missing) => "install",
            Some(Readiness::Installed { .. }) if self.spotify_account().is_some() => "key",
            Some(Readiness::Installed { .. }) => "sign-in",
            Some(Readiness::Ready { .. }) => "ready",
            None => "error",
        })
    }
}
