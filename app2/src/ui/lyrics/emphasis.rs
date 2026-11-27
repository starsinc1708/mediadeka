use std::time::Instant;

use crate::ui::design_system::motion::smoothstep;

use super::follow;

#[derive(Default)]
pub(super) struct Emphasis {
    previous: Vec<(usize, f32)>,
    target: Option<usize>,
    started: Option<Instant>,
    #[cfg(feature = "capture")]
    sample: Option<std::time::Duration>,
}

impl Emphasis {
    pub(super) fn select(&mut self, line: Option<usize>, now: Instant) {
        let mut previous: Vec<_> = self
            .previous
            .iter()
            .map(|(index, _)| (*index, self.amount(*index, now, false)))
            .filter(|(_, amount)| *amount > 0.01)
            .collect();
        if let Some(target) = self.target
            && !previous.iter().any(|(index, _)| *index == target)
        {
            previous.push((target, self.amount(target, now, false)));
        }
        self.previous = previous;
        self.target = line;
        self.started = Some(now);
    }

    pub(super) fn running(&self, now: Instant) -> bool {
        #[cfg(feature = "capture")]
        if self.sample.is_some() {
            return false;
        }
        self.started
            .is_some_and(|start| now.saturating_duration_since(start) < follow::TRAVEL)
    }

    pub(super) fn amount(&self, line: usize, now: Instant, reduced: bool) -> f32 {
        let goal = f32::from(u8::from(self.target == Some(line)));
        let Some(started) = self.started.filter(|_| !reduced) else {
            return goal;
        };
        let elapsed = now.saturating_duration_since(started);
        #[cfg(feature = "capture")]
        let elapsed = self.sample.unwrap_or(elapsed);
        if elapsed >= follow::TRAVEL {
            return goal;
        }
        let from = self
            .previous
            .iter()
            .find_map(|&(index, amount)| (index == line).then_some(amount))
            .unwrap_or(0.);
        from + (goal - from) * smoothstep(elapsed.as_secs_f32() / follow::TRAVEL.as_secs_f32())
    }

    #[cfg(feature = "capture")]
    pub(super) fn sample(&mut self, elapsed: Option<std::time::Duration>) {
        self.sample = elapsed;
    }
}
