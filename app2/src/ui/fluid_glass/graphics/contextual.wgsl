// The procedural effects share the same warped coordinates and the same
// artwork light. Refraction, dispersion and scattering sample this field.
struct ContextPattern {
    thermal: f32,
    fluted: f32,
    interference: f32,
    caustic: f32,
    signal: f32,
    leak: f32,
    scale: f32,
    direction: f32,
    speed: f32,
    deformation: f32,
    amount: f32,
    coverage: f32,
}

// Matched with graphics.rs. GPUI keeps the four input colors in straight
// alpha, so the data lanes do not affect premultiplication or layer opacity.
fn context_word(lane: f32) -> u32 {
    return u32(lane * 16777216.0);
}

fn context_band(lane: f32) -> f32 {
    return f32(context_word(lane) & 4095u) / 4095.0;
}

fn context_raster(lane: f32) -> f32 {
    return f32((context_word(lane) & 7u) + 1u) / 8.0;
}

fn context_exposure(word: f32) -> f32 {
    return f32(u32(word) & 1023u) / 1000.0;
}

fn context_pattern(input: ShaderInput, coverage: f32) -> ContextPattern {
    let words = vec3<u32>(
        context_word(input.colors[0].a),
        context_word(input.colors[1].a),
        context_word(input.colors[2].a),
    );
    let a = vec3<f32>((words >> vec3<u32>(12u)) & vec3<u32>(63u)) / 63.0;
    let b = vec3<f32>(words >> vec3<u32>(18u)) / 63.0;
    let variation = context_word(input.colors[3].a);
    let scale = f32((variation >> 3u) & 127u) / 127.0;
    let direction = f32((variation >> 10u) & 127u) / 127.0;
    let speed = f32(variation >> 17u) / 127.0;
    let deformation = f32(u32(input.params[1].w) >> 10u) / 127.0;
    return ContextPattern(
        a.x, b.x, a.y, b.y, a.z, b.z,
        scale, direction, speed, deformation,
        min(1.0, dot(a + b, vec3<f32>(1.0))), coverage,
    );
}

fn contextual_light(uv: vec2<f32>, warp: vec2<f32>, light: Atmosphere) -> vec3<f32> {
    let profile = light.context;
    let coordinates = uv + warp * mix(0.72, 1.68, profile.deformation);
    let angle = mix(-0.53, 0.53, profile.direction);
    let p = rotate((coordinates - vec2<f32>(0.87, 0.29)) * vec2<f32>(2.25, 1.36), angle)
        * mix(0.79, 1.29, profile.scale);
    let phase = light.time * mix(0.08, 0.31, profile.speed);
    let drift = 0.14 * sin(phase + p.x * 1.21);
    var color = vec3<f32>(0.0);

    if profile.thermal > 0.0 {
        let curve = p.y + 0.28 * sin(3.31 * p.x + drift) - 0.33 * p.x * p.x;
        let warm = ribbon(curve, -0.32, 0.24);
        let inner = ribbon(curve, -0.13, 0.073);
        let cool = ribbon(curve, 0.19 + 0.16 * sin(p.x * 2.9 - phase), 0.19);
        let aperture = ribbon(p.x + 0.27 * p.y, 0.20, 0.63);
        color += profile.thermal * aperture
            * (0.57 * warm * light.hot + 0.38 * inner * light.warm_light + 0.42 * cool * light.cyan);
    }
    if profile.fluted > 0.0 {
        let glass_wave = p.y + 0.28 * cos(3.64 * p.x + drift) + 0.18 * p.x;
        let light_gap = ribbon(glass_wave, -0.09, 0.18) * ribbon(p.x, 0.04, 0.52);
        let reflection = ribbon(glass_wave, 0.07, 0.085) * ribbon(p.x, 0.22, 0.48);
        let fade = smoothstep(-0.63, 0.51, p.x + 0.72 * p.y);
        color += profile.fluted
            * (light_gap * mix(0.68 * light.hot, 0.50 * light.cyan, fade)
                + reflection * 0.41 * light.warm_light);
    }
    if profile.interference > 0.0 {
        let a = length((p - vec2<f32>(0.27, 0.16)) * vec2<f32>(1.0, 0.8));
        let b = length(p - vec2<f32>(-0.39, -0.37));
        let arcs = sin(a * 24.0 - phase) + sin(b * 22.0 - phase * 0.79);
        let ridge = smoothstep(0.68, 1.9, abs(arcs));
        let aperture = ribbon(a, 0.46, 0.31) * ribbon(p.y, -0.02, 0.60);
        color += profile.interference * aperture
            * (0.15 * light.blue + ridge * mix(0.46 * light.rose, 0.49 * light.cyan, step(0.0, arcs)));
    }
    if profile.caustic > 0.0 {
        let edge = p.y + 0.43 * sin(p.x * 4.2 + phase) + 0.12 * sin(p.x * 7.7 - p.y);
        let fold = ribbon(edge, -0.02, 0.16);
        let aperture = ribbon(p.x + p.y * 0.13, 0.14, 0.61);
        let specular = ribbon(edge + 0.09 * sin(phase + p.x * 5.3), 0.12, 0.036);
        let echo = ribbon(edge, -0.30 + p.x * 0.12, 0.065);
        color += profile.caustic * aperture
            * (0.44 * fold * light.hot + 0.50 * specular * light.warm_light + 0.40 * echo * light.cyan);
    }
    if profile.signal > 0.0 {
        let strip = floor(p.y * 27.0);
        let offset = 0.26 * sin(strip * 2.79 + phase);
        let gate = smoothstep(0.09, 0.91, sin(strip * 1.91 + 0.62));
        let track = ribbon(p.x + offset + 0.34 * sin(p.y * 3.4), 0.20, 0.38);
        let aperture = ribbon(p.y, -0.07, 0.43);
        let fringe = ribbon(p.x + offset - 0.14, 0.19, 0.24);
        color += profile.signal * aperture * gate
            * (0.48 * track * light.hot + 0.42 * fringe * light.cyan);
    }
    if profile.leak > 0.0 {
        let sweep = rotate(p - vec2<f32>(0.26, -0.28 - drift), -0.71);
        let bright = light_field(sweep, vec2<f32>(0.09, -0.14), vec2<f32>(0.35, 0.52));
        let dye = light_field(sweep, vec2<f32>(-0.15, 0.17), vec2<f32>(0.48, 0.64));
        let afterglow = ribbon(sweep.y + 0.14 * sin(sweep.x * 4.1), 0.42, 0.24)
            * ribbon(sweep.x, 0.18, 0.52);
        color += profile.leak * (0.65 * bright * light.hot + 0.43 * dye * light.rose
            + 0.31 * afterglow * light.cyan);
    }
    return color;
}
