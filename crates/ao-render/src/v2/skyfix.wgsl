// Resets depth to 1.0 where no geometry wrote the G-buffer (G1.a = 1 marks every G-class surface).
@group(0) @binding(0) var g1t: texture_2d<f32>;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

struct Out { @builtin(frag_depth) depth: f32 }

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> Out {
    if textureLoad(g1t, vec2<i32>(pos.xy), 0).a > 0.0 { discard; }
    return Out(1.0);
}
