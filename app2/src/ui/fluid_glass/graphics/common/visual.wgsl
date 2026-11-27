// Independent visual layers shared by every procedural pattern.
//
// Palette colours are data, never pattern geometry. Patterns return a scalar
// field (value, coverage, emission, depth); the colour mapper reads the four
// palette light stops. The same pattern therefore runs unchanged under Solar,
// Abyss, Ultraviolet, Monochrome and every other palette.

const VISUAL_FLUTED: u32 = 1u;
const VISUAL_REFRACTION: u32 = 2u;
const VISUAL_DISPERSION: u32 = 4u;
const VISUAL_GRAIN: u32 = 8u;
const VISUAL_BLOOM: u32 = 16u;
const VISUAL_GLITCH: u32 = 32u;
const VISUAL_VIGNETTE: u32 = 64u;

struct VisualField {
    pattern_a: u32,
    pattern_b: u32,
    mix: f32,
    seed: f32,
    scale: f32,
    speed: f32,
    rotation: f32,
    intensity: f32,
    distortion: f32,
    softness: f32,
    complexity: f32,
    coverage: f32,
    flags: u32,
    octaves: u32,
}

struct PatternSample {
    value: f32,
    coverage: f32,
    emission: f32,
    depth: f32,
}

struct PatternPalette {
    colors: array<vec3<f32>, 4>,
    highlight: vec3<f32>,
    canvas: vec3<f32>,
}

/// A fixed, hand-unrolled two-octave fbm. Naga lowers a dynamic loop to a
/// `while(true)` with a loop-bound counter; FXC then spends minutes optimizing
/// the many inlined copies. Two constant branches keep the one pipeline inside
/// the FXC budget. Value noise keeps the hash taps to float maths, which FXC
/// folds much faster than the integer gradient hash.
fn visual_noise(p: vec2<f32>) -> f32 {
    return value_noise(p) * 2.0 - 1.0;
}

fn visual_fbm(p: vec2<f32>, octaves: i32) -> f32 {
    var total = visual_noise(p) * 0.5;
    var weight = 0.5;
    if octaves >= 2 {
        let point = rotate(p, 0.6435) * 2.03 + vec2<f32>(17.13, -9.71);
        total += visual_noise(point) * 0.25;
        weight += 0.25;
    }
    return total / weight;
}

fn visual_word(lane: f32) -> u32 {
    return u32(round(lane * 16777216.0));
}

fn visual_byte(lane: f32, shift: u32) -> u32 {
    return (visual_word(lane) >> shift) & 255u;
}

fn visual_six(lane: f32, shift: u32) -> f32 {
    return f32((visual_word(lane) >> shift) & 63u) / 63.0;
}

fn visual_field(input: ShaderInput, coverage: f32) -> VisualField {
    let lane1 = input.colors[1].a;
    let lane2 = input.colors[2].a;
    let lane3 = input.colors[3].a;
    return VisualField(
        visual_byte(lane3, 8u) & 15u,
        visual_byte(lane3, 12u) & 15u,
        visual_six(lane2, 18u),
        f32(visual_byte(lane3, 0u)),
        visual_six(lane1, 0u),
        visual_six(lane1, 6u),
        visual_six(lane1, 12u) * 6.283185307,
        visual_six(lane1, 18u),
        visual_six(lane2, 0u),
        visual_six(lane2, 6u),
        visual_six(lane2, 12u),
        coverage,
        visual_byte(lane3, 16u),
        max(1u, min(2u, (u32(input.params[1].y) >> 2u) & 7u)),
    );
}

struct AudioModulation {
    bass: f32,
    mid: f32,
    treble: f32,
    light: f32,
    reflection: f32,
    dispersion: f32,
    pulse: f32,
    glitch: f32,
}

fn visual_audio(input: ShaderInput) -> AudioModulation {
    let frequency = visual_word(input.colors[0].a);
    let optics = u32(input.params[1].w);
    return AudioModulation(
        f32(frequency & 31u) / 31.0,
        f32((frequency >> 5u) & 31u) / 31.0,
        f32((frequency >> 10u) & 31u) / 31.0,
        f32((frequency >> 15u) & 31u) / 31.0,
        f32((frequency >> 20u) & 15u) / 15.0,
        f32((optics >> 10u) & 31u) / 31.0,
        f32((optics >> 15u) & 31u) / 31.0,
        f32((optics >> 20u) & 15u) / 15.0,
    );
}

fn visual_flag(field: VisualField, flag: u32) -> bool {
    return (field.flags & flag) != 0u;
}

fn light_field(p: vec2<f32>, center: vec2<f32>, radii: vec2<f32>) -> f32 {
    let d = (p - center) / radii;
    return exp2(-dot(d, d));
}

fn ribbon(distance: f32, center: f32, width: f32) -> f32 {
    let normalized = (distance - center) / width;
    return exp2(-normalized * normalized);
}

fn palette_stop(colors: array<vec3<f32>, 4>, index: i32) -> vec3<f32> {
    switch clamp(index, 0, 3) {
        case 0: { return colors[0]; }
        case 1: { return colors[1]; }
        case 2: { return colors[2]; }
        default: { return colors[3]; }
    }
}

fn pattern_color(sample: PatternSample, palette: PatternPalette) -> vec3<f32> {
    let t = clamp(sample.value, 0.0, 1.0) * 3.0;
    let index = i32(floor(t));
    let fraction = smoothstep(0.0, 1.0, fract(t));
    let gradient = mix(
        palette_stop(palette.colors, index),
        palette_stop(palette.colors, index + 1),
        fraction,
    );
    // The motif tints the dark atmosphere and only its crests carry the
    // highlight. Depth scales the broad body so folds keep their volume
    // instead of flattening into a coloured fill.
    let depth_gain = mix(0.82, 1.0, clamp(0.5 + 0.5 * sample.depth, 0.0, 1.0));
    return gradient * sample.coverage * 0.60 * depth_gain
        + palette.highlight * sample.emission * 0.66;
}

fn mix_sample(a: PatternSample, b: PatternSample, progress: f32) -> PatternSample {
    let t = smoothstep(0.0, 1.0, progress);
    return PatternSample(
        mix(a.value, b.value, t),
        mix(a.coverage, b.coverage, t),
        mix(a.emission, b.emission, t),
        mix(a.depth, b.depth, t),
    );
}
