// Enhanced ground grass. Shares the native terrain texture, palette, fog and projection (shader.wgsl).
//
// Every blade is generated here from its cell: no CPU blade records. Blade shapes, tufting, species, wind and gusts,
// the character push, the minimum drawn width and the distance fade follow ao-vk's procedural grass
// (https://github.com/wannabuh/ao-vk, MIT, rvk/grass.cpp + shaders/grass*.glsl; see THIRD_PARTY). Placement and colour
// come from the real terrain instead: each root's oriented green-texel mask, neighbouring bare ground,
// terrain tile colour, native palette and lighting.
struct GrassCell { origin: vec4<f32>, heights: vec4<f32>, identity: vec4<u32>, normals: array<vec4<f32>, 4> }
struct GrassParams {
    wind_time: vec4<f32>, // x, y: filtered bend direction (not normalized); z: bounded wind seconds; w: grass radius
    limits: vec4<f32>,    // x: blades per cell; y: pixels per unit at depth 1; z: least blade width (px); w: bender count
    wind: vec4<f32>,      // x: wind strength (1 = a fresh breeze); y: gust share
    features: vec4<f32>,  // tufts/dry/tall, species, path thinning, gust light bands
    lighting: vec4<f32>,  // sunlight, grass sun shadows, grass point shadows, reserved
    bands: array<vec4<f32>, 2>, // blades instanced per cell in eight distance bands
    benders: array<vec4<f32>, 16>,
    trail_origin: vec4<f32>, // world grid minimum x/z, cell size, enabled
    trails: array<vec4<f32>, 1024>, // lean x/z, foot height, compression
}
@group(3) @binding(0) var<storage, read> grass_cells: array<GrassCell>;
@group(3) @binding(1) var<uniform> grass_params: GrassParams;
// 2..4 remain reserved for the optional shared shadow receiver.
@group(3) @binding(5) var<storage, read> grass_visible: array<u32>;
// Tileable slow fields (grass_noise.rs): layer 0 clump, dry, tall, meadow; layer 1 lean x, lean z, lean strength.
@group(3) @binding(6) var grass_noise: texture_2d_array<f32>;
@group(3) @binding(7) var grass_noise_sampler: sampler;
// Which texels of each ground tile picture are green (grass_mask.rs); the cell names its tile's layer.
@group(3) @binding(8) var grass_mask: texture_2d_array<f32>;

const GRASS_HEIGHT: f32 = 0.4;
const GRASS_MAX_HEIGHT: f32 = 0.5;
const NOISE_PERIOD: f32 = 64.0;
const MEADOW_LUMA: f32 = 0.42;
const LUMA: vec3<f32> = vec3<f32>(0.299, 0.587, 0.114);

fn grass_hash(input: u32) -> u32 {
    var v = input ^ (input >> 16u); v *= 0x7feb352du;
    v ^= v >> 15u; v *= 0x846ca68bu; return v ^ (v >> 16u);
}
// Share of a cell's blades that survive at a distance: the CPU instances only the ranks that can (grass.rs).
fn thinning_keep(dist: f32) -> f32 {
    let d = max(dist - 12.0, 0.0) / 10.0;
    return 1.0 / (1.0 + d * d);
}
fn grass_unit(v: u32) -> f32 { return f32(v & 0x00ffffffu) / 16777216.0; }
// Byte n (0..3) of a hash as 0..1: one hash feeds four decisions.
fn hb(h: u32, n: u32) -> f32 { return f32((h >> (8u * n)) & 255u) / 255.0; }
fn lattice(ix: i32, iz: i32) -> u32 { return grass_hash(u32(ix) * 0x9e3779b1u + u32(iz) * 0x85ebca6bu + 0x27d4eb2fu); }
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

// Cross sections of each kind (0 blade, 1 broad blade, 2 seed stalk, 3 flower): how far up and how wide.
var<private> SECTION_T = array<vec4<f32>, 4>(vec4<f32>(0.0, 0.3333, 0.6667, 1.0), vec4<f32>(0.0, 0.3333, 0.6667, 1.0),
    vec4<f32>(0.0, 0.62, 0.8, 1.0), vec4<f32>(0.0, 0.86, 0.91, 1.0));
var<private> SECTION_W = array<vec4<f32>, 4>(vec4<f32>(1.0, 0.6667, 0.3333, 0.0), vec4<f32>(1.0, 0.92, 0.62, 0.0),
    vec4<f32>(0.4, 0.32, 1.0, 0.0), vec4<f32>(0.45, 0.4, 1.25, 0.95));
