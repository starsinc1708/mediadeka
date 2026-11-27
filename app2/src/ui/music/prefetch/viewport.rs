use std::{cell::RefCell, ops::Range, rc::Rc, time::Duration};

use gpui::{
    AnyElement, Bounds, Context, IntoElement, ListOffset, ListState, Pixels, Styled, Task, canvas,
    px,
};

use crate::ui::components::scroll_viewport::ReadAhead;

use super::{Destination, Key, MainView};

#[cfg(all(test, windows, feature = "capture"))]
mod native_collection_search;
#[cfg(all(test, windows, feature = "capture"))]
mod native_playlist;

const VISIBLE_DELAY: Duration = Duration::from_millis(160);
const MAX_VISIBLE_TARGETS: usize = 4;

#[derive(Default)]
pub(super) struct Viewport {
    frame: Rc<RefCell<Sample>>,
    visible: Vec<Key>,
    pending: Option<Task<()>>,
}

impl Viewport {
    pub(super) fn visible(&self) -> &[Key] {
        &self.visible
    }
}

#[derive(Default)]
struct Sample {
    current: Vec<Key>,
    previous: Vec<Key>,
    list_offset: Option<ListOffset>,
    read_ahead: ReadAhead,
}

impl Sample {
    fn observe(&mut self, keys: &[Key], bounds: Bounds<Pixels>, mask: Bounds<Pixels>) {
        let visible = bounds.intersect(&mask).size;
        if visible.width <= Pixels::ZERO
            || visible.height <= Pixels::ZERO
            || visible.height < bounds.size.height * 0.25
        {
            return;
        }
        for key in keys {
            if self.current.len() == MAX_VISIBLE_TARGETS {
                break;
            }
            if !self.current.contains(key) {
                self.current.push(key.clone());
            }
        }
    }

    fn take_change(&mut self) -> Option<Vec<Key>> {
        if self.current == self.previous {
            self.current.clear();
            return None;
        }
        self.previous.clone_from(&self.current);
        Some(std::mem::take(&mut self.current))
    }

    fn list_window(
        &mut self,
        list: &ListState,
        prefix: usize,
        rows: usize,
        top_inset: Pixels,
    ) -> Option<(Range<usize>, bool)> {
        let offset = list.logical_scroll_top();
        let previous = self.list_offset.replace(offset);
        if previous.is_some_and(|previous| {
            previous.item_ix == offset.item_ix && previous.offset_in_item == offset.offset_in_item
        }) {
            return None;
        }
        let viewport = list.viewport_bounds();
        let first = offset.item_ix.max(prefix).min(prefix + rows);
        let top = viewport.top() + top_inset;
        let visible = (first..prefix + rows)
            .map_while(|item| list.bounds_for_item(item).map(|bounds| (item, bounds)))
            .take_while(|(_, bounds)| bounds.top() < viewport.bottom())
            .map(|(item, bounds)| (item - prefix, bounds.bottom() > top))
            .skip_while(|(_, exposed)| !exposed)
            .map(|(row, _)| row);
        let mut visible = visible;
        let Some(start) = visible.next() else {
            return previous.map(|_| (0..0, false));
        };
        if previous.is_none() {
            self.read_ahead.seed(start);
            return None;
        }
        let end = visible.last().unwrap_or(start) + 1;
        let next = self.read_ahead.range(start..end, rows);
        let backwards = next.end <= start;
        Some((next, backwards))
    }
}

type ListPrefetch = fn(&mut MainView, Range<usize>, bool, f32, &mut Context<MainView>);

