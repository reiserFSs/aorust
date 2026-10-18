// Adapted from ao-vk rvk/shaders/fxaa.frag, MIT, Copyright (c) 2026 wannabuh.
// https://github.com/wannabuh/ao-vk/tree/a0dc47ea2c853d26a4491d5a25ef0331f714b6ce
// See THIRD_PARTY. SpatialPost prepends bindings, guarded loads and fullscreen vertex.
@fragment fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy); let uv = position.xy-vec2(0.5);
    let original = textureLoad(image,p,0);
    let nw = luma(linear_sample(uv+vec2(-0.5,-0.5),p));
    let ne = luma(linear_sample(uv+vec2(0.5,-0.5),p));
    let sw = luma(linear_sample(uv+vec2(-0.5,0.5),p));
    let se = luma(linear_sample(uv+vec2(0.5,0.5),p));
    let lm = luma(original.rgb);
    let low = min(lm,min(min(nw,ne),min(sw,se)));
    let high = max(lm,max(max(nw,ne),max(sw,se)));
    if high-low < max(0.0312,high*0.125) { return original; }
    let direction = vec2(-((nw+ne)-(sw+se)),(nw+sw)-(ne+se));
    let reduce = max((nw+ne+sw+se)*0.03125,1.0/128.0);
    let span = clamp(direction/(min(abs(direction.x),abs(direction.y))+reduce),vec2(-8.0),vec2(8.0));
    let a = 0.5*(linear_sample(uv-span/6.0,p)+linear_sample(uv+span/6.0,p));
    let b = a*0.5+0.25*(linear_sample(uv-span*0.5,p)+linear_sample(uv+span*0.5,p));
    let lb = luma(b);
    return safe_output(select(b,a,lb<low || lb>high),original.rgb,original.a,p);
}
