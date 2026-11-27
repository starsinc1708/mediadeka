use std::{path::Path, sync::Arc};

use gpui::{Context, Window};
use serde_json::{Value, json};

use crate::{
    music::{ArtistDetails, MusicPage, ObjectKind, ObjectRef, ScopedPage},
    providers::ProviderId,
    ui::{
        MainView,
        components::artwork,
        workspace::{MusicTarget, Page, Route},
    },
};

use super::detail::Detail;

impl MainView {
    pub fn replace_release_artwork_for_capture(&mut self, artwork: &str, cx: &mut Context<Self>) {
        if let Some(data) = self
            .music
            .object
            .as_mut()
            .and_then(|detail| detail.data.as_mut())
        {
            Arc::make_mut(&mut data.page).artwork = Some(artwork.to_owned());
            cx.notify();
        }
    }

    pub fn seed_artist_artwork_for_capture(
        profile: &Path,
        url: &str,
        fixture: &Path,
    ) -> std::io::Result<()> {
        artwork::seed_cover_for_capture(profile, url, fixture)
    }

    pub fn show_artist_artwork_for_capture(
        &mut self,
        portrait: Option<&str>,
        banner: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let target = MusicTarget {
            object: ObjectRef {
                provider: ProviderId::Spotify,
                id: "ArtistCapture".into(),
                kind: ObjectKind::Artist,
            },
            profile: None,
            title: "Артист".into(),
            artwork: portrait.map(str::to_owned),
        };
        let mut page = MusicPage::new(target.object.clone(), target.title.clone());
        page.artwork = target.artwork.clone();
        page.artist_details = Some(ArtistDetails {
            banner: banner.map(str::to_owned),
            monthly_listeners: Some(31_440),
            ..Default::default()
        });
        self.workspace.routes.push(Route::Object(target.clone()));
        let mut detail = Detail::new(target);
        detail.data = Some(ScopedPage {
            page: Arc::new(page),
            profile: None,
            receipt: None,
        });
        self.music.object = Some(detail);
        cx.notify();
    }

    pub fn artist_artwork_decoded_for_capture(&self, url: &str, cx: &mut gpui::App) -> bool {
        artwork::decoded_for_capture(url, cx)
    }

    pub fn selected_release_artwork_decoded_for_capture(
        &self,
        window: &Window,
        cx: &mut gpui::App,
    ) -> bool {
        self.has_selected_release()
            && self
                .displayed_music_object()
                .and_then(|detail| detail.data.as_ref())
                .and_then(|data| data.page.artwork.as_deref())
                .is_some_and(|url| {
                    artwork::decoded_edge_for_capture(
                        url,
                        crate::ui::fluid_glass::artwork::HERO_IMAGE_EDGE,
                        window,
                        cx,
                    )
                })
    }

    pub fn submit_music_url_for_capture(
        &mut self,
        url: &str,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.submit_catalog(ObjectKind::Track, url.to_owned(), window, cx);
    }

    pub fn show_music_collection_for_capture(
        &mut self,
        playlists: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.show_music_page(
            if playlists {
                Page::Playlists
            } else {
                Page::Tracks
            },
            window,
            cx,
        );
    }

    pub fn music_semantics_for_capture(&self) -> Value {
        let page = match self.music_page() {
            Page::Object => "object",
            Page::Tracks => "tracks",
            Page::Albums => "albums",
            Page::Artists => "artists",
            Page::Playlists => "playlists",
            Page::Playlist => "playlist",
            Page::Search => "search",
            _ => "workspace",
        };
        let details = self.music.object.as_ref().map(|detail| {
            json!({
                "title": detail.data.as_ref().map(|data| data.page.title.as_str()),
                "kind": detail.data.as_ref().map(|data| data.page.reference.kind.path()),
                "tracks": detail.data.as_ref().map(|data| data.page.tracks.len()),
                "releases": detail.data.as_ref().map(|data| data.page.releases.len()),
                "single": detail.data.as_ref().map(|data| data.page.single),
                "verified_sources": detail.links.iter().map(|link| link.platform.as_str()).collect::<Vec<_>>(),
                "error": detail.error.as_ref().map(|error| &error.message),
            })
        });
        let sources = self
            .music
            .collections
            .sources
            .iter()
            .map(|(provider, source)| (provider.as_str(), source.entries.len()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let release_selection = self.selected_release_index().and_then(|index| {
            let page = &self.displayed_music_object()?.data.as_ref()?.page;
            Some(json!({
                "release": page.title,
                "track": page.tracks.get(index)?.title,
                "index": index,
                "tracks": page.tracks.len()
            }))
        });
        json!({ "page": page, "pending": self.music.request.is_some(), "details": details, "collections": sources,
            "release_selection": release_selection })
    }

    pub fn retry_liked_for_capture(&mut self, cx: &mut Context<Self>) {
        self.refresh_library(cx);
    }
}
