//! The shell and Fluid Glass palette stored in the profile settings.
//!
//! Environment variables remain test and acceptance overrides. A normal launch
//! reads the saved row and keeps it across restarts.

use serde::{Deserialize, Serialize};

use crate::{storage::Store, visual::VisualSettings};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum TrackDensity {
    Compact,
    #[default]
    Standard,
    Comfortable,
}

impl TrackDensity {
    pub(crate) const ALL: [Self; 3] = [Self::Compact, Self::Standard, Self::Comfortable];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Compact => "compact",
            Self::Standard => "standard",
            Self::Comfortable => "comfortable",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|density| density.as_str() == value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TrackColumn {
    Artist,
    Album,
    Date,
    Source,
}

impl TrackColumn {
    pub(crate) const ALL: [Self; 4] = [Self::Artist, Self::Album, Self::Date, Self::Source];

    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Artist => "artist",
            Self::Album => "album",
            Self::Date => "date",
            Self::Source => "source",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct TrackColumnVisibility(u8);

impl TrackColumnVisibility {
    const ALL_HIDDEN: u8 = (1 << TrackColumn::ALL.len()) - 1;

    pub(crate) fn parse(hidden_columns: u8) -> Option<Self> {
        (hidden_columns <= Self::ALL_HIDDEN).then_some(Self(hidden_columns))
    }

    pub(crate) const fn hidden_columns(self) -> u8 {
        self.0
    }

    pub(crate) const fn shows(self, column: TrackColumn) -> bool {
        self.0 & (1 << column as u8) == 0
    }

    pub(crate) const fn toggle(self, column: TrackColumn) -> Self {
        Self(self.0 ^ (1 << column as u8))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shell {
    FluidGlass,
    Classic,
}

impl Shell {
    pub const ALL: [Self; 2] = [Self::FluidGlass, Self::Classic];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FluidGlass => "fluid-glass",
            Self::Classic => "classic",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "fluid-glass" => Some(Self::FluidGlass),
            "classic" => Some(Self::Classic),
            _ => None,
        }
    }

    /// `MEDIADEKA_GUI` selects a shell for this process without touching the profile.
    pub fn env_override() -> Option<Self> {
        match std::env::var("MEDIADEKA_GUI").ok()?.as_str() {
            "3" => Some(Self::FluidGlass),
            "2" => Some(Self::Classic),
            _ => None,
        }
    }
}

impl Default for Shell {
    fn default() -> Self {
        if cfg!(feature = "fluid-glass-default") {
            Self::FluidGlass
        } else {
            Self::Classic
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GlassPalette {
    #[default]
    Auto,
    Solar,
    Abyss,
    Ultraviolet,
    Monochrome,
    Ember,
    Acid,
    Glacier,
    Rosewood,
}

impl GlassPalette {
    pub const ALL: [Self; 9] = [
        Self::Auto,
        Self::Solar,
        Self::Abyss,
        Self::Ultraviolet,
        Self::Monochrome,
        Self::Ember,
        Self::Acid,
        Self::Glacier,
        Self::Rosewood,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Solar => "solar",
            Self::Abyss => "abyss",
            Self::Ultraviolet => "ultraviolet",
            Self::Monochrome => "monochrome",
            Self::Ember => "ember",
            Self::Acid => "acid",
            Self::Glacier => "glacier",
            Self::Rosewood => "rosewood",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "solar" => Some(Self::Solar),
            "abyss" => Some(Self::Abyss),
            "ultraviolet" => Some(Self::Ultraviolet),
            "monochrome" => Some(Self::Monochrome),
            "ember" => Some(Self::Ember),
            "acid" => Some(Self::Acid),
            "glacier" => Some(Self::Glacier),
            "rosewood" => Some(Self::Rosewood),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Auto => "Авто по обложке",
            Self::Solar => "Solar",
            Self::Abyss => "Abyss",
            Self::Ultraviolet => "Ultraviolet",
            Self::Monochrome => "Monochrome",
            Self::Ember => "Ember",
            Self::Acid => "Acid",
            Self::Glacier => "Glacier",
            Self::Rosewood => "Rosewood",
        }
    }

    /// `MEDIADEKA_GLASS_THEME` keeps fixed-theme acceptance runs reproducible.
    pub fn env_override() -> Option<Self> {
        Self::parse(
            &std::env::var("MEDIADEKA_GLASS_THEME")
                .ok()?
                .trim()
                .to_ascii_lowercase(),
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Appearance {
    pub shell: Shell,
    pub palette: GlassPalette,
    pub visual: VisualSettings,
}

impl Appearance {
    /// The control shell used by the non-Fluid-Glass test harnesses.
    #[cfg(test)]
    pub const fn classic() -> Self {
        Self {
            shell: Shell::Classic,
            palette: GlassPalette::Auto,
            visual: VisualSettings::standard(),
        }
    }

    pub fn load() -> Self {
        match Store::open_default().and_then(|store| store.appearance()) {
            Ok(Some(appearance)) => appearance,
            Ok(None) => Self::default(),
            Err(error) => {
                eprintln!("Не удалось прочитать сохранённое оформление: {error}");
                Self::default()
            }
        }
    }

    pub fn startup() -> Self {
        let mut appearance = Self::load();
        if let Some(shell) = Shell::env_override() {
            appearance.shell = shell;
        }
        if let Some(palette) = GlassPalette::env_override() {
            appearance.palette = palette;
        }
        appearance.visual = appearance.visual.with_env();
        appearance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_round_trips_through_its_stored_name() {
        for shell in Shell::ALL {
            assert_eq!(Shell::parse(shell.as_str()), Some(shell));
        }
        for palette in GlassPalette::ALL {
            assert_eq!(GlassPalette::parse(palette.as_str()), Some(palette));
        }
        assert_eq!(Shell::parse("unknown"), None);
        assert_eq!(GlassPalette::parse("unknown"), None);
    }
}
