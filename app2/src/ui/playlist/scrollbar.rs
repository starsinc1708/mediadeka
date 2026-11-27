use gpui::{
    AccessibleAction, Context, Div, KeyDownEvent, Stateful, accesskit::ActionData, prelude::*, px,
};

use crate::ui::components::scrollbar::{ScrollChrome, ScrollSurface, scrollbar_element};

use super::{MainView, page::ROW_HEIGHT};

pub(super) fn scrollbar(
    cx: &Context<MainView>,
    surface: &ScrollSurface,
    chrome: ScrollChrome,
) -> Stateful<Div> {
    let set = cx.weak_entity();
    scrollbar_element("playlist-scrollbar", "Прокрутка плейлиста", surface, chrome)
        .on_key_down(cx.listener(|view, event: &KeyDownEvent, _, cx| {
            let surface = ScrollSurface::VirtualList(view.playlist.list.clone());
            let row_height = if view.uses_fluid_glass() {
                view.track_density.row_height()
            } else {
                ROW_HEIGHT
            };
            let Some(target) = surface.offset_for_key(&event.keystroke.key, px(row_height)) else {
                return;
            };
            if surface.set_offset(target) {
                cx.notify();
            }
            cx.stop_propagation();
        }))
        .on_a11y_action(AccessibleAction::SetValue, move |data, _, cx| {
            if let Some(ActionData::NumericValue(position)) = data
                && position.is_finite()
            {
                let _ = set.update(cx, |view, cx| {
                    let surface = ScrollSurface::VirtualList(view.playlist.list.clone());
                    if surface.set_offset(px(*position as f32)) {
                        cx.notify();
                    }
                });
            }
        })
}
