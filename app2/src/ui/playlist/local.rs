use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::{Context, Div, IntoElement, Role, div, prelude::*, px, rgb};
use guise::{Button, Icon, IconName, Variant};

use crate::{
    storage::ImportProgress,
    ui::{
        components::{Activate, button},
        design_system::controls::{ControlState, ControlVariant},
        theme::PRIMARY,
    },
};

use super::{
    Content, MainView, OpenPlaylist, PlaylistUi, actions::Edit, labels::status_label,
    popup::MenuKind,
};
use crate::{
    playlist::{PlaylistKey, library::LocalChange},
    storage::{LocalPlaylistSummary, playlist_name},
};

pub(super) enum PendingEdit {
    Create {
        name: String,
    },
    Rename {
        id: i64,
        saved: String,
        name: String,
    },
    Delete {
        id: i64,
        summary: Option<(usize, LocalPlaylistSummary)>,
    },
    RemoveTrack {
        id: i64,
        index: usize,
        path: String,
        revision: Option<u64>,
        selected: bool,
    },
}

impl PlaylistUi {
    pub(super) fn preview_local_edit(&mut self, change: &LocalChange) -> bool {
        self.pending_local_edit = match change {
            LocalChange::Create { name, files } if files.is_empty() => playlist_name(name)
                .ok()
                .map(|name| PendingEdit::Create { name: name.into() }),
            LocalChange::Rename { id, name } => playlist_name(name).ok().and_then(|name| {
                let saved = self.local_name(*id)?.to_owned();
                self.set_local_name(*id, name);
                Some(PendingEdit::Rename {
                    id: *id,
                    saved,
                    name: name.into(),
                })
            }),
            LocalChange::Delete(id) => {
                let summary = self
                    .local
                    .iter()
                    .position(|item| item.id == *id)
                    .map(|index| (index, self.local.remove(index)));
                Some(PendingEdit::Delete { id: *id, summary })
            }
            LocalChange::RemoveTrack {
                id,
                index,
                path,
                revision,
            } => {
                let selected = self.open.as_mut().is_some_and(|open| {
                    if open.target.key == PlaylistKey::Local(*id) && open.selected == Some(*index) {
                        open.selected = None;
                        true
                    } else {
                        false
                    }
                });
                Some(PendingEdit::RemoveTrack {
                    id: *id,
                    index: *index,
                    path: path.clone(),
                    revision: *revision,
                    selected,
                })
            }
            _ => None,
        };
        matches!(
            self.pending_local_edit,
            Some(PendingEdit::Rename { .. } | PendingEdit::Delete { .. })
        )
    }

    pub(super) fn local_name(&self, id: i64) -> Option<&str> {
        self.open
            .as_ref()
            .filter(|open| open.target.key == PlaylistKey::Local(id))
            .map(|open| {
                open.playlist()
                    .map_or(open.target.name.as_str(), |playlist| playlist.name.as_str())
            })
            .or_else(|| {
                self.local
                    .iter()
                    .find(|item| item.id == id)
                    .map(|item| item.name.as_str())
            })
    }

    pub(super) fn pending_local_name(&self, key: &PlaylistKey) -> Option<&str> {
        match &self.pending_local_edit {
            Some(PendingEdit::Rename { id, name, .. }) if *key == PlaylistKey::Local(*id) => {
                Some(name)
            }
            _ => None,
        }
    }

    pub(super) fn set_local_name(&mut self, id: i64, name: &str) -> bool {
        if let Some(open) = self
            .open
            .as_mut()
            .filter(|open| open.target.key == PlaylistKey::Local(id))
        {
            open.target.name = name.to_owned();
            if let Content::Loaded(snapshot) = &mut open.content
                && snapshot.playlist.name != name
            {
                Arc::make_mut(snapshot).playlist.name = name.to_owned();
            }
        }
        if let Some(summary) = self.local.iter_mut().find(|item| item.id == id) {
            summary.name = name.to_owned();
            return true;
        }
        false
    }

