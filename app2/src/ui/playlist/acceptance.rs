use std::path::Path;

use gpui::{Bounds, Context, Pixels, Window, point, px};

use crate::{
    playback::Status, playlist::PlaylistKey, providers::ProviderId, state::Intent, storage::Store,
};

use super::{MainView, Target, actions::QueueAction, popup::MenuKind};
use crate::ui::{
    components::{
        overlay::TriggerBounds,
        scrollbar::{ScrollChrome, ScrollSurface},
    },
    workspace::Page,
};

pub fn seed_playlist_capture(profile: &Path, files: &[std::path::PathBuf]) -> gpui::Result<i64> {
    let store = Store::open(profile)?;
    store.use_guest(|| Some(()))?;
    Ok(store.create_local_playlist(CAPTURE_PLAYLIST_NAME, files)?)
}

pub fn seed_playlist_source_capture(profile: &Path, provider: ProviderId) -> gpui::Result<()> {
    let store = Store::open(profile)?;
    let account = store
        .preferred_account(provider)?
        .ok_or_else(|| std::io::Error::other("Нет тестового подключения."))?;
    if !store.select_account(provider, &account.profile.id, || Some(()))? {
        return Err(std::io::Error::other("Не удалось выбрать тестовое подключение.").into());
    }
    Ok(())
}

const CAPTURE_PLAYLIST_NAME: &str =
    "Ночной кодинг и музыка для длинного маршрута домой по пустым улицам города";

impl MainView {
    pub fn open_local_playlist_for_capture(
        &mut self,
        id: i64,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.open_local_playlist_with_artwork_for_capture(id, None, window, cx);
    }

    pub fn open_local_playlist_with_artwork_for_capture(
        &mut self,
        id: i64,
        artwork: Option<&str>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if self.state.home_view().is_none() {
            self.state.begin(Intent::Startup);
        }
        self.show_music_page(Page::Playlists, window, cx);
        let mut target = Target::local(id, CAPTURE_PLAYLIST_NAME.to_owned());
        target.artwork = artwork.map(str::to_owned);
        self.show_playlist(target, window, cx);
    }

    pub fn open_source_playlist_for_capture(
        &mut self,
        provider: ProviderId,
        id: String,
        title: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.state.begin(Intent::Startup);
        self.show_playlist(
            Target {
                key: PlaylistKey::Source {
                    provider,
                    profile: None,
                    id,
                },
                name: title,
                artwork: None,
                link: None,
            },
            window,
            cx,
        );
    }

    pub fn open_saved_playlist_for_capture(
        &mut self,
        provider: ProviderId,
        id: String,
        title: String,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Result<()> {
        let account = Store::open_default()?.preferred_account(provider)?;
        let profile = account.map(|account| account.profile.id);
        self.state.begin(Intent::Startup);
        self.show_playlist(
            Target {
                key: PlaylistKey::Source {
                    provider,
                    profile,
                    id,
                },
                name: title,
                artwork: None,
                link: None,
            },
            window,
            cx,
        );
        Ok(())
    }

    pub fn playlist_status_for_capture(&self) -> Option<Result<usize, String>> {
        self.playlist
            .open
            .as_ref()
            .and_then(|open| match &open.content {
                super::Content::Loaded(snapshot) => Some(Ok(snapshot.playlist.tracks.len())),
                super::Content::Loading | super::Content::Partial { .. } => None,
                super::Content::Error(error) => Some(Err(error.message.clone())),
            })
    }

    pub fn playlist_state_for_capture(
        &self,
        cx: &gpui::App,
    ) -> (Option<usize>, Option<usize>, String, f64) {
        let player = self.player.read(cx);
        let status = match &player.snapshot().status {
            Status::Idle => "idle".into(),
            Status::Loading => "loading".into(),
            Status::Playing => "playing".into(),
            Status::Paused => "paused".into(),
            Status::Failed(error) => format!("failed: {error}"),
        };
        (
            self.playlist.open.as_ref().and_then(|open| open.selected),
            player.snapshot().index,
            status,
            player.position(),
        )
    }

    pub fn select_playlist_track_for_capture(
        &mut self,
        index: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.select_track(index, window, cx);
    }

    pub fn close_right_panel_for_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_panel(window, cx);
    }

    pub fn playlist_scrollbar_size_for_capture(&self, window: &Window) -> Option<(f32, f32)> {
        let chrome = ScrollChrome::new(self.uses_fluid_glass(), window);
        ScrollSurface::VirtualList(self.playlist.list.clone()).scrollbar_size_for_capture(chrome)
    }

