use std::time::{Duration, Instant};

use gpui::{Pixels, ScrollHandle, point, px};

use crate::ui::design_system::motion::out_cubic;

const ANCHOR: f32 = 0.36;
pub(super) const TRAVEL: Duration = Duration::from_millis(280);

#[derive(Default)]
pub(super) struct FollowScroll {
    pending: Option<ScrollIntent>,
    glide: Option<Glide>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ScrollIntent {
    Glide,
    Snap,
}

struct Glide {
    from: Pixels,
    to: Pixels,
    began: Instant,
}

impl FollowScroll {
    pub(super) fn request(&mut self, intent: ScrollIntent) {
        if self.pending != Some(ScrollIntent::Snap) {
            self.pending = Some(intent);
        }
    }

    pub(super) fn busy(&self) -> bool {
        self.pending.is_some() || self.glide.is_some()
    }

    pub(super) fn advance(
        &mut self,
        scroll: &ScrollHandle,
        line: Option<usize>,
        reduced: bool,
    ) -> bool {
        let now = Instant::now();
        if let Some(intent) = self.pending
            && let Some(target) = line.and_then(|line| offset_for_line(scroll, line))
        {
            self.pending = None;
            if reduced || intent == ScrollIntent::Snap {
                self.glide = None;
                return set_offset(scroll, target);
            }
            let from = scroll.offset().y;
            self.glide = ((target - from).abs() > px(0.5)).then_some(Glide {
                from,
                to: target,
                began: now,
            });
        }
        let Some(glide) = self.glide.as_ref() else {
            return false;
        };
        let fraction = if reduced {
            1.
        } else {
            (now.duration_since(glide.began).as_secs_f32() / TRAVEL.as_secs_f32()).min(1.)
        };
        let ease_out = out_cubic(fraction);
        let next = glide.from + (glide.to - glide.from) * ease_out;
        if fraction >= 1. {
            self.glide = None;
        }
        set_offset(scroll, next)
    }
}

fn offset_for_line(scroll: &ScrollHandle, line: usize) -> Option<Pixels> {
    let item = scroll.bounds_for_item(line)?;
    let viewport = scroll.bounds();
    let anchor = viewport.top() + viewport.size.height * ANCHOR;
    let offset = anchor - item.top() - item.size.height * 0.5;
    Some(offset.clamp(-scroll.max_offset().y, px(0.)))
}

fn set_offset(scroll: &ScrollHandle, next: Pixels) -> bool {
    let current = scroll.offset();
    if current.y == next {
        return false;
    }
    scroll.set_offset(point(current.x, next));
    true
}
