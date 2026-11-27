//! Baked, bounded variants for static covers. The palette and identity are
//! shared with the live atmospheric pass.

use std::{collections::VecDeque, f32::consts::TAU, sync::Arc};

use gpui::{App, Asset, Global, Pixels, RenderImage, Window};
use image::{Frame, Rgba, RgbaImage};

use super::{PALETTES, Seed, optics_enabled, tokens};

const COVER_BUDGET: usize = 18 * 1024 * 1024;
const COVER_ENTRIES: usize = 80;

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
struct Request {
    seed: Seed,
    edge: u32,
}

impl Request {
    fn new(seed: Seed, edge: Pixels, scale: f32) -> Self {
        let physical = f32::from(edge) * scale;
        let edge = if physical <= 64. {
            64
        } else if physical <= 192. {
            192
        } else {
            320
        };
        Self { seed, edge }
    }

    fn bytes(self) -> usize {
        (self.edge * self.edge * 4) as usize
    }
}

enum Texture {}

impl Asset for Texture {
    type Source = Request;
    type Output = Arc<RenderImage>;

    fn load(request: Request, _: &mut App) -> impl Future<Output = Self::Output> + Send + 'static {
        let palette = PALETTES[request.seed.family()];
        async move { raster(request, palette) }
    }
}

#[derive(Default)]
struct Covers {
    recent: VecDeque<Request>,
    bytes: usize,
}

impl Global for Covers {}

pub(in crate::ui) fn cached_cover(
    seed: Seed,
    edge: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> Option<Arc<RenderImage>> {
    if !optics_enabled(cx) {
        return None;
    }
    let request = Request::new(seed, edge, window.scale_factor());
    let image = window.use_asset::<Texture>(&request, cx);
    let cache = cx.default_global::<Covers>();
    if let Some(index) = cache.recent.iter().position(|entry| *entry == request) {
        cache.recent.remove(index);
    } else {
        cache.bytes += request.bytes();
    }
    cache.recent.push_back(request);
    let mut evicted = Vec::new();
    while cache.bytes > COVER_BUDGET || cache.recent.len() > COVER_ENTRIES {
        let Some(request) = cache.recent.pop_front() else {
            break;
        };
        cache.bytes -= request.bytes();
        evicted.push(request);
    }
    for request in evicted {
        if let Some(image) = cx.fetch_asset::<Texture>(&request) {
            cx.defer(move |cx| cx.drop_image(image, None));
        }
        cx.remove_asset::<Texture>(&request);
    }
    image
}

fn raster(Request { seed, edge }: Request, palette: [u32; 3]) -> Arc<RenderImage> {
    let family = seed.family();
    let [primary, secondary, highlight] = palette.map(bgr);
    let floor = bgr(tokens::CANVAS);
    let phase = seed.unit(32) * TAU;
    let (sin, cos) = ((seed.unit(0) - 0.5) * 2.8).sin_cos();
    let zoom = 0.93 + 0.42 * seed.unit(16);
    let image = RgbaImage::from_fn(edge, edge, |ix, iy| {
        let u = (ix as f32 + 0.5) / edge as f32 - 0.5;
        let v = (iy as f32 + 0.5) / edge as f32 - 0.5;
        let x = (u * cos - v * sin) * zoom;
        let y = (u * sin + v * cos) * zoom;
        let fold = y + 0.30 * (x * 3.4 + phase).sin();
        let band = |value: f32, width: f32| (-(value / width).powi(2)).exp2();
        let light = match family {
            0 => [
                band(fold + 0.16, 0.18),
                band(fold - 0.12, 0.14),
                band(fold + 0.03, 0.06),
            ],
            1 => {
                let weave = ((x * 1.7 + y * 1.1) * TAU + phase).sin() * ((x - y * 1.6) * TAU).sin();
                [weave.max(0.), (-weave).max(0.), weave.abs().powi(4) * 0.5]
            }
            2 => {
                let lens = ((x + 0.12 * (y * 4.2 + phase).sin()) * 6.1).sin();
                let ray = fold + lens * 0.23;
                [
                    band(ray + 0.18, 0.24),
                    band(ray - 0.22, 0.30),
                    lens.max(0.) * 0.40,
                ]
            }
            _ => {
                let segment = ((y + 0.73) * 12.).floor();
                let slip = 0.18 * (segment * 2.1 + phase).sin().max(0.);
                let wave = fold + (x + slip) * 0.92;
                [
                    band(wave + 0.09, 0.14),
                    band(wave - 0.23, 0.25),
                    band(wave, 0.045),
                ]
            }
        };
        let falloff = 1. - 0.70 * (u * u + v * v);
        let bits = ix.wrapping_mul(0x1f123bb5) ^ iy.wrapping_mul(0x05491333) ^ seed.pattern_seed();
        let grain = ((bits ^ (bits >> 16)).wrapping_mul(0x7feb352d) & 255) as f32 / 255. - 0.5;
        let channel = |i: usize| {
            let color = floor[i]
                + falloff
                    * (primary[i] * 0.50 * light[0]
                        + secondary[i] * 0.41 * light[1]
                        + highlight[i] * 0.28 * light[2]);
            (color + grain * 6.).clamp(0., 255.).round() as u8
        };
        // GPUI's RenderImage expects BGRA.
        Rgba([channel(0), channel(1), channel(2), 255])
    });
    Arc::new(RenderImage::new([Frame::new(image)]))
}

fn bgr(color: u32) -> [f32; 3] {
    [0, 8, 16].map(|shift| ((color >> shift) & 255) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn baked_artwork_is_stable_and_separates_playlist_identities() {
        let fixture = Request {
            seed: Seed::playlist(5),
            edge: 64,
        };
        let bake = |request: Request| raster(request, PALETTES[request.seed.family()]);
        let first = bake(fixture);
        let second = bake(fixture);
        let other = bake(Request {
            seed: Seed::playlist(6),
            ..fixture
        });
        assert_eq!(first.as_bytes(0), second.as_bytes(0));
        assert_ne!(first.as_bytes(0), other.as_bytes(0));
    }
}
