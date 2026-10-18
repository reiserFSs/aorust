// Native material sprigs, inspired by ao-vk rvk/leaves.cpp (MIT).
// Native shader, shared moon receiver and sun-shadow entrypoints are assembled by foliage_leaves.rs.
struct CanopyParams {
    // Stable effect phase; attached, flutter and falling switches.
    control: vec4<f32>,
    // Smoothed world X/Z direction, strength, unused.
    wind: vec4<f32>,
}
@group(2) @binding(0) var<uniform> canopy: CanopyParams;

fn canopy_vertex(v: VIn, center_phase: vec4<f32>, falling: f32) -> VIn {
    var leaf = v;
    let phase = center_phase.w;
    let clock = canopy.control.x;
    let gust = pow(max(0.0, sin(clock * 3.0 + phase)), 4.0);
    let local = v.pos - center_phase.xyz;
    let angle = canopy.control.z * sin(clock * 53.0 + phase) * (0.08 + 0.18 * gust) * canopy.wind.z;
    // Flutter about a tangent; rotate the normal so sprigs catch native sunlight.
    let reference = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(v.normal.y) > 0.9);
    let tangent = cross(v.normal, reference);
    let axis = tangent / max(length(tangent), 0.00001);
    leaf.pos = center_phase.xyz + local * cos(angle) + cross(axis, local) * sin(angle)
        + axis * dot(axis, local) * (1.0 - cos(angle));
    leaf.normal = v.normal * cos(angle) + cross(axis, v.normal) * sin(angle)
        + axis * dot(axis, v.normal) * (1.0 - cos(angle));
    if falling > 0.5 {
        // One short fall per deterministic gust. No actors, particle history or
        // respawn allocation; disabled gusts contribute no detached geometry.
        let cycle = clock * 3.0 + phase;
        let age = fract((cycle - 1.570796327) / 6.283185307) * 6.283185307;
        let life = 1.1;
        let visible = canopy.control.w > 0.5 && canopy.wind.z > 0.3 && age < life;
        let seconds = age * 1.7;
        let world_drop = vec3<f32>(canopy.wind.x * seconds * 0.3,
            -0.5 * seconds * seconds, canopy.wind.y * seconds * 0.3);
        // Undo native instance scaling/rotation for a world-down fall.
        let a = v.m0.xyz;
        let b = v.m1.xyz;
        let c = v.m2.xyz;
        let determinant = dot(a, cross(b, c));
        if abs(determinant) > 0.000001 {
            leaf.pos += vec3<f32>(dot(world_drop, cross(b, c)),
                dot(world_drop, cross(c, a)), dot(world_drop, cross(a, b))) / determinant;
        }
        if !visible { leaf.pos = vec3<f32>(0.0); leaf.color.a = 0.0; }
    } else if canopy.control.y < 0.5 {
        leaf.pos = vec3<f32>(0.0);
        leaf.color.a = 0.0;
    }
    return leaf;
}
@vertex fn vs_canopy(v: VIn, @location(8) center_phase: vec4<f32>, @location(9) falling: f32) -> VOut {
    var output = canopy_native_vs(canopy_vertex(v, center_phase, falling));
    output.fog = max(output.fog, vtx(v).fog);
    return output;
}
struct CanopyOutput {
    @location(0) beauty: vec4<f32>,
    //__AMBIENT_FIELD__
    //__EMISSIVE_FIELD__
    //__SUN_FIELD__
}
@fragment fn fs_canopy(i: VOut) -> CanopyOutput {
    var output: CanopyOutput;
    // Shared lighting adjustment precedes exact native material, alpha test and fog.
    //__MOON_RECEIVER__
    output.beauty = shade(receiver, 1u);
    if i.color.a <= 0.0 { discard; }
    // Added leaves never emit an HDR/bloom signal or alter native fog extinction.
    //__AMBIENT_WRITE__
    //__EMISSIVE_WRITE__
    //__SUN_WRITE__
    return output;
}
@vertex fn vs_canopy_shadow(v: VIn, @location(8) center_phase: vec4<f32>, @location(9) falling: f32) -> ShadowVertex {
    let leaf = canopy_vertex(v, center_phase, falling);
    var output = shadow_vertex(leaf, leaf.uv, 0u, shadow_frame.matrices[shadow_frame.control.x], false);
    if (u32(mat.native.z) & 64u) != 0u {
        let moved = foliage_vertex(leaf, vtx(leaf));
        output.clip = shadow_frame.matrices[shadow_frame.control.x] * vec4<f32>(moved.wpos, 1.0);
    }
    return output;
}
@fragment fn fs_canopy_shadow(i: ShadowVertex) {
    shadow_alpha(i, true);
    if i.alpha <= 0.0 { discard; }
}
