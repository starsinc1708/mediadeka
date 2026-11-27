#[cfg(test)]
mod tests;
mod transfer;

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, Instant},
};

use async_channel::Sender;
use tempfile::TempPath;

use crate::{
    playback::{AudioQuality, QueueEntry, SoundSource, quality::validate},
    providers::{ProviderId, downloads},
    storage::Store,
};

use super::{Candidate, Failure, Input, Message, Resolved};

const IMPROVEMENT_WINDOW: Duration = Duration::from_secs(12);
const CANCEL_INTERVAL: Duration = Duration::from_millis(80);

pub(super) struct Plan {
    pub store: Store,
    pub entry: QueueEntry,
    pub generation: u64,
    pub cancelled: Arc<AtomicBool>,
    pub candidates: Vec<Candidate>,
    pub notes: Vec<String>,
    pub setup_failure: bool,
}

pub(super) struct Downloader(Sender<Plan>);

impl Downloader {
    pub(super) fn spawn(sender: mpsc::Sender<Message>) -> Result<Self, String> {
        let (requests, receiver) = async_channel::bounded::<Plan>(1);
        thread::Builder::new()
            .name("mediadeka-audio-downloads".into())
            .spawn(move || {
                while let Ok(plan) = receiver.recv_blocking() {
                    plan.run(&sender, downloads::original);
                }
            })
            .map_err(|_| "Не удалось запустить загрузку звука.")?;
        Ok(Self(requests))
    }

    pub(super) fn request(&self, plan: Plan) -> Result<(), Failure> {
        self.0
            .force_send(plan)
            .map(|_| ())
            .map_err(|_| "Загрузки звука завершили работу.".into())
    }
}

struct Verified {
    path: Arc<TempPath>,
    quality: AudioQuality,
    declared: Vec<String>,
}

impl Plan {
    fn run(
        mut self,
        sender: &mpsc::Sender<Message>,
        original: impl Fn(
            &Store,
            ProviderId,
            &str,
            &str,
            &AtomicBool,
        ) -> Result<downloads::DownloadCandidate, String>
        + Sync,
    ) {
        if self.cancelled.load(Ordering::Acquire) {
            return;
        }
        let result = self.download(sender, original);
        if let Err(error) = result
            && !self.cancelled.load(Ordering::Acquire)
        {
            self.notes.push(error);
            let reason = self.notes.join(" · ");
            let failure = if self.setup_failure {
                Failure::SoloistSetup(reason)
            } else {
                Failure::Track(reason)
            };
            let _ = sender.send(Message::Resolved {
                generation: self.generation,
                input: Err(failure),
            });
        }
    }

    fn download(
        &mut self,
        sender: &mpsc::Sender<Message>,
        original: impl Fn(
            &Store,
            ProviderId,
            &str,
            &str,
            &AtomicBool,
        ) -> Result<downloads::DownloadCandidate, String>
        + Sync,
    ) -> Result<(), String> {
        let directory = self
            .store
            .sound_partial_directory()
            .map_err(|error| error.to_string())?;
        let mut jobs = Vec::new();
        for provider in downloads::PROVIDERS {
            if !self
                .store
                .sound_download_enabled(provider)
                .map_err(|error| error.to_string())?
            {
                continue;
            }
            let candidates: Vec<_> = self
                .candidates
                .iter()
                .filter(|candidate| {
                    matches!(candidate,
                        Candidate::Provider { provider: owner, .. } if *owner == provider
                    )
                })
                .collect();
            if !candidates.is_empty() {
                jobs.push((provider, candidates));
            }
        }
        if jobs.is_empty() {
            return Err("Среди включённых источников загрузки нет подтверждённой записи.".into());
        }
        let notes = self.notes.join(" · ");
        let (played, errors) = race(
            jobs,
            self.cancelled.as_ref(),
            IMPROVEMENT_WINDOW,
            |candidates, cancelled| self.fetch(candidates, &directory, cancelled, &original),
            |provider, file| {
                let Some(path) = file.path.to_str() else {
                    return false;
                };
                let declared = if file.declared.is_empty() {
                    String::new()
                } else {
                    format!(" · Площадка сообщает {}", file.declared.join(", "))
                };
                let selection = Resolved {
                    input: Input::Mpv(path.to_owned()),
                    source: SoundSource::Download(provider),
                    candidate: Candidate::Download(provider),
                    reason: format!(
                        "Первый проверенный оригинал{declared} · {}",
                        file.quality.label()
                    ),
                    lease: Some(Arc::clone(&file.path)),
                };
                let mut selection = selection;
                if !notes.is_empty() {
                    selection.reason.push_str(&format!(" · {notes}"));
                }
                sender
                    .send(Message::Resolved {
                        generation: self.generation,
                        input: Ok(selection),
                    })
                    .is_ok()
            },
            |file| match self
                .store
                .publish_cached_audio(&self.entry, &file.path, &file.quality)
            {
                Ok(true) => {
                    let _ = sender.send(Message::CacheChanged);
                }
                Ok(false) => {}
                Err(error) => eprintln!("Не удалось сохранить проверенный оригинал: {error}"),
            },
        );
        self.notes.extend(errors);
        if played {
            Ok(())
        } else {
            Err("Загрузка не дала проверенного аудиофайла.".into())
        }
    }

