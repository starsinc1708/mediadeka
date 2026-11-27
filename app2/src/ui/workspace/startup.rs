//! Restore the workspace from SQLite before starting account and feed work.

use std::time::{Duration, Instant};

use gpui::{AnyWindowHandle, Context, Task, Window};

use crate::{state::Intent, storage::Store};

use super::super::connections::ConnectionWorker;
use super::{MainView, Route};

#[cfg(all(test, windows, feature = "capture"))]
mod tests;

#[derive(Default)]
pub(super) struct Startup {
    began: Option<Instant>,
    pub(super) loading: bool,
    waiting_for_frame: bool,
    last_route: Option<Route>,
    pending_route: Option<String>,
    writer: Option<Task<()>>,
    close: Option<AnyWindowHandle>,
    first_action: Option<Duration>,
}

impl MainView {
    pub(in crate::ui) fn start_desktop(
        &mut self,
        began: Instant,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace.startup.began = Some(began);
        self.workspace.startup.loading = true;
        self.workspace.startup.waiting_for_frame = true;
        self.player.update(cx, |bar, _| bar.begin_queue_restore());
        let request = self.state.begin(Intent::Startup);
        cx.spawn_in(window, async move |view, cx| {
            let work = request.clone();
            let local = cx
                .background_executor()
                .spawn(async move {
                    let store = Store::open_default()?;
                    let preview = work.home_preview(&store);
                    let route = store
                        .workspace_route()
                        .ok()
                        .flatten()
                        .and_then(|payload| serde_json::from_str::<Route>(&payload).ok())
                        .filter(|route| match route {
                            Route::Playlist(target)
                                if let crate::playlist::PlaylistKey::Local(id) = target.key =>
                            {
                                store.local_playlist_exists(id).unwrap_or(false)
                            }
                            _ => true,
                        });
                    let queue = store.queue_resume().unwrap_or_else(|error| {
                        eprintln!("Не удалось восстановить очередь Mediadeka: {error}");
                        None
                    });
                    Ok::<_, crate::storage::StoreError>((store, preview, route, queue))
                })
                .await;
            let _ = view.update_in(cx, |view, window, cx| {
                view.workspace.startup.loading = false;
                let (store, preview, route, queue) = match local {
                    Ok(value) => value,
                    Err(error) => {
                        view.workspace.startup.waiting_for_frame = false;
                        view.player
                            .update(cx, |bar, cx| bar.finish_queue_restore(None, cx));
                        view.finish(&request, request.store_failure(error), cx);
                        return;
                    }
                };
                view.player
                    .update(cx, |bar, cx| bar.finish_queue_restore(queue, cx));
                if let Some(preview) = preview
                    && view.state.set_home_preview(&request, preview)
                {
                    if let Some(route) = route {
                        view.workspace.startup.last_route = Some(route.clone());
                        view.workspace.routes.push(route);
                    }
                    view.show_route_with(true, cx);
                }
                cx.notify();
                // GPUI runs a next-frame callback before drawing that frame.
                // The second callback runs after the local workspace was drawn.
                cx.on_next_frame(window, |_, window, cx| {
                    cx.on_next_frame(window, move |view, _, cx| {
                        view.workspace.startup.waiting_for_frame = false;
                        view.connection_worker = Some(ConnectionWorker::start(cx));
                        view.start_soloist_on_startup(cx);
                        view.sync_home_sources(cx);
                        if view.state.accepts(&request) {
                            cx.spawn(async move |view, cx| {
                                let work = request.clone();
                                let screen = cx
                                    .background_executor()
                                    .spawn(async move { work.run(&store) })
                                    .await;
                                let _ =
                                    view.update(cx, |view, cx| view.finish(&request, screen, cx));
                            })
                            .detach();
                        }
                    });
                });
            });
        })
        .detach();
        cx.notify();
    }

    pub(in crate::ui) fn waiting_for_workspace_frame(&self) -> bool {
        self.workspace.startup.waiting_for_frame
    }

    pub(super) fn note_first_workspace_action(&mut self) {
        let startup = &mut self.workspace.startup;
        if self.state.home_view().is_some()
            && let Some(began) = startup.began.take()
        {
            startup.first_action = Some(began.elapsed());
        }
    }

    pub(in crate::ui) fn startup_first_action(&self) -> Option<Duration> {
        self.workspace.startup.first_action
    }

    pub(super) fn persist_workspace_route(&mut self, cx: &mut Context<Self>) {
        let startup = &mut self.workspace.startup;
        if (startup.began.is_none() && startup.first_action.is_none())
            || self.state.home_view().is_none()
        {
            return;
        }
        let route = self.workspace.routes.current();
        if startup.last_route.as_ref() == Some(route) {
            return;
        }
        let Ok(payload) = serde_json::to_string(route) else {
            return;
        };
        startup.last_route = Some(route.clone());
        startup.pending_route = Some(payload);
        if startup.writer.is_some() {
            return;
        }
        startup.writer = Some(cx.spawn(async move |view, cx| {
            while let Ok(Some(payload)) =
                view.update(cx, |view, _| view.workspace.startup.pending_route.take())
            {
                let result = cx
                    .background_executor()
                    .spawn(async move { Store::open_default()?.save_workspace_route(&payload) })
                    .await;
                if let Err(error) = result {
                    eprintln!("Не удалось сохранить страницу Mediadeka: {error}");
                    let _ = view.update(cx, |view, _| {
                        if view.workspace.startup.pending_route.is_none() {
                            view.workspace.startup.last_route = None;
                        }
                    });
                }
            }
            let close = view.update(cx, |view, _| {
                view.workspace.startup.writer = None;
                view.workspace.startup.close.take()
            });
            if let Ok(Some(window)) = close {
                let _ = window.update(cx, |root, window, cx| {
                    if let Ok(root) = root.downcast::<MainView>() {
                        root.update(cx, |view, cx| {
                            if view.close_after_listening(window, cx) {
                                window.remove_window();
                            }
                        });
                    }
                });
            }
        }));
    }

    pub(in crate::ui) fn close_after_saved_route(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.workspace.startup.writer.is_some() {
            self.workspace.startup.close = Some(window.window_handle());
            return false;
        }
        self.close_after_listening(window, cx)
    }
}
