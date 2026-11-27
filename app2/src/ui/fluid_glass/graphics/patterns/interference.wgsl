fn pattern_interference(p: vec2<f32>, t: f32, field: VisualField) -> PatternSample {
    let phase = t * 0.9;
    let a = vec2<f32>(-0.42, 0.18) * (1.0 + 0.15 * sin(t * 0.12));
    let b = vec2<f32>(0.44, -0.12 + 0.05 * cos(t * 0.09));
    let c = vec2<f32>(0.02, 0.46);
    let wave = sin(length(p - a) * 31.0 - phase)
        + sin(length(p - b) * 27.0 - phase * 0.83)
        + 0.65 * sin(length(p - c) * 35.0 - phase * 0.61);
    let level = wave / 2.65;
    let lines = pow(clamp(0.5 + 0.5 * level, 0.0, 1.0), 7.0);
    let veil = smoothstep(0.2, 0.8, abs(level));
    return PatternSample(
        clamp(lines * 0.9 + veil * 0.12, 0.0, 1.0),
        0.2 + 0.65 * lines,
        lines * 0.5,
        0.4,
    );
}
