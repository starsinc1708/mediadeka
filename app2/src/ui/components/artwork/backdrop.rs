use gpui::{
    Bounds, Corners, Hsla, IntoElement, ObjectFit, Pixels, Styled, canvas, fill, linear_color_stop,
    linear_gradient, rgb,
};

use super::{ArtworkEdge, ArtworkImage, ArtworkSource, use_artwork};
use crate::ui::theme::{ARTWORK_TINT, WINDOW};

const PANORAMA_RATIO: f32 = 1.85;
const PORTRAIT_TINT: f32 = 0.17;
// Even a white panorama has to keep enough contrast with the secondary text.
const PANORAMA_SHADE: f32 = 0.84;

#[derive(Clone, Copy)]
pub(in crate::ui) enum HeroBackdrop {
    Collection,
    Feature,
    Portrait,
}

impl HeroBackdrop {
    fn image_bounds(self, mut bounds: Bounds<Pixels>) -> Bounds<Pixels> {
        match self {
            Self::Collection => bounds.size.width *= 0.78,
            Self::Feature => {
                bounds.origin.x += bounds.size.width * 0.32;
                bounds.size.width *= 0.68;
            }
            Self::Portrait => {
                let width = (bounds.size.width * 0.68).min(bounds.size.height * 2.9);
                bounds.origin.x = bounds.right() - width;
                bounds.size.width = width;
            }
        }
        bounds
    }
}

pub(in crate::ui) fn hero_backdrop(
    image: Option<&str>,
    banner: Option<&str>,
    base: Hsla,
    style: HeroBackdrop,
) -> impl IntoElement {
    let image = image.map(|url| ArtworkSource::new(url, true));
    let banner = banner
        .map(|url| ArtworkSource::new(url, true))
        .filter(|banner| image.as_ref() != Some(banner));
    canvas(
        move |bounds, window, cx| {
            if !bounds.intersects(&window.content_mask().bounds) {
                return None;
            }
            let area = style.image_bounds(bounds);
            let edge = ArtworkEdge::for_render(area.size.width.max(area.size.height), window);
            banner
                .as_ref()
                .and_then(|source| use_artwork(source, edge, window, cx))
                .or_else(|| {
                    image
                        .as_ref()
                        .and_then(|source| use_artwork(source, edge, window, cx))
                })
        },
        move |bounds, image, window, _| {
            let Some(artwork) = image.filter(|artwork| artwork.image.frame_count() > 0) else {
                return;
            };
            let area = style.image_bounds(bounds);
            let mut fitted = ObjectFit::Cover.get_bounds(area, artwork.image.size(0));
            if matches!(style, HeroBackdrop::Portrait) && !is_panorama(&artwork) {
                let vertical_crop = fitted.size.height - area.size.height;
                fitted.origin.x += area.size.width * 0.12;
                fitted.origin.y = area.top() - vertical_crop.max(Pixels::ZERO) * 0.19;
            }
            let _ = window.paint_image(area, fitted, Corners::default(), artwork.image, 0, false);

            let (shade, crossfade, top) = match style {
                // Keep the maximum under the table's secondary text contrast threshold.
                HeroBackdrop::Collection => (
                    0.80,
                    linear_gradient(
                        90.,
                        linear_color_stop(base.alpha(0.), 0.09),
                        linear_color_stop(base, 0.78),
                    ),
                    0.64,
                ),
                // The shaded left half holds the heading and all of its controls.
                HeroBackdrop::Feature | HeroBackdrop::Portrait => (
                    0.30,
                    linear_gradient(
                        90.,
                        linear_color_stop(base, 0.30),
                        linear_color_stop(base.alpha(0.), 0.83),
                    ),
                    1.,
                ),
            };
            window.paint_quad(fill(area, base.alpha(shade)));
            window.paint_quad(fill(
                area,
                linear_gradient(
                    130.,
                    linear_color_stop(artwork.tint.alpha(0.09), 0.),
                    linear_color_stop(artwork.tint.alpha(0.), 0.78),
                ),
            ));
            window.paint_quad(fill(area, crossfade));
            if !matches!(style, HeroBackdrop::Collection) {
                window.paint_quad(fill(
                    area,
                    linear_gradient(
                        90.,
                        linear_color_stop(base.alpha(0.), 0.87),
                        linear_color_stop(base, 1.),
                    ),
                ));
            }
            window.paint_quad(fill(
                area,
                linear_gradient(
                    180.,
                    linear_color_stop(base.alpha(top), 0.),
                    linear_color_stop(base.alpha(0.), 0.30),
                ),
            ));
            window.paint_quad(fill(
                area,
                linear_gradient(
                    180.,
                    linear_color_stop(base.alpha(0.), 0.40),
                    linear_color_stop(base, 1.),
                ),
            ));
        },
    )
    .absolute()
    .inset_0()
}

