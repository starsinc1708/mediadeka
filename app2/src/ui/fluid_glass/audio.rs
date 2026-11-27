//! Read-only spectrum and amplitude from the application's own rendered audio.

#[cfg(windows)]
mod loopback;
pub(super) mod modulation;
#[cfg(any(windows, test))]
mod spectrum;

#[cfg(windows)]
pub(super) use loopback::Monitor;

pub(super) const ENVELOPE_POINTS: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct AudioFeatures {
    pub(super) rms: f32,
    pub(super) peak: f32,
    pub(super) sub_bass: f32,
    pub(super) bass: f32,
    pub(super) low_mid: f32,
    pub(super) mid: f32,
    pub(super) high_mid: f32,
    pub(super) treble: f32,
    pub(super) spectral_flux: f32,
    /// Ratio to the upper audible frequency analyzed by the FFT.
    pub(super) spectral_centroid: f32,
    pub(super) transient_strength: f32,
    pub(super) beat_strength: f32,
}

impl AudioFeatures {
    pub(super) fn low(self) -> f32 {
        self.sub_bass.max(self.bass)
    }

    pub(super) fn middle(self) -> f32 {
        self.low_mid.max(self.mid).max(self.high_mid * 0.65)
    }

    pub(super) fn high(self) -> f32 {
        self.treble.max(self.high_mid * 0.75)
    }

    pub(super) fn follow(self, target: Self, dt: f32) -> Self {
        let follow = |value, goal, attack, release| approach(value, goal, dt, attack, release);
        Self {
            rms: follow(self.rms, target.rms, 0.16, 0.55),
            peak: follow(self.peak, target.peak, 0.05, 0.32),
            sub_bass: follow(self.sub_bass, target.sub_bass, 0.08, 0.35),
            bass: follow(self.bass, target.bass, 0.08, 0.35),
            low_mid: follow(self.low_mid, target.low_mid, 0.12, 0.46),
            mid: follow(self.mid, target.mid, 0.12, 0.46),
            high_mid: follow(self.high_mid, target.high_mid, 0.07, 0.32),
            treble: follow(self.treble, target.treble, 0.05, 0.25),
            spectral_flux: follow(self.spectral_flux, target.spectral_flux, 0.022, 0.14),
            spectral_centroid: follow(self.spectral_centroid, target.spectral_centroid, 0.12, 0.32),
            transient_strength: follow(
                self.transient_strength,
                target.transient_strength,
                0.015,
                0.15,
            ),
            beat_strength: follow(self.beat_strength, target.beat_strength, 0.020, 0.24),
        }
    }
}

pub(super) fn approach(current: f32, target: f32, dt: f32, attack: f32, release: f32) -> f32 {
    const SILENCE: f32 = 0.001;
    let time_constant = if target > current { attack } else { release };
    let next = current + (target - current) * (1.0 - (-dt / time_constant).exp());
    if next < SILENCE && target < SILENCE {
        0.0
    } else {
        next
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct Signal {
    pub(super) features: AudioFeatures,
    /// Normalized stereo RMS from the oldest window to the newest one.
    pub(super) envelope: [f32; ENVELOPE_POINTS],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::ui) enum Availability {
    #[default]
    Inactive,
    Starting,
    Silent,
    Live,
    Unavailable,
}

impl Availability {
    pub(in crate::ui) fn label(self) -> &'static str {
        match self {
            Self::Inactive => "Ожидание воспроизведения",
            Self::Starting => "Подключение к аудиопотоку",
            Self::Silent => "В аудиопотоке тишина",
            Self::Live => "Получаем аудиосигнал",
            Self::Unavailable => "Аудиосигнал недоступен",
        }
    }
}

#[cfg(not(windows))]
pub(super) struct Monitor;

#[cfg(not(windows))]
impl Monitor {
    pub(super) fn start() -> std::io::Result<Self> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    pub(super) fn signal(&self) -> Option<Signal> {
        Some(Signal::default())
    }

    pub(super) fn availability(&self) -> Availability {
        Availability::Unavailable
    }

    #[cfg(feature = "capture")]
    pub(super) fn diagnosis(&self) -> &'static str {
        "unavailable"
    }
}
