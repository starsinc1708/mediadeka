mod profile;
mod wire;

pub(crate) use profile::probe_managed_soloist;

use std::{
    net::SocketAddr,
    path::PathBuf,
    sync::{
        Arc, Mutex, TryLockError,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use librespot_core::SpotifyUri;
use serde_json::json;

use crate::soloist::pairing::check_paired_account;
use profile::{SIGN_IN_REQUIRED, auth_status};
use wire::{Client, Status, WireState};

use super::worker::Failure;

const POLL_INTERVAL: Duration = Duration::from_millis(50);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(650);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);
const START_TIMEOUT: Duration = Duration::from_secs(9);
const DURATION_TOLERANCE_MS: u64 = 2_000;
const START_POSITION_TOLERANCE: f64 = 2.0;
const SEEK_CONFIRMATION_TOLERANCE_MS: u64 = 50;

const DISCONNECTED: &str = "Подключите локальный Spotify Soloist.";
const WIRE_ERROR: &str = "Связь с локальным Spotify Soloist прервана.";
const COMMAND_ERROR: &str = "Spotify Soloist отклонил команду управления.";
const UNAVAILABLE: &str = "Spotify Soloist не подтвердил воспроизведение полной записи.";

static PLAYING: Mutex<()> = Mutex::new(());

#[derive(Clone)]
pub(super) struct SoloistAudio {
    endpoint: SocketAddr,
    profile: PathBuf,
    account: String,
    uri: String,
    expected_duration: Option<u64>,
}

#[derive(Clone, Copy)]
pub(super) enum Preparation {
    Playback,
    Prefetch,
}

#[derive(Clone, Copy)]
pub(super) struct SoloistState {
    pub position: f64,
    pub duration: f64,
    pub paused: bool,
    pub buffering: bool,
    pub at_end: bool,
    pub seek_sequence: Option<u64>,
    pub volume: u32,
}

type Update = Arc<Mutex<Option<Result<SoloistState, Failure>>>>;

pub(super) struct SoloistPlayback {
    commands: SyncSender<Control>,
    stopped: Arc<AtomicBool>,
    update: Update,
    worker: Option<JoinHandle<Result<(), Failure>>>,
}

#[derive(Clone, Copy)]
enum Control {
    Paused(bool),
    Seek(f64, u64),
    Volume(u32),
}

impl SoloistAudio {
    pub(super) fn prepare(
        uri: SpotifyUri,
        expected_duration: Option<u64>,
        account: String,
        cancelled: &AtomicBool,
        preparation: Preparation,
    ) -> Result<Self, Failure> {
        if !matches!(uri, SpotifyUri::Track { .. }) {
            return Err("Spotify Soloist требуется идентификатор трека Spotify.".into());
        }
        let (endpoint, profile) = profile::prepare(&account, cancelled, preparation)?;
        Ok(Self {
            endpoint,
            profile,
            account,
            uri: uri.to_uri(),
            expected_duration,
        })
    }

    pub(super) fn start(
        &self,
        volume: u32,
        position: f64,
        paused: bool,
    ) -> Result<SoloistPlayback, String> {
        let (commands, receiver) = mpsc::sync_channel(32);
        let stopped = Arc::new(AtomicBool::new(false));
        let update = Arc::new(Mutex::new(None));
        let worker_update = update.clone();
        let worker_stopped = stopped.clone();
        let audio = self.clone();
        let worker = thread::Builder::new()
            .name("spotify-soloist".into())
            .spawn(move || {
                let cancelled = || worker_stopped.load(Ordering::Acquire);
                let result = audio.play(
                    Start {
                        volume: volume.min(100),
                        position: position.max(0.0),
                        paused,
                    },
                    &receiver,
                    &worker_update,
                    &cancelled,
                );
                if let Err(error) = &result
                    && !cancelled()
                    && let Ok(mut update) = worker_update.lock()
                {
                    *update = Some(Err(error.clone()));
                }
                result
            })
            .map_err(|_| "Не удалось запустить управление Spotify Soloist.".to_owned())?;
        Ok(SoloistPlayback {
            commands,
            stopped,
            update,
            worker: Some(worker),
        })
    }

    fn play(
        &self,
        start: Start,
        receiver: &Receiver<Control>,
        update: &Update,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), Failure> {
        let deadline = Instant::now() + COMMAND_TIMEOUT;
        let _guard = loop {
            if cancelled() {
                return Ok(());
            }
            match PLAYING.try_lock() {
                Ok(guard) => break guard,
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    thread::sleep(POLL_INTERVAL);
                }
                Err(_) => return Err(COMMAND_ERROR.into()),
            }
        };
        let mut client = Client::connect(self.endpoint).map_err(Failure::SoloistSetup)?;
        if !auth_status(&mut client, cancelled).map_err(Failure::SoloistSetup)? {
            return Err(Failure::SoloistSetup(SIGN_IN_REQUIRED.into()));
        }
        client.wait(CONNECT_TIMEOUT, cancelled, |state| {
            state.logged_in == Some(false) || state.complete
        })?;
        if client.state.logged_in != Some(true) {
            return Err(Failure::SoloistSetup(SIGN_IN_REQUIRED.into()));
        }
        check_paired_account(&self.profile, &self.account)
            .map_err(|error| Failure::SoloistSetup(error.message().into()))?;
        let previous_volume = client.state.volume.ok_or(WIRE_ERROR)?;
        let mut requested = false;
        let mut muted = false;
        let result = self
            .arm(&mut client, start, cancelled, &mut requested, &mut muted)
            .and_then(|()| self.monitor(&mut client, receiver, update, cancelled));
        let restore_volume = muted.then_some(previous_volume);
        let cleanup = self
            .release(&mut client, requested, restore_volume)
            .map_err(Failure::from);
        if cancelled() {
            cleanup
        } else {
            result.and(cleanup)
        }
    }

    fn release(
        &self,
        client: &mut Client,
        requested: bool,
        restore_volume: Option<u32>,
    ) -> Result<(), String> {
        if !requested && restore_volume.is_none() {
            return Ok(());
        }
        let release = |client: &mut Client| {
            client.refresh()?;
            client.wait(
                CONNECT_TIMEOUT,
                || false,
                |state| state.logged_in == Some(true) && state.complete,
            )?;
            if requested && client.state.uri == self.uri && client.state.status.advancing() {
                client.send("pause", json!({}))?;
                client.wait(
                    CONNECT_TIMEOUT,
                    || false,
                    |state| state.uri != self.uri || !state.status.advancing(),
                )?;
            }
            if let Some(volume) = restore_volume
                && volume != 0
                && client.state.volume == Some(0)
            {
                client.send("set_volume", json!({"volume": volume}))?;
                client.wait(
                    CONNECT_TIMEOUT,
                    || false,
                    |state| state.volume == Some(volume),
                )?;
            }
            Ok(())
        };
        match release(client) {
            Ok(()) => Ok(()),
            Err(_) => release(&mut Client::connect(self.endpoint)?),
        }
    }

    fn arm(
        &self,
        client: &mut Client,
        start: Start,
        cancelled: &dyn Fn() -> bool,
        requested: &mut bool,
        muted: &mut bool,
    ) -> Result<(), Failure> {
        if cancelled() {
            return Err(COMMAND_ERROR.into());
        }
        client.send("set_volume", json!({"volume": 0}))?;
        *muted = true;
        client.wait(COMMAND_TIMEOUT, cancelled, |state| state.volume == Some(0))?;
        if cancelled() {
            return Err(COMMAND_ERROR.into());
        }
        client.send("play", json!({"uri": self.uri}))?;
        *requested = true;
        client
            .wait(START_TIMEOUT, cancelled, |state| {
                state.logged_in == Some(false)
                    || self.matches(state)
                        && state.status == Status::Playing
                        && state.position.speed > 0.0
                        && state.position.seconds() < START_POSITION_TOLERANCE
            })
            .map_err(|_| UNAVAILABLE.to_owned())?;
        if client.state.logged_in != Some(true) {
            return Err(Failure::SoloistSetup(SIGN_IN_REQUIRED.into()));
        }

        if start.position > 0.0 {
            let target_ms = seek_ms(start.position, client.state.duration);
            client.send("seek", json!({"position_ms": target_ms}))?;
            client.wait(COMMAND_TIMEOUT, cancelled, |state| {
                self.matches(state) && state.position.milliseconds.abs_diff(target_ms) <= 500
            })?;
        }
        if start.paused {
            client.send("pause", json!({}))?;
            client.wait(COMMAND_TIMEOUT, cancelled, |state| {
                self.matches(state) && state.status == Status::Paused
            })?;
        }
        client.send("set_volume", json!({"volume": start.volume}))?;
        client.wait(COMMAND_TIMEOUT, cancelled, |state| {
            self.matches(state) && state.volume == Some(start.volume)
        })?;
        *muted = false;
        Ok(())
    }

    fn monitor(
        &self,
        client: &mut Client,
        receiver: &Receiver<Control>,
        update: &Update,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(), Failure> {
        let mut previous = self.state(&client.state)?;
        let mut pending_seek: Option<(u64, u64, u64)> = None;
        let mut seek_sequence = None;
        publish(update, previous);
        while !cancelled() {
            client.tick()?;
            if client.state.logged_in != Some(true) {
                return Err(Failure::SoloistSetup(SIGN_IN_REQUIRED.into()));
            }
            if let Ok(mut state) = self.state(&client.state) {
                if let Some((sequence, revision, target)) = pending_seek
                    && client.state.position_revision != revision
                    && client.state.position.milliseconds.abs_diff(target)
                        <= SEEK_CONFIRMATION_TOLERANCE_MS
                {
                    seek_sequence = Some(sequence);
                    pending_seek = None;
                }
                state.seek_sequence = seek_sequence;
                if client.state.status == Status::Idle {
                    if state.at_end
                        || previous.duration - previous.position < START_POSITION_TOLERANCE
                    {
                        publish(
                            update,
                            SoloistState {
                                position: previous.position,
                                paused: true,
                                at_end: true,
                                ..state
                            },
                        );
                        return Ok(());
                    }
                    return Err("Spotify Soloist остановил воспроизведение записи.".into());
                }
                previous = state;
                publish(update, state);
            } else if previous.duration - previous.position < START_POSITION_TOLERANCE {
                if client.state.status.advancing() {
                    client.send("pause", json!({}))?;
                }
                publish(
                    update,
                    SoloistState {
                        paused: true,
                        at_end: true,
                        ..previous
                    },
                );
                return Ok(());
            } else {
                return Err("Запись Spotify Soloist изменилась во время воспроизведения.".into());
            }
            for control in receiver.try_iter().take(32) {
                match control {
                    Control::Paused(paused) => {
                        client.send(if paused { "pause" } else { "play" }, json!({}))?;
                    }
                    Control::Seek(position, sequence) => {
                        let target = seek_ms(position, client.state.duration);
                        client.send("seek", json!({"position_ms": target}))?;
                        pending_seek = Some((sequence, client.state.position_revision, target));
                    }
                    Control::Volume(volume) => {
                        client.send("set_volume", json!({"volume": volume}))?
                    }
                }
            }
        }
        Ok(())
    }

    fn matches(&self, state: &WireState) -> bool {
        state.uri == self.uri
            && state.duration > 0
            && self.expected_duration.is_none_or(|expected| {
                expected == 0 || expected.abs_diff(state.duration) <= DURATION_TOLERANCE_MS
            })
    }

    fn state(&self, state: &WireState) -> Result<SoloistState, String> {
        if !self.matches(state) {
            return Err(UNAVAILABLE.into());
        }
        let duration = state.duration as f64 / 1_000.0;
        let position = if state.status == Status::Playing {
            state.position.seconds()
        } else {
            state.position.milliseconds as f64 / 1_000.0
        }
        .clamp(0.0, duration);
        Ok(SoloistState {
            position,
            duration,
            paused: state.status == Status::Paused,
            buffering: state.status == Status::Buffering,
            at_end: state.status == Status::Idle && duration - position < START_POSITION_TOLERANCE,
            seek_sequence: None,
            volume: state.volume.ok_or(WIRE_ERROR)?,
        })
    }
}

struct Start {
    volume: u32,
    position: f64,
    paused: bool,
}

impl SoloistPlayback {
    pub(super) fn poll(&mut self) -> Result<Option<SoloistState>, Failure> {
        self.update
            .lock()
            .map_err(|_| WIRE_ERROR.to_owned())?
            .take()
            .transpose()
    }

    pub(super) fn seek(&self, position: f64, sequence: u64) -> Result<(), String> {
        self.send(Control::Seek(position, sequence))
    }

    pub(super) fn set_paused(&self, paused: bool) -> Result<(), String> {
        self.send(Control::Paused(paused))
    }

    pub(super) fn set_volume(&self, volume: u32) -> Result<(), String> {
        self.send(Control::Volume(volume.min(100)))
    }

    pub(super) fn stop(&self) -> Result<(), String> {
        self.stopped.store(true, Ordering::Release);
        Ok(())
    }

    pub(super) fn stop_and_wait(mut self) -> Result<(), String> {
        self.stop()?;
        self.worker.take().map_or(Ok(()), |worker| {
            worker
                .join()
                .map_err(|_| WIRE_ERROR.to_owned())?
                .map_err(Failure::into_message)
        })
    }

    fn send(&self, command: Control) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|_| COMMAND_ERROR.into())
    }
}

