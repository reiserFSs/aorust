// Shared by every v2 compute/raster pass: concatenate in front of the pass source and declare
// `@group(G) @binding(B) var<uniform> fc: FC;` there. Mirror of `FrameConsts` (consts.rs).
struct FC {
    view: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    view_proj_unjit: mat4x4<f32>,
    prev_view_proj_unjit: mat4x4<f32>,
    eye: vec4<f32>,          // xyz camera, w far
    viewport: vec4<f32>,     // w, h, 1/w, 1/h
    proj: vec4<f32>,         // near, far, tan(fovy/2), aspect
    jitter: vec4<f32>,       // xy pixels, z frame, w seconds
    sun_dir: vec4<f32>,      // toward sun, w = lights the world
    sun_color: vec4<f32>,
    moon_dir: vec4<f32>,     // moon A, w = active
    moon_color: vec4<f32>,
    sky_ambient: vec4<f32>,
    ground_ambient: vec4<f32>,
    fog_color: vec4<f32>,    // linear
    fog: vec4<f32>,          // start, end
    grade: vec4<f32>,        // exposure, saturation, contrast, vignette
    flags: vec4<u32>,        // x frame, y bit0 = AO valid
}

// World position of pixel `px` (integer coordinates) with hardware depth `d` (jittered inverse).
fn world_pos(px: vec2<f32>, d: f32) -> vec3<f32> {
    let uv = (px + vec2<f32>(0.5)) * fc.viewport.zw;
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, d, 1.0);
    let w = fc.inv_view_proj * ndc;
    return w.xyz / w.w;
}
// Positive distance along the view axis (what the native vertex fog measures).
fn view_depth(wp: vec3<f32>) -> f32 { return -(fc.view * vec4<f32>(wp, 1.0)).z; }

// Native linear fog `T_native = 1 - clamp((d - start) / (end - start), 0, 1)` (DESIGN section 8): never thinner than the original.
fn native_transmittance(d: f32) -> f32 {
    return 1.0 - clamp((d - fc.fog.x) / max(fc.fog.y - fc.fog.x, 1e-3), 0.0, 1.0);
}

