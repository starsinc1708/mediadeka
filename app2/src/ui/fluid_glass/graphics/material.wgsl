struct GlassRay {
    center: vec2<f32>,
    dispersion: vec2<f32>,
    scatter: vec2<f32>,
    bevel: f32,
}

// A lamp under the now-playing cover lights the horizontal and vertical
// channels of the same surface. The two lengths share the elbow as origin.
fn guided_light(pixel: vec2<f32>, junction: vec2<f32>) -> f32 {
    let p = pixel - junction;
    let along_player = max(p.x, 0.0) * 0.0035 + abs(p.y - 26.0) * 0.020;
    let along_rail = max(-p.y, 0.0) * 0.0050 + abs(p.x + 20.0) * 0.024;
    let on_player = smoothstep(-32.0, 34.0, p.y);
    return exp2(-mix(along_rail, along_player, on_player));
}

fn bevel_width(pixel: vec2<f32>, normal: vec2<f32>, junction: vec2<f32>, right_join: vec2<f32>) -> f32 {
    let rail_bend = light_field(pixel, junction + vec2<f32>(15.0, -14.0), vec2<f32>(88.0, 94.0));
    let dock_bend = light_field(pixel, right_join + vec2<f32>(-15.0, -14.0), vec2<f32>(74.0, 88.0));
    let diagonal = 2.0 * abs(normal.x * normal.y);
    return 5.5 + 3.5 * abs(normal.y) + 7.0 * max(rail_bend, dock_bend) + 3.0 * diagonal;
}

// Bend the viewing ray through a convex cross-section. Travel through the
// tinted slab displaces the background across the rounded shoulder; farther
// from the contour, micro-ribs still change the transmission angle.
fn glass_ray(
    uv: vec2<f32>,
    normal: vec2<f32>,
    depth: f32,
    width: f32,
    flutes: vec2<f32>,
    dimensions: vec2<f32>,
    scale: f32,
) -> GlassRay {
    let bevel = 1.0 - smoothstep(0.0, width, depth);
    let slope = 0.86 * bevel;
    let face = normalize(vec3<f32>(normal * slope, sqrt(1.0 - slope * slope)));
    let transmitted = refract(vec3<f32>(0.0, 0.0, -1.0), face, 1.0 / 1.48);
    let prism = transmitted.xy * (21.0 + 12.0 * bevel) / max(0.5, -transmitted.z);
    let ribs = vec2<f32>(0.58 * flutes.x, 0.30 * flutes.y);
    let curvature = min(1.0, 2.0 * abs(normal.x * normal.y));
    let dispersion = normal * bevel * (0.22 + 1.40 * curvature) / scale;
    // A narrow tangent lobe softens the transmitted source along the edge.
    let tangent = vec2<f32>(normal.y, -normal.x);
    let scatter = (mix(vec2<f32>(0.24, 1.0), tangent, bevel) * (3.5 + 6.0 * bevel)) / dimensions;
    return GlassRay(uv + (prism + ribs) / dimensions, dispersion / dimensions, scatter, bevel);
}

fn glass_tint(
    sampled: vec3<f32>,
    depth: f32,
    width: f32,
    normal: vec2<f32>,
    lamp: f32,
    light: Atmosphere,
    reading_grade: f32,
    header_weight: f32,
    opacity: f32,
) -> vec3<f32> {
    // The titlebar sits deeper in the slab than the content behind the artwork.
    let tint = clamp(mix(0.36, 0.75, reading_grade) + 0.16 * header_weight + 0.44 * (opacity - 0.5), 0.16, 0.96);
    let fill = mix(sampled, GLASS_TINT, tint);
    let inner = (depth - width * 0.76) / max(width * 0.69, 2.0);
    let occlusion = exp2(-inner * inner) * (0.20 + 0.075 * max(normal.y, 0.0));
    let scatter = lamp * (0.025 * light.hot + 0.020 * light.rose + 0.020 * light.cyan);
    return fill * (1.0 - occlusion) + scatter * (0.62 + 0.38 * light.intensity);
}

fn junction_caustic(pixel: vec2<f32>, junction: vec2<f32>, light: Atmosphere) -> vec3<f32> {
    let p = pixel - junction;
    let bend = p.y + 14.0 - 0.45 * p.x - 0.0027 * p.x * p.x;
    let spread = light_field(p, vec2<f32>(-29.0, 5.0), vec2<f32>(134.0, 90.0));
    let warm_ridge = bend / 12.0;
    let cool_ridge = (bend - 14.0) / 25.0;
    let warm = exp2(-warm_ridge * warm_ridge) * smoothstep(-132.0, 30.0, p.x);
    let cool = exp2(-cool_ridge * cool_ridge) * (1.0 - smoothstep(-125.0, 46.0, p.x));
    return spread * light.intensity * (0.23 * warm * light.hot + 0.13 * cool * light.cyan);
}

