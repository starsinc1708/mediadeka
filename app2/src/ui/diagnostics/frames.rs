use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use gpui::{FrameEvent, FrameTiming, FrameTimingCollector, WindowId};

const SAMPLE_WINDOW: Duration = Duration::from_secs(1);
const IDLE_GAP: Duration = Duration::from_millis(250);
const MAX_FRAMES: usize = 1_024;

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FrameSample {
    pub fps: Option<f64>,
    pub cpu_p95_ms: Option<f64>,
    pub interval_p95_ms: Option<f64>,
    pub over_budget_percent: Option<f64>,
}

pub(super) struct FrameHistory {
    window_id: WindowId,
    started_at: Instant,
    collector: FrameTimingCollector,
    frames: VecDeque<FrameTiming>,
    trace_enabled_here: bool,
}

impl FrameHistory {
    pub(super) fn new(window_id: WindowId) -> Self {
        Self {
            window_id,
            started_at: Instant::now(),
            collector: FrameTimingCollector::new(),
            frames: VecDeque::new(),
            trace_enabled_here: gpui::set_trace_enabled(true),
        }
    }

    pub(super) fn sample(&mut self, refresh_interval: Option<Duration>) -> FrameSample {
        let now = Instant::now();
        while self
            .frames
            .front()
            .is_some_and(|frame| now.saturating_duration_since(frame.draw_end) > SAMPLE_WINDOW)
        {
            self.frames.pop_front();
        }
        for event in self.collector.collect_unseen() {
            if let FrameEvent::Draw(frame) = event
                && frame.window_id == self.window_id
                && now.saturating_duration_since(frame.draw_end) <= SAMPLE_WINDOW
            {
                if self.frames.len() == MAX_FRAMES {
                    self.frames.pop_front();
                }
                self.frames.push_back(frame);
            }
        }

        let elapsed = now
            .saturating_duration_since(self.started_at)
            .min(SAMPLE_WINDOW);
        if self.frames.is_empty() || elapsed.is_zero() {
            return FrameSample::default();
        }
        let mut draw_times: Vec<_> = self.frames.iter().map(FrameTiming::draw_duration).collect();
        let over_budget_percent =
            refresh_interval
                .filter(|interval| !interval.is_zero())
                .map(|budget| {
                    let over_budget = draw_times
                        .iter()
                        .filter(|&&duration| duration > budget)
                        .count();
                    100. * over_budget as f64 / draw_times.len() as f64
                });
        let mut intervals: Vec<_> = self
            .frames
            .iter()
            .zip(self.frames.iter().skip(1))
            .filter_map(|(previous, current)| {
                let became_dirty = current.dirty_at.unwrap_or(current.draw_start);
                if became_dirty.saturating_duration_since(previous.draw_end) > IDLE_GAP {
                    return None;
                }
                let interval = current
                    .draw_end
                    .saturating_duration_since(previous.draw_end);
                (!interval.is_zero()).then_some(interval)
            })
            .collect();
        FrameSample {
            fps: Some(self.frames.len() as f64 / elapsed.as_secs_f64()),
            cpu_p95_ms: p95_milliseconds(&mut draw_times),
            interval_p95_ms: p95_milliseconds(&mut intervals),
            over_budget_percent,
        }
    }
}

impl Drop for FrameHistory {
    fn drop(&mut self) {
        if self.trace_enabled_here {
            gpui::set_trace_enabled(false);
        }
    }
}

fn p95_milliseconds(samples: &mut [Duration]) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let rank = (samples.len() * 95).div_ceil(100) - 1;
    let (_, value, _) = samples.select_nth_unstable(rank);
    Some(value.as_secs_f64() * 1_000.)
}
