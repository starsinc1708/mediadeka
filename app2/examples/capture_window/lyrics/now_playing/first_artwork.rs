use std::{io, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::json;

use super::record;

const MAX_ORIGIN_DELTA_PX: f32 = 1.;

pub(super) fn sample(
    view: WindowHandle<MainView>,
    phase_ms: u64,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    view.update(cx, |view, _, cx| {
        view.sample_listening_artwork_for_capture(Some(Duration::from_millis(phase_ms)), cx);
    })
}

pub(super) fn capture(
    view: WindowHandle<MainView>,
    output: &Path,
    step: &str,
    phase_ms: u64,
    cx: &mut AsyncApp,
) -> gpui::Result<[f32; 4]> {
    let (bounds, exact_size_ready, scale) = view.update(cx, |view, window, cx| {
        (
            view.listening_artwork_bounds_for_capture(window),
            view.listening_artwork_decoded_for_capture(window, cx),
            window.scale_factor(),
        )
    })?;
    let frame =
        AnyWindowHandle::from(view).update(cx, |_, window, _| window.render_to_image())??;
    let name = format!("{step}-artwork-{phase_ms:03}ms");
    frame.save(output.join(format!("{name}.png")))?;
    let physical = bounds.map(|bounds| {
        [bounds.left(), bounds.top(), bounds.right(), bounds.bottom()]
            .map(|edge| f32::from(edge) * scale)
    });
    record(
        output,
        &name,
        &json!({"bounds_px": physical, "exact_size_ready": exact_size_ready}),
    )?;
    physical
        .ok_or_else(|| io::Error::other(format!("В кадре {name} нет геометрии обложки.")).into())
}

pub(super) fn check_origin(output: &Path, cover: [f32; 4], text: [f32; 4]) -> gpui::Result<()> {
    let delta: [f32; 4] = std::array::from_fn(|index| (cover[index] - text[index]).abs());
    record(
        output,
        "artwork-cover-to-text",
        &json!({"cover_px": cover, "text_px": text, "edge_delta_px": delta,
            "tolerance_px": MAX_ORIGIN_DELTA_PX}),
    )?;
    if delta.into_iter().any(|edge| edge > MAX_ORIGIN_DELTA_PX) {
        return Err(io::Error::other(format!(
            "При переходе к тексту изменились границы обложки: {cover:?} → {text:?}."
        ))
        .into());
    }
    Ok(())
}
