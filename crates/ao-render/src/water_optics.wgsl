// Current-frame optics over the already opaque/native-liquid scene. No history,
// actor lists, pre-liquid colour, or hidden geometry is sampled.
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
@group(0) @binding(2) var scene: texture_2d<f32>;
@group(0) @binding(3) var positions: texture_2d<f32>;
@group(0) @binding(4) var normals: texture_2d<f32>;
@group(0) @binding(5) var bottom_scene_depth: texture_depth_2d;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
}
fn world_at(pixel: vec2<i32>, depth: f32) -> vec3<f32> {
    let uv = (vec2<f32>(pixel) + 0.5) / vec2<f32>(textureDimensions(scene));
    let h = p.inverse_vp * vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), depth, 1.0);
    return h.xyz / h.w;
}
@fragment fn copy(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(scene, vec2<i32>(pos.xy), 0);
}
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(pos.xy);
    let original = textureLoad(scene, pixel, 0);
    if p.fog.w > 0.0 {
        // Membership is tested on the CPU against the exact finite triangles.
        // Absorption never boosts any original channel or removes native fog.
        let d = textureLoad(scene_depth, pixel, 0);
        var distance = 60.0;
        if d < 1.0 { distance = min(length(world_at(pixel, d) - p.eye_time.xyz), 60.0); }
        let absorb = exp(-vec3<f32>(0.035, 0.017, 0.012) * distance);
        return vec4<f32>(original.rgb * absorb, original.a);
    }
    let mask = textureLoad(positions, pixel, 0);
    if mask.w == 0.0 || p.fog.z == 0.0 { return original; }
    let distance = length(mask.xyz - p.eye_time.xyz);
    let fog_share = clamp((p.fog.y - distance) / max(p.fog.y - p.fog.x, 0.001), 0.0, 1.0);
    if fog_share <= 0.0 { return original; }
    let n = textureLoad(normals, pixel, 0);
    // Cached source classification, independent of displaced wave normals.
    // Negative packed shore encodes -(distance below the connected lip + 1).
    let waterfall = n.w < 0.0;
    if waterfall && p.effects.z == 0.0 { return original; }
    var normal = normalize(n.xyz);
    if p.control.w > 0.0 {
        let xz = mask.xz;
        let t = p.eye_time.w;
        let slope = vec2<f32>(sin(dot(xz, vec2<f32>(0.69, 0.23)) + t * 0.8), cos(dot(xz, vec2<f32>(-0.31, 0.47)) - t * 0.63));
        normal = normalize(normal + vec3<f32>(slope.x, 0.0, slope.y) * 0.075);
    }
    var colour = original.rgb;
    if p.sun_direction.w > 0.0 || p.control.w > 0.0 {
        // Broad water BRDF response, independent of the optional sharp glint.
        // Modulate the faithful water radiance rather than replacing its opacity.
        let view = normalize(p.eye_time.xyz - mask.xyz);
        let sky_energy = dot(max(p.sky_colour.rgb, vec3<f32>(0.0)), vec3<f32>(0.2126, 0.7152, 0.0722)) * p.sky_colour.w;
        let sun_energy = dot(max(p.sun_colour.rgb, vec3<f32>(0.0)), vec3<f32>(0.2126, 0.7152, 0.0722)) * p.sun_colour.w;
        var sun_delta = 0.0;
        var flat_sun = 0.0;
        if length(p.sun_direction.xyz) > 0.001 {
            let light = normalize(p.sun_direction.xyz);
            flat_sun = max(light.y, 0.0);
            sun_delta = max(dot(normal, light), 0.0) - flat_sun;
        }
        let sky_delta = abs(view.y) - abs(dot(normal, view));
        let irradiance_delta = sun_delta * sun_energy + sky_delta * sky_energy;
        let flat_irradiance = flat_sun * sun_energy + sky_energy;
        let response = clamp(1.0 + 3.0 * irradiance_delta / max(flat_irradiance, 0.0001), 0.65, 1.35);
        colour *= response;
    }
    let dimensions = vec2<i32>(textureDimensions(scene));
    if p.control.x > 0.0 {
        let offset = vec2<i32>(normal.xz * 5.0);
        let tap = clamp(pixel + offset, vec2<i32>(0), dimensions - 1);
        let tap_mask = textureLoad(positions, tap, 0);
        // Reject dry/foreground taps and a different overlapping liquid body.
        if tap_mask.w > 0.0 && abs(tap_mask.y - mask.y) < 0.2 {
            let refracted = min(textureLoad(scene, tap, 0).rgb, original.rgb);
            colour = mix(colour, refracted, 0.16 * clamp(n.w, 0.0, 1.0));
        }
        let bottom_depth = textureLoad(bottom_scene_depth, pixel, 0);
        var path = 0.0;
        if bottom_depth < 1.0 {
            let bottom = world_at(pixel, bottom_depth);
            if bottom.y < mask.y { path = min(length(bottom - mask.xyz), 60.0); }
        }
        let absorption = exp(-vec3<f32>(0.075, 0.035, 0.022) * path);
        colour *= mix(vec3<f32>(0.985, 0.993, 0.997), absorption, p.control.z);
    }
    if p.control.y > 0.0 {
        let incident = normalize(mask.xyz - p.eye_time.xyz);
        let ray = reflect(incident, normal);
        // Authored static native sky only: no hidden geometry/probe/history.
        let sky = mix(p.fog_colour.rgb, p.sky_colour.rgb, sqrt(clamp(ray.y, 0.0, 1.0)));
        var reflected = mix(original.rgb, max(sky, vec3<f32>(0.0)), p.sky_colour.w);
        var previous_delta = -1.0;
        for (var i = 1; i <= 24; i += 1) {
            let t = 0.15 + 0.085 * f32(i * i);
            let q = mask.xyz + normal * 0.06 + ray * t;
            let clip = p.vp * vec4<f32>(q, 1.0);
            if clip.w <= 0.0 { break; }
            let ndc = clip.xyz / clip.w;
            let uv = ndc.xy * vec2<f32>(0.5, -0.5) + 0.5;
            if any(uv <= vec2<f32>(0.0)) || any(uv >= vec2<f32>(1.0)) || ndc.z <= 0.0 || ndc.z >= 1.0 { break; }
            let hit = vec2<i32>(uv * vec2<f32>(dimensions));
            let d = textureLoad(scene_depth, hit, 0);
            if d >= 1.0 { continue; }
            let receiver = world_at(hit, d);
            let delta = dot(q - receiver, normalize(receiver - p.eye_time.xyz));
            // A bounded crossing test avoids reflecting nearer foreground
            // silhouettes merely because the march passed behind them.
            if previous_delta < 0.0 && delta >= 0.0 && delta < 0.2 + t * 0.015 && textureLoad(positions, hit, 0).w == 0.0 {
                reflected = textureLoad(scene, hit, 0).rgb;
                break;
            }
            previous_delta = delta;
        }
        let fresnel = 0.02 + 0.28 * pow(1.0 - abs(dot(-incident, normal)), 5.0);
        let reflection = mix(min(reflected, original.rgb), reflected, p.control.z);
        colour = mix(colour, reflection, fresnel * clamp(n.w, 0.0, 1.0));
    }
    if p.effects.x > 0.0 {
        // Concentrate actual native sun on the visible submerged receiver;
        // absorption and the final native fog share still attenuate the web.
        let d = textureLoad(bottom_scene_depth, pixel, 0);
        if d < 1.0 {
            let bottom = world_at(pixel, d);
            let water_depth = mask.y - bottom.y;
            if water_depth > 0.05 && water_depth < 8.0 {
                let a = sin(bottom.x * 1.7 + sin(bottom.z * 1.2 + p.eye_time.w));
                let b = sin(bottom.z * 1.9 - sin(bottom.x * 1.1 - p.eye_time.w * 0.7));
                let web = pow(1.0 - min(abs(a), abs(b)), 6.0);
                colour *= 0.92 + 0.08 * web;
                let transmitted_sun = max(p.sun_colour.rgb, vec3<f32>(0.0)) * p.sun_colour.w * exp(-water_depth * 0.35);
                colour += transmitted_sun * web * 0.08;
            }
        }
    }
    if (p.effects.y > 0.0 || (p.effects.z > 0.0 && waterfall)) && p.sun_colour.w > 0.0 && length(p.sun_direction.xyz) > 0.001 {
        let light = normalize(p.sun_direction.xyz);
        let view = normalize(p.eye_time.xyz - mask.xyz);
        let halfway = light + view;
        if length(halfway) > 0.001 {
            let glint = pow(max(dot(normal, normalize(halfway)), 0.0), 192.0);
            // Actual native daylight sun radiance; final native fog share
            // attenuates this contribution with every other optical delta.
            colour += max(p.sun_colour.rgb, vec3<f32>(0.0)) * (glint * 0.12);
        }
    }
    if p.effects.z > 0.0 && waterfall {
        // Source-classified sheets carry negative encoded lip distance.
        let along = max(-n.w - 1.0, 0.0);
        let fall_tau = (sqrt(9.0 + 19.6 * along) - 3.0) / 9.8;
        let phase = fall_tau - p.eye_time.w;
        let columns = 0.5 + 0.5 * sin(mask.x * 9.0 + mask.z * 7.0);
        let lumps = 0.5 + 0.5 * sin(phase * 7.54 + columns * 3.0);
        let strand = pow(clamp(columns * 0.7 + lumps * 0.3, 0.0, 1.0), 3.0);
        colour *= 0.86 + 0.14 * strand;
    }
    if p.effects.y > 0.0 || (p.effects.z > 0.0 && waterfall) {
        let view = normalize(p.eye_time.xyz - mask.xyz);
        // Only caller-admitted current native lights. Fixed bound; no actor
        // discovery or temporal lighting state in this shader.
        for (var i = 0u; i < min(u32(p.fog_colour.w), 64u); i += 1u) {
            let lamp = p.lamps[i];
            let to_light = lamp.position_range.xyz - mask.xyz;
            let d = length(to_light);
            if d <= 0.001 || d >= lamp.position_range.w { continue; }
            let light = to_light / d;
            var attenuation = max(0.0, 1.0 - d / lamp.position_range.w);
            if any(lamp.attenuation.xyz != vec3<f32>(0.0)) {
                attenuation = 1.0 / max(dot(lamp.attenuation.xyz, vec3<f32>(1.0, d, d * d)), 0.0001);
            }
            if lamp.direction.w > 0.0 {
                let cosine = dot(-light, lamp.direction.xyz);
                attenuation *= clamp((cosine - lamp.cone.y) / max(lamp.cone.x - lamp.cone.y, 0.00001), 0.0, 1.0);
            }
            let halfway = light + view;
            if length(halfway) > 0.001 {
                let glint = pow(max(dot(normal, normalize(halfway)), 0.0), 128.0);
                colour += max(lamp.colour.rgb, vec3<f32>(0.0)) * (glint * attenuation * 0.12);
            }
        }
    }
    if p.effects.w > 0.0 && !waterfall {
        // Full-resolution shore separation avoids sparse vertex-only foam.
        let d = textureLoad(bottom_scene_depth, pixel, 0);
        if d < 1.0 {
            let separation = length(world_at(pixel, d) - mask.xyz);
            let shore = max(1.0 - smoothstep(0.05, 0.65, separation), 1.0 - smoothstep(0.0, 1.2, n.w));
            let bubbles = 0.5 + 0.5 * sin(mask.x * 13.0 + sin(mask.z * 11.0) + p.eye_time.w * 1.5);
            let illumination = max(p.sky_colour.rgb, vec3<f32>(0.0)) * p.sky_colour.w + max(p.sun_colour.rgb, vec3<f32>(0.0)) * p.sun_colour.w;
            let whitewater = mix(illumination, vec3<f32>(dot(illumination, vec3<f32>(0.2126, 0.7152, 0.0722))), 0.65);
            let foam = whitewater * 0.9;
            colour = mix(colour, foam, shore * smoothstep(0.35, 0.75, bubbles) * 0.55);
        }
    }
    if p.effects.w > 0.0 || p.effects.z > 0.0 {
        let churn = clamp(mask.w - 1.0, 0.0, 1.0);
        let churn_irradiance = max(p.sky_colour.rgb, vec3<f32>(0.0)) * p.sky_colour.w + max(p.sun_colour.rgb, vec3<f32>(0.0)) * p.sun_colour.w;
        let churn_light = mix(churn_irradiance, vec3<f32>(dot(churn_irradiance, vec3<f32>(0.2126, 0.7152, 0.0722))), 0.65) * 0.9;
        colour = mix(colour, churn_light, churn * 0.7);
    }
    // Native richer colour remains at realism zero; realism controls the
    // physical absorption/reflection blend above, not a single effect gain.
    // Attenuate only the optical delta: the faithful scene's fog stays intact.
    return vec4<f32>(original.rgb + (colour - original.rgb) * fog_share, original.a);
}
