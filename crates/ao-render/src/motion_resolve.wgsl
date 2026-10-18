// Resolve the same nearest native depth sample used by the Enhanced depth resolve.
// Integer identities are copied explicitly, never averaged by hardware resolve.
@group(0) @binding(0) var native_depth: texture_depth_multisampled_2d;
@group(0) @binding(1) var sample_motion: texture_multisampled_2d<f32>;
@group(0) @binding(2) var sample_identity: texture_multisampled_2d<u32>;
@vertex fn vs_resolve(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let x = f32((vertex << 1u) & 2u);
    let y = f32(vertex & 2u);
    return vec4<f32>(vec2<f32>(x, y) * 2.0 - vec2<f32>(1.0), 0.0, 1.0);
}
struct ResolvedMotion {
    @location(0) motion: vec4<f32>,
    @location(1) identity: u32,
}
@fragment fn fs_resolve(@builtin(position) position: vec4<f32>) -> ResolvedMotion {
    let pixel = vec2<i32>(position.xy);
    var nearest = 0;
    var depth = textureLoad(native_depth, pixel, 0);
    for (var sample = 1; sample < i32(textureNumSamples(native_depth)); sample++) {
        let candidate = textureLoad(native_depth, pixel, sample);
        if candidate < depth { depth = candidate; nearest = sample; }
    }
    var o: ResolvedMotion;
    o.motion = textureLoad(sample_motion, pixel, nearest);
    o.identity = textureLoad(sample_identity, pixel, nearest).x;
    // A multisample beauty pixel mixes coverage. Do not let a second surface or
    // reactive additive sample borrow the nearest surface's history/blur vector.
    for (var sample = 0; sample < i32(textureNumSamples(native_depth)); sample++) {
        let id = textureLoad(sample_identity, pixel, sample).x;
        let motion = textureLoad(sample_motion, pixel, sample);
        if id != o.identity || motion.w > 0.0 { o.motion.w = 1.0; }
    }
    return o;
}
