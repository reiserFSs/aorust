// G-buffer packing helpers shared by the G-buffer pass and every consumer (no FC dependency).
const PI: f32 = 3.14159265;

fn luma(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722)); }

// Octahedral unit-vector packing into two [0,1] channels (G1.rg).
fn oct_encode(n: vec3<f32>) -> vec2<f32> {
    let a = n / (abs(n.x) + abs(n.y) + abs(n.z));
    var e = a.xy;
    if a.z < 0.0 {
        e = (vec2<f32>(1.0) - abs(a.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), a.xy >= vec2<f32>(0.0));
    }
    return e * 0.5 + vec2<f32>(0.5);
}
fn oct_decode(p: vec2<f32>) -> vec3<f32> {
    let f = p * 2.0 - vec2<f32>(1.0);
    var n = vec3<f32>(f.x, f.y, 1.0 - abs(f.x) - abs(f.y));
    let t = clamp(-n.z, 0.0, 1.0);
    n = vec3<f32>(n.xy + select(vec2<f32>(t), vec2<f32>(-t), n.xy >= vec2<f32>(0.0)), n.z);
    return normalize(n);
}

// G0.a material flags (8 bit, stored /255).
const MF_NO_FOG: u32 = 1u;
const MF_GROUND: u32 = 2u;
const MF_BAKED: u32 = 4u;   // lighting is the native baked/unlit colour, carried in G3
const MF_ACTOR: u32 = 8u;
