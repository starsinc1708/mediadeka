use std::collections::VecDeque;

use gpui::{Context, Div, Role, Stateful, div, prelude::*, px};

use crate::providers::Home;

use super::{FeedContext, Layout, MainView, SHELF_OVERSCAN, ShelfSurface, section, shelf_gap};

impl MainView {
    pub(in crate::ui::home::content) fn combined_feed<'a>(
        &self,
        feeds: impl Iterator<Item = (FeedContext<'a>, &'a Home)>,
        layout: Layout,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let stacks = feeds
            .map(|(source, home)| {
                self.positioned_shelves(home, source.provider, ShelfSurface::Combined, layout)
                    .map(move |shelf| (source, shelf))
            })
            .collect();
        let scroll_top =
            -f32::from(self.scroll.offset().y) - self.shelf_origin(ShelfSurface::Combined);
        let viewport = f32::from(self.scroll.bounds().size.height);
        let mut top = 0.;
        section()
            .relative()
            .id("home-feed")
            .accessibility_id("home-feed")
            .role(Role::Group)
            .aria_label("Подборки площадок")
            .gap(px(shelf_gap(layout)))
            .child(self.track_shelf_origin(ShelfSurface::Combined))
            .children(interleaved(stacks).map(|(source, placed)| {
                let bottom = top + placed.height;
                let nearby = viewport <= 0.
                    || (bottom >= scroll_top - SHELF_OVERSCAN
                        && top <= scroll_top + viewport + SHELF_OVERSCAN);
                top = bottom + shelf_gap(layout);
                let child = if nearby {
                    self.render_shelf(source, &placed.shelf, placed.page, placed.tabs, cx)
                } else {
                    div().w_full()
                };
                child.h(px(placed.height)).flex_none()
            }))
    }
}

fn interleaved<I: Iterator>(mut stacks: VecDeque<I>) -> impl Iterator<Item = I::Item> {
    std::iter::from_fn(move || {
        while let Some(mut next) = stacks.pop_front() {
            if let Some(item) = next.next() {
                stacks.push_back(next);
                return Some(item);
            }
        }
        None
    })
}

#[cfg(test)]
#[test]
fn interleaves_feeds_without_changing_each_sources_shelf_order() {
    let shelves = [vec!["A1", "A2", "A3"], vec![], vec!["C1"], vec!["D1", "D2"]]
        .into_iter()
        .map(Vec::into_iter)
        .collect();
    assert_eq!(
        interleaved(shelves).collect::<Vec<_>>(),
        ["A1", "C1", "D1", "A2", "D2", "A3"]
    );
}
