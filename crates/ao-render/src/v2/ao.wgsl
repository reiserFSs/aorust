// Enhanced v2 ambient occlusion: half-res GTAO on the HiZ + G1 normals, 3x3 bilateral + temporal denoise, bilateral upsample to the
// full-res R8Unorm texture the resolve reads. `common.wgsl` and `packing.wgsl` precede this file. Entry points `gtao`, `denoise`
// (compute, half res) and `up_vs`/`up_fs` (render, full res) share one module; each pipeline's layout lists only its own bindings.
const HIZ_SKY: f32 = 1.0e9;

@group(0) @binding(0) var<uniform> fc: FC;
@group(0) @binding(1) var hiz: texture_2d<f32>;
@group(0) @binding(2) var g1t: texture_2d<f32>;
@group(0) @binding(3) var raw_out: texture_storage_2d<r32float, write>;
@group(0) @binding(4) var raw_in: texture_2d<f32>;
@group(0) @binding(5) var hist_prev: texture_2d<f32>;       // rg = AO, view depth of the previous frame's denoised texel
@group(0) @binding(6) var hist_out: texture_storage_2d<rg32float, write>;
@group(0) @binding(7) var<uniform> params: vec4<u32>;       // x = 1: drop the history (admission change, resize, jitter toggle)
@group(0) @binding(8) var hist_in: texture_2d<f32>;

const RADIUS: f32 = 1.5;          // world metres
const SLICES: u32 = 2u;
const STEPS: u32 = 6u;            // per side
const MAX_PX: f32 = 96.0;         // screen radius cap (full-res pixels)
const FALLOFF: f32 = 0.45;       // fraction of the radius over which an occluder's weight fades out
const THIN: f32 = 0.15;          // thin-occluder compensation: how fast the horizon relaxes behind a thin object (0 = solid walls)
const POWER: f32 = 2.2;          // contact creases read, open ground stays ~1
const GRAZE: vec2<f32> = vec2<f32>(0.05, 0.35); // |n.v| range over which AO fades in; edge-on surfaces (silhouettes) stay 1
const HALF_PI: f32 = 1.5707963;

// Fully fogged (native transmittance 0) and sky texels carry no AO and contribute no occluder: AO never describes geometry the
// native fog hides (the `ambient_occlusion::fog_depth_limit` rule, expressed in view depth).
fn visible(z: f32) -> bool { return z < HIZ_SKY * 0.5 && native_transmittance(z) > 0.0; }

// View-space position of full-res pixel centre `pix` at view depth `z` (the sub-pixel TAA jitter is ignored).
fn vs_pos(pix: vec2<f32>, z: f32) -> vec3<f32> {
    let uv = pix * fc.viewport.zw;
    let ndc = vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0);
    return vec3<f32>(ndc.x * fc.proj.z * fc.proj.w * z, ndc.y * fc.proj.z * z, -z);
}

fn ign(p: vec2<f32>) -> f32 { return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715)))); }

