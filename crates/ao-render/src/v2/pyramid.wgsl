// Enhanced v2 depth pyramid (HiZ): R32Float min VIEW depth, mips 0..7. `common.wgsl` precedes this file.
//   mip 0 = linearised view depth (metres along the view axis), SKY (1e9) where the G-buffer has no geometry;
//   mip m>0 = min of the 2x2 block of mip m-1 (blocks past the edge repeat the last row/column).
// Two dispatches: `build_a` reads the D32F depth and writes mips 0..3 (one 16x16 source tile per workgroup, mips 2 and 3
// through workgroup memory); `build_b` reads mip 3 and writes mips 4..7 the same way.
const HIZ_SKY: f32 = 1.0e9;

@group(0) @binding(0) var<uniform> fc: FC;
@group(0) @binding(1) var depth_in: texture_depth_2d;
@group(0) @binding(2) var m0: texture_storage_2d<r32float, write>;
@group(0) @binding(3) var m1: texture_storage_2d<r32float, write>;
@group(0) @binding(4) var m2: texture_storage_2d<r32float, write>;
@group(0) @binding(5) var m3: texture_storage_2d<r32float, write>;
@group(0) @binding(6) var m3_in: texture_2d<f32>;
@group(0) @binding(7) var m4: texture_storage_2d<r32float, write>;
@group(0) @binding(8) var m5: texture_storage_2d<r32float, write>;
@group(0) @binding(9) var m6: texture_storage_2d<r32float, write>;
@group(0) @binding(10) var m7: texture_storage_2d<r32float, write>;

var<workgroup> tile: array<f32, 64>;

// Standard-Z (0 near .. 1 far) hardware depth to view depth: the inverse of `perspective_rh`.
fn linear_depth(d: f32) -> f32 {
    if d >= 1.0 { return HIZ_SKY; }
    let n = fc.proj.x;
    let f = fc.proj.y;
    return n * f / (f - d * (f - n));
}

fn min4(i: u32) -> f32 { return min(min(tile[i], tile[i + 1u]), min(tile[i + 8u], tile[i + 9u])); }

@compute @workgroup_size(8, 8)
fn build_a(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let d0 = vec2<i32>(textureDimensions(m0));
    let src = vec2<i32>(wg.xy * 16u + l.xy * 2u);
    var m = HIZ_SKY;
    for (var k = 0; k < 4; k++) {
        let q = src + vec2<i32>(k & 1, k / 2);
        let z = linear_depth(textureLoad(depth_in, min(q, d0 - vec2<i32>(1)), 0));
        if all(q < d0) { textureStore(m0, q, vec4<f32>(z)); }
        m = min(m, z);
    }
    let t1 = vec2<i32>(wg.xy * 8u + l.xy);
    if all(t1 < vec2<i32>(textureDimensions(m1))) { textureStore(m1, t1, vec4<f32>(m)); }
    tile[li] = m;
    workgroupBarrier();
    var v2 = 0.0;
    if l.x < 4u && l.y < 4u {
        v2 = min4(l.y * 16u + l.x * 2u);
        let t2 = vec2<i32>(wg.xy * 4u + l.xy);
        if all(t2 < vec2<i32>(textureDimensions(m2))) { textureStore(m2, t2, vec4<f32>(v2)); }
    }
    workgroupBarrier();
    if l.x < 4u && l.y < 4u { tile[li] = v2; }
    workgroupBarrier();
    if l.x < 2u && l.y < 2u {
        let t3 = vec2<i32>(wg.xy * 2u + l.xy);
        if all(t3 < vec2<i32>(textureDimensions(m3))) { textureStore(m3, t3, vec4<f32>(min4(l.y * 16u + l.x * 2u))); }
    }
}

@compute @workgroup_size(8, 8)
fn build_b(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let d3 = vec2<i32>(textureDimensions(m3_in));
    let src = vec2<i32>(wg.xy * 16u + l.xy * 2u);
    var m = HIZ_SKY;
    for (var k = 0; k < 4; k++) {
        let q = min(src + vec2<i32>(k & 1, k / 2), d3 - vec2<i32>(1));
        m = min(m, textureLoad(m3_in, q, 0).r);
    }
    let t4 = vec2<i32>(wg.xy * 8u + l.xy);
    if all(t4 < vec2<i32>(textureDimensions(m4))) { textureStore(m4, t4, vec4<f32>(m)); }
    tile[li] = m;
    workgroupBarrier();
    var v5 = 0.0;
    if l.x < 4u && l.y < 4u {
        v5 = min4(l.y * 16u + l.x * 2u);
        let t5 = vec2<i32>(wg.xy * 4u + l.xy);
        if all(t5 < vec2<i32>(textureDimensions(m5))) { textureStore(m5, t5, vec4<f32>(v5)); }
    }
    workgroupBarrier();
    var v6 = 0.0;
    if l.x < 4u && l.y < 4u { tile[li] = v5; }
    workgroupBarrier();
    if l.x < 2u && l.y < 2u {
        v6 = min4(l.y * 16u + l.x * 2u);
        let t6 = vec2<i32>(wg.xy * 2u + l.xy);
        if all(t6 < vec2<i32>(textureDimensions(m6))) { textureStore(m6, t6, vec4<f32>(v6)); }
    }
    workgroupBarrier();
    if l.x < 2u && l.y < 2u { tile[li] = v6; }
    workgroupBarrier();
    if li == 0u {
        let t7 = vec2<i32>(wg.xy);
        if all(t7 < vec2<i32>(textureDimensions(m7))) { textureStore(m7, t7, vec4<f32>(min4(0u))); }
    }
}
