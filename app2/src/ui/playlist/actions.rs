use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::{Context, PathPromptOptions, Window, px};

use crate::{
    playback::{CollectionPlan, Status, can_resolve},
    playlist::{
        Media, PlaylistKey,
        library::{ChangeError, LocalChange, LocalChanged, local_playlists},
    },
    storage::{ImportProgress, is_m3u_path},
    ui::workspace::{LibraryTab, Page, PageState, PlaylistLocation, Route},
};

use super::{
    Content, MainView, Target,
    local::{Import, PendingEdit},
    snapshot::Snapshot,
};

#[cfg(all(test, feature = "capture"))]
mod tests;

#[derive(Clone, Copy)]
pub(super) enum Edit {
    Create,
    Rename(i64),
    Delete(i64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ImportPaths {
    Files,
    Folder,
    M3u,
}

#[derive(Clone, Copy)]
pub(in crate::ui) enum QueueAction {
    Play,
    Shuffle,
    Next,
    End,
}

impl MainView {
    pub(in crate::ui) fn open_playlist_route(&mut self, cx: &mut Context<Self>) {
        let place = match self.workspace.routes.state() {
            PageState::Playlist(place) => place.clone(),
            _ => PlaylistLocation::default(),
        };
        {
            let ui = &mut self.playlist;
            ui.menu = None;
            ui.edit = None;
            ui.edit_return_focus = None;
            ui.notice = None;
            ui.open = None;
            ui.pending_offset = None;
            ui.pending_selection = place.selection;
            ui.sort = place.sort;
            ui.query = place.query.trim().to_lowercase();
            ui.search.update(cx, |field, cx| {
                if field.text() != place.query {
                    field.set_text(&place.query, cx);
                }
            });
        }
        match self.workspace.routes.current().clone() {
            Route::Playlist(target) => {
                self.playlist.pending_offset = (place.offset.item_ix > 0
                    || place.offset.offset_in_item > px(0.))
                .then_some(place.offset);
                self.open_playlist(target, cx);
            }
            Route::Library(LibraryTab::Playlists) => self.refresh_local_playlists(cx),
            _ => {}
        }
    }

    pub(super) fn choose_playlist_paths(&mut self, kind: ImportPaths, cx: &mut Context<Self>) {
        self.choose_local_import(kind, None, cx);
    }

    pub(super) fn add_files_to_playlist(&mut self, id: i64, cx: &mut Context<Self>) {
        self.choose_local_import(ImportPaths::Files, Some(id), cx);
    }

    fn choose_local_import(
        &mut self,
        kind: ImportPaths,
        target: Option<i64>,
        cx: &mut Context<Self>,
    ) {
        if self.playlist.local_busy {
            return;
        }
        self.playlist.local_busy = true;
        let folders = kind == ImportPaths::Folder;
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: !folders,
            directories: folders,
            multiple: kind == ImportPaths::Files,
            prompt: Some(
                match kind {
                    ImportPaths::Files => "Выберите аудиофайлы",
                    ImportPaths::Folder => "Выберите папку с музыкой",
                    ImportPaths::M3u => "Выберите плейлист M3U или M3U8",
                }
                .into(),
            ),
        });
        let task = cx.spawn(async move |view, cx| {
            let paths = paths.await;
            let _ = view.update(cx, |view, cx| {
                view.playlist.local_busy = false;
                match paths {
                    Ok(Ok(Some(files)))
                        if kind == ImportPaths::M3u
                            && files.iter().any(|file| !is_m3u_path(file)) =>
                    {
                        view.playlist.notice =
                            Some("Выберите файл с расширением M3U или M3U8.".into());
                    }
                    Ok(Ok(Some(files))) if !files.is_empty() => {
                        let change = if let Some(id) = target {
                            LocalChange::Append { id, files }
                        } else {
                            let name = if files.len() == 1 {
                                (if folders {
                                    files[0].file_name()
                                } else {
                                    files[0].file_stem()
                                })
                                .and_then(|name| name.to_str())
                                .unwrap_or("Новый плейлист")
                                .to_owned()
                            } else {
                                "Новый плейлист".to_owned()
                            };
                            LocalChange::Create { name, files }
                        };
                        view.start_local_change(change, cx);
                    }
                    Ok(Err(error)) if view.shows_import_destination(target) => {
                        view.playlist.notice =
                            Some(format!("Не удалось открыть выбор пути. {error}"));
                    }
                    _ => {}
                }
                cx.notify();
            });
        });
        self.playlist.local_task = Some(task);
        cx.notify();
    }

