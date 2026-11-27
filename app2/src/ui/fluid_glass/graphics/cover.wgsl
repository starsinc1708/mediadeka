fn cover_band(value: f32, center: f32, width: f32) -> f32 {
    let distance = (value - center) / width;
    return exp2(-distance * distance);
}

fn cover_grain(pixel: vec2<f32>) -> f32 {
    let cell = vec2<u32>(pixel);
    var bits = (cell.x * 0x1f123bb5u) ^ (cell.y * 0x05491333u);
    bits = (bits ^ (bits >> 16u)) * 0x7feb352du;
    return f32((bits ^ (bits >> 15u)) & 255u) / 255.0 - 0.5;
}

fn cover_field(p: vec2<f32>, family: u32, phase: f32, sample_size: f32) -> vec3<f32> {
    let tau = 6.2831853;
    let warp = sin(p.y * 4.4 + phase) * cos(p.x * 3.1 - phase * 0.53);
    switch family {
        case 0u: {
            let fold = p.y + 0.31 * sin(p.x * 2.8 + phase) + 0.21 * warp;
            let ribbon = fold + 0.15 * sin(fold * 7.0 + p.x * 1.8);
            return vec3<f32>(
                cover_band(ribbon, -0.17, 0.16) + 0.45 * cover_band(ribbon, 0.29, 0.11),
                cover_band(ribbon, 0.05, 0.10) + 0.74 * cover_band(ribbon, 0.44, 0.14),
                cover_band(ribbon, -0.07, max(0.035, sample_size))
            );
        }
        case 1u: {
            let a = sin((p.x * 1.6 + p.y * 1.3 + 0.32 * warp) * tau + phase);
            let b = sin((p.x * 0.9 - p.y * 1.5 - 0.23 * warp) * tau);
            let joint = a * b;
            return vec3<f32>(
                max(joint, 0.0) * 0.8,
                max(-joint, 0.0) * 0.9,
                pow(abs(joint), 4.0) * 0.55
            );
        }
        case 2u: {
            let reed = p.x * (5.3 + 0.35 * sin(p.y * 3.7 + phase)) + 0.16 * warp;
            let lens = clamp(2.0 * fract(reed) - 1.0, -0.95, 0.95);
            let normal = lens / sqrt(1.0 - 0.64 * lens * lens);
            let ray = p + vec2<f32>(normal * 0.13, -normal * 0.15);
            let wave = ray.y * 0.9 + 0.25 * sin(ray.x * 4.2 + phase);
            return vec3<f32>(
                cover_band(wave, -0.17, 0.26) * (0.8 + 0.2 * lens),
                cover_band(wave, 0.23, 0.32),
                max(normal, 0.0) * 0.45 * cover_band(wave, -0.03, 0.19)
            );
        }
        default: {
            let group = floor((p.y + 0.73) * 12.0);
            let slip = smoothstep(0.42, 0.84, sin(group * 2.1 + phase)) * 0.24;
            let smear = (p.x + slip + 0.22 * warp) * 2.4 + p.y * 1.9;
            let line = p.y + 0.29 * sin(smear * 3.6 + phase);
            return vec3<f32>(
                cover_band(line, -0.09, 0.12),
                cover_band(line + p.x * 0.25, 0.26, 0.20),
                cover_band(abs(line + 0.09), 0.08, max(0.022, sample_size)) * 0.5
            );
        }
    }
}

fn shade(input: ShaderInput) -> vec4<f32> {
    let seed = input.params[0];
    let angle = (seed.y - 0.5) * 2.8;
    let c = cos(angle);
    let s = sin(angle);
    let pos = input.uv - vec2<f32>(0.5);
    let p = mat2x2<f32>(c, s, -s, c) * pos * (0.93 + 0.42 * seed.z)
        + vec2<f32>(seed.z - 0.5, seed.w - 0.5) * 0.34;
    let sample_size = 1.0 / min(input.device_size.x, input.device_size.y);
    let light = cover_field(p, u32(seed.x), seed.w * 6.2831853, sample_size);
    let falloff = 1.0 - 0.55 * smoothstep(0.19, 0.76, length(pos));
    let color = input.colors[3].rgb + falloff * (
        input.colors[0].rgb * (0.50 * light.x)
        + input.colors[1].rgb * (0.41 * light.y)
        + input.colors[2].rgb * (0.28 * light.z)
    );
    let grain = cover_grain(input.uv * input.device_size) * (0.020 + 0.037 * max(light.x, light.y));
    return vec4<f32>(max(color + vec3<f32>(grain), vec3<f32>(0.0)), 1.0);
}
