// ao-vk (MIT) cloud_shadow.frag: camera-centred world-space transmittance map.
// Native-layer shared extinction is evaluated only at 256x256 map texels, never
// per scene pixel. Neither actor data nor primary-view depth enters this map.
struct ShadowParams {
    area: vec4<f32>, // relative-eye x/z corner, world span, relative-eye plane y
    sun: vec4<f32>, // common authored directional source, enabled
    control: vec4<f32>, // layer count, reach
}
@group(0) @binding(0) var<uniform> frame: ShadowParams;
@group(0) @binding(1) var<storage, read> layers: array<CloudDensityLayer>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4(vec2(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn shadow(@builtin(position) position: vec4<f32>) -> @location(0) f32 {
    if frame.sun.w < 0.5 { return 1.0; }
    let xz = frame.area.xy + position.xy / 256.0 * frame.area.z;
    let origin = vec3(xz.x, frame.area.w, xz.y);
    var transmittance = 1.0;
    for (var i = 0u; i < u32(frame.control.x); i++) {
        transmittance *= cloud_transmittance(origin, frame.sun.xyz, layers[i], frame.control.y);
    }
    return transmittance;
}
