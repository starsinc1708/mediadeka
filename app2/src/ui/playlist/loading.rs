use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::Context;

use crate::{
    music::library::{PlaylistCache, PlaylistScope},
    playlist::{
        PlaylistKey,
        library::{ReadError, read_local_playlist_snapshot, read_playlist},
    },
    providers::{ErrorKind, ProviderError, ProviderId},
    telemetry::{self, CatalogAccess},
};

use super::{
    Content, MainView, OpenPlaylist, PlaylistUi, Request, SourceFailure, Target, snapshot::Snapshot,
};

#[cfg(all(test, feature = "capture", windows))]
mod tests;

enum LoadProgress {
    Cached(Arc<Snapshot>),
    Page {
        snapshot: Arc<Snapshot>,
        offset: usize,
        total: Option<usize>,
    },
}

impl MainView {
    pub(super) fn open_playlist(&mut self, mut target: Target, cx: &mut Context<Self>) {
        if let PlaylistKey::Source {
            provider, profile, ..
        } = &mut target.key
            && profile.is_none()
        {
            *profile = self
                .connected_provider_profile(*provider)
                .map(str::to_owned);
        }
        if let Some(name) = self.playlist.pending_local_name(&target.key) {
            target.name = name.to_owned();
        }
        let ui = &mut self.playlist;
        ui.open = Some(OpenPlaylist {
            target,
            content: Content::Loading,
            request: None,
            refresh_error: None,
            visible: Vec::new(),
            selected: None,
            visible_audio: false,
        });
        ui.list.reset(super::page::PREFIX_ROWS);
        self.load_playlist(false, cx);
    }

    pub(super) fn refresh_playlist(&mut self, cx: &mut Context<Self>) {
        self.load_playlist(true, cx);
    }

    fn load_playlist(&mut self, force: bool, cx: &mut Context<Self>) {
        let ui = &mut self.playlist;
        let Some(open) = ui.open.as_mut() else {
            return;
        };
        if open.request.is_some() {
            return;
        }
        let key = open.target.key.clone();
        let previous = open
            .complete_snapshot()
            .cloned()
            .or_else(|| ui.snapshots.get(&key));
        let disk_revision = ui.snapshots.revision_for(&key);
        let revision = ui.snapshots.revision_for(&key);
        if open.snapshot().is_none() {
            open.content = Content::Loading;
        }
        open.refresh_error = None;
        let cancelled = Arc::new(AtomicBool::new(false));
        let work = cancelled.clone();
        let request_flag = cancelled.clone();
        let task = cx.spawn(async move |view, cx| {
            let (progress_tx, progress_rx) = async_channel::bounded(4);
            let worker_key = key.clone();
            let task = cx.background_executor().spawn(async move {
                read_snapshot(
                    &worker_key,
                    previous,
                    disk_revision,
                    revision,
                    force,
                    &work,
                    progress_tx,
                )
            });
            while let Ok(progress) = progress_rx.recv().await {
                if cancelled.load(Ordering::Relaxed)
                    || view
                        .update(cx, |view, cx| {
                            view.present_playlist_progress(&key, progress, cx)
                        })
                        .is_err()
                {
                    return;
                }
            }
            let result = task.await;
            if !cancelled.load(Ordering::Relaxed) {
                let _ = view.update(cx, |view, cx| view.finish_playlist(key, result, force, cx));
            }
        });
        open.request = Some(Request {
            cancelled: request_flag,
            _task: task,
        });
        cx.notify();
    }

    fn present_playlist_progress(
        &mut self,
        key: &PlaylistKey,
        progress: LoadProgress,
        cx: &mut Context<Self>,
    ) {
        let ui = &mut self.playlist;
        let Some(open) = ui
            .open
            .as_mut()
            .filter(|open| &open.target.key == key && open.complete_snapshot().is_none())
        else {
            return;
        };
        let first = match progress {
            LoadProgress::Cached(snapshot) => {
                open.content = Content::Loaded(snapshot);
                true
            }
            LoadProgress::Page {
                snapshot,
                offset: 0,
                total,
            } => {
                let mut snapshot = snapshot;
                let selected = open.selected.take();
                if let Some(previous) = open.snapshot() {
                    if let Some(index) = selected
                        && let Some(track) = previous.playlist.tracks.get(index)
                    {
                        ui.pending_selection = Some((index, track.id.clone()));
                    }
                    if previous.revision == snapshot.revision {
                        Arc::make_mut(&mut snapshot).revision = ui.snapshots.revision_for(key);
                    }
                    ui.menu = None;
                }
                open.content = Content::Partial { snapshot, total };
                true
            }
            LoadProgress::Page {
                snapshot,
                offset,
                total,
            } => {
                let Content::Partial {
                    snapshot: current,
                    total: current_total,
                } = &mut open.content
                else {
                    return;
                };
                if current.playlist.tracks.len() != offset || *current_total != total {
                    return;
                }
                Arc::make_mut(current).append_page(Arc::unwrap_or_clone(snapshot));
                false
            }
        };
        let receipt = open
            .complete_snapshot()
            .and_then(|snapshot| snapshot.receipt());
        ui.restore_playlist_selection();
        ui.project(false);
        if let Some(receipt) = receipt {
            self.observe_prefetched_playlist(receipt);
        }
        if first {
            self.ensure_playlist_favorites(key, cx);
        }
        cx.notify();
    }

