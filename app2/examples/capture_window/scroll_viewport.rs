use std::{fs, io, path::Path, time::Duration};

use gpui::{AnyWindowHandle, AsyncApp, Bounds, Context, Pixels, WindowHandle};
use image::RgbaImage;
use mediadeka::MainView;
use serde_json::{Value, json};

use super::Scene;
use crate::render_image;

const WALL: Duration = Duration::from_secs(60);
const PHASE: Duration = Duration::from_millis(4600);
type Region = [u32; 4];

#[derive(Clone, Copy)]
pub(super) enum Surface {
    Playlist,
    Workspace,
}

impl Surface {
    fn seek(self, view: &MainView, fraction: f32, cx: &mut Context<MainView>) {
        match self {
            Self::Playlist => view.set_playlist_scroll_fraction_for_capture(fraction, cx),
            Self::Workspace => view.set_home_scroll_offset_for_capture(
                view.home_scroll_extent_for_capture() * fraction,
                cx,
            ),
        }
    }

    pub(super) fn bounds(self, view: &MainView) -> (Bounds<Pixels>, f32, f32) {
        match self {
            Self::Playlist => view.playlist_scroll_bounds_for_capture(),
            Self::Workspace => (
                view.home_scroll_bounds_for_capture(),
                -view.home_scroll_offset_for_capture(),
                view.home_scroll_extent_for_capture(),
            ),
        }
    }
}

pub(super) fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    surface: Surface,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    fs::create_dir_all(output)?;
    let raw = AnyWindowHandle::from(window);
    window.update(cx, |view, _, cx| {
        view.sample_fluid_glass_palette_for_capture(WALL, PHASE, cx);
    })?;
    let mut samples = Vec::with_capacity(3);
    for (name, fraction) in [("start", 0.), ("middle", 0.5), ("end", 1.)] {
        window.update(cx, |view, _, cx| {
            surface.seek(view, fraction, cx);
        })?;
        // The first frame updates scroll-dependent shader presentation; use
        // the following frame for all three otherwise-identical paint passes.
        render_image(raw, scene, cx)?;
        // Real paint is kept for first, middle and last frames. The other two
        // captures change only this viewport's paint, using the same geometry.
        let masked = render_image(raw, scene, cx)?;
        let (bounds, offset, extent) = window.update(cx, |view, window, cx| {
            let (_, offset, extent) = surface.bounds(view);
            let content = match surface {
                Surface::Playlist => view.playlist_scroll_content_bounds_for_capture(window, cx),
                Surface::Workspace => view.home_scroll_content_bounds_for_capture(window, cx),
            };
            (content, offset, extent)
        })?;
        let unmasked = reference(raw, scene, true, cx)?;
        let background = reference(raw, scene, false, cx)?;
        let scale = scene.dpi as f32 / 96.;
        let [top, center, bottom] = strips(bounds, scale, masked.dimensions());
        let bands = [top, center, bottom].map(|region| {
            let total = contribution(&unmasked, &background, region);
            let pixels = u64::from(region[2] - region[0]) * u64::from(region[3] - region[1]);
            // Where only the animated shell is visible there is no signal to
            // attribute to content. Ignore sub-level raster noise.
            let ratio = (total > pixels)
                .then(|| contribution(&masked, &background, region) as f64 / total as f64);
            (total as f64 / pixels as f64, ratio)
        });
        let ratios = bands.map(|(_, ratio)| ratio);
        let signal = bands.map(|(signal, _)| signal);
        for (image, suffix) in [
            (&masked, "masked"),
            (&unmasked, "unmasked"),
            (&background, "background"),
        ] {
            image.save(output.join(format!("scroll-{name}-{suffix}.png")))?;
        }
        samples.push(json!({
            "position": name,
            "offset_dip": offset,
            "extent_dip": extent,
            "strips": [top, center, bottom],
            "unmasked_content_signal_per_pixel": signal,
            "masked_content_contribution_ratio": ratios,
        }));
        let [top, _, bottom] = ratios;
        let intact = |band: usize| {
            let (signal, ratio) = bands[band];
            // Permit sub-channel rounding noise on very dim content. On
            // stronger content the relative error is limited to 10%.
            ratio.is_none_or(|ratio| (ratio - 1.).abs() * signal <= (signal * 0.10).max(0.6))
        };
        let fades = |ratio: f64| ratio < 0.86;
        let proper = match name {
            "start" => offset <= 1. && intact(0) && bottom.is_some_and(fades),
            // A sibling sticky header can cover the top probe. Require
            // attenuation at every visible edge and a signal on at least one.
            "middle" => {
                (top.is_some() || bottom.is_some())
                    && top.is_none_or(fades)
                    && bottom.is_none_or(fades)
            }
            _ => extent - offset <= 1. && top.is_none_or(fades) && intact(2),
        };
        if !proper || !intact(1) {
            save(output, &samples)?;
            return Err(io::Error::other(format!(
                "Затухание списка не соответствует положению {name}: {ratios:?}, {offset}/{extent} DIP."
            ))
            .into());
        }
    }
    save(output, &samples)?;
    window.update(cx, |view, _, cx| {
        surface.seek(view, 0., cx);
        view.resume_fluid_glass_for_capture(cx);
    })?;
    Ok(())
}

fn reference(
    raw: AnyWindowHandle,
    scene: Scene,
    visible: bool,
    cx: &mut AsyncApp,
) -> gpui::Result<RgbaImage> {
    cx.update(|cx| MainView::set_scroll_content_paint_for_capture(false, visible, cx));
    let result = render_image(raw, scene, cx);
    cx.update(|cx| MainView::set_scroll_content_paint_for_capture(true, true, cx));
    result
}

fn strips(bounds: Bounds<Pixels>, scale: f32, image_size: (u32, u32)) -> [Region; 3] {
    let pixel = |dip: Pixels| (f32::from(dip) * scale).round() as u32;
    let [left, top, right, bottom] = [
        pixel(bounds.left()),
        pixel(bounds.top()),
        pixel(bounds.right()).min(image_size.0),
        pixel(bounds.bottom()).min(image_size.1),
    ];
    let mid = top + (bottom - top) / 2;
    let height_dip = (bottom - top) as f32 / scale;
    let center_depth = (14. * scale).round() as u32;
    let top_depth = ((height_dip * 0.045).clamp(14., 26.) * scale).round() as u32;
    let bottom_depth = ((height_dip * 0.06).clamp(19., 48.) * scale).round() as u32;
    // The scrollbar sits at the right edge and paints in a sibling layer.
    let right = right - (20. * scale).ceil() as u32;
    [
        [left, top, right, top + top_depth],
        [left, mid - center_depth, right, mid + center_depth],
        [left, bottom - bottom_depth, right, bottom],
    ]
}

fn contribution(frame: &RgbaImage, background: &RgbaImage, region: Region) -> u64 {
    let [left, top, right, bottom] = region;
    (top..bottom)
        .flat_map(|y| (left..right).map(move |x| (x, y)))
        .map(|(x, y)| {
            let a = frame.get_pixel(x, y).0;
            let b = background.get_pixel(x, y).0;
            a[..3]
                .iter()
                .zip(b)
                .map(|(&a, b)| u64::from(a.abs_diff(b)))
                .sum::<u64>()
        })
        .sum()
}

fn save(output: &Path, samples: &[Value]) -> gpui::Result<()> {
    fs::write(
        output.join("scroll-edge-fade.json"),
        serde_json::to_vec_pretty(samples)?,
    )?;
    Ok(())
}
