// Appended to the native shader, preserving its material/UV/alpha equations.
struct ShadowFrame {
    matrices: array<mat4x4<f32>, 3>,
    primary: mat4x4<f32>,
    splits: vec4<f32>,
    eye: vec4<f32>,
    forward: vec4<f32>,
    control: vec4<u32>,
}
@group(3) @binding(0) var<uniform> shadow_frame: ShadowFrame;
@group(3) @binding(1) var shadow_maps: texture_depth_2d_array;
@group(3) @binding(2) var shadow_sampler: sampler_comparison;
@group(3) @binding(3) var shadow_primary_depth: texture_depth_2d;
@group(3) @binding(4) var shadow_primary_ids: texture_2d<u32>;
struct ShadowVertex {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) alpha: f32,
    @location(3) @interpolate(flat) id: u32,
}
fn shadow_vertex(v: VIn, uv: vec2<f32>, id: u32, primary: bool) -> ShadowVertex {
    let model = mat4x4<f32>(vec4<f32>(v.m0.xyz, 0.0), v.m1, v.m2, v.m3);
    let wp = model * vec4<f32>(v.pos, 1.0);
    var o: ShadowVertex;
    o.world = wp.xyz;
    o.uv = uv + mat.scroll.xy * g.fog.z;
    o.alpha = v.color.a * select(mat.color.a, 1.0 + v.m0.w, v.m0.w < 0.0);
    o.id = id;
    if primary { o.clip = g.vp * wp; }
    else { o.clip = shadow_frame.matrices[shadow_frame.control.x] * wp; }
    return o;
}
// Primary variants deliberately do not reference group3; they may write the
// same depth/ID images subsequently sampled by the cascade caster pass.
fn shadow_primary_vertex(v: VIn, uv: vec2<f32>, id: u32) -> ShadowVertex {
    let model = mat4x4<f32>(vec4<f32>(v.m0.xyz, 0.0), v.m1, v.m2, v.m3);
    let wp = model * vec4<f32>(v.pos, 1.0);
    var o: ShadowVertex;
    o.clip = g.vp * wp;
    o.world = wp.xyz;
    o.uv = uv + mat.scroll.xy * g.fog.z;
    o.alpha = v.color.a * select(mat.color.a, 1.0 + v.m0.w, v.m0.w < 0.0);
    o.id = id;
    return o;
}
@vertex fn vs_shadow_static(v: VIn) -> ShadowVertex {
    return shadow_vertex(v, v.uv, 0u, false);
}
@vertex fn vs_shadow_actor(v: VIn, @location(11) uv: vec4<f32>, @builtin(instance_index) id: u32) -> ShadowVertex {
    return shadow_vertex(v, v.uv * uv.xy + uv.zw, id + 1u, false);
}
@vertex fn vs_shadow_primary_static(v: VIn, @builtin(instance_index) id: u32) -> ShadowVertex {
    return shadow_primary_vertex(v, v.uv, 0x80000000u | (id + 1u));
}
@vertex fn vs_shadow_primary_actor(v: VIn, @location(11) uv: vec4<f32>, @builtin(instance_index) id: u32) -> ShadowVertex {
    return shadow_primary_vertex(v, v.uv * uv.xy + uv.zw, id + 1u);
}
fn shadow_alpha(i: ShadowVertex, cutout: bool) {
    let t = sample_clamped(tex, i.uv);
    let texture_alpha = select(t.a, 1.0, mat.emissive.w > 0.5 && mat.emissive.w < 1.5);
    let alpha = texture_alpha * i.alpha;
    if native_alpha_test() && !native_alpha_pass(alpha) { discard; }
    if cutout && !native_alpha_test() && alpha < 0.5 { discard; }
}
fn shadow_admission(i: ShadowVertex) {
    let clip = shadow_frame.primary * vec4<f32>(i.world, 1.0);
    if clip.w <= 0.0 { discard; }
    let ndc = clip.xyz / clip.w;
    let uv = ndc.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    if any(uv < vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) || ndc.z < 0.0 || ndc.z > 1.0 { discard; }
    let pixel = vec2<i32>(uv * vec2<f32>(textureDimensions(shadow_primary_depth)));
    // Same-frame mesh-instance identity AND depth: a coplanar hidden actor
    // cannot borrow static geometry's depth or a different actor's identity.
    if i.id == 0u || textureLoad(shadow_primary_ids, pixel, 0).x != i.id { discard; }
    if abs(textureLoad(shadow_primary_depth, pixel, 0) - ndc.z) > 0.000002 { discard; }
}
@fragment fn fs_shadow_primary_opaque(i: ShadowVertex) -> @location(0) u32 { shadow_alpha(i, false); return i.id; }
@fragment fn fs_shadow_primary_test(i: ShadowVertex) -> @location(0) u32 { shadow_alpha(i, true); return i.id; }
@fragment fn fs_shadow_opaque(i: ShadowVertex) { shadow_alpha(i, false); }
@fragment fn fs_shadow_test(i: ShadowVertex) { shadow_alpha(i, true); }
@fragment fn fs_shadow_dynamic_opaque(i: ShadowVertex) { shadow_alpha(i, false); shadow_admission(i); }
@fragment fn fs_shadow_dynamic_test(i: ShadowVertex) { shadow_alpha(i, true); shadow_admission(i); }
fn cascade_visibility(world: vec3<f32>, layer: u32) -> f32 {
    let clip = shadow_frame.matrices[layer] * vec4<f32>(world, 1.0);
    let p = clip.xyz / clip.w;
    let uv = p.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    if any(uv <= vec2<f32>(0.002)) || any(uv >= vec2<f32>(0.998)) || p.z <= 0.0 || p.z >= 1.0 { return 1.0; }
    var visibility = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            visibility += textureSampleCompareLevel(shadow_maps, shadow_sampler,
                uv + vec2<f32>(f32(x), f32(y)) / vec2<f32>(textureDimensions(shadow_maps)), i32(layer), p.z - 0.00002);
        }
    }
    return visibility / 9.0;
}
fn sun_visibility(world: vec3<f32>) -> f32 {
    let depth = dot(world - shadow_frame.eye.xyz, shadow_frame.forward.xyz);
    if depth < shadow_frame.splits.w || depth >= shadow_frame.splits.z { return 1.0; }
    var layer = 0u;
    if depth > shadow_frame.splits.x { layer = 1u; }
    if depth > shadow_frame.splits.y { layer = 2u; }
    var visible = cascade_visibility(world, layer);
    let start = select(shadow_frame.splits.w, shadow_frame.splits[layer - min(layer, 1u)], layer > 0u);
    let end = shadow_frame.splits[layer];
    let blend = smoothstep(end - (end - start) * 0.1, end, depth);
    if layer < 2u { visible = mix(visible, cascade_visibility(world, layer + 1u), blend); }
    else { visible = mix(visible, 1.0, blend); }
    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));
}
