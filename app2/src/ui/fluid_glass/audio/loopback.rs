use std::{
    io,
    sync::{
        Arc, Mutex, OnceLock, PoisonError, TryLockError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use wasapi::{AudioClient, Direction, SampleType, StreamMode, WasapiError, WaveFormat};

use super::{
    Availability, ENVELOPE_POINTS, Signal,
    spectrum::{FRAME_BYTES, SAMPLE_RATE, Spectrum},
};

const EVENT_WAIT_MS: u32 = 35;
const SIGNAL_TIMEOUT: Duration = Duration::from_millis(120);
const MAX_PACKET_FRAMES: usize = SAMPLE_RATE;
const TRACE_REPORT_INTERVAL: Duration = Duration::from_millis(250);
const NONZERO_EPSILON: f32 = 1e-6;
const ACTIVE_SIGNAL_MS: u64 = 500;

fn audio_trace() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| std::env::var_os("MEDIADEKA_GLASS_AUDIO_TRACE").is_some())
}

struct Shared {
    cancelled: AtomicBool,
    signal: Mutex<Signal>,
    active: AtomicBool,
    ended: AtomicBool,
    started: Instant,
    last_signal_ms: AtomicU64,
    #[cfg(any(test, feature = "capture"))]
    packets: AtomicU64,
    #[cfg(any(test, feature = "capture"))]
    silent_packets: AtomicU64,
    #[cfg(any(test, feature = "capture"))]
    nonzero_packets: AtomicU64,
    /// Milliseconds since `started` at the last packet with a nonzero sample,
    /// or `u64::MAX` when the stream has only carried digital silence.
    #[cfg(any(test, feature = "capture"))]
    last_nonzero_ms: AtomicU64,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
            signal: Mutex::new(Signal::default()),
            active: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            started: Instant::now(),
            last_signal_ms: AtomicU64::new(u64::MAX),
            #[cfg(any(test, feature = "capture"))]
            packets: AtomicU64::new(0),
            #[cfg(any(test, feature = "capture"))]
            silent_packets: AtomicU64::new(0),
            #[cfg(any(test, feature = "capture"))]
            nonzero_packets: AtomicU64::new(0),
            #[cfg(any(test, feature = "capture"))]
            last_nonzero_ms: AtomicU64::new(u64::MAX),
        }
    }
}

#[cfg(any(test, feature = "capture"))]
#[derive(Clone, Copy, Debug)]
pub(in crate::ui::fluid_glass) struct Health {
    pub(in crate::ui::fluid_glass) streaming: bool,
    pub(in crate::ui::fluid_glass) packets: u64,
    pub(in crate::ui::fluid_glass) nonzero_packets: u64,
    /// Age of the last nonzero sample; `None` if none has arrived yet.
    pub(in crate::ui::fluid_glass) since_sound_ms: Option<u64>,
    pub(in crate::ui::fluid_glass) age_ms: u64,
}

impl Shared {
    fn publish(&self, signal: Signal) {
        *self.signal.lock().unwrap_or_else(PoisonError::into_inner) = signal;
    }
}

/// Describes what the capture stream has actually delivered. "Streaming" alone
/// only means WASAPI accepted the client and keeps handing over packets; it
/// says nothing about whether those packets carry signal.
#[cfg(any(test, feature = "capture"))]
fn diagnose(health: Health) -> &'static str {
    const WARMUP_MS: u64 = 500;
    if !health.streaming {
        return "stream not started";
    }
    if health.age_ms < WARMUP_MS {
        return "streaming";
    }
    if health.packets == 0 {
        return "streaming, no packets";
    }
    match health.since_sound_ms {
        None | Some(0..=WARMUP_MS) => {
            // A live stream that only ever carries digital silence is not
            // producing signal even though WASAPI reports it as running.
            if health.nonzero_packets == 0 {
                "streaming, digital silence"
            } else {
                "streaming"
            }
        }
        Some(_) => "streaming, silence",
    }
}

