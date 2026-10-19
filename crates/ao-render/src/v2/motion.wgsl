// Enhanced v2 actor motion pass (G2), appended to the native `shader.wgsl` (texture addressing, alpha rules, vertex stages are native).
// Draws the admitted opaque / alpha-tested actor items again with a hardware Equal depth test against the G-buffer depth and writes
// the screen-space motion (uv_current - uv_previous, unjittered) of the displayed pose; `64` marks "no valid history".
struct MotionPose {
    current_vp: mat4x4<f32>,
    previous_vp: mat4x4<f32>,
    previous_model: mat4x4<f32>,
    control: vec4<u32>,   // y = 1: pose has no valid previous (first frame, teleport, history reset)
}
@group(2) @binding(2) var<uniform> motion_pose: MotionPose;

struct MotionVertex {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) previous: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) alpha: f32,
    @location(3) native_current: vec4<f32>,
}

@vertex fn vs_motion_actor(v: VIn, @location(11) uv: vec4<f32>, @location(12) previous: vec3<f32>) -> MotionVertex {
    let model = mat4x4<f32>(vec4<f32>(v.m0.xyz, 0.0), v.m1, v.m2, v.m3);
    var o: MotionVertex;
    // Native operation order (model first, then the jittered view projection the G-buffer used) keeps depth bit-identical.
    let wp = model * vec4<f32>(v.pos, 1.0);
    let previous_wp = motion_pose.previous_model * vec4<f32>(previous, 1.0);
    o.clip = g.vp * wp;
    o.native_current = motion_pose.current_vp * wp;
    o.previous = motion_pose.previous_vp * previous_wp;
    o.uv = v.uv * uv.xy + uv.zw;
    o.alpha = v.color.a * select(mat.color.a, 1.0 + v.m0.w, v.m0.w < 0.0);
    return o;
}

fn motion_pixel(i: MotionVertex, cutout: bool) -> vec4<f32> {
    let t = sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z);
    let a = select(t.a, 1.0, mat.emissive.w > 0.5 && mat.emissive.w < 1.5) * i.alpha;
    if native_alpha_test() && !native_alpha_pass(a) { discard; }
    if cutout && !native_alpha_test() && a < 0.5 { discard; }
    let uvc = i.native_current.xy / i.native_current.w * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let uvp = i.previous.xy / i.previous.w * vec2<f32>(0.5, -0.5) + vec2<f32>(0.5);
    let invalid = motion_pose.control.y != 0u || i.previous.w <= 0.0 || i.native_current.w <= 0.0;
    return vec4<f32>(select(uvc - uvp, vec2<f32>(64.0), invalid), 0.0, 0.0);
}
@fragment fn fs_motion_opaque(i: MotionVertex) -> @location(0) vec4<f32> { return motion_pixel(i, false); }
@fragment fn fs_motion_test(i: MotionVertex) -> @location(0) vec4<f32> { return motion_pixel(i, true); }
