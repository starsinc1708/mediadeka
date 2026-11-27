use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use gpui::Context;

use crate::{music::station, playlist::PlaylistKey, providers::yandex::station::StationFeedback};

use super::{Job, MainView};

struct Feedback {
    batch: String,
    track: String,
    event: Event,
}

pub(super) enum Event {
    Started,
    Ended { seconds: f64, skipped: bool },
    Like,
    Dislike { seconds: f64 },
}

pub(super) struct Worker {
    sender: async_channel::Sender<Feedback>,
    _job: Job,
}

impl Worker {
    pub(super) fn new(key: &PlaylistKey, session: &str, cx: &Context<MainView>) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = cancelled.clone();
        let (feedback, inbox) = async_channel::bounded::<Feedback>(16);
        let work_key = key.clone();
        let work_session = session.to_owned();
        let task = cx.background_executor().spawn(async move {
            while let Ok(item) = inbox.recv().await {
                if flag.load(Ordering::Relaxed) {
                    break;
                }
                let track_id = &item.track;
                let event = match item.event {
                    Event::Started => StationFeedback::TrackStarted { track_id },
                    Event::Ended {
                        seconds,
                        skipped: false,
                    } => StationFeedback::TrackFinished { track_id, seconds },
                    Event::Ended {
                        seconds,
                        skipped: true,
                    } => StationFeedback::Skip { track_id, seconds },
                    Event::Like => StationFeedback::Like { track_id },
                    Event::Dislike { seconds } => StationFeedback::Dislike { track_id, seconds },
                };
                if station::feedback(&work_key, &work_session, &item.batch, event, &flag).is_err()
                    && !flag.load(Ordering::Relaxed)
                {
                    eprintln!("Яндекс Музыка не подтвердила событие станции.");
                }
            }
        });
        Self {
            sender: feedback,
            _job: Job {
                cancelled,
                _task: task,
            },
        }
    }

    pub(super) fn send(&self, batch: String, track: String, event: Event) {
        if self
            .sender
            .try_send(Feedback {
                batch,
                track,
                event,
            })
            .is_err()
        {
            eprintln!("Очередь событий станции Яндекс Музыки занята.");
        }
    }
}
