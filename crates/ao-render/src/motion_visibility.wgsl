// Reduce hardware-depth-admitted ID samples. Fragment storage writes cannot
// prove visibility because their side effects may precede late depth tests.
struct VisibilityParams { count: u32, capacity: u32, unused0: u32, unused1: u32 }
@group(0) @binding(0) var identities: texture_multisampled_2d<u32>;
@group(0) @binding(1) var<storage, read_write> current_visible: array<atomic<u32>>;
@group(0) @binding(2) var<storage, read> previous_visible: array<u32>;
@group(0) @binding(3) var<storage, read_write> visibility_changed: atomic<u32>;
@group(0) @binding(4) var<storage, read> actor_slots: array<vec2<u32>>;
@group(0) @binding(5) var<uniform> params: VisibilityParams;
fn admit(id: u32) {
    if params.count == 0u { return; }
    if id < actor_slots[0].x || id > actor_slots[params.count - 1u].x { return; }
    var first = 0u;
    var end = params.count;
    while first < end {
        let middle = first + (end - first) / 2u;
        let entry = actor_slots[middle];
        if entry.x < id { first = middle + 1u; } else { end = middle; }
    }
    if first < params.count && actor_slots[first].x == id {
        atomicStore(&current_visible[actor_slots[first].y], id);
    }
}
@compute @workgroup_size(8, 8) fn collect(@builtin(global_invocation_id) invocation: vec3<u32>) {
    if any(invocation.xy >= textureDimensions(identities)) { return; }
    var previous = 0u;
    for (var sample = 0; sample < i32(textureNumSamples(identities)); sample++) {
        let id = textureLoad(identities, vec2<i32>(invocation.xy), sample).x;
        if id != previous { admit(id); }
        previous = id;
    }
}
@compute @workgroup_size(64) fn compare(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let slot = invocation.x;
    if slot >= params.capacity { return; }
    if atomicLoad(&current_visible[slot]) != previous_visible[slot] {
        atomicStore(&visibility_changed, 1u);
    }
}