pub(in crate::ui) fn artist_backdrop(
    portrait: Option<&str>,
    banner: Option<&str>,
    portrait_edge: Pixels,
    compact: bool,
    radius: Pixels,
) -> impl IntoElement {
    let portrait = portrait.map(|url| ArtworkSource::new(url, true));
    let banner = banner
        .filter(|_| !compact)
        .map(|url| ArtworkSource::new(url, true))
        .filter(|banner| portrait.as_ref() != Some(banner));

    canvas(
        move |bounds, window, cx| {
            if !bounds.intersects(&window.content_mask().bounds) {
                return (None, None);
            }
            let small_edge = ArtworkEdge::for_render(portrait_edge, window);
            let wide_edge = ArtworkEdge::for_render(bounds.size.width, window);
            let portrait_preview = portrait
                .as_ref()
                .and_then(|source| use_artwork(source, small_edge, window, cx));
            let banner = banner
                .as_ref()
                .and_then(|source| use_artwork(source, wide_edge, window, cx));
            let tint = portrait_preview
                .as_ref()
                .or(banner.as_ref())
                .map(|art| art.tint);
            let panorama = if compact {
                None
            } else {
                banner.filter(is_panorama).or_else(|| {
                    let preview = portrait_preview.filter(is_panorama)?;
                    let source = portrait.as_ref()?;
                    use_artwork(source, wide_edge, window, cx).or(Some(preview))
                })
            };
            (tint, panorama)
        },
        move |bounds, (tint, panorama), window, _| {
            let background = Hsla::from(rgb(WINDOW));
            if let Some(panorama) = panorama {
                let fitted = ObjectFit::Cover.get_bounds(bounds, panorama.image.size(0));
                let _ = window.paint_image(
                    bounds,
                    fitted,
                    Corners::all(radius),
                    panorama.image,
                    0,
                    false,
                );
                window.paint_quad(
                    fill(
                        bounds,
                        linear_gradient(
                            90.,
                            linear_color_stop(background, 0.),
                            linear_color_stop(background.alpha(PANORAMA_SHADE), 1.),
                        ),
                    )
                    .corner_radii(radius),
                );
            } else {
                let tint = tint.unwrap_or_else(|| rgb(ARTWORK_TINT).into());
                window.paint_quad(
                    fill(
                        bounds,
                        linear_gradient(
                            90.,
                            linear_color_stop(tint.alpha(0.), 0.),
                            linear_color_stop(tint.alpha(PORTRAIT_TINT), 1.),
                        ),
                    )
                    .corner_radii(radius),
                );
            }
        },
    )
    .absolute()
    .inset_0()
}

fn is_panorama(artwork: &ArtworkImage) -> bool {
    if artwork.image.frame_count() == 0 {
        return false;
    }
    let dimensions = artwork.image.size(0);
    dimensions.height.0 > 0
        && dimensions.width.0 as f32 >= dimensions.height.0 as f32 * PANORAMA_RATIO
}