impl Drop for SoloistPlayback {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn publish(update: &Update, state: SoloistState) {
    if let Ok(mut slot) = update.lock() {
        *slot = Some(Ok(state));
    }
}

fn seek_ms(position: f64, duration_ms: u64) -> u64 {
    ((position * 1000.0).round() as u64).min(duration_ms.saturating_sub(1))
}

#[cfg(test)]
#[test]
fn soloist_reports_exact_track_duration_and_volume() {
    let uri = "spotify:track:0On1SxfkvTOfzvjVgFLK7V";
    let mut wire = WireState::default();
    wire.event(&json!({"type": "auth_state", "logged_in": true}))
        .unwrap();
    wire.event(&json!({
        "type": "playback_state", "status": "paused", "volume": 0,
        "item": {"uri": uri, "decorations": {"playback": {"duration_ms": 263640}}},
        "position": {"position_ms": 230000, "timestamp_ms": 0, "speed": 0.0},
    }))
    .unwrap();
    let item = SoloistAudio {
        endpoint: "127.0.0.1:9999".parse().unwrap(),
        profile: PathBuf::new(),
        account: String::new(),
        uri: uri.into(),
        expected_duration: Some(263640),
    };
    let state = item.state(&wire).unwrap();
    assert_eq!(state.duration, 263.64);
    assert_eq!(state.position, 230.0);
    assert_eq!(state.volume, 0);
    assert!(state.paused);
    wire.duration = 30_000;
    assert!(item.state(&wire).is_err());
}
