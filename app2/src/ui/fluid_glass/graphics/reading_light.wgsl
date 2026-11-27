const READING_LIGHT_KNEE: f32 = 0.025;
const READING_LIGHT_CEILING: f32 = 0.038;

fn limit_reading_light(
    color: vec3<f32>,
    pixel: vec2<f32>,
    work_area: vec4<f32>,
    dock_interior: f32,
) -> vec3<f32> {
    // The squared RGB estimate is conservative around the target lightness.
    // Native acceptance measures the displayed sRGB pixels with exact WCAG luminance.
    let positive = max(color, vec3<f32>(0.0));
    let lightness = dot(positive * positive, vec3<f32>(0.2126, 0.7152, 0.0722));
    if lightness <= READING_LIGHT_KNEE { return color; }

    // One envelope covers the whole glass, header included. An internal edge here
    // would cut the refraction and read as a horizontal seam across the material.
    // The short top ramp only protects the rounded outer contour.
    let left = smoothstep(work_area.x, work_area.x + 8.0, pixel.x);
    let right_fade = clamp(0.02 * (work_area.z - work_area.x), 8.0, 32.0);
    let right = 1.0 - smoothstep(work_area.z - right_fade, work_area.z + right_fade, pixel.x);
    let top = smoothstep(work_area.y, work_area.y + 18.0, pixel.y);
    let bottom = 1.0 - smoothstep(work_area.w, work_area.w + 14.0, pixel.y);
    let strength = left * right * top * bottom * (1.0 - dock_interior);
    let range = READING_LIGHT_CEILING - READING_LIGHT_KNEE;
    let excess = lightness - READING_LIGHT_KNEE;
    let compressed = READING_LIGHT_KNEE + range * excess / (range + excess);
    return color * mix(1.0, sqrt(compressed / lightness), strength);
}