    fn shows_import_destination(&self, target: Option<i64>) -> bool {
        target.map_or_else(
            || self.music_page() == Page::Playlists,
            |id| {
                self.playlist
                    .open
                    .as_ref()
                    .is_some_and(|open| open.target.key == PlaylistKey::Local(id))
            },
        )
    }

    pub(super) fn edit_local_playlist(
        &mut self,
        edit: Edit,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ui = &mut self.playlist;
        if ui.local_busy {
            return;
        }
        ui.edit = Some(edit);
        ui.edit_return_focus = window.focused(cx);
        ui.notice = None;
        if matches!(edit, Edit::Delete(_)) {
            window.focus(&ui.edit_cancel_focus, cx);
        } else {
            let name = match edit {
                Edit::Create | Edit::Delete(_) => "",
                Edit::Rename(id) => ui.local_name(id).unwrap_or_default(),
            };
            ui.editor.update(cx, |field, cx| {
                field.set_text(name, cx);
                field.focus(window, cx);
            });
        }
        cx.notify();
    }

    pub(super) fn dismiss_local_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let ui = &mut self.playlist;
        if ui.local_busy || ui.edit.take().is_none() {
            return false;
        }
        if let Some(restore) = ui.edit_return_focus.take() {
            window.focus(&restore, cx);
        }
        cx.notify();
        true
    }

    pub(super) fn submit_local_change(&mut self, cx: &mut Context<Self>) {
        let ui = &self.playlist;
        let name = ui.editor.read(cx).text().trim().to_owned();
        let change = match ui.edit {
            Some(Edit::Create) => LocalChange::Create {
                name,
                files: Vec::new(),
            },
            Some(Edit::Rename(id)) => LocalChange::Rename { id, name },
            Some(Edit::Delete(id)) => LocalChange::Delete(id),
            None => return,
        };
        self.start_local_change(change, cx);
    }

    pub(in crate::ui) fn remove_local_playlist_track(
        &mut self,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.playlist.open.as_ref() else {
            return;
        };
        let PlaylistKey::Local(id) = open.target.key else {
            return;
        };
        let Some(snapshot) = open.snapshot() else {
            return;
        };
        let Some(track) = snapshot.playlist.tracks.get(index) else {
            return;
        };
        if !matches!(track.media, Some(Media::File(_))) {
            return;
        }
        self.start_local_change(
            LocalChange::RemoveTrack {
                id,
                index,
                path: track.id.clone(),
                revision: snapshot.revision,
            },
            cx,
        );
    }

    fn start_local_change(&mut self, change: LocalChange, cx: &mut Context<Self>) {
        let ui = &mut self.playlist;
        if ui.local_busy {
            return;
        }
        ui.local_busy = true;
        ui.notice = None;
        let previewed = ui.preview_local_edit(&change);
        if matches!(ui.pending_local_edit, Some(PendingEdit::RemoveTrack { .. })) {
            ui.project(false);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let import = match &change {
            LocalChange::Create { files, .. } if !files.is_empty() => Some(None),
            LocalChange::Append { id, .. } => Some(Some(*id)),
            _ => None,
        };
        if let Some(target) = import {
            ui.import = Some(Import {
                target,
                cancelled: cancelled.clone(),
                progress: ImportProgress::Discovering { found: 0 },
            });
        }
        let (sender, receiver) = async_channel::bounded(2);
        let task = cx.spawn(async move |view, cx| {
            let worker = cx.background_executor().spawn(async move {
                change
                    .run(&cancelled, |progress| {
                        if sender.send_blocking(progress).is_err() {
                            cancelled.store(true, Ordering::Relaxed);
                        }
                    })
                    .map(|change| {
                        change.map_playlist(|id, stored| {
                            Snapshot::prepare(
                                stored.playlist,
                                stored.revision,
                                &PlaylistKey::Local(id),
                            )
                        })
                    })
            });
            while let Ok(progress) = receiver.recv().await {
                let _ = view.update(cx, |view, cx| {
                    if let Some(import) = &mut view.playlist.import {
                        import.progress = progress;
                        cx.notify();
                    }
                });
            }
            let result = worker.await;
            let _ = view.update(cx, |view, cx| {
                view.playlist.import = None;
                view.playlist.local_busy = false;
                let preview = view.playlist.pending_local_edit.take();
                match result {
                    Ok(LocalChanged::Created(id, name)) if view.music_page() == Page::Playlists => {
                        let target = Target::local(id, name);
                        view.playlist.edit = None;
                        view.remember_page_state(cx);
                        view.workspace.routes.push(Route::Playlist(target));
                        view.show_current_route(cx);
                    }
                    Ok(LocalChanged::Renamed(id, name)) => {
                        view.workspace.routes.rename_local(id, &name);
                        let ui = &mut view.playlist;
                        ui.edit = None;
                        ui.edit_return_focus = None;
                        let interrupted = ui
                            .open
                            .as_mut()
                            .filter(|open| open.target.key == PlaylistKey::Local(id))
                            .and_then(|open| open.request.take())
                            .is_some();
                        if ui.set_local_name(id, &name) {
                            view.sync_library_local_playlists(cx);
                        } else if view.music_page() == Page::Playlists {
                            view.refresh_local_playlists(cx);
                        }
                        if interrupted {
                            view.refresh_playlist(cx);
                        }
                        view.refresh_pins(cx);
                    }
                    Ok(LocalChanged::Deleted(id)) => {
                        let playlists = view.workspace.routes.current().page() == Page::Playlists;
                        view.workspace.routes.remove_local(id);
                        view.playlist.edit = None;
                        view.playlist.local.retain(|entry| entry.id != id);
                        if playlists || view.music_page() == Page::Playlists {
                            view.show_current_route(cx);
                        }
                        view.refresh_pins(cx);
                    }
                    Ok(
                        LocalChanged::Appended(id, snapshot)
                        | LocalChanged::TrackRemoved(id, snapshot),
                    ) => {
                        view.finish_local_tracks_change(id, snapshot, preview, cx);
                    }
                    Ok(LocalChanged::Created(..)) => view.refresh_library(cx),
                    Err(error) => {
                        let reload = matches!((&error, &preview),
                            (ChangeError::TrackChanged, Some(PendingEdit::RemoveTrack { id, .. }))
                            if view.shows_import_destination(Some(*id)));
                        if let Some(preview) = preview {
                            view.playlist.undo_local_edit_preview(preview);
                            view.sync_library_local_playlists(cx);
                        }
                        if import.is_none_or(|target| view.shows_import_destination(target)) {
                            view.playlist.notice = Some(error.to_string());
                        }
                        if reload {
                            view.refresh_playlist(cx);
                        }
                    }
                }
                cx.notify();
            });
        });
        self.playlist.local_task = Some(task);
        if previewed {
            self.sync_library_local_playlists(cx);
        }
        cx.notify();
    }

    fn finish_local_tracks_change(
        &mut self,
        id: i64,
        snapshot: Arc<Snapshot>,
        pending: Option<PendingEdit>,
        cx: &mut Context<Self>,
    ) {
        let ui = &mut self.playlist;
        let count = snapshot.playlist.tracks.len();
        if let Some(summary) = ui.local.iter_mut().find(|item| item.id == id) {
            summary.track_count = count;
            if count == 0 {
                summary.has_files = false;
            }
        }
        if let Some(open) = ui
            .open
            .as_mut()
            .filter(|open| open.target.key == PlaylistKey::Local(id))
        {
            let removed = match &pending {
                Some(PendingEdit::RemoveTrack { index, .. }) => Some(*index),
                _ => None,
            };
            open.request.take();
            open.selected = open.selected.and_then(|selected| {
                let track = open.snapshot()?.playlist.tracks.get(selected)?;
                let preferred =
                    selected - usize::from(removed.is_some_and(|index| index < selected));
                if snapshot
                    .playlist
                    .tracks
                    .get(preferred)
                    .is_some_and(|row| row.id == track.id)
                {
                    Some(preferred)
                } else {
                    snapshot
                        .playlist
                        .tracks
                        .iter()
                        .position(|row| row.id == track.id)
                }
            });
            open.content = Content::Loaded(snapshot);
            if let Some(removed) = removed
                && let Some((selected, _)) = &mut ui.pending_selection
            {
                if *selected == removed {
                    ui.pending_selection = None;
                } else if *selected > removed {
                    *selected -= 1;
                }
            }
            ui.restore_playlist_selection();
            ui.pending_selection = None;
            ui.menu = None;
            ui.project(false);
        }
        self.sync_library_local_playlists(cx);
        self.refresh_local_playlists(cx);
        self.refresh_library(cx);
    }

    fn refresh_local_playlists(&mut self, cx: &mut Context<Self>) {
        if self.playlist.local_busy {
            return;
        }
        let task = cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async { local_playlists() })
                .await;
            let _ = view.update(cx, |view, cx| {
                match result {
                    Ok(items) => {
                        view.playlist.local = items;
                        view.sync_library_local_playlists(cx);
                    }
                    Err(error) => view.playlist.notice = Some(error.to_string()),
                }
                cx.notify();
            });
        });
        self.playlist.local_task = Some(task);
    }

    pub(super) fn select_track(&mut self, index: usize, window: &Window, cx: &mut Context<Self>) {
        self.select_inspector_track(index, window, cx);
        self.reveal_track_tab(window, cx);
        cx.notify();
    }

    /// The row menu command «Сведения» shows the row in the track tab.
    pub(in crate::ui) fn show_playlist_details(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_inspector_track(index, window, cx);
        self.open_panel(crate::storage::PanelTab::Track, true, window, cx);
    }

    fn select_inspector_track(&mut self, index: usize, window: &Window, cx: &Context<Self>) {
        let follows_playing = self.panel_follows_playing_track();
        let Some(open) = self.playlist.open.as_mut() else {
            return;
        };
        let changed = open.selected != Some(index) || !self.panel.open || follows_playing;
        open.selected = Some(index);
        if changed {
            let span = crate::ui::panel::playlist_track_artwork_span(&open.target.key, index);
            let has_image = open
                .playlist()
                .and_then(|playlist| playlist.tracks.get(index))
                .is_some_and(|track| track.artwork.is_some());
            self.point_inspector_at_playlist_track(span, has_image, window, cx);
        }
    }

    pub(super) fn activate_playlist(&mut self, cx: &mut Context<Self>) {
        let Some(open) = self.playlist.open.as_ref() else {
            return;
        };
        let current = self.player.read(cx).snapshot();
        if current.key.as_ref() == Some(&open.target.key)
            && matches!(current.status, Status::Playing | Status::Paused)
        {
            self.player.update(cx, |bar, cx| {
                bar.command(cx, |playback| playback.toggle_pause())
            });
        } else {
            self.playlist_queue(QueueAction::Play, None, cx);
        }
    }

    pub(in crate::ui) fn playlist_queue(
        &mut self,
        action: QueueAction,
        row: Option<usize>,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &self.playlist.open else {
            return;
        };
        let Some(snapshot) = open.snapshot() else {
            return;
        };
        let playlist = &snapshot.playlist;
        let can_play = |track| can_resolve(&open.target.key, track);
        let first = row.or_else(|| {
            open.visible
                .iter()
                .copied()
                .find(|&index| can_play(&playlist.tracks[index]))
        });
        let Some(first) = first else {
            return;
        };
        if !playlist.tracks.get(first).is_some_and(can_play) {
            return;
        }
        let play = matches!(action, QueueAction::Play | QueueAction::Shuffle);
        let indices = if !play && row.is_some() {
            std::slice::from_ref(&first)
        } else {
            &open.visible
        };
        let mut start = 0;
        let entries = indices
            .iter()
            .filter_map(|&index| open.queue_entry(index))
            .enumerate()
            .map(|(position, entry)| {
                if entry.index == first {
                    start = position;
                }
                entry
            })
            .collect::<Vec<_>>();
        let plan = || {
            open.complete_snapshot().map(|_| {
                Arc::new(CollectionPlan::new(
                    open.target.key.clone(),
                    &playlist.name,
                    playlist
                        .artwork
                        .as_deref()
                        .or(open.target.artwork.as_deref()),
                    &playlist.tracks,
                ))
            })
        };
        self.player.update(cx, |bar, cx| {
            bar.command(cx, |playback| match action {
                QueueAction::Play => match plan() {
                    Some(plan) => playback.play_collection(entries, start, plan),
                    None => playback.play(entries, start),
                },
                QueueAction::Shuffle => match plan() {
                    Some(plan) => playback.shuffle_collection(entries, plan),
                    None => playback.play_shuffled(entries),
                },
                QueueAction::Next => playback.insert_next(entries),
                QueueAction::End => playback.append(entries),
            })
        });
    }
}