pub(in crate::ui::fluid_glass) struct Monitor {
    shared: Arc<Shared>,
}

impl Monitor {
    pub(in crate::ui::fluid_glass) fn start() -> io::Result<Self> {
        Self::start_for(std::process::id())
    }

    fn start_for(pid: u32) -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let worker = Arc::clone(&shared);
        if audio_trace() {
            eprintln!("[glass-audio] starting monitor pid={pid}");
        }
        thread::Builder::new()
            .name("fluid-glass-audio".into())
            .spawn(move || {
                if let Err(error) = run(pid, &worker)
                    && !worker.cancelled.load(Ordering::Relaxed)
                {
                    eprintln!("Аудиосигнал Fluid Glass недоступен: {error}");
                }
                worker.publish(Signal::default());
                worker.ended.store(true, Ordering::Relaxed);
            })?;
        Ok(Self { shared })
    }

    pub(in crate::ui::fluid_glass) fn signal(&self) -> Option<Signal> {
        match self.shared.signal.try_lock() {
            Ok(signal) => Some(*signal),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(error)) => Some(*error.into_inner()),
        }
    }

    pub(in crate::ui::fluid_glass) fn availability(&self) -> Availability {
        if self.shared.ended.load(Ordering::Relaxed) {
            return Availability::Unavailable;
        }
        let age = self.shared.started.elapsed().as_millis() as u64;
        let last = self.shared.last_signal_ms.load(Ordering::Relaxed);
        if last != u64::MAX && age.saturating_sub(last) <= ACTIVE_SIGNAL_MS {
            Availability::Live
        } else if self.shared.active.load(Ordering::Relaxed) && age >= ACTIVE_SIGNAL_MS {
            Availability::Silent
        } else {
            Availability::Starting
        }
    }

    #[cfg(any(test, feature = "capture"))]
    pub(in crate::ui::fluid_glass) fn health(&self) -> Health {
        let age_ms = self.shared.started.elapsed().as_millis() as u64;
        let last = self.shared.last_nonzero_ms.load(Ordering::Relaxed);
        Health {
            streaming: self.shared.active.load(Ordering::Relaxed)
                && !self.shared.ended.load(Ordering::Relaxed),
            packets: self.shared.packets.load(Ordering::Relaxed),
            nonzero_packets: self.shared.nonzero_packets.load(Ordering::Relaxed),
            since_sound_ms: (last != u64::MAX).then(|| age_ms.saturating_sub(last)),
            age_ms,
        }
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui::fluid_glass) fn diagnosis(&self) -> &'static str {
        diagnose(self.health())
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        if audio_trace() {
            eprintln!("[glass-audio] monitor cancelled");
        }
        self.shared.cancelled.store(true, Ordering::Relaxed);
    }
}

fn run(pid: u32, shared: &Shared) -> Result<(), WasapiError> {
    if shared.cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    wasapi::initialize_mta().ok()?;
    let result = capture(pid, shared);
    wasapi::deinitialize();
    result
}

#[derive(Debug)]
struct Trace {
    began: Instant,
    packets: u64,
    frames: u64,
    silent_packets: u64,
    nonzero_packets: u64,
    discontinuities: u64,
    peak: f32,
    interval_packets: u64,
    interval_frames: u64,
    interval_silent: u64,
    interval_nonzero: u64,
    interval_peak: f32,
    last_report: Instant,
    first_sound: bool,
}

