//! The queue's visual positions while GPUI owns the drag preview.

use std::{collections::HashMap, ops::Range, time::Instant};

use gpui::{Bounds, ListState, Pixels, Point, SpanId, px};

use crate::ui::design_system::motion::{ExitingSpans, MOTION, out_quad, smoothstep};

pub(super) const HEADING_HEIGHT: f32 = 22.;

#[derive(Clone, Copy)]
pub(super) struct Row {
    pub id: usize,
    pub index: usize,
    pub revision: u64,
    pub current: Option<usize>,
}

impl Row {
    pub(super) fn can_move(self) -> bool {
        self.current.is_none_or(|current| self.index > current)
    }
}

#[derive(Clone, Copy)]
pub(super) struct Drag {
    pub row: Row,
    pub height: Pixels,
    pub cursor_offset: Pixels,
    pub target: Option<usize>,
}

#[derive(Default)]
pub(super) struct QueueMotion {
    pub drag: Option<Drag>,
    offsets: HashMap<usize, Offset>,
    frame: Option<Instant>,
    entries: Option<(u64, usize)>,
    inserted: Vec<(Range<usize>, Instant)>,
    pub exits: ExitingSpans,
    #[cfg(feature = "capture")]
    capture_time: Option<(Instant, Instant)>,
}

struct Offset {
    index: usize,
    from: Pixels,
    target: Pixels,
    started: Instant,
    seen: Instant,
}

fn transition_progress(started: Instant, now: Instant) -> f32 {
    let elapsed = now.saturating_duration_since(started).as_secs_f32();
    smoothstep(elapsed / MOTION.normal().as_secs_f32())
}

pub(super) fn queue_span(id: usize) -> SpanId {
    0xe391_8f77_b80a_84ab ^ id as u64
}

impl Offset {
    fn at(&self, now: Instant) -> Pixels {
        self.from + (self.target - self.from) * transition_progress(self.started, now)
    }
}

impl QueueMotion {
    pub(super) fn now(&self) -> Instant {
        self.sample_time().unwrap_or_else(Instant::now)
    }

    pub(super) fn sample_time(&self) -> Option<Instant> {
        #[cfg(feature = "capture")]
        {
            self.capture_time.map(|(_, sample)| sample)
        }
        #[cfg(not(feature = "capture"))]
        {
            None
        }
    }

    #[cfg(feature = "capture")]
    pub(super) fn sample(&mut self, elapsed: std::time::Duration) {
        let origin = self
            .capture_time
            .map_or_else(Instant::now, |(origin, _)| origin);
        self.capture_time = Some((origin, origin + elapsed));
    }

    pub(super) fn prepare(
        &mut self,
        now: Instant,
        dragging: bool,
        revision: u64,
        current: Option<usize>,
    ) {
        if let Some(previous_frame) = self.frame.replace(now) {
            self.offsets
                .retain(|_, offset| offset.seen >= previous_frame);
        }
        self.drag = self.drag.filter(|drag| {
            dragging && drag.row.revision == revision && drag.row.current == current
        });
    }

    pub(super) fn observe_entries(&mut self, replacement: u64, next_id: usize, now: Instant) {
        self.inserted
            .retain(|(_, started)| now.saturating_duration_since(*started) < MOTION.normal());
        match self.entries.replace((replacement, next_id)) {
            Some((previous, first_id)) if previous == replacement && first_id < next_id => {
                self.inserted.push((first_id..next_id, now));
            }
            Some((previous, _)) if previous != replacement => self.inserted.clear(),
            _ => {}
        }
    }

    pub(super) fn appearance(&self, id: usize, now: Instant, reduced: bool) -> f32 {
        if reduced {
            return 1.;
        }
        let Some((_, started)) = self.inserted.iter().find(|(ids, _)| ids.contains(&id)) else {
            return 1.;
        };
        out_quad(
            now.saturating_duration_since(*started).as_secs_f32() / MOTION.normal().as_secs_f32(),
        )
    }

