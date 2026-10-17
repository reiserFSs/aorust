// Shares the native terrain texture, palette, fog and projection (shader.wgsl).
struct GrassCell { origin: vec4<f32>, heights: vec4<f32>, identity: vec4<u32>, normals: array<vec4<f32>, 4> }
struct GrassParams { wind_time: vec4<f32>, limits: vec4<f32>, benders: array<vec4<f32>, 16> }
@group(3) @binding(0) var<storage, read> grass_cells: array<GrassCell>;
@group(3) @binding(1) var<uniform> grass_params: GrassParams;
// 2..4 remain reserved for the optional shared shadow receiver.
@group(3) @binding(5) var grass_atlas: texture_2d_array<f32>;
@group(3) @binding(6) var grass_atlas_sampler: sampler;
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
    let c = grass_cells[instance / (GRASS_TUFTS * GRASS_BLADES)];
    let tuft = (instance / GRASS_BLADES) % GRASS_TUFTS;
    let blade = instance % GRASS_BLADES;
    let seed = grass_hash(c.identity.x ^ tuft);
    let blade_seed = grass_hash(seed ^ (blade * 0x9e3779b9u));
    var o: VOut;
    o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    // A deterministic tuft has 3-7 spreading blades, not one isolated needle.
    if blade >= 3u + grass_hash(seed ^ 0x571u) % 5u { return o; }
    let spread_angle = grass_unit(blade_seed) * 6.283185307;
    let spread = vec2<f32>(cos(spread_angle), sin(spread_angle)) * (grass_unit(grass_hash(blade_seed)) * 0.14);
    let uv = clamp(vec2<f32>(grass_unit(seed), grass_unit(grass_hash(seed))) + spread / c.origin.z, vec2<f32>(0.0125), vec2<f32>(0.9875));
    let triangle = select(select(2u, 1u, uv.x >= uv.y), select(2u, 1u, uv.x + uv.y <= 1.0), c.identity.z == 0u);
    if (c.identity.w & triangle) == 0u { return o; }
    let root = vec3<f32>(c.origin.x + uv.x * c.origin.z, grass_height(c, uv), c.origin.y - uv.y * c.origin.z);
    let radius = min(grass_params.wind_time.w, g.eye.w);
    let distance_to_eye = distance(root, g.eye.xyz);
    let density_fade = 1.0 - smoothstep(radius * 0.3, max(radius, 0.001), distance_to_eye);
    let fade = 1.0 - smoothstep(radius * 0.72, max(radius, 0.001), distance_to_eye);
    if fade <= 0.0 || grass_unit(grass_hash(seed ^ 0x993u)) >= grass_params.limits.x * density_fade { return o; }
    // Three paired cross-sections retain the six-vertex curved strip. The
    // alpha-tested atlas, rather than a pinched triangle, defines the silhouette.
    let along = f32(vertex / 2u) * 0.5;
    let side = select(-1.0, 1.0, (vertex & 1u) != 0u);
    let corner = vec2<f32>(side, along);
    let angle = grass_unit(grass_hash(seed ^ 0x42u)) * 6.283185307 + f32(blade) * 2.39996323;
    let direction = vec2<f32>(cos(angle), sin(angle));
    let blade_shape = grass_unit(grass_hash(blade_seed ^ 0x128u));
    let accent = grass_hash(seed ^ 0xaceu) % 128u;
    var species = grass_hash(seed ^ 0x128u) % 3u;
    // Only one blade in a rare tuft becomes a tiny flower or leafy weed.
    if blade == 0u && accent < 3u { species = 3u + accent % 2u; }
    let width = direction * (corner.x * (0.10 + grass_unit(grass_hash(blade_seed ^ 0x31u)) * 0.035) * fade);
    let height = 0.38 * (0.6 + grass_unit(grass_hash(blade_seed ^ 0x85u)) * 0.8) * fade;
    let wind = clamp(grass_params.wind_time.xy, vec2<f32>(-3.0), vec2<f32>(3.0)) * (sin(grass_params.wind_time.z * 1.7 + root.x * 0.37 + root.z * 0.29) * 0.06 * fade);
    var bend = wind + direction * ((0.03 + grass_unit(grass_hash(blade_seed ^ 0x29u)) * 0.11) * fade);
    bend += vec2<f32>(-direction.y, direction.x) * ((blade_shape - 0.5) * 0.12 * fade);
    // Current frame only: no recovery/trail state can retain a vanished actor.
    for (var index = 0u; index < min(u32(grass_params.limits.z), 16u); index += 1u) {
        let actor = grass_params.benders[index];
        if actor.w == 0.0 || abs(actor.y - root.y) > 1.5 { continue; }
        let delta = root.xz - actor.xz;
        let d2 = dot(delta, delta);
        if d2 >= 0.64 { continue; }
        let d = sqrt(d2);
        bend += delta / max(d, 0.001) * (max(1.0 - d / 0.8, 0.0) * 0.35 * fade);
    }
    if grass_params.limits.z > 0.0 {
        bend = clamp(bend, vec2<f32>(-0.45 * fade), vec2<f32>(0.45 * fade));
    }
    let curve = along * along;
    let p = root + vec3<f32>(width.x + bend.x * curve, height * along, width.y + bend.y * curve);
    o.clip = g.vp * vec4<f32>(p, 1.0);
    o.wpos = root;
    // Keep wpos at the native root for palette sampling; n carries the actual
    // blade position for the shared geometric-plane shadow receiver.
    o.n = p;
    o.uv = grass_uv(c.identity.y, uv);
    // Ground-root fog: raised tips cannot reduce native extinction.
    o.fog = max(vertex_fog(-(g.view * vec4<f32>(root, 1.0)).z), vertex_fog(-(g.view * vec4<f32>(p, 1.0)).z));
    o.color = vec4<f32>(c.origin.w, f32(species), grass_unit(grass_hash(blade_seed ^ 0x619u)), along);
    // Blade-local coordinates are independent of terrain UV/palette sampling.
    o.tint = vec3<f32>(side * 0.5 + 0.5, 1.0 - along, blade_shape);
    o.fade = 1.0 - smoothstep(radius * 0.65, max(radius, 0.001), distance_to_eye);
    // Evaluate the same four native terrain-corner Gouraud values, then
    // interpolate on the native diagonal. Blade tips never gain extra light.
    let offsets = array<vec2<f32>, 4>(vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0));
    let weights = grass_weights(c.identity.z, uv);
    o.light = vec3<f32>(0.0);
    o.spec = vec3<f32>(0.0);
    o.dlight = vec3<f32>(0.0);
    var root_fog = 0.0;
    for (var j = 0u; j < 4u; j += 1u) {
        let position = vec3<f32>(c.origin.x + offsets[j].x * c.origin.z, c.heights[j], c.origin.y - offsets[j].y * c.origin.z);
        let normal = normalize(c.normals[j].xyz);
        let local = vertex_lights(position, normal, 0.0)[0];
        let light = clamp(g.sun_g.rgb * max(dot(normal, g.sun_dir.xyz), 0.0) + local, vec3<f32>(0.0), vec3<f32>(1.0));
        let without_sun = clamp(local, vec3<f32>(0.0), vec3<f32>(1.0));
        // Simple ground adds/saturates ambient at each corner before interpolation.
        o.light += weights[j] * select(clamp(light + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), light, c.origin.w > 0.5);
        // Subtract only the sun's surviving share after native saturation.
        o.spec += weights[j] * select(clamp(without_sun + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), without_sun, c.origin.w > 0.5);
        o.dlight += weights[j] * (clamp(light + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) - light);
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
    let leaf = textureSample(grass_atlas, grass_atlas_sampler, i.tint.xy, i32(i.color.g + 0.5));
    let coverage = clamp((leaf.a - 0.5) / max(fwidth(leaf.a), 0.0001) + 0.5, 0.0, 1.0);
    let palette = ground_palette(i);
    // Evaluate derivatives before cutout; empty card texels skip shadow PCF.
    //__SHADOW_NORMAL__
    // Atlas root-tip color and folds only modulate the native terrain material.
    // Neither flowers nor backfaces emit extra light.
    let jitter = vec3<f32>(0.94 + i.color.b * 0.06, 0.97 + i.color.b * 0.03, 0.92 + i.color.b * 0.08);
    let tile = ground_tile(i) * leaf.rgb * jitter;
    if (g.fog.w <= 0.5 && leaf.a < 0.5) || (g.fog.w > 0.5 && coverage <= 0.0) { discard; }
    let bayer = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0, 3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    let pixel = vec2<u32>(i.clip.xy) & vec2<u32>(3u);
    if i.fade <= (bayer[pixel.x + pixel.y * 4u] + 0.5) / 16.0 { discard; }
    var light = select(i.light, clamp(i.light + palette, vec3<f32>(0.0), vec3<f32>(1.0)), i.color.r > 0.5);
    let without_sun = select(i.spec, clamp(i.spec + palette, vec3<f32>(0.0), vec3<f32>(1.0)), i.color.r > 0.5);
    //__SHADOW_RECEIVE__
    // Backlighting transmits more surviving direct sun, never exceeds the
    // terrain budget, and cannot brighten indirect light or bypass shadows.
    let backlit = max(dot(normalize(i.n - g.eye.xyz), g.sun_dir.xyz), 0.0);
    light = without_sun + max(light - without_sun, vec3<f32>(0.0)) * mix(0.9, 1.0, backlit * backlit);
    let fog = ground_fog(i);
    var color = mix(tile * light, g.fog_g.rgb, fog);
    if i.color.r > 1.5 {
        // Preserve native two-pass fog multiplication, not the fused approximation.
        color = mix(tile, g.fog_g.rgb, fog) * mix(light, vec3<f32>(1.0), fog);
    }
    var output: GrassOutput;
    output.color = vec4<f32>(color, select(1.0, coverage, g.fog.w > 0.5));
    //__AMBIENT_WRITE__
    //__AO_RECEIVE__
    //__EMISSIVE_WRITE__
    return output;
}
