use std::{fs, io, path::Path, time::Duration};

use gpui::{
    AsyncApp, Bounds, InputEvent as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    Pixels, WindowHandle,
};
use image::RgbaImage;
use mediadeka::MainView;
use serde_json::json;

use super::super::super::super::render_image;
use super::super::super::{Scene, named, node_label, press, require, snapshot, wait};

const SAMPLES_MS: [u64; 5] = [0, 105, 210, 315, 420];
const SOURCE: &str = "library-album-0";
const HEADING: &str = "music-liked-title";

pub(in super::super) async fn verify_card(
    handle: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    title: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = handle.into();
    let animated = handle.update(cx, |view, window, cx| {
        let quality = view.fluid_glass_quality_for_capture(window, cx);
        !scene.reduced && quality.is_some_and(|quality| quality.0 != "static")
    })?;
    let (grid, source) = wait(
        cx,
        "готовая обложка первой карточки альбома",
        |cx| {
            let grid = render_image(raw, scene, cx)?;
            let source = handle.update(cx, |view, window, _| {
                view.first_album_tile_cover_for_capture(window)
            })?;
            Ok(source
                .filter(|&source| !animated || cover_marker(&grid, raster_bounds(source)).is_ok())
                .map(|source| (grid, source)))
        },
    )
    .await?;
    let point = source.center();
    raw.update(cx, |_, window, cx| {
        for event in [
            MouseMoveEvent {
                position: point,
                ..Default::default()
            }
            .to_platform_input(),
            MouseDownEvent {
                position: point,
                button: MouseButton::Left,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
            MouseUpEvent {
                position: point,
                button: MouseButton::Left,
                click_count: 1,
                ..Default::default()
            }
            .to_platform_input(),
        ] {
            window.dispatch_event(event, cx);
        }
    })?;
    wait(
        cx,
        "открытие альбома нажатием на обложку",
        |cx| {
            let tree = snapshot(raw, cx)?;
            Ok((node_label(&tree, HEADING) == Some(title)
                && node_label(&tree, "library-group-play") == Some("Слушать"))
            .then_some(()))
        },
    )
    .await?;
    let bounds = raster_bounds(source);
    let report = if animated {
        grid.save(output.join("album-cover-motion-card.png"))?;
        record_samples(handle, scene, output, &grid, source, cx)?
    } else {
        require(
            sample(handle, 0, cx)?.is_none(),
            "Ограничение движения должно открыть альбом сразу в конечном положении.",
        )?;
        render_image(raw, scene, cx)?.save(output.join("album-cover-motion-static.png"))?;
        json!({ "animated": false, "source_crop_px": bounds })
    };
    fs::write(
        output.join("album-cover-motion.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    press(raw, &named("workspace-back"), cx)?;
    wait(
        cx,
        "возврат к карточке альбома",
        |cx| {
            Ok(node_label(&snapshot(raw, cx)?, SOURCE)
                .is_some_and(|label| label.starts_with(title))
                .then_some(()))
        },
    )
    .await?;
    println!(
        "{}: album card pointer transition, animated {animated}",
        scene.name
    );
    Ok(())
}

pub(in super::super) fn verify_keyboard(
    handle: WindowHandle<MainView>,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    require(
        sample(handle, 0, cx)?.is_none(),
        "Enter должен открыть альбом без анимации обложки.",
    )
}

fn sample(
    handle: WindowHandle<MainView>,
    millis: u64,
    cx: &mut AsyncApp,
) -> gpui::Result<Option<Bounds<Pixels>>> {
    handle.update(cx, |view, _, cx| {
        view.sample_hero_motion_for_capture(Duration::from_millis(millis), cx)
    })
}

fn record_samples(
    handle: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    grid: &RgbaImage,
    source: Bounds<Pixels>,
    cx: &mut AsyncApp,
) -> gpui::Result<serde_json::Value> {
    let bounds = raster_bounds(source);
    let (source_width, source_height) = bounds.edge();
    require(
        source_width.abs_diff(source_height) <= 1 && (96..=256).contains(&source_width),
        "Карточка должна иметь квадратную обложку выбранного масштаба.",
    )?;
    let marker = cover_marker(grid, bounds)?;
    let mut frames = Vec::with_capacity(SAMPLES_MS.len());
    let mut sizes = Vec::with_capacity(SAMPLES_MS.len());
    for millis in SAMPLES_MS {
        require(
            sample(handle, millis, cx)? == Some(source),
            "Переход должен использовать границы той карточки, по которой нажали.",
        )?;
        let image = render_image(handle.into(), scene, cx)?;
        let name = format!("album-cover-motion-{millis:03}ms.png");
        image.save(output.join(&name))?;
        let painted = handle
            .update(cx, |view, window, _| {
                view.rendered_hero_artwork_for_capture(window)
            })?
            .map(raster_bounds)
            .ok_or_else(|| io::Error::other("В кадре отсутствует paint span обложки Hero."))?;
        let (width, height) = painted.edge();
        let observed = cover_marker(&image, painted)?;
        // Use the internal RGB grid of each actual cover. A pigment detector
        // can shift at the rounded edge or pick up the coloured atmosphere.
        let difference = sample_difference(grid, bounds, &image, painted);
        if millis == 0 {
            require(
                painted == bounds && difference <= 9.,
                &format!(
                    "В начале должен совпадать рисунок карточки. Границы {bounds:?} → {painted:?}, RGB {difference:.2}."
                ),
            )?;
        }
        require(
            width.abs_diff(height) <= 1,
            &format!("Обложка сохраняет пропорции в {millis} мс: {width}×{height}."),
        )?;
        require(
            difference <= 14.,
            &format!("Рисунок карточки должен сохраняться в {millis} мс: RGB {difference:.2}."),
        )?;
        sizes.push(width);
        frames.push(json!({
            "elapsed_ms": millis,
            "frame": name,
            "painted_marker_px": observed,
            "painted_cover_px": painted,
            "painted_cover_edge_px": [width, height],
            "mean_rgb_delta_vs_card": difference,
        }));
    }
    require(
        sizes.windows(2).all(|pair| pair[0] < pair[1])
            && sizes.last().is_some_and(|edge| (276..=380).contains(edge)),
        &format!("Обложка должна вырасти от карточки до полноразмерного квадрата: {sizes:?}."),
    )?;
    Ok(json!({
        "animated": true,
        "source_crop_px": bounds,
        "source_pattern_marker_px": marker,
        "frames": frames,
    }))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
struct Rect {
    left: u32,
    top: u32,
    right: u32,
    bottom: u32,
}

impl Rect {
    fn edge(self) -> (u32, u32) {
        (self.right - self.left, self.bottom - self.top)
    }
}

fn raster_bounds(bounds: Bounds<Pixels>) -> Rect {
    Rect {
        left: f32::from(bounds.left()).round() as u32,
        top: f32::from(bounds.top()).round() as u32,
        right: f32::from(bounds.right()).round() as u32,
        bottom: f32::from(bounds.bottom()).round() as u32,
    }
}

fn cover_marker(frame: &RgbaImage, region: Rect) -> gpui::Result<Rect> {
    let mut marker = Rect {
        left: region.right,
        top: region.bottom,
        right: 0,
        bottom: 0,
    };
    for y in region.top..region.bottom {
        for x in region.left..region.right {
            let [r, g, b, _] = frame.get_pixel(x, y).0;
            // Sample the bright pigment of the cover. The dim workspace and
            // the nearby icon highlights can have the same hue.
            if r > 100 && r.saturating_sub(b) > 23
                || b > 100 && b.saturating_sub(r) > 32 && g.saturating_sub(r) > 14
            {
                marker.left = marker.left.min(x);
                marker.top = marker.top.min(y);
                marker.right = marker.right.max(x + 1);
                marker.bottom = marker.bottom.max(y + 1);
            }
        }
    }
    require(
        marker.left < marker.right && marker.top < marker.bottom,
        "Рисунок альбома отсутствует в кадре.",
    )?;
    Ok(marker)
}

fn sample_difference(a: &RgbaImage, source: Rect, b: &RgbaImage, target: Rect) -> f32 {
    const GRID: u32 = 7;
    let absolute = |frame: &RgbaImage, box_: Rect, x: u32, y: u32| {
        let (w, h) = box_.edge();
        frame
            .get_pixel(
                box_.left + w * x / (GRID + 1),
                box_.top + h * y / (GRID + 1),
            )
            .0
    };
    let total: u32 = (1..=GRID)
        .flat_map(|y| (1..=GRID).map(move |x| (x, y)))
        .map(|(x, y)| {
            let a = absolute(a, source, x, y);
            let b = absolute(b, target, x, y);
            (0..3)
                .map(|channel| u32::from(a[channel].abs_diff(b[channel])))
                .sum::<u32>()
        })
        .sum();
    total as f32 / (GRID * GRID * 3) as f32
}
