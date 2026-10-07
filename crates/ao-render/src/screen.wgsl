struct Layer {color:vec4<f32>,uv:vec4<f32>};
@group(0) @binding(0) var<uniform> layer:Layer;
@group(0) @binding(1) var image:texture_2d<f32>;
@group(0) @binding(2) var image_sampler:sampler;
struct Out {@builtin(position) pos:vec4<f32>,@location(0) uv:vec2<f32>};
@vertex fn vs(@builtin(vertex_index) i:u32)->Out {
    let p=array<vec2<f32>,3>(vec2(-1.0,-1.0),vec2(3.0,-1.0),vec2(-1.0,3.0));
    var out:Out;out.pos=vec4(p[i],0.0,1.0);
    // Native DS1002eec0: bottom row v=offset, top row v=offset+span.
    out.uv=layer.uv.xy+(p[i]*0.5+vec2(0.5))*layer.uv.zw;
    return out;
}
@fragment fn fs(input:Out)->@location(0) vec4<f32> {
    return textureSample(image,image_sampler,input.uv)*layer.color;
}
