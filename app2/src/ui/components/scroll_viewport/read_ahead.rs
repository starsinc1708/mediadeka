use std::{
    ops::Range,
    time::{Duration, Instant},
};

const MAX_ENTRIES: usize = 48;
const FAST_SCREEN_TIME: Duration = Duration::from_millis(200);

#[derive(Default)]
pub(in crate::ui) struct ReadAhead {
    start: usize,
    backwards: bool,
    last_move: Option<Instant>,
    fast: bool,
}

impl ReadAhead {
    pub(in crate::ui) fn seed(&mut self, first: usize) {
        self.start = first;
        self.last_move = None;
        self.fast = false;
    }

    pub(in crate::ui) fn range(&mut self, visible: Range<usize>, len: usize) -> Range<usize> {
        self.range_at(visible, len, Instant::now())
    }

    fn range_at(&mut self, visible: Range<usize>, len: usize, now: Instant) -> Range<usize> {
        let start = visible.start.min(len);
        let end = visible.end.clamp(start, len);
        let screen = (end - start).max(1);
        let distance = start.abs_diff(self.start);
        if distance > 0 {
            self.fast = distance >= screen
                || self.last_move.is_some_and(|last| {
                    now.duration_since(last)
                        <= FAST_SCREEN_TIME.mul_f64(distance as f64 / screen as f64)
                });
            self.backwards = start < self.start;
            self.start = start;
            self.last_move = Some(now);
        }
        let screens = if self.fast { 2 } else { 1 };
        let entries = screen.saturating_mul(screens).min(MAX_ENTRIES);
        if self.backwards {
            start.saturating_sub(entries)..start
        } else {
            end..end.saturating_add(entries).min(len)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scales_the_window_with_visible_entries_speed_and_direction() {
        let start = Instant::now();
        let mut ahead = ReadAhead::default();
        ahead.seed(50);
        assert_eq!(ahead.range_at(51..57, 200, start), 57..63);
        let fast = start + Duration::from_millis(20);
        assert_eq!(ahead.range_at(53..59, 200, fast), 59..71);
        let slow = fast + Duration::from_millis(400);
        assert_eq!(ahead.range_at(52..58, 200, slow), 46..52);
        assert_eq!(ahead.range_at(2..8, 200, slow), 0..2);
    }

    #[test]
    fn caps_large_viewports_and_clamps_to_the_list_end() {
        let start = Instant::now();
        let mut ahead = ReadAhead::default();
        assert_eq!(ahead.range_at(100..140, 1000, start), 140..188);
        assert_eq!(ahead.range_at(900..966, 1000, start), 966..1000);
    }
}
