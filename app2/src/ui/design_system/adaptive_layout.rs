//! Composition rules keyed to the space actually available to the page.

use crate::appearance::{TrackColumn, TrackColumnVisibility, TrackDensity};

const THRESHOLDS: [f32; 4] = [500., 750., 1050., 1400.];
const HYSTERESIS: f32 = 16.;
const MAX_TABLE_WIDTH: f32 = 1340.;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::ui) enum LayoutBand {
    Narrow,
    Compact,
    Single,
    Standard,
    Expanded,
}

impl LayoutBand {
    const ALL: [Self; 5] = [
        Self::Narrow,
        Self::Compact,
        Self::Single,
        Self::Standard,
        Self::Expanded,
    ];
}

#[derive(Clone, Copy)]
pub(in crate::ui) struct AdaptiveContent {
    pub width: f32,
    pub band: LayoutBand,
}

impl AdaptiveContent {
    pub fn resolve(width: f32, previous: Option<LayoutBand>) -> Self {
        let previous = previous.map(|band| band as usize);
        let index = THRESHOLDS
            .iter()
            .enumerate()
            .filter(|&(index, threshold)| {
                let bias = previous.map_or(0., |band| {
                    if band > index {
                        -HYSTERESIS
                    } else {
                        HYSTERESIS
                    }
                });
                width >= threshold + bias
            })
            .count();
        Self {
            width: width.max(0.),
            band: LayoutBand::ALL[index],
        }
    }

    pub fn compact(self) -> bool {
        matches!(self.band, LayoutBand::Narrow | LayoutBand::Compact)
    }

    pub fn table_width(self) -> f32 {
        self.width.min(MAX_TABLE_WIDTH)
    }

    pub fn reading_width(self) -> f32 {
        self.width.min(match self.band {
            LayoutBand::Expanded | LayoutBand::Standard => 1120.,
            _ => 950.,
        })
    }
}

#[derive(Clone, Copy)]
pub(in crate::ui) struct TrackColumns {
    pub artist: Option<f32>,
    pub album: Option<f32>,
    pub date: Option<f32>,
    pub source: Option<f32>,
    pub duration: f32,
}

impl TrackColumns {
    pub fn library(width: f32, visibility: TrackColumnVisibility) -> Self {
        Self::fit(width, true).with_visibility(visibility)
    }

    pub fn playlist(width: f32, visibility: TrackColumnVisibility) -> Self {
        Self::fit(width, false).with_visibility(visibility)
    }

    pub fn with_visibility(self, visibility: TrackColumnVisibility) -> Self {
        Self {
            artist: self
                .artist
                .filter(|_| visibility.shows(TrackColumn::Artist)),
            album: self.album.filter(|_| visibility.shows(TrackColumn::Album)),
            date: self.date.filter(|_| visibility.shows(TrackColumn::Date)),
            source: self
                .source
                .filter(|_| visibility.shows(TrackColumn::Source)),
            ..self
        }
    }

    fn fit(width: f32, source: bool) -> Self {
        let width = width.min(MAX_TABLE_WIDTH);
        let roomy = width >= THRESHOLDS[2];
        let needs_source_room = if source { 65. } else { 0. };
        Self {
            artist: (width >= 720. + needs_source_room).then_some(if roomy { 202. } else { 180. }),
            album: (width >= 950. + needs_source_room).then_some(if roomy { 216. } else { 180. }),
            date: (width >= 1120. + needs_source_room).then_some(120.),
            source: (source && width >= 690.).then_some(if roomy { 76. } else { 64. }),
            duration: if width >= 600. { 68. } else { 56. },
        }
    }
}

impl TrackDensity {
    pub(in crate::ui) const fn label(self) -> &'static str {
        match self {
            Self::Compact => "Компактная",
            Self::Standard => "Стандартная",
            Self::Comfortable => "Комфортная",
        }
    }

    pub(in crate::ui) const fn row_height(self) -> f32 {
        match self {
            Self::Compact => 36.,
            Self::Standard => 42.,
            Self::Comfortable => 54.,
        }
    }

    pub(in crate::ui) const fn cover(self) -> f32 {
        match self {
            Self::Compact => 28.,
            Self::Standard => 36.,
            Self::Comfortable => 44.,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_width_can_change_band_and_small_resize_keeps_it_stable() {
        let unobstructed = AdaptiveContent::resolve(1410., None);
        assert_eq!(unobstructed.band, LayoutBand::Expanded);
        assert_eq!(
            AdaptiveContent::resolve(1394.1, Some(unobstructed.band)).band,
            LayoutBand::Expanded
        );
        let docked = AdaptiveContent::resolve(1090., Some(unobstructed.band));
        assert_eq!(docked.band, LayoutBand::Standard);
        assert_eq!(
            AdaptiveContent::resolve(1408., Some(docked.band)).band,
            LayoutBand::Standard
        );
        assert_eq!(
            AdaptiveContent::resolve(1420., Some(docked.band)).band,
            LayoutBand::Expanded
        );
    }

    #[test]
    fn optional_columns_release_space_before_the_title_and_duration() {
        let default = TrackColumnVisibility::default();
        let full = TrackColumns::library(1400., default);
        let docked = TrackColumns::library(910., default);
        let narrow = TrackColumns::library(510., default);
        assert_eq!(full.date, Some(120.));
        assert!(docked.date.is_none() && docked.album.is_none());
        assert!(docked.artist.is_some() && docked.source.is_some());
        assert!(narrow.artist.is_none() && narrow.source.is_none());
        assert_eq!(narrow.duration, 56.);
        let custom = TrackColumns::library(1400., default.toggle(TrackColumn::Album));
        assert!(custom.album.is_none() && custom.date.is_some());
    }
}