@compute @workgroup_size(8, 8)
fn gtao(@builtin(global_invocation_id) id: vec3<u32>) {
    let hd = textureDimensions(raw_out);
    if id.x >= hd.x || id.y >= hd.y { return; }
    let fd = vec2<i32>(fc.viewport.xy);
    let hp = vec2<i32>(id.xy);
    let fp = min(hp * 2, fd - vec2<i32>(1));
    let z = textureLoad(hiz, fp, 0).r;
    let rpx = min(RADIUS / z * (fc.viewport.y * 0.5) / fc.proj.z, MAX_PX);
    if !visible(z) || rpx < 2.0 {
        textureStore(raw_out, hp, vec4<f32>(1.0));
        return;
    }
    let p = vs_pos(vec2<f32>(fp) + vec2<f32>(0.5), z);
    var n = normalize((fc.view * vec4<f32>(oct_decode(textureLoad(g1t, fp, 0).rg), 0.0)).xyz);
    let v = normalize(-p);
    let facing = dot(n, v);
    if facing < 0.0 { n = -n; }

    // The sampling pattern only rotates through time when TAA jitters the frame (TAA integrates it); otherwise identical frames
    // produce identical AO (no flicker).
    let t = select(0.0, f32(fc.flags.x & 7u), fc.jitter.x != 0.0 || fc.jitter.y != 0.0);
    let n_slice = ign(vec2<f32>(hp) + vec2<f32>(5.588238 * t));
    let n_step = ign(vec2<f32>(hp.yx) + vec2<f32>(37.0, 17.0) + vec2<f32>(5.588238 * t));
    let falloff_mul = -1.0 / (RADIUS * FALLOFF);
    let falloff_add = (RADIUS * (1.0 - FALLOFF)) / (RADIUS * FALLOFF) + 1.0;

    var vis = 0.0;
    for (var s = 0u; s < SLICES; s++) {
        let phi = (f32(s) + n_slice) * (PI / f32(SLICES));
        let omega = vec2<f32>(cos(phi), -sin(phi)); // pixel space, y down
        let dir = vec3<f32>(cos(phi), sin(phi), 0.0);
        let ortho = dir - dot(dir, v) * v;
        let axis = normalize(cross(ortho, v));
        let proj_n = n - axis * dot(n, axis);
        let plen = max(length(proj_n), 1e-4);
        let sgn = select(-1.0, 1.0, dot(ortho, proj_n) >= 0.0);
        let cos_n = clamp(dot(proj_n, v) / plen, 0.0, 1.0);
        let ang_n = sgn * acos(cos_n);
        var h = vec2<f32>(cos(ang_n + HALF_PI), cos(ang_n - HALF_PI)); // lowest horizons: the tangent plane
        let low = h;
        for (var k = 0u; k < STEPS; k++) {
            let u = (f32(k) + n_step) / f32(STEPS);
            let off_px = u * u * rpx + 1.3;
            let mip = u32(clamp(log2(off_px) - 3.0, 0.0, 4.0));
            for (var side = 0u; side < 2u; side++) {
                let o = omega * off_px * select(-1.0, 1.0, side == 0u);
                let c = vec2<i32>(floor(vec2<f32>(fp) + vec2<f32>(0.5) + o));
                if any(c < vec2<i32>(0)) || any(c >= fd) { continue; }
                let zs = textureLoad(hiz, clamp(c, vec2<i32>(0), fd - vec2<i32>(1)) >> vec2<u32>(mip), i32(mip)).r;
                if !visible(zs) { continue; }
                let delta = vs_pos(vec2<f32>(c) + vec2<f32>(0.5), zs) - p;
                let dl = max(length(delta), 1e-4);
                let weight = clamp(dl * falloff_mul + falloff_add, 0.0, 1.0);
                let shc = mix(low[side], dot(delta / dl, v), weight);
                h[side] = select(mix(h[side], shc, THIN), shc, shc > h[side]);
            }
        }
        let h0 = ang_n + clamp(-acos(h.y) - ang_n, -HALF_PI, HALF_PI);
        let h1 = ang_n + clamp(acos(h.x) - ang_n, -HALF_PI, HALF_PI);
        let sn = sin(ang_n);
        let arc0 = (cos_n + 2.0 * h0 * sn - cos(2.0 * h0 - ang_n)) * 0.25;
        let arc1 = (cos_n + 2.0 * h1 * sn - cos(2.0 * h1 - ang_n)) * 0.25;
        vis += plen * (arc0 + arc1);
    }
    textureStore(raw_out, hp, vec4<f32>(mix(1.0, clamp(vis / f32(SLICES), 0.0, 1.0), smoothstep(GRAZE.x, GRAZE.y, abs(facing)))));
}

