use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
};

use gpui::{AnyWindowHandle, Context, Task, Window};

use crate::{
    playback::{
        CollectionPlan, CollectionProgressUpdate, QueueResumePlan, QueueResumeUpdate, SoundStarted,
    },
    storage::Store,
};

use super::{MainView, Page};

enum Write {
    Track(Box<SoundStarted>),
    Collection(CollectionProgressUpdate),
    Queue(QueueResumeUpdate),
}

enum Saved {
    Track,
    Collection(Arc<CollectionPlan>),
    Queue(Arc<QueueResumePlan>),
}

#[derive(Default)]
pub(super) struct Journal {
    writes: VecDeque<Write>,
    writer: Option<Task<()>>,
    saved_plan: Weak<CollectionPlan>,
    saved_queue: Weak<QueueResumePlan>,
    close: Option<AnyWindowHandle>,
}

impl MainView {
    pub(in crate::ui) fn close_after_listening(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.history.journal.close.is_some() {
            return false;
        }
        if let Some(update) = self.player.update(cx, |bar, _| bar.collection_checkpoint()) {
            self.record_collection_listening(update, cx);
        }
        if let Some(update) = self.player.update(cx, |bar, _| bar.queue_resume_on_close()) {
            self.record_queue_resume(update, cx);
        }
        let journal = &mut self.history.journal;
        if journal.writer.is_none() {
            return true;
        }
        journal.close = Some(window.window_handle());
        false
    }

    pub(in crate::ui) fn load_history(&mut self, cx: &mut Context<Self>) {
        self.read_listening(true, cx);
    }

    fn read_listening(&mut self, tracks: bool, cx: &mut Context<Self>) {
        // Keep a requested track reload when a position-only refresh arrives.
        let tracks = tracks || self.history.loading;
        self.history.loading = tracks;
        if tracks {
            self.history.error = None;
        }
        let latest = self.history.latest_id();
        self.history.task = Some(cx.spawn(async move |view, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let store = Store::open_default()?;
                    Ok::<_, crate::storage::StoreError>((
                        tracks
                            .then(|| store.listening_history_after(latest))
                            .transpose()?,
                        store.collection_records(),
                    ))
                })
                .await;
            let _ = view.update(cx, |view, cx| {
                view.history.loading = false;
                view.history.task = None;
                match result {
                    Ok((rows, collections)) => {
                        if let Some(rows) = rows {
                            view.history.apply(rows);
                            view.history.settle();
                        }
                        match collections {
                            Ok(collections) => view.history.collections = collections,
                            Err(error) => {
                                eprintln!("Не удалось прочитать прогресс Mediadeka: {error}")
                            }
                        }
                    }
                    Err(error) => {
                        view.history.error = Some(error.to_string());
                        eprintln!("Не удалось прочитать историю Mediadeka: {error}");
                    }
                }
                cx.notify();
            });
        }));
    }

    pub(in crate::ui) fn record_listening(
        &mut self,
        started: SoundStarted,
        cx: &mut Context<Self>,
    ) {
        self.write_listening(Write::Track(Box::new(started)), cx);
    }

    pub(in crate::ui) fn record_collection_listening(
        &mut self,
        update: CollectionProgressUpdate,
        cx: &mut Context<Self>,
    ) {
        if let Some(Write::Collection(pending)) = self.history.journal.writes.back_mut()
            && Arc::ptr_eq(&pending.plan, &update.plan)
        {
            *pending = update;
        } else {
            self.write_listening(Write::Collection(update), cx);
        }
    }

    pub(in crate::ui) fn record_queue_resume(
        &mut self,
        update: QueueResumeUpdate,
        cx: &mut Context<Self>,
    ) {
        if let Some(Write::Queue(pending)) = self.history.journal.writes.back_mut() {
            *pending = update;
        } else {
            self.write_listening(Write::Queue(update), cx);
        }
    }

    fn write_listening(&mut self, write: Write, cx: &mut Context<Self>) {
        let journal = &mut self.history.journal;
        journal.writes.push_back(write);
        if journal.writer.is_some() {
            return;
        }
        // Journal writes follow playback events in order. Only the first
        // checkpoint of a queue serializes the track snapshot.
        journal.writer = Some(cx.spawn(async move |view, cx| {
            let mut recorded_tracks = false;
            let mut recorded_collection = false;
            loop {
                let operation = view.update(cx, |view, _| {
                    let journal = &mut view.history.journal;
                    let write = journal.writes.pop_front()?;
                    let replace_plan = match &write {
                        Write::Track(_) => false,
                        Write::Collection(update) => {
                            journal.saved_plan.as_ptr() != Arc::as_ptr(&update.plan)
                        }
                        Write::Queue(update) => {
                            journal.saved_queue.as_ptr() != Arc::as_ptr(&update.plan)
                        }
                    };
                    Some((write, replace_plan))
                });
                let Ok(Some((write, replace_plan))) = operation else {
                    break;
                };
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        let store = Store::open_default()?;
                        match write {
                            Write::Track(started) => {
                                store.record_sound(&started)?;
                                Ok::<_, crate::storage::StoreError>(Saved::Track)
                            }
                            Write::Collection(update) => {
                                store.save_collection_listening(&update, replace_plan)?;
                                Ok(Saved::Collection(update.plan))
                            }
                            Write::Queue(update) => {
                                store.save_queue_resume(&update, replace_plan)?;
                                Ok(Saved::Queue(update.plan))
                            }
                        }
                    })
                    .await;
                match result {
                    Ok(Saved::Collection(plan)) => {
                        recorded_collection = true;
                        let _ = view.update(cx, |view, _| {
                            view.history.journal.saved_plan = Arc::downgrade(&plan)
                        });
                    }
                    Ok(Saved::Queue(plan)) => {
                        let _ = view.update(cx, |view, _| {
                            view.history.journal.saved_queue = Arc::downgrade(&plan)
                        });
                    }
                    Ok(Saved::Track) => recorded_tracks = true,
                    Err(error) => eprintln!("Не удалось записать историю Mediadeka: {error}"),
                }
            }
            let close = view.update(cx, |view, cx| {
                view.history.journal.writer = None;
                let close = view.history.journal.close.take();
                if close.is_none()
                    && (recorded_tracks || recorded_collection)
                    && matches!(view.music_page(), Page::Home | Page::History)
                {
                    view.read_listening(recorded_tracks, cx);
                }
                close
            });
            if let Ok(Some(window)) = close {
                let _ = window.update(cx, |_, window, _| window.remove_window());
            }
        }));
    }
}
