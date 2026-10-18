// ao-vk gi.frag/gi_blur.frag (MIT): horizon increments gather diffuse radiance.
// Fixed spatial phase; all geometry comes from this frame's admitted primary depth.
// Local-bounce work fades out independently of native fog or sight distance.
const FADE_NEAR = 50.0;
const FADE_FAR = 80.0;
struct Params {
    inverse_vp: mat4x4<f32>,
    vp: mat4x4<f32>,
    eye: vec4<f32>,
    control: vec4<f32>,
}
struct Room { min: vec4<f32>, max: vec4<f32> }
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var depth: texture_depth_2d;
@group(0) @binding(2) var diffuse: texture_2d<f32>;
@group(0) @binding(3) var albedo: texture_2d<f32>;
@group(0) @binding(5) var incoming: texture_2d<f32>;
@group(0) @binding(7) var<storage, read> rooms: array<Room>;
@group(0) @binding(8) var donor_cache: texture_2d<u32>;
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}
fn size() -> vec2<i32> { return vec2<i32>(textureDimensions(depth)); }
fn bounded(pixel: vec2<i32>) -> vec2<i32> { return clamp(pixel, vec2<i32>(0), size() - 1); }
fn depth_at(pixel: vec2<i32>) -> f32 { return textureLoad(depth, bounded(pixel), 0); }
fn world_from_depth(pixel: vec2<i32>, z: f32) -> vec3<f32> {
    let uv = (vec2<f32>(bounded(pixel)) + 0.5) / vec2<f32>(size());
    let p = params.inverse_vp * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), z, 1.0);
    return p.xyz / p.w;
}
fn world(pixel: vec2<i32>) -> vec3<f32> { return world_from_depth(pixel, depth_at(pixel)); }
// One fixed, actual full-resolution sample represents each quarter texel.
// Keeping this mapping identical in all stages avoids filtering across silhouettes.
fn full_pixel(quarter: vec2<i32>) -> vec2<i32> { return bounded(quarter * 4 + vec2<i32>(2)); }
fn normal(pixel: vec2<i32>, p: vec3<f32>) -> vec3<f32> {
    let l = p - world(pixel - vec2<i32>(1, 0));
    let r = world(pixel + vec2<i32>(1, 0)) - p;
    let u = p - world(pixel - vec2<i32>(0, 1));
    let d = world(pixel + vec2<i32>(0, 1)) - p;
    let dx = select(r, l, dot(l, l) < dot(r, r));
    let dy = select(d, u, dot(u, u) < dot(d, d));
    let c = cross(dy, dx);
    let n = c * inverseSqrt(max(dot(c, c), 1e-20));
    return select(n, -n, dot(n, -p) < 0.0);
}
fn eligible(pixel: vec2<i32>) -> bool {
    let material = textureLoad(albedo, pixel, 0);
    return depth_at(pixel) < 1.0 && material.a > 0.0 && any(material.rgb > vec3<f32>(0.0));
}
fn room_at(relative: vec3<f32>) -> u32 {
    let p = relative + params.eye.xyz;
    var i = 0u;
    var priority = 16777216.0;
    var room = 0u;
    loop {
        if i >= u32(params.control.z) { break; }
        let node = rooms[i];
        let inside = all(p >= node.min.xyz) && all(p <= node.max.xyz);
        if node.min.w < 0.0 {
            // Internal bounds miss: skip the entire preorder subtree.
            if !inside { i = u32(node.max.w); } else { i++; }
        } else {
            if inside && node.max.w < priority {
                priority = node.max.w;
                room = u32(node.min.w);
                if priority == 0.0 { break; }
            }
            i++;
        }
    }
    return room;
}
// Resolved colour averages MSAA coverage while depth selects one surface.
// A donor footprint must describe one geometric plane, even within one room.
struct Donor { n: vec3<f32>, admitted: bool }
fn source_eligible(pixel: vec2<i32>, p: vec3<f32>) -> Donor {
    let rejected = Donor(vec3<f32>(0.0), false);
    if !eligible(pixel) { return rejected; }
    let indoors = params.control.w > 0.5;
    var room = 0u;
    if indoors {
        room = room_at(p);
        if room == 0u { return rejected; }
    }
    if any(pixel <= vec2<i32>(0)) || any(pixel >= size() - 1) { return rejected; }
    let left_pixel = pixel - vec2<i32>(1, 0);
    let right_pixel = pixel + vec2<i32>(1, 0);
    let up_pixel = pixel - vec2<i32>(0, 1);
    let down_pixel = pixel + vec2<i32>(0, 1);
    if depth_at(left_pixel) >= 1.0 { return rejected; }
    let left = world(left_pixel);
    if indoors && room_at(left) != room { return rejected; }
    if depth_at(right_pixel) >= 1.0 { return rejected; }
    let right = world(right_pixel);
    if indoors && room_at(right) != room { return rejected; }
    if depth_at(up_pixel) >= 1.0 { return rejected; }
    let up = world(up_pixel);
    if indoors && room_at(up) != room { return rejected; }
    if depth_at(down_pixel) >= 1.0 { return rejected; }
    let down = world(down_pixel);
    if indoors && room_at(down) != room { return rejected; }
    let l = p - left;
    let r = right - p;
    let u = p - up;
    let d = down - p;
    let dx = select(r, l, dot(l, l) < dot(r, r));
    let dy = select(d, u, dot(u, u) < dot(d, d));
    let c = cross(dy, dx);
    var n = c * inverseSqrt(max(dot(c, c), 1e-20));
    if dot(n, n) < 0.5 { return rejected; }
    n = select(n, -n, dot(n, -p) < 0.0);
    // Millimetre-scale allowance plus a small distance-dependent precision budget;
    // unlike blur's broad plane filter this rejects same-room mixed-plane edges.
    let tolerance = 0.005 + length(p) * 0.0001;
    if abs(dot(n, left - p)) > tolerance { return rejected; }
    if abs(dot(n, right - p)) > tolerance { return rejected; }
    if abs(dot(n, up - p)) > tolerance { return rejected; }
    if abs(dot(n, down - p)) > tolerance { return rejected; }
    return Donor(n, true);
}
// The native lattice describes ground/roof occupancy, not facade exposure.
// Probe the admitted surface's facing air side; flat ground stays unshifted.
// Room classification always uses the unshifted point and remains authoritative.
fn sky_probe(n: vec3<f32>, room: u32) -> vec3<f32> {
    if room != 0u || params.control.w > 0.5 { return vec3<f32>(0.0); }
    return vec3<f32>(n.x, 0.0, n.z) * 0.5;
}
// Alpha caches domains: 1=known closed, 2=known open outdoors, room+3=room.
// Depth comes from admitted depth. Binary16 represents room codes <=2048 exactly.
fn domain_code(room: u32, openness: f32, probe_world: vec3<f32>) -> f32 {
    if room > 0u {
        if room > 2045u { return 0.0; }
        return f32(room + 3u);
    }
    if params.control.w > 0.5 || !enhanced_sky_known(probe_world) { return 0.0; }
    return select(1.0, 2.0, openness >= 0.05);
}
fn cached_domain_matches(a: f32, b: f32) -> bool {
    return a > 0.0 && a == b;
}
// Cached endpoint domains reject unknown and open/closed or room mismatches.
// Ground-lattice values at intermediate air points do not describe walls:
// retain actual room-volume checks indoors and primary-depth segment tests.
fn transfer(p: vec3<f32>, q: vec3<f32>, room: u32) -> bool {
    // ponytail: two interior screen-depth tests cannot see offscreen/subpixel
    // blockers; use an admitted geometry ray query if that ceiling matters.
    for (var i = 1; i <= 2; i++) {
        let t = f32(i) / 3.0;
        let mid = mix(p, q, t);
        if params.control.w > 0.5 || room > 0u {
            if room_at(mid) != room { return false; }
        }
        let clip = params.vp * vec4<f32>(mid, 1.0);
        if clip.w <= 0.0 { return false; }
        let ndc = clip.xy / clip.w;
        if any(abs(ndc) >= vec2<f32>(1.0)) { return false; }
        let pixel = bounded(vec2<i32>((ndc * vec2<f32>(0.5, -0.5) + 0.5) * vec2<f32>(size())));
        if depth_at(pixel) < 1.0 {
            let surface = world(pixel);
            let surface_clip = params.vp * vec4<f32>(surface, 1.0);
            if clip.w - surface_clip.w > 0.08 { return false; }
        }
    }
    return true;
}

