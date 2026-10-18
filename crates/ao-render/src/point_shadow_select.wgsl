// MIT: cube axes/projection and incumbent distance bias ported from ao-vk pointshadow.cpp.
// The input pool has already passed THIS frame's GPU primary-owner admission.
struct CubeState {
    identity: vec4<u32>, fade: vec4<f32>, light: vec4<f32>,
    cache: vec4<u32>, progress: vec4<u32>, clock: vec4<f32>,
}
struct PointParams { eye: vec4<f32>, timing: vec4<f32>, cache: vec4<f32>, schedule: vec4<f32>, }
struct PointFaceMetadata { static_matrix: mat4x4<f32>, dynamic_matrix: mat4x4<f32>, params: vec4<f32>, }
struct PointMetadata { slots: array<vec4<f32>,16>, lights: array<vec4<f32>,16>, faces: array<PointFaceMetadata,96>, }
@group(0) @binding(0) var<storage, read_write> pool: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> face_data: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> states: array<CubeState>;
@group(0) @binding(3) var<storage, read_write> metadata: PointMetadata;
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
    let world_hash = u32(params.cache.y) | (u32(params.cache.w) << 16u);
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
                states[slot].progress = vec4<u32>(63u,63u,0u,0u);
                states[slot].clock.w = 0.0;
                states[slot].fade.x = source_fade(index);
                break;
            }
        }
    }
    for (var slot = 0u; slot < 16u; slot++) {
        let index = chosen[slot];
        var light = vec4<f32>(0.0, 0.0, 0.0, 1.0);
        var owned = false;
        metadata.slots[slot] = vec4<f32>(0.0);
        if index != 0xffffffffu {
            let li = 66u + index * 4u;
            light = pool[li];
            let encoded = pool[li + 3u].w;
            owned = encoded < 0.0;
            pool[li + 3u].w = select(f32(slot + 1u), encoded - f32(slot + 1u), owned);
            let group = select(0.0, floor(abs(encoded) / 32.0) - 1.0, owned);
            metadata.slots[slot] = vec4<f32>(states[slot].fade.x, group, 0.0, 0.0);
        }
        if slot >= limit { continue; }
        var actor_pose = 0u;
        var membership = 0u;
        var membership_second = 0u;
        var dynamic_world = false;
        var has_actor_depth = false;
        for (var i=u32(params.timing.w); i<u32(params.cache.x); i++) {
            let c = casters[i];
            // Hidden owners must not affect even a radius test or cache schedule.
            if c.key.w == 0u || !caster_admitted(c) { continue; }
            if distance(c.sphere.xyz, light.xyz) > light.w + c.sphere.w { continue; }
            if owned && c.key.z != 0u { continue; }
            if c.draw.x != 0u { has_actor_depth = true; }
            if c.key.w == 2u { dynamic_world = true; }
            membership += (c.key.y * 1664525u + c.spare.w * 1013904223u + c.draw.y + c.draw.x + c.key.w * 2654435761u);
            var stable = c.key.y ^ (c.spare.w * 2246822519u) ^ (c.key.w * 2654435761u);
            stable = (stable ^ (stable >> 16u)) * 3266489917u;
            membership_second += stable ^ (stable >> 13u);
            actor_pose += c.key.x;
        }
        metadata.lights[slot] = light;
        let d = distance(light.xyz, params.eye.xyz);
        let ratio = select(select(0.25,0.5,d<80.0),1.0,d<30.0);
        let cold = states[slot].clock.w == 0.0;
        let changed_light = any(states[slot].light != light);
        let changed_size = states[slot].cache.w != bitcast<u32>(ratio);
        let previous_membership_second = u32(states[slot].clock.y) | (u32(states[slot].clock.z) << 16u);
        let membership_changed = states[slot].cache.z != membership || previous_membership_second != membership_second;
        let refresh_epoch=floor(max(params.cache.z,0.0)/120.0);
        if changed_light || changed_size || states[slot].cache.x != world_hash || states[slot].clock.x != refresh_epoch {
            states[slot].progress.x = 63u;
            states[slot].clock.x = refresh_epoch;
        }
        if changed_light || changed_size || states[slot].cache.y != actor_pose || membership_changed {
            states[slot].progress.y = 63u;
        }
        // Restoring a mature source after owner/cap loss must not temporarily
        // remove its visible world occlusion. New arrivals retain their native fade.
        let restoration = cold && states[slot].fade.x >= 1.0 && params.schedule.y > 0.0;
        let bootstrap = cold && (params.schedule.z > 0.0 || restoration);
        let allowed_mask = select(u32(params.schedule.x),63u,bootstrap);
        let static_mask = u32(params.eye.w) & allowed_mask & states[slot].progress.x;
        states[slot].progress.x &= ~static_mask;
        var actor_mask = 0u;
        if cold { actor_mask=63u; }
        else {
            for (var k=0u;k<2u;k++) {
                for (var scan=0u;scan<6u;scan++) {
                    let f=states[slot].progress.w%6u; states[slot].progress.w=(f+1u)%6u;
                    if (states[slot].progress.y & (1u<<f)) != 0u {
                        actor_mask |= 1u<<f; states[slot].progress.y &= ~(1u<<f); break;
                    }
                }
            }
            // Exact admission loss exception: remove all retained hidden depth now.
            if membership_changed { actor_mask=63u; }
        }
        // Native pushed vegetation is current-frame geometry, never private cached history.
        if dynamic_world { actor_mask=63u; states[slot].progress.y=0u; }
        if cold { states[slot].progress.y=0u; }
        if membership_changed { states[slot].progress.y=0u; }
        actor_mask &= u32(params.schedule.w);
        states[slot].light=light;
        states[slot].cache=vec4<u32>(world_hash,actor_pose,membership,bitcast<u32>(ratio));
        states[slot].clock.w=1.0;
        states[slot].clock.y=f32(membership_second & 65535u);
        states[slot].clock.z=f32(membership_second >> 16u);
        for (var face = 0u; face < 6u; face++) {
            let face_index=slot*6u+face;
            if cold {
                metadata.faces[face_index].params.x=0.0;
                metadata.faces[face_index].params.w=0.0;
            }
            if !has_actor_depth { metadata.faces[face_index].params.w=0.0; }
            let matrix=face_matrix(face,light.xyz,light.w);
            for (var kind=0u;kind<2u;kind++) {
                let layer=face_index+kind*limit*6u;
                let base=layer*u32(params.timing.z);
                let mask=select(static_mask,actor_mask,kind==1u);
                let update=(mask & (1u<<face)) != 0u && index != 0xffffffffu;
                if update {
                    if kind==0u {
                        metadata.faces[face_index].static_matrix=matrix;
                        metadata.faces[face_index].params.x=1.0; metadata.faces[face_index].params.y=ratio;
                    } else {
                        metadata.faces[face_index].dynamic_matrix=matrix;
                        metadata.faces[face_index].params.z=ratio;
                        metadata.faces[face_index].params.w=select(0.0,1.0,has_actor_depth);
                    }
                    // Raster into the upper-left bounded quadrant; receiver stores the unscaled matrix.
                    var raster=matrix;
                    for (var col=0u;col<4u;col++) {
                        raster[col].x=matrix[col].x*ratio+matrix[col].w*(ratio-1.0);
                        raster[col].y=matrix[col].y*ratio+matrix[col].w*(1.0-ratio);
                        face_data[base+col]=raster[col];
                    }
                    face_data[base+14u]=vec4<f32>(0.0,0.0,0.0,ratio*256.0);
                }
                face_data[base+15u]=bitcast<vec4<f32>>(vec4<u32>(0u,0x80000000u,select(1u,0u,update),select(0u,1u,owned)));
            }
        }
    }
}