    fn finish_playlist(
        &mut self,
        key: PlaylistKey,
        result: Result<Arc<Snapshot>, ReadError>,
        force: bool,
        cx: &mut Context<Self>,
    ) {
        let ui = &mut self.playlist;
        let pending_name = ui.pending_local_name(&key).map(str::to_owned);
        let Some(open) = ui.open.as_mut().filter(|open| open.target.key == key) else {
            return;
        };
        open.request.take();
        match result {
            Ok(mut snapshot) => {
                let was_partial = matches!(open.content, Content::Partial { .. });
                if was_partial && let Some(partial) = open.snapshot() {
                    if snapshot
                        .playlist
                        .tracks
                        .starts_with(&partial.playlist.tracks)
                    {
                        Arc::make_mut(&mut snapshot).revision = partial.revision;
                    } else if snapshot.revision == partial.revision {
                        Arc::make_mut(&mut snapshot).revision = ui.snapshots.revision_for(&key);
                    }
                }
                if let Some(name) = pending_name
                    && snapshot.playlist.name != name
                {
                    Arc::make_mut(&mut snapshot).playlist.name = name;
                }
                let unchanged = open
                    .snapshot()
                    .is_some_and(|shown| Arc::ptr_eq(shown, &snapshot));
                if !unchanged && open.snapshot().is_some() {
                    if (!force || was_partial)
                        && ui.pending_selection.is_none()
                        && let Some(index) = open.selected
                        && let Some(track) = open
                            .snapshot()
                            .and_then(|snapshot| snapshot.playlist.tracks.get(index))
                    {
                        ui.pending_selection = Some((index, track.id.clone()));
                    }
                    open.selected = None;
                    ui.menu = None;
                }
                let receipt = snapshot.receipt();
                ui.snapshots.remember(key.clone(), Arc::clone(&snapshot));
                if !unchanged {
                    open.content = Content::Loaded(snapshot);
                    ui.restore_playlist_selection();
                    ui.project(false);
                }
                ui.pending_selection = None;
                if let Some(receipt) = receipt {
                    self.observe_prefetched_playlist(receipt);
                }
                self.ensure_playlist_favorites(&key, cx);
            }
            Err(error) if retain_playlist(&error) && open.snapshot().is_some() => {
                open.refresh_error = Some(playlist_failure(&key, &error));
            }
            Err(error) => {
                if let PlaylistKey::Source {
                    provider,
                    profile: Some(profile),
                    ..
                } = &key
                    && (error.revokes_account()
                        || matches!(
                            error,
                            ReadError::Access(crate::playlist::library::AccessError::Disconnected)
                        ))
                {
                    ui.snapshots.forget_account(*provider, Some(profile));
                } else {
                    ui.snapshots.forget(&key);
                }
                open.content = Content::Error(playlist_failure(&key, &error));
                ui.menu = None;
                ui.list.reset(super::page::PREFIX_ROWS);
            }
        }
        cx.notify();
    }

    pub(in crate::ui) fn forget_account_playlists(
        &mut self,
        provider: ProviderId,
        profile: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let ui = &mut self.playlist;
        ui.snapshots.forget_account(provider, profile);
        if let Some(open) = &mut ui.open
            && super::matches_playlist_account(&open.target.key, provider, profile)
        {
            open.request.take();
            open.content = Content::Loading;
            open.refresh_error = None;
            open.visible.clear();
            open.visible_audio = false;
            open.selected = None;
            ui.menu = None;
            ui.list.reset(super::page::PREFIX_ROWS);
            self.load_playlist(false, cx);
        }
    }

