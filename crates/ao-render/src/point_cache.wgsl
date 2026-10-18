// Feature-local indirect culling; current GPU owner proof precedes every actor hash.
struct CacheCaster { sphere: vec4<f32>, draw: vec4<u32>, key: vec4<u32>, spare: vec4<u32>, }
@group(0) @binding(5) var<storage, read> casters: array<CacheCaster>;
@group(0) @binding(6) var<storage, read_write> commands: array<u32>;
@group(0) @binding(7) var<storage, read> actor_visibility: array<u32>;
fn caster_admitted(c: CacheCaster) -> bool {
    if c.key.w == 0u || c.key.w == 2u { return true; }
    if c.draw.w == 0u || c.draw.w >= arrayLength(&actor_visibility) { return false; }
    return actor_visibility[c.draw.w] != 0u;
}
fn sphere_in_face(matrix: mat4x4<f32>, sphere: vec4<f32>, light: vec4<f32>) -> bool {
    if distance(sphere.xyz, light.xyz) > light.w + sphere.w { return false; }
    let m = transpose(matrix);
    let planes = array<vec4<f32>, 6>(m[3]+m[0],m[3]-m[0],m[3]+m[1],m[3]-m[1],m[2],m[3]-m[2]);
    for (var i=0u; i<6u; i++) {
        if dot(planes[i],vec4<f32>(sphere.xyz,1.0)) < -sphere.w*length(planes[i].xyz) { return false; }
    }
    return true;
}
@compute @workgroup_size(64)
fn cull_casters(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x > u32(params.timing.w) { return; }
    cull_command(id.x,id.y);
}
@compute @workgroup_size(64)
fn cull_actor_casters(@builtin(global_invocation_id) id: vec3<u32>) {
    let static_count=u32(params.timing.w);
    if id.x > u32(params.cache.x)-static_count { return; }
    let item=select(static_count+id.x,0u,id.x==0u);
    cull_command(item,id.y+u32(params.timing.y)*6u);
}
fn cull_command(item:u32,layer:u32) {
    let n = u32(params.cache.x);
    let face_count = u32(params.timing.y)*6u;
    let actor = layer >= face_count;
    let face = layer % face_count;
    let base = (layer*(n+1u)+item)*5u;
    let data_base = layer*u32(params.timing.z);
    let update = bitcast<vec4<u32>>(face_data[data_base+15u]).z == 0u;
    if item == 0u {
        commands[base]=3u; commands[base+1u]=select(0u,1u,update);
        commands[base+2u]=0u; commands[base+3u]=0u; commands[base+4u]=0u;
        return;
    }
    let c = casters[item-1u];
    let matrix = select_matrix(face,actor);
    let owned = bitcast<vec4<u32>>(face_data[data_base+15u]).w != 0u;
    let visible = update && (actor == (c.key.w != 0u)) && caster_admitted(c)
        && !(owned && c.key.z != 0u) && sphere_in_face(matrix,c.sphere,metadata.lights[face/6u]);
    commands[base]=c.draw.x; commands[base+1u]=select(0u,1u,visible);
    commands[base+2u]=c.draw.y; commands[base+3u]=0u; commands[base+4u]=0u;
}
fn select_matrix(face:u32, actor:bool)->mat4x4<f32> {
    if actor { return metadata.faces[face].dynamic_matrix; }
    return metadata.faces[face].static_matrix;
}
@vertex fn clear_face(@builtin(vertex_index) index:u32)->@builtin(position) vec4<f32> {
    let p=array<vec2<f32>,3>(vec2<f32>(-1,-1),vec2<f32>(3,-1),vec2<f32>(-1,3));
    return vec4<f32>(p[index],1.0,1.0);
}
