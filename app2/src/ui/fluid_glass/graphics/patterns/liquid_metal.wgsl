fn pattern_height(p: vec2<f32>, t: f32, field: VisualField) -> f32 {
    let seed = field.seed * 0.011;
    let broad = visual_fbm(p * 3.1 + vec2<f32>(seed, -t * 0.018), i32(field.octaves));
    return broad
        + 0.18 * sin(p.x * 5.3 + p.y * 3.7 + broad * 6.0 + t * 0.08)
        + 0.07 * sin(p.x * 11.0 - p.y * 8.3 + t * 0.14);
}

fn pattern_liquid_metal(p: vec2<f32>, t: f32, field: VisualField) -> PatternSample {
    let h = pattern_height(p, t, field);
    // The height gradient of the quad gives the metal a continuous normal at
    // one noise evaluation; two extra finite-difference heights tripled the
    // cost of the most expensive pattern.
    // Normalize the screen derivative by the pattern footprint: the same
    // slope gives the same chrome tilt at any window size, DPI or raster scale.
    let footprint = max(fwidth(p), vec2<f32>(1e-5));
    let nx = dpdx(h) / footprint.x;
    let ny = dpdy(h) / footprint.y;
    let normal = normalize(vec3<f32>(-nx * 5.0, -ny * 5.0, 1.0));
    let view = normalize(vec3<f32>(p * 0.5, 1.7));
    let reflected = reflect(-view, normal);
    let key = pow(max(dot(reflected, normalize(vec3<f32>(-0.35, 0.72, 0.5))), 0.0), 24.0);
    let sheen = pow(max(dot(reflected, view), 0.0), 5.0);
    // A screen-stable ripple in the reflected direction gives the metal broad
    // chrome bands even when the key highlight is outside the current phase.
    let ripple = 0.5 + 0.5 * sin(dot(reflected.xy, vec2<f32>(11.0, 7.0)) + h * 5.0 + t * 0.10);
    let chrome = smoothstep(0.58, 1.0, ripple) * 0.55;
    let value = clamp(h * 0.72 + key * 1.15 + sheen * 0.35 + chrome, 0.0, 1.0);
    return PatternSample(
        value,
        smoothstep(0.04, 0.62, h) * 0.88 + key * 0.40 + chrome * 0.25,
        key * 1.12 + sheen * 0.34 + chrome * 0.72,
        h,
    );
}
