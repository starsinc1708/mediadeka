fn pattern_voronoi(p: vec2<f32>) -> vec2<f32> {
    let base = floor(p);
    let f = fract(p);
    var d1 = 8.0;
    var d2 = 8.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let g = vec2<f32>(f32(x), f32(y));
            let o = hash2d_vec(vec2<i32>(i32(base.x) + x, i32(base.y) + y));
            let r = g + o - f;
            let d = dot(r, r);
            if (d < d1) {
                d2 = d1;
                d1 = d;
            } else if (d < d2) {
                d2 = d;
            }
        }
    }
    return sqrt(vec2<f32>(d1, d2));
}

fn pattern_gem_smoke(p: vec2<f32>, t: f32, field: VisualField) -> PatternSample {
    let seed = field.seed * 0.013;
    let octaves = i32(field.octaves);
    let warp = vec2<f32>(
        visual_fbm(p * 1.7 + vec2<f32>(seed, 0.0) + t * 0.025, octaves),
        visual_fbm(p * 1.7 + vec2<f32>(4.3, 1.7 - seed) - t * 0.02, octaves),
    );
    let q = p * 1.35 + warp * mix(0.6, 1.8, field.distortion);
    let cells = pattern_voronoi(q * 2.4 + vec2<f32>(t * 0.04, -t * 0.03));
    let crystal = 1.0 - smoothstep(0.02, 0.32, cells.y - cells.x);
    let facet = exp2(-pow((cells.y - cells.x) / 0.045, 2.0));
    let smoke = clamp(
        0.5 + 0.5 * (warp.x * 0.72 + warp.y * 0.36) + (cells.x - 0.5) * 0.5,
        0.0,
        1.0,
    );
    let smoke3 = smoke * smoke * smoke;
    let value = clamp(
        crystal * 0.55 + facet * 0.32 + smoothstep(0.25, 0.8, smoke) * 0.42,
        0.0,
        1.0,
    );
    return PatternSample(
        value,
        smoothstep(0.04, 0.45, smoke * 0.8 + crystal * 0.6) * 0.85 + facet * 0.12,
        crystal * 0.55 + facet * 0.72 + smoke3 * 0.15,
        smoke,
    );
}
