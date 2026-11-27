use std::{
    ops::Range,
    time::{Duration, Instant},
};

use crate::music::lyrics::{Line, Lyrics};
use crate::ui::player::PLAYBACK_POLL_INTERVAL;

// A seek takes effect in the lyrics as soon as the user requests it. The next
// worker poll may still report the old position, so allow it two poll periods
// to confirm the new position before falling back to the observed clock.
const SEEK_ACK_WINDOW: Duration = Duration::from_millis(330);
const SEEK_ACK_TOLERANCE: f64 = 0.35;

pub(super) struct Clock {
    observed: f64,
    position: f64,
    sampled_at: Instant,
    playing: bool,
    seeking: bool,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            observed: 0.,
            position: 0.,
            sampled_at: Instant::now(),
            playing: false,
            seeking: false,
        }
    }
}

impl Clock {
    pub(super) fn sample(&mut self, position: f64, playing: bool) {
        let now = Instant::now();
        if self.seeking {
            if now.duration_since(self.sampled_at) < SEEK_ACK_WINDOW
                && (self.position - position).abs() > SEEK_ACK_TOLERANCE
            {
                self.playing = playing;
                return;
            }
            self.seeking = false;
        } else if self.observed == position && self.playing == playing {
            return;
        }
        self.observed = position;
        self.position = position;
        self.sampled_at = now;
        self.playing = playing;
    }

    pub(super) fn seek(&mut self, position: f64) {
        self.position = position;
        self.sampled_at = Instant::now();
        self.seeking = true;
    }

    pub(super) fn playing(&self) -> bool {
        self.playing
    }

    pub(super) fn position_ms(&self) -> u64 {
        let elapsed = if self.playing {
            self.sampled_at
                .elapsed()
                .min(PLAYBACK_POLL_INTERVAL)
                .as_secs_f64()
        } else {
            0.
        };
        ((self.position + elapsed) * 1000.).round().max(0.) as u64
    }
}

#[derive(Default, PartialEq, Eq)]
pub(super) struct Highlight {
    pub(super) line: Option<usize>,
    pub(super) word: Option<usize>,
}

impl Highlight {
    pub(super) fn at(lyrics: &Lyrics, position_ms: u64) -> Self {
        let line = lyrics.active_line(position_ms);
        let word = line.and_then(|index| {
            let words = &lyrics.lines[index].words;
            let index = words.iter().rposition(|word| word.time_ms <= position_ms)?;
            words[index]
                .end_ms
                .is_none_or(|end| end > position_ms)
                .then_some(index)
        });
        Self { line, word }
    }
}

/// GPUI text runs address UTF-8 bytes; provider timings address UTF-16 units.
pub(super) fn word_bytes(line: &Line, index: usize) -> Option<Range<usize>> {
    let word = line.words.get(index)?;
    let start = byte_at_utf16(&line.text, word.start_utf16)?;
    let end = byte_at_utf16(&line.text, word.end_utf16)?;
    (start < end).then_some(start..end)
}

fn byte_at_utf16(text: &str, offset: usize) -> Option<usize> {
    let mut consumed = 0;
    for (byte, character) in text.char_indices() {
        if consumed == offset {
            return Some(byte);
        }
        consumed += character.len_utf16();
        if consumed > offset {
            return None;
        }
    }
    (consumed == offset).then_some(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::music::lyrics::{Source, Word};

    #[test]
    fn the_active_word_uses_a_unicode_range_and_clears_at_its_end() {
        let lyrics = Lyrics {
            source: Source::Local,
            instrumental: false,
            attribution: None,
            lines: vec![Line {
                time_ms: Some(1_000),
                text: "🎵 Привет".into(),
                words: vec![Word {
                    time_ms: 1_200,
                    end_ms: Some(1_500),
                    start_utf16: 3,
                    end_utf16: 9,
                }],
            }],
        };
        assert_eq!(Highlight::at(&lyrics, 1_199).word, None);
        assert_eq!(Highlight::at(&lyrics, 1_200).word, Some(0));
        assert_eq!(Highlight::at(&lyrics, 1_500).word, None);
        let line = &lyrics.lines[0];
        assert_eq!(&line.text[word_bytes(line, 0).unwrap()], "Привет");
    }

    #[test]
    fn stale_seek_samples_cannot_move_the_highlight_back() {
        let mut clock = Clock::default();
        clock.sample(1., false);
        clock.seek(7.);
        clock.sample(1.1, false);
        assert_eq!(clock.position_ms(), 7_000);
        clock.sample(7.09, false);
        assert_eq!(clock.position_ms(), 7_090);
    }

    #[test]
    fn the_clock_stops_projecting_when_playback_stops_reporting_progress() {
        let mut clock = Clock::default();
        clock.sample(3., true);
        clock.sampled_at -= Duration::from_secs(2);
        assert_eq!(clock.position_ms(), 3_150);
    }
}
