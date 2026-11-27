const QUARTER_TURN: f32 = 1.570796327;
const TURN: f32 = 6.283185307;

fn ribbon_density(along: f32, across: f32, width: f32, wave: f32, seed: f32) -> vec2<f32> {
    // Two softly lit folds move independently across the fog.
    let broad = sin(along * 0.021 - wave + seed);
    let loose = sin(along * 0.039 + wave + 0.12 * sin(wave) + seed * 0.74);
    let upper = across - width * (0.37 * broad + 0.10 * loose);
    let lower = across - width * (0.31 - 0.29 * loose + 0.09 * broad);
    let drift = sin(wave + seed * 0.44);
    let spot = plume(
        vec2<f32>(along, across),
        vec2<f32>(38.0 + 17.0 * drift, width * -0.13),
        vec2<f32>(57.0, width * 1.45),
    );
    let shoulder = gaussian(upper + width * 0.17, width * 0.80);
    let backing = gaussian(lower - width * 0.20, width * 0.94);
    let body = shoulder * (0.50 + 0.18 * spot) + backing * 0.22;
    let light = 0.19 * gaussian(upper, width * 0.61) * spot;
    return vec2<f32>(body, light);
}

fn dust_mote(local: vec2<f32>, scroll: f32, seed: f32, scale: f32) -> f32 {
    let span = vec2<f32>(22.0, 17.0);
    let cell_point = (local - vec2<f32>(scroll, 0.0)) / span;
    let cell = vec2<i32>(floor(cell_point));
    let wrapped_x = ((cell.x % 12) + 12) % 12;
    let random_cell = vec2<i32>(wrapped_x + i32(seed * 7.0), cell.y);
    let density = hash2d(random_cell);
    if density < 0.82 {
        return 0.0;
    }
    let offset = (hash2d_vec(random_cell) - vec2<f32>(0.5)) * 0.57;
    let distance = length((fract(cell_point) - vec2<f32>(0.5) - offset) * span);
    let radius = sqrt(0.49 + 0.19 / (scale * scale));
    return 0.84 * gaussian(distance, radius) + 0.16 * gaussian(distance, radius * 2.1);
}

fn route_ribbon(
    pixel: vec2<f32>,
    endpoints: vec4<f32>,
    timeline: vec4<f32>,
    compact: bool,
    seed: f32,
    time: f32,
    scale: f32,
) -> vec2<f32> {
    let delta = endpoints.zw - endpoints.xy;
    let distance = max(length(delta), 1.0);
    let tangent = delta / distance;
    let offset = pixel - endpoints.xy;
    let across = dot(offset, vec2<f32>(-tangent.y, tangent.x));
    let along = dot(offset, tangent);
    let u = along / distance;

    if timeline.x < 0.5 {
        let feather = 1.0 - smoothstep(43.0, 57.0, abs(across));
        let envelope = smoothstep(-0.12, 0.12, u) * (1.0 - smoothstep(0.26, 1.04, u)) * feather;
        if envelope * timeline.w < 0.003 {
            return vec2<f32>(0.0);
        }
        let wave = time * TURN / 12.0;
        let width = mix(21.0, 30.0, saturate(u));
        let fog = ribbon_density(along, across, width, wave, seed).x;
        let dust = dust_mote(vec2<f32>(along, across), time * 22.0, seed, scale);
        let strength = timeline.w * envelope;
        return vec2<f32>(0.32, dust * 0.095) * fog * strength;
    }

    let progress = timeline.y;
    let head = smoothstep(0.0, 0.61, progress) * 1.06;
    let start = smoothstep(-0.25, 0.11, u - max(0.0, head - 0.79));
    let end = 1.0 - smoothstep(head - 0.055, head + 0.28, u);
    let cross_limit = select(82.0, 55.0, compact);
    let feather = 1.0 - smoothstep(cross_limit * 0.72, cross_limit, abs(across));
    let envelope = start * end * feather
        * smoothstep(0.0, 0.078, progress)
        * (1.0 - smoothstep(0.57, 0.90, progress));
    if envelope < 0.003 {
        return vec2<f32>(0.0);
    }

    let profile = smoothstep(0.05, 0.51, u);
    let billow = sin(TURN * 0.5 * saturate(u)) * select(12.0, 7.0, compact);
    let width = mix(19.0, select(32.0, 22.0, compact), profile);
    let wave = progress * TURN * 1.4;
    let mist = ribbon_density(along, across + billow, width, wave, seed);
    let dust = dust_mote(vec2<f32>(along, across), progress * 96.0, seed, scale);
    return vec2<f32>(
        0.57 * mist.x * envelope,
        (mist.y * 0.26 + dust * mist.x * 0.075) * envelope,
    );
}

