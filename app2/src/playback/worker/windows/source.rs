use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use crate::{
    playback::{QueueEntry, SoundSource},
    storage::Store,
};

use super::{
    Driver, Failure, Message, MpvFile, Resolved, Status,
    resolve::{self, Candidate, Input, Resolve, Resolver, Ticket},
};

const LOAD_TIMEOUT: Duration = Duration::from_secs(35);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(75);

pub(super) struct Pending {
    entry: QueueEntry,
    started_at: Instant,
    selected: Option<(Candidate, SoundSource)>,
    rejected: Vec<(Candidate, String)>,
    setup_failed: bool,
    prefetched: Option<Ticket>,
    downloading: bool,
}

impl Driver {
    pub(super) fn open(
        &mut self,
        entry: QueueEntry,
        position: f64,
        started_at: Instant,
        sender: &mpsc::Sender<Message>,
    ) -> Result<(), String> {
        self.frame.completed = false;
        self.frame.status = Status::Loading;
        self.frame.duration = None;
        self.frame.position = position;
        self.frame.seekable = false;
        self.frame.output = None;
        self.frame.source = None;
        self.frame.sound = Default::default();
        self.frame.soloist_setup_required = false;
        self.stop()?;
        self.opening = Some(Instant::now());
        let local = resolve::local_file(&entry);
        let prefetched = self
            .prefetch
            .as_mut()
            .and_then(|prefetch| prefetch.take(&entry));
        self.pending = Some(Pending {
            entry,
            started_at,
            selected: None,
            rejected: Vec::new(),
            setup_failed: false,
            prefetched,
            downloading: false,
        });
        if let Some(local) = local? {
            return self.load(local);
        }
        if self.resolver.is_none() {
            self.resolver = Some(Resolver::spawn(sender.clone())?);
        }
        self.resolve()
    }

    fn resolve(&mut self) -> Result<(), String> {
        let pending = self
            .pending
            .as_mut()
            .ok_or("Не выбрана запись для воспроизведения.")?;
        let resolver = self
            .resolver
            .as_ref()
            .ok_or("Выбор источника звука недоступен.")?;
        let cancelled = Arc::new(AtomicBool::new(false));
        resolver.request(Resolve {
            generation: self.frame.generation,
            entry: pending.entry.clone(),
            rejected: pending.rejected.clone(),
            setup_failed: pending.setup_failed,
            prefetched: pending.prefetched.take(),
            cancelled: Arc::clone(&cancelled),
        })?;
        self.cancelled = Some(cancelled);
        self.opening = Some(Instant::now());
        Ok(())
    }

    pub(super) fn load(&mut self, input: Resolved) -> Result<(), String> {
        let volume = Self::percent(self.frame.volume);
        #[cfg(test)]
        let failed_stream = self
            .direct_failure_url
            .as_deref()
            .filter(|_| matches!(input.source, SoundSource::Stream(_)))
            .map(str::to_owned);
        if let Some(pending) = &mut self.pending {
            pending.selected = Some((input.candidate, input.source));
            pending.downloading = false;
        }
        self.frame.sound.reason = Some(input.reason);
        self.opening = Some(Instant::now());
        let lease = input.lease;
        match input.input {
            Input::Mpv(input) => {
                #[cfg(test)]
                let input = failed_stream.unwrap_or(input);
                let position = self.frame.position;
                let mpv = self.mpv()?;
                mpv.set(c"volume", &volume.to_string())?;
                let id = mpv.load(&input, position)?;
                self.file = Some(MpvFile {
                    id,
                    started: false,
                    loaded: false,
                    paused: false,
                    buffering: false,
                    _lease: lease,
                });
            }
            Input::Soloist(audio) => {
                self.signals.soloist.store(true, Ordering::SeqCst);
                let result = if self.signals.closing.load(Ordering::SeqCst) {
                    Err("Воспроизведение завершено при закрытии Mediadeka.".into())
                } else {
                    audio.start(volume, self.frame.position, false)
                };
                if result.is_err() {
                    self.signals.soloist.store(false, Ordering::SeqCst);
                }
                self.soloist = Some(result?);
            }
        }
        Ok(())
    }

    pub(super) fn start_failed(&mut self, error: Failure) {
        if self.frame.sound.time_to_sound_ms.is_none()
            && let Some(pending) = &mut self.pending
            && let Some((candidate, source)) = pending.selected.take()
            && candidate != Candidate::File
        {
            let setup_required = matches!(error, Failure::SoloistSetup(_));
            pending.setup_failed |= setup_required;
            let reason = format!("{}: {}", source.label(), error.into_message());
            pending.rejected.push((candidate, reason.clone()));
            match self.stop_input().and_then(|_| self.resolve()) {
                Ok(()) => {
                    self.frame.status = Status::Loading;
                    self.frame.sound.reason = Some(reason);
                }
                Err(error) if setup_required => self.fail(Failure::SoloistSetup(error)),
                Err(error) => self.fail(error.into()),
            }
            return;
        }
        self.fail(error);
    }

    pub(super) fn download_started(&mut self) {
        if let Some(pending) = &mut self.pending {
            pending.downloading = true;
            self.opening = Some(Instant::now());
            self.frame.sound.reason = Some("Поиск и проверка оригиналов".into());
        }
    }

    pub(super) fn opening_expired(&self) -> bool {
        let timeout = if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.downloading)
        {
            DOWNLOAD_TIMEOUT
        } else {
            LOAD_TIMEOUT
        };
        self.opening.is_some_and(|start| start.elapsed() > timeout)
    }

    pub(super) fn mark_started(&mut self) {
        if self.frame.status == Status::Playing
            && let Some(pending) = self.pending.take()
            && let Some((candidate, source)) = pending.selected
        {
            self.frame.source = Some(source);
            self.frame.sound.time_to_sound_ms = Some(
                pending
                    .started_at
                    .elapsed()
                    .as_millis()
                    .try_into()
                    .unwrap_or(u64::MAX),
            );
            if let Candidate::Cached(path) = candidate
                && let Err(error) =
                    Store::open_default().and_then(|store| store.touch_cached_audio(&path))
            {
                eprintln!("Не удалось обновить время использования аудиокэша: {error}");
            }
        }
    }
}
