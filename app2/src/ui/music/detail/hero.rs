use gpui::{
    Div, FontWeight, Pixels, Role, Stateful, div, linear_color_stop, linear_gradient, prelude::*,
    px, rgb,
};

use crate::{
    music::ObjectKind,
    ui::{
        components::{artwork, music::cover},
        fluid_glass::{artist_portrait::ArtistPortrait, artwork::Seed, tokens},
        theme::{ELEVATED, WINDOW, surface_border},
    },
};

use super::super::detail_style::{self, HeroSize, ReleasePresentation};
use super::hero_motion::{HERO_ARTWORK_SPAN, HeroMotion, shared_artwork};

fn reveal(body: Div, motion: Option<HeroMotion>) -> Div {
    body.when_some(motion, |body, motion| body.opacity(motion.opacity()))
}

const FEATURE_MIN_WIDTH: f32 = 900.;
const FEATURE_HEIGHT: f32 = 316.;
const FEATURE_INSET: f32 = 24.;
const FEATURE_GAP: f32 = 32.;
const FEATURE_CONTENT_SHARE: f32 = 0.56;
const FEATURE_TEXT_MAX_WIDTH: f32 = 620.;

pub(super) struct Hero {
    compact: bool,
    artist: bool,
    content_width: f32,
    hide_portrait: bool,
    feature_body_width: Option<f32>,
    fluid: Option<HeroSize>,
    stage: bool,
    short_release: bool,
}

pub(super) struct HeroArtwork<'a> {
    pub image: Option<&'a str>,
    pub banner: Option<&'a str>,
    pub fallback: Seed,
    pub scale: f32,
}

impl Hero {
    pub(super) fn portrait_height(size: HeroSize, compact: bool) -> f32 {
        if compact {
            (size.cover + 36.).max(122.)
        } else {
            (size.cover + 32.).clamp(256., 368.)
        }
    }

    pub(super) fn new(
        kind: ObjectKind,
        content_width: f32,
        compact: bool,
        short: bool,
        has_image: bool,
        glass: bool,
        release: ReleasePresentation,
    ) -> Self {
        let fluid = glass.then(|| match kind {
            ObjectKind::Album => HeroSize::release(content_width, compact || short, release),
            ObjectKind::Artist => HeroSize::fluid(content_width, compact || short, true),
            ObjectKind::Track | ObjectKind::Station => {
                HeroSize::detail(content_width, compact || short)
            }
            _ => HeroSize::fluid(content_width, compact || short, false),
        });
        let mut hero = Self {
            compact,
            artist: kind == ObjectKind::Artist,
            content_width,
            hide_portrait: false,
            feature_body_width: None,
            fluid,
            stage: fluid.is_some()
                && matches!(
                    kind,
                    ObjectKind::Album
                        | ObjectKind::Artist
                        | ObjectKind::Playlist
                        | ObjectKind::Collection
                ),
            short_release: kind == ObjectKind::Album && release != ReleasePresentation::Album,
        };
        if glass {
            hero.compact = compact || short;
            return hero;
        }
        if !short
            && content_width >= FEATURE_MIN_WIDTH
            && matches!(kind, ObjectKind::Artist | ObjectKind::Album)
        {
            hero.hide_portrait = hero.artist && has_image;
            let artwork_room = if hero.hide_portrait {
                0.
            } else {
                hero.feature_edge() + FEATURE_GAP
            };
            let body_width = content_width * FEATURE_CONTENT_SHARE - FEATURE_INSET - artwork_room;
            hero.feature_body_width = Some(body_width.min(FEATURE_TEXT_MAX_WIDTH));
        }
        hero
    }

    pub(super) fn is_feature(&self) -> bool {
        self.feature_body_width.is_some() || (self.fluid.is_some() && !self.compact)
    }

    pub(super) fn stage(&self) -> bool {
        self.stage
    }

    pub(super) fn title(&self, title: Stateful<Div>, full_title: &str) -> Stateful<Div> {
        if let Some(size) = self.fluid {
            detail_style::title(title, detail_style::title_size(size.title, full_title), 2)
                .when(self.artist, |title| title.font_weight(FontWeight::BOLD))
        } else if self.is_feature() {
            let (size, line) = if self.hide_portrait {
                (64., 66.)
            } else if self.artist {
                (52., 56.)
            } else {
                (50., 54.)
            };
            title
                .text_size(px(size))
                .line_height(px(line))
                .line_clamp(2)
        } else {
            let size = match (self.compact, self.artist) {
                (true, _) => 27.,
                (false, true) => 40.,
                (false, false) => 35.,
            };
            title.text_size(px(size))
        }
    }