// 3x3 depth-weighted blur of the raw visibility, then the temporal blend against the reprojected previous result.
@compute @workgroup_size(8, 8)
fn denoise(@builtin(global_invocation_id) id: vec3<u32>) {
    let hd = vec2<i32>(textureDimensions(hist_out));
    if id.x >= u32(hd.x) || id.y >= u32(hd.y) { return; }
    let fd = vec2<i32>(fc.viewport.xy);
    let hp = vec2<i32>(id.xy);
    let fp = min(hp * 2, fd - vec2<i32>(1));
    let z0 = textureLoad(hiz, fp, 0).r;
    if !visible(z0) {
        textureStore(hist_out, hp, vec4<f32>(1.0, z0, 0.0, 0.0));
        return;
    }
    var sum = 0.0;
    var wsum = 0.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let q = clamp(hp + vec2<i32>(x, y), vec2<i32>(0), hd - vec2<i32>(1));
            let zq = textureLoad(hiz, min(q * 2, fd - vec2<i32>(1)), 0).r;
            if !visible(zq) { continue; }
            let w = exp(-abs(zq - z0) / (0.05 * z0 + 0.05) - f32(x * x + y * y) * 0.35);
            sum += textureLoad(raw_in, q, 0).r * w;
            wsum += w;
        }
    }
    let ao = pow(clamp(sum / max(wsum, 1e-4), 0.0, 1.0), POWER);

    // Reproject through the unjittered matrices: world position from the view matrix, then the previous frame's clip.
    let p = vs_pos(vec2<f32>(fp) + vec2<f32>(0.5), z0);
    let rot = mat3x3<f32>(fc.view[0].xyz, fc.view[1].xyz, fc.view[2].xyz);
    let wp = fc.eye.xyz + transpose(rot) * p;
    let pc = fc.prev_view_proj_unjit * vec4<f32>(wp, 1.0);
    var blend = 0.0;
    var hist = ao;
    if params.x == 0u && pc.w > 0.0 {
        let uv = vec2<f32>(pc.x / pc.w * 0.5 + 0.5, 0.5 - pc.y / pc.w * 0.5);
        let ph = uv * vec2<f32>(hd) - vec2<f32>(0.5);
        let base = vec2<i32>(floor(ph));
        let f = ph - vec2<f32>(base);
        var hsum = 0.0;
        var hw = 0.0;
        for (var k = 0; k < 4; k++) {
            let tap = base + vec2<i32>(k & 1, k / 2);
            if any(tap < vec2<i32>(0)) || any(tap >= hd) { continue; }
            let hv = textureLoad(hist_prev, tap, 0).rg;
            let bil = select(1.0 - f.x, f.x, (k & 1) == 1) * select(1.0 - f.y, f.y, k >= 2);
            let w = bil * exp(-abs(hv.g - pc.w) / (0.04 * pc.w + 0.04));
            hsum += hv.r * w;
            hw += w;
        }
        if hw > 0.2 {
            hist = hsum / hw;
            blend = 0.88 * min(hw, 1.0);
        }
    }
    textureStore(hist_out, hp, vec4<f32>(mix(ao, hist, blend), z0, 0.0, 0.0));
}

@vertex
fn up_vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - vec2<f32>(1.0), 0.0, 1.0);
}

// Depth-aware bilinear upsample of the half-res result to full res (sky and fully fogged pixels stay 1).
@fragment
fn up_fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = vec2<i32>(pos.xy);
    let z = textureLoad(hiz, c, 0).r;
    if !visible(z) { return vec4<f32>(1.0); }
    let hd = vec2<i32>(textureDimensions(hist_in));
    let ph = pos.xy * 0.5 - vec2<f32>(0.5);
    let base = vec2<i32>(floor(ph));
    let f = ph - vec2<f32>(base);
    var sum = 0.0;
    var wsum = 0.0;
    var best = 1.0;
    var best_dz = 1.0e30;
    for (var k = 0; k < 4; k++) {
        let tap = clamp(base + vec2<i32>(k & 1, k / 2), vec2<i32>(0), hd - vec2<i32>(1));
        let s = textureLoad(hist_in, tap, 0).rg;
        let dz = abs(s.g - z);
        let w = select(1.0 - f.x, f.x, (k & 1) == 1) * select(1.0 - f.y, f.y, k >= 2) * exp(-dz / (0.03 * z + 0.03));
        sum += s.r * w;
        wsum += w;
        if dz < best_dz { best_dz = dz; best = s.r; }
    }
    return vec4<f32>(clamp(select(best, sum / max(wsum, 1e-6), wsum > 1e-3), 0.0, 1.0));
}
