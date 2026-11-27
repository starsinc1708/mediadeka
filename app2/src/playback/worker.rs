use std::sync::mpsc;
#[cfg(windows)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use async_channel::Receiver;

use super::{
    AudioDevice, DEFAULT_AUDIO_DEVICE, PollChange, QueueEntry, SoundDetails, SoundSource, Status,
};

#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;

pub(super) const WORKER_STOPPED: &str = "Движок воспроизведения завершился.";

#[derive(Clone, PartialEq)]
pub(super) struct Frame {
    pub generation: u64,
    pub sequence: u64,
    pub status: Status,
    pub position: f64,
    pub listened: Duration,
    pub seek_revision: u64,
    pub duration: Option<f64>,
    pub volume: f64,
    pub seekable: bool,
    pub completed: bool,
    pub soloist_setup_required: bool,
    pub error: Option<String>,
    pub device: String,
    pub devices: Vec<AudioDevice>,
    pub output: Option<String>,
    pub source: Option<SoundSource>,
    pub sound: SoundDetails,
    pub cache_revision: u64,
    #[cfg(test)]
    pub test_audio_format: Option<(String, f64, f64)>,
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            generation: 0,
            sequence: 0,
            status: Status::Idle,
            position: 0.,
            listened: Duration::ZERO,
            seek_revision: 0,
            duration: None,
            volume: 1.,
            seekable: false,
            completed: false,
            soloist_setup_required: false,
            error: None,
            device: DEFAULT_AUDIO_DEVICE.into(),
            devices: Vec::new(),
            output: None,
            source: None,
            sound: SoundDetails::default(),
            cache_revision: 0,
            #[cfg(test)]
            test_audio_format: None,
        }
    }
}

impl Frame {
    pub(super) fn change(&self, next: &Self) -> PollChange {
        if self.generation != next.generation
            || self.status != next.status
            || self.duration != next.duration
            || self.volume != next.volume
            || self.seekable != next.seekable
            || self.soloist_setup_required != next.soloist_setup_required
            || self.error != next.error
            || self.device != next.device
            || self.devices != next.devices
            || self.output != next.output
            || self.source != next.source
            || self.sound != next.sound
        {
            PollChange::Transport
        } else if self.position != next.position {
            PollChange::Position
        } else {
            PollChange::None
        }
    }
}

pub(super) enum Command {
    Load {
        entry: Box<QueueEntry>,
        position: f64,
        requested_at: Instant,
    },
    Prefetch {
        entries: Vec<QueueEntry>,
        near_end: bool,
    },
    Pause(bool),
    Seek(f64),
    Volume(f64),
    Devices,
    Device(String),
}

#[cfg(windows)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Failure {
    Track(String),
    SoloistSetup(String),
}

#[cfg(windows)]
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self::Track(message)
    }
}

#[cfg(windows)]
impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        Self::Track(message.into())
    }
}

#[cfg(windows)]
impl Failure {
    pub(super) fn into_message(self) -> String {
        match self {
            Self::Track(message) | Self::SoloistSetup(message) => message,
        }
    }
}

enum Message {
    Request {
        sequence: u64,
        generation: u64,
        command: Command,
    },
    #[cfg(windows)]
    Resolved {
        generation: u64,
        input: Result<windows::Resolved, Failure>,
    },
    #[cfg(windows)]
    Downloading {
        generation: u64,
    },
    #[cfg(windows)]
    CacheChanged,
    Shutdown,
}

#[cfg(windows)]
#[derive(Default)]
struct Signals {
    requested: AtomicU64,
    closing: AtomicBool,
    soloist: AtomicBool,
}

pub(super) struct Worker {
    commands: mpsc::Sender<Message>,
    pub frames: Receiver<Frame>,
    #[cfg(windows)]
    signals: Arc<Signals>,
    #[cfg(windows)]
    stopped: mpsc::Receiver<()>,
}

impl Worker {
    pub(super) fn spawn(volume: f64) -> Result<Self, &'static str> {
        #[cfg(windows)]
        {
            Self::spawn_windows(volume, windows::run)
        }
        #[cfg(not(windows))]
        {
            let _ = volume;
            Err("Аудиодвижок Mediadeka доступен на Windows.")
        }
    }

    #[cfg(windows)]
    fn spawn_windows(
        volume: f64,
        run: impl FnOnce(
            mpsc::Sender<Message>,
            mpsc::Receiver<Message>,
            async_channel::Sender<Frame>,
            Arc<Signals>,
            f64,
        ) + Send
        + 'static,
    ) -> Result<Self, &'static str> {
        let (commands, receiver) = mpsc::channel();
        let (output, frames) = async_channel::bounded(1);
        let sender = commands.clone();
        let signals = Arc::new(Signals::default());
        let worker_signals = Arc::clone(&signals);
        let (done, stopped) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("mediadeka-audio".into())
            .spawn(move || {
                run(sender, receiver, output, worker_signals, volume);
                let _ = done.send(());
            })
            .map_err(|_| "Не удалось запустить аудиодвижок.")?;
        Ok(Self {
            commands,
            frames,
            signals,
            stopped,
        })
    }

    pub(super) fn send(
        &self,
        sequence: u64,
        generation: u64,
        command: Command,
    ) -> Result<(), &'static str> {
        #[cfg(windows)]
        self.signals.requested.store(generation, Ordering::Release);
        self.commands
            .send(Message::Request {
                sequence,
                generation,
                command,
            })
            .map_err(|_| WORKER_STOPPED)
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        #[cfg(windows)]
        self.signals.closing.store(true, Ordering::SeqCst);
        let _ = self.commands.send(Message::Shutdown);
        #[cfg(windows)]
        if self.signals.soloist.load(Ordering::SeqCst) {
            // Soloist renders outside our process. Give its worker time to
            // confirm the pause before closing the application.
            let _ = self.stopped.recv_timeout(Duration::from_secs(4));
        }
    }
}
