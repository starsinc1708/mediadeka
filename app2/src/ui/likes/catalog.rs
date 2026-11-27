use std::sync::{Arc, atomic::AtomicBool};

use gpui::Context;

use crate::{music::likes, storage::Store, ui::MainView};

use super::{Destination, State, Work};

impl MainView {
    pub(super) fn search_like_tracks(&mut self, row: Destination, cx: &mut Context<Self>) {
        let (Some(provider), Some(profile)) = (row.provider, row.profile.clone()) else {
            return;
        };
        let Some(dialog) = self.likes.dialog.as_mut() else {
            return;
        };
        dialog.candidates = None;
        dialog.named_errors.remove(&row.provider);
        let subject = dialog.subject.clone();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let task = cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = Store::open_default().map_err(|error| error.to_string())?;
                    likes::candidates(&store, &subject, provider, &profile, &flag)
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                let Some(dialog) = view.likes.dialog.as_mut() else {
                    return;
                };
                dialog.search = None;
                match result {
                    Ok(candidates) => dialog.candidates = Some((row, candidates)),
                    Err(error) => {
                        dialog.named_errors.insert(row.provider, error);
                    }
                }
                cx.notify();
            });
        });
        dialog.search = Some((
            provider,
            Work {
                cancelled,
                _task: task,
            },
        ));
        cx.notify();
    }

    pub(super) fn check_unknown_like_marks(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.likes.dialog.as_mut() else {
            return;
        };
        for row in dialog.choices.iter().flatten().cloned() {
            let (
                Some(provider),
                State::Ready {
                    recording,
                    liked: None,
                },
            ) = (row.provider, &row.state)
            else {
                continue;
            };
            if dialog.status_checks.contains_key(&provider)
                || self
                    .likes
                    .writes
                    .contains_key(&(dialog.subject.identity.clone(), Some(provider)))
            {
                continue;
            }
            let recording = recording.clone();
            let cancelled = Arc::new(AtomicBool::new(false));
            let flag = cancelled.clone();
            let task = cx.spawn(async move |view, cx| {
                let checked = row.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let store = Store::open_default().map_err(|error| error.to_string())?;
                        likes::check_remote(&store, &row, &recording, &flag)
                    })
                    .await;
                let _ = view.update(cx, |view, cx| {
                    let Some(dialog) = view.likes.dialog.as_mut() else {
                        return;
                    };
                    dialog.status_checks.remove(&provider);
                    let Some(row) = dialog.choices.iter_mut().flatten().find(|row| {
                        row.provider == checked.provider && row.profile == checked.profile
                    }) else {
                        return;
                    };
                    if let State::Ready {
                        liked: liked @ None,
                        ..
                    } = &mut row.state
                    {
                        if let Ok(saved) = result {
                            *liked = Some(saved);
                            view.cache_like_status(cx);
                        }
                        cx.notify();
                    }
                });
            });
            dialog.status_checks.insert(
                provider,
                Work {
                    cancelled,
                    _task: task,
                },
            );
        }
    }
}