    fn fetch(
        &self,
        candidates: Vec<&Candidate>,
        directory: &Path,
        cancelled: &AtomicBool,
        original: impl Fn(
            &Store,
            ProviderId,
            &str,
            &str,
            &AtomicBool,
        ) -> Result<downloads::DownloadCandidate, String>,
    ) -> Result<Verified, String> {
        let mut failure = "Источник не подтвердил исходный файл.".to_owned();
        for candidate in candidates {
            if cancelled.load(Ordering::Acquire) {
                return Err("Загрузка звука отменена.".into());
            }
            let Candidate::Provider {
                provider,
                profile,
                id,
            } = candidate
            else {
                continue;
            };
            let result = (|| {
                let original = original(&self.store, *provider, profile, id, cancelled)?;
                let mut declared: Vec<_> = original.format.iter().cloned().collect();
                if let Some(bitrate) = original.bitrate_bps {
                    declared.push(format!("{} кбит/с", bitrate / 1000));
                }
                if let Some(rate) = original.sample_rate_hz {
                    declared.push(format!("{rate} Гц"));
                }
                let identity = serde_json::to_string(&(*provider, profile, id))
                    .map_err(|error| error.to_string())?;
                let path = transfer::fetch(directory, &identity, &original, cancelled)?;
                let quality = validate(&path, self.entry.track.duration, cancelled)?;
                Ok(Verified {
                    path: Arc::new(path),
                    quality,
                    declared,
                })
            })();
            match result {
                Ok(ready) => return Ok(ready),
                Err(error) => failure = error,
            }
        }
        Err(failure)
    }
}

fn race<T: Send>(
    jobs: Vec<(ProviderId, T)>,
    cancelled: &AtomicBool,
    improvement_window: Duration,
    fetch: impl Fn(T, &AtomicBool) -> Result<Verified, String> + Sync,
    first: impl FnOnce(ProviderId, &Verified) -> bool,
    cache: impl Fn(&Verified),
) -> (bool, Vec<String>) {
    let (ready, results) = mpsc::channel();
    let stop = AtomicBool::new(false);
    thread::scope(|scope| {
        for (provider, job) in jobs {
            let ready = ready.clone();
            let (fetch, stop) = (&fetch, &stop);
            scope.spawn(move || {
                let result = fetch(job, stop);
                let _ = ready.send((provider, result));
            });
        }
        drop(ready);
        let mut start = Some(first);
        let mut deadline = None;
        let mut best_rank = None;
        let mut played = false;
        let mut errors = Vec::new();
        loop {
            if cancelled.load(Ordering::Acquire)
                || deadline.is_some_and(|end| Instant::now() >= end)
            {
                break;
            }
            match results.recv_timeout(CANCEL_INTERVAL) {
                Ok((provider, Ok(file))) => {
                    if cancelled.load(Ordering::Acquire)
                        || deadline.is_some_and(|end| Instant::now() >= end)
                    {
                        break;
                    }
                    if let Some(first) = start.take() {
                        played = first(provider, &file);
                        deadline = Some(Instant::now() + improvement_window);
                        if !played {
                            break;
                        }
                    }
                    let rank = file.quality.rank();
                    if !cancelled.load(Ordering::Acquire)
                        && best_rank.is_none_or(|best| rank > best)
                    {
                        best_rank = Some(rank);
                        cache(&file);
                    }
                }
                Ok((provider, Err(error))) => {
                    errors.push(format!("{}: {error}", provider.display_name()))
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        stop.store(true, Ordering::Release);
        (played, errors)
    })
}