var<private> FLOWERS = array<vec3<f32>, 6>(vec3<f32>(0.84, 0.84, 0.78), vec3<f32>(0.86, 0.74, 0.24), vec3<f32>(0.56, 0.42, 0.72),
    vec3<f32>(0.44, 0.52, 0.80), vec3<f32>(0.82, 0.56, 0.64), vec3<f32>(0.84, 0.60, 0.30));

// Gentle traveling wind. Every temporal frequency is an integer harmonic of
// the same wrapped phase (grass.rs); nested gust modulation wraps with it.
fn grass_gust(aa: vec2<f32>, time: f32) -> f32 {
    let wind_phase = time * 0.05;
    let s = aa.x * 0.39 - wind_phase * 14.0 + 1.6 * sin(aa.y * 0.07 + wind_phase) + 0.9 * sin(aa.y * 0.21 - aa.x * 0.05);
    var g = 0.5 + 0.5 * sin(s);
    let strength = 0.55 + 0.45 * sin(aa.y * 0.043 + aa.x * 0.021 - wind_phase * 2.0);
    g *= g;
    return g * g * strength;
}
const GUST_MEAN: f32 = 0.15;
fn grass_sway(aa: vec2<f32>, time: f32, phase: f32, gust: f32) -> f32 {
    let wind_phase = time * 0.05;
    let rows = sin(aa.x * 0.55 - wind_phase * 12.0 + 0.7 * sin(phase));
    let cross = sin(aa.y * 0.23 + aa.x * 0.31 - wind_phase * 8.0 + 1.3);
    let flutter = sin(wind_phase * 24.0 + phase * 1.7);
    return 0.45 + 0.35 * rows + 0.15 * cross + (0.12 + 0.35 * gust) * flutter;
}
// A sideways bend b of a point `above` units up its blade, keeping the blade's length.
fn bend_keeping_length(b_in: vec3<f32>, above: f32) -> vec3<f32> {
    var b = b_in;
    let l = length(b); let most = 0.9 * above;
    if l > most { b *= most / l; }
    return vec3<f32>(b.x, -(above - sqrt(max(above * above - dot(b, b), 0.0))), b.z);
}

// World-anchored samples, not a camera-scrolling texture. CPU ownership revokes
// every sample from a source as soon as final-camera admission is lost.
fn grass_trail(pos: vec3<f32>) -> vec2<f32> {
    let coordinate = pos.xz / grass_params.trail_origin.z - vec2<f32>(0.5);
    let base = vec2<i32>(floor(coordinate));
    let fraction = fract(coordinate);
    let origin = vec2<i32>(grass_params.trail_origin.xy);
    var bend = vec2<f32>(0.0);
    for (var z = 0; z < 2; z += 1) {
        for (var x = 0; x < 2; x += 1) {
            let cell = base + vec2<i32>(x, z);
            if any(cell < origin) || any(cell >= origin + vec2<i32>(32)) { continue; }
            let wrapped = ((cell % vec2<i32>(32)) + vec2<i32>(32)) % vec2<i32>(32);
            let sample = grass_params.trails[u32(wrapped.y * 32 + wrapped.x)];
            if abs(sample.z - pos.y) > 1.5 { continue; }
            let weight = select(1.0 - fraction.x, fraction.x, x == 1) * select(1.0 - fraction.y, fraction.y, z == 1);
            bend += sample.xy * sample.w * weight;
        }
    }
    return bend;
}
// Instant admitted-character push plus independently enabled recovering trails.
fn grass_push(pos: vec3<f32>, h: f32, plant_height: f32) -> vec3<f32> {
    var push = vec2<f32>(0.0);
    for (var index = 0u; index < min(u32(grass_params.limits.w), 16u); index += 1u) {
        let actor = grass_params.benders[index];
        if actor.w == 0.0 || abs(actor.y - pos.y) > 1.5 { continue; }
        let d = pos.xz - actor.xz;
        let dist = length(d);
        if dist >= 0.8 { continue; }
        let near = 1.0 - smoothstep(0.12, 0.8, dist);
        push += select(vec2<f32>(0.7071), d / dist, dist > 1e-3) * near;
    }
    if grass_params.trail_origin.w > 0.0 { push += grass_trail(pos); }
    let len = length(push);
    if len <= 1e-4 { return vec3<f32>(0.0); }
    let above = h * plant_height;
    let lean = min(min(0.45, 0.7 * plant_height) * min(len, 1.0) * h * sqrt(h), 0.97 * above);
    let dir = push / len;
    return vec3<f32>(dir.x * lean, -(above - sqrt(max(above * above - lean * lean, 0.0))), dir.y * lean);
}

