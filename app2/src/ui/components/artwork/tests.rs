use base64::{Engine, engine::general_purpose::STANDARD};
use gpui::{Context, Image, ImageFormat, ParentElement, Render, TestAppContext, div};
use std::sync::Arc;

use super::cache::CACHE_ENTRIES;
use super::*;

#[cfg(windows)]
mod conditional;
#[cfg(windows)]
mod native;

struct Cover(ArtworkRequest, f32);

impl Render for Cover {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .size(px(self.1))
            .child(remote(&self.0.source.url, false, true))
    }
}

#[test]
fn evicting_cover_releases_its_painted_atlas_entry() {
    let profile = tempfile::tempdir().unwrap();
    // A one-pixel PNG, stored in the real Spotify disk cache for each test URL.
    let pixel = STANDARD
        .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAAEElEQVR4AQEFAPr/AP8AAP8FAAH/+lyI0QAAAABJRU5ErkJggg==")
        .unwrap();
    let image = Image::from_bytes(ImageFormat::Png, pixel);
    let covers = (0..=CACHE_ENTRIES)
        .map(|index| {
            let source = ArtworkSource::new(&format!("https://i.scdn.co/image/{index}"), true);
            let path = disk::path(profile.path(), &source.url, None).unwrap();
            disk::save(&path, &image);
            ArtworkRequest {
                source,
                edge: ArtworkEdge::new(40., 2.),
                treatment: decode::Treatment::Original,
            }
        })
        .collect::<Vec<_>>();
    let mut context = TestAppContext::single();
    context.update(|cx| {
        cx.set_global(ArtworkCache::new(Some(profile.path().to_owned())));
    });
    let first = &covers[0];
    let window = context.add_window(|_, _| Cover(first.clone(), 40.));
    context.run_until_parked();
    let initially_painted = window
        .update(&mut context, |_, window, cx| {
            let image = cx.fetch_asset::<Artwork>(first).flatten().unwrap().image;
            assert!(window.has_image_atlas_entry(&image));
            image
        })
        .unwrap();
    for cover in covers.iter().skip(1).chain(std::iter::once(first)) {
        window
            .update(&mut context, |view, _, cx| {
                view.0 = cover.clone();
                cx.notify();
            })
            .unwrap();
        context.run_until_parked();
    }
    window
        .update(&mut context, |_, window, cx| {
            let reloaded = cx.fetch_asset::<Artwork>(first).flatten().unwrap().image;
            assert!(!window.has_image_atlas_entry(&initially_painted));
            assert!(window.has_image_atlas_entry(&reloaded));
            assert!(!Arc::ptr_eq(&initially_painted, &reloaded));
        })
        .unwrap();
}

#[test]
fn different_visible_sizes_share_original_after_the_disk_file_disappears() {
    let profile = tempfile::tempdir().unwrap();
    let source = ArtworkSource::new("https://i.scdn.co/image/shared-test", true);
    let path = disk::path(profile.path(), &source.url, None).unwrap();
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        320,
        240,
        image::Rgba([224, 28, 52, 255]),
    ))
    .write_to(&mut bytes, image::ImageFormat::Png)
    .unwrap();
    disk::save(
        &path,
        &Image::from_bytes(ImageFormat::Png, bytes.into_inner()),
    );

    let row = ArtworkRequest {
        source: source.clone(),
        edge: ArtworkEdge::new(40., 2.),
        treatment: decode::Treatment::Original,
    };
    let card = ArtworkRequest {
        source,
        edge: ArtworkEdge::new(140., 2.),
        treatment: decode::Treatment::Original,
    };
    let mut context = TestAppContext::single();
    context.update(|cx| cx.set_global(ArtworkCache::new(Some(profile.path().to_owned()))));
    let window = context.add_window(|_, _| Cover(row.clone(), 40.));
    context.run_until_parked();
    let small = context.update(|cx| cx.fetch_asset::<Artwork>(&row).flatten().unwrap());
    assert_eq!(small.image.size(0).width.0, 96);
    std::fs::remove_file(path).unwrap();
    window
        .update(&mut context, |view, _, cx| {
            view.0 = card.clone();
            view.1 = 140.;
            cx.notify();
        })
        .unwrap();
    context.run_until_parked();
    let large = context.update(|cx| cx.fetch_asset::<Artwork>(&card).flatten().unwrap());
    assert_eq!(large.image.size(0).width.0, 320);
    assert_eq!(small.tint, large.tint);
    assert_eq!(small.glow, large.glow);
}
