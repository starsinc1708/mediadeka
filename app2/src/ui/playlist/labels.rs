use crate::{providers::ProviderId, ui::theme::SECONDARY};
use gpui::{Div, Role, SharedString, Stateful, div, prelude::*, px, rgb};

use super::{MainView, OpenPlaylist};

impl MainView {
    pub(super) fn playlist_owner_name<'a>(&'a self, open: &'a OpenPlaylist) -> Option<&'a str> {
        let owner = open.playlist()?.owner.as_ref()?.name().trim();
        if owner.is_empty() {
            return None;
        }
        if open.target.key.provider() != Some(ProviderId::Spotify) {
            return Some(owner);
        }
        // Spotify's playlist protocol provides an account key. Resolve it
        // with the display name from a known connected profile when possible.
        let (home, _) = self.state.home_view()?;
        home.accounts
            .iter()
            .find(|account| account.provider == ProviderId::Spotify && account.profile.id == owner)
            .map(|account| account.profile.name.trim())
            .filter(|display| !display.is_empty() && *display != owner)
    }
}

pub(super) fn status_label(id: &'static str, text: impl Into<SharedString>) -> Stateful<Div> {
    let text = text.into();
    div()
        .id(id)
        .accessibility_id(id)
        .role(Role::Status)
        .aria_label(text.clone())
        .text_color(rgb(SECONDARY))
        .text_size(px(13.))
        .py(px(4.))
        .child(text)
}