    pub(super) fn projected_removed_track(&self, open: &OpenPlaylist) -> Option<usize> {
        let PendingEdit::RemoveTrack {
            id,
            index,
            path,
            revision,
            ..
        } = self.pending_local_edit.as_ref()?
        else {
            return None;
        };
        let snapshot = open.snapshot()?;
        (open.target.key == PlaylistKey::Local(*id)
            && snapshot.revision == *revision
            && snapshot
                .playlist
                .tracks
                .get(*index)
                .is_some_and(|track| &track.id == path))
        .then_some(*index)
    }

    pub(super) fn undo_local_edit_preview(&mut self, pending: PendingEdit) {
        match pending {
            PendingEdit::Rename { id, saved, .. } => {
                self.set_local_name(id, &saved);
            }
            PendingEdit::Delete {
                summary: Some((index, summary)),
                ..
            } => {
                self.local.insert(index.min(self.local.len()), summary);
            }
            PendingEdit::RemoveTrack {
                id,
                index,
                path,
                revision,
                selected: true,
            } => {
                if let Some(open) = self.open.as_mut()
                    && open.target.key == PlaylistKey::Local(id)
                    && open.selected.is_none()
                    && open.snapshot().is_some_and(|snapshot| {
                        snapshot.revision == revision
                            && snapshot
                                .playlist
                                .tracks
                                .get(index)
                                .is_some_and(|track| track.id == path)
                    })
                {
                    open.selected = Some(index);
                }
                self.project(false);
            }
            PendingEdit::RemoveTrack { .. } => self.project(false),
            _ => {}
        }
    }
}

pub(super) struct Import {
    pub(super) target: Option<i64>,
    pub(super) cancelled: Arc<AtomicBool>,
    pub(super) progress: ImportProgress,
}

impl Drop for Import {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl MainView {
    pub(super) fn cancel_playlist_import(&mut self, cx: &mut Context<Self>) {
        if let Some(import) = &self.playlist.import {
            import.cancelled.store(true, Ordering::Relaxed);
            cx.notify();
        }
    }

    pub(in crate::ui) fn local_playlist_actions(&self, cx: &Context<Self>) -> impl IntoElement {
        let ui = &self.playlist;
        let create = if let Some(material) = self.glass_controls(cx) {
            ControlVariant::TextAction
                .paint(
                    button("playlist-create", "Создать плейлист", 25)
                        .h(px(32.))
                        .px(px(8.))
                        .gap(px(6.))
                        .text_size(px(13.))
                        .child(Icon::new(IconName::Plus).pixel_size(px(16.)))
                        .child("Создать"),
                    material,
                    ControlState {
                        disabled: ui.local_busy,
                        ..ControlState::default()
                    },
                )
                .when(!ui.local_busy, |control| {
                    control.on_activate(cx.listener(|view, _, window, cx| {
                        view.edit_local_playlist(Edit::Create, window, cx);
                    }))
                })
                .into_any_element()
        } else {
            Button::new("playlist-create", "Создать плейлист")
                .variant(Variant::Default)
                .disabled(ui.local_busy)
                .tab_order(25)
                .on_click(cx.listener(|view, _, window, cx| {
                    view.edit_local_playlist(Edit::Create, window, cx)
                }))
                .into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .text_color(rgb(PRIMARY))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(create)
                    .child(self.playlist_menu_trigger(
                        "playlist-import",
                        "Импорт",
                        IconName::Import,
                        26,
                        MenuKind::Import,
                        cx,
                    )),
            )
            .children(self.playlist_operation_status(None, cx))
            .children(self.local_editor(cx))
            .children(
                ui.notice
                    .as_ref()
                    .map(|notice| status_label("playlist-library-notice", notice.clone())),
            )
    }

    pub(super) fn playlist_operation_status(
        &self,
        target: Option<i64>,
        cx: &Context<Self>,
    ) -> Option<Div> {
        self.playlist
            .import
            .as_ref()
            .filter(|import| import.target == target)
            .map(|import| self.import_status(import, cx))
            .or_else(|| {
                if self.playlist.edit.is_some() {
                    return None;
                }
                let label = match self.playlist.pending_local_edit.as_ref()? {
                    PendingEdit::Create { name } if target.is_none() => {
                        format!("Создаём «{name}»…")
                    }
                    PendingEdit::Rename { id, name, .. }
                        if target.is_none_or(|target| target == *id) =>
                    {
                        format!("Сохраняем «{name}»…")
                    }
                    PendingEdit::Delete { id, .. } if target.is_none_or(|target| target == *id) => {
                        "Удаляем плейлист…".into()
                    }
                    PendingEdit::RemoveTrack { id, .. } if target == Some(*id) => {
                        "Удаляем трек…".into()
                    }
                    _ => return None,
                };
                Some(div().child(status_label("playlist-local-pending", label)))
            })
    }

