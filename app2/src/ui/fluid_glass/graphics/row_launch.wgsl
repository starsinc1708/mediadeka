fn shade(input: ShaderInput) -> vec4<f32> {
    let flight = input.params[0];
    let axis = flight.xy;
    let phase = flight.z;
    let impact = flight.w;
    let local = input.position - input.size * 0.5;
    let along = dot(local, axis);
    let across = dot(local, vec2<f32>(-axis.y, axis.x));
    let envelope = smoothstep(0.0, 0.09, phase) * (1.0 - smoothstep(0.76, 1.0, phase));
    let tip = along / (21.0 + 20.0 * impact);
    let face = across / (8.0 + 8.0 * impact);
    let ridge = exp2(-1.7 * tip * tip - 2.0 * face * face);
    let length = (along + 19.0 * (1.0 - impact)) / (47.0 + 38.0 * impact);
    let falloff = (across + 2.2 * sin(tip * 1.4)) / (19.0 + 7.0 * impact);
    let scatter = exp2(-1.8 * length * length - 1.55 * falloff * falloff);
    let edge = abs(input.uv - vec2<f32>(0.5));
    let crop = 1.0 - smoothstep(0.37, 0.50, max(edge.x, edge.y));
    let tint = mix(input.colors[0].rgb, input.colors[1].rgb, 0.46 + 0.40 * ridge);
    return vec4<f32>(tint, envelope * crop * (0.089 * ridge + 0.054 * scatter));
}
