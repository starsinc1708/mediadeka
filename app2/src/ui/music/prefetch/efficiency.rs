use std::collections::VecDeque;

#[cfg(all(test, windows, feature = "capture"))]
mod native;

const HISTORY_LIMIT: usize = 32;

pub(super) struct Efficiency<R> {
    epoch: u64,
    pending: VecDeque<(R, u64)>,
    consumed: VecDeque<R>,
    pub(super) completed: u64,
    pub(super) useful: u64,
    pub(super) unused_bytes: u64,
}

impl<R> Default for Efficiency<R> {
    fn default() -> Self {
        Self {
            epoch: 0,
            pending: VecDeque::new(),
            consumed: VecDeque::new(),
            completed: 0,
            useful: 0,
            unused_bytes: 0,
        }
    }
}

impl<R: Eq> Efficiency<R> {
    pub(super) fn epoch(&self) -> u64 {
        self.epoch
    }

    pub(super) fn finished(&mut self, epoch: u64, bytes: u64, receipt: Option<R>) {
        if bytes == 0 {
            // An existing snapshot or another worker's singleflight result.
            return;
        }
        self.unused_bytes += bytes;
        let Some(receipt) = receipt else {
            return;
        };
        self.completed += 1;
        if epoch != self.epoch {
            return;
        }
        if let Some(index) = self.consumed.iter().position(|seen| *seen == receipt) {
            self.consumed.remove(index);
            self.credit(bytes);
        } else {
            if self.pending.len() == HISTORY_LIMIT {
                self.pending.pop_front();
            }
            self.pending.push_back((receipt, bytes));
        }
    }

    pub(super) fn observed(&mut self, receipt: R, flight_running: bool) {
        if let Some(index) = self.pending.iter().position(|(seen, _)| *seen == receipt) {
            let (_, bytes) = self.pending.remove(index).expect("matched receipt exists");
            self.credit(bytes);
        } else if flight_running && !self.consumed.contains(&receipt) {
            if self.consumed.len() == HISTORY_LIMIT {
                self.consumed.pop_front();
            }
            self.consumed.push_back(receipt);
        }
    }

    fn credit(&mut self, bytes: u64) {
        self.useful += 1;
        self.unused_bytes -= bytes;
    }

    pub(super) fn invalidate(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.pending.clear();
        self.consumed.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_use_credits_an_exact_collected_snapshot_before_or_after_completion() {
        let mut efficiency = Efficiency::default();
        efficiency.finished(0, 140, Some(("A", 1)));
        efficiency.observed(("A", 0), false); // Old cache entry for the exact same route.
        efficiency.observed(("B", 1), false); // Same timestamp in a different scope.
        assert_eq!((efficiency.useful, efficiency.unused_bytes), (0, 140));
        efficiency.observed(("A", 1), false);
        efficiency.observed(("A", 1), false);
        efficiency.observed(("B", 3), true); // Foreground received the shared result first.
        efficiency.finished(0, 80, Some(("B", 3)));
        efficiency.observed(("C", 2), true);
        efficiency.finished(0, 0, Some(("C", 2))); // The speculative worker only joined.
        assert_eq!(
            (
                efficiency.completed,
                efficiency.useful,
                efficiency.unused_bytes
            ),
            (2, 2, 0)
        );
    }

    #[test]
    fn failures_retired_generations_and_evicted_receipts_remain_uncredited() {
        let mut efficiency = Efficiency::default();
        let old = efficiency.epoch();
        efficiency.finished(old, 20, Some(0));
        efficiency.invalidate();
        efficiency.finished(old, 22, Some(1)); // Response raced with clearing/reconnecting.
        efficiency.finished(old, 12, None); // Cancellation/failure after reading a body.
        efficiency.observed(0, false);
        efficiency.observed(1, false);
        for receipt in 2..=HISTORY_LIMIT + 2 {
            efficiency.finished(efficiency.epoch(), 3, Some(receipt));
        }
        assert_eq!(efficiency.pending.len(), HISTORY_LIMIT);
        efficiency.observed(2, false);
        efficiency.observed(3, false);
        assert_eq!(efficiency.useful, 1);
        assert_eq!(efficiency.unused_bytes, 54 + 3 * HISTORY_LIMIT as u64);
    }
}
