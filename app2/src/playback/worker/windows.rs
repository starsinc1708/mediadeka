mod listening;
mod resolve;
mod source;
#[cfg(test)]
mod tests;

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    time::{Duration, Instant},
};

use async_channel::Sender;

use super::super::{
    DEFAULT_AUDIO_DEVICE,
    mpv::{Event, Mpv},
    soloist::SoloistPlayback,
};
use super::{Command, Failure, Frame, Message, Signals, Status};
use listening::ListeningClock;
pub(super) use resolve::Resolved;
use resolve::{Prefetch, Resolver};
use source::Pending;

const TICK: Duration = Duration::from_millis(40);
#[derive(Default)]
struct Driver {
    frame: Frame,
    listening: ListeningClock,
    mpv: Option<Mpv>,
    opening: Option<Instant>,
    file: Option<MpvFile>,
    soloist: Option<SoloistPlayback>,
    resolver: Option<Resolver>,
    prefetch: Option<Prefetch>,
    cancelled: Option<Arc<AtomicBool>>,
    pending: Option<Pending>,
    signals: Arc<Signals>,
    #[cfg(test)]
    direct_failure_url: Option<String>,
}

struct MpvFile {
    id: i64,
    started: bool,
    loaded: bool,
    paused: bool,
    buffering: bool,
    _lease: Option<Arc<tempfile::TempPath>>,
}

pub(super) fn run(
    sender: mpsc::Sender<Message>,
    receiver: mpsc::Receiver<Message>,
    output: Sender<Frame>,
    signals: Arc<Signals>,
    volume: f64,
) {
    let driver = Driver {
        frame: Frame {
            volume,
            ..Frame::default()
        },
        signals,
        ..Driver::default()
    };
    run_driver(driver, sender, receiver, output);
}

