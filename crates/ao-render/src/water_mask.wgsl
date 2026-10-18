// Exact native loaded triangles: no infinite water plane or expanded shoreline.
struct Lamp {
    position_range: vec4<f32>, colour: vec4<f32>, attenuation: vec4<f32>,
    direction: vec4<f32>, cone: vec4<f32>,
};
struct Params {
    vp: mat4x4<f32>, inverse_vp: mat4x4<f32>,
    eye_time: vec4<f32>, control: vec4<f32>, fog: vec4<f32>,
    effects: vec4<f32>, sun_direction: vec4<f32>, sun_colour: vec4<f32>,
    sky_colour: vec4<f32>, fog_colour: vec4<f32>,
    raster: vec4<f32>,
    lamps: array<Lamp, 64>,
};
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var scene_depth: texture_depth_2d;
struct VertexOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) shore: f32,
    @location(3) foam: f32,
};
@vertex fn vs(@location(0) world: vec3<f32>, @location(1) normal: vec3<f32>, @location(2) shore: f32, @location(3) foam: f32) -> VertexOut {
    var o: VertexOut;
    o.clip = p.vp * vec4<f32>(world, 1.0);
    o.world = world;
    o.normal = normal;
    o.shore = shore;
    o.foam = foam;
    return o;
}
struct MaskOut {
    @location(0) position: vec4<f32>,
    @location(1) normal: vec4<f32>,
};
@fragment fn fs(v: VertexOut) -> MaskOut {
    // Evaluate derivatives before any discard. Resolved scene depth is the
    // minimum of MSAA samples, whereas this mask evaluates the pixel centre.
    let half_pixel_span = 0.5 * (abs(dpdx(v.clip.z)) + abs(dpdy(v.clip.z)));
    let depth = textureLoad(scene_depth, vec2<i32>(v.clip.xy), 0);
    let reference_depth = max(depth, v.clip.z);
    let roundoff = bitcast<f32>(bitcast<u32>(reference_depth) + 4u) - reference_depth;
    let sample_slack = select(0.0, half_pixel_span, p.raster.x > 1.0);
    // Admit only this triangle's bounded subpixel depth span, not a global
    // bias that could expose water behind an unrelated foreground receiver.
    if v.clip.z > depth + sample_slack + roundoff { discard; }
    var o: MaskOut;
    o.position = vec4<f32>(v.world, 1.0 + clamp(v.foam, 0.0, 1.0));
    o.normal = vec4<f32>(normalize(v.normal), clamp(v.shore, -65504.0, 65504.0));
    return o;
}
