use std::sync::{Arc, LazyLock};

use gpui::{
    App, Background, Bounds, Corners, Hsla, IntoElement, ObjectFit, Pixels, RenderImage,
    SharedString, Styled, Window, canvas, fill, linear_color_stop, linear_gradient, px, rgb,
};

use crate::{providers::soundcloud_card_artwork, ui::theme::ARTWORK_TINT};

pub(in crate::ui) use backdrop::{HeroBackdrop, artist_backdrop, hero_backdrop};
use cache::{Artwork, ArtworkCache, ArtworkImage, ArtworkRequest};
pub(in crate::ui) use cache::{
    ArtworkClearStatus, artwork_clear_status, artwork_disk_usage, clear_artwork_cache,
};
pub(in crate::ui) use colors::ArtworkColors;
use decode::ArtworkEdge;

const RESPONSE_LIMIT: u64 = 1024 * 1024;
static DEFAULT_GLOW: LazyLock<Background> = LazyLock::new(|| glow(rgb(ARTWORK_TINT).into()));

pub(in crate::ui) fn remote(url: &str, circular: bool, card: bool) -> impl IntoElement {
    remote_rounded(url, circular, Pixels::ZERO, card)
}

pub(in crate::ui) fn remote_rounded(
    url: &str,
    circular: bool,
    radius: Pixels,
    card: bool,
) -> impl IntoElement {
    remote_rounded_or(Some(url), circular, radius, card, |_, _, _| None)
}

pub(in crate::ui) fn remote_rounded_or(
    url: Option<&str>,
    circular: bool,
    radius: Pixels,
    card: bool,
    fallback: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) -> Option<std::sync::Arc<RenderImage>>
    + 'static,
) -> impl IntoElement {
    remote_rounded_or_at_edge(url, circular, radius, card, None, fallback)
}

pub(in crate::ui) fn remote_rounded_or_at_edge(
    url: Option<&str>,
    circular: bool,
    radius: Pixels,
    card: bool,
    decode_edge: Option<Pixels>,
    fallback: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) -> Option<std::sync::Arc<RenderImage>>
    + 'static,
) -> impl IntoElement {
    let source = url.map(|url| ArtworkSource::new(url, card));
    rounded_image(circular, radius, move |bounds, window, cx| {
        let edge = ArtworkEdge::for_render(
            decode_edge.unwrap_or_else(|| bounds.size.width.max(bounds.size.height)),
            window,
        );
        source
            .as_ref()
            .and_then(|source| use_artwork(source, edge, window, cx))
            .map(|artwork| artwork.image)
            .filter(|image| image.frame_count() > 0)
            .or_else(|| fallback(bounds, window, cx))
    })
}

pub(in crate::ui) fn remote_rounded_for_transition(
    url: &str,
    radius: Pixels,
    painted_edge: Pixels,
    fallback: Option<Arc<RenderImage>>,
) -> impl IntoElement {
    let source = ArtworkSource::new(url, false);
    rounded_image(false, radius, move |bounds, window, cx| {
        let target = bounds.size.width.max(bounds.size.height);
        let target_edge = ArtworkEdge::for_render(target, window);
        let required_edge = ArtworkEdge::for_render(painted_edge, window);
        let exact = use_artwork(&source, target_edge, window, cx)
            .map(|artwork| artwork.image)
            .filter(|image| image.frame_count() > 0);
        if required_edge == target_edge && exact.is_some() {
            return exact;
        }
        cx.try_global::<ArtworkCache>()
            .and_then(|cache| cache.ready_cover(&source, required_edge))
            .or(exact)
            .or(fallback)
    })
}

