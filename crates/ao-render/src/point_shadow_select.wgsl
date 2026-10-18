// MIT: cube axes/projection and incumbent distance bias ported from ao-vk pointshadow.cpp.
// The input pool has already passed THIS frame's GPU primary-owner admission.
struct CubeState { identity: vec4<u32>, fade: vec4<f32>, }
struct PointParams { eye: vec4<f32>, timing: vec4<f32>, }
@group(0) @binding(0) var<storage, read_write> pool: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> face_data: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> states: array<CubeState>;
@group(0) @binding(3) var<storage, read_write> fades: array<vec4<f32>>;
@group(0) @binding(4) var<uniform> params: PointParams;

fn source_identity(index: u32) -> vec4<u32> {
    let source = bitcast<vec4<u32>>(pool[323u + index]);
    // Owned lights retain identity when unrelated actors reorder the source list.
    let kind = (source.x & 1u) + 1u;
    if pool[66u + index * 4u + 3u].w < 0.0 { return vec4<u32>(kind, 0xffffffffu, source.z, source.w); }
    return vec4<u32>(kind, source.y, 0u, 0u);
}
fn source_fade(index: u32) -> f32 {
    // Birth age covers every current native candidate, outside both capacity caps.
    let packed = bitcast<u32>(pool[323u + index].x);
    return clamp(bitcast<f32>(packed & 0xfffffffeu), 0.0, 1.0);
}
fn face_matrix(face: u32, position: vec3<f32>, far: f32) -> mat4x4<f32> {
    let major = array<vec3<f32>, 6>(vec3<f32>(1,0,0), vec3<f32>(-1,0,0), vec3<f32>(0,1,0), vec3<f32>(0,-1,0), vec3<f32>(0,0,1), vec3<f32>(0,0,-1));
    let s = array<vec3<f32>, 6>(vec3<f32>(0,0,-1), vec3<f32>(0,0,1), vec3<f32>(1,0,0), vec3<f32>(1,0,0), vec3<f32>(1,0,0), vec3<f32>(-1,0,0));
    let t = array<vec3<f32>, 6>(vec3<f32>(0,-1,0), vec3<f32>(0,-1,0), vec3<f32>(0,0,1), vec3<f32>(0,0,-1), vec3<f32>(0,-1,0), vec3<f32>(0,-1,0));
    let a = far / (far - 0.1);
    let b = -far * 0.1 / (far - 0.1);
    return transpose(mat4x4<f32>(vec4<f32>(s[face], -dot(s[face], position)),
        vec4<f32>(-t[face], dot(t[face], position)),
        vec4<f32>(major[face] * a, -a * dot(major[face], position) + b),
        vec4<f32>(major[face], -dot(major[face], position))));
}

@compute @workgroup_size(1)
fn select_cubes() {
    let count = min(bitcast<u32>(pool[322u].x), 64u);
    let limit = min(u32(params.timing.y), 16u);
    let step = max(params.timing.x, 0.0);
    var wanted: array<u32, 16>;
    var scores: array<f32, 16>;
    var chosen: array<u32, 16>;
    for (var slot = 0u; slot < 16u; slot++) {
        wanted[slot] = 0xffffffffu;
        chosen[slot] = 0xffffffffu;
        scores[slot] = 3.402823e38;
    }
    for (var index = 0u; index < count; index++) {
        let light = pool[66u + index * 4u];
        if light.w <= 0.1 || !any(pool[66u + index * 4u + 1u].xyz > vec3<f32>(0.0)) { continue; }
        let identity = source_identity(index);
        var score = distance(light.xyz, params.eye.xyz);
        for (var slot = 0u; slot < limit; slot++) {
            if all(states[slot].identity == identity) { score *= 0.8; break; }
        }
        var at = 0u;
        loop {
            if at >= limit { break; }
            if score < scores[at] { break; }
            at++;
        }
        if at >= limit { continue; }
        var shift = limit - 1u;
        loop {
            if shift <= at { break; }
            scores[shift] = scores[shift - 1u];
            wanted[shift] = wanted[shift - 1u];
            shift--;
        }
        scores[at] = score;
        wanted[at] = index;
    }
    // Keep a current source in its previous slot while its departure fades.
    for (var slot = 0u; slot < 16u; slot++) {
        var current = 0xffffffffu;
        if slot < limit && states[slot].identity.x != 0u {
            for (var index = 0u; index < count; index++) {
                if pool[66u + index * 4u].w <= 0.1 || !any(pool[66u + index * 4u + 1u].xyz > vec3<f32>(0.0)) { continue; }
                if all(states[slot].identity == source_identity(index)) { current = index; break; }
            }
        }
        if current == 0xffffffffu { states[slot].identity = vec4<u32>(0u); states[slot].fade.x = 0.0; continue; }
        var retained = false;
        for (var k = 0u; k < limit; k++) { if wanted[k] == current { retained = true; } }
        let fade = select(states[slot].fade.x - step, source_fade(current), retained);
        if fade > 0.0 || retained { states[slot].fade.x = max(fade, 0.0); chosen[slot] = current; }
        else { states[slot].identity = vec4<u32>(0u); states[slot].fade.x = 0.0; }
    }
    for (var k = 0u; k < limit; k++) {
        let index = wanted[k];
        if index == 0xffffffffu { continue; }
        var assigned = false;
        for (var slot = 0u; slot < limit; slot++) { if chosen[slot] == index { assigned = true; } }
        if assigned { continue; }
        for (var slot = 0u; slot < limit; slot++) {
            if chosen[slot] == 0xffffffffu {
                chosen[slot] = index;
                states[slot].identity = source_identity(index);
                states[slot].fade.x = source_fade(index);
                break;
            }
        }
    }
    for (var slot = 0u; slot < 16u; slot++) {
        let index = chosen[slot];
        var light = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        var owned = false;
        fades[slot] = vec4<f32>(0.0);
        if index != 0xffffffffu {
            let li = 66u + index * 4u;
            light = pool[li];
            let encoded = pool[li + 3u].w;
            owned = encoded < 0.0;
            pool[li + 3u].w = select(f32(slot + 1u), encoded - f32(slot + 1u), owned);
            let group = select(0.0, floor(abs(encoded) / 32.0) - 1.0, owned);
            fades[slot] = vec4<f32>(states[slot].fade.x, group, 0.0, 0.0);
        }
        if slot >= limit { continue; }
        for (var face = 0u; face < 6u; face++) {
            let base = (slot * 6u + face) * u32(params.timing.z);
            let matrix = face_matrix(face, light.xyz, light.w);
            for (var column = 0u; column < 4u; column++) { face_data[base + column] = matrix[column]; }
            face_data[base + 15u] = bitcast<vec4<f32>>(vec4<u32>(0u, 0u, select(0u, 1u, index == 0xffffffffu), select(0u, 1u, owned)));
        }
    }
}