struct GOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) root: vec3<f32>,
    @location(1) @interpolate(flat) uv: vec2<f32>,
    @location(2) light: vec3<f32>,
    @location(3) dlight: vec3<f32>,
    @location(4) spec: vec3<f32>,
    @location(5) shape: vec4<f32>,    // x: up the blade; y: across it (-1..1); z: gradient x canopy; w: gust light
    @location(6) @interpolate(flat) hue: vec4<f32>,    // rgb: per-blade colour jitter; w: dry-patch share
    @location(7) @interpolate(flat) head: vec4<f32>,   // rgb: seed head / flower colour; w: 1 on a head
    @location(8) mixes: vec4<f32>,    // x: root goes to ground; y: whole blade goes to ground; z: light through it; w: brightness scale
    @location(9) fog: f32,
    @location(10) @interpolate(flat) path: u32,
    @location(11) @interpolate(flat) normal: vec4<f32>, // xyz: normal; w: same baked sky openness as terrain
    @location(12) @interpolate(flat) coverage: vec2<f32>, // field coverage, then geometry-LOD AO share
    @location(13) sunlight: vec2<f32>, // rounded-edge delta, physical blade sheen (before sun visibility)
    @location(14) world: vec3<f32>, // shared visible/caster geometry, including wind and admitted pushes
}

// Raw light visibility, before the artistic shadow-strength floor used by beauty.
var<private> grass_direct_visibility: f32 = 1.0;

