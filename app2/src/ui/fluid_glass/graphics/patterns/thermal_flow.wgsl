fn pattern_thermal_flow(p: vec2<f32>, t: f32, field: VisualField) -> PatternSample {
    let seed = field.seed * 0.017;
    let drift = vec2<f32>(sin(t * 0.13 + seed), cos(t * 0.11 - seed)) * 0.16;
    let domain = p + drift;
    let octaves = i32(field.octaves);
    let q = vec2<f32>(
        visual_fbm(domain + vec2<f32>(seed, 0.0), octaves),
        visual_fbm(domain + vec2<f32>(5.2 + seed, 1.3), octaves),
    );
    let warped = domain + 3.2 * q;
    let broad = visual_fbm(warped, octaves);
    // The fine crests ride the broad level set instead of paying for another
    // three octaves of noise in a second domain.
    let fine = 0.5 + 0.5 * sin(warped.x * 6.1 - warped.y * 4.7 + broad * 5.0 + t * 0.12);
    let broad01 = 0.5 + 0.5 * broad;
    let fold = abs(broad01 - 0.52);
    let band = exp2(-pow(fold / mix(0.18, 0.075, field.softness), 2.0));
    let glow = smoothstep(0.24, 0.84, broad01) * (0.65 + 0.35 * fine);
    let value = clamp(glow * 0.68 + band * 0.72, 0.0, 1.0);
    let coverage = smoothstep(0.03, 0.36, glow + band * 0.85);
    let emission = clamp(band * 0.62 + pow(max(fine - 0.40, 0.0), 2.0) * 1.1, 0.0, 1.0);
    return PatternSample(value, coverage, emission, broad);
}
