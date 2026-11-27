const PULSE_SHIFT: u32 = 7u;
const DOCK_SHIFT: u32 = 15u;
const RASTER_SHIFT: u32 = 21u;
const PULSE_MASK: u32 = 127u;
const DOCK_MASK: u32 = 63u;
const RASTER_MASK: u32 = 7u;
const PORTRAIT_LIGHT: u32 = 2u;

fn inspect_surface(
    mode: u32,
    distance: f32,
    normal: vec2<f32>,
    mask: f32,
    uv: vec2<f32>,
    offset: vec2<f32>,
    rim: vec4<f32>,
) -> vec3<f32> {
    switch mode {
        case 1u: {
            let magnitude = smoothstep(0.0, 64.0, abs(distance));
            let side = select(vec3<f32>(0.20, 0.44, 0.90), vec3<f32>(0.92, 0.36, 0.17), distance > 0.0);
            return mix(vec3<f32>(1.0), side, magnitude);
        }
        case 2u: {
            let near = 1.0 - smoothstep(24.0, 60.0, abs(distance));
            return mix(vec3<f32>(0.10), vec3<f32>(normal * 0.5 + 0.5, 0.55), near);
        }
        case 3u: { return vec3<f32>(mask); }
        case 4u: {
            let grid = abs(fract(uv * 16.0 + 0.5) - 0.5);
            let line = 1.0 - smoothstep(0.0, 0.018, min(grid.x, grid.y));
            return mix(vec3<f32>(uv, 0.20), vec3<f32>(1.0), line * 0.45);
        }
        case 5u: {
            // Neutral gray means zero; red and green encode signed DIP offsets.
            return vec3<f32>(clamp(offset / 64.0 + 0.5, vec2<f32>(0.0), vec2<f32>(1.0)), 0.5);
        }
        default: { return rim.rgb * rim.a; }
    }
}

fn preview_shade(
    input: ShaderInput,
    context: VisualField,
    light: Atmosphere,
    motif: vec3<f32>,
) -> vec4<f32> {
    var color = light.canvas + motif;
    let center = input.uv - vec2<f32>(0.5);
    let vignette = select(
        1.0,
        1.0 - 0.22 * smoothstep(0.28, 0.82, length(center * vec2<f32>(1.15, 1.35))),
        visual_flag(context, VISUAL_VIGNETTE),
    );
    color *= vignette;
    if visual_flag(context, VISUAL_GRAIN) {
        color += vec3<f32>(film_grain(input.frag_coord, input.uv, input.time, 1.0, 1.0));
    }
    return vec4<f32>(color, 1.0);
}

