// Registry maps the two active pattern ids to their scalar fields. Pattern
// identity is independent of the palette; colour mapping happens afterwards.

fn pattern_sample(id: u32, p: vec2<f32>, t: f32, field: VisualField, audio: AudioModulation) -> PatternSample {
    if id == 0u {
        let swell = 0.10 * audio.bass + 0.05 * audio.mid;
        return pattern_thermal_flow(p * vec2<f32>(1.0 - swell, 1.0 + swell), t, field);
    }
    if id == 1u {
        return pattern_fluted_glass(p, t + 0.45 * audio.reflection, field);
    }
    if id == 2u {
        return pattern_interference(p, t + 0.30 * audio.bass + 0.9 * audio.pulse, field);
    }
    if id == 3u {
        return pattern_signal_distortion(p + vec2<f32>(0.032 * audio.glitch, 0.0), t, field);
    }
    if id == 4u {
        let focus = 0.09 * audio.mid + 0.11 * audio.treble;
        return pattern_optical_caustics(p, t + 2.0 * focus, field);
    }
    if id == 5u {
        var sample = pattern_light_leak(p, t, field);
        sample.emission *= 1.0 + 0.22 * audio.light + 0.10 * audio.bass;
        return sample;
    }
    if id == 6u {
        let density = 0.10 * audio.mid + 0.06 * audio.light;
        var sample = pattern_gem_smoke(p, t + 0.8 * audio.mid, field);
        sample.coverage *= 1.0 + density;
        return sample;
    }
    if id == 7u {
        let phase = 0.65 * audio.bass + 0.50 * audio.mid;
        return pattern_swirl(p, t + phase, field);
    }
    var sample = pattern_liquid_metal(p, t, field);
    sample.emission *= 1.0 + 0.25 * audio.reflection + 0.10 * audio.treble;
    return sample;
}

fn pattern_layer(uv: vec2<f32>, time: f32, field: VisualField, audio: AudioModulation) -> PatternSample {
    let centered = uv - vec2<f32>(0.5);
    let p = rotate(centered, field.rotation) * mix(0.55, 2.2, field.scale);
    let t = time * field.speed * 1.4 + field.seed * 0.37;
    let first = pattern_sample(field.pattern_a, p, t, field, audio);
    var sample = first;
    if field.mix > 0.001 {
        let second = pattern_sample(field.pattern_b, p, t, field, audio);
        sample = mix_sample(first, second, field.mix);
    }
    // Complexity adds a very fine pattern-space harmonic without changing the
    // identity of any motif.
    let detail = (field.complexity - 0.5) * 0.10
        * sin(dot(p, vec2<f32>(16.7, 11.3)) + t * 0.6);
    sample.value = clamp(sample.value + detail, 0.0, 1.0);
    sample.coverage = clamp(sample.coverage + detail * 0.5, 0.0, 1.0);
    return sample;
}