    pub(super) fn body(&self) -> Div {
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(6.))
            .when(self.fluid.is_some() && !self.compact, |body| {
                body.gap(px(10.))
            })
            .when(self.stage, |body| {
                body.gap(px(if self.compact { 4. } else { 12. }))
            })
            .when_some(self.feature_body_width, |body, width| {
                body.max_w(px(width)).gap(px(8.))
            })
    }

    pub(super) fn frame(
        &self,
        art: HeroArtwork<'_>,
        body: Div,
        aside: Option<Div>,
        high_contrast: bool,
        motion: Option<HeroMotion>,
    ) -> Div {
        let HeroArtwork {
            image,
            banner,
            fallback,
            ..
        } = art;
        if let Some(size) = self.fluid {
            if self.stage {
                return self.stage_frame(size, art, body, aside, high_contrast, motion);
            }
            return self.fluid_frame(size, image, fallback, body, motion);
        }
        let edge = self.artwork_edge();
        let frame = div().flex().items_center().relative().overflow_hidden();
        let frame = if self.is_feature() {
            frame
                .min_h(px(FEATURE_HEIGHT))
                .gap(px(FEATURE_GAP))
                .px(px(FEATURE_INSET))
                .py(px(28.))
                .bg(rgb(WINDOW))
                .child(artwork::hero_backdrop(
                    image,
                    banner,
                    rgb(WINDOW).into(),
                    if self.artist {
                        artwork::HeroBackdrop::Portrait
                    } else {
                        artwork::HeroBackdrop::Feature
                    },
                ))
        } else {
            self.panel(frame, image, banner, px(edge))
        };
        let radius = if self.artist { edge / 2. } else { 12. };
        frame
            .when(!self.hide_portrait, |frame| {
                frame.child(cover(image, edge, radius, true))
            })
            .child(body)
    }

    fn stage_frame(
        &self,
        size: HeroSize,
        art: HeroArtwork<'_>,
        body: Div,
        aside: Option<Div>,
        high_contrast: bool,
        motion: Option<HeroMotion>,
    ) -> Div {
        let HeroArtwork {
            image,
            banner,
            fallback,
            scale,
        } = art;
        let width = self.content_width.max(0.);
        let body = reveal(body, motion);
        let row = div()
            .relative()
            .w_full()
            .when(self.short_release, |row| row.max_w(px(1080.)))
            .flex()
            .items_center()
            .gap(px(size.gap))
            .py(px(size.vertical_inset));
        if self.artist {
            let body = body.max_w(px(760.));
            if image.is_none() && banner.is_none() {
                // The page atmosphere remains when no portrait is available.
                return row
                    .min_h(px(if self.compact { 110. } else { 200. }))
                    .child(body);
            }
            let portrait_width = if self.compact {
                (width * 0.40).min(286.)
            } else {
                (width * 0.55).min(760.)
            };
            let portrait_height = Self::portrait_height(size, self.compact);
            let portrait = ArtistPortrait {
                portrait: image.map(gpui::SharedString::from),
                panorama: banner.map(gpui::SharedString::from),
                fallback,
                width: portrait_width,
                height: portrait_height,
                compact: self.compact,
                high_contrast,
                decorative: false,
                decode_edge: None,
            };
            let portrait = shared_artwork(
                div().flex_none().layer_scale(scale).child(portrait),
                HERO_ARTWORK_SPAN,
                motion,
            );
            return row.child(
                div()
                    .w_full()
                    .max_w(px(tokens::ARTIST_STAGE_MAX_WIDTH))
                    .flex()
                    .items_center()
                    .gap(px(size.gap))
                    .child(body)
                    .when(portrait_width > 0., |row| row.child(portrait)),
            );
        }
        let artwork = detail_style::cover(image, Some(fallback), size)
            .id("music-object-artwork")
            .accessibility_id("music-object-artwork")
            .role(Role::Image)
            .aria_label("Обложка")
            .layer_scale(scale);
        let artwork = shared_artwork(
            artwork,
            HERO_ARTWORK_SPAN,
            motion.filter(|_| image.is_some()),
        );
        row.child(artwork)
            .child(body.when(aside.is_some(), |body| body.max_w(px(700.))))
            .children(aside)
    }

    fn fluid_frame(
        &self,
        size: HeroSize,
        image: Option<&str>,
        fallback: Seed,
        body: Div,
        motion: Option<HeroMotion>,
    ) -> Div {
        let artwork = detail_style::cover(image, Some(fallback), size)
            .id("music-object-artwork")
            .accessibility_id("music-object-artwork")
            .role(Role::Image)
            .aria_label("Обложка");
        let artwork = shared_artwork(
            artwork,
            HERO_ARTWORK_SPAN,
            motion.filter(|_| image.is_some()),
        );
        div()
            .relative()
            .flex()
            .items_center()
            .gap(px(size.gap))
            .py(px(size.vertical_inset))
            .child(artwork)
            .child(reveal(body, motion))
    }

    fn panel(&self, frame: Div, image: Option<&str>, banner: Option<&str>, edge: Pixels) -> Div {
        let radius = px(16.);
        frame
            .gap(px(if self.compact { 13. } else { 23. }))
            .p(px(if self.compact { 12. } else { 20. }))
            .rounded(radius)
            .border_1()
            .border_color(surface_border())
            .bg(linear_gradient(
                115.,
                linear_color_stop(rgb(ELEVATED), 0.),
                linear_color_stop(rgb(WINDOW), 0.9),
            ))
            .when(self.artist, |hero| {
                hero.bg(rgb(WINDOW)).child(artwork::artist_backdrop(
                    image,
                    banner,
                    edge,
                    self.compact,
                    radius,
                ))
            })
    }

    fn artwork_edge(&self) -> f32 {
        if self.is_feature() {
            self.feature_edge()
        } else {
            match (self.compact, self.artist) {
                (true, _) => 96.,
                (false, true) => 166.,
                (false, false) => 142.,
            }
        }
    }

    fn feature_edge(&self) -> f32 {
        if self.artist { 184. } else { 176. }
    }
}
