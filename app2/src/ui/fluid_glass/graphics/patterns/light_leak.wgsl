fn pattern_light_leak(p: vec2<f32>, t: f32, field: VisualField) -> PatternSample {
    let drift = vec2<f32>(sin(t * 0.07), cos(t * 0.06)) * 0.12;
    let axis = normalize(vec2<f32>(0.84, -0.54));
    let side = vec2<f32>(-axis.y, axis.x);
    let along = dot(p, axis);
    let across = dot(p, side);
    // A narrow core inside a wide anisotropic wash reads as light entering
    // through an edge, not as a generic radial gradient.
    let bend = across - 0.10 + 0.16 * sin(along * 1.6 + t * 0.05);
    let streak = exp2(-(bend * bend) / 0.075);
    let core = exp2(-(bend * bend) / 0.012) * smoothstep(-1.3, 0.5, along);
    let wash = exp2(-(bend * bend) / 0.34);
    let noise = visual_fbm(p * 2.6 + drift + t * 0.03, i32(field.octaves));
    let warm = vec2<f32>(-0.34, 0.24) + drift;
    let da = length((p - warm) * vec2<f32>(0.62, 1.5));
    let glow = exp2(-da * da * 3.4);
    let value = clamp(
        (core * 1.1 + streak * 0.62 + wash * 0.26 + glow * 0.36)
            * (0.72 + 0.28 * noise),
        0.0,
        1.0,
    );
    return PatternSample(
        value,
        smoothstep(0.02, 0.38, core + streak * 0.8 + wash * 0.6 + glow * 0.7),
        core * 0.85 + streak * 0.34 + glow * 0.18,
        0.35,
    );
}
