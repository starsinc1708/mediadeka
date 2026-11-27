use std::sync::Arc;

use gpui::RenderImage;

use super::{ArtworkCache, ArtworkEdge, ArtworkSource, decode::Treatment};

impl ArtworkCache {
    pub(in crate::ui::components::artwork) fn ready_cover(
        &self,
        source: &ArtworkSource,
        required: ArtworkEdge,
    ) -> Option<Arc<RenderImage>> {
        let ready = self.recent.iter().rev().filter_map(|entry| {
            if &entry.request.source != source || entry.request.treatment != Treatment::Original {
                return None;
            }
            entry
                .ready
                .as_ref()
                .filter(|image| image.image.frame_count() > 0)
        });
        let revision = self
            .originals
            .iter()
            .find(|(key, _)| key == source)
            .and_then(|(_, slot)| slot.get()?.as_ref())
            .map(|image| image.content)
            .or_else(|| ready.clone().next().map(|image| image.content))?;
        let required = required.pixels();
        ready
            .filter(|image| image.content == revision)
            .min_by_key(|image| {
                let size = image.image.size(0);
                let edge = size.width.0.max(size.height.0);
                // Use the nearest size with enough pixels, or the largest undersized size.
                (edge < required, edge.abs_diff(required))
            })
            .map(|image| Arc::clone(&image.image))
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Image, ImageFormat, rgb};
    use image::{Frame, RgbaImage};
    use std::sync::OnceLock;

    use super::super::{ArtworkImage, ArtworkRequest, OriginalArtwork, glow};
    use super::*;

    #[test]
    fn transition_uses_the_nearest_raster_that_covers_the_frame() {
        let source = ArtworkSource::new("https://example.com/cover", false);
        let other = ArtworkSource::new("https://example.com/other", false);
        let original = OriginalArtwork {
            image: Image::from_bytes(ImageFormat::Png, Vec::new()),
            tint: rgb(0).into(),
            glow: glow(rgb(0).into()),
            dimensions: [768, 768],
            colors: None,
            content: [1; 32],
            watch: None,
        };
        let mut cache = ArtworkCache::new(None);
        let mut insert = |key: &ArtworkSource, treatment, edge: u32, revision| {
            let image = Arc::new(RenderImage::new([Frame::new(RgbaImage::new(edge, edge))]));
            let mut ready = ArtworkImage::new(image, &original);
            ready.content = revision;
            assert!(
                cache
                    .touch(
                        ArtworkRequest {
                            source: key.clone(),
                            edge: ArtworkEdge::new(edge as f32, 1.),
                            treatment,
                        },
                        ready.bytes,
                        Some(ready),
                    )
                    .is_empty()
            );
        };
        insert(&source, Treatment::Original, 128, [1; 32]);
        insert(&source, Treatment::Original, 256, [1; 32]);
        insert(&source, Treatment::ArtistFeather, 384, [1; 32]);
        insert(&other, Treatment::Original, 384, [1; 32]);
        insert(&source, Treatment::Original, 512, [2; 32]);
        cache
            .originals
            .push_back((source.clone(), Arc::new(OnceLock::from(Some(original)))));
        let select = |edge| {
            cache
                .ready_cover(&source, ArtworkEdge::new(edge, 1.))
                .unwrap()
                .size(0)
                .width
                .0
        };
        assert_eq!(select(120.), 128);
        assert_eq!(select(228.), 256);
        assert_eq!(select(320.), 256);
        assert!(
            cache
                .ready_cover(
                    &ArtworkSource::new("https://example.com/missing", false),
                    ArtworkEdge::new(240., 1.)
                )
                .is_none()
        );
    }
}