fn rounded_image(
    circular: bool,
    radius: Pixels,
    load: impl FnOnce(Bounds<Pixels>, &mut Window, &mut App) -> Option<Arc<RenderImage>> + 'static,
) -> impl IntoElement {
    canvas(
        move |bounds, window, cx| {
            // Long feeds should not fetch or evict covers outside the scroll viewport.
            if bounds.intersects(&window.content_mask().bounds) {
                load(bounds, window, cx)
            } else {
                None
            }
        },
        move |bounds, image, window, _| {
            if let Some(image) = image {
                let fitted = ObjectFit::Cover.get_bounds(bounds, image.size(0));
                let radii = if circular {
                    Corners::all(bounds.size.width.min(bounds.size.height) * 0.5)
                } else {
                    Corners::all(radius)
                };
                let _ = window.paint_image(bounds, fitted, radii, image, 0, false);
            }
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

pub(in crate::ui) fn ambilight(url: Option<&str>, edge: Pixels, large: bool) -> impl IntoElement {
    let source = url.map(|url| ArtworkSource::new(url, large));
    canvas(
        move |bounds, window, cx| {
            if !bounds.intersects(&window.content_mask().bounds) {
                return None;
            }
            let edge = ArtworkEdge::for_render(edge, window);
            let background = source
                .as_ref()
                .and_then(|source| use_artwork(source, edge, window, cx))
                .map_or(*DEFAULT_GLOW, |artwork| artwork.glow);
            Some(background)
        },
        |bounds, background, window, _| {
            let Some(background) = background else {
                return;
            };
            let mut glow = fill(bounds, background);
            glow.corner_radii = Corners::all(px(10.));
            window.paint_quad(glow);
        },
    )
    .size_full()
}

fn glow(tint: Hsla) -> Background {
    linear_gradient(
        103.,
        linear_color_stop(tint.alpha(0.23), 0.),
        linear_color_stop(tint.alpha(0.), 0.7),
    )
}

fn use_artwork(
    source: &ArtworkSource,
    edge: ArtworkEdge,
    window: &mut Window,
    cx: &mut App,
) -> Option<ArtworkImage> {
    use_artwork_result(source, edge, window, cx).flatten()
}

fn use_artwork_result(
    source: &ArtworkSource,
    edge: ArtworkEdge,
    window: &mut Window,
    cx: &mut App,
) -> Option<Option<ArtworkImage>> {
    use_artwork_variant(source, edge, decode::Treatment::Original, window, cx)
}

fn use_artwork_variant(
    source: &ArtworkSource,
    edge: ArtworkEdge,
    treatment: decode::Treatment,
    window: &mut Window,
    cx: &mut App,
) -> Option<Option<ArtworkImage>> {
    if artwork_clear_status(cx) == ArtworkClearStatus::Clearing {
        return Some(None);
    }
    let request = ArtworkRequest {
        source: source.clone(),
        edge,
        treatment,
    };
    let result = window.use_asset::<Artwork>(&request, cx);
    retain_variant(request, result, cx)
}

pub(in crate::ui) fn prefetch(url: &str, edge: ArtworkPrefetch, scale: f32, cx: &mut App) {
    if artwork_clear_status(cx) == ArtworkClearStatus::Clearing {
        return;
    }
    let pixels = match edge {
        ArtworkPrefetch::Row => 40.,
        ArtworkPrefetch::Hero => f32::from(crate::ui::fluid_glass::artwork::HERO_IMAGE_EDGE),
    };
    let request = ArtworkRequest {
        source: ArtworkSource::new(url, false),
        edge: ArtworkEdge::new(pixels, scale),
        treatment: decode::Treatment::Original,
    };
    let result = cx.fetch_asset::<Artwork>(&request);
    retain_variant(request, result, cx);
}

#[derive(Clone, Copy)]
pub(in crate::ui) enum ArtworkPrefetch {
    Row,
    Hero,
}

fn retain_variant(
    request: ArtworkRequest,
    result: Option<Option<ArtworkImage>>,
    cx: &mut App,
) -> Option<Option<ArtworkImage>> {
    let ready = result.as_ref().and_then(Option::as_ref).cloned();
    let (presented, bytes) = cache::present_result(&request, result, cx);
    let evicted = cx
        .default_global::<ArtworkCache>()
        .touch(request, bytes, ready);
    for variant in evicted {
        cache::discard_variant(variant, cx);
    }
    presented
}

/// Distinguishes an unfinished request from a decoded cover or a permanent miss.
pub(in crate::ui) fn decoded_cover(
    url: &str,
    edge: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> Option<Option<std::sync::Arc<RenderImage>>> {
    decoded_variant(url, edge, decode::Treatment::Original, window, cx)
}

pub(in crate::ui) fn artist_photo(
    url: &str,
    edge: Pixels,
    feather: bool,
    window: &mut Window,
    cx: &mut App,
) -> Option<(std::sync::Arc<RenderImage>, [u32; 2])> {
    let treatment = if feather {
        decode::Treatment::ArtistFeather
    } else {
        decode::Treatment::Original
    };
    let source = ArtworkSource::new(url, false);
    let edge = ArtworkEdge::for_render(edge, window);
    let art = use_artwork_variant(&source, edge, treatment, window, cx)??;
    (art.image.frame_count() > 0).then_some((art.image, art.dimensions))
}

fn decoded_variant(
    url: &str,
    edge: Pixels,
    treatment: decode::Treatment,
    window: &mut Window,
    cx: &mut App,
) -> Option<Option<std::sync::Arc<RenderImage>>> {
    let source = ArtworkSource::new(url, false);
    let edge = ArtworkEdge::for_render(edge, window);
    use_artwork_variant(&source, edge, treatment, window, cx).map(|result| {
        result
            .filter(|artwork| artwork.image.frame_count() > 0)
            .map(|artwork| artwork.image)
    })
}

pub(in crate::ui) fn cover_colors(
    url: &str,
    window: &mut Window,
    cx: &mut App,
) -> Option<Option<ArtworkColors>> {
    let source = ArtworkSource::new(url, false);
    // Use the same small image for analysis across viewport sizes and DPI.
    let edge = ArtworkEdge::new(colors::SAMPLE_EDGE as f32, 1.);
    let artwork = use_artwork_result(&source, edge, window, cx)?;
    if let Some(artwork) = &artwork
        && cache::cover_revision(url, cx).is_some_and(|current| current != artwork.content)
    {
        return None;
    }
    Some(artwork.and_then(|artwork| artwork.colors))
}

pub(in crate::ui) fn cover_revision(url: &str, cx: &App) -> Option<[u8; 32]> {
    cache::cover_revision(url, cx)
}

#[cfg(feature = "capture")]
pub(in crate::ui) fn seed_cover_for_capture(
    profile: &std::path::Path,
    url: &str,
    fixture: &std::path::Path,
) -> std::io::Result<()> {
    let source = ArtworkSource::new(url, true);
    let path = disk::path(profile, &source.url, source.larger.as_deref())
        .ok_or_else(|| std::io::Error::other("Неверная ссылка на обложку."))?;
    let image = disk::read(fixture)
        .ok_or_else(|| std::io::Error::other("Не прочиталась обложка проверки."))?;
    disk::save(&path, &image);
    if path.is_file() {
        Ok(())
    } else {
        Err(std::io::Error::other("Обложка проверки не попала в кэш."))
    }
}

#[cfg(feature = "capture")]
pub(in crate::ui) fn decoded_for_capture(url: &str, cx: &mut App) -> bool {
    loaded_for_capture(&ArtworkSource::new(url, true), cx).unwrap_or(false)
}

#[cfg(feature = "capture")]
pub(in crate::ui) fn decoded_edge_for_capture(
    url: &str,
    edge: Pixels,
    window: &Window,
    cx: &mut App,
) -> bool {
    let request = ArtworkRequest {
        source: ArtworkSource::new(url, true),
        edge: ArtworkEdge::for_render(edge, window),
        treatment: decode::Treatment::Original,
    };
    cx.fetch_asset::<Artwork>(&request).flatten().is_some()
}

#[cfg(feature = "capture")]
fn loaded_for_capture(source: &ArtworkSource, cx: &mut App) -> Option<bool> {
    let request = cx
        .default_global::<ArtworkCache>()
        .request_for_capture(source)?;
    cx.fetch_asset::<Artwork>(&request)
        .map(|artwork| artwork.is_some())
}

#[derive(Clone, Eq, Hash, PartialEq)]
struct ArtworkSource {
    url: SharedString,
    larger: Option<SharedString>,
}

impl ArtworkSource {
    fn new(url: &str, card: bool) -> Self {
        Self {
            url: url.to_owned().into(),
            larger: if card {
                soundcloud_card_artwork(url).map(SharedString::from)
            } else {
                None
            },
        }
    }
}

mod backdrop;
mod cache;
mod colors;
mod decode;
pub(in crate::ui) use decode::MIN_FEATHER_EDGE;
mod disk;
mod http;
mod palette;
mod source;
#[cfg(all(test, feature = "capture"))]
mod tests;
