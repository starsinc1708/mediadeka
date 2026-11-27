use std::{io, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, MouseButton, WindowHandle, point, px};
use mediadeka::MainView;

use super::{Scene, click, draw, scroll_offset};

const SHELVES: [(&str, &str); 6] = [
    ("В стиле", "in-style"),
    ("Волна открытий", "waves"),
    ("Новые релизы", "new-releases"),
    ("Подборки музыки", "collections"),
    ("Вы могли пропустить", "missed"),
    ("Больше новой музыки", "more-new-music"),
];
const ARTWORK_WAIT: Duration = Duration::from_secs(1);

pub(super) async fn capture_shelves(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let viewport = view.update(cx, |view, _, _| view.home_scroll_bounds_for_capture())?;
    let layout = view.update(cx, |view, _, _| {
        view.home_shelf_geometry_for_capture(gpui::size(px(scene.width), px(scene.height)))
    })?;
    let compact = scene.name == "minimum";
    let feed_y = view.update(cx, |view, _, _| view.home_shelf_origin_for_capture())?;
    let content_x = viewport.left() + px(if compact { 17. } else { 29. });
    for (title, name) in SHELVES {
        let Some((shelf, _, top, tabs)) = layout.iter().find(|shelf| shelf.1 == title) else {
            return Err(io::Error::other(format!("GPUI: не найдена полка «{title}».")).into());
        };
        let shelf_y = feed_y + top;
        view.update(cx, |view, _, cx| {
            view.set_home_scroll_offset_for_capture(shelf_y - 12., cx)
        })?;
        draw(raw, scene, None, cx)?;
        cx.background_executor().timer(ARTWORK_WAIT).await;
        let path = output.join(format!("yandex-{}-{name}.png", scene.name));
        draw(raw, scene, Some(&path), cx)?;

        if matches!(name, "in-style" | "waves" | "new-releases") {
            let [_, (tab, x, y), ..] = tabs.as_slice() else {
                return Err(
                    io::Error::other(format!("GPUI: нет второго фильтра «{title}».")).into(),
                );
            };
            let before = view.update(cx, |view, _, _| {
                view.home_shelf_selected_tab_for_capture(*shelf)
            })?;
            let offset = scroll_offset(view, cx)?;
            let position = point(
                content_x + px(*x),
                viewport.top() + px(shelf_y + y + offset),
            );
            if before == Some(*tab) || !viewport.contains(&position) {
                return Err(io::Error::other(format!(
                    "GPUI: фильтр «{title}» не готов к переключению ({position:?})."
                ))
                .into());
            }
            click(raw, MouseButton::Left, position, cx)?;
            draw(raw, scene, None, cx)?;
            let selected = view.update(cx, |view, _, _| {
                view.home_shelf_selected_tab_for_capture(*shelf)
            })?;
            if selected != Some(*tab) {
                return Err(io::Error::other(format!(
                    "GPUI: щелчок не переключил «{title}» на фильтр {tab}."
                ))
                .into());
            }
            cx.background_executor().timer(ARTWORK_WAIT).await;
            let path = output.join(format!("yandex-{}-{name}-filter.png", scene.name));
            draw(raw, scene, Some(&path), cx)?;
            println!("GPUI: «{title}» выбрал фильтр {tab} ({})", scene.name);
        }
    }
    Ok(())
}
