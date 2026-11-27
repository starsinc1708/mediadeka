fn pattern_signal_distortion(p: vec2<f32>, t: f32, field: VisualField) -> PatternSample {
    let rows = 64.0 + 32.0 * field.distortion;
    let row = floor(p.y * rows);
    let tick = floor(t * 6.0);
    let gate = hash2d(vec2<i32>(i32(row), i32(tick) + i32(field.seed)));
    let energized = smoothstep(0.84, 0.98, gate);
    let shift = (gate - 0.5) * 0.45 * energized;
    let q = p + vec2<f32>(shift, 0.0);
    let scan = 0.5 + 0.5 * sin(q.y * rows * 1.4 + t * 2.0);
    let blocks = smoothstep(
        0.55,
        0.92,
        hash2d(vec2<i32>(i32(floor(q.x * 16.0)), i32(row) + i32(field.seed))),
    );
    let value = clamp(scan * 0.55 + blocks * 0.55 * energized + 0.1, 0.0, 1.0);
    return PatternSample(value, 0.25 + 0.55 * energized + 0.15 * scan, energized * 0.35, 0.25);
}
