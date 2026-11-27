fn glass_lamp(offset: vec2<f32>, radii: vec2<f32>) -> f32 {
    let normalized = offset / radii;
    return exp2(-dot(normalized, normalized));
}

fn shade(input: ShaderInput) -> vec4<f32> {
    let settings = input.params[0];
    let half_size = max(input.size * 0.5 - vec2<f32>(settings.x), vec2<f32>(1.0));
    let offset = input.position - input.size * 0.5;
    let unit = offset / half_size;
    let distance = sd_rounded_rect(offset, half_size, vec4<f32>(settings.y));
    let rim = 1.0 - smoothstep(0.35, 1.7, abs(distance));
    let haze = exp2(-abs(distance) / 13.0) * (1.0 - smoothstep(22.0, 54.0, distance));
    let corner_fill = 1.0 - smoothstep(-18.0, 2.0, distance);

    let warm_lamp = glass_lamp(unit - vec2<f32>(-0.92, -0.95), vec2<f32>(0.55, 0.40));
    let cool_lamp = glass_lamp(unit - vec2<f32>(0.96, 0.91), vec2<f32>(0.45, 0.41));
    let warm = warm_lamp * (0.20 * rim + 0.07 * haze + 0.08 * corner_fill);
    let cool = cool_lamp * (0.22 * rim + 0.07 * haze + 0.08 * corner_fill);

    // Position and pigment are weighted by the spring for each row. Changing
    // targets moves the light on the glass without changing any layout bounds.
    let highlight_y = (settings.w * 2.0 - 1.0);
    let row_light = glass_lamp(vec2<f32>(abs(unit.x) - 1.0, unit.y - highlight_y), vec2<f32>(0.46, 0.42));
    let live = settings.z * row_light * (0.35 * rim + 0.21 * haze + 0.055 * corner_fill);

    let alpha = warm + cool + live;
    let tint = (vec3<f32>(0.92, 0.83, 0.74) * warm +
        vec3<f32>(0.75, 0.72, 0.91) * cool +
        input.colors[0].rgb * live) / max(alpha, 0.0001);
    return vec4<f32>(tint, saturate(alpha));
}
