use std::{fs, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, WindowHandle};
use mediadeka::MainView;
use serde_json::json;

use super::require;

// This rectangle contains only the shader in the desktop library with the queue closed.
const REGION: [u32; 4] = [1370, 80, 1700, 185];

pub(super) fn verify(
    window: WindowHandle<MainView>,
    output: &Path,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = AnyWindowHandle::from(window);
    let mut previous: Option<Vec<[u8; 4]>> = None;
    let mut changes = Vec::new();
    let mut samples = Vec::new();
    for (wall_ms, active, evidence) in [
        (0, true, None),
        (2_000, true, Some("atmosphere-active.png")),
        (7_000, false, None),
        (77_000, false, Some("atmosphere-frozen.png")),
        (80_000, true, None),
        (81_500, true, Some("atmosphere-resumed.png")),
    ] {
        window.update(cx, |view, _, cx| {
            view.sample_fluid_glass_activity_for_capture(
                Duration::from_millis(wall_ms),
                active,
                cx,
            );
        })?;
        let frame = raw.update(cx, |_, window, cx| {
            window.draw(cx).clear(cx);
            window.render_to_image()
        })??;
        require(
            frame.dimensions() == (1920, 1080),
            "Проверка фазы Fluid Glass ожидает кадр 1920 × 1080.",
        )?;
        let [left, top, right, bottom] = REGION;
        let pixels = (top..bottom)
            .flat_map(|y| (left..right).map(move |x| (x, y)))
            .map(|(x, y)| frame.get_pixel(x, y).0)
            .collect::<Vec<_>>();
        let changed = previous.as_ref().map(|previous| {
            previous
                .iter()
                .zip(&pixels)
                .filter(|(before, after)| before != after)
                .count()
        });
        previous = Some(pixels);
        changes.push(changed);
        samples.push(json!({
            "wall_ms": wall_ms,
            "active": active,
            "changed_pixels_from_previous": changed,
            "frame": evidence,
        }));
        if let Some(path) = evidence {
            frame.save(output.join(path))?;
        }
    }
    fs::write(
        output.join("atmosphere-motion.json"),
        serde_json::to_vec_pretty(&json!({
            "region_exclusive_right_bottom": REGION,
            "samples": samples,
        }))?,
    )?;
    require(
        matches!(
            changes.as_slice(),
            [None, Some(moving), Some(0), Some(0), Some(0), Some(resumed)]
                if *moving > 0 && *resumed > 0
        ),
        "Деактивация и первый кадр после возврата должны сохранить фазу атмосферы.",
    )?;
    println!("Fluid Glass motion checked, changed background pixels per frame {changes:?}");
    Ok(())
}