fn run_driver(
    mut driver: Driver,
    sender: mpsc::Sender<Message>,
    receiver: mpsc::Receiver<Message>,
    output: Sender<Frame>,
) {
    let signals = Arc::clone(&driver.signals);
    let mut previous = driver.frame.clone();
    loop {
        let message =
            if driver.opening.is_some() || driver.file.is_some() || driver.soloist.is_some() {
                match receiver.recv_timeout(TICK) {
                    Ok(message) => Some(message),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match receiver.recv() {
                    Ok(message) => Some(message),
                    Err(_) => break,
                }
            };
        match message {
            Some(Message::Shutdown) => break,
            _ if signals.closing.load(Ordering::SeqCst) => break,
            Some(Message::Request {
                sequence,
                generation,
                command,
            }) => {
                let current = generation == signals.requested.load(Ordering::Acquire);
                let global = matches!(
                    &command,
                    Command::Volume(_) | Command::Devices | Command::Device(_)
                );
                if !current && !global {
                    continue;
                }
                driver.frame.sequence = sequence;
                if current {
                    driver.frame.generation = generation;
                }
                driver.handle(command, &sender);
            }
            Some(Message::Resolved { generation, input })
                if generation == signals.requested.load(Ordering::Acquire)
                    && generation == driver.frame.generation
                    && driver.opening.is_some() =>
            {
                let result = input.and_then(|input| driver.load(input).map_err(Failure::from));
                if let Err(error) = result {
                    driver.start_failed(error);
                }
            }
            Some(Message::CacheChanged) => {
                driver.frame.cache_revision = driver.frame.cache_revision.wrapping_add(1);
            }
            Some(Message::Downloading { generation })
                if generation == signals.requested.load(Ordering::Acquire)
                    && generation == driver.frame.generation =>
            {
                driver.download_started();
            }
            _ => {}
        }
        driver.poll();
        driver.listening.sample(&mut driver.frame, Instant::now());
        if driver.frame != previous {
            previous = driver.frame.clone();
            if output.force_send(previous.clone()).is_err() {
                break;
            }
        }
    }
    let _ = driver.stop();
}

impl Driver {
    fn handle(&mut self, command: Command, sender: &mpsc::Sender<Message>) {
        if !matches!(&command, Command::Prefetch { .. }) {
            self.frame.error = None;
        }
        let result = match command {
            Command::Load {
                entry,
                position,
                requested_at,
            } => {
                self.listening = ListeningClock::default();
                self.frame.listened = Duration::ZERO;
                self.frame.seek_revision = 0;
                if let Err(error) = self.open(*entry, position, requested_at, sender) {
                    self.fail(error.into());
                }
                return;
            }
            Command::Prefetch { entries, near_end } => {
                if self.prefetch.is_none() && !entries.is_empty() {
                    self.prefetch = Prefetch::spawn().ok();
                }
                if let Some(prefetch) = &mut self.prefetch {
                    prefetch.replace(entries, near_end);
                }
                return;
            }
            Command::Pause(paused) => self.pause(paused),
            Command::Seek(seconds) => self.seek(seconds),
            Command::Volume(volume) => self.volume(volume),
            Command::Devices => self.devices(),
            Command::Device(device) => self.select_device(&device),
        };
        if let Err(error) = result {
            self.frame.error = Some(error);
        }
    }

    fn mpv(&mut self) -> Result<&Mpv, String> {
        if self.mpv.is_none() {
            let mpv = Mpv::open()?;
            mpv.set(c"volume", &Self::percent(self.frame.volume).to_string())?;
            self.mpv = Some(mpv);
        }
        self.mpv.as_ref().ok_or_else(|| "libmpv недоступен.".into())
    }

    fn stop(&mut self) -> Result<(), String> {
        self.pending = None;
        self.stop_input()
    }

    fn stop_input(&mut self) -> Result<(), String> {
        self.opening = None;
        if let Some(cancelled) = self.cancelled.take() {
            cancelled.store(true, Ordering::Release);
        }
        if let Some(soloist) = self.soloist.take() {
            let result = soloist.stop_and_wait();
            self.signals.soloist.store(false, Ordering::SeqCst);
            result?;
        }
        if let Some(file) = self.file.take()
            && let Some(mpv) = &self.mpv
        {
            mpv.command(&["stop"])?;
            drop(file);
        }
        Ok(())
    }

    fn fail(&mut self, error: Failure) {
        let shutdown = self.stop().err();
        self.frame.soloist_setup_required = matches!(error, Failure::SoloistSetup(_));
        let error = error.into_message();
        self.frame.sound.reason = Some(error.clone());
        self.frame.status = Status::Failed(format!("Не удалось воспроизвести запись. {error}"));
        if let Some(shutdown) = shutdown.filter(|shutdown| shutdown != &error) {
            self.frame.error = Some(shutdown);
        }
        self.frame.seekable = false;
        self.frame.output = None;
        self.frame.source = None;
        self.frame.sound.quality = None;
    }

    fn finish(&mut self) {
        self.frame.status = Status::Idle;
        self.listening.sample(&mut self.frame, Instant::now());
        if let Err(error) = self.stop() {
            self.fail(error.into());
            return;
        }
        self.frame.position = self.frame.duration.unwrap_or(self.frame.position);
        self.frame.completed = true;
        self.frame.seekable = false;
        self.frame.output = None;
    }

    fn poll(&mut self) {
        while let Some(event) = self.mpv.as_ref().and_then(Mpv::next_event) {
            let result = self.event(event);
            if let Err(error) = result {
                self.start_failed(error.into());
            }
        }
        if let Some(soloist) = &mut self.soloist {
            match soloist.poll() {
                Ok(Some(state)) => {
                    self.listening
                        .soloist_position(state.seek_sequence, Instant::now());
                    self.opening = None;
                    self.frame.duration = Some(state.duration);
                    self.frame.position = state.position;
                    self.frame.volume = f64::from(state.volume) / 100.;
                    self.frame.seekable = true;
                    self.frame.output = Some("soloist".into());
                    if state.at_end {
                        self.finish();
                    } else {
                        self.update_status(state.paused, state.buffering);
                    }
                }
                Ok(None) => {}
                Err(error) => self.start_failed(error),
            }
        }
        if self.opening_expired() {
            self.start_failed("Источник не подтвердил запуск аудио вовремя.".into());
        }
    }

    fn event(&mut self, event: Event) -> Result<(), String> {
        match event {
            Event::Volume(value) if self.soloist.is_none() => {
                self.frame.volume = (value / 100.).clamp(0., 1.);
            }
            Event::Devices => {
                let devices = self.mpv.as_ref().and_then(|mpv| mpv.devices().ok());
                if let Some(devices) = devices {
                    self.frame.devices = devices;
                }
            }
            Event::Device => {
                if let Some(device) = self
                    .mpv
                    .as_ref()
                    .and_then(|mpv| mpv.string(c"audio-device"))
                {
                    self.frame.device = device;
                }
            }
            Event::UnexpectedStop => {
                self.mpv = None;
                self.frame.device = DEFAULT_AUDIO_DEVICE.into();
                self.frame.devices.clear();
                if self.file.take().is_some() {
                    return Err("Движок libmpv завершился во время воспроизведения.".into());
                }
            }
            Event::Overflow => {
                return Err("libmpv потерял состояние воспроизведения.".into());
            }
            _ => self.file_event(event)?,
        }
        Ok(())
    }

    fn file_event(&mut self, event: Event) -> Result<(), String> {
        let Some(file) = &mut self.file else {
            return Ok(());
        };
        match event {
            Event::Start(id) => file.started = id == file.id,
            Event::End { id, result } if id == file.id => {
                result?;
                if self.opening.is_some() {
                    return Err("Источник завершился до начала аудио.".into());
                }
                self.finish();
            }
            Event::Loaded if file.started => file.loaded = true,
            Event::Restarted if file.loaded && self.opening.is_some() => {
                let mpv = self.mpv.as_ref().ok_or("libmpv недоступен.")?;
                let output = mpv
                    .string(c"current-ao")
                    .filter(|driver| driver != "null")
                    .ok_or("Нет активного устройства вывода аудио.")?;
                self.frame.output = Some(output);
                self.frame.sound.quality = mpv.audio_quality();
                // A replacement can leave the observed properties unchanged.
                // Read initial values explicitly after the new entry has started.
                self.frame.duration = mpv.double(c"duration").filter(|value| *value > 0.);
                self.frame.seekable = mpv.flag(c"seekable").unwrap_or(false);
                self.frame.position = mpv.double(c"time-pos").unwrap_or_default().max(0.);
                file.paused = mpv.flag(c"pause").unwrap_or(false);
                file.buffering = mpv.flag(c"paused-for-cache").unwrap_or(false);
                #[cfg(test)]
                {
                    self.frame.test_audio_format = mpv
                        .string(c"audio-codec-name")
                        .zip(mpv.double(c"audio-params/samplerate"))
                        .zip(mpv.double(c"audio-out-params/samplerate"))
                        .map(|((codec, decoded), output)| (codec, decoded, output));
                }
                self.opening = None;
                let (paused, buffering) = (file.paused, file.buffering);
                self.update_status(paused, buffering);
            }
            Event::Seek if file.loaded && self.opening.is_none() => {
                self.listening.mpv_seeking();
            }
            Event::Restarted if file.loaded && self.listening.mpv_seek_pending() => {
                if let Some(position) = self.mpv.as_ref().and_then(|mpv| mpv.double(c"time-pos")) {
                    self.frame.position = position.max(0.);
                }
                self.listening.landed();
            }
            Event::AudioChanged if file.loaded && self.opening.is_none() => {
                let output = self.mpv.as_ref().and_then(|mpv| mpv.string(c"current-ao"));
                if output.as_deref().is_some_and(|driver| driver != "null") {
                    self.frame.output = output;
                }
                if let Some(quality) = self.mpv.as_ref().and_then(Mpv::audio_quality) {
                    self.frame.sound.quality = Some(quality);
                }
            }
            Event::Position(position) if file.started => self.frame.position = position.max(0.),
            Event::Duration(duration) if file.started => {
                self.frame.duration = duration.filter(|value| *value > 0.)
            }
            Event::Seekable(seekable) if file.started => self.frame.seekable = seekable,
            Event::Paused(paused) if file.started => {
                file.paused = paused;
                let buffering = file.buffering;
                if paused
                    && let Some(position) =
                        self.mpv.as_ref().and_then(|mpv| mpv.double(c"time-pos"))
                {
                    self.frame.position = position.max(0.);
                }
                self.update_status(paused, buffering);
            }
            Event::Buffering(buffering) if file.started => {
                file.buffering = buffering;
                let paused = file.paused;
                self.update_status(paused, buffering);
            }
            _ => {}
        }
        Ok(())
    }

    fn update_status(&mut self, paused: bool, buffering: bool) {
        self.frame.status = if self.opening.is_some() || buffering {
            Status::Loading
        } else if paused {
            Status::Paused
        } else {
            Status::Playing
        };
        self.mark_started();
    }

    fn pause(&mut self, paused: bool) -> Result<(), String> {
        if let Some(soloist) = &self.soloist {
            soloist.set_paused(paused)
        } else if self.file.is_some() {
            self.mpv()?.set(c"pause", if paused { "yes" } else { "no" })
        } else {
            Ok(())
        }
    }

    fn seek(&mut self, seconds: f64) -> Result<(), String> {
        if let Some(soloist) = &self.soloist {
            soloist.seek(seconds, self.frame.sequence)?;
            self.listening
                .soloist_seek(self.frame.sequence, Instant::now());
        } else if self.file.is_some() {
            self.mpv()?
                .command(&["seek", &seconds.to_string(), "absolute+exact"])?;
            self.listening.mpv_seeking();
        }
        Ok(())
    }

    fn volume(&mut self, volume: f64) -> Result<(), String> {
        let volume = Self::percent(volume);
        if let Some(soloist) = &self.soloist {
            soloist.set_volume(volume)
        } else {
            let mpv = self.mpv()?;
            mpv.set(c"volume", &volume.to_string())?;
            let confirmed = mpv
                .double(c"volume")
                .ok_or("libmpv не подтвердил громкость.")?;
            self.frame.volume = (confirmed / 100.).clamp(0., 1.);
            Ok(())
        }
    }

    fn devices(&mut self) -> Result<(), String> {
        self.frame.devices = self.mpv()?.devices()?;
        Ok(())
    }

    fn select_device(&mut self, id: &str) -> Result<(), String> {
        if self.soloist.is_some() {
            return Err("Выход Spotify Soloist выбирается средствами Windows.".into());
        }
        let mpv = self.mpv()?;
        if !mpv.devices()?.iter().any(|device| device.id == id) {
            return Err("Выбранное устройство вывода недоступно.".into());
        }
        mpv.set(c"audio-device", id)?;
        self.frame.device = mpv
            .string(c"audio-device")
            .ok_or("libmpv не подтвердил устройство вывода.")?;
        Ok(())
    }

    fn percent(volume: f64) -> u32 {
        (volume * 100.).round() as u32
    }
}
