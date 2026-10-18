// Wrapped world-anchored particles adapt ao-vk rain.vert/rain_common.glsl (MIT).
// Native f3 sand, f4 ash, f5 red fallout, f6 green fallout, never a colour classifier.
struct Params {
    vp: mat4x4<f32>, eye: vec4<f32>, drift: vec4<f32>, intensity: vec4<f32>,
    viewport: vec4<f32>, fog: vec4<f32>, light: vec4<f32>, shelter: vec4<f32>,
}
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var shelter: texture_2d<f32>;
fn hash(input: u32) -> u32 {
    var s = input; s ^= s >> 16u; s *= 0x7feb352du;
    s ^= s >> 15u; s *= 0x846ca68bu; return s ^ (s >> 16u);
}
fn unit(s: u32) -> f32 { return f32(hash(s) >> 8u) / 16777216.0; }
struct Out {
    @builtin(position) clip: vec4<f32>, @location(0) uv: vec2<f32>,
    @location(1) colour: vec3<f32>, @location(2) alpha: f32,
}
@vertex fn vs(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> Out {
    var o: Out; o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    let kind = instance / 2048u;
    let seed = hash(instance * 7919u + 3u);
    let strength = p.intensity[kind];
    if strength <= 0.0 { return o; }
    let base = vec3<f32>(unit(seed), unit(seed + 1u), unit(seed + 2u)) * 64.0;
    let ash = kind == 1u;
    let fall = select(0.7, 0.25, ash);
    let moved = base + p.drift.xyz - vec3<f32>(0.0, p.eye.w * fall, 0.0);
    let low = p.eye.xyz - 32.0;
    var world = low + fract((moved - low) / 64.0) * 64.0;
    // Small deterministic eddies distinguish slow ash flakes from wind-borne grains.
    let phase = p.eye.w * select(2.3, 0.9, ash) + unit(seed + 3u) * 6.2831853;
    world += vec3<f32>(sin(phase), cos(phase * 0.7), cos(phase)) * select(0.08, 0.25, ash);
    if p.shelter.z <= 0.0 { return o; } // Missing native shelter coverage fails closed.
    let uv = (world.xz - p.shelter.xy) * p.shelter.z;
    if any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) { return o; }
    let covered = textureLoad(shelter, vec2<i32>(uv * vec2<f32>(textureDimensions(shelter))), 0).rg;
    if world.y < covered.r + 0.1 || world.y <= covered.g + 0.05 { return o; }
    let clip = p.vp * vec4<f32>(world, 1.0);
    if clip.w <= 0.05 { return o; }
    let relative = abs(world - p.eye.xyz) / 32.0;
    let edge = 1.0 - smoothstep(0.7, 1.0, max(relative.x, max(relative.y, relative.z)));
    let distance = length(world - p.eye.xyz);
    // Added particles fade out through native fog, never replacing or reducing its extinction.
    let visible = clamp((p.fog.y - distance) / max(p.fog.y - p.fog.x, 0.001), 0.0, 1.0);
    let corners = array<vec2<f32>, 6>(vec2(-1.0,-1.0), vec2(1.0,-1.0), vec2(1.0,1.0), vec2(-1.0,-1.0), vec2(1.0,1.0), vec2(-1.0,1.0));
    let corner = corners[vertex];
    let radius = select(0.018, 0.045, ash) * (0.7 + unit(seed + 4u));
    let projected = radius * length(p.vp[1].xyz) / clip.w * p.viewport.y * 0.5;
    let pixels = max(projected, 0.65);
    o.clip = clip;
    o.clip.x += corner.x * pixels * 2.0 / p.viewport.x * clip.w;
    o.clip.y += corner.y * pixels * 2.0 / p.viewport.y * clip.w;
    o.uv = corner;
    // Enhanced artistic grain tints, not a claim of native particle assets.
    let tints = array<vec3<f32>, 4>(vec3(0.75,0.49,0.23), vec3(0.64,0.61,0.58), vec3(0.72,0.18,0.12), vec3(0.20,0.63,0.16));
    o.colour = tints[kind] * clamp(p.light.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    o.alpha = strength * edge * visible * smoothstep(0.4, 1.3, distance) * min(projected / 0.65, 1.0) * 0.7;
    return o;
}
@fragment fn fs(o: Out) -> @location(0) vec4<f32> {
    let q = vec2<i32>(o.clip.xy);
    if o.clip.z > textureLoad(depth, q, 0) { discard; }
    let coverage = 1.0 - smoothstep(0.35, 1.0, dot(o.uv, o.uv));
    let alpha = o.alpha * coverage;
    if alpha <= 0.001 { discard; }
    return vec4<f32>(o.colour, alpha);
}
