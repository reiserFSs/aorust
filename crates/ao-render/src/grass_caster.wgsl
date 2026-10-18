// Sun-only low-poly silhouettes reuse the cached beauty root, tip and wind.
// ponytail: one triangle omits species shoulders; restore sections if silhouettes need them.
struct GrassCasterFrame {
    matrices: array<mat4x4<f32>, 3>,
    splits: vec4<f32>, eye: vec4<f32>, forward: vec4<f32>, control: vec4<u32>,
    moon_direction: vec4<f32>, moon_budget: vec4<f32>,
}
@group(3) @binding(2) var<uniform> grass_caster_frame: GrassCasterFrame;
@vertex fn vs_grass_shadow(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> @builtin(position) vec4<f32> {
    let clipped = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    let band = instance >> 28u;
    let within = instance & 0x0fffffffu;
    let stride = u32(grass_params.bands[band / 4u][band % 4u]);
    let cell = grass_visible[band * GRASS_MAX_CELLS + within / stride] & (GRASS_MAX_CELLS - 1u);
    let span = grass_blade_cache.spans[cell];
    let k = within % stride;
    if k >= span.count { return clipped; }
    let index = span.offset + k;
    let blade = grass_blade_cache.blades[index];
    if blade.base.head.w == 0.0 { return clipped; }
    let delta = blade.base.root_coverage.xyz - grass_caster_frame.eye.xyz;
    if dot(delta, delta) > 15.0 * 15.0 { return clipped; }
    var world = select(blade.sections[0].left.xyz, blade.sections[0].right.xyz, vertex == 1u);
    if vertex == 2u { world = (blade.sections[2].left.xyz + blade.sections[2].right.xyz) * 0.5; }
    return grass_caster_frame.matrices[0] * vec4<f32>(world, 1.0);
}
