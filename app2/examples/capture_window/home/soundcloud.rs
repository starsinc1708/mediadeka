use std::{io, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle, px};
use mediadeka::MainView;

use super::{Scene, draw};

pub(super) async fn capture_pages(
    view: WindowHandle<MainView>,
    raw: AnyWindowHandle,
    scene: Scene,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let (home_tops, activity_top, library_tops) = view.update(cx, |view, _, _| {
        view.soundcloud_home_tops_for_capture(gpui::size(px(scene.width), px(scene.height)))
    })?;
    if home_tops.is_empty() && activity_top.is_none() && library_tops.is_empty() {
        return Err(io::Error::other("GPUI: главная SoundCloud без полок.").into());
    }
    draw(raw, scene, None, cx)?;
    cx.background_executor().timer(Duration::from_secs(2)).await;
    let path = output.join(format!("soundcloud-{}-home.png", scene.name));
    draw(raw, scene, Some(&path), cx)?;

    let origin = view.update(cx, |view, _, _| view.home_shelf_origin_for_capture())?;
    let mut scrolled = false;
    if let Some((_, top)) = home_tops.iter().find(|(title, _)| title == "Buzzing") {
        scroll_to(view, origin + top - 12., cx)?;
        draw(raw, scene, None, cx)?;
        cx.background_executor().timer(Duration::from_secs(2)).await;
        let path = output.join(format!("soundcloud-{}-buzzing.png", scene.name));
        draw(raw, scene, Some(&path), cx)?;
        scrolled = true;
    }
    if let Some(top) = activity_top {
        scroll_to(view, origin + top - 12., cx)?;
        draw(raw, scene, None, cx)?;
        cx.background_executor().timer(Duration::from_secs(2)).await;
        let path = output.join(format!("soundcloud-{}-activity.png", scene.name));
        draw(raw, scene, Some(&path), cx)?;
        scrolled = true;
    }
    if let Some((_, top)) = library_tops.first() {
        scroll_to(view, origin + top - 12., cx)?;
        draw(raw, scene, None, cx)?;
        cx.background_executor().timer(Duration::from_secs(2)).await;
        let path = output.join(format!("soundcloud-{}-library.png", scene.name));
        draw(raw, scene, Some(&path), cx)?;
        scrolled = true;
    }
    if !scrolled {
        return Err(io::Error::other("GPUI: полки SoundCloud не найдены.").into());
    }
    Ok(())
}

fn scroll_to(view: WindowHandle<MainView>, offset: f32, cx: &mut AsyncApp) -> gpui::Result<()> {
    view.update(cx, |view, _, cx| {
        view.set_home_scroll_offset_for_capture(offset, cx)
    })?;
    Ok(())
}