// Counterpart of the rounded-rectangle distance function that also provides
// arclength. The origin sits midway along the left edge, then runs clockwise.
fn perimeter_coordinate(local: vec2<f32>, straight: vec2<f32>, radius: f32) -> f32 {
    let unit = abs(local) - straight;
    let turn = radius * QUARTER_TURN;
    let h = straight.x;
    let v = straight.y;
    let length = 4.0 * (h + v + turn);
    if min(unit.x, unit.y) > 0.0 {
        if local.y < 0.0 {
            if local.x < 0.0 {
                return v + radius * atan2(unit.y, unit.x);
            }
            return v + turn + 2.0 * h + radius * atan2(unit.x, unit.y);
        }
        if local.x > 0.0 {
            return 3.0 * v + 2.0 * turn + 2.0 * h + radius * atan2(unit.y, unit.x);
        }
        return 3.0 * v + 3.0 * turn + 4.0 * h + radius * atan2(unit.x, unit.y);
    }
    if unit.x > unit.y {
        if local.x > 0.0 {
            return v + 2.0 * turn + 2.0 * h + clamp(local.y + v, 0.0, 2.0 * v);
        }
        let y = clamp(local.y, -v, v);
        return select(length - y, -y, y <= 0.0);
    }
    if local.y < 0.0 {
        return v + turn + clamp(local.x + h, 0.0, 2.0 * h);
    }
    return 3.0 * v + 3.0 * turn + 2.0 * h + clamp(h - local.x, 0.0, 2.0 * h);
}

fn ring_distance(start_site: f32, end_site: f32) -> f32 {
    let clockwise = fract(end_site - start_site + 1.0);
    return min(clockwise, 1.0 - clockwise);
}

fn assembly_contour(
    pixel: vec2<f32>,
    pane: vec4<f32>,
    entry: vec2<f32>,
    progress: f32,
    assembly: f32,
    compact: bool,
    radius_hint: f32,
) -> vec2<f32> {
    let half_size = max(pane.zw * 0.5, vec2<f32>(1.0));
    let local = pixel - pane.xy - half_size;
    let radius = min(radius_hint, min(half_size.x, half_size.y));
    let straight = half_size - vec2<f32>(radius);
    let corner = abs(local) - straight;
    let sdf = length(max(corner, vec2<f32>(0.0))) + min(max(corner.x, corner.y), 0.0) - radius;
    if sdf < -18.0 || sdf > 63.0 || assembly < 0.003 {
        return vec2<f32>(0.0);
    }

    let turn = radius * QUARTER_TURN;
    let perimeter = 4.0 * (straight.x + straight.y + turn);
    let site = perimeter_coordinate(local, straight, radius) / perimeter;
    let entry_y = clamp(entry.y - pane.y - half_size.y, -straight.y, straight.y);
    let entry_site = fract(-entry_y / perimeter + 1.0);
    var around = 2.0 * ring_distance(site, entry_site);
    var across = saturate((local.x + half_size.x) / (2.0 * half_size.x));
    if compact {
        let top_center = (straight.y + turn + straight.x) / perimeter;
        let bottom_center = (3.0 * (straight.y + turn + straight.x)) / perimeter;
        around = 4.0 * min(ring_distance(site, top_center), ring_distance(site, bottom_center));
        across = saturate(abs(local.x) / half_size.x);
    }

    // The horizontal reveal and perimeter travel meet at the same painted
    // fronts. A little arclength slack lets smoke visibly turn each corner.
    let travel = saturate(0.86 * across + 0.14 * around);
    let built = (1.0 - smoothstep(assembly - 0.046, assembly + 0.028, travel))
        * smoothstep(0.004, 0.115, assembly);
    let front = gaussian(travel - assembly, 0.090) * smoothstep(0.01, 0.13, assembly);
    let tail = 1.0 - smoothstep(0.76, 1.0, progress);
    let exterior = smoothstep(-18.0, -3.0, sdf);
    let unit = local / half_size;
    let drift = sin(progress * TURN);
    let upper_lamp = plume(
        unit, vec2<f32>(-0.91 + 0.17 * drift, -0.77), vec2<f32>(0.65, 0.56),
    );
    let lower_lamp = plume(
        unit, vec2<f32>(0.57, 0.88 - 0.07 * drift), vec2<f32>(0.69, 0.48),
    );
    let lamp = 0.77 * upper_lamp + 0.61 * lower_lamp;
    let sway = sin((site * 2.0 - progress * 0.9) * TURN);
    let rim = gaussian(sdf - 0.5, 1.35) * built;
    let shoulder = gaussian(sdf - (3.0 + 3.8 * sway), 15.0) * built;

    let turn_region = smoothstep(-16.0, 21.0, min(corner.x, corner.y));
    let angle = atan2(max(corner.y, 0.025), max(corner.x, 0.025));
    let curl = sin(angle * 3.9 + site * TURN - progress * 8.4);
    let crest = gaussian(sdf - (5.0 + 4.4 * sway), 18.0) * front;
    let turn_fog = gaussian(sdf - (10.0 + 6.0 * curl), 22.0) * turn_region;
    let body = (0.13 + 0.15 * lamp) * shoulder
        + (0.20 + 0.07 * lamp) * crest + (0.09 * front + 0.028 * built) * turn_fog;
    let light = (0.06 + 0.27 * lamp + 0.075 * front) * rim + 0.046 * crest * lamp;
    return vec2<f32>(body, light) * exterior * tail;
}

