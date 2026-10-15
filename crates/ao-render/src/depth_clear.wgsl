// Full-viewport native depth clear: all samples, no color writes.
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4(p[index], 1.0, 1.0);
}
@fragment fn fs() -> @location(0) vec4<f32> {
    return vec4(0.0);
}
