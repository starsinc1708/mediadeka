fn gaussian(distance: f32, radius: f32) -> f32 {
    let ratio = distance / radius;
    return exp2(-ratio * ratio);
}

fn plume(p: vec2<f32>, center: vec2<f32>, radii: vec2<f32>) -> f32 {
    let offset = (p - center) / radii;
    return exp2(-dot(offset, offset));
}

// Incorporate the pixel footprint so the line remains smooth at different DPI
// and window sizes. The highlight is evaluated at native device resolution.
fn ridge(distance: f32, radius: f32) -> f32 {
    let footprint = fwidth(distance) * 0.65;
    return gaussian(distance, sqrt(radius * radius + footprint * footprint));
}

