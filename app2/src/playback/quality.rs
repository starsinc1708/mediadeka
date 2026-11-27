use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod decode;
#[cfg(windows)]
pub(crate) use decode::validate;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AudioQuality {
    pub codec: String,
    pub bitrate_bps: Option<u32>,
    pub sample_rate_hz: u32,
    #[serde(default)]
    spectral_bitrate_bps: Option<u32>,
}

impl AudioQuality {
    pub(crate) fn new(codec: String, bitrate_bps: Option<u32>, sample_rate_hz: u32) -> Self {
        Self {
            codec,
            bitrate_bps,
            sample_rate_hz,
            spectral_bitrate_bps: None,
        }
    }

    pub(crate) fn rank(&self) -> (bool, u32, u32) {
        let bitrate = self.bitrate_bps.unwrap_or_default();
        if let Some(estimated) = self.spectral_bitrate_bps {
            return (false, estimated, self.sample_rate_hz.min(48_000));
        }
        (self.is_lossless(), bitrate, self.sample_rate_hz)
    }

    pub(crate) fn label(&self) -> String {
        let mut quality = self.codec_label();
        if let Some(bitrate) = self.bitrate_label() {
            quality.push_str(&format!(" · {bitrate}"));
        }
        quality.push_str(&format!(" · {}", self.sample_rate_label()));
        quality
    }

    pub(crate) fn compact_label(&self) -> String {
        let rate = self
            .bitrate_label()
            .unwrap_or_else(|| self.sample_rate_label());
        format!("{} · {rate}", self.codec_label())
    }

    fn is_lossless(&self) -> bool {
        matches!(
            self.codec.as_str(),
            "alac"
                | "als"
                | "ape"
                | "flac"
                | "mlp"
                | "shorten"
                | "tak"
                | "truehd"
                | "tta"
                | "wavpack"
                | "wmalossless"
        ) || ["pcm_s", "pcm_u", "pcm_f"]
            .iter()
            .any(|prefix| self.codec.starts_with(prefix))
    }

    pub(crate) fn codec_label(&self) -> String {
        match self.codec.as_str() {
            "aac_latm" => "AAC".into(),
            codec if codec.starts_with("pcm_") => "PCM".into(),
            codec => codec.to_ascii_uppercase(),
        }
    }

    pub(crate) fn bitrate_label(&self) -> Option<String> {
        self.bitrate_bps.map(|bitrate| {
            let kilobits = (f64::from(bitrate) / 1000.).round() as u32;
            format!("{kilobits} кбит/с")
        })
    }

    pub(crate) fn sample_rate_label(&self) -> String {
        let kilohertz = f64::from(self.sample_rate_hz) / 1000.;
        format!("{kilohertz} кГц").replace('.', ",")
    }
}

#[cfg(test)]
mod tests {
    use super::AudioQuality;

    #[test]
    fn ranks_actual_format_and_keeps_display_separate_from_spectral_estimate() {
        let lossy = AudioQuality::new("mp3".into(), Some(320_000), 48_000);
        let mut lossless = AudioQuality::new("flac".into(), Some(700_000), 44_100);
        assert!(lossless.rank() > lossy.rank());
        lossless.spectral_bitrate_bps = Some(128_000);
        assert!(lossless.rank() < lossy.rank());
        assert_eq!(lossless.label(), "FLAC · 700 кбит/с · 44,1 кГц");
        assert_eq!(lossless.compact_label(), "FLAC · 700 кбит/с");
    }
}