fn grass_vertex(vertex: u32, instance: u32) -> GOut {
    var o: GOut;
    o.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    // The instance index carries its distance band in the top bits, then runs over the band's visible cells' blades.
    let band = instance >> 28u;
    let within = instance & 0x0fffffffu;
    let band_stride = u32(grass_params.bands[band / 4u][band % 4u]);
    let c = grass_cells[grass_visible[band * GRASS_MAX_CELLS + within / band_stride]];
    let k = within % band_stride;
    let stride = u32(grass_params.limits.x);
    let size = c.origin.z;
    let seed = c.identity.x;
    let radius = min(grass_params.wind_time.w, g.eye.w);
    let hk = grass_hash(seed ^ (k * 0x9e3779b9u + 0x68bc21ebu));
    let hv = grass_hash(hk ^ 0x5bf03635u);
    let hw = grass_hash(hv ^ 0x1b873593u);
    let darkness = 1.0 - smoothstep(0.0, 0.25, g.ambient_g.w);
    // Low-discrepancy scatter (R2): any prefix of the cell's blades is evenly spread, so the distance thinning
    // below removes blades without clumps or bare gaps.
    let uv = fract(vec2<f32>(grass_unit(seed), grass_unit(grass_hash(seed ^ 0xabcdu))) + f32(k + 1u) * vec2<f32>(0.7548776662, 0.5698402910));
    let triangle = select(select(2u, 1u, uv.x >= uv.y), select(2u, 1u, uv.x + uv.y <= 1.0), c.identity.z == 0u);
    if (c.identity.w & triangle) == 0u { return o; }
    var root = vec3<f32>(c.origin.x + uv.x * size, grass_height(c, uv) - 0.02, c.origin.y - uv.y * size);
    let eye = g.eye.xyz;
    let dist = distance(root.xz, eye.xz);
    // Stable rank prefixes form wider, sparse tufts in the distance. Rank transitions shrink smoothly;
    // the outer field also dithers coverage, rather than cutting a ring into the terrain.
    let rank = (f32(k) + 0.5) / f32(stride);
    let keep = thinning_keep(dist);
    let by_rank = smoothstep(0.0, min(0.08, keep * 0.25), max(keep - rank, 0.0));
    let by_range = 1.0 - smoothstep(0.7 * radius, radius, dist);
    let fade = by_rank * by_range;
    if fade <= 0.002 { return o; }
    o.coverage = vec2<f32>(by_range, 1.0);
    let widen = min(inverseSqrt(keep), 6.0);

    // The terrain under the root: slope, and distance from neighbouring path, soil or rock.
    let w4 = grass_weights(c.identity.z, uv);
    var ground_n = vec3<f32>(0.0);
    for (var j = 0u; j < 4u; j += 1u) { ground_n += w4[j] * c.normals[j].xyz; }
    ground_n = normalize(ground_n);
    // Green hills remain grassy; only near-vertical cliff faces thin out (60 to 75 degrees).
    if hb(hw, 0u) > (ground_n.y - 0.2588) / 0.2412 { return o; }
    // Grass grows where the ground's own picture is green, texel by texel: a transition tile's soil stays bare.
    let tile_uv = grass_uv(c.identity.y & 255u, uv);
    let layer = i32(c.identity.y >> 8u);
    // The root texel mask is always authoritative, independently of optional path thinning.
    let m0 = textureSampleLevel(grass_mask, grass_noise_sampler, tile_uv, layer, 0.0).r;
    if m0 < 0.15 { return o; }
    var edge = 1.0;
    if grass_params.features.z > 0.5 {
        let reach = 0.5 / size;
        let around = (m0 + textureSampleLevel(grass_mask, grass_noise_sampler, clamp(tile_uv + vec2<f32>(reach, 0.0), vec2<f32>(0.0), vec2<f32>(1.0)), layer, 0.0).r
            + textureSampleLevel(grass_mask, grass_noise_sampler, clamp(tile_uv - vec2<f32>(reach, 0.0), vec2<f32>(0.0), vec2<f32>(1.0)), layer, 0.0).r
            + textureSampleLevel(grass_mask, grass_noise_sampler, clamp(tile_uv + vec2<f32>(0.0, reach), vec2<f32>(0.0), vec2<f32>(1.0)), layer, 0.0).r
            + textureSampleLevel(grass_mask, grass_noise_sampler, clamp(tile_uv - vec2<f32>(0.0, reach), vec2<f32>(0.0), vec2<f32>(1.0)), layer, 0.0).r) * 0.2;
        let out_mask = c.identity.w >> 8u;
        var edge_distance = 1.0e3;
        let to_edge = vec4<f32>(uv.x, 1.0 - uv.x, uv.y, 1.0 - uv.y) * size;
        for (var s = 0u; s < 4u; s += 1u) { if ((out_mask >> s) & 1u) != 0u { edge_distance = min(edge_distance, to_edge[s]); } }
        for (var s = 0u; s < 4u; s += 1u) {
            if ((out_mask >> (4u + s)) & 1u) != 0u {
                edge_distance = min(edge_distance, length(vec2<f32>(select(to_edge.x, to_edge.y, (s & 1u) != 0u), select(to_edge.z, to_edge.w, (s & 2u) != 0u))));
            }
        }
        edge = min(clamp(edge_distance / 1.2, 0.0, 1.0), smoothstep(0.1, 0.6, around));
        if hb(hw, 1u) > smoothstep(0.15, 0.45, m0) * (0.3 + 0.7 * edge) { return o; }
    }

    // The slow fields at the root (grass_noise.rs): two fetches.
    let noise_uv = root.xz / NOISE_PERIOD;
    let field0 = textureSampleLevel(grass_noise, grass_noise_sampler, noise_uv, 0, 0.0);
    let field1 = textureSampleLevel(grass_noise, grass_noise_sampler, noise_uv, 1, 0.0);
    let tufts = grass_params.features.x;
    let clump = field0.x * tufts;
    var out_dir = vec2<f32>(0.0);
    var vigour = 0.5;
    var rim = 1.0;
    if tufts > 0.5 {
        if hb(hv, 0u) > 0.72 + 0.28 * clump { return o; }
        // The nearest of four jittered lattice centres: thinning between tufts and fan-out within them.
        let tuft_cell = 0.5;
        let base = vec2<i32>(floor(root.xz / tuft_cell - 0.5));
        var nearest = 1.0e9;
        for (var dj = 0; dj <= 1; dj += 1) {
            for (var di = 0; di <= 1; di += 1) {
                let cell = base + vec2<i32>(di, dj);
                let h = lattice(cell.x, cell.y);
                let centre = (vec2<f32>(cell) + 0.15 + 0.7 * vec2<f32>(hb(h, 0u), hb(h, 1u))) * tuft_cell;
                let d = root.xz - centre;
                let d2 = dot(d, d);
                if d2 < nearest { nearest = d2; out_dir = d; vigour = hb(h, 2u); }
            }
        }
        let tuft_d = sqrt(nearest);
        rim = min(tuft_d / (0.75 * tuft_cell), 1.0);
        out_dir = select(vec2<f32>(0.0), out_dir / tuft_d, tuft_d > 1e-4);
        if hb(hv, 1u) > 1.0 - 0.45 * rim * rim { return o; }
    }

    // What it is: mostly blades; a few broad blades, seed stalks and flowers (in patches of one colour).
    var kind = 0u;
    let pick = hb(hw, 2u);
    if grass_params.features.y > 0.5 && pick < 0.124 {
        let flower = 0.024 * clamp((field0.w - 0.45) / 0.3, 0.0, 1.0);
        if pick < flower { kind = 3u; }
        else if pick < flower + 0.04 { kind = 2u; }
        else if pick < flower + 0.1 { kind = 1u; }
    }
    let v0 = hb(hk, 0u); let v2 = hb(hk, 3u); let v4 = hb(hv, 2u);
    let yaw = f32((hk >> 8u) & 0xffffu) * (6.2831853 / 65536.0);
    let tall = 1.0 + 0.5 * (field0.z - 0.5) * tufts;
    var height = GRASS_HEIGHT * (0.45 + 1.2 * v0) * tall * (1.0 + 0.35 * (1.0 - rim) * vigour) * (0.65 + 0.35 * edge);
    var half_width = 0.5 * (0.03 + 0.03 * v2) * 0.9;
    var droop = 0.06 + 0.18 * v4;
    switch kind {
        case 1u: { height *= 0.7 + 0.2 * v4; half_width *= 1.9; droop = 0.2 + 0.25 * v4; }
        case 2u: { height *= 1.45 + 0.4 * v4; half_width *= 0.75; droop = 0.12 + 0.2 * v4; }
        case 3u: { height *= 0.95 + 0.35 * v4; half_width *= 1.1; droop = 0.03; }
        default: {}
    }
    // Blades never hide a target: tall ones are compressed towards GRASS_MAX_HEIGHT.
    height = select(height, 0.4 + (height - 0.4) * 0.3, height > 0.4);
    height = min(height, GRASS_MAX_HEIGHT);

    // Use the same first triangle in every band; collapse the shoulder onto its tip before dropping the rest.
    let step = vertex >> 1u;
    let section = select(0u, step + 1u, step > 0u);
    let t0 = SECTION_T[kind][section];
    // Finish collapsing the shoulder before band 2 (~29 m nearest cell edge).
    // Evaluating all geometry and shading at the tip also makes the remaining triangles degenerate.
    let simple_tuft = smoothstep(22.0, 27.0, dist);
    let t = select(t0, mix(t0, 1.0, simple_tuft), section == 2u);
    // Half-resolution AO cannot resolve root/tip tufts: fade their receiving share with the geometry.
    o.coverage.y = 1.0 - simple_tuft;
    let profile = mix(SECTION_W[kind][section], 1.0 - t, simple_tuft);
    let side = select(-1.0, 1.0, (vertex & 1u) != 0u);
    let is_head = kind >= 2u && section >= 2u;

    // Lean: along the ground's slope, a low-frequency field (a patch leans together), scatter, out from the tuft.
    let rr = vec3<f32>(cos(yaw), 0.0, sin(yaw));
    let lean_field = field1.xy - 0.5;
    let lean_dir = lean_field / (length(lean_field) + 1e-6) * (0.05 + 0.22 * field1.z);
    let scatter = (v2 - 0.5) * 0.18;
    let fan = 0.3 * min(rim * 1.5, 1.0) * tufts;
    // Growth tends upwards on steep hills, rather than turning a green cliff into horizontal fur.
    let growth_n = normalize(mix(ground_n, vec3<f32>(0.0, 1.0, 0.0), 1.0 - smoothstep(0.5, 0.85, ground_n.y)));
    var up = normalize(vec3<f32>(growth_n.x + lean_dir.x + rr.x * scatter + out_dir.x * fan, growth_n.y,
        growth_n.z + lean_dir.y + rr.z * scatter + out_dir.y * fan));
    // Seen from above an upright field is mostly the ground between blades: the steeper the view, the further they
    // lean (each its own way) and the wider they are drawn.
    let to_eye = eye - root;
    let steep = smoothstep(0.4, 0.95, to_eye.y / max(length(to_eye), 1e-3));
    let tilt = 0.45 * steep * edge * edge;
    let up_view = normalize(up * cos(tilt) + rr * sin(tilt));
    let axis0 = root + up_view * (height * t) + rr * (droop * height * mix(t * t, t, simple_tuft));
    let pos = root + (axis0 - root) * fade;

    // Wind: waves running through the field (the upper blade bending most) with gusts on top.
    let wd = grass_params.wind_time.xy;
    let time = grass_params.wind_time.z;
    // Never rotate world position with weather: even a tiny direction change
    // becomes a large phase jump at distant world coordinates (e.g. ICC).
    let aa = vec2<f32>(dot(root.xz, vec2<f32>(0.8, 0.6)), dot(root.xz, vec2<f32>(-0.6, 0.8)));
    let gust = grass_gust(aa, time) * grass_params.wind.y;
    let strength = grass_params.wind.x;
    let lean_level = select(1.0 + 1.3 * (1.0 - exp(-(strength - 1.0) / 1.2)), strength, strength <= 1.0);
    let phase = root.x * 0.3 + root.z * 0.25 + yaw;
    let hh = height * fade * lean_level * mix(t * t, t, simple_tuft);
    let above = height * t * fade;
    let wind_dir3 = vec3<f32>(wd.x, 0.0, wd.y);
    let wind_bend = bend_keeping_length(wind_dir3 * (0.16 * hh * grass_sway(aa, time, phase, gust) + 0.7 * hh * gust), above);
    let axis = pos + wind_bend + grass_push(pos, t, height);

    // Billboard about the blade's own axis: never edge-on.
    let vh = eye.xz - axis.xz;
    let vl = length(vh);
    let horiz = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(-vh.y, 0.0, vh.x) / vl, vl > 1e-3);
    let view_dir = normalize(axis - eye);
    let right = normalize(cross(normalize(up_view + rr * droop), view_dir) + 0.1 * horiz);
    // A blade thinner than a pixel flickers: draw it at least `limits.z` pixels wide, its colour going towards the
    // ground's by as much as it was widened.
    let root_width = 2.0 * half_width * widen * max(fade, 0.05);
    let pixels = root_width * grass_params.limits.y / max((g.vp * vec4<f32>(root, 1.0)).w, 1e-3);
    let widen_px = max(grass_params.limits.z / max(pixels, 1e-4), 1.0);
    let p = axis + right * (side * half_width * profile * fade * widen * (1.0 + 0.3 * steep) * widen_px);
    o.clip = g.vp * vec4<f32>(p, 1.0);
    o.world = p;

    // Colour inputs; the terrain sample itself is taken per fragment at the root.
    let dry = clamp((field0.y - 0.56) / 0.16, 0.0, 1.0) * 0.55 * (0.6 + 0.4 * hb(hv, 3u)) * tufts;
    let hj = grass_hash(hw);
    o.hue = vec4<f32>(1.0 + 0.12 * (vec3<f32>(hb(hj, 0u), hb(hj, 1u), hb(hj, 2u)) - 0.5), dry);
    var head = vec3<f32>(0.0);
    if kind == 2u { head = vec3<f32>(0.62, 0.56, 0.30) * (1.0 + 0.2 * v2); }
    if kind == 3u {
        var colour = lattice(i32(floor(root.x * 0.08)) + 911, i32(floor(root.z * 0.08)) - 37) % 6u;
        if hb(hj, 3u) < 0.15 { colour = hj % 6u; }
        head = FLOWERS[colour] * (0.85 + 0.15 * v2);
    }
    // Fade the flat head-colour flag with the geometric morph.
    o.head = vec4<f32>(head, select(0.0, 1.0 - simple_tuft, is_head));
    let canopy = clamp(0.55 * (1.0 - rim) * (0.5 + vigour) + 0.6 * clump - 0.05, 0.0, 1.0);
    let grad = mix(0.9, 1.2, t);
    let ao = (1.0 - 0.55 * canopy * (1.0 - t) * (1.0 - t)) / (1.0 - 0.275 * canopy);
    // Sun on the blade itself: the lit and shaded edges of a rounded blade (zero-mean), light through its thin upper
    // part when the sun is behind it, a denser canopy keeping a low sun off the lower blades.
    let sun = normalize(g.sun_dir.xyz + vec3<f32>(0.0, 1e-6, 0.0));
    let sun_on = clamp(dot(g.sun_g.rgb, LUMA) * 2.5, 0.0, 1.0) * smoothstep(0.0, 0.1, sun.y);
    let canopy_shade = mix(1.0, smoothstep(0.0, 0.75, t), canopy * 0.7 * (1.0 - sun.y));
    let sunlit = sun_on * canopy_shade * grass_params.lighting.x;
    let face = vec3<f32>(vh.x, 0.0, vh.y) / max(vl, 1e-3);
    let behind = max(-dot(face, sun), 0.0);
    let towards = max(dot(view_dir, sun), 0.0);
    let through = (0.5 * behind + 0.5 * towards * towards * towards) * pow(t, 1.3);
    let tangent = normalize(up_view + rr * (2.0 * droop * t));
    let half_vector = sun - view_dir;
    let half_dir = half_vector / max(length(half_vector), 1e-6);
    let th = dot(tangent, half_dir);
    let sheen = pow(max(1.0 - th * th, 0.0), 20.0) * t;
    o.sunlight = vec2<f32>(sunlit * 0.3 * side * dot(right, sun), 0.07 * sheen * sunlit);
    o.shape = vec4<f32>(t, side, grad * ao,
        1.0 + grass_params.features.w * 0.45 * (gust - GUST_MEAN * grass_params.wind.y) * t);
    o.mixes = vec4<f32>(0.55 * (1.0 - smoothstep(0.0, 0.4, t)),
        max(0.95 * smoothstep(24.0, 80.0, dist), min(0.7 * smoothstep(0.55 * radius, radius, dist) + 0.5 * (1.0 - 1.0 / widen_px), 0.85)),
        0.4 * through * sunlit, 0.94 + 0.12 * v0);

    // Preserve the daylight appearance; night lamps use the native per-corner
    // saturation and Gouraud order rather than relighting the root independently.
    o.root = root;
    o.uv = tile_uv;
    o.normal = vec4<f32>(ground_n, c.normals[0].w);
    o.path = u32(c.origin.w + 0.5);
    let simple = c.origin.w < 0.5;
    if darkness < 1.0 {
        let local = vertex_lights(root, ground_n, 0.0)[0];
        let without_sun = clamp(local, vec3<f32>(0.0), vec3<f32>(1.0));
        o.spec = select(without_sun, clamp(without_sun + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), simple);
        if g.moon_g.w > 0.0 { o.spec = mix(o.spec, without_sun, g.moon_g.w); }
        for (var j = 0u; j < 4u; j += 1u) {
            let light = clamp(g.sun_g.rgb * max(dot(c.normals[j].xyz, g.sun_dir.xyz), 0.0) + local, vec3<f32>(0.0), vec3<f32>(1.0));
            var corner_light = select(light, clamp(light + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), simple);
            var corner_ambient = clamp(light + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) - light;
            if g.moon_g.w > 0.0 {
                corner_light = mix(corner_light, without_sun, g.moon_g.w);
                corner_ambient *= 1.0 - g.moon_g.w;
            }
            o.light += w4[j] * corner_light;
            o.dlight += w4[j] * corner_ambient;
        }
    }
    if darkness > 0.0 {
        // Native lighting saturates at each terrain vertex before Gouraud interpolation.
        // Sampling lamps once at the blade root is not equivalent near a lamp/grid edge.
        var native_light = vec3<f32>(0.0);
        var native_dlight = vec3<f32>(0.0);
        var native_spec = vec3<f32>(0.0);
        for (var j = 0u; j < 4u; j += 1u) {
            let corner = vec3<f32>(c.origin.x + f32(j & 1u) * size, c.heights[j], c.origin.y - f32(j >> 1u) * size);
            let n = normalize(c.normals[j].xyz);
            let lamps = vertex_lights(corner, n, 0.0)[0];
            let diffuse = clamp(g.sun_g.rgb * max(dot(n, g.sun_dir.xyz), 0.0) + lamps, vec3<f32>(0.0), vec3<f32>(1.0));
            let ambient = clamp(diffuse + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
            var corner_light = select(diffuse, ambient, simple);
            var corner_ambient = ambient - diffuse;
            var corner_spec = select(clamp(lamps, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(lamps + g.ambient_g.rgb, vec3<f32>(0.0), vec3<f32>(1.0)), simple);
            if g.moon_g.w > 0.0 {
                // Dark retains exactly the native saturated local energy, before
                // Gouraud interpolation; it never subtracts a lamp a second time.
                let retained = clamp(lamps, vec3<f32>(0.0), vec3<f32>(1.0));
                corner_light = mix(corner_light, retained, g.moon_g.w);
                corner_spec = mix(corner_spec, retained, g.moon_g.w);
                corner_ambient *= 1.0 - g.moon_g.w;
            }
            native_light += w4[j] * corner_light;
            native_dlight += w4[j] * corner_ambient;
            native_spec += w4[j] * corner_spec;
        }
        o.light = mix(o.light, native_light, darkness);
        o.dlight = mix(o.dlight, native_dlight, darkness);
        o.spec = mix(o.spec, native_spec, darkness);
    }
    // Ground-root fog: raised tips cannot reduce native extinction. (Fog is linear in view depth, so the root's
    // equals the ground's own interpolation.)
    let root_fog = vertex_fog(-(g.view * vec4<f32>(root, 1.0)).z);
    o.fog = max(vertex_fog(-(g.view * vec4<f32>(p, 1.0)).z), root_fog);
    return o;
}

@vertex
fn vs_grass(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> GOut {
    return grass_vertex(vertex, instance);
}

struct GrassOutput {
    @location(0) color: vec4<f32>,
    //__AMBIENT_FIELD__
    //__EMISSIVE_FIELD__
    //__SUN_FIELD__
}
@fragment
fn fs_grass(i: GOut) -> GrassOutput {
    // Use the native mip chain once: full-detail roots near the camera, the composite's 8x8 tile mip far away.
    let coarse_level = max(log2(f32(textureDimensions(tex).x) / 8.0), 0.0) * min(i.mixes.y / 0.95, 1.0);
    var ground: vec3<f32>;
    if mat.flags.x > 0.5 {
        ground = textureSampleLevel(tex, samp_clamp, i.uv, coarse_level).rgb;
    } else {
        ground = textureSampleLevel(tex, samp, i.uv, coarse_level).rgb;
    }
    let palette = sample_clamped(holo0, vec2<f32>(i.root.x, -i.root.z) * mat.wave.xz).rgb * (1.0 - g.moon_g.w);
    var light = i.light;
    var visibility = 1.0;
    var moon_visibility = 1.0;
    //__SHADOW_VISIBILITY__
    // Match native ground: remove the pre-palette direct sun, THEN add/clamp the baked palette.
    let sun_share = max(i.light - i.spec, vec3<f32>(0.0));
    light = max(light - sun_share * (1.0 - visibility), vec3<f32>(0.0));
    // Root openness is the authored terrain lattice already carried by this cell.
    // Add moon once, before the native palette clamp and both native fog equations.
    if g.moon_direction.w > 0.0 {
        light += g.moon_g.rgb * max(dot(i.normal.xyz, g.moon_direction.xyz), 0.0)
            * clamp(i.normal.w, 0.0, 1.0) * moon_visibility;
    }
    light = select(light, clamp(light + palette, vec3<f32>(0.0), vec3<f32>(1.0)), i.path > 0u);
    // Blade colour: the meadow's green at the ground texel's brightness, the root growing out of the texel itself,
    // each over its blade's area averaging 1 so the field is as bright as the ground it stands on.
    let green = mix(ground, vec3<f32>(0.20, 0.44, 0.10), 0.8);
    var hue = (vec3<f32>(0.44, 0.62, 0.30) * 0.45 + green * 0.55) * vec3<f32>(0.88, 0.92, 0.82);
    hue = mix(hue, vec3<f32>(0.62, 0.56, 0.30) * 0.6, i.hue.w) * i.hue.rgb;
    let ground_luma = dot(ground, LUMA);
    // Only surviving direct daylight can lift the dull substrate towards meadow luminance.
    // Ambient/local lights alone must not brighten night or covered ground.
    let daylight = g.ambient_g.w * clamp(dot(sun_share, LUMA) * 2.5, 0.0, 1.0) * grass_direct_visibility * smoothstep(0.0, 0.1, g.sun_dir.y);
    let lit = smoothstep(0.2, 0.5, dot(light, LUMA)) * daylight * (1.0 - i.fog);
    let target_luma = mix(ground_luma, max(ground_luma, MEADOW_LUMA), lit);
    let body = hue * (target_luma / max(dot(hue, LUMA), 1e-3)) * i.mixes.w;
    let vein = 1.04 - 0.14 * exp(-i.shape.y * i.shape.y * 12.0);
    var albedo = mix(mix(body, ground, i.mixes.x) * i.shape.z, i.head.rgb, i.head.w) * vein;
    // Raw visibility (not the beauty shadow floor) gates every extra direct-sun term.
    let direct = grass_direct_visibility * (1.0 - g.moon_g.w);
    let sunlight_extra = (albedo * light * (i.sunlight.x * i.shape.w)
        + albedo * vec3<f32>(1.2, 1.25, 0.7) * g.sun_g.rgb * i.mixes.z
        + g.sun_g.rgb * i.sunlight.y) * direct;
    let gust_extra = albedo * light * (i.shape.w - 1.0);
    var colour = albedo * light * i.shape.w + sunlight_extra;
    colour = mix(colour, ground * light, i.mixes.y);
    albedo = mix(albedo, ground, i.mixes.y);
    let fog = i.fog;
    var color = mix(colour, g.fog_g.rgb, fog);
    if i.path > 1u {
        // Preserve native two-pass fog multiplication, not the fused approximation.
        color = mix(albedo, g.fog_g.rgb, fog) * mix(light, vec3<f32>(1.0), fog)
            + (sunlight_extra + gust_extra) * ((1.0 - fog) * (1.0 - i.mixes.y));
    }
    var output: GrassOutput;
    // Hardware alpha-to-coverage dithers the outer field without fragment discard.
    // At one sample the geometric shrink remains the continuous fade.
    output.color = vec4<f32>(color, i.coverage.x);
    //__AMBIENT_WRITE__
    //__AO_RECEIVE__
    //__EMISSIVE_WRITE__
    //__SUN_WRITE__
    return output;
}
