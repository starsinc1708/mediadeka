struct Atmosphere {
    time: f32,
    canvas: vec3<f32>,
    hot: vec3<f32>,
    rose: vec3<f32>,
    blue: vec3<f32>,
    cyan: vec3<f32>,
    warm_light: vec3<f32>,
    intensity: f32,
    energy: f32,
    audio: AudioModulation,
    context: VisualField,
}

fn unpack_rgb(value: f32) -> vec3<f32> {
    let packed = u32(value);
    return vec3<f32>(f32(packed >> 16u), f32((packed >> 8u) & 255u), f32(packed & 255u)) / 255.0;
}

fn atmosphere_for(
    input: ShaderInput,
    animated: f32,
    upper_weight: f32,
    context: VisualField,
) -> Atmosphere {
    let references = input.params[2];
    let audio = visual_audio(input);
    var light = Atmosphere(
        input.time,
        unpack_rgb(references.x),
        input.colors[0].rgb,
        input.colors[1].rgb,
        input.colors[2].rgb,
        input.colors[3].rgb,
        unpack_rgb(references.y),
        mix(0.82, 1.0, animated) * (1.0 + 0.26 * audio.light),
        audio.light,
        audio,
        context,
    );
    if upper_weight > 0.0 {
        let upper = input.params[3];
        light.canvas = mix(light.canvas, unpack_rgb(references.z), upper_weight);
        light.hot = mix(light.hot, unpack_rgb(upper.x), upper_weight);
        light.rose = mix(light.rose, unpack_rgb(upper.y), upper_weight);
        light.blue = mix(light.blue, unpack_rgb(upper.z), upper_weight);
        light.cyan = mix(light.cyan, unpack_rgb(upper.w), upper_weight);
        light.warm_light = mix(light.warm_light, unpack_rgb(references.w), upper_weight);
    }
    return light;
}

fn domain_warp(uv: vec2<f32>, time: f32, mid: f32, octaves: i32) -> vec2<f32> {
    let drift = vec2<f32>(0.031, -0.025) * time;
    let p = rotate(uv * vec2<f32>(2.65, 1.72), -0.34);
    let crossflow = vec2<f32>(sin(5.2 * uv.y + 0.9 * uv.x), sin(5.5 * uv.x - 2.3 * uv.y));
    // Bend the noise domain before applying it to the large light folds. The smooth
    // first bend avoids a second fBm evaluation for every refracted color channel.
    let folded = p + 0.59 * crossflow + drift;
    let spectral_shift = mid * smoothstep(0.52, 0.90, uv.x) * vec2<f32>(0.023, -0.016);
    return spectral_shift + vec2<f32>(0.28, 0.22) * vec2<f32>(
        visual_fbm(folded + vec2<f32>(7.2, 1.3), octaves),
        visual_fbm(folded.yx + vec2<f32>(2.8, 9.1), octaves),
    );
}

// Window-sized field with its source in the cover or portrait area. Reuse the
// same low-frequency warp as the atmosphere so both fields drift together.
fn artwork_influence(pixel: vec2<f32>, size: vec2<f32>, warp: vec2<f32>, portrait: bool) -> f32 {
    let center = vec2<f32>(
        select(clamp(size.x * 0.12, 58.0, 174.0), min(size.x * 0.76, ARTIST_STAGE_MAX_WIDTH * 0.80), portrait),
        clamp(size.y * 0.26, 62.0, 205.0),
    );
    let radius = vec2<f32>(clamp(size.x * 0.43, 110.0, 720.0), clamp(size.y * 0.34, 84.0, 320.0));
    let p = (pixel - center) / radius;
    let bend = vec2<f32>(0.13 * sin(p.y * 2.5 + p.x), 0.14 * sin(p.x * 2.9));
    let flow = rotate(p + bend + warp * 1.15, select(-0.23, 0.29, portrait))
        * vec2<f32>(0.83, 1.16);
    return exp(-1.45 * dot(flow, flow));
}