fn edge_lighting(
    distance: f32,
    normal: vec2<f32>,
    pixel: vec2<f32>,
    uv: vec2<f32>,
    junction: vec2<f32>,
    right_join: vec2<f32>,
    light: Atmosphere,
    scale: f32,
    pulse: f32,
) -> vec4<f32> {
    let left_elbow = light_field(pixel, junction + vec2<f32>(33.0, -18.0), vec2<f32>(98.0, 78.0));
    let right_elbow = light_field(pixel, right_join + vec2<f32>(-33.0, -15.0), vec2<f32>(84.0, 68.0));
    let overhead = light_field(uv, vec2<f32>(0.08, 0.12), vec2<f32>(0.15, 0.33));
    let lower_right = light_field(uv, vec2<f32>(0.83, 0.96), vec2<f32>(0.17, 0.11));
    let rail_key = ribbon(uv.y, 0.27, 0.23)
        * ribbon(pixel.x - junction.x, 0.0, 14.0) * max(normal.x, 0.0);
    let rail_rim = (1.0 - smoothstep(0.0, 22.0, abs(pixel.x - junction.x)))
        * max(normal.x, 0.0);
    let player_key = ribbon(uv.x, 0.56, 0.105)
        * ribbon(pixel.y - junction.y, 0.0, 15.0) * max(-normal.y, 0.0);
    let reflection = 0.56 * left_elbow + 0.32 * right_elbow + 0.32 * overhead
        + 0.20 * lower_right + 0.40 * rail_key + 0.58 * player_key;
    let incidence = max(dot(normal, vec2<f32>(-0.56, -0.83)), 0.0);
    let fresnel = pow(1.0 - 0.53 * incidence, 3.0);
    var gleam = 0.0;
    if pulse > 0.0 {
        let envelope = smoothstep(0.0, 0.23, pulse) * (1.0 - smoothstep(0.37, 1.0, pulse));
        let center = junction + vec2<f32>(mix(-16.0, 186.0, pulse), -8.0);
        gleam = envelope * light_field(pixel, center, vec2<f32>(89.0, 33.0));
    }
    let warm = mix(light.rose * 0.61, light.warm_light, min(1.0, 0.88 * left_elbow + 0.67 * gleam));
    let cool = mix(light.cyan * 0.73, light.warm_light, 0.30 * right_elbow);
    let color = mix(warm, cool, smoothstep(0.38, 0.90, uv.x)) * 0.79 + vec3<f32>(0.14, 0.13, 0.17);
    let width = max(0.70 / scale, 0.55 + 0.78 * reflection + 0.42 * gleam);
    let edge = (distance + 0.22) / width;
    let shoulder = (distance + 1.5 + 2.0 * reflection) / max(0.76 / scale, 0.82 + 0.34 * reflection);
    let bass_rim = 0.22 * light.audio.pulse * (left_elbow + 0.5 * player_key);
    var profile = vec3<f32>(exp2(-edge * edge));
    if visual_flag(light.context, VISUAL_REFRACTION) && visual_flag(light.context, VISUAL_DISPERSION) {
        // The bevel can catch light even when the transmitted source has
        // little colour gradient. Refract its specular lobe per channel.
        let curvature = 2.0 * abs(normal.x * normal.y);
        let spread = (0.34 + 0.20 * curvature + 0.20 * light.audio.dispersion) / (width * scale);
        let bands = edge + vec3<f32>(spread, 0.0, -spread);
        profile = exp2(-bands * bands);
    }
    let rim = profile * (0.14 + 0.12 * rail_rim + fresnel * 0.08 + reflection * 0.48 + 0.44 * gleam + bass_rim);
    let reflected = exp2(-shoulder * shoulder)
        * (0.13 * reflection + light.audio.reflection * 0.20 * left_elbow);
    let energy = rim + reflected;
    let alpha = min(max(energy.r, max(energy.g, energy.b)), 1.0);
    return vec4<f32>(color * energy / max(alpha, 0.0001), alpha);
}

fn chromatic_edge(
    sampled: vec3<f32>,
    ray: vec2<f32>,
    offset: vec2<f32>,
    warp: vec2<f32>,
    light: Atmosphere,
) -> vec3<f32> {
    let red = atmosphere_sample(ray + offset, warp, light).r;
    let blue = atmosphere_sample(ray - offset, warp, light).b;
    return vec3<f32>(red, sampled.g, blue);
}
