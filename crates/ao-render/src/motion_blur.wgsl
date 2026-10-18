// Gather structure adapted from ao-vk rvk/shaders/motion_blur.frag.
// MIT, Copyright (c) 2026 wannabuh; see THIRD_PARTY.
// https://github.com/wannabuh/ao-vk/tree/a0dc47ea2c853d26a4491d5a25ef0331f714b6ce
// Exact camera/object vectors are produced by the renderer, never actor heuristics.
@group(0) @binding(3) var motion_vectors: texture_2d<f32>;
@group(0) @binding(4) var identities: texture_2d<u32>;
@fragment fn fs(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let p = vec2<i32>(position.xy); let original = textureLoad(image,p,0);
    let vector = textureLoad(motion_vectors,p,0);
    if vector.w > 0.0 || any(vector.xy != vector.xy) || vector.z < 0.0 || vector.z > 1.0 { return original; }
    let id = textureLoad(identities,p,0).r;
    if id == 0u { return original; }
    var motion = vector.xy*vec2<f32>(textureDimensions(image))*params.effect.z;
    let distance = length(motion);
    if distance < 0.5 || distance > 100000.0 { return original; }
    motion *= min(1.0,24.0/distance);
    let z = view_z(p);
    var sum = original.rgb; var weight = 1.0;
    for (var i = 0u; i < 12u; i += 1u) {
        let t = (f32(i)+0.5)/12.0-0.5;
        let q = coord(p+vec2<i32>(round(motion*t)));
        if fog_fraction(q) < fog_fraction(p) { continue; }
        if textureLoad(identities,q,0).r != id { continue; }
        let sample_motion = textureLoad(motion_vectors,q,0);
        if sample_motion.w > 0.0 || abs(view_z(q)-z) > max(0.01,z*0.01) { continue; }
        sum += colour(q); weight += 1.0;
    }
    return safe_output(sum/weight,original.rgb,original.a,p);
}
