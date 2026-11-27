use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AudioProfile {
    Calm,
    #[default]
    Balanced,
    Immersive,
}

impl AudioProfile {
    pub const ALL: [Self; 3] = [Self::Calm, Self::Balanced, Self::Immersive];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Calm => "calm",
            Self::Balanced => "balanced",
            Self::Immersive => "immersive",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|profile| profile.as_str() == value)
    }

    pub(crate) const fn gain(self) -> f32 {
        match self {
            Self::Calm => 0.35,
            Self::Balanced => 0.68,
            Self::Immersive => 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudioChannel {
    Patterns,
    Lighting,
    Reflections,
    Dispersion,
    Pulses,
    Glitch,
}

impl AudioChannel {
    pub const ALL: [Self; 6] = [
        Self::Patterns,
        Self::Lighting,
        Self::Reflections,
        Self::Dispersion,
        Self::Pulses,
        Self::Glitch,
    ];
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct AudioResponse {
    pub enabled: bool,
    pub profile: AudioProfile,
    /// A persisted percentage in 0..=100.
    sensitivity: u8,
    channels: u8,
}

impl AudioResponse {
    const ALL_CHANNELS: u8 = (1 << AudioChannel::ALL.len()) - 1;

    pub const fn standard() -> Self {
        Self {
            enabled: true,
            profile: AudioProfile::Balanced,
            sensitivity: 65,
            channels: Self::ALL_CHANNELS,
        }
    }

    pub const fn from_stored(
        enabled: bool,
        profile: AudioProfile,
        sensitivity: u8,
        channels: u8,
    ) -> Option<Self> {
        if sensitivity <= 100 && channels <= Self::ALL_CHANNELS {
            Some(Self {
                enabled,
                profile,
                sensitivity,
                channels,
            })
        } else {
            None
        }
    }

    pub const fn sensitivity(self) -> u8 {
        self.sensitivity
    }

    pub fn set_sensitivity(&mut self, percent: u8) {
        self.sensitivity = percent.min(100);
    }

    pub const fn channels(self) -> u8 {
        self.channels
    }

    pub const fn allows(self, channel: AudioChannel) -> bool {
        self.enabled && self.channel_enabled(channel)
    }

    pub const fn channel_enabled(self, channel: AudioChannel) -> bool {
        self.channels & (1 << channel as u8) != 0
    }

    pub fn toggle_channel(&mut self, channel: AudioChannel) {
        self.channels ^= 1 << channel as u8;
    }
}

impl Default for AudioResponse {
    fn default() -> Self {
        Self::standard()
    }
}