fn flowing_light(p: vec2<f32>, drift: vec2<f32>, light: Atmosphere) -> vec3<f32> {
    // Two independently warped level sets leave unlit space between them. The
    // unequal bands on either side of each fold separate its warm and cool faces.
    let top = rotate(p - vec2<f32>(0.925, 0.15) - drift, -0.48) / vec2<f32>(0.22, 0.31);
    let top_fold = length(vec2<f32>(top.x + 0.29 * top.y * top.y, top.y)) - 0.80;
    let warm_face = 1.0 - smoothstep(-0.46, 0.62, top.y + 0.38 * top.x);
    let warm = ribbon(top_fold, -0.15, 0.23);
    let lip = ribbon(top_fold, -0.005, 0.080);
    let cool_face = ribbon(top_fold, 0.078, 0.124);
    let top_light = warm * mix(0.16 * light.rose, 0.44 * light.hot, warm_face)
        + lip * mix(0.14, 0.24, warm_face) * light.warm_light
        + cool_face * mix(0.35, 0.15, warm_face) * light.cyan;

    let lower = rotate(p - vec2<f32>(1.04, 0.57) + drift.yx, 0.63) / vec2<f32>(0.30, 0.36);
    let lower_fold = length(vec2<f32>(lower.x, lower.y - 0.42 * sin(2.10 * lower.x))) - 0.83;
    let dark_side = ribbon(lower_fold, -0.24, 0.22);
    let crest = ribbon(lower_fold, 0.07, 0.108);
    let tail = ribbon(lower_fold, 0.17, 0.13);
    let reflect = smoothstep(-0.55, 0.58, lower.y - lower.x * 0.72);
    // The crossing wave fronts modulate only the reflection on this fold.
    let interference = sin(dot(lower, vec2<f32>(9.7, 8.8)))
        * sin(dot(lower, vec2<f32>(-7.5, 12.0)) + 0.7);
    let lower_light = 0.10 * dark_side * light.blue
        + crest * mix(0.11 * light.rose, 0.29 * light.cyan, reflect) * (0.94 + 0.06 * interference)
        + tail * mix(0.12, 0.19, reflect) * light.blue;
    var bloom = vec3<f32>(0.0);
    if visual_flag(light.context, VISUAL_BLOOM) {
        // Broaden the two bright crests in their own field coordinates so
        // the halo extends into adjacent dark material.
        let top_halo = ribbon(top_fold, -0.005, 0.17);
        let lower_halo = ribbon(lower_fold, 0.07, 0.21);
        bloom = (0.065 * top_halo * light.warm_light + 0.045 * lower_halo * light.cyan)
            * (1.0 + 0.22 * light.audio.light);
    }
    return top_light + lower_light + bloom;
}

// The source fields are intrinsically soft. Evaluating them at the glass ray
// position avoids an intermediate texture and a full-window blur pass.
fn atmosphere_sample(uv: vec2<f32>, warp: vec2<f32>, light: Atmosphere) -> vec3<f32> {
    let p = uv + warp;
    let slow = light.time * 0.075;
    let drift = vec2<f32>(0.043 * sin(slow), 0.031 * sin(slow * 0.73));
    let upper_haze = light_field(p, vec2<f32>(0.79, 0.105) + drift, vec2<f32>(0.19, 0.17));
    let lower_haze = light_field(p, vec2<f32>(1.05, 0.67), vec2<f32>(0.31, 0.42));
    let shoulder = light_field(p, vec2<f32>(0.08, 0.10), vec2<f32>(0.23, 0.40));
    let shade = 0.12 * upper_haze * light.hot + 0.07 * lower_haze * light.blue
        + flowing_light(p, drift, light);
    let center_dim = smoothstep(0.48, 0.91, uv.x);
    let field = shade * center_dim;
    return light.canvas + light.intensity * (field + 0.032 * shoulder * light.rose);
}

/// The motif is evaluated once per pixel and composited after the glass, so
/// refraction, tint and the bevel still shape the atmosphere around it while
/// the pattern stays crisp instead of repeating the full shader per ray.
fn pattern_light(uv: vec2<f32>, light: Atmosphere) -> vec3<f32> {
    let sample = pattern_layer(uv, light.time, light.context, light.audio);
    let highlight = select(
        vec3<f32>(0.0),
        light.warm_light * (1.0 + 0.22 * light.audio.light),
        visual_flag(light.context, VISUAL_BLOOM),
    );
    let palette = PatternPalette(
        array<vec3<f32>, 4>(light.hot, light.rose, light.blue, light.cyan),
        highlight,
        light.canvas,
    );
    // The whole shell follows the record: the motif gains a little energy with
    // bass and mids while the palette light does the larger breathing.
    return pattern_color(sample, palette)
        * light.context.intensity
        * (0.84 + 0.28 * light.energy);
}
