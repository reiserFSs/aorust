// Enhanced-only non-emissive waterfall mist, lit by the native environment.
struct Params {
    vp: mat4x4<f32>,
    eye_time: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    fog: vec4<f32>,
    illumination: vec4<f32>,
}
@group(0) @binding(0) var<uniform> g: Params;
@group(0) @binding(1) var scene_depth: texture_depth_2d;
struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) opacity: f32,
    @location(2) seed: f32,
    @location(3) fog_depth: f32,
}
@vertex fn vs(@builtin(vertex_index) vertex: u32, @location(0) position_radius: vec4<f32>, @location(1) opacity_seed: vec4<f32>) -> Out {
    let corners = array<vec2<f32>, 6>(vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0));
    let uv = corners[vertex];
    let world = position_radius.xyz + position_radius.w * (g.right.xyz * uv.x + g.up.xyz * uv.y);
    var o: Out;
    o.position = g.vp * vec4<f32>(world, 1.0);
    o.uv = uv;
    o.opacity = opacity_seed.x;
    o.seed = opacity_seed.y;
    o.fog_depth = dot(world - g.eye_time.xyz, g.forward.xyz);
    return o;
}
@fragment fn fs(i: Out) -> @location(0) vec4<f32> {
    // Sampling the final resolved world depth prevents spray through terrain,
    // structures and actors; never infer visibility from the source's bounds.
    let pixel = vec2<i32>(i.position.xy);
    let dimensions = vec2<i32>(textureDimensions(scene_depth));
    if any(pixel < vec2<i32>(0)) || any(pixel >= dimensions) { discard; }
    let depth = textureLoad(scene_depth, pixel, 0);
    if i.position.z > depth { discard; }
    let radial = max(1.0 - dot(i.uv, i.uv), 0.0);
    let drift = i.uv * vec2<f32>(11.0, 8.0) + vec2<f32>(i.seed, -g.eye_time.w * 1.7);
    let wisps = 0.6 + 0.4 * sin(drift.x + sin(drift.y)) * sin(drift.y * 1.3);
    // Randy10041876 uses linear camera-depth fog. Fade the added effect to
    // identity rather than adding fog colour and revealing hidden features.
    let extinction = clamp((i.fog_depth - g.fog.x) / max(g.fog.y - g.fog.x, 0.001), 0.0, 1.0);
    let transmission = select(1.0, 1.0 - extinction, g.fog.z > 0.5);
    let opacity = clamp(i.opacity * radial * radial * wisps * transmission, 0.0, 1.0);
    return vec4<f32>(g.illumination.rgb, opacity);
}