    pub(in crate::ui) fn retire_catalog_playlists(&mut self, generation: i64) {
        self.playlist.snapshots.retire(generation);
    }
}

impl PlaylistUi {
    pub(super) fn restore_playlist_selection(&mut self) {
        let Some((index, id)) = &self.pending_selection else {
            return;
        };
        let removed = self
            .open
            .as_ref()
            .and_then(|open| self.projected_removed_track(open));
        let Some(open) = self.open.as_mut() else {
            return;
        };
        let Some(snapshot) = open.snapshot() else {
            return;
        };
        let tracks = &snapshot.playlist.tracks;
        let index = if tracks.get(*index).is_some_and(|track| &track.id == id) {
            Some(*index)
        } else {
            tracks.iter().position(|track| &track.id == id)
        };
        if let Some(index) = index.filter(|index| removed != Some(*index)) {
            open.selected = Some(index);
            self.pending_selection = None;
        }
    }
}

fn read_snapshot(
    key: &PlaylistKey,
    previous: Option<Arc<Snapshot>>,
    disk_revision: Option<u64>,
    revision: Option<u64>,
    force: bool,
    cancelled: &AtomicBool,
    progress: async_channel::Sender<LoadProgress>,
) -> Result<Arc<Snapshot>, ReadError> {
    if !matches!(key, PlaylistKey::Source { .. }) {
        if let PlaylistKey::Local(id) = key {
            let data = read_local_playlist_snapshot(*id)?;
            return Ok(previous
                .filter(|snapshot| {
                    snapshot.playlist == data.playlist && snapshot.revision == data.revision
                })
                .unwrap_or_else(|| Snapshot::prepare(data.playlist, data.revision, key)));
        }
        let playlist = read_playlist(key, cancelled)?;
        return Ok(previous
            .filter(|snapshot| snapshot.playlist == playlist)
            .unwrap_or_else(|| Snapshot::prepare(playlist, revision, key)));
    }
    let scope = PlaylistScope::open(key)?;
    let receipt = previous.as_ref().and_then(|snapshot| snapshot.receipt());
    let presented = match scope.saved_version(receipt.as_ref()).ok().flatten() {
        Some(PlaylistCache::Unchanged) => previous,
        Some(PlaylistCache::Changed(saved)) => {
            Some(Snapshot::from_source(saved, disk_revision, key))
        }
        None => None,
    };
    let fresh = presented.as_ref().is_some_and(|snapshot| snapshot.fresh());
    if !force {
        telemetry::catalog_open(match (fresh, presented.is_some()) {
            (true, _) => CatalogAccess::Fresh,
            (false, true) => CatalogAccess::Stale,
            _ => CatalogAccess::Miss,
        });
    }
    crate::music::shared_reads::check_cancelled(cancelled).map_err(ReadError::Provider)?;
    scope.require_current().map_err(ReadError::Provider)?;
    let saved = if let Some(cached) = &presented {
        if fresh && !force {
            return Ok(Arc::clone(cached));
        }
        let _ = progress.send_blocking(LoadProgress::Cached(Arc::clone(cached)));
        scope.refresh(force, cancelled)?
    } else {
        scope.refresh_with_progress(force, cancelled, |update| {
            let _ = progress.send_blocking(LoadProgress::Page {
                snapshot: Snapshot::prepare(update.playlist.as_ref().clone(), revision, key),
                offset: update.offset,
                total: update.total,
            });
        })?
    };
    if let Some(snapshot) =
        presented.filter(|snapshot| &snapshot.playlist == saved.playlist.as_ref())
    {
        snapshot.mark_saved(saved.receipt);
        Ok(snapshot)
    } else {
        Ok(Snapshot::from_source(saved, revision, key))
    }
}

fn retain_playlist(error: &ReadError) -> bool {
    error.can_show_cached()
        || matches!(
            error,
            ReadError::Provider(ProviderError {
                kind: ErrorKind::InvalidResponse,
                ..
            })
        )
}

fn playlist_failure(key: &PlaylistKey, error: &ReadError) -> SourceFailure {
    let needs_sign_in = error.needs_sign_in();
    let message = if needs_sign_in && matches!(key, PlaylistKey::Source { profile: None, .. }) {
        "Для этого плейлиста нужно подключить аккаунт площадки.".into()
    } else {
        error.to_string()
    };
    SourceFailure {
        message,
        needs_sign_in,
    }
}