fn glass_front(
    pixel: vec2<f32>,
    pane: vec4<f32>,
    progress: f32,
    assembly: f32,
    compact: bool,
    seed: f32,
) -> vec2<f32> {
    let opacity = smoothstep(0.003, 0.09, assembly) *
        (1.0 - smoothstep(0.84, 1.0, assembly)) *
        (1.0 - smoothstep(0.72, 0.94, progress));
    if opacity < 0.003 {
        return vec2<f32>(0.0);
    }
    let half_size = pane.zw * 0.5;
    let local = pixel - pane.xy - half_size;
    let edge = select(
        local.x + half_size.x - pane.z * assembly,
        abs(local.x) - half_size.x * assembly,
        compact,
    );
    if abs(edge) > 63.0 || abs(local.y) > half_size.y + 34.0 {
        return vec2<f32>(0.0);
    }

    // The soft shoulder covers the physical edge of the GPUI clip. The
    // highlight fades along the height so it does not become a bright ruler.
    let reach = 1.0 - smoothstep(half_size.y - 8.0, half_size.y + 34.0, abs(local.y));
    let from_top = local.y + half_size.y;
    let flow = ribbon_density(from_top, edge, 26.0, progress * TURN * 0.91, seed);
    let upper_lamp = gaussian(local.y / half_size.y + 0.22, 0.51);
    let lower_lamp = gaussian(local.y / half_size.y - 0.61, 0.30);
    let core = gaussian(edge, 10.0);
    let density = (0.12 * core + 0.27 * flow.x) * (0.62 + 0.38 * upper_lamp) * reach * opacity;
    let color_core = (0.075 * core * upper_lamp + 0.10 * flow.y * lower_lamp) * reach * opacity;
    return vec2<f32>(density, color_core);
}

fn shade(input: ShaderInput) -> vec4<f32> {
    let endpoints = input.params[0];
    let pane = input.params[1];
    let timeline = input.params[2];
    let options = input.params[3];
    let compact = options.x > 0.5;
    var fog = route_ribbon(
        input.position, endpoints, timeline, compact, options.z, input.time, input.scale_factor,
    );
    if timeline.x > 0.5 {
        let pane_half = pane.zw * 0.5;
        let pane_distance = sd_rounded_rect(
            input.position - pane.xy - pane_half, pane_half, vec4<f32>(options.y),
        );
        // The edge keeps a trace of color when the input fields become legible.
        // Inside the surface, let only the initial puff cross the future form.
        fog *= 1.0 - smoothstep(0.41, 0.72, timeline.y) *
            (1.0 - smoothstep(-37.0, -7.0, pane_distance));
        fog += assembly_contour(
            input.position, pane, endpoints.zw, timeline.y, timeline.z, compact, options.y,
        );
        fog += glass_front(input.position, pane, timeline.y, timeline.z, compact, options.z);
        if compact {
            let top = smoothstep(0.0, 28.0, input.position.y - options.w);
            let bottom = smoothstep(0.0, 32.0, timeline.w - input.position.y);
            fog *= top * bottom;
        }
    }
    let accent = input.colors[0].rgb;
    let chroma = accent - vec3<f32>(dot(accent, vec3<f32>(1.0 / 3.0)));
    let body = saturate(
        vec3<f32>(0.55, 0.53, 0.62) + chroma * select(0.53, 0.38, timeline.x > 0.5),
    );
    let pearl = mix(
        vec3<f32>(0.94, 0.89, 0.81),
        vec3<f32>(0.78, 0.82, 0.91),
        saturate(0.51 + 0.50 * (accent.b - accent.r)),
    );
    let alpha = fog.x + fog.y;
    let color = (body * fog.x + pearl * fog.y) / max(alpha, 0.002);
    return vec4<f32>(color, saturate(alpha));
}