impl Trace {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            began: now,
            packets: 0,
            frames: 0,
            silent_packets: 0,
            nonzero_packets: 0,
            discontinuities: 0,
            peak: 0.,
            interval_packets: 0,
            interval_frames: 0,
            interval_silent: 0,
            interval_nonzero: 0,
            interval_peak: 0.,
            last_report: now,
            first_sound: false,
        }
    }

    fn packet(&mut self, frames: usize, info: &wasapi::BufferInfo, pcm: &[u8], signal: &Signal) {
        let peak = pcm
            .as_chunks::<4>()
            .0
            .iter()
            .map(|sample| f32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]).abs())
            .fold(0.0_f32, f32::max);
        self.packets += 1;
        self.frames += frames as u64;
        self.interval_packets += 1;
        self.interval_frames += frames as u64;
        self.peak = self.peak.max(peak);
        self.interval_peak = self.interval_peak.max(peak);
        if info.flags.silent {
            self.silent_packets += 1;
            self.interval_silent += 1;
        }
        if info.flags.data_discontinuity {
            self.discontinuities += 1;
            eprintln!(
                "[glass-audio] +{:.3}s data discontinuity",
                self.began.elapsed().as_secs_f32()
            );
        }
        if peak > NONZERO_EPSILON {
            self.nonzero_packets += 1;
            self.interval_nonzero += 1;
            if !self.first_sound {
                self.first_sound = true;
                eprintln!(
                    "[glass-audio] +{:.3}s first non-silent packet: peak={peak:.6} silent_flag={} bands={:?}",
                    self.began.elapsed().as_secs_f32(),
                    info.flags.silent,
                    signal.features
                );
            }
        }
        if self.last_report.elapsed() >= TRACE_REPORT_INTERVAL {
            eprintln!(
                "[glass-audio] +{:.3}s pkts={} (+{}) frames={} (+{}) silent={} (+{}) nonzero={} (+{}) disc={} report_peak={:.6} peak={:.6} bands={:?}",
                self.began.elapsed().as_secs_f32(),
                self.packets,
                self.interval_packets,
                self.frames,
                self.interval_frames,
                self.silent_packets,
                self.interval_silent,
                self.nonzero_packets,
                self.interval_nonzero,
                self.discontinuities,
                self.interval_peak,
                self.peak,
                signal.features
            );
            self.last_report = Instant::now();
            self.interval_packets = 0;
            self.interval_frames = 0;
            self.interval_silent = 0;
            self.interval_nonzero = 0;
            self.interval_peak = 0.;
        }
    }

    fn summary(&self) -> String {
        format!(
            "duration={:.3}s packets={} frames={} silent_packets={} nonzero_packets={} discontinuities={} peak={:.6}",
            self.began.elapsed().as_secs_f32(),
            self.packets,
            self.frames,
            self.silent_packets,
            self.nonzero_packets,
            self.discontinuities,
            self.peak
        )
    }
}

