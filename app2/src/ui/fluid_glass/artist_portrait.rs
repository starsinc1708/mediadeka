use std::sync::Arc;

use gpui::{
    App, Corners, IntoElement, Pixels, RenderImage, Role, SharedString, Window, canvas, div,
    prelude::*, px,
};

use crate::ui::components::{artwork, music::fluid_cover_slot};

use super::{
    artwork::{Seed, hero_thumbnail},
    effects_enabled,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Framing {
    Compact,
    Portrait,
    Cinematic,
}

fn framing(width: u32, height: u32) -> Framing {
    if width.min(height) < artwork::MIN_FEATHER_EDGE {
        Framing::Compact
    } else if width as f32 >= 1.52 * height as f32 {
        Framing::Cinematic
    } else {
        Framing::Portrait
    }
}

#[derive(IntoElement)]
pub(in crate::ui) struct ArtistPortrait {
    pub portrait: Option<SharedString>,
    pub panorama: Option<SharedString>,
    pub fallback: Seed,
    pub width: f32,
    pub height: f32,
    pub compact: bool,
    pub high_contrast: bool,
    pub decorative: bool,
    pub decode_edge: Option<Pixels>,
}

impl RenderOnce for ArtistPortrait {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let effects = !self.high_contrast && effects_enabled();
        let edge = self
            .decode_edge
            .unwrap_or_else(|| px(self.width.max(self.height * 1.75)));
        let panorama = self
            .panorama
            .as_deref()
            .filter(|url| self.portrait.as_deref() != Some(*url));
        let selected = panorama
            .into_iter()
            .chain(self.portrait.as_deref())
            .filter_map(|url| {
                let (image, [width, height]) =
                    artwork::artist_photo(url, edge, effects, window, cx)?;
                Some((image, framing(width, height)))
            })
            .max_by_key(|(_, framing)| match framing {
                Framing::Compact => 0,
                Framing::Portrait => 1,
                Framing::Cinematic => 2,
            });
        let Some((image, framing)) = selected else {
            let edge = self.width.min(self.height);
            let radius = if self.compact { 6. } else { 10. };
            let cover = fluid_cover_slot(edge, radius).child(hero_thumbnail(
                None,
                Some(self.fallback),
                px(radius),
            ));
            return div().flex_none().child(if self.decorative {
                cover.into_any_element()
            } else {
                cover
                    .id("music-object-artwork")
                    .accessibility_id("music-object-artwork")
                    .role(Role::Image)
                    .aria_label("Обложка исполнителя")
                    .into_any_element()
            });
        };
        let source = image.size(0);
        let available = match framing {
            Framing::Compact => [self.width.min(172.), self.height.min(146.)],
            Framing::Portrait => [self.width.min(self.height * 1.10), self.height],
            Framing::Cinematic => [self.width, self.height * 0.90],
        };
        let (width, height) = fitted_size(
            [source.width.0 as f32, source.height.0 as f32],
            available,
            window.scale_factor(),
        );
        let slot_width = (width + if self.compact { 12. } else { 34. }).min(self.width);
        let slot_height = (height + if self.compact { 8. } else { 24. }).min(self.height);
        let picture = div().w(px(width)).h(px(height)).child(photo(image));
        let picture = if self.decorative {
            picture.into_any_element()
        } else {
            picture
                .id("music-object-artwork")
                .accessibility_id("music-object-artwork")
                .role(Role::Image)
                .aria_label(if framing == Framing::Cinematic {
                    "Панорамная фотография исполнителя"
                } else {
                    "Портрет исполнителя"
                })
                .into_any_element()
        };
        div()
            .relative()
            .flex_none()
            .w(px(slot_width))
            .h(px(slot_height))
            .flex()
            .items_center()
            .justify_center()
            .child(picture)
    }
}

fn fitted_size(device: [f32; 2], available: [f32; 2], scale: f32) -> (f32, f32) {
    let density = (1. / scale)
        .min(available[0] / device[0])
        .min(available[1] / device[1]);
    (device[0] * density, device[1] * density)
}

fn photo(image: Arc<RenderImage>) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            if bounds.intersects(&window.content_mask().bounds) {
                let _ =
                    window.paint_image(bounds, bounds, Corners::default(), image.clone(), 0, false);
            }
        },
    )
    .size_full()
}

#[cfg(test)]
mod tests {
    use super::{Framing, fitted_size, framing};

    #[test]
    fn portrait_keeps_aspect_and_respects_source_pixels_at_any_dpi() {
        assert_eq!(fitted_size([64., 64.], [590., 370.], 1.), (64., 64.));
        assert_eq!(fitted_size([64., 32.], [590., 370.], 2.), (32., 16.));
        assert_eq!(fitted_size([960., 960.], [560., 360.], 1.), (360., 360.));
        assert_eq!(fitted_size([1200., 800.], [240., 310.], 1.), (240., 160.));
    }

    #[test]
    fn framing_uses_the_decoded_image_geometry() {
        assert_eq!(framing(1400, 720), Framing::Cinematic);
        assert_eq!(framing(720, 720), Framing::Portrait);
        assert_eq!(framing(220, 100), Framing::Compact);
    }
}
