// Resolve plugin (group 1): sun cascades + moon A. Each cascade is a STATIC depth layer (world casters only, cached, sampled with the
// matrix it was rendered with) and a DYNAMIC depth layer (admitted actors only, rebuilt every frame with this frame's matrix); the
// receiver takes the `min` of the two taps. Concatenated before `resolve.wgsl`, which declares `fc`.
struct Receive {
    sun_static: array<mat4x4<f32>, 3>,
    sun_dyn: array<mat4x4<f32>, 3>,
    moon_static: mat4x4<f32>,
    moon_dyn: mat4x4<f32>,
    splits: vec4<f32>,   // cascade ends 0, 1, 2 (view depth), near
    fade: vec4<f32>,     // x sun horizon strength, y last sun cascade fade end, z moon horizon strength, w moon range
    flags: vec4<u32>,    // x sun dynamic layers valid, y moon dynamic layer valid, z sun active, w moon active
}
@group(1) @binding(0) var<uniform> sm: Receive;
@group(1) @binding(1) var sun_static_t: texture_depth_2d_array;
@group(1) @binding(2) var sun_dyn_t: texture_depth_2d_array;
@group(1) @binding(3) var moon_static_t: texture_depth_2d_array;
@group(1) @binding(4) var moon_dyn_t: texture_depth_2d_array;
@group(1) @binding(5) var sm_cmp: sampler_comparison;

// 3x3 box PCF that slides bilinearly with the receiver: four 2x2 compare-gathers cover the 4x4 texels, the outer rows/columns
// are weighted by the sub-texel position. Gather lanes: x = (0,1), y = (1,1), z = (1,0), w = (0,0) (column, row).
fn pcf_box(t: texture_depth_2d_array, layer: i32, uv: vec2<f32>, dim: vec2<f32>, zref: f32) -> f32 {
    let pos = uv * dim - vec2<f32>(0.5);
    let base = floor(pos);
    let f = pos - base;
    let c0 = base / dim;
    let c1 = (base + vec2<f32>(2.0)) / dim;
    let g00 = textureGatherCompare(t, sm_cmp, vec2<f32>(c0.x, c0.y), layer, zref);
    let g10 = textureGatherCompare(t, sm_cmp, vec2<f32>(c1.x, c0.y), layer, zref);
    let g01 = textureGatherCompare(t, sm_cmp, vec2<f32>(c0.x, c1.y), layer, zref);
    let g11 = textureGatherCompare(t, sm_cmp, vec2<f32>(c1.x, c1.y), layer, zref);
    let wx = vec4<f32>(1.0 - f.x, 1.0, 1.0, f.x);
    let r0 = dot(wx, vec4<f32>(g00.w, g00.z, g10.w, g10.z));
    let r1 = dot(wx, vec4<f32>(g00.x, g00.y, g10.x, g10.y));
    let r2 = dot(wx, vec4<f32>(g01.w, g01.z, g11.w, g11.z));
    let r3 = dot(wx, vec4<f32>(g01.x, g01.y, g11.x, g11.y));
    return ((1.0 - f.y) * r0 + r1 + r2 + f.y * r3) / 9.0;
}

// One depth layer: normal offset + slope-scaled depth bias (both bounded, in texels), 3x3 PCF. Outside the rendered square the
// layer says "lit" (the next cascade or the fade takes over).
fn layer_visibility(t: texture_depth_2d_array, layer: i32, m: mat4x4<f32>, wp: vec3<f32>, n: vec3<f32>, l: vec3<f32>) -> f32 {
    let dim = vec2<f32>(textureDimensions(t));
    let row_x = vec3<f32>(m[0].x, m[1].x, m[2].x);
    let row_z = vec3<f32>(m[0].z, m[1].z, m[2].z);
    let texel = 2.0 / (length(row_x) * dim.x);   // world size of one texel
    let ndl = clamp(dot(n, l), 0.0, 1.0);
    let tan_theta = min(sqrt(max(1.0 - ndl * ndl, 0.0)) / max(ndl, 0.05), 2.5);
    let p = (m * vec4<f32>(wp + n * (texel * 0.8 * (1.0 - ndl)), 1.0)).xyz;
    let uv = p.xy * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let guard = vec2<f32>(2.5) / dim;
    if any(uv <= guard) || any(uv >= vec2<f32>(1.0) - guard) || p.z <= 0.0 || p.z >= 1.0 { return 1.0; }
    let zref = p.z - texel * (0.5 + tan_theta) * length(row_z);
    return pcf_box(t, layer, uv, dim, zref);
}

fn sun_cascade(k: i32, wp: vec3<f32>, n: vec3<f32>) -> f32 {
    let l = fc.sun_dir.xyz;
    var v = layer_visibility(sun_static_t, k, sm.sun_static[k], wp, n, l);
    if sm.flags.x != 0u && v > 0.0 { v = min(v, layer_visibility(sun_dyn_t, k, sm.sun_dyn[k], wp, n, l)); }
    return v;
}

fn sun_visibility(wp: vec3<f32>, view_depth: f32, n: vec3<f32>) -> f32 {
    if sm.flags.z == 0u { return 1.0; }
    let s = sm.splits;
    if view_depth < s.w || view_depth >= sm.fade.y { return 1.0; }
    var k = 0;
    var start = s.w;
    var end = s.x;
    if view_depth > s.x { k = 1; start = s.x; end = s.y; }
    if view_depth > s.y { k = 2; start = s.y; end = sm.fade.y; }
    var vis = sun_cascade(k, wp, n);
    // 10% crossfade into the next cascade; the last one fades out to lit toward its end (fog end / last split).
    let blend = smoothstep(end - (end - start) * 0.1, end, view_depth);
    if blend > 0.0 {
        if k < 2 { vis = mix(vis, sun_cascade(k + 1, wp, n), blend); } else { vis = mix(vis, 1.0, blend); }
    }
    return mix(1.0, vis, sm.fade.x);
}

fn moon_visibility(wp: vec3<f32>, view_depth: f32, n: vec3<f32>) -> f32 {
    if sm.flags.w == 0u || view_depth >= sm.fade.w { return 1.0; }
    let l = fc.moon_dir.xyz;
    var vis = layer_visibility(moon_static_t, 0, sm.moon_static, wp, n, l);
    if sm.flags.y != 0u && vis > 0.0 { vis = min(vis, layer_visibility(moon_dyn_t, 0, sm.moon_dyn, wp, n, l)); }
    vis = mix(vis, 1.0, smoothstep(sm.fade.w * 0.9, sm.fade.w, view_depth));
    return mix(1.0, vis, sm.fade.z);
}