fn shade(input: ShaderInput) -> vec4<f32> {
    let animation = u32(input.params[1].y);
    let layer_kind = (animation >> 5u) & 3u;
    let preview = layer_kind == 3u;
    var paint_motif = false;
    var motif_uv: vec2<f32>;
    var dimensions: vec2<f32>;
    var pixel: vec2<f32>;
    var uv: vec2<f32>;
    var dims: vec4<f32>;
    var junction: vec2<f32>;
    var influence: f32;
    var context: VisualField;
    var light: Atmosphere;
    var rim: vec4<f32>;
    var glass_mask: f32;
    var dock_grade: f32;
    var rail_grade: f32;
    var reading_grade: f32;
    var glitch: f32;
    var flutes: vec2<f32>;
    var digital_split: bool;
    var color: vec3<f32>;
    var caustic_ray: vec2<f32>;
    var grain: f32;

    if preview {
        context = visual_field(input, 1.0);
        light = atmosphere_for(input, 1.0, 0.0, context);
        motif_uv = input.uv;
    } else {
        let pulse = f32((animation >> PULSE_SHIFT) & PULSE_MASK) / 127.0;
        let dock_strength = f32((animation >> DOCK_SHIFT) & DOCK_MASK) / 63.0;
        let raster_step = f32(((animation >> RASTER_SHIFT) & RASTER_MASK) + 1u) / 8.0;
        let raster_scale = select(1.0, raster_step, layer_kind == 1u);
        let shape = input.params[0];
        let debug_word = u32(shape.w / DEBUG_MODE_STRIDE);
        let debug_mode = debug_word % STUDIO_SCENE;
        let studio = debug_word >= STUDIO_SCENE;
        dimensions = select(
            max(gpui_globals.viewport_size / max(input.scale_factor, 1e-4), vec2<f32>(1.0)),
            STUDIO_SIZE,
            studio,
        );
        let raster_dpi = input.scale_factor * raster_scale;
        pixel = select(input.position / raster_scale, input.uv * dimensions, studio);
        uv = pixel / dimensions;
        let surface_word = shape.w - DEBUG_MODE_STRIDE * f32(debug_word);
        let hero_light = floor(surface_word / HERO_LIGHT_STRIDE);
        dims = vec4<f32>(shape.xyz, surface_word - HERO_LIGHT_STRIDE * hero_light);
        let packed_surface = floor(input.params[1].x / SURFACE_STRIDE);
        let inner = input.params[1].x - packed_surface * SURFACE_STRIDE;
        let opacity = f32(u32(packed_surface) & 63u) / 63.0;
        let blur = f32((u32(packed_surface) >> 6u) & 63u) / 63.0;
        let animated = f32(animation & 1u);
        let octaves = i32(max(1u, min(4u, (animation >> 2u) & 7u)));
        junction = vec2<f32>(dims.x + dims.y, dimensions.y - dims.x - dims.z);
        let shell = shell_distance(pixel, dimensions, dims, inner);
        let dock_width = input.params[1].z;
        let dock = dock_distance(pixel, dimensions, dims, dock_width, inner);
        let right_join = vec2<f32>(dimensions.x - dims.x - dock_width, junction.y);
        let distance = min(shell, dock);
        let slope = vec2<f32>(dpdx(distance), dpdy(distance));
        let normal = slope / max(length(slope), 0.001);
        // Optical detail stays on the device grid above the resampled material.
        let on_joined_edge = abs(distance) < OPTICAL_EDGE_LIMIT;
        if layer_kind == 2u && !on_joined_edge { return vec4<f32>(0.0); }
        let content_left = junction.x / dimensions.x;
        let content_top = dims.x + TITLEBAR_HEIGHT;
        let content_right = dimensions.x - dims.x - dock_width;
        let exposure_word = u32(input.params[1].w);
        let artwork_placement = exposure_word & 3u;
        let exposure = f32((exposure_word >> 2u) & 255u) / 255.0;
        let hero = artwork_placement != 0u;
        var warp = vec2<f32>(0.0);
        if hero || layer_kind != 2u {
            warp = domain_warp(uv, input.time, visual_audio(input).mid, octaves);
        }
        if hero {
            let origin = vec2<f32>(junction.x, content_top);
            let size = max(vec2<f32>(content_right, junction.y) - origin, vec2<f32>(1.0));
            let feather_left = smoothstep(junction.x - 8.0, junction.x + 40.0, pixel.x);
            let feather_right = 1.0 - smoothstep(content_right - 30.0, content_right + 12.0, pixel.x);
            let feather_top = smoothstep(content_top - 22.0, content_top + 28.0, pixel.y);
            let feather_bottom = 1.0 - smoothstep(junction.y - 74.0, junction.y - 16.0, pixel.y);
            influence = artwork_influence(pixel - origin, size, warp, artwork_placement == PORTRAIT_LIGHT)
                * feather_left * feather_right * feather_top * feather_bottom * (hero_light / 255.0);
        }
        let content_gate = smoothstep(content_top - 8.0, content_top + 44.0, pixel.y);
        let page_right = 1.0 - smoothstep(content_right - 48.0, content_right + 30.0, pixel.x);
        let page_left = smoothstep(junction.x - 22.0, junction.x + 48.0, pixel.x);
        let page_bottom = 1.0 - smoothstep(junction.y - 62.0, junction.y + 35.0, pixel.y);
        var upper_weight = 0.0;
        if (animation & 16384u) != 0u {
            // Fade the palette over the opening on its own scale. The reading
            // and optical masks still follow their existing shell geometry.
            let side_feather = clamp((content_right - junction.x) * 0.10, 48.0, 144.0);
            let top_feather = clamp((junction.y - content_top) * 0.16, 48.0, 136.0);
            let bleed = 28.0;
            let palette_top = smoothstep(content_top - bleed, content_top + top_feather, pixel.y);
            let palette_left = smoothstep(junction.x - bleed, junction.x + side_feather, pixel.x);
            let palette_right = 1.0 - smoothstep(content_right - side_feather, content_right + bleed, pixel.x);
            upper_weight = palette_top * palette_left * palette_right * page_bottom;
        }
        // The route exposure belongs to the content. Ramping it in below the toolbar
        // keeps the header from carrying a brighter copy of the same field.
        let header_weight = 1.0 - smoothstep(content_top - 4.0, content_top + 40.0, pixel.y);
        // Keep the chrome stable when the workspace changes its lighting.
        let workspace = smoothstep(0.52, 0.83, uv.x)
            * (1.0 - smoothstep(junction.y - 135.0, junction.y - 37.0, pixel.y))
            * content_gate;
        let context_coverage = smoothstep(max(0.36, content_left + 0.10), 0.75, uv.x)
            * smoothstep(content_top - 24.0, content_top + 66.0, pixel.y)
            * (1.0 - smoothstep(junction.y - 80.0, junction.y - 14.0, pixel.y));
        let coverage = select(context_coverage, min(1.0, 0.22 * context_coverage + 0.80 * influence), hero);
        let upper_light = 1.0 - smoothstep(content_top + 64.0, min(content_top + 390.0, max(content_top + 65.0, junction.y)), pixel.y);
        reading_grade = content_gate * page_left * page_right * page_bottom
            * (1.0 - 0.64 * max(influence, upper_light * exposure));
        rail_grade = (1.0 - page_left) * page_bottom * select(0.0, 1.0, dims.y > 0.0);
        context = visual_field(input, coverage);
        light = atmosphere_for(input, animated, upper_weight, context);
        light.intensity *= 0.83 + (exposure - LIBRARY_EXPOSURE) * workspace * 0.78;
        light.intensity *= select(1.0, 0.54 + 0.22 * influence, hero);
        if layer_kind != 1u {
            if on_joined_edge {
                rim = edge_lighting(distance, normal, pixel, uv, junction, right_join, light, input.scale_factor, pulse);
                // The uninterrupted upright has only a faint rim away from the
                // two rounded ends, particularly beside the queue's reading area.
                let dock_span = smoothstep(TITLEBAR_HEIGHT + 24.0, TITLEBAR_HEIGHT + 112.0, pixel.y - dims.x)
                    * (1.0 - smoothstep(junction.y - inner - 92.0, junction.y - inner, pixel.y));
                rim.a *= 1.0 - select(0.0, 0.53 * abs(normal.x) * dock_span, dock < shell);
            }
            if layer_kind == 2u { return rim; }
        }
        let upright = select(0.0, abs(normal.x), dock < shell);
        let falloff = mix(0.65, 1.50, upright) / raster_dpi;
        glass_mask = 1.0 - smoothstep(-falloff, falloff, distance);
        let surface_distance = distance;
        let surface_normal = normal;
        dock_grade = dock_strength * (1.0 - smoothstep(-22.0, 26.0, pixel.y - junction.y))
            * (1.0 - smoothstep(-16.0, 4.0, dock));

        if (animation & 2u) != 0u && visual_flag(context, VISUAL_GLITCH) {
            glitch = track_glitch(pixel, uv, pulse, input.scale_factor);
        }
        if animated > 0.0 && light.audio.glitch > 0.0 && visual_flag(context, VISUAL_GLITCH) {
            let onset = audio_glitch(pixel, uv, light.audio.glitch, input.scale_factor, input.time);
            glitch = clamp(glitch + onset, -TRACK_GLITCH_SHIFT_PX, TRACK_GLITCH_SHIFT_PX);
        }
        let scanline_shift = vec2<f32>(glitch / input.scale_factor, 0.0);
        // Ribs live on the glass body only. They must not ripple the atmosphere
        // outside the silhouette or run under the window controls in the header.
        // The attached queue is a reading surface, so the shell silhouette, not the
        // dock union, keeps its upright calm.
        // The flag skips the ribs entirely when the optical fluting is off.
        if visual_flag(context, VISUAL_FLUTED) && glass_mask > 0.0 {
            flutes = fluted_glass(pixel + scanline_shift, uv, junction, context)
                * smoothstep(0.0, 12.0, -shell) * content_gate * mix(0.58, 1.0, animated)
                * (1.0 + 0.30 * light.audio.reflection);
        }
        let displaced = uv + scanline_shift / dimensions;
        // All optical paths read the same source. The low-frequency warp is
        // shared between nearby samples and the field is evaluated at each ray.
        digital_split = abs(glitch) > 0.05;
        let digital_offset = vec2<f32>(TRACK_GLITCH_SPLIT_RATIO * glitch / input.scale_factor, 0.0) / dimensions;
        if glass_mask < 1.0 {
            color = atmosphere_sample(displaced, warp, light);
            if digital_split {
                color = chromatic_edge(color, displaced, digital_offset, warp, light);
            }
        }
        caustic_ray = displaced;
        var refraction_offset = vec2<f32>(0.0);
        if glass_mask > 0.0 {
            let width = bevel_width(pixel, surface_normal, junction, right_join);
            let depth = max(-surface_distance, 0.0);
            var ray = GlassRay(displaced + vec2<f32>(flutes.x, 0.0) / dimensions, vec2<f32>(0.0), vec2<f32>(0.0), 0.0);
            if visual_flag(context, VISUAL_REFRACTION) {
                ray = glass_ray(
                    displaced,
                    surface_normal,
                    depth,
                    width,
                    flutes,
                    dimensions,
                    input.scale_factor,
                );
            }
            var transmitted = atmosphere_sample(ray.center, warp, light);
            let disperses = ray.bevel > 0.06 && visual_flag(context, VISUAL_DISPERSION);
            if disperses || digital_split {
                let dispersion = ray.dispersion * (1.0 + 0.45 * light.audio.dispersion);
                let offset = select(vec2<f32>(0.0), dispersion, disperses) + digital_offset;
                transmitted = chromatic_edge(transmitted, ray.center, offset, warp, light);
            }
            let lamp = guided_light(pixel, junction);
            if blur > 0.0 && (ray.bevel > 0.07 || lamp > 0.26) {
                let spread = ray.scatter * (1.0 + 16.0 * blur * blur);
                let diffuse = atmosphere_sample(ray.center + spread, warp, light)
                    + atmosphere_sample(ray.center - spread, warp, light);
                transmitted = mix(transmitted, diffuse * 0.5, (0.18 + 0.27 * ray.bevel) * min(1.0, 2.0 * blur));
            }
            let glass = glass_tint(transmitted, depth, width, surface_normal, lamp, light, dock_grade, header_weight, opacity);
            color = mix(color, glass, glass_mask);
            caustic_ray = mix(displaced, ray.center, glass_mask);
            refraction_offset = (caustic_ray - displaced) * dimensions;
        }
        if debug_mode != 0u {
            return vec4<f32>(
                inspect_surface(debug_mode, distance, normal, glass_mask, uv, refraction_offset, rim),
                1.0,
            );
        }
        // Decorative material influence, separate from the exact shell mask. The
        // tint and local light bleed into the page over the same distance the eye
        // reads as the glass shoulder; clipping and hit testing never use it.
        let feather = clamp(inner * 1.6, 40.0, 100.0);
        let feather_band = 1.0 - smoothstep(0.0, feather, max(distance, 0.0));
        if feather_band > 0.0 && glass_mask < 0.999 {
            let edge_glass = glass_tint(
                atmosphere_sample(displaced, warp, light),
                0.0,
                bevel_width(pixel, surface_normal, junction, right_join),
                surface_normal,
                guided_light(pixel, junction),
                light,
                dock_grade,
                header_weight,
                opacity,
            );
            color = mix(color, edge_glass, feather_band * (1.0 - glass_mask) * 0.72);
        }
        if visual_flag(context, VISUAL_GRAIN) {
            grain = film_grain(input.frag_coord, uv, light.time, animated, raster_scale)
                * (1.0 + 0.08 * light.audio.light);
        }
        paint_motif = light.context.coverage > 0.0 && light.context.intensity > 0.0;
        if paint_motif {
            motif_uv = caustic_ray + 0.32 * influence * warp;
        }
    }

    // A shared call keeps FXC from inlining the entire pattern registry twice.
    var motif = vec3<f32>(0.0);
    if preview || paint_motif {
        motif = pattern_light(motif_uv, light);
    }
    if preview {
        return preview_shade(input, context, light, motif);
    }
    if paint_motif {
        // The attached queue is a reading surface: its interior keeps only a
        // trace of the motif so secondary labels keep their measured contrast.
        let reading = (1.0 - 0.82 * dock_grade) * (1.0 - 0.37 * reading_grade);
        let tinted = color * (1.0 - 0.20 * light.context.coverage)
            + motif * mix(1.02, 0.76, glass_mask) * reading;
        color = mix(color, tinted, light.context.coverage * 0.92);
    }
    let spill = (0.060 * light.hot + 0.027 * light.rose) * context.intensity
        * influence * (1.0 - dock_grade) * (0.92 + 0.14 * light.energy);
    color += spill;
    let caustic = junction_caustic(caustic_ray * dimensions, junction, light);
    color += caustic;
    let focused = color + (color - light.canvas) * (0.24 * flutes.y);
    let neutral_chrome = max(dock_grade * 0.72, rail_grade * 0.62);
    let subdued = mix(mix(focused, NEUTRAL_CANVAS, 0.24 * reading_grade), GLASS_TINT, neutral_chrome);
    let vignette = select(
        1.0,
        1.0 - 0.30 * smoothstep(0.30, 0.92, length((uv - vec2<f32>(0.5)) * vec2<f32>(1.05, 1.15))),
        visual_flag(context, VISUAL_VIGNETTE),
    );
    let work_area = vec4<f32>(dims.x, dims.x, dimensions.x - dims.x, junction.y);
    let displayed = limit_reading_light(subdued * vignette + vec3<f32>(grain), pixel, work_area, dock_grade);
    let composited = mix(displayed, rim.rgb, rim.a);
    if digital_split { return vec4<f32>(composited * glitch_transmission(glitch), 1.0); }
    return vec4<f32>(composited, 1.0);
}
