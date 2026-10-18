// Plant push spring adapted from ao-vk (MIT, Copyright 2026 wannabuh).
// Shared card/sprig/caster deformation, with ao-vk's centre-rooted branch motion
// and fixed-world gust axes. All clocks use integer harmonics of the wrapped phase.
fn foliage_owner_admitted(owner: u32) -> bool {
    // Slot zero is explicit local own-control only, never a missing remote actor.
    if owner == 0u { return true; }
    if owner >= arrayLength(&interaction_visibility) { return false; }
    return interaction_visibility[owner] != 0u;
}

fn foliage_vertex(v: VIn, vertex: VOut) -> VOut {
    return foliage_vertex_at(v, vertex, g.foliage_wind.w, true);
}

// A detached leaf samples only wind at its release clock. Actor contacts must
// never be baked into a persistent fall trajectory after admission revocation.
fn foliage_vertex_at(v: VIn, vertex: VOut, clock: f32, with_push: bool) -> VOut {
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
    let time = clock / 0.05;
    let crown = (o.wpos - origin) / max(height * 0.5, 0.001);
    let radius = min(length(crown), 1.3);
    let branch = crown / max(radius, 0.001);
    let phase = origin.x * 0.31 + origin.z * 0.23 + dot(branch, vec3<f32>(2.3, 1.7, 3.1));
    let gust = grass_gust(aa, time);
    let strength = min(0.65 + g.foliage_wind.z, 2.3);
    let storm = clamp((g.foliage_wind.z - 1.2) / 1.3, 0.0, 1.0);
    let swing = (0.55 * sin(clock * 26.0 + phase) + 0.3 * sin(clock * 58.0 + phase * 1.9))
        * (1.0 - 0.4 * storm) + 0.6 * gust + 0.8 * storm;
    // Small plants bend from the ground; raised crowns flex out from their
    // centre, which stays attached to the unmoving native trunk.
    let tree = smoothstep(2.0, 5.0, height);
    let attachment = mix(h * h, min(radius * radius, 1.0) * h, tree);
    let amplitude = min(height * 0.09, 0.9) * attachment * strength * g.foliage_flags.x;
    let across = vec2<f32>(-g.foliage_wind.y, g.foliage_wind.x);
    var bend = amplitude * (g.foliage_wind.xy * swing
        + across * 0.22 * sin(clock * 42.0 + phase * 2.7));
    if with_push && g.foliage_flags.y > 0.0 {
        var push = vec2<f32>(0.0);
        for (var k = 0u; k < 16u; k += 1u) {
            let owner = g.foliage_owners[k / 4u][k % 4u];
            if !foliage_owner_admitted(owner) { continue; }
            let source = g.foliage_pushers[k];
            let delta = o.wpos.xz - source.xz;
            let distance = length(delta);
            // Vertex height, not canopy-base height: nearby low branches can
            // part around the body, without moving foliage on another floor.
            let dy = o.wpos.y - source.y;
            let vertical_reach = smoothstep(-1.0, -0.3, dy) * (1.0 - smoothstep(2.0, 3.0, dy));
            let near = (1.0 - smoothstep(0.3, 1.8, distance)) * vertical_reach;
            let direction = select(vec2<f32>(0.7071), delta / max(distance, 0.001), distance > 0.001);
            push += direction * near * source.w;
        }
        let size = length(push);
        bend += push / max(size, 1.0) * min(0.85, height * 0.8) * h * sqrt(h);
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
