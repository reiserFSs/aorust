// Enhanced v2 lighting resolve: G-buffer + depth -> linear HDR. `common.wgsl` and `packing.wgsl` precede this file.
@group(0) @binding(0) var<uniform> fc: FC;
@group(0) @binding(1) var g0t: texture_2d<f32>;
@group(0) @binding(2) var g1t: texture_2d<f32>;
@group(0) @binding(3) var g3t: texture_2d<f32>;
@group(0) @binding(4) var deptht: texture_depth_2d;
@group(0) @binding(5) var skyt: texture_2d<f32>;
@group(0) @binding(6) var aot: texture_2d<f32>;
@group(0) @binding(7) var out: texture_storage_2d<rgba16float, write>;

fn to_lin(v: vec3<f32>) -> vec3<f32> { return pow(max(v, vec3<f32>(0.0)), vec3<f32>(2.2)); }

// GGX normal distribution with the Smith-GGX visibility folded in (cheap analytic form).
fn ggx_spec(n: vec3<f32>, v: vec3<f32>, l: vec3<f32>, rough: f32) -> f32 {
    let h = normalize(v + l);
    let nh = max(dot(n, h), 0.0);
    let a = rough * rough;
    let a2 = a * a;
    let d = nh * nh * (a2 - 1.0) + 1.0;
    return a2 / (PI * d * d + 1e-5) * 0.25;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= u32(fc.viewport.x) || id.y >= u32(fc.viewport.y) { return; }
    let c = vec2<i32>(id.xy);
    let depth = textureLoad(deptht, c, 0);
    if depth >= 1.0 {
        textureStore(out, c, vec4<f32>(textureLoad(skyt, c, 0).rgb, 1.0));
        return;
    }
    let g0 = textureLoad(g0t, c, 0);
    let g1 = textureLoad(g1t, c, 0);
    let emission = textureLoad(g3t, c, 0).rgb;
    let flags = u32(g0.a * 255.0 + 0.5);
    let albedo = to_lin(g0.rgb);
    let n = oct_decode(g1.rg);
    let wp = world_pos(vec2<f32>(id.xy), depth);
    let vd = view_depth(wp);
    let v = normalize(fc.eye.xyz - wp);

    var ao = 1.0;
    if (fc.flags.y & 1u) != 0u { ao = textureLoad(aot, c, 0).r; }

    var light = vec3<f32>(0.0);
    var spec = vec3<f32>(0.0);
    let ndl = max(dot(n, fc.sun_dir.xyz), 0.0);
    if fc.sun_dir.w > 0.5 && ndl > 0.0 {
        let vis = 1.0;
        light += fc.sun_color.rgb * (ndl * vis);
        spec += fc.sun_color.rgb * (ndl * vis * 0.04 * PI * ggx_spec(n, v, fc.sun_dir.xyz, clamp(g1.b, 0.12, 1.0)));
    }
    var ambient = mix(fc.ground_ambient.rgb, fc.sky_ambient.rgb, n.y * 0.5 + 0.5);
    if (flags & MF_GROUND) != 0u { ambient *= g1.b * 2.0; } // baked terrain lightmap share
    light += ambient * ao;

    var col = albedo * light + spec + emission;
    if (flags & MF_NO_FOG) == 0u {
        col = mix(fc.fog_color.rgb, col, native_transmittance(vd));
    }
    textureStore(out, c, vec4<f32>(col, 1.0));
}
