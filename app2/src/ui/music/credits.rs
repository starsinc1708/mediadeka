use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::{Context, Div, Role, Task, div, prelude::*, px};

use crate::{
    music::{
        ObjectKind,
        lyrics::{
            Recording,
            genius::{self, Credits},
        },
    },
    playlist::PlaylistTrack,
    storage::Store,
};

use super::{MainView, Route, detail_style::Ink};

#[cfg(all(test, windows, feature = "capture"))]
mod tests;

#[derive(Default)]
pub(super) struct TrackCredits(Vec<Read>);

struct Read {
    recording: Recording,
    job: Option<Job>,
    credits: Option<Credits>,
}

struct Job {
    cancelled: Arc<AtomicBool>,
    _task: Task<()>,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

impl MainView {
    pub(in crate::ui) fn refresh_genius_credits(&mut self, cx: &mut Context<Self>) {
        let player = self.player.read(cx);
        let current = player
            .current_sound_source()
            .and_then(|_| player.current_entry())
            .map(Recording::from_entry);
        let on_page = match self.workspace.routes.current() {
            Route::LocalTrack(target) => Some(&target.track),
            Route::Object(target) if target.object.kind == ObjectKind::Track => self
                .music
                .object
                .as_ref()
                .filter(|detail| detail.target == *target)
                .and_then(|detail| detail.data.as_ref())
                .and_then(|data| data.page.tracks.first()),
            _ => None,
        }
        .map(Recording::from_track);
        let wanted = [current, on_page];
        self.music.credits.0.retain(|read| {
            wanted
                .iter()
                .flatten()
                .any(|wanted| *wanted == read.recording)
        });
        for recording in wanted.into_iter().flatten() {
            if self
                .music
                .credits
                .0
                .iter()
                .any(|read| read.recording == recording)
            {
                continue;
            }
            let cancelled = Arc::new(AtomicBool::new(false));
            let flag = Arc::clone(&cancelled);
            let requested = recording.clone();
            let task = cx.spawn(async move |view, cx| {
                let check = Arc::clone(&flag);
                let fetched = cx
                    .background_executor()
                    .spawn(async move {
                        let store = Store::open_default().map_err(|error| error.to_string())?;
                        genius::load_credits(&store, &requested, &check)
                    })
                    .await;
                if !flag.load(Ordering::Acquire) {
                    let _ = view.update(cx, |view, cx| {
                        if let Some(read) = view.music.credits.0.iter_mut().find(|read| {
                            read.job
                                .as_ref()
                                .is_some_and(|job| Arc::ptr_eq(&job.cancelled, &flag))
                        }) {
                            read.job = None;
                            read.credits = fetched.ok().flatten();
                            cx.notify();
                        }
                    });
                }
            });
            self.music.credits.0.push(Read {
                recording,
                job: Some(Job {
                    cancelled,
                    _task: task,
                }),
                credits: None,
            });
        }
    }

    pub(in crate::ui) fn reset_genius_credits(&mut self, cx: &mut Context<Self>) {
        self.music.credits.0.clear();
        self.refresh_genius_credits(cx);
        cx.notify();
    }

    pub(super) fn track_credits_block(
        &self,
        track: &PlaylistTrack,
        cx: &Context<Self>,
    ) -> Option<Div> {
        let recording = Recording::from_track(track);
        let credits = self
            .music
            .credits
            .0
            .iter()
            .find(|read| read.recording == recording)?
            .credits
            .as_ref()?;
        if credits.writers.is_empty() && credits.producers.is_empty() {
            return None;
        }
        let rows = [
            ("music-track-writers", "Авторы", &credits.writers),
            ("music-track-producers", "Продюсеры", &credits.producers),
        ]
        .into_iter()
        .filter(|(_, _, names)| !names.is_empty());
        let ink = Ink::new(self.uses_fluid_glass(), self.fluid_high_contrast(cx));
        Some(
            div().child(
                div()
                    .id("music-track-credits")
                    .accessibility_id("music-track-credits")
                    .role(Role::Group)
                    .aria_label("Кредиты Genius")
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_size(px(13.))
                    .text_color(ink.secondary)
                    .children(rows.map(|(id, role, names)| {
                        let label = format!("{role} · {}", names.join(", "));
                        div()
                            .id(id)
                            .accessibility_id(id)
                            .role(Role::Label)
                            .aria_label(label.clone())
                            .child(label)
                    }))
                    .child(div().child("Genius")),
            ),
        )
    }
}
