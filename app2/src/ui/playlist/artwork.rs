use gpui::{
    Bounds, Corners, Div, FontWeight, ObjectFit, Pixels, RenderImage, SharedString, canvas, div,
    point, prelude::*, px, rgb,
};
use guise::{Icon, IconName};

use std::sync::Arc;

use crate::{
    playlist::{PlaylistKey, PlaylistTrack},
    ui::{
        MainView,
        components::{
            artwork::{decoded_cover, remote_rounded},
            music::fluid_cover_slot,
        },
        fluid_glass::artwork::{HERO_IMAGE_EDGE, Seed, cached_cover},
        theme::playlist::*,
    },
};

use super::OpenPlaylist;

pub(super) fn collection_cover(open: &OpenPlaylist) -> Option<&str> {
    open.playlist()
        .and_then(|playlist| playlist.artwork.as_deref())
        .or(open.target.artwork.as_deref())
}

const COLLAGE_TILES: usize = 4;

pub(super) fn cover_candidates(tracks: &[PlaylistTrack]) -> Vec<String> {
    let mut urls = Vec::with_capacity(COLLAGE_TILES);
    add_cover_candidates(
        &mut urls,
        tracks.iter().filter_map(|track| track.artwork.as_deref()),
    );
    urls
}

pub(super) fn add_cover_candidates<'a>(
    urls: &mut Vec<String>,
    candidates: impl IntoIterator<Item = &'a str>,
) {
    for url in candidates {
        if urls.len() == COLLAGE_TILES {
            break;
        }
        if !url.is_empty() && !urls.iter().any(|candidate| candidate == url) {
            urls.push(url.to_owned());
        }
    }
}

impl MainView {
    pub(in crate::ui) fn playlist_atmosphere_artwork<'a>(
        &'a self,
        key: &PlaylistKey,
        fallback: Option<&'a str>,
    ) -> Option<&'a str> {
        self.playlist
            .open
            .as_ref()
            .filter(|open| &open.target.key == key)
            .and_then(collection_cover)
            .or(fallback)
            .or_else(|| {
                self.playlist_artwork_candidates(key)
                    .first()
                    .map(String::as_str)
            })
    }

    pub(in crate::ui) fn playlist_artwork_candidates(&self, key: &PlaylistKey) -> &[String] {
        self.playlist
            .open
            .as_ref()
            .filter(|open| &open.target.key == key)
            .and_then(|open| open.snapshot().map(|snapshot| snapshot.as_ref()))
            .map(|snapshot| snapshot.cover_candidates.as_slice())
            .unwrap_or_default()
    }
}

pub(in crate::ui) fn playlist_cover(
    url: Option<&str>,
    candidates: &[String],
    seed: Seed,
    edge: f32,
    radius: f32,
) -> Div {
    cover(url, candidates, seed, edge, radius, None)
}

pub(super) fn playlist_hero_cover(
    url: Option<&str>,
    candidates: &[String],
    seed: Seed,
    edge: f32,
    radius: f32,
) -> Div {
    cover(url, candidates, seed, edge, radius, Some(HERO_IMAGE_EDGE))
}

fn cover(
    url: Option<&str>,
    candidates: &[String],
    seed: Seed,
    edge: f32,
    radius: f32,
    source_edge: Option<Pixels>,
) -> Div {
    let url = url.map(SharedString::from);
    let tracks = candidates
        .iter()
        .cloned()
        .map(SharedString::from)
        .collect::<Vec<_>>();
    let radius = px(radius);
    fluid_cover_slot(edge, f32::from(radius)).child(
        canvas(
            move |bounds, window, cx| {
                if !bounds.intersects(&window.content_mask().bounds) {
                    return None;
                }
                let edge = source_edge.unwrap_or(bounds.size.width);
                if let Some(url) = &url {
                    match decoded_cover(url, edge, window, cx) {
                        Some(Some(image)) => return Some(vec![image]),
                        None => {
                            return Some(
                                cached_cover(seed, edge, window, cx).into_iter().collect(),
                            );
                        }
                        Some(None) => {}
                    }
                }
                let mut decoded = Vec::with_capacity(tracks.len());
                let mut pending = false;
                for track in &tracks {
                    match decoded_cover(track, edge * 0.72, window, cx) {
                        Some(Some(image)) => decoded.push(image),
                        None => pending = true,
                        Some(None) => {}
                    }
                }
                Some(if pending || decoded.is_empty() {
                    cached_cover(seed, edge, window, cx).into_iter().collect()
                } else {
                    decoded
                })
            },
            move |bounds, images, window, _| {
                let Some(images) = images else { return };
                let tiles = collage_tiles(bounds, images.len());
                for (image, tile) in images.into_iter().zip(tiles) {
                    paint_tile(bounds, tile, radius, image, window);
                }
            },
        )
        .absolute()
        .inset_0(),
    )
}

