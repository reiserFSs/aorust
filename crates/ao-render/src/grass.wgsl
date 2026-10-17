// Shares the native terrain texture, palette, fog and projection (shader.wgsl).
struct GrassCell { origin: vec4<f32>, heights: vec4<f32>, identity: vec4<u32>, normals: array<vec4<f32>, 4> }
struct GrassParams { wind_time: vec4<f32>, limits: vec4<f32>, benders: array<vec4<f32>, 16> }
@group(3) @binding(0) var<storage, read> grass_cells: array<GrassCell>;
@group(3) @binding(1) var<uniform> grass_params: GrassParams;
fn grass_hash(input: u32) -> u32 {
    var v = input ^ (input >> 16u); v *= 0x7feb352du;
    v ^= v >> 15u; v *= 0x846ca68bu; return v ^ (v >> 16u);
}
fn grass_unit(v: u32) -> f32 { return f32(v & 0x00ffffffu) / 16777216.0; }
// Native alternating diagonal (N3 10017800), not bilinear interpolation.
fn grass_height(c: GrassCell, uv: vec2<f32>) -> f32 {
    let h = c.heights;
    if c.identity.z == 0u {
        if uv.x + uv.y <= 1.0 { return h.x + uv.x * (h.y - h.x) + uv.y * (h.z - h.x); }
        return h.w + (1.0 - uv.x) * (h.z - h.w) + (1.0 - uv.y) * (h.y - h.w);
    }
    if uv.x >= uv.y { return h.x + uv.x * (h.y - h.x) + uv.y * (h.w - h.y); }
    return h.x + uv.y * (h.z - h.x) + uv.x * (h.w - h.z);
}
fn grass_weights(parity: u32, uv: vec2<f32>) -> vec4<f32> {
    if parity == 0u {
        if uv.x + uv.y <= 1.0 { return vec4<f32>(1.0 - uv.x - uv.y, uv.x, uv.y, 0.0); }
        return vec4<f32>(0.0, 1.0 - uv.y, 1.0 - uv.x, uv.x + uv.y - 1.0);
    }
    if uv.x >= uv.y { return vec4<f32>(1.0 - uv.x, uv.x - uv.y, 0.0, uv.y); }
    return vec4<f32>(1.0 - uv.y, 0.0, uv.y - uv.x, uv.x);
}
fn grass_uv(orientation: u32, uv: vec2<f32>) -> vec2<f32> {
    switch orientation {
        case 0u: { return vec2<f32>(uv.x, 1.0 - uv.y); }
        case 1u: { return vec2<f32>(1.0 - uv.y, 1.0 - uv.x); }
        case 2u: { return vec2<f32>(1.0 - uv.x, uv.y); }
        default: { return vec2<f32>(uv.y, uv.x); }
    }
}
@vertex
fn vs_grass(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> VOut {
    let c = grass_cells[instance / 16u];
    let seed = grass_hash(c.identity.x ^ (instance % 16u));
    let uv = vec2<f32>(grass_unit(seed), grass_unit(grass_hash(seed)));
    let root = vec3<f32>(c.origin.x + uv.x * c.origin.z, grass_height(c, uv), c.origin.y - uv.y * c.origin.z);
    let radius = min(grass_params.wind_time.w, g.eye.w);
    let fade = 1.0 - smoothstep(radius * 0.65, max(radius, 0.001), distance(root, g.eye.xyz));
    var o: VOut;
    if fade <= 0.0 || grass_unit(grass_hash(seed ^ 0x993u)) >= grass_params.limits.x {
        o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return o;
    }
    let corners = array<vec2<f32>, 6>(vec2<f32>(-1.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(-0.2, 1.0), vec2<f32>(-0.2, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.2, 1.0));
    let corner = corners[vertex];
    let angle = grass_unit(grass_hash(seed ^ 0x42u)) * 6.283185307;
    let width = vec2<f32>(cos(angle), sin(angle)) * (corner.x * 0.035 * fade);
    let height = (0.25 + grass_unit(grass_hash(seed ^ 0x85u)) * 0.2) * fade;
    let wind = clamp(grass_params.wind_time.xy, vec2<f32>(-3.0), vec2<f32>(3.0)) * (sin(grass_params.wind_time.z * 1.7 + root.x * 0.37 + root.z * 0.29) * 0.06 * fade);
    var bend = wind;
    // Current frame only: no recovery/trail state can retain a vanished actor.
    for (var index = 0u; index < min(u32(grass_params.limits.z), 16u); index += 1u) {
        let actor = grass_params.benders[index];
        if actor.w == 0.0 || abs(actor.y - root.y) > 1.5 { continue; }
        let delta = root.xz - actor.xz;
        let d = length(delta);
        bend += delta / max(d, 0.001) * (max(1.0 - d / 0.8, 0.0) * 0.35 * fade);
    }
    if grass_params.limits.z > 0.0 {
        bend = clamp(bend, vec2<f32>(-0.45 * fade), vec2<f32>(0.45 * fade));
    }
    let p = root + vec3<f32>(width.x + bend.x * corner.y, height * corner.y, width.y + bend.y * corner.y);
    o.clip = g.vp * vec4<f32>(p, 1.0);
    o.wpos = root;
    o.uv = grass_uv(c.identity.y, uv);
    // Ground-root fog: raised tips cannot reduce native extinction.
    o.fog = max(vertex_fog(-(g.view * vec4<f32>(root, 1.0)).z), vertex_fog(-(g.view * vec4<f32>(p, 1.0)).z));
    o.color = vec4<f32>(c.origin.w, 1.0, 1.0, 1.0);
    // Evaluate the same four native terrain-corner Gouraud values, then
    // interpolate on the native diagonal. Blade tips never gain extra light.
    let offsets = array<vec2<f32>, 4>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0));
    let weights = grass_weights(c.identity.z, uv);
    o.light = vec3<f32>(0.0);
    var root_fog = 0.0;
    for (var j = 0u; j < 4u; j += 1u) {
        let position = vec3<f32>(c.origin.x + offsets[j].x * c.origin.z, c.heights[j], c.origin.y - offsets[j].y * c.origin.z);
        let normal = normalize(c.normals[j].xyz);
        let light = clamp(g.sun_g.rgb * max(dot(normal, g.sun_dir.xyz), 0.0) + vertex_lights(position, normal, 0.0)[0], vec3<f32>(0.0), vec3<f32>(1.0));
        // Simple ground adds/saturates ambient at each corner before interpolation.
        o.light += weights[j] * select(clamp(light + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), light, c.origin.w > 0.5);
        root_fog += weights[j] * vertex_fog(-(g.view * vec4<f32>(position, 1.0)).z);
    }
    o.fog = max(o.fog, root_fog);
    return o;
}
struct GrassOutput {
    @location(0) color: vec4<f32>,
    //__AMBIENT_FIELD__
    //__EMISSIVE_FIELD__
}
@fragment
fn fs_grass(i: VOut) -> GrassOutput {
    let light = select(i.light, clamp(i.light + ground_palette(i), vec3<f32>(0.0), vec3<f32>(1.0)), i.color.r > 0.5);
    let tile = ground_tile(i);
    let fog = ground_fog(i);
    var color = mix(tile * light, g.fog_g.rgb, fog);
    if i.color.r > 1.5 {
        // Preserve native two-pass fog multiplication, not the fused approximation.
        color = mix(tile, g.fog_g.rgb, fog) * mix(light, vec3<f32>(1.0), fog);
    }
    var output: GrassOutput;
    output.color = vec4<f32>(color, 1.0);
    //__AMBIENT_WRITE__
    //__EMISSIVE_WRITE__
    return output;
}