fn oct_sign(v: vec2<f32>) -> vec2<f32> {
    return select(vec2<f32>(-1.0), vec2<f32>(1.0), v >= vec2<f32>(0.0));
}
fn encode_normal(n: vec3<f32>) -> u32 {
    var oct = n.xy / max(abs(n.x) + abs(n.y) + abs(n.z), 0.00001);
    if n.z < 0.0 { oct = (1.0 - abs(oct.yx)) * oct_sign(oct); }
    return pack2x16snorm(oct);
}
fn decode_normal(bits: u32) -> vec3<f32> {
    let oct = unpack2x16snorm(bits);
    var n = vec3<f32>(oct, 1.0 - abs(oct.x) - abs(oct.y));
    if n.z < 0.0 { n = vec3<f32>((1.0 - abs(n.yx)) * oct_sign(n.xy), n.z); }
    return normalize(n);
}
// Every frame refreshes this cache from actual admitted quarter-centre pixels.
// Depth is retained even for rejected/zero donors so those surfaces still occlude.
@fragment fn source_cache(@builtin(position) position: vec4<f32>) -> @location(0) vec4<u32> {
    let pixel = full_pixel(vec2<i32>(position.xy));
    let z = depth_at(pixel);
    let empty = vec4<u32>(0u, 0u, 0u, bitcast<u32>(z));
    if z >= 1.0 || !eligible(pixel) { return empty; }
    let p = world_from_depth(pixel, z);
    let source_far = FADE_FAR + params.control.y;
    if dot(p, p) >= source_far * source_far { return empty; }
    var radiance = max(textureLoad(diffuse, pixel, 0).rgb, vec3<f32>(0.0));
    var n = vec3<f32>(0.0);
    if any(radiance > vec3<f32>(0.0)) {
        let donor = source_eligible(pixel, p);
        n = donor.n;
        if !donor.admitted { radiance = vec3<f32>(0.0); }
    }
    // Receiver geometry is useful even when this point cannot donate radiance.
    if dot(n, n) < 0.5 { n = normal(pixel, p); }
    if dot(n, n) < 0.5 { return empty; }
    var room = 0u;
    if params.control.z > 0.0 { room = room_at(p); }
    let probe_world = p + sky_probe(n, room) + params.eye.xyz;
    let openness = enhanced_sky_open(probe_world);
    let domain = domain_code(room, openness, probe_world);
    if domain <= 0.0 { return empty; }
    return vec4<u32>(pack2x16float(radiance.rg), pack2x16float(vec2<f32>(radiance.b, domain)), encode_normal(n), bitcast<u32>(z));
}
@fragment fn gather(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let centre = textureLoad(donor_cache, vec2<i32>(position.xy), 0);
    let z = bitcast<f32>(centre.w);
    let domain = unpack2x16float(centre.y).y;
    if z >= 1.0 || domain <= 0.0 { return vec4<f32>(0.0); }
    let pixel = full_pixel(vec2<i32>(position.xy));
    let p = world_from_depth(pixel, z);
    if dot(p, p) >= FADE_FAR * FADE_FAR { return vec4<f32>(0.0); }
    let n = decode_normal(centre.z);
    var room = 0u;
    if domain >= 4.0 {
        room = u32(domain) - 3u;
    }
    let clip = params.vp * vec4<f32>(p, 1.0);
    let radius = params.control.y;
    // Translation cancels from the three projected world-axis offsets.
    let projection_scale = radius * max(length(params.vp[0].xy), max(length(params.vp[1].xy), length(params.vp[2].xy)));
    let pixel_radius = clamp(projection_scale / max(clip.w, 0.01) * 0.5 * f32(size().x), 4.0, 0.25 * f32(size().x));
    let tile = vec2<u32>(position.xy) % vec2<u32>(3u);
    let phase = (f32((tile.x * 3u + tile.y * 7u) % 9u) + 0.5) / 9.0;
    var light = vec3<f32>(0.0);
    // Deterministic ao-vk search: 18 samples per quarter pixel.
    for (var direction = 0; direction < 3; direction++) {
        let angle = (f32(direction) + phase) * 2.094395102;
        let dir = vec2<f32>(cos(angle), sin(angle));
        var horizon = 0.0;
        for (var step = 0; step < 6; step++) {
            let t = (f32(step) + 0.5) / 6.0;
            let hit = vec2<i32>(vec2<f32>(pixel) + dir * (2.0 + t * t * pixel_radius));
            if any(hit < vec2<i32>(0)) || any(hit >= size()) { continue; }
            let cell = hit / 4;
            let cached = textureLoad(donor_cache, cell, 0);
            let z = bitcast<f32>(cached.w);
            if z >= 1.0 { continue; }
            let q = world_from_depth(full_pixel(cell), z);
            let delta = q - p;
            let len2 = dot(delta, delta);
            if len2 < 0.00001 || len2 > radius * radius { continue; }
            let elevation = dot(n, delta) * inverseSqrt(len2);
            if elevation <= horizon { continue; }
            let increment = elevation - horizon;
            // Even an ineligible/closed foreground surface occludes later light.
            horizon = elevation;
            let blue_domain = unpack2x16float(cached.y);
            if !cached_domain_matches(domain, blue_domain.y) { continue; }
            let radiance = vec3<f32>(unpack2x16float(cached.x), blue_domain.x);
            if !any(radiance > vec3<f32>(0.0)) { continue; }
            if !transfer(p, q, room) { continue; }
            light += radiance * increment * (1.0 - len2 / (radius * radius));
        }
    }
    return vec4<f32>(light / 3.0, domain);
}
fn plane_weight(p: vec3<f32>, n: vec3<f32>, q: vec3<f32>) -> f32 {
    let off = abs(dot(n, q - p));
    let cutoff = 0.04 + 0.01 * length(p);
    if off >= cutoff || length(q - p) > params.control.y { return 0.0; }
    return 1.0 - off / cutoff;
}
fn blur(position: vec4<f32>, axis: vec2<i32>) -> vec4<f32> {
    let c = vec2<i32>(position.xy);
    let centre = textureLoad(incoming, c, 0);
    let pixel = full_pixel(c);
    if centre.a <= 0.0 { return vec4<f32>(0.0); }
    let geometry = textureLoad(donor_cache, c, 0);
    let p = world_from_depth(pixel, bitcast<f32>(geometry.w));
    let n = decode_normal(geometry.z);
    var sum = centre.rgb;
    var weight = 1.0;
    for (var i = -2; i <= 2; i++) {
        if i == 0 { continue; }
        let tap = c + axis * i;
        if any(tap < vec2<i32>(0)) || any(tap >= vec2<i32>(textureDimensions(incoming))) { continue; }
        let sample = textureLoad(incoming, tap, 0);
        let hit = full_pixel(tap);
        if !cached_domain_matches(centre.a, sample.a) { continue; }
        let geometry_q = textureLoad(donor_cache, tap, 0);
        let q = world_from_depth(hit, bitcast<f32>(geometry_q.w));
        let w = exp(-f32(i * i) / 2.0) * plane_weight(p, n, q);
        if w <= 0.0 { continue; }
        sum += sample.rgb * w;
        weight += w;
    }
    return vec4<f32>(sum / weight, centre.a);
}
@fragment fn blur_x1(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return blur(p, vec2<i32>(1, 0)); }
@fragment fn blur_y1(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> { return blur(p, vec2<i32>(0, 1)); }
fn incoming_at(tap: vec2<i32>) -> vec4<f32> {
    if any(tap < vec2<i32>(0)) || any(tap >= vec2<i32>(textureDimensions(incoming))) { return vec4<f32>(0.0); }
    return textureLoad(incoming, tap, 0);
}
@fragment fn composite(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    if !eligible(pixel) { return vec4<f32>(0.0); }
    let coord = (vec2<f32>(pixel) - 2.0) * 0.25;
    let base = vec2<i32>(floor(coord));
    let fraction = fract(coord);
    let s00 = incoming_at(base);
    let s10 = incoming_at(base + vec2<i32>(1, 0));
    let s01 = incoming_at(base + vec2<i32>(0, 1));
    let s11 = incoming_at(base + vec2<i32>(1, 1));
    let has_radiance = any(s00.rgb > vec3<f32>(0.0)) || any(s10.rgb > vec3<f32>(0.0))
        || any(s01.rgb > vec3<f32>(0.0)) || any(s11.rgb > vec3<f32>(0.0));
    if !has_radiance { return vec4<f32>(0.0); }
    let p = world(pixel);
    let distance = length(p);
    if distance >= FADE_FAR { return vec4<f32>(0.0); }
    let fade = 1.0 - smoothstep(FADE_NEAR, FADE_FAR, distance);
    var room = 0u;
    if params.control.z > 0.0 { room = room_at(p); }
    if params.control.w > 0.5 && room == 0u { return vec4<f32>(0.0); }
    var domain = 0.0;
    var classified = false;
    let tolerance = 0.005 + distance * 0.0001;
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    for (var y = 0; y < 2; y++) {
        for (var x = 0; x < 2; x++) {
            let tap = base + vec2<i32>(x, y);
            if any(tap < vec2<i32>(0)) || any(tap >= vec2<i32>(textureDimensions(incoming))) { continue; }
            let sample = select(select(s00, s10, x == 1), select(s01, s11, x == 1), y == 1);
            if sample.a <= 0.0 { continue; }
            let geometry = textureLoad(donor_cache, tap, 0);
            let q = world_from_depth(full_pixel(tap), bitcast<f32>(geometry.w));
            let n = decode_normal(geometry.z);
            let delta = q - p;
            // Use the cached candidate plane, not a freshly reconstructed
            // full-resolution normal; strict rejection preserves silhouettes.
            if abs(dot(n, delta)) > tolerance || dot(delta, delta) > params.control.y * params.control.y { continue; }
            if !classified {
                let probe_world = p + sky_probe(n, room) + params.eye.xyz;
                let openness = enhanced_sky_open(probe_world);
                domain = domain_code(room, openness, probe_world);
                classified = true;
                if domain <= 0.0 { return vec4<f32>(0.0); }
            }
            if !cached_domain_matches(domain, sample.a) { continue; }
            let bilinear = select(1.0 - fraction.x, fraction.x, x == 1) * select(1.0 - fraction.y, fraction.y, y == 1);
            let w = bilinear * plane_weight(p, n, q);
            if w <= 0.0 { continue; }
            sum += sample.rgb * w;
            total += w;
        }
    }
    if total <= 0.00001 { return vec4<f32>(0.0); }
    let material = textureLoad(albedo, pixel, 0);
    // Match the native gamma-space HDR exposure convention in grade.wgsl.
    // RGB already carries native fog extinction and resolved/blended coverage;
    // alpha is eligibility only and must not attenuate that budget a second time.
    let bounce = sum / total * material.rgb * params.control.x;
    return vec4<f32>(min(bounce, material.rgb * 0.25) * fade, 0.0);
}