fn collage_tiles(bounds: Bounds<Pixels>, count: usize) -> impl Iterator<Item = Bounds<Pixels>> {
    let (left, right, top, bottom) = (bounds.left(), bounds.right(), bounds.top(), bounds.bottom());
    let center = bounds.center();
    let rect =
        |left, top, right, bottom| Bounds::from_corners(point(left, top), point(right, bottom));
    let tiles = match count {
        2 => [
            rect(left, top, center.x, bottom),
            rect(center.x, top, right, bottom),
            bounds,
            bounds,
        ],
        3 => [
            rect(left, top, center.x, bottom),
            rect(center.x, top, right, center.y),
            rect(center.x, center.y, right, bottom),
            bounds,
        ],
        4 => [
            rect(left, top, center.x, center.y),
            rect(center.x, top, right, center.y),
            rect(left, center.y, center.x, bottom),
            rect(center.x, center.y, right, bottom),
        ],
        _ => [bounds; COLLAGE_TILES],
    };
    tiles.into_iter().take(count)
}

fn paint_tile(
    area: Bounds<Pixels>,
    tile: Bounds<Pixels>,
    radius: Pixels,
    image: Arc<RenderImage>,
    window: &mut gpui::Window,
) {
    let corner = |outside| if outside { radius } else { Pixels::ZERO };
    let left = tile.left() == area.left();
    let right = tile.right() == area.right();
    let top = tile.top() == area.top();
    let bottom = tile.bottom() == area.bottom();
    let radii = Corners {
        top_left: corner(top && left),
        top_right: corner(top && right),
        bottom_left: corner(bottom && left),
        bottom_right: corner(bottom && right),
    };
    let fitted = ObjectFit::Cover.get_bounds(tile, image.size(0));
    let _ = window.paint_image(tile, fitted, radii, image, 0, false);
}

pub(super) fn sleeve(url: Option<&str>, title: &str, edge: f32) -> Div {
    let radius = px(RADIUS);
    let initials = title
        .split_whitespace()
        .filter_map(|word| word.chars().find(|c| c.is_alphanumeric()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect::<String>();
    div()
        .size(px(edge))
        .relative()
        .flex_none()
        .overflow_hidden()
        .rounded(radius)
        .bg(rgb(SLEEVE))
        .child(
            div()
                .absolute()
                .inset(px(edge * 0.105))
                .flex()
                .flex_col()
                .justify_between()
                .border_t_1()
                .border_b_1()
                .border_color(rgb(TRACK_LINE))
                .child(
                    div()
                        .pt(px(edge * 0.02))
                        .font_family(DISPLAY_FACE)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_size(px(edge * 0.35))
                        .line_height(px(edge * 0.43))
                        .text_color(rgb(SLEEVE_LABEL))
                        .child(initials),
                )
                .child(
                    div().pb(px(edge * 0.05)).child(
                        Icon::new(IconName::AudioLines)
                            .pixel_size(px(edge * 0.16))
                            .color(gpui::Hsla::from(rgb(SIGNAL))),
                    ),
                ),
        )
        .children(url.map(|url| remote_rounded(url, false, radius, true)))
}

pub(in crate::ui) fn track_cover(url: Option<&str>, edge: f32) -> Div {
    let radius = px(5.);
    div()
        .size(px(edge))
        .relative()
        .flex_none()
        .overflow_hidden()
        .rounded(radius)
        .flex()
        .items_center()
        .justify_center()
        .child(
            Icon::new(IconName::Music2)
                .pixel_size(px(edge * 0.44))
                .color(gpui::Hsla::from(rgb(INK_SECONDARY)).alpha(0.62)),
        )
        .children(url.map(|url| remote_rounded(url, false, radius, false)))
}