fn capture(pid: u32, shared: &Shared) -> Result<(), WasapiError> {
    let mut trace = audio_trace().then(Trace::new);
    if audio_trace() {
        eprintln!(
            "[glass-audio] activating application loopback for pid={pid} (include_tree=true)"
        );
    }
    // Declare the event first so it stays valid until the client releases it.
    let event;
    let mut client = AudioClient::new_application_loopback_client(pid, true)?;
    if audio_trace() {
        eprintln!("[glass-audio] application loopback client activated");
    }
    let format = WaveFormat::new(32, 32, SampleType::Float, SAMPLE_RATE, 2, None);
    client.initialize_client(
        &format,
        Direction::Capture,
        StreamMode::EventsShared {
            autoconvert: true,
            buffer_duration_hns: 0,
        },
        None,
    )?;
    if audio_trace() {
        eprintln!(
            "[glass-audio] client initialized: {SAMPLE_RATE} Hz stereo f32, events/autoconvert"
        );
    }
    event = client.set_get_eventhandle()?;
    let capture = client.get_audiocaptureclient()?;
    let mut buffer = Vec::new();
    let mut spectrum = Spectrum::new();
    let mut signal = Signal::default();
    let mut envelope_step = false;
    let mut last_packet: Option<Instant> = None;
    client.start_stream()?;
    if audio_trace() {
        eprintln!("[glass-audio] stream started");
    }
    shared.active.store(true, Ordering::Relaxed);
    while !shared.cancelled.load(Ordering::Relaxed) {
        match event.wait_for_event(EVENT_WAIT_MS) {
            Ok(()) | Err(WasapiError::EventTimeout) => {}
            Err(error) => return Err(error),
        }
        while !shared.cancelled.load(Ordering::Relaxed)
            && let Some(frames @ 1..) = capture.get_next_packet_size()?
        {
            let frames = frames as usize;
            if frames > MAX_PACKET_FRAMES {
                return Err(WasapiError::DataLengthTooShort {
                    received: MAX_PACKET_FRAMES,
                    expected: frames,
                });
            }
            let bytes = frames * FRAME_BYTES;
            if buffer.len() < bytes {
                buffer.resize(bytes, 0);
            }
            let (frames, info) = capture.read_from_device(&mut buffer)?;
            if info.flags.data_discontinuity {
                spectrum.reset();
                signal = Signal::default();
                shared.publish(signal);
            }
            if frames != 0 {
                last_packet = Some(Instant::now());
                let pcm = &mut buffer[..frames as usize * FRAME_BYTES];
                #[cfg(any(test, feature = "capture"))]
                {
                    let peak = pcm
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|sample| f32::from_le_bytes(*sample).abs())
                        .fold(0.0_f32, f32::max);
                    shared.packets.fetch_add(1, Ordering::Relaxed);
                    if info.flags.silent {
                        shared.silent_packets.fetch_add(1, Ordering::Relaxed);
                    }
                    if peak > NONZERO_EPSILON {
                        shared.nonzero_packets.fetch_add(1, Ordering::Relaxed);
                        let elapsed = shared.started.elapsed().as_millis() as u64;
                        shared.last_nonzero_ms.store(elapsed, Ordering::Relaxed);
                    }
                }
                if let Some(trace) = &mut trace {
                    trace.packet(frames as usize, &info, pcm, &signal);
                }
                let mut updated = false;
                let mut has_audio = false;
                spectrum.push(pcm, |measurement| {
                    signal.features = signal.features.follow(measurement.features, Spectrum::HOP);
                    if !envelope_step {
                        signal.envelope.rotate_left(1);
                        signal.envelope[ENVELOPE_POINTS - 1] = measurement.envelope_rms;
                    }
                    envelope_step = !envelope_step;
                    has_audio |= measurement.features.rms > 0.;
                    updated = true;
                });
                pcm.fill(0);
                if updated {
                    shared.publish(signal);
                }
                if has_audio {
                    let age = shared.started.elapsed().as_millis() as u64;
                    shared.last_signal_ms.store(age, Ordering::Relaxed);
                }
            }
        }
        if last_packet.is_some_and(|time| time.elapsed() >= SIGNAL_TIMEOUT) {
            last_packet = None;
            spectrum.reset();
            signal = Signal::default();
            shared.publish(signal);
            if audio_trace() {
                eprintln!(
                    "[glass-audio] no packets for {} ms; publishing zero signal",
                    SIGNAL_TIMEOUT.as_millis()
                );
            }
        }
    }
    if let Some(trace) = &trace {
        eprintln!("[glass-audio] capture ended: {}", trace.summary());
    }
    client.stop_stream()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnosis_separates_unavailable_quiet_and_silent_streams() {
        let health = |streaming, age_ms, packets, nonzero_packets, since_sound_ms| Health {
            streaming,
            packets,
            nonzero_packets,
            since_sound_ms,
            age_ms,
        };

        assert_eq!(
            diagnose(health(false, 2_000, 0, 0, None)),
            "stream not started"
        );
        assert_eq!(diagnose(health(true, 100, 0, 0, None)), "streaming");
        assert_eq!(
            diagnose(health(true, 2_000, 0, 0, None)),
            "streaming, no packets"
        );
        // A running stream delivering only digital zeros is reported as such,
        // not mistaken for an unreachable or idle API.
        assert_eq!(
            diagnose(health(true, 2_000, 40, 0, None)),
            "streaming, digital silence"
        );
        assert_eq!(diagnose(health(true, 2_000, 40, 8, Some(120))), "streaming");
        assert_eq!(
            diagnose(health(true, 2_000, 40, 8, Some(900))),
            "streaming, silence"
        );
    }

    /// Starts a real local playback first, then activates process loopback.
    /// This is the order the Fluid Glass backdrop uses when the user starts a
    /// track before the audio-reactive layer is sampled: WASAPI can report the
    /// client as running while delivering only digital silence.
    #[test]
    #[ignore = "requires libmpv, MEDIADEKA_AUDIO_FIXTURE and a real WASAPI output"]
    fn captures_playback_started_before_the_client() {
        use crate::playback::{Playback, Status};
        use crate::playlist::{Media, PlaylistKey, PlaylistTrack};

        let fixture = std::path::PathBuf::from(
            std::env::var_os("MEDIADEKA_AUDIO_FIXTURE").expect("MEDIADEKA_AUDIO_FIXTURE"),
        );
        let entry = crate::playback::QueueEntry {
            playlist: PlaylistKey::Local(1),
            playlist_revision: None,
            index: 0,
            track: PlaylistTrack {
                id: "same-recording".into(),
                title: "Recording".into(),
                artist: "Test artist".into(),
                artists: Vec::new(),
                artist_credits: Vec::new(),
                album_ref: None,
                album: None,
                artwork: None,
                added: None,
                duration: None,
                media: Some(Media::File(fixture)),
            },
            recommendations: Vec::new(),
        };
        let mut playback = Playback::new();
        playback.set_volume(0.5);
        playback.play(vec![entry], 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            playback.poll();
            if playback.status() == &Status::Playing && playback.position() > 0.4 {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "playback did not start: status={:?}",
                playback.status()
            );
            thread::sleep(Duration::from_millis(20));
        }

        let monitor = Monitor::start().unwrap();
        let deadline = Instant::now() + Duration::from_secs(6);
        loop {
            playback.poll();
            if let Some(signal) = monitor.signal() {
                let features = signal.features;
                let peak = features.low().max(features.middle()).max(features.high());
                let rms = signal.envelope[ENVELOPE_POINTS - 1];
                if peak > 0.02 && rms > 0.001 {
                    println!(
                        "Capture activated after playback started: peak={peak:.3} rms={rms:.3} health={:?}",
                        monitor.health()
                    );
                    return;
                }
            }
            assert!(
                Instant::now() < deadline,
                "capture carried no signal while playback advanced to {:.2}s: health={:?}",
                playback.position(),
                monitor.health()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }

    #[test]
    #[ignore = "requires a real Windows audio stream in a sibling process"]
    fn excludes_audio_from_another_process() {
        let sibling = std::env::var("MEDIADEKA_GLASS_EXTERNAL_TONE_PID")
            .expect("set MEDIADEKA_GLASS_EXTERNAL_TONE_PID to the sibling player's PID")
            .parse::<u32>()
            .expect("a Windows process ID");
        assert_ne!(sibling, std::process::id());
        let own = Monitor::start().unwrap();
        let external = Monitor::start_for(sibling).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut positive = Vec::new();
        while positive.len() < 4 {
            let external_signal = external.signal();
            if let (Some(signal), Some(own_signal)) = (external_signal, own.signal())
                && own.shared.active.load(Ordering::Relaxed)
            {
                let features = signal.features;
                let peak = features.low().max(features.middle()).max(features.high());
                let rms = signal.envelope[ENVELOPE_POINTS - 1];
                if peak > 0.05 && rms > 0.05 {
                    assert_eq!(own_signal, Signal::default());
                    positive.push((peak, rms));
                }
            }
            assert!(
                Instant::now() < deadline,
                "external signal={external_signal:?}"
            );
            thread::sleep(Duration::from_millis(60));
        }
        println!("Sibling WASAPI band peaks and RMS {positive:?}, Mediadeka signal remained zero");
    }
}
