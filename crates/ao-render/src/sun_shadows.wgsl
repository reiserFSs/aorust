// Appended to the native shader, preserving its material/UV/alpha equations.
struct ShadowFrame {
    matrices: array<mat4x4<f32>, 3>,
    splits: vec4<f32>,
    eye: vec4<f32>,
    forward: vec4<f32>,
    control: vec4<u32>,
}
@group(3) @binding(0) var<uniform> shadow_frame: ShadowFrame;
@group(3) @binding(1) var shadow_maps: texture_depth_2d_array;
@group(3) @binding(2) var shadow_sampler: sampler_comparison;
@group(3) @binding(3) var<storage, read> shadow_actor_visibility: array<u32>;
struct ShadowVertex {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) fog: f32,
    @location(1) uv: vec2<f32>,
    @location(2) alpha: f32,
    @location(3) @interpolate(flat) id: u32,
}
fn shadow_vertex(v: VIn, uv: vec2<f32>, id: u32, matrix: mat4x4<f32>, fogged: bool) -> ShadowVertex {
    let model = mat4x4<f32>(vec4<f32>(v.m0.xyz, 0.0), v.m1, v.m2, v.m3);
    let wp = model * vec4<f32>(v.pos, 1.0);
    var o: ShadowVertex;
    o.fog = 0.0;
    if fogged { o.fog = vertex_fog(-(g.view * wp).z); }
    o.uv = uv;
    o.alpha = v.color.a * select(mat.color.a, 1.0 + v.m0.w, v.m0.w < 0.0);
    o.id = id;
    o.clip = matrix * wp;
    return o;
}
@vertex fn vs_shadow_static(v: VIn) -> ShadowVertex {
    return shadow_vertex(v, v.uv, 0u, shadow_frame.matrices[shadow_frame.control.x], false);
}
@vertex fn vs_shadow_actor(v: VIn, @location(11) uv: vec4<f32>, @location(12) id: u32) -> ShadowVertex {
    return shadow_vertex(v, v.uv * uv.xy + uv.zw, id, shadow_frame.matrices[shadow_frame.control.x], false);
}
@vertex fn vs_shadow_primary_static(v: VIn, @builtin(instance_index) id: u32) -> ShadowVertex {
    return shadow_vertex(v, v.uv, 0x80000000u | (id + 1u), g.vp, false);
}
@vertex fn vs_shadow_primary_actor(v: VIn, @location(10) effect: vec4<f32>, @location(11) uv: vec4<f32>, @location(12) id: u32) -> ShadowVertex {
    var o = shadow_vertex(v, v.uv * uv.xy + uv.zw, id, g.vp, true);
    if u32(effect.z) == 2u { o.fog = 0.0; }
    return o;
}
fn shadow_alpha(i: ShadowVertex, cutout: bool) {
    if !cutout && !native_alpha_test() { return; }
    let t = sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z);
    let texture_alpha = select(t.a, 1.0, mat.emissive.w > 0.5 && mat.emissive.w < 1.5);
    let alpha = texture_alpha * i.alpha;
    if native_alpha_test() && !native_alpha_pass(alpha) { discard; }
    if cutout && !native_alpha_test() && alpha < 0.5 { discard; }
}
fn shadow_admission(i: ShadowVertex) {
    if i.id == 0u || i.id >= arrayLength(&shadow_actor_visibility) { discard; }
    if shadow_actor_visibility[i.id] == 0u { discard; }
}
fn primary_actor_id(i: ShadowVertex, cutout: bool) -> u32 {
    shadow_alpha(i, cutout);
    if i.id < 0x80000000u && native_fog(i.fog) >= 1.0 { discard; }
    return i.id;
}
@fragment fn fs_shadow_primary_opaque(i: ShadowVertex) -> @location(0) u32 { return primary_actor_id(i, false); }
@fragment fn fs_shadow_primary_test(i: ShadowVertex) -> @location(0) u32 { return primary_actor_id(i, true); }
@fragment fn fs_shadow_opaque(i: ShadowVertex) { shadow_alpha(i, false); }
@fragment fn fs_shadow_test(i: ShadowVertex) { shadow_alpha(i, true); }
@fragment fn fs_shadow_dynamic_opaque(i: ShadowVertex) { shadow_alpha(i, false); shadow_admission(i); }
@fragment fn fs_shadow_dynamic_test(i: ShadowVertex) { shadow_alpha(i, true); shadow_admission(i); }
fn cascade_visibility(world: vec3<f32>, normal: vec3<f32>, layer: u32) -> f32 {
    let clip = shadow_frame.matrices[layer] * vec4<f32>(world, 1.0);
    let p = clip.xyz / clip.w;
    let uv = p.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let dimensions = vec2<f32>(textureDimensions(shadow_maps));
    let guard = vec2<f32>(2.0) / dimensions;
    if any(uv <= guard) || any(uv >= vec2<f32>(1.0) - guard) || p.z <= 0.0 || p.z >= 1.0 { return 1.0; }
    let matrix = shadow_frame.matrices[layer];
    let row_x = vec3<f32>(matrix[0].x, matrix[1].x, matrix[2].x);
    let row_y = vec3<f32>(matrix[0].y, matrix[1].y, matrix[2].y);
    let row_z = vec3<f32>(matrix[0].z, matrix[1].z, matrix[2].z);
    // Orthogonal projection rows give the inverse-transpose plane normal.
    // Compare every texel against that receiver plane, not a constant depth:
    // this removes PCF self-acne without pulling caster shadows off the feet.
    let plane = vec3<f32>(dot(normal, row_x) / dot(row_x, row_x),
        dot(normal, row_y) / dot(row_y, row_y), dot(normal, row_z) / dot(row_z, row_z));
    var gradient = vec2<f32>(0.0);
    if abs(plane.z) > 0.000001 { gradient = vec2<f32>(-2.0 * plane.x, 2.0 * plane.y) / plane.z; }
    let texel_position = uv * dimensions - vec2<f32>(0.5);
    let center = floor(texel_position) + vec2<f32>(0.5);
    let fraction = fract(texel_position);
    var visibility = 0.0;
    // Bilinearly slide the 3x3 box instead of jumping its centre by a texel.
    // Compare the underlying texel centres separately: hardware bilinear PCF
    // would use one depth for four different points on a sloped receiver plane.
    for (var y = -1; y <= 2; y++) {
        let wy = select(select(1.0, fraction.y, y == 2), 1.0 - fraction.y, y == -1);
        for (var x = -1; x <= 2; x++) {
            let wx = select(select(1.0, fraction.x, x == 2), 1.0 - fraction.x, x == -1);
            let sample_uv = (center + vec2<f32>(f32(x), f32(y))) / dimensions;
            let receiver_depth = p.z + dot(gradient, sample_uv - uv) - 0.00002;
            visibility += wx * wy * textureSampleCompareLevel(shadow_maps, shadow_sampler,
                sample_uv, i32(layer), receiver_depth);
        }
    }
    return visibility / 9.0;
}
fn sun_visibility(world: vec3<f32>, normal: vec3<f32>) -> f32 {
    if shadow_frame.forward.w <= 0.0 { return 1.0; }
    let depth = dot(world - shadow_frame.eye.xyz, shadow_frame.forward.xyz);
    if depth < shadow_frame.splits.w || depth >= min(shadow_frame.splits.z, shadow_frame.eye.w) { return 1.0; }
    var layer = 0u;
    if depth > shadow_frame.splits.x { layer = 1u; }
    if depth > shadow_frame.splits.y { layer = 2u; }
    var visible = cascade_visibility(world, normal, layer);
    let start = select(shadow_frame.splits.w, shadow_frame.splits[layer - min(layer, 1u)], layer > 0u);
    let end = shadow_frame.splits[layer];
    let blend = smoothstep(end - (end - start) * 0.1, end, depth);
    if layer < 2u { visible = mix(visible, cascade_visibility(world, normal, layer + 1u), blend); }
    else { visible = mix(visible, 1.0, blend); }
    // Camera-local fog may end before the stable cascade envelope. Retain a
    // smooth receiver fade there, including native materials that disable fog.
    if shadow_frame.eye.w < shadow_frame.splits.z {
        let fade_start = max(shadow_frame.splits.w, shadow_frame.eye.w * 0.9);
        visible = mix(visible, 1.0, smoothstep(fade_start, shadow_frame.eye.w, depth));
    }
    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));
}
