fn pattern_optical_caustics(p: vec2<f32>, t: f32, field: VisualField) -> PatternSample {
    let warp = mix(0.10, 0.35, field.distortion);
    let q = p + vec2<f32>(sin(p.y * 3.7 + t * 0.6), cos(p.x * 3.2 - t * 0.5)) * warp;
    let a = sin(q.x * 11.0 + q.y * 8.0 + t * 0.4);
    let b = sin(q.y * 13.0 - q.x * 6.0 - t * 0.3);
    let c = sin((q.x + q.y) * 17.0 + t * 0.2);
    let ridge = abs(a * b + 0.30 * c);
    let line = exp2(-ridge * 7.0);
    let veil = exp2(-ridge * 1.6);
    return PatternSample(
        clamp(line * 0.9 + veil * 0.15, 0.0, 1.0),
        smoothstep(0.03, 0.6, line + veil * 0.7),
        line * 0.7 + veil * 0.1,
        0.6,
    );
}
