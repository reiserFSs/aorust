// 2D quad pipeline for ao-gui draw lists (see gui.rs). Coordinates are window pixels.
struct Globals {
    screen: vec2<f32>,
    srgb: f32,
    _pad: f32,
};
@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var gfx_tex: texture_2d_array<f32>;
@group(0) @binding(2) var glyph_tex: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

struct VIn {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    // bits 0..15: atlas page, bit 16: 1 = glyph coverage, 0 = skin image
    @location(3) kind: u32,
};
struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) kind: u32,
};

@vertex
fn vs(v: VIn) -> VOut {
    var o: VOut;
    o.clip = vec4<f32>(v.pos.x / g.screen.x * 2.0 - 1.0, 1.0 - v.pos.y / g.screen.y * 2.0, 0.0, 1.0);
    o.uv = v.uv;
    o.color = v.color;
    o.kind = v.kind;
    return o;
}

fn decode(c: vec3<f32>) -> vec3<f32> {
    // exact sRGB decode: the GUI is authored in display (gamma) space; an sRGB target re-encodes on write
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs(i: VOut) -> @location(0) vec4<f32> {
    var rgb: vec3<f32>;
    var a: f32;
    if ((i.kind >> 16u) == 1u) {
        let cov = textureSampleLevel(glyph_tex, samp, i.uv, 0.0).r;
        rgb = i.color.rgb;
        a = cov * i.color.a;
    } else if ((i.kind >> 16u) == 2u) {
        rgb = i.color.rgb;
        a = i.color.a;
    } else {
        let t = textureSampleLevel(gfx_tex, samp, i.uv, i.kind & 0xffffu, 0.0);
        rgb = t.rgb * i.color.rgb;
        a = t.a * i.color.a;
    }
    if (g.srgb > 0.5) {
        rgb = decode(rgb);
    }
    return vec4<f32>(rgb, a);
}