    pub fn playlist_sort_for_capture(&self) -> &'static str {
        self.playlist.sort.label()
    }

    pub fn sample_playlist_sort_menu_for_capture(
        &mut self,
        sample: Option<std::time::Duration>,
        cx: &mut Context<Self>,
    ) {
        self.playlist.sort_select.sample_menu_for_capture(sample);
        cx.notify();
    }

    pub fn playlist_sort_bounds_for_capture(&self) -> (Bounds<Pixels>, Option<Bounds<Pixels>>) {
        (
            self.playlist.sort_select.trigger_bounds_for_capture(),
            self.playlist.sort_select.menu_bounds_for_capture(),
        )
    }

    pub fn playlist_sort_row_bounds_for_capture(&self, choice: usize) -> Option<Bounds<Pixels>> {
        self.playlist.sort_select.row_bounds_for_capture(choice)
    }

    pub fn playlist_selection_for_capture(&self) -> Option<&str> {
        let open = self.playlist.open.as_ref()?;
        open.playlist()?
            .tracks
            .get(open.selected?)
            .map(|track| track.title.as_str())
    }

    pub fn play_playlist_track_for_capture(&mut self, index: usize, cx: &mut Context<Self>) {
        self.playlist_queue(QueueAction::Play, Some(index), cx);
    }

    pub fn playlist_history_for_capture(
        &mut self,
        forward: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.remember_page_state(cx);
        self.workspace.routes.step(forward);
        self.show_route(window, cx);
    }

    pub fn show_playlist_menu_for_capture(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(row) = self
            .playlist
            .list
            .bounds_for_item(index + super::page::PREFIX_ROWS)
        {
            let anchor = Bounds::from_corners(
                point(row.right() - px(51.), row.top() + px(8.)),
                point(row.right() - px(26.), row.bottom() - px(8.)),
            );
            let bounds = TriggerBounds::default();
            bounds.set(anchor);
            self.show_playlist_menu(MenuKind::Track(index), bounds, window, cx);
        }
    }

    pub fn playlist_scroll_for_capture(&mut self, index: usize, cx: &mut Context<Self>) {
        self.playlist.list.scroll_to(gpui::ListOffset {
            item_ix: index + super::page::PREFIX_ROWS,
            offset_in_item: px(0.),
        });
        cx.notify();
    }

    pub fn set_playlist_scroll_fraction_for_capture(&self, fraction: f32, cx: &mut Context<Self>) {
        let surface = ScrollSurface::VirtualList(self.playlist.list.clone());
        if surface.set_offset(surface.extent() * fraction) {
            cx.notify();
        }
    }

    pub fn playlist_scroll_bounds_for_capture(&self) -> (Bounds<Pixels>, f32, f32) {
        let surface = ScrollSurface::VirtualList(self.playlist.list.clone());
        (
            surface.viewport(),
            f32::from(surface.offset()),
            f32::from(surface.extent()),
        )
    }

    pub fn playlist_geometry_for_capture(&self, row: usize) -> (f32, f32, f32, usize, bool) {
        let list = &self.playlist.list;
        let bounds = list.viewport_bounds();
        let rendered = list
            .bounds_for_item(row + super::page::PREFIX_ROWS)
            .is_some_and(|row| row.intersects(&bounds));
        (
            f32::from(bounds.left()),
            f32::from(bounds.top()),
            f32::from(bounds.bottom()),
            list.logical_scroll_top().item_ix,
            rendered,
        )
    }

    pub fn playlist_density_for_capture(&self) -> gpui::Result<(&'static str, Option<f32>)> {
        let (density, _) = Store::open_default()?.track_display()?;
        let height = self
            .playlist
            .list
            .bounds_for_item(super::page::PREFIX_ROWS)
            .map(|bounds| f32::from(bounds.size.height));
        Ok((density.as_str(), height))
    }

    pub fn playlist_filter_top_for_capture(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<f32> {
        if let Some(hero) = self.playlist_collapsing_hero(window, cx) {
            return Some(f32::from(
                self.playlist.list.viewport_bounds().top() + hero.height(),
            ));
        }
        self.playlist
            .list
            .bounds_for_item(super::page::TOOLBAR_ITEM)
            .map(|bounds| f32::from(bounds.top()))
    }

    pub fn playlist_scrollbar_for_capture(
        &self,
        window: &Window,
    ) -> Option<(Bounds<Pixels>, Pixels, bool)> {
        let list = &self.playlist.list;
        let chrome = ScrollChrome::new(self.uses_fluid_glass(), window);
        let (track, thumb_y) =
            ScrollSurface::VirtualList(list.clone()).track_for_capture(chrome)?;
        Some((track, thumb_y, list.is_scrollbar_dragging()))
    }
}
