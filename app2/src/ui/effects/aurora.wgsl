fn shade(input: ShaderInput) -> vec4<f32> {
    let uv = input.uv;
    let fold = input.params[0];
    let aspect = input.size.x / max(input.size.y, 1.0);
    let spread = mix(1.0, 1.18, smoothstep(0.9, 2.25, aspect));
    let x = 0.5 + (uv.x - 0.5) * spread;
    let p = vec2<f32>(x, uv.y);

    // One revolution during the twelve-second keyframe track. Offset the two
    // waves so the surfaces breathe independently.
    let phase = input.time * 0.5235988;
    let upper_wave = sin(phase + x * 4.8);
    let lower_wave = sin(phase * 2.0 - x * 4.0 + 1.35);
    let drift = sin(phase + 0.4);

    var color = mix(
        vec3<f32>(0.060, 0.062, 0.073),
        vec3<f32>(0.081, 0.082, 0.092),
        1.0 - uv.y,
    );
    color += vec3<f32>(0.039, 0.046, 0.064) *
        plume(p, vec2<f32>(0.62 + fold.z, 0.07), vec2<f32>(0.34, 0.36));
    color += vec3<f32>(0.036, 0.043, 0.061) *
        plume(p, vec2<f32>(0.04, 0.83 + fold.z), vec2<f32>(0.34, 0.31));
    color += vec3<f32>(0.063, 0.057, 0.085) *
        plume(p, vec2<f32>(0.91, 0.61 - fold.z), vec2<f32>(0.26, 0.21));

    let upper_edge = 0.31 - 0.68 * x - 1.58 * x * x + fold.x + 0.028 * upper_wave;
    let upper_distance = (uv.y - upper_edge) / (1.0 + 1.4 * max(x, 0.0));
    let upper_lamp = gaussian(x - 0.119 - 0.10 * drift, 0.23);
    let upper_glare = gaussian(upper_distance + 0.018, 0.079);
    let upper_rim = ridge(upper_distance, 0.0055);
    let upper_inside = 1.0 - smoothstep(-0.055, 0.040, upper_distance);
    let upper_chroma = saturate(0.3 + x * 0.8 + fold.w);
    let upper_light = mix(vec3<f32>(0.75, 0.66, 0.55), vec3<f32>(0.67, 0.69, 0.79), upper_chroma);
    let upper_core = mix(upper_light, vec3<f32>(0.94, 0.89, 0.81), gaussian(x - 0.15, 0.19));
    color += (upper_light * 0.24 * upper_glare + upper_core * 0.29 * upper_rim) * upper_lamp;
    color += vec3<f32>(0.13, 0.11, 0.096) * upper_inside *
        plume(p, vec2<f32>(0.05, 0.2), vec2<f32>(0.31, 0.22));

    // A second broad fold sits behind the upper edge. At the opposite side,
    // a cool reflection shows most strongly near the top of the window.
    let lilac_edge = 0.59 - 1.79 * x + 0.54 * x * x + fold.y - 0.023 * upper_wave;
    let lilac_distance = (uv.y - lilac_edge) / 1.45;
    let lilac_lamp = gaussian(x - 0.18 + 0.05 * drift, 0.19);
    color += vec3<f32>(0.43, 0.40, 0.55) * lilac_lamp *
        (0.12 * gaussian(lilac_distance, 0.095) + 0.075 * ridge(lilac_distance, 0.007));

    let slate_edge = 0.84 - 0.29 * uv.y + 0.087 * uv.y * uv.y + fold.y * 0.68;
    color += vec3<f32>(0.42, 0.48, 0.58) * gaussian(uv.y - 0.13, 0.32) *
        (0.085 * gaussian(x - slate_edge, 0.075) + 0.036 * ridge(x - slate_edge, 0.008));

    let lower_edge = 1.09 - 0.14 * x - 0.30 * smoothstep(0.52, 1.07, x) +
        fold.z + 0.036 * lower_wave;
    let lower_distance = uv.y - lower_edge;
    let lower_lamp = gaussian(x - 0.87 + 0.10 * drift, 0.26);
    let lower_rim = ridge(lower_distance, 0.005);
    let lower_bloom = gaussian(lower_distance + 0.019, 0.072);
    let lower_inside = smoothstep(-0.025, 0.068, lower_distance);
    let lower_chroma = saturate(0.37 + 1.06 * (x - 0.70) - fold.w);
    let lower_light = mix(vec3<f32>(0.52, 0.48, 0.67), vec3<f32>(0.76, 0.70, 0.65), lower_chroma);
    color += lower_light * lower_lamp * (0.27 * lower_bloom + 0.32 * lower_rim);
    color += vec3<f32>(0.088, 0.086, 0.112) * lower_inside * lower_lamp *
        gaussian(lower_distance, 0.20);
    color += vec3<f32>(0.043, 0.042, 0.054) *
        gaussian(lower_distance + 0.12, 0.052) * gaussian(x - 0.95, 0.32);

    let center = plume(p, vec2<f32>(0.5, 0.55), vec2<f32>(0.24, 0.34));
    color *= 1.0 - 0.16 * center;

    // One quantization step is enough to suppress bands in the 8-bit target.
    // Keep it fixed to the screen grid so the dark surface has no moving grain.
    let noise = (hash2d(vec2<i32>(input.frag_coord)) - 0.5) / 255.0;
    return vec4<f32>(saturate(color + vec3<f32>(noise)), 1.0);
}
