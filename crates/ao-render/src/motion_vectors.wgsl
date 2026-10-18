// Appended to the native shader: texture addressing and alpha-test are native.
struct MotionPose {
    current_vp: mat4x4<f32>,
    previous_vp: mat4x4<f32>,
    previous_model: mat4x4<f32>,
    control: vec4<u32>,
}
@group(2) @binding(2) var<uniform> motion_pose: MotionPose;
struct MotionVertex {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) @interpolate(perspective, sample) previous: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) alpha: f32,
    @location(3) @interpolate(perspective, sample) native_current: vec4<f32>,
}
fn motion_vertex(v: VIn, previous: vec3<f32>, uv: vec2<f32>) -> MotionVertex {
    let model = mat4x4<f32>(vec4<f32>(v.m0.xyz, 0.0), v.m1, v.m2, v.m3);
    var o: MotionVertex;
    // Preserve the native operation order (model first, then view projection).
    let wp = model * vec4<f32>(v.pos, 1.0);
    let previous_wp = motion_pose.previous_model * vec4<f32>(previous, 1.0);
    o.clip = g.vp * wp;
    o.native_current = motion_pose.current_vp * wp;
    o.previous = motion_pose.previous_vp * previous_wp;
    o.uv = uv;
    o.alpha = v.color.a * select(mat.color.a, 1.0 + v.m0.w, v.m0.w < 0.0);
    return o;
}
@vertex fn vs_motion_world(v: VIn) -> MotionVertex {
    return motion_vertex(v, v.pos, v.uv);
}
@vertex fn vs_motion_actor(v: VIn, @location(11) uv: vec4<f32>, @location(12) previous: vec3<f32>) -> MotionVertex {
    return motion_vertex(v, previous, v.uv * uv.xy + uv.zw);
}
struct MotionPixel {
    @location(0) motion: vec4<f32>,
    @location(1) identity: u32,
}
fn motion_pixel(i: MotionVertex, sample: u32, cutout: bool, reactive: bool) -> MotionPixel {
    let t = sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z);
    let a = select(t.a, 1.0, mat.emissive.w > 0.5 && mat.emissive.w < 1.5) * i.alpha;
    if native_alpha_test() && !native_alpha_pass(a) { discard; }
    // Native per-sample depth already contains the exact alpha-to-coverage mask.
    if cutout && !native_alpha_test() && g.fog.w < 0.5 && a < 0.5 { discard; }
    // ONE/ONE native effects can emit RGB with zero texture alpha.
    if reactive && a <= 0.00001 && all(t.rgb <= vec3<f32>(0.0)) { discard; }
    // Hardware Equal/LessEqual tests against the read-only native depth attachment.
    // Never compare interpolated fragment position.z to a sampled depth in WGSL.
    let uv = i.native_current.xy / i.native_current.w * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let previous_uv = i.previous.xy / i.previous.w * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let invalid = reactive || motion_pose.control.y != 0u || i.previous.w <= 0.0;
    var o: MotionPixel;
    o.motion = vec4<f32>(uv - previous_uv, i.previous.z / i.previous.w, select(0.0, 1.0, invalid));
    o.identity = motion_pose.control.x;
    return o;
}
@fragment fn fs_motion_opaque(i: MotionVertex, @builtin(sample_index) sample: u32) -> MotionPixel { return motion_pixel(i, sample, false, false); }
@fragment fn fs_motion_test(i: MotionVertex, @builtin(sample_index) sample: u32) -> MotionPixel { return motion_pixel(i, sample, true, false); }
@fragment fn fs_motion_reactive(i: MotionVertex, @builtin(sample_index) sample: u32) -> MotionPixel { return motion_pixel(i, sample, false, true); }
