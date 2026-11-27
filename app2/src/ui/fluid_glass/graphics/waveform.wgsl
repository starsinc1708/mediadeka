fn amplitude(input: ShaderInput, index: u32) -> f32 {
    return input.params[index / 4u][index % 4u];
}

fn shade(input: ShaderInput) -> vec4<f32> {
    // Interpolate adjacent measured levels. The centre taper only shapes the
    // visible ends of each strip; it never creates a pulse without PCM.
    let columns = 32.0;
    let depth = input.uv.y - 0.5;
    let bow = depth * abs(depth) * (0.32 + 0.46 * sin(input.uv.x * 6.283));
    let ribbon = clamp(input.uv.x * columns + bow, 0.0, columns - 0.001);
    let column_centre = floor(ribbon) + 0.5;
    let sample_position = column_centre / columns * 15.0;
    let index = min(15u, u32(sample_position));
    let level = mix(amplitude(input, index), amplitude(input, min(15u, index + 1u)), fract(sample_position));
    let fade = smoothstep(0.0, 0.22, input.uv.x) * (1.0 - smoothstep(0.78, 1.0, input.uv.x));
    // Stretch the middle of the normalized RMS range so quiet passages and
    // peaks remain distinguishable, with a ceiling inside the player glass.
    let response = mix(level, smoothstep(0.18, 0.73, level), 0.82);
    let half_height = response * 0.42 * input.size.y * fade;
    let x = (ribbon - column_centre) * input.size.x / columns;
    let y = input.position.y - input.size.y * 0.5;
    let distance = max(abs(x) - 0.78, abs(y) - half_height);
    let line = coverage(distance, input.scale_factor);
    let scatter_x = (x - 0.52 * depth) / (1.4 + 1.1 * abs(depth));
    let scatter_y = max(abs(y) - half_height * 0.41, 0.0) / (2.0 + 2.2 * level);
    let scatter = exp2(-scatter_x * scatter_x - scatter_y * scatter_y);
    let base = mix(input.colors[0].rgb, input.colors[1].rgb, smoothstep(0.0, 1.0, input.uv.x));
    let color = mix(base, input.colors[2].rgb, 0.18 * line);
    let opacity = input.colors[0].a * fade * smoothstep(0.0, 0.1, level) * (0.34 * line + 0.18 * scatter);
    return vec4<f32>(color, opacity);
}
