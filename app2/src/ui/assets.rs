use std::{borrow::Cow, collections::HashMap, sync::Arc};

use gpui::{
    App, AssetSource, DevicePixels, Global, IntoElement, ObjectFit, Pixels, RenderImage,
    RenderOnce, SharedString, Styled, SvgSize, Window, div, img, prelude::*, px, size,
};

use crate::providers::ProviderId;
pub(super) use crate::providers::provider_index;

pub struct Assets;

pub(super) const BRAND_MARKS: [&str; 5] = [
    "spotify.svg",
    "youtubemusic.svg",
    "yandex.svg",
    "soundcloud.svg",
    "jellyfin.svg",
];

pub(super) fn provider_mark(provider: ProviderId, edge: f32) -> impl IntoElement {
    mark(BRAND_MARKS[provider_index(provider)], px(edge))
}

macro_rules! embedded_assets {
    ($($icon:literal),* $(,)?) => {
        const FILES: &[(&str, &[u8])] = &[
            $((concat!($icon, ".svg"), include_bytes!(concat!("../../assets/", $icon, ".svg"))),)*
        ];
    };
}

embedded_assets!(
    "mediadeka",
    "spotify",
    "youtubemusic",
    "yandex",
    "soundcloud",
    "jellyfin",
    "applemusic",
    "deezer",
    "tidal",
    "diagnostics",
);

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(FILES
            .iter()
            .find_map(|&(name, bytes)| (name == path).then_some(Cow::Borrowed(bytes))))
    }

    fn list(&self, prefix: &str) -> gpui::Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(name, _)| name.starts_with(prefix))
            .map(|&(name, _)| SharedString::from(name))
            .collect())
    }
}

pub(super) fn mark(path: &'static str, edge: Pixels) -> impl IntoElement {
    Mark { path, edge }
}

#[derive(IntoElement)]
struct Mark {
    path: &'static str,
    edge: Pixels,
}

#[derive(Default)]
struct MarkCache(HashMap<(&'static str, i32), Arc<RenderImage>>);

impl Global for MarkCache {}

impl RenderOnce for Mark {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Rasterize at the target DPI. Sampling a large PNG into a small
        // window icon discards the edge coverage of thin paths.
        let side = (f32::from(self.edge) * window.scale_factor()).round() as i32;
        let key = (self.path, side);
        let cached = cx
            .try_global::<MarkCache>()
            .and_then(|cache| cache.0.get(&key).cloned());
        let image = cached.or_else(|| {
            let bytes = Assets.load(self.path).ok()??;
            let renderer = cx.svg_renderer();
            let svg = renderer.parse_svg(&bytes).ok()?;
            let edge = DevicePixels(side.max(1));
            let image = renderer
                .render_parsed(&svg, SvgSize::ExactSize(size(edge, edge)))
                .ok()?;
            let cache = &mut cx.default_global::<MarkCache>().0;
            if cache.len() >= 128 {
                cache.clear();
            }
            cache.insert(key, image.clone());
            Some(image)
        });
        div()
            .size(self.edge)
            .flex_none()
            .children(image.map(|image| img(image).size_full().object_fit(ObjectFit::Fill)))
    }
}
