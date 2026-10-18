// Plant push spring adapted from ao-vk (MIT, Copyright 2026 wannabuh).
// Wind is the exact stabilized grass oscillator appended by foliage.rs.
fn foliage_vertex(v: VIn, vertex: VOut) -> VOut {
    var o = vertex;
    let model = mat4x4<f32>(vec4<f32>(v.m0.xyz, 0.0), v.m1, v.m2, v.m3);
    // Choose the authored axis which actually points up after placement. This
    // anchors sideways-authored palms and upside-down cards at their real base.
    let vertical = vec3<f32>(v.m0.y, v.m1.y, v.m2.y);
    var axis = 0u;
    if abs(vertical.y) > abs(vertical.x) { axis = 1u; }
    if abs(vertical.z) > abs(vertical[axis]) { axis = 2u; }
    let low = mat.wave.xyz;
    let high = mat.uv_camera.xyz;
    let extent = max(high[axis] - low[axis], 0.001);
    let base = select(high[axis], low[axis], vertical[axis] >= 0.0);
    let h = clamp((v.pos[axis] - base) / extent * sign(vertical[axis]), 0.0, 1.0);
    let height = extent * length(model[axis].xyz);
    var root_model = v.pos;
    root_model[axis] = base;
    let root = (model * vec4<f32>(root_model, 1.0)).xyz;
    let origin = (model * vec4<f32>((low + high) * 0.5, 1.0)).xyz;
    // World axes remain fixed under weather turns: never multiply spatial
    // phases by a changing wind direction (GrassTwitch a5473f12).
    let aa = vec2<f32>(dot(origin.xz, vec2<f32>(0.8, 0.6)), dot(origin.xz, vec2<f32>(-0.6, 0.8)));
    let time = g.foliage_wind.w / 0.05;
    let phase = origin.x * 0.3 + origin.z * 0.25;
    let gust = grass_gust(aa, time);
    let amplitude = min(height * 0.06, 0.65) * h * h;
    var bend = g.foliage_wind.xy * (amplitude * g.foliage_wind.z * (grass_sway(aa, time, phase, gust) + 0.7 * gust)) * g.foliage_flags.x;
    if g.foliage_flags.y > 0.0 {
        var push = vec2<f32>(0.0);
        for (var k = 0u; k < 16u; k += 1u) {
            let source = g.foliage_pushers[k];
            let delta = o.wpos.xz - source.xz;
            let distance = length(delta);
            let near = max(1.0 - distance / 1.4, 0.0) * max(1.0 - abs(root.y - source.y) / 1.5, 0.0);
            let direction = select(vec2<f32>(0.7071), delta / max(distance, 0.001), distance > 0.001);
            push += direction * near * near * source.w;
        }
        let size = length(push);
        bend += push / max(size, 1.0) * min(0.75, height * 0.8) * h * h;
    }
    // Length-preserving bend: tips cannot rise and expose a brighter silhouette.
    let above = max((o.wpos.y - root.y), 0.0);
    let length_bend = length(bend);
    let capped = min(length_bend, above * 0.8);
    bend *= capped / max(length_bend, 0.001);
    o.wpos += vec3<f32>(bend.x, sqrt(max(above * above - dot(bend, bend), 0.0)) - above, bend.y);
    o.clip = g.vp * vec4<f32>(o.wpos, 1.0);
    o.fog = max(vertex.fog, vertex_fog(-(g.view * vec4<f32>(o.wpos, 1.0)).z));
    return o;
}
