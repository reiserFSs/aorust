// Current-frame admission only: no actor history survives this reduction.
@group(0) @binding(0) var primary_ids: texture_2d<u32>;
@group(0) @binding(1) var<storage, read_write> admitted: array<atomic<u32>>;

@compute @workgroup_size(8, 8)
fn visible_actors(@builtin(global_invocation_id) thread: vec3<u32>) {
    if any(thread.xy >= textureDimensions(primary_ids)) { return; }
    let id = textureLoad(primary_ids, vec2<i32>(thread.xy), 0).x;
    if id > 0u && id < 0x80000000u && id < arrayLength(&admitted) {
        atomicStore(&admitted[id], 1u);
    }
}
