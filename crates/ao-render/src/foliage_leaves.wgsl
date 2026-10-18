// Native-texture sprigs and stateless falls, after ao-vk leaves.cpp / ffp.vert (MIT).
struct CanopyParams {
    control: vec4<f32>,
    wind: vec4<f32>,
}
@group(2) @binding(0) var<uniform> canopy: CanopyParams;
// Header: patch count. Each patch has two descriptors then its native heights.
@group(2) @binding(1) var<storage, read> canopy_ground: array<vec4<f32>>;

fn canopy_rotate(v: vec3<f32>, axis: vec3<f32>, angle: f32) -> vec3<f32> {
    return v * cos(angle) + cross(axis, v) * sin(angle) + axis * dot(axis, v) * (1.0 - cos(angle));
}
fn canopy_inverse(v: vec3<f32>, a: vec3<f32>, b: vec3<f32>, c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(v, cross(b, c)), dot(v, cross(c, a)), dot(v, cross(a, b))) / dot(a, cross(b, c));
}
fn canopy_height(index: u32) -> f32 {
    return canopy_ground[index / 4u][index % 4u];
}
// Exact native unit-cell checkerboard diagonals, not an invented ground plane.
fn canopy_surface(xz: vec2<f32>) -> vec4<f32> {
    for (var index = 0u; index < u32(canopy_ground[0].x); index += 1u) {
        let ground_patch = canopy_ground[1u + index * 2u];
        let grid = vec2<f32>(xz.x, -xz.y) / ground_patch.z - ground_patch.xy;
        if any(grid < vec2<f32>(0.0)) || any(grid >= vec2<f32>(ground_patch.w)) { continue; }
        let cell = vec2<u32>(floor(grid));
        let f = fract(grid);
        let stride = u32(ground_patch.w) + 1u;
        let offset = u32(canopy_ground[2u + index * 2u].x) + cell.y * stride + cell.x;
        let h00 = canopy_height(offset);
        let h10 = canopy_height(offset + 1u);
        let h01 = canopy_height(offset + stride);
        let h11 = canopy_height(offset + stride + 1u);
        var gradient = vec2<f32>(h11 - h01, h01 - h00);
        var height = h00;
        let parity = (~u32(i32(floor(grid.y + ground_patch.y))) ^ u32(i32(floor(grid.x + ground_patch.x)))) & 1u;
        if parity == 1u {
            if f.x >= f.y { gradient = vec2<f32>(h10 - h00, h11 - h10); }
            height += dot(f, gradient);
        } else {
            gradient = vec2<f32>(h10 - h00, h01 - h00);
            if f.x + f.y > 1.0 {
                gradient = vec2<f32>(h11 - h01, h11 - h10);
                height = h11 + dot(f - vec2<f32>(1.0), gradient);
            } else { height += dot(f, gradient); }
        }
        return vec4<f32>(height, normalize(vec3<f32>(-gradient.x / ground_patch.z, 1.0, gradient.y / ground_patch.z)));
    }
    return vec4<f32>(-1e20, 0.0, 1.0, 0.0);
}
fn canopy_vertex(v: VIn, center_phase: vec4<f32>, falling: f32) -> VIn {
    var leaf = v;
    let phase = center_phase.w;
    let time = canopy.control.x / 0.05;
    let a = v.m0.xyz;
    let b = v.m1.xyz;
    let c = v.m2.xyz;
    let determinant = dot(a, cross(b, c));
    if abs(determinant) < 0.000001 { leaf.color.a = 0.0; return leaf; }
    let model = mat4x4<f32>(vec4<f32>(a, 0.0), v.m1, v.m2, v.m3);
    var anchor = v;
    anchor.pos = center_phase.xyz;
    let original = vtx(anchor);
    if falling < 0.5 {
        let branch = foliage_vertex(anchor, original).wpos - original.wpos;
        let aa = vec2<f32>(dot(original.wpos.xz, vec2<f32>(0.8, 0.6)), dot(original.wpos.xz, vec2<f32>(-0.6, 0.8)));
        let gust = grass_gust(aa, time);
        let angle = canopy.control.z * sin(time * (6.0 + 2.0 * fract(phase * 7.0)) + phase)
            * (0.15 + 0.3 * gust) * canopy.wind.z;
        let reference = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(v.normal.y) > 0.9);
        let axis = normalize(cross(v.normal, reference));
        leaf.pos = center_phase.xyz + canopy_rotate(v.pos - center_phase.xyz, axis, angle)
            + canopy_inverse(branch, a, b, c);
        leaf.normal = canopy_rotate(v.normal, axis, angle);
        if canopy.control.y < 0.5 { leaf.color.a = 0.0; }
        return leaf;
    }
    // Six cycles per stable wind-clock period: 20.944 s, enough for the
    // bounded 13 s descent, five complete seconds at rest, then one shrinking.
    let period = 20.943951;
    let age = fract((canopy.control.x * 6.0 + phase) / 6.283185307) * period;
    let release_time = time - age;
    let aa = vec2<f32>(dot(original.wpos.xz, vec2<f32>(0.8, 0.6)), dot(original.wpos.xz, vec2<f32>(-0.6, 0.8)));
    let gust = grass_gust(aa, release_time);
    let chance = fract(sin(phase * 12.9898) * 43758.5453);
    if chance >= min(0.3 + 1.2 * gust, 1.0) { leaf.color.a = 0.0; return leaf; }
    let release_branch = foliage_vertex_at(anchor, original, canopy.control.x - age * 0.05, false).wpos - original.wpos;
    let start = original.wpos;
    let root_ground = canopy_surface(start.xz);
    if canopy.control.w < 0.5 || root_ground.x < -1e19 { leaf.color.a = 0.0; return leaf; }
    // Fixed-world release direction avoids the native weather random walk
    // moving a leaf already at rest. Its strength still follows release gusts.
    let heading = normalize(vec2<f32>(0.8, 0.6) + vec2<f32>(sin(phase), cos(phase)) * 0.22);
    let drift = heading * min(max(start.y - root_ground.x, 0.0) * (0.18 + 0.25 * gust), 5.0);
    let landing = canopy_surface(start.xz + drift);
    if landing.x < -1e19 { leaf.color.a = 0.0; return leaf; }
    let height = max(start.y - landing.x, 0.0);
    let speed = max(0.85 + 0.25 * fract(phase), height / 13.0);
    let fall_time = height / speed;
    if age >= fall_time + 6.0 { leaf.color.a = 0.0; return leaf; }
    let flying = min(age, fall_time);
    let travel = select(1.0, flying / max(fall_time, 0.001), fall_time > 0.001);
    let spin = flying * (2.0 + fract(phase) * 2.0) + phase;
    let spiral = vec2<f32>(cos(spin), sin(spin)) * 0.3 * sin(travel * 3.141592654);
    let center = vec3<f32>(start.xz + drift * travel + spiral, start.y - speed * flying).xzy
        + release_branch * (1.0 - travel);
    let local = (model * vec4<f32>(v.pos - center_phase.xyz, 0.0)).xyz;
    let normal = normalize((cross(b, c) * v.normal.x + cross(c, a) * v.normal.y + cross(a, b) * v.normal.z) / determinant);
    var offset: vec3<f32>;
    var world_normal: vec3<f32>;
    if age < fall_time {
        let axis = normalize(vec3<f32>(sin(phase), 0.35, cos(phase)));
        let angle = age * (2.5 + 3.0 * fract(phase * 1.7));
        offset = canopy_rotate(local, axis, angle);
        world_normal = canopy_rotate(normal, axis, angle);
    } else {
        let up = landing.yzw;
        let heading_axis = normalize(vec3<f32>(heading.x, 0.0, heading.y));
        let u = normalize(heading_axis - up * dot(up, heading_axis));
        let tangent = normalize(cross(normal, select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(normal.y) > 0.9)));
        offset = (u * dot(local, tangent) + cross(up, u) * dot(local, cross(normal, tangent)))
            * (1.0 - clamp(age - fall_time - 5.0, 0.0, 1.0));
        world_normal = up;
    }
    let rest_lift = select(0.0, 0.035, age >= fall_time);
    let world = center + offset + vec3<f32>(0.0, rest_lift, 0.0);
    leaf.pos = canopy_inverse(world - v.m3.xyz, a, b, c);
    // Convert a world normal back for the shared inverse-transpose vertex path.
    leaf.normal = vec3<f32>(dot(a, world_normal), dot(b, world_normal), dot(c, world_normal));
    return leaf;
}
@vertex fn vs_canopy(v: VIn, @location(8) center_phase: vec4<f32>, @location(9) falling: f32, @location(10) corner: vec2<f32>) -> VOut {
    var output = canopy_native_vs(canopy_vertex(v, center_phase, falling));
    output.fog = max(output.fog, vtx(v).fog);
    output.canopy_corner = corner;
    return output;
}
struct CanopyOutput {
    @location(0) beauty: vec4<f32>,
    //__AMBIENT_FIELD__
    //__EMISSIVE_FIELD__
    //__SUN_FIELD__
    //__WATER_FIELD__
}
@fragment fn fs_canopy(i: VOut) -> CanopyOutput {
    var output: CanopyOutput;
    // Initialize the shared shade cache before any divergent coverage. The
    // generated world wrappers normally own this prefix; sprigs call shade directly.
    early_primary = sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z);
    if mat.native.x > 0.5 { early_environment = sample_clamped(env_tex, i.env_uv); }
    //__SUN_MEMO_RESET__
    let surface_normal = cross(dpdx(i.wpos), dpdy(i.wpos));
    if i.color.a <= 0.0 { discard; }
    var rounded = i;
    rounded.color.a *= 1.0 - smoothstep(0.6, 1.0, length(i.canopy_corner));
    // PCF receiver-plane gradients need the actual card plane, not its
    // outward-rounded lighting normal (the native lighting is already baked).
    rounded.n = select(i.n, surface_normal, dot(surface_normal, surface_normal) > 0.0000001);
    early_reject(rounded, 1u);
    //__MOON_RECEIVER__
    output.beauty = shade(receiver, 1u);
    //__AMBIENT_WRITE__
    //__EMISSIVE_WRITE__
    //__SUN_WRITE__
    //__WATER_WRITE__
    return output;
}
struct CanopyShadow {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) alpha: f32,
    @location(2) corner: vec2<f32>,
}
@vertex fn vs_canopy_shadow(v: VIn, @location(8) center_phase: vec4<f32>, @location(9) falling: f32, @location(10) corner: vec2<f32>) -> CanopyShadow {
    let leaf = canopy_vertex(v, center_phase, falling);
    let vertex = shadow_vertex(leaf, leaf.uv, 0u, shadow_frame.matrices[shadow_frame.control.x], false);
    return CanopyShadow(vertex.clip, vertex.uv, vertex.alpha, corner);
}
@fragment fn fs_canopy_shadow(i: CanopyShadow) {
    var vertex: ShadowVertex;
    vertex.uv = i.uv;
    vertex.alpha = i.alpha * (1.0 - smoothstep(0.6, 1.0, length(i.corner)));
    shadow_alpha(vertex, true);
    if vertex.alpha <= 0.0 { discard; }
}
