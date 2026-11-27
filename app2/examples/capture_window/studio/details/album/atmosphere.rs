//! Native comparison using different artwork geometry with the same swatches.

use std::{
    fs,
    io::{self, Cursor},
    path::Path,
};

use gpui::{AsyncApp, WindowHandle};
use image::{DynamicImage, ImageFormat, Rgb, RgbImage, RgbaImage};
use mediadeka::MainView;
use serde_json::json;

use super::super::super::{Scene, luminance, require, snapshot, wait};
use crate::render_image;

const COVERS: [&str; 3] = [
    "https://i.scdn.co/image/mediadeka-atmosphere-upright",
    "https://i.scdn.co/image/mediadeka-atmosphere-rotated",
    "https://i.scdn.co/image/mediadeka-atmosphere-warm",
];

pub(super) fn seed(profile: &Path) -> gpui::Result<()> {
    let strips = |primary, accent| {
        RgbImage::from_fn(640, 640, |x, _| Rgb(if x < 400 { primary } else { accent }))
    };
    let upright = strips([28, 135, 156], [132, 64, 165]);
    let rotated = image::imageops::rotate90(&upright);
    let warm = strips([239, 165, 66], [217, 87, 101]);
    let temp = profile.join("atmosphere-palette.cover");
    for (url, image) in COVERS.into_iter().zip([upright, rotated, warm]) {
        let mut bytes = Cursor::new(vec![2]);
        bytes.set_position(1);
        DynamicImage::ImageRgb8(image).write_to(&mut bytes, ImageFormat::Png)?;
        fs::write(&temp, bytes.into_inner())?;
        MainView::seed_artist_artwork_for_capture(profile, url, &temp)?;
    }
    Ok(())
}

pub(super) async fn verify(
    window: WindowHandle<MainView>,
    scene: Scene,
    output: &Path,
    original: &str,
    cx: &mut AsyncApp,
) -> gpui::Result<()> {
    let raw = window.into();
    let mut frames = Vec::new();
    for (index, url) in COVERS.into_iter().enumerate() {
        window.update(cx, |view, _, cx| {
            view.replace_release_artwork_for_capture(url, cx)
        })?;
        let swatches = wait(cx, "кэш обложки и её палитра", |cx| {
            snapshot(raw, cx)?;
            window.update(cx, |view, _, cx| {
                view.artist_artwork_decoded_for_capture(url, cx)
                    .then(|| view.fluid_glass_page_palette_for_capture(cx))
                    .flatten()
            })
        })
        .await?;
        super::super::stage::settle_atmosphere(window, 60 + 10 * index as u64, cx)?;
        let image = render_image(raw, scene, cx)?;
        image.save(output.join(format!("palette-geometry-{index}.png")))?;
        frames.push((image, swatches));
    }
    window.update(cx, |view, _, cx| {
        view.replace_release_artwork_for_capture(original, cx);
        view.resume_fluid_glass_for_capture(cx);
    })?;
    let [
        (first, palette),
        (second, rotated_palette),
        (third, warm_palette),
    ] = frames.as_slice()
    else {
        return Err(io::Error::other("Ожидались три кадра проверочного релиза.").into());
    };
    // Wide single: inside the square sleeve, then across the title and the
    // adjoining area below the toolbar and above the track list.
    let cover = mean_channel_delta(first, second, [345, 145, 510, 300]);
    const BACKGROUND: [u32; 4] = [650, 105, 1090, 370];
    let background = mean_channel_delta(first, second, BACKGROUND);
    let recolored_background = mean_channel_delta(first, third, BACKGROUND);
    let minimum_contrast = frames
        .iter()
        .map(|(image, _)| secondary_text_contrast(image))
        .fold(f64::INFINITY, f64::min);
    fs::write(
        output.join("palette-geometry.json"),
        serde_json::to_vec_pretty(&json!({
            "palette": palette,
            "rotated_palette": rotated_palette,
            "warm_palette": warm_palette,
            "cover_mean_rgb_delta": cover,
            "background_mean_rgb_delta": background,
            "recolored_background_mean_rgb_delta": recolored_background,
            "background_rect_px": BACKGROUND,
            "minimum_secondary_contrast_at_title": minimum_contrast,
        }))?,
    )?;
    require(
        palette == rotated_palette && cover > 10. && background <= 0.12,
        &format!(
            "Перестановка цветов должна менять обложку и сохранять фон. Обложка {cover:.3}, фон {background:.3}."
        ),
    )?;
    require(
        palette != warm_palette && recolored_background >= 1. && minimum_contrast >= 4.5,
        &format!(
            "Новая палитра должна менять свет, сохраняя контраст. RGB {recolored_background:.3}, контраст {minimum_contrast:.2}:1."
        ),
    )
}

fn secondary_text_contrast(image: &RgbaImage) -> f64 {
    // Blank strips between the type, title, byline, metadata and actions.
    let brightest = [123, 195, 230, 265, 344]
        .into_iter()
        .flat_map(|y| (650..990).map(move |x| (x, y)))
        .map(|(x, y)| {
            let [r, g, b, _] = image.get_pixel(x, y).0;
            luminance([r, g, b])
        })
        .fold(0., f64::max);
    (luminance([176, 170, 185]) + 0.05) / (brightest + 0.05)
}

fn mean_channel_delta(
    first: &RgbaImage,
    second: &RgbaImage,
    [left, top, right, bottom]: [u32; 4],
) -> f64 {
    let sum = (top..bottom)
        .flat_map(|y| (left..right).map(move |x| (x, y)))
        .map(|(x, y)| {
            let [r, g, b, _] = first.get_pixel(x, y).0;
            let [rr, gg, bb, _] = second.get_pixel(x, y).0;
            u64::from(r.abs_diff(rr)) + u64::from(g.abs_diff(gg)) + u64::from(b.abs_diff(bb))
        })
        .sum::<u64>();
    sum as f64 / f64::from(3 * (right - left) * (bottom - top))
}
