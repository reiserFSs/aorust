// Appended to the native shader, preserving its material/UV/alpha equations.
struct ShadowFrame {
    matrices: array<mat4x4<f32>, 3>,
    splits: vec4<f32>,
    eye: vec4<f32>,
    forward: vec4<f32>,
    control: vec4<u32>,
    moon_direction: vec4<f32>,
    moon_budget: vec4<f32>,
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
    var o = shadow_vertex(v, v.uv, 0u, shadow_frame.matrices[shadow_frame.control.x], false);
    if shadow_frame.control.z != 0u { o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0); }
    return o;
}
@vertex fn vs_shadow_actor(v: VIn, @location(11) uv: vec4<f32>, @location(12) id: u32, @location(13) character: u32) -> ShadowVertex {
    var o = shadow_vertex(v, v.uv * uv.xy + uv.zw, id, shadow_frame.matrices[shadow_frame.control.x], false);
    if shadow_frame.control.z != 0u || (shadow_frame.control.w != 0u && character != 0u) { o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0); }
    return o;
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
fn shadow_atlas_clip(i: ShadowVertex) {
    if (shadow_frame.control.y & 0x80000000u) != 0u && shadow_frame.forward.w > 0.0 {
        if any(i.clip.xy >= vec2<f32>(shadow_frame.forward.w)) { discard; }
    }
}
@fragment fn fs_shadow_opaque(i: ShadowVertex) { shadow_atlas_clip(i); shadow_alpha(i, false); }
@fragment fn fs_shadow_test(i: ShadowVertex) { shadow_atlas_clip(i); shadow_alpha(i, true); }
@fragment fn fs_shadow_dynamic_opaque(i: ShadowVertex) { shadow_atlas_clip(i); shadow_alpha(i, false); shadow_admission(i); }
@fragment fn fs_shadow_dynamic_test(i: ShadowVertex) { shadow_atlas_clip(i); shadow_alpha(i, true); shadow_admission(i); }
// Vogel (golden-angle) disk point, radius 1, rotated.
fn shadow_vogel(index: u32, count: u32, rotation: f32) -> vec2<f32> {
    let r = sqrt((f32(index) + 0.5) / f32(count));
    let a = f32(index) * 2.39996323 + rotation;
    return r * vec2<f32>(cos(a), sin(a));
}
// Depth texels hold the caster plane at their centres: compare the receiver
// plane there too, or off-centre taps self-shadow.
fn shadow_center(uv: vec2<f32>, dimensions: vec2<f32>) -> vec2<f32> {
    return (floor(uv * dimensions) + vec2<f32>(0.5)) / dimensions;
}
// Soft sun shadow, after ao-vk rvk/shaders/ffp_main.glsl CascadeVisibility
// (MIT, https://github.com/wannabuh/ao-vk): a blocker search finds the mean
// caster depth, and the penumbra grows with caster-to-receiver distance
// (sharp at contact, wide far out). Taps are rotated per pixel by a fixed
// screen-space interleaved-gradient pattern: identical every frame.
fn cascade_visibility(world: vec3<f32>, normal: vec3<f32>, layer: u32, pixel: vec2<f32>) -> f32 {
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
    let softness = bitcast<f32>(shadow_frame.control.y);
    if softness <= 0.0 {
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
    // Wide taps must not slope-extrapolate past ~76 degrees of receiver tilt.
    let slope = 8.0 * length(row_z) / length(row_x);
    gradient = clamp(gradient, vec2<f32>(-slope), vec2<f32>(slope));
    let rotation = 6.2831853 * fract(52.9829189 * fract(dot(pixel, vec2<f32>(0.06711056, 0.00583715))));
    // Taps stay inside the rendered square, never reading clamped border texels.
    let lo = guard;
    let hi = vec2<f32>(1.0) - guard;
    let texel_uv = 1.0 / dimensions;
    if layer >= 2u {
        // Distant texels are wider than any penumbra: a small fixed filter.
        var visibility = 0.0;
        for (var i = 0u; i < 4u; i++) {
            let sample_uv = shadow_center(clamp(uv + shadow_vogel(i, 4u, rotation) * 1.5 * texel_uv, lo, hi), dimensions);
            let receiver_depth = p.z + dot(gradient, sample_uv - uv) - 0.00002;
            visibility += textureSampleCompareLevel(shadow_maps, shadow_sampler, sample_uv, i32(layer), receiver_depth);
        }
        return visibility * 0.25;
    }
    let texel_world = 2.0 / (length(row_x) * dimensions.x);
    let range_world = 1.0 / length(row_z);
    let k = 0.02 * softness;
    let search = clamp(k * 30.0 / texel_world, 1.5, 24.0);
    var blocker_sum = 0.0;
    var blockers = 0.0;
    // Centre plus four disk taps: a thin pole must not fall between the taps.
    for (var i = 0u; i < 5u; i++) {
        var offset = vec2<f32>(0.0);
        if i < 4u { offset = shadow_vogel(i, 4u, rotation) * search * texel_uv; }
        let sample_uv = shadow_center(clamp(uv + offset, lo, hi), dimensions);
        let depth = textureLoad(shadow_maps, vec2<i32>(floor(sample_uv * dimensions)), i32(layer), 0);
        if depth < p.z + dot(gradient, sample_uv - uv) - 0.00002 { blocker_sum += depth; blockers += 1.0; }
    }
    if blockers == 0.0 { return 1.0; }
    if blockers == 5.0 { return 0.0; }
    let distance = (p.z - blocker_sum / blockers) * range_world;
    let radius = clamp(distance * k / texel_world, 1.5, 24.0);
    var visibility = 0.0;
    for (var i = 0u; i < 12u; i++) {
        let sample_uv = shadow_center(clamp(uv + shadow_vogel(i, 12u, rotation) * radius * texel_uv, lo, hi), dimensions);
        let receiver_depth = p.z + dot(gradient, sample_uv - uv) - 0.00002;
        visibility += textureSampleCompareLevel(shadow_maps, shadow_sampler, sample_uv, i32(layer), receiver_depth);
    }
    return visibility / 12.0;
}
fn sun_visibility(world: vec3<f32>, normal: vec3<f32>, pixel: vec2<f32>) -> f32 {
    if shadow_frame.forward.w <= 0.0 { return 1.0; }
    let depth = dot(world - shadow_frame.eye.xyz, shadow_frame.forward.xyz);
    if depth < shadow_frame.splits.w || depth >= min(shadow_frame.splits.z, shadow_frame.eye.w) { return 1.0; }
    var layer = 0u;
    if depth > shadow_frame.splits.x { layer = 1u; }
    if depth > shadow_frame.splits.y { layer = 2u; }
    var visible = cascade_visibility(world, normal, layer, pixel);
    let start = select(shadow_frame.splits.w, shadow_frame.splits[layer - min(layer, 1u)], layer > 0u);
    let end = shadow_frame.splits[layer];
    let blend = smoothstep(end - (end - start) * 0.1, end, depth);
    if layer < 2u { visible = mix(visible, cascade_visibility(world, normal, layer + 1u, pixel), blend); }
    else { visible = mix(visible, 1.0, blend); }
    // Camera-local fog may end before the stable cascade envelope. Retain a
    // smooth receiver fade there, including native materials that disable fog.
    if shadow_frame.eye.w < shadow_frame.splits.z {
        let fade_start = max(shadow_frame.splits.w, shadow_frame.eye.w * 0.9);
        visible = mix(visible, 1.0, smoothstep(fade_start, shadow_frame.eye.w, depth));
    }
    return mix(1.0, visible, clamp(shadow_frame.forward.w, 0.0, 1.0));
}
