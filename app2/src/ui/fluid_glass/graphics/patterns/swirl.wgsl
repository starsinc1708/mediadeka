fn pattern_swirl(p: vec2<f32>, t: f32, field: VisualField) -> PatternSample {
    let radius = length(p);
    let angle = radius * mix(1.6, 5.4, field.distortion) + t * 0.5 + field.seed * 0.01;
    let q = rotate(p, angle);
    let warp = visual_fbm(q * 2.1 + t * 0.03, i32(field.octaves));
    let bands = 0.5 + 0.5 * sin(q.x * 9.0 + q.y * 5.5 + warp * 3.2 + t * 0.15);
    let arms = pow(bands, 3.0);
    let veil = smoothstep(0.15, 0.85, radius);
    return PatternSample(
        clamp(arms * 0.9 + veil * 0.12, 0.0, 1.0),
        smoothstep(0.05, 0.5, arms),
        arms * 0.45 + warp * 0.08,
        0.5,
    );
}