impl MainView {
    /// Placed after the virtual list so it sees GPUI's final, clamped position.
    pub(in crate::ui) fn prefetch_after_list_layout(
        &self,
        list: ListState,
        prefix: usize,
        rows: usize,
        top_inset: Pixels,
        prepare: ListPrefetch,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let frame = Rc::clone(&self.music.prefetch.viewport.frame);
        let view = cx.weak_entity();
        canvas(
            move |_, window, cx| {
                let Some((rows, backwards)) = frame
                    .borrow_mut()
                    .list_window(&list, prefix, rows, top_inset)
                else {
                    return;
                };
                let frame = Rc::clone(&frame);
                let view = view.clone();
                let scale = window.scale_factor();
                window.defer(cx, move |_, cx| {
                    let _ = view.update(cx, |view, cx| {
                        if Rc::ptr_eq(&frame, &view.music.prefetch.viewport.frame) {
                            prepare(view, rows, backwards, scale, cx);
                        }
                    });
                });
            },
            |_, (), _, _| {},
        )
        .absolute()
        .size(px(1.))
    }

    /// Collects noninteractive geometry during prepaint. One parent marker
    /// flushes all observations after the clipped content has been laid out.
    pub(in crate::ui) fn prefetch_visibility_probe(
        &self,
        targets: impl IntoIterator<Item = Destination>,
    ) -> Option<AnyElement> {
        let keys = targets
            .into_iter()
            .filter_map(Key::from_destination)
            .take(MAX_VISIBLE_TARGETS)
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return None;
        }
        let frame = Rc::clone(&self.music.prefetch.viewport.frame);
        Some(
            canvas(
                move |bounds, window, _| {
                    frame
                        .borrow_mut()
                        .observe(&keys, bounds, window.content_mask().bounds);
                },
                |_, (), _, _| {},
            )
            .absolute()
            .inset_0()
            .into_any_element(),
        )
    }

    pub(in crate::ui) fn flush_prefetch_visibility(
        &self,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let frame = Rc::clone(&self.music.prefetch.viewport.frame);
        let view = cx.weak_entity();
        canvas(
            move |_, window, cx| {
                let Some(keys) = frame.borrow_mut().take_change() else {
                    return;
                };
                window.defer(cx, move |_, cx| {
                    let _ = view.update(cx, |view, cx| {
                        let scheduler = &mut view.music.prefetch;
                        if !Rc::ptr_eq(&frame, &scheduler.viewport.frame) {
                            return;
                        }
                        scheduler.viewport.visible.retain(|key| keys.contains(key));
                        scheduler.viewport.pending = None;
                        scheduler.cancel_obsolete();
                        if scheduler.viewport.visible != keys {
                            scheduler.viewport.pending = Some(cx.spawn(async move |view, cx| {
                                cx.background_executor().timer(VISIBLE_DELAY).await;
                                let _ = view.update(cx, |view, cx| {
                                    let scheduler = &mut view.music.prefetch;
                                    scheduler.viewport.pending = None;
                                    scheduler.viewport.visible = keys;
                                    scheduler.drive(cx);
                                });
                            }));
                        }
                        scheduler.drive(cx);
                    });
                });
            },
            |_, (), _, _| {},
        )
        .absolute()
        .size(px(1.))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{point, size};

    use super::*;
    use crate::{
        music::{ObjectKind, ObjectRef},
        providers::ProviderId,
    };

    fn track(id: &str) -> Key {
        Key {
            object: ObjectRef::native(ProviderId::YandexMusic, ObjectKind::Track, id.into())
                .unwrap(),
            profile: None,
        }
    }

    #[test]
    fn clips_overscan_deduplicates_targets_and_retires_the_previous_window() {
        let mut sample = Sample::default();
        let viewport = Bounds::new(point(px(20.), px(30.)), size(px(100.), px(80.)));
        let row = |y| Bounds::new(point(px(20.), px(y)), size(px(100.), px(40.)));
        sample.observe(&[track("10")], row(106.), viewport);
        sample.observe(
            &[
                track("11"),
                track("11"),
                track("12"),
                track("13"),
                track("14"),
                track("15"),
            ],
            row(40.),
            viewport,
        );
        let first = sample.take_change().unwrap();
        assert!(
            first
                .into_iter()
                .map(|key| key.object.id)
                .eq(["11", "12", "13", "14"])
        );
        sample.observe(
            &[track("11"), track("12"), track("13"), track("14")],
            row(40.),
            viewport,
        );
        assert!(sample.take_change().is_none());
        assert!(sample.take_change().is_some_and(|change| change.is_empty()));
    }
}
