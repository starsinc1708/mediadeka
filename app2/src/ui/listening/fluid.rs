use gpui::{AnyElement, Context, Div, Role, Stateful, Window, div, prelude::*, px, rgb};

use crate::ui::{
    MainView,
    components::{artwork::remote_rounded_for_transition, music::fluid_cover_slot},
    fluid_glass::{Geometry, PageAtmosphere, artwork, tokens},
    music::detail::hero_motion::{IMMERSIVE_ARTWORK_SPAN, paint_offset, shared_artwork},
};

use super::reveal;

use super::{ListeningMode, WIDE_TEXT, track_heading};

impl MainView {
    pub(super) fn fluid_listening_view(
        &self,
        mode: ListeningMode,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let lyrics = mode == ListeningMode::Lyrics;
        let geometry = Geometry::for_window(window);
        let high_contrast = self.fluid_high_contrast(cx);
        let player = self.player.read(cx);
        let entry = player.current();
        let title = entry.map_or("Сейчас ничего не играет", |entry| {
            entry.track.title.as_str()
        });
        let artist = entry
            .map(|entry| entry.track.artist.as_str())
            .filter(|name| !name.is_empty());
        let layout = Stage::for_window(geometry, window, mode);
        let compact = layout.compact;
        let compact_lyrics = lyrics && compact;
        let motion = self.listening.artwork_motion;
        let cover = entry
            .map(|entry| {
                let radius = px(layout.radius);
                let seed = artwork::Seed::track(&entry.track);
                let artwork = fluid_cover_slot(layout.artwork, layout.radius)
                    .when_some(seed, |art, seed| {
                        art.child(artwork::pattern(seed, radius, 1.))
                    })
                    .when_some(player.current_artwork(), |art, url| {
                        let target = px(layout.artwork);
                        let painted =
                            motion.map_or(target, |motion| motion.painted_square_edge(target));
                        art.child(remote_rounded_for_transition(
                            url,
                            radius,
                            painted,
                            player.displayed_cover_image(),
                        ))
                    });
                div()
                    .id("fluid-listening-artwork")
                    .accessibility_id("fluid-listening-artwork")
                    .role(Role::Image)
                    .aria_label(format!("Обложка · {title}"))
                    .size(px(layout.artwork))
                    .flex_none()
                    .child(artwork)
            })
            .map(|cover| shared_artwork(cover, IMMERSIVE_ARTWORK_SPAN, motion));
        let headings = track_heading(
            title,
            artist,
            layout.title,
            layout.title + 7.,
            if compact_lyrics { 1 } else { 2 },
            if high_contrast {
                tokens::contrast::SECONDARY
            } else {
                tokens::SECONDARY
            },
        );
        let headings: AnyElement = match motion {
            Some(motion) => {
                let (opacity, offset) = reveal::metadata(Some(motion));
                paint_offset(div().opacity(opacity).child(headings), px(offset))
            }
            None => headings.into_any_element(),
        };
        let stage = div()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .w_full()
            .max_w(px(1520.))
            .mx_auto()
            .relative()
            .flex()
            .gap(px(layout.gap))
            .px(px(layout.gutter));
        let stage = if lyrics {
            stage
                .pt(px(if compact { 4. } else { 40. }))
                .pb(px(if compact { 2. } else { 20. }))
                .child(
                    div()
                        .w(px(if compact_lyrics {
                            layout.artwork.max(COMPACT_LYRICS_CAPTION_WIDTH)
                        } else {
                            layout.artwork
                        }))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .gap(px(if compact { 10. } else { 26. }))
                        .children(cover)
                        .child(headings),
                )
                .child(self.lyrics.clone())
        } else {
            stage
                .items_center()
                .justify_center()
                .pb(px(if compact { 8. } else { 40. }))
                .children(cover)
                .child(div().min_w_0().max_w(px(680.)).child(headings))
        };
        let backdrop = self.fluid_scene_backdrop(
            cx,
            if lyrics {
                tokens::atmosphere::LYRICS
            } else {
                tokens::atmosphere::NOW_PLAYING
            },
            true,
            lyrics.then(PageAtmosphere::lyrics),
        );
        self.listening_container(mode)
            .bg(rgb(if high_contrast {
                tokens::contrast::CANVAS
            } else {
                tokens::CANVAS
            }))
            .text_color(rgb(if high_contrast {
                tokens::contrast::PRIMARY
            } else {
                tokens::PRIMARY
            }))
            .child(backdrop)
            .child(self.listening_header(mode, compact, window, cx))
            .child(stage)
            .child(
                div()
                    .relative()
                    .flex_none()
                    .px(px(geometry.inset()))
                    .pb(px(geometry.inset()))
                    .child(self.player.clone()),
            )
    }

    #[cfg(feature = "capture")]
    pub(in crate::ui) fn fluid_listening_artwork_edge(
        &self,
        window: &Window,
    ) -> Option<gpui::Pixels> {
        let mode = self.listening.mode()?;
        let stage = Stage::for_window(Geometry::for_window(window), window, mode);
        Some(px(stage.artwork))
    }
}

struct Stage {
    compact: bool,
    artwork: f32,
    title: f32,
    gutter: f32,
    gap: f32,
    radius: f32,
}

const COMPACT_LYRICS_CAPTION_WIDTH: f32 = 144.;

impl Stage {
    fn for_window(geometry: Geometry, window: &Window, mode: ListeningMode) -> Self {
        let viewport = window.viewport_size();
        let compact = viewport.width < px(WIDE_TEXT);
        let lyrics = mode == ListeningMode::Lyrics;
        let width = f32::from(viewport.width);
        let height = f32::from(viewport.height);
        let height = (height - geometry.player() - geometry.inset() - 80.).max(0.);
        let (fraction, maximum, vertical, title) = match (lyrics, compact) {
            (false, false) => (0.33, 490., 0.83, 66.),
            (false, true) => (0.34, 270., 0.91, 38.),
            (true, false) => (0.24, 368., 0.66, 43.),
            (true, true) => (0.20, 175., 0.49, 18.),
        };
        Self {
            compact,
            artwork: (width * fraction)
                .min(height * vertical)
                .clamp(84., maximum)
                .round(),
            title,
            gutter: if compact { 26. } else { 76. },
            gap: if compact { 24. } else { 66. },
            radius: if compact { 10. } else { 16. },
        }
    }
}