    fn import_status(&self, import: &Import, cx: &Context<Self>) -> Div {
        let (label, titles) = match &import.progress {
            ImportProgress::Discovering { found: 0 } => ("Ищем аудиофайлы…".to_owned(), None),
            ImportProgress::Discovering { found } => (format!("Найдено файлов: {found}"), None),
            ImportProgress::Reading {
                processed,
                reused,
                unreadable,
                titles,
            } => {
                let mut label = format!("Обработано файлов: {processed}, из кэша: {reused}");
                if *unreadable != 0 {
                    label.push_str(&format!(", не удалось прочитать теги: {unreadable}"));
                }
                (label, Some(titles.join(" · ")))
            }
            ImportProgress::Appending { count, titles } => (
                format!("Добавляем треки · {count}…"),
                Some(titles.join(" · ")),
            ),
        };
        let cancelled = import.cancelled.load(Ordering::Relaxed);
        let appending = matches!(&import.progress, ImportProgress::Appending { .. });
        let mut preview = titles.filter(|titles| !titles.is_empty()).map(|titles| {
            div()
                .id("playlist-import-preview")
                .accessibility_id("playlist-import-preview")
                .role(Role::Label)
                .aria_label(titles.clone())
                .tooltip(guise::tooltip(titles.clone()))
                .min_w_0()
                .truncate()
                .child(titles)
        });
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.))
                    .child(status_label("playlist-import-progress", label))
                    .children(
                        preview
                            .take_if(|_| appending)
                            .map(|preview| preview.flex_1().max_w(px(420.))),
                    )
                    .child(
                        Button::new(
                            "playlist-cancel-import",
                            if cancelled {
                                "Отменяем…"
                            } else {
                                "Отменить импорт"
                            },
                        )
                        .tab_order(29)
                        .disabled(cancelled)
                        .on_click(cx.listener(|view, _, _, cx| view.cancel_playlist_import(cx))),
                    ),
            )
            .children(preview)
    }

    pub(super) fn local_editor(&self, cx: &Context<Self>) -> Option<Div> {
        let ui = &self.playlist;
        let edit = ui.edit?;
        let label = match edit {
            Edit::Create => "Создать",
            Edit::Rename(_) => "Сохранить название",
            Edit::Delete(_) => "Удалить плейлист",
        };
        Some(
            div()
                .pt(px(5.))
                .w_full()
                .flex()
                .flex_col()
                .items_start()
                .gap(px(8.))
                .when(matches!(edit, Edit::Delete(_)), |editor| {
                    editor.child("Удалить из локальной медиатеки?")
                })
                .when(!matches!(edit, Edit::Delete(_)), |editor| {
                    editor.child(div().w_full().max_w(px(720.)).child(ui.editor.clone()))
                })
                .child(
                    div()
                        .w_full()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            Button::new(
                                "playlist-confirm-edit",
                                if ui.local_busy {
                                    match edit {
                                        Edit::Create => "Создаём…",
                                        Edit::Rename(_) => "Сохраняем…",
                                        Edit::Delete(_) => "Удаляем…",
                                    }
                                } else {
                                    label
                                },
                            )
                            .variant(Variant::Light)
                            .when_some(self.glass_controls(cx), |button, material| {
                                ControlVariant::DialogAction.dialog_button(button, material)
                            })
                            .disabled(ui.local_busy)
                            .tab_order(27)
                            .on_click(cx.listener(|view, _, _, cx| view.submit_local_change(cx))),
                        )
                        .child(
                            Button::new("playlist-cancel-edit", "Отмена")
                                .variant(Variant::Default)
                                .disabled(ui.local_busy)
                                .track_focus(&ui.edit_cancel_focus)
                                .tab_order(28)
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.dismiss_local_editor(window, cx);
                                })),
                        ),
                ),
        )
    }
}
