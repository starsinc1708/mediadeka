const AIR_TO_GLASS: f32 = 1.0 / 1.48;

// The x component displaces the light sample in DIP. The y component is the
// signed focusing response at the flank of a rib.
fn fluted_glass(pixel: vec2<f32>, uv: vec2<f32>, junction: vec2<f32>, field: VisualField) -> vec2<f32> {
    let far_right = smoothstep(0.60, 0.84, uv.x) * (1.0 - smoothstep(0.43, 0.69, uv.y));
    let clear_patch = light_field(uv, vec2<f32>(0.76, 0.30), vec2<f32>(0.05, 0.16));
    let curtain = far_right * (1.0 - 0.65 * clear_patch);
    let near_junction = light_field(pixel, junction, vec2<f32>(158.0, 114.0)) * 0.29;
    let page_lens = mix(1.0, 0.72 + 0.62 * field.intensity, field.coverage);
    let strength = max(curtain * page_lens, near_junction);
    if strength < 0.002 { return vec2<f32>(0.0); }

    // Return to the undisplaced ray with zero slope at each rib boundary.
    // A sawtooth reverses the ray in one pixel and aliases when resampled.
    let groove_axis = pixel.x + 19.0 * sin(uv.x * 15.2 + uv.y * 4.8)
        + 14.0 * sin(uv.x * 36.0 - uv.y * 5.7);
    let variation_weight = field.coverage * 0.45;
    let tilt = variation_weight * mix(-36.0, 36.0, 0.5 + 0.5 * sin(field.rotation));
    let pitch = 23.0 + variation_weight * mix(-5.0, 8.0, field.softness);
    let ridge = (groove_axis + uv.y * tilt) / pitch;
    let variation = hash2d(vec2<i32>(i32(floor(ridge)), 29));
    let local = fract(ridge) * 2.0 - 1.0;
    let seam = smoothstep(0.0, 0.20, 1.0 - abs(local));
    let slope = local * seam * mix(0.69, 0.95, variation);
    let normal = vec3<f32>(slope, 0.0, sqrt(1.0 - slope * slope));
    let ray = refract(vec3<f32>(0.0, 0.0, -1.0), normal, AIR_TO_GLASS);
    let reach = mix(72.0, 105.0, variation) * (0.82 + 0.18 * cos(uv.y * 10.0 + uv.x * 3.0));
    let flank = smoothstep(0.10, 0.89, local) - smoothstep(0.18, 0.94, -local);
    return strength * vec2<f32>(reach * ray.x / -ray.z, flank * seam);
}

const TRACK_GLITCH_SHIFT_PX: f32 = 8.0;
const TRACK_GLITCH_SPLIT_RATIO: f32 = 0.48;
const TRACK_GLITCH_SHADE: f32 = 0.22;

// The pulse shares the track-change clock with the optical reflection. The
// clipped light field keeps the displaced scanlines local to the fluted area.
fn track_glitch(pixel: vec2<f32>, uv: vec2<f32>, pulse: f32, dpi: f32) -> f32 {
    let spot = (uv - vec2<f32>(0.88, 0.27)) / vec2<f32>(0.105, 0.107);
    let local = 1.0 - smoothstep(0.52, 1.0, dot(spot, spot));
    if local <= 0.0 { return 0.0; }
    let envelope = smoothstep(0.0, 0.20, pulse) * (1.0 - smoothstep(0.42, 0.88, pulse));
    if envelope <= 0.0 { return 0.0; }
    let band = i32(floor(pixel.y * dpi / 4.0));
    let tick = i32(floor(pulse * 7.0));
    let pick = hash2d(vec2<i32>(band, tick)) * 2.0 - 1.0;
    return TRACK_GLITCH_SHIFT_PX * pick * smoothstep(0.65, 1.0, abs(pick)) * envelope * local;
}

fn glitch_transmission(shift: f32) -> f32 {
    // Darker shifted ribs stay visible in the graded field without raising its peak.
    return 1.0 - TRACK_GLITCH_SHADE * abs(shift) / TRACK_GLITCH_SHIFT_PX;
}

fn audio_glitch(pixel: vec2<f32>, uv: vec2<f32>, onset: f32, dpi: f32, time: f32) -> f32 {
    let spot = (uv - vec2<f32>(0.88, 0.27)) / vec2<f32>(0.12, 0.11);
    let local = 1.0 - smoothstep(0.45, 1.0, dot(spot, spot));
    let band = i32(floor(pixel.y * dpi / 5.0));
    let tick = i32(floor(time * 9.0));
    let pick = hash2d(vec2<i32>(band, tick + 541)) * 2.0 - 1.0;
    return 3.0 * onset * pick * smoothstep(0.72, 1.0, abs(pick)) * local;
}

fn film_grain(coord: vec2<f32>, uv: vec2<f32>, time: f32, animated: f32, raster_scale: f32) -> f32 {
    // Grain advances with the material phase and stays one physical-ish cell
    // wide when the material is rasterized below device resolution.
    let tick = i32(select(0.0, floor(time * 24.0), animated > 0.5));
    let cell = vec2<i32>(floor(coord / max(raster_scale, 0.25)));
    let phase = vec2<i32>(tick * 17, tick * 31);
    let grain = hash2d(cell + phase) + hash2d(cell + phase + vec2<i32>(37, 101)) - 1.0;
    let fade = smoothstep(0.28, 0.91, uv.x);
    return grain * mix(0.004, 0.014, fade);
}
