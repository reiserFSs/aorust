// Actor admission reduction: the R32Uint id target (actor interaction slots of the depth-tested, unfogged actor pixels) ->
// `visible[slot] = 1`. Current frame only: the buffer is cleared right before this pass, nothing is carried over.
@group(0) @binding(0) var ids: texture_2d<u32>;
@group(0) @binding(1) var<storage, read_write> visible: array<atomic<u32>>;

var<workgroup> tile: array<u32, 64>;

@compute @workgroup_size(8, 8)
fn reduce(@builtin(global_invocation_id) t: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    var id = 0u;
    if all(t.xy < textureDimensions(ids)) { id = textureLoad(ids, vec2<i32>(t.xy), 0).x; }
    tile[li] = id;
    workgroupBarrier();
    if id == 0u || id >= arrayLength(&visible) { return; }
    // One store per distinct slot per tile: a character covers thousands of pixels with one address.
    for (var j = 0u; j < li; j++) { if tile[j] == id { return; } }
    atomicStore(&visible[id], 1u);
}
