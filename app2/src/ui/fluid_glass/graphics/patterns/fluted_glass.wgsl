fn pattern_fluted_glass(p: vec2<f32>, t: f32, field: VisualField) -> PatternSample {
    let drift = sin(t * 0.18) * 0.15;
    let axis = p.x + field.distortion * 0.25 * sin(p.y * 2.1 + drift) + field.seed * 0.01;
    let rib = 0.5 + 0.5 * sin(axis * 64.0 + t * 0.35);
    let soft = exp2(-pow((fract(axis * 2.2) - 0.5) / 0.42, 2.0));
    let vertical = 0.5 + 0.5 * sin(p.y * 2.3 - t * 0.2 + drift);
    let value = clamp(pow(rib, 3.0) * 0.75 + soft * vertical * 0.30, 0.0, 1.0);
    return PatternSample(value, 0.34 + 0.58 * pow(rib, 2.0), rib * 0.42, 0.7);
}
