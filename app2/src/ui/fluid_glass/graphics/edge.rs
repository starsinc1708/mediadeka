//! Bounds the full-DPI optical pass to nonoverlapping masks around the contour.

use gpui::{Bounds, ContentMask, Corners, Pixels, Shader, ShaderInstance, Window, point, px};

use super::super::{Geometry, tokens::TITLEBAR};

pub(super) const EDGE_LIMIT: f32 = 6.;
// A guard for snapping to device pixels and neighbouring derivative lanes.
const PAD: f32 = EDGE_LIMIT + 2.;
const MAX_STRIPS: usize = 12;

#[derive(Clone, Copy, Default)]
struct DeviceRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl DeviceRect {
    #[cfg(feature = "capture")]
    fn area(self) -> u64 {
        (self.right - self.left) as u64 * (self.bottom - self.top) as u64
    }

    fn mask(self, scale: f32) -> ContentMask<Pixels> {
        // GPUI floors the origin and ceils the far edge. An inward fraction
        // avoids adjacent masks growing over the same pixel at fractional DPI.
        ContentMask {
            bounds: Bounds::from_corners(
                point(
                    px((self.left as f32 + 0.25) / scale),
                    px((self.top as f32 + 0.25) / scale),
                ),
                point(
                    px((self.right as f32 - 0.25) / scale),
                    px((self.bottom as f32 - 0.25) / scale),
                ),
            ),
        }
    }
}

struct Strips {
    rects: [DeviceRect; MAX_STRIPS],
    len: usize,
    frame: DeviceRect,
    scale: f32,
}

impl Strips {
    fn new(bounds: Bounds<Pixels>, scale: f32) -> Self {
        let edge = |value: Pixels| (f32::from(value) * scale).round() as i32;
        Self {
            rects: [DeviceRect::default(); MAX_STRIPS],
            len: 0,
            frame: DeviceRect {
                left: edge(bounds.left()),
                top: edge(bounds.top()),
                right: edge(bounds.right()),
                bottom: edge(bounds.bottom()),
            },
            scale,
        }
    }

    fn add(&mut self, left: f32, top: f32, right: f32, bottom: f32) {
        let device = |edge: f32| (edge * self.scale).round() as i32;
        let clip = DeviceRect {
            left: device(left).max(self.frame.left),
            top: device(top).max(self.frame.top),
            right: device(right).min(self.frame.right),
            bottom: device(bottom).min(self.frame.bottom),
        };
        if clip.left < clip.right && clip.top < clip.bottom {
            self.rects[self.len] = clip;
            self.len += 1;
        }
    }

    fn as_slice(&self) -> &[DeviceRect] {
        &self.rects[..self.len]
    }

    fn for_geometry(bounds: Bounds<Pixels>, geometry: Geometry, dock: f32, scale: f32) -> Self {
        let mut strips = Self::new(bounds, scale);
        let left = f32::from(bounds.left()) + geometry.inset;
        let top = f32::from(bounds.top()) + geometry.inset;
        let right = f32::from(bounds.right()) - geometry.inset;
        let bottom = f32::from(bounds.bottom()) - geometry.inset;
        let rail = left + geometry.rail;
        let player = bottom - geometry.player;
        if geometry.rail == 0. {
            if geometry.player > 0. {
                strips.add(left - PAD, player - PAD, right + PAD, bottom + PAD);
            }
            return strips;
        }
        let radius = geometry
            .outer_radius
            .min(geometry.rail.min(geometry.player) * 0.5);
        let junction = geometry.inner_radius.min((right - rail).min(player - top));
        let rail_top = top + radius + PAD;
        let elbow = player - junction - PAD;
        let upper_bar = player - PAD;
        let upper_tip = player + radius + PAD;
        let lower_bar = bottom - radius - PAD;
        let dock_left = right - dock;
        if junction <= 0.
            || rail_top > elbow
            || upper_tip > lower_bar
            || geometry.rail < PAD * 2.
            || (dock > 0. && (dock < PAD * 2. || dock_left < rail + junction + PAD * 2.))
        {
            // Degenerate sizes still use the unmodified edge shader.
            strips.rects[0] = strips.frame;
            strips.len = 1;
            return strips;
        }

        // Envelope of the top cap, two rail sides, concave junction, and
        // the player. Partitioning at common edges prevents double blending.
        strips.add(left - PAD, top - PAD, rail + PAD, rail_top);
        strips.add(left - PAD, rail_top, left + PAD, lower_bar);
        strips.add(rail - PAD, rail_top, rail + PAD, elbow);
        strips.add(rail - PAD, elbow, rail + junction + PAD, upper_bar);
        strips.add(rail - PAD, upper_bar, right + PAD, upper_tip);
        strips.add(right - PAD, upper_tip, right + PAD, lower_bar);
        strips.add(left - PAD, lower_bar, right + PAD, bottom + PAD);

        let dock_top = top + TITLEBAR;
        if dock > 0. && player > dock_top {
            let dock_radius = geometry.outer_radius * 0.5;
            let dock_bend = junction
                .min((dock_left - rail) * 0.5)
                .min(player - dock_top - dock_radius)
                .max(0.);
            // Pixels at or below upper_bar already belong to the player mask.
            let top_cap = (dock_top + dock_radius + PAD).min(upper_bar);
            let bend_top = (player - dock_bend - PAD).clamp(top_cap, upper_bar);
            strips.add(dock_left - PAD, dock_top - PAD, right + PAD, top_cap);
            strips.add(dock_left - PAD, top_cap, dock_left + PAD, bend_top);
            strips.add(
                dock_left - dock_bend - PAD,
                bend_top,
                dock_left + PAD,
                upper_bar,
            );
            strips.add(right - PAD, top_cap, right + PAD, upper_bar);
        }

        strips
    }
}

pub(super) fn paint(
    bounds: Bounds<Pixels>,
    geometry: Geometry,
    dock: f32,
    shader: &Shader,
    instance: ShaderInstance,
    window: &mut Window,
) {
    let scale = window.scale_factor();
    let strips = Strips::for_geometry(bounds, geometry, dock, scale);
    for &clip in strips.as_slice() {
        window.with_content_mask(Some(clip.mask(scale)), |window| {
            // Retain the full bounds so all strips use the same shader space.
            window.paint_shader(bounds, Corners::default(), shader, instance);
        });
    }
}

#[cfg(feature = "capture")]
pub(in crate::ui::fluid_glass) fn work(
    geometry: Geometry,
    dock: f32,
    window: &Window,
) -> (usize, u64, u64) {
    let bounds = Bounds {
        origin: point(px(0.), px(0.)),
        size: window.viewport_size(),
    };
    let strips = Strips::for_geometry(bounds, geometry, dock, window.scale_factor());
    (
        strips.len,
        strips.as_slice().iter().map(|clip| clip.area()).sum(),
        strips.frame.area(),
    )
}
