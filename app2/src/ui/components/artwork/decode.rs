use std::{io::Cursor, sync::Arc};

use gpui::{Image, ImageFormat, Pixels, RenderImage, Window};
use image::{DynamicImage, Frame, GenericImageView, ImageDecoder, ImageReader, Limits};

use crate::ui::theme::ARTWORK_TINT;

const EDGES: [u32; 12] = [48, 64, 96, 128, 192, 256, 384, 512, 768, 1024, 1280, 1536];
const MAX_SOURCE_EDGE: u32 = 8192;
const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;
const TINT_SAMPLES: u64 = 512;
pub(in crate::ui) const MIN_FEATHER_EDGE: u32 = 160;

#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub(super) enum Treatment {
    #[default]
    Original,
    ArtistFeather,
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub(super) struct ArtworkEdge(u32);

impl ArtworkEdge {
    pub(super) fn for_render(logical_edge: Pixels, window: &Window) -> Self {
        Self::new(f32::from(logical_edge), window.scale_factor())
    }

    pub(super) fn new(logical_edge: f32, scale: f32) -> Self {
        let physical_edge = (logical_edge * scale).ceil() as u32;
        Self(
            EDGES
                .into_iter()
                .find(|&edge| edge >= physical_edge)
                .unwrap_or(EDGES[EDGES.len() - 1]),
        )
    }

    pub(super) fn byte_bound(self) -> usize {
        (self.0 * self.0 * 4) as usize
    }

    pub(super) fn pixels(self) -> i32 {
        self.0 as i32
    }

    pub(super) fn render(self, source: DynamicImage, treatment: Treatment) -> Arc<RenderImage> {
        let feather = treatment == Treatment::ArtistFeather
            && source.width().min(source.height()) >= MIN_FEATHER_EDGE;
        let image = resize_for_render(&source, self.0).unwrap_or(source);
        let mut pixels = image.into_rgba8();
        if feather {
            let width = pixels.width() as f32;
            let height = pixels.height() as f32;
            for (x, y, pixel) in pixels.enumerate_pixels_mut() {
                let u = (x as f32 + 0.5) / width;
                let v = (y as f32 + 0.5) / height;
                pixel.0[3] = (f32::from(pixel.0[3]) * portrait_alpha(u, v)).round() as u8;
            }
        }
        for rgba in pixels.as_chunks_mut::<4>().0 {
            rgba.swap(0, 2);
        }
        Arc::new(RenderImage::new([Frame::new(pixels)]))
    }
}

fn resize_for_render(source: &DynamicImage, edge: u32) -> Option<DynamicImage> {
    (source.width().max(source.height()) > edge)
        .then(|| source.resize(edge, edge, image::imageops::FilterType::Triangle))
}

pub(super) fn palette_sample(source: &DynamicImage) -> Arc<RenderImage> {
    let edge = super::colors::SAMPLE_EDGE as u32;
    let sample = resize_for_render(source, edge).unwrap_or_else(|| source.clone());
    ArtworkEdge(edge).render(sample, Treatment::Original)
}

fn portrait_alpha(u: f32, v: f32) -> f32 {
    use crate::ui::design_system::motion::smoothstep;

    // A tilted silhouette with varying shoulder widths keeps the centre
    // sharp and makes the transition follow the photo instead of its box.
    let dx = (u - 0.54 - 0.09 * (0.46 - v)) / (0.51 + 0.042 * (7. * v).sin());
    let dy = (v - 0.44 + 0.04 * (9. * u).cos()) / 0.55;
    let contour = dx * dx + dy * dy;
    let silhouette = 1. - smoothstep((contour - 0.44) / 0.62);
    let nearest_edge = u.min(1. - u).min(v).min(1. - v);
    silhouette * smoothstep(nearest_edge / 0.07)
}

pub(super) fn decode(source: &Image) -> Option<DynamicImage> {
    let format = match source.format {
        ImageFormat::Jpeg => image::ImageFormat::Jpeg,
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Webp => image::ImageFormat::WebP,
        _ => return None,
    };
    let mut reader = ImageReader::with_format(Cursor::new(&source.bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_EDGE);
    limits.max_image_height = Some(MAX_SOURCE_EDGE);
    limits.max_alloc = Some(MAX_SOURCE_BYTES);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().ok()?;
    if decoder.total_bytes() > MAX_SOURCE_BYTES {
        return None;
    }
    let orientation = decoder.orientation().ok()?;
    let mut image = DynamicImage::from_decoder(decoder).ok()?;
    image.apply_orientation(orientation);
    Some(image)
}

pub(super) fn tint(source: &DynamicImage) -> u32 {
    let width = u64::from(source.width());
    let count = width * u64::from(source.height());
    let samples = count.min(TINT_SAMPLES);
    let mut sums = [0.; 3];
    let mut weight = 0.;
    for sample in 0..samples {
        let index = (sample * count + count / 2) / samples;
        let [r, g, b, a] = source
            .get_pixel((index % width) as u32, (index / width) as u32)
            .0
            .map(f32::from);
        let chroma = (r.max(g).max(b) - r.min(g).min(b)) * (a / 255.);
        for (sum, channel) in sums.iter_mut().zip([r, g, b]) {
            *sum += channel * chroma;
        }
        weight += chroma;
    }
    if weight > 0. {
        let [r, g, b] = sums.map(|sum| (sum / weight) as u32);
        (r << 16) | (g << 8) | b
    } else {
        ARTWORK_TINT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_size_follows_dpi_and_does_not_upscale() {
        let source = DynamicImage::new_rgba8(1600, 800);
        let row = ArtworkEdge::new(40., 1.).render(source.clone(), Treatment::Original);
        let dense_row = ArtworkEdge::new(40., 2.).render(source.clone(), Treatment::Original);
        let card = ArtworkEdge::new(160., 1.5).render(source.clone(), Treatment::Original);
        let player = ArtworkEdge::new(1000., 1.).render(source, Treatment::Original);
        assert_eq!((row.size(0).width.0, row.size(0).height.0), (48, 24));
        assert_eq!(dense_row.size(0).width.0, 96);
        assert_eq!(card.size(0).width.0, 256);
        assert_eq!(player.size(0).width.0, 1024);
        let small = ArtworkEdge::new(1000., 1.)
            .render(DynamicImage::new_rgba8(32, 16), Treatment::ArtistFeather);
        assert_eq!((small.size(0).width.0, small.size(0).height.0), (32, 16));
    }

    #[test]
    fn portrait_alpha_is_opaque_at_the_subject_and_releases_at_all_edges() {
        assert_eq!(portrait_alpha(0.53, 0.46), 1.);
        for (u, v) in [(0., 0.5), (1., 0.5), (0.5, 0.), (0.5, 1.)] {
            assert_eq!(portrait_alpha(u, v), 0.);
        }
        assert!(portrait_alpha(0.15, 0.45) > portrait_alpha(0.15, 0.83));
    }
}