    pub(super) fn offset(&mut self, row: Row, now: Instant, reduced: bool) -> (Pixels, bool) {
        let zero = Pixels::ZERO;
        let target = self
            .drag
            .and_then(|drag| {
                Some(neighbor_offset(
                    row.index,
                    drag.row.index,
                    drag.target?,
                    drag.height,
                ))
            })
            .unwrap_or(zero);
        if reduced {
            self.offsets.remove(&row.id);
            return (target, false);
        }
        let Some(mut offset) = self.offsets.remove(&row.id).or_else(|| {
            (target != zero).then_some(Offset {
                index: row.index,
                from: zero,
                target,
                started: now,
                seen: now,
            })
        }) else {
            return (zero, false);
        };
        let visible = offset.at(now);
        if offset.target != target {
            offset.from = visible;
            offset.target = target;
            offset.started = now;
        }
        let animating = visible != target;
        if animating || target != zero {
            offset.index = row.index;
            offset.seen = now;
            self.offsets.insert(row.id, offset);
        }
        (visible, animating)
    }

    pub(super) fn commit(
        &mut self,
        drag: Drag,
        target: usize,
        list: &ListState,
        cursor_y: Pixels,
        now: Instant,
    ) {
        self.drag = None;
        let source = drag.row.index;
        let zero = Pixels::ZERO;
        // A drop replaces the virtual layout. Keep every previously rendered
        // neighbor at its displayed position and animate the remaining delta.
        for (&id, offset) in &mut self.offsets {
            if id == drag.row.id {
                continue;
            }
            let layout_shift = neighbor_offset(offset.index, source, target, drag.height);
            offset.from = offset.at(now) - layout_shift;
            offset.target = zero;
            offset.started = now;
        }
        let Some(bounds) = row_bounds(list, target, drag.row.current) else {
            return;
        };
        let top = if source < target {
            bounds.bottom() - drag.height
        } else {
            bounds.top()
        };
        self.offsets.insert(
            drag.row.id,
            Offset {
                index: target,
                from: cursor_y - drag.cursor_offset - top,
                target: zero,
                started: now,
                seen: now,
            },
        );
    }

    pub(super) fn remove(
        &mut self,
        id: usize,
        height: Pixels,
        neighbors: Vec<(usize, usize)>,
        now: Instant,
    ) {
        self.offsets.remove(&id);
        for (index, neighbor) in neighbors {
            let from = self
                .offsets
                .get(&neighbor)
                .map_or(Pixels::ZERO, |offset| offset.at(now));
            self.offsets.insert(
                neighbor,
                Offset {
                    index: index - 1,
                    from: from + height,
                    target: Pixels::ZERO,
                    started: now,
                    seen: now,
                },
            );
        }
        self.exits.start(queue_span(id), now);
    }
}

fn neighbor_offset(index: usize, source: usize, target: usize, height: Pixels) -> Pixels {
    if source < index && index <= target {
        -height
    } else if target <= index && index < source {
        height
    } else {
        Pixels::ZERO
    }
}

pub(super) fn row_bounds(
    list: &ListState,
    index: usize,
    current: Option<usize>,
) -> Option<Bounds<Pixels>> {
    let mut bounds = list.bounds_for_item(index)?;
    if current.is_some_and(|current| index == current || index == current + 1) {
        bounds.origin.y += px(HEADING_HEIGHT);
        bounds.size.height -= px(HEADING_HEIGHT);
    }
    Some(bounds)
}

pub(super) fn target_at(list: &ListState, row: Row, pointer: Point<Pixels>) -> Option<usize> {
    let viewport = list.viewport_bounds();
    if !viewport.contains(&pointer) {
        return None;
    }
    let first = list.logical_scroll_top().item_ix;
    let upcoming = row.current.map_or(0, |current| current + 1);
    let mut target = first;
    for index in first..list.item_count() {
        let Some(bounds) = row_bounds(list, index, row.current) else {
            break;
        };
        if bounds.top() >= viewport.bottom() {
            break;
        }
        target = index;
        if pointer.y < bounds.center().y {
            if index > row.index {
                target -= 1;
            }
            break;
        }
        if pointer.y <= bounds.bottom() {
            if index < row.index {
                target += 1;
            }
            break;
        }
    }
    (target >= upcoming).then_some(target)
}
