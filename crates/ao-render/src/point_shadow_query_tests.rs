//! Execute the generated BRDF loops, counting visibility calls without cube sampling.
use super::*;
use wgpu::util::DeviceExt;

fn function(source: &str, name: &str) -> String {
    let start = source.find(&format!("fn {name}(")).unwrap();
    let end = start + source[start..].find("\n}").unwrap() + 2;
    source[start..end].to_owned()
}

const GUARDED_QUERY: &str = "        if dot(n, l) > 0.0 {\n            intensity *= point_shadow_visibility(-d, a.w, dyn_lights[li + 3u].w, point_receiver_character);\n        }";

#[test]
fn point_query_guard_preserves_admission_and_all_three_accumulations() {
    for clustered_lighting in [false, true] {
        let generated = crate::enhanced_shader::source(EnhancedSettings {
            clustered_lighting, material_specular: true, local_light_shadows: true, ..Default::default()
        });
        for name in ["clustered_pixel_lights", "clustered_native_spec", "authored_cluster_spec"] {
            let body = function(&generated, name);
            assert_eq!(body.matches(GUARDED_QUERY).count(), 1);
            let admission = body.find("if !owner_light_admitted(dyn_lights[li + 3u].w) { continue; }").unwrap();
            let query = body.find(GUARDED_QUERY).unwrap();
            assert!(admission < query);
            assert!(body[query + GUARDED_QUERY.len()..].starts_with("\n        diffuse += "));
            assert_eq!(body.contains("used += 1u;"), !clustered_lighting);
        }
        assert!(function(&generated, "point_shadow_visibility").contains("!owner_light_admitted(packed_slot)"));
    }
}

#[test]
fn gpu_point_backface_query_rejection_is_bit_exact_with_admission() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let renderer = Renderer::new(&instance, None).expect("point query regression requires GPU");
    for clustered_lighting in [false, true] {
        let generated = crate::enhanced_shader::source(EnhancedSettings {
            clustered_lighting, material_specular: true, local_light_shadows: true, ..Default::default()
        });
        let mut loops = String::new();
        for name in ["clustered_pixel_lights", "clustered_native_spec", "authored_cluster_spec"] {
            let body = function(&generated, name);
            assert_eq!(body.matches(GUARDED_QUERY).count(), 1);
            // One runtime-controlled loop prevents independent function compilation
            // from reassociating front-face GGX arithmetic differently (observed 1 ULP).
            // Storage supplies false/true, so the compiler cannot specialize either run.
            loops.push_str(&body.replace("if dot(n, l) > 0.0 {", "if dot(n, l) > 0.0 || force_queries {"));
            loops.push('\n');
        }
        let owner = function(&generated, "owner_light_admitted");
        let ggx = function(&generated, "authored_ggx");
        let source = format!(r#"
struct Globals {{ eye:vec4<f32>, dyn_info:vec4<f32> }}
var<private> g:Globals;
var<private> dyn_lights:array<vec4<f32>, 80>;
var<private> point_receiver_character:bool;
var<private> query_count:u32;
var<private> test_visibility:f32;
var<private> force_queries:bool;
@group(0) @binding(0) var<storage,read> point_actor_visibility:array<u32>;
@group(0) @binding(1) var<storage,read_write> output:array<vec4<u32>>;
{owner}
{ggx}
fn point_shadow_visibility(delta:vec3<f32>, range:f32, packed:f32, character:bool)->f32 {{
    query_count += 1u;
    return test_visibility;
}}
{loops}
@compute @workgroup_size(1) fn probe(@builtin(global_invocation_id) id:vec3<u32>) {{
    let test_index = id.x;
    // Front, back, tangent, zero normal, and coincident receiver/light.
    let facing = test_index % 5u;
    var n = vec3<f32>(0.0, 0.0, 1.0);
    if facing == 1u {{ n = -n; }}
    if facing == 2u {{ n = vec3<f32>(1.0, 0.0, 0.0); }}
    if facing == 3u {{ n = vec3<f32>(0.0); }}
    let ownership = (test_index / 5u) % 3u;
    let variant = test_index / 15u;
    g.eye = vec4<f32>(0.0, 0.0, 5.0, 0.0);
    dyn_lights[64u] = vec4<f32>(-10.0, -10.0, -10.0, 20.0);
    dyn_lights[65u] = bitcast<vec4<f32>>(vec4<u32>(1u, 1u, 1u, 70u));
    dyn_lights[70u] = bitcast<vec4<f32>>(vec4<u32>(71u, 1u, 0u, 0u));
    dyn_lights[71u] = vec4<f32>(0.0);
    dyn_lights[66u] = vec4<f32>(0.0, 0.0, select(2.0, 0.0, facing == 4u), 10.0);
    // Include black light, zero visibility, spotlight and both attenuation modes.
    dyn_lights[67u] = vec4<f32>(select(vec3<f32>(0.4, 0.2, 0.1), vec3<f32>(0.0), variant == 2u), select(2.0, 0.5, variant == 3u));
    dyn_lights[68u] = vec4<f32>(select(1.0, -1.0, variant == 1u), 0.1, 0.01, 1.0);
    dyn_lights[69u] = vec4<f32>(0.0, 0.0, -1.0, select(select(1.0, -65.0, ownership == 1u), -97.0, ownership == 2u));
    test_visibility = select(select(0.375, 0.0, variant == 1u), 1.0, variant == 2u);
    let alpha = select(0.02, 1.0, variant == 1u);
    let power = select(16.0, 0.0, variant == 3u);
    let p = vec3<f32>(0.0);
    for (var brdf = 0u; brdf < 3u; brdf += 1u) {{
        query_count = 0u;
        force_queries = point_actor_visibility[0u] != 0u;
        var actual:vec3<f32>;
        if brdf == 0u {{ actual = clustered_pixel_lights(p, n, 0.1); }}
        if brdf == 1u {{ actual = clustered_native_spec(p, n, 0.1, power); }}
        if brdf == 2u {{ actual = authored_cluster_spec(p, n, 0.1, alpha, vec3<f32>(0.04, 0.0, 1.0)); }}
        let actual_count = query_count;
        query_count = 0u;
        force_queries = point_actor_visibility[1u] != 0u;
        var reference:vec3<f32>;
        if brdf == 0u {{ reference = clustered_pixel_lights(p, n, 0.1); }}
        if brdf == 1u {{ reference = clustered_native_spec(p, n, 0.1, power); }}
        if brdf == 2u {{ reference = authored_cluster_spec(p, n, 0.1, alpha, vec3<f32>(0.04, 0.0, 1.0)); }}
        let row = (test_index * 3u + brdf) * 2u;
        output[row] = vec4<u32>(bitcast<vec3<u32>>(actual), actual_count);
        output[row + 1u] = vec4<u32>(bitcast<vec3<u32>>(reference), query_count);
    }}
}}
"#);
        let device = &renderer.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("generated point query count oracle"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("point backface exact regression"), layout: None, module: &shader,
            entry_point: Some("probe"), compilation_options: Default::default(), cache: None,
        });
        let owners = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("visible and hidden point owners"), contents: bytemuck::cast_slice(&[0u32, 1, 0]), usage: wgpu::BufferUsages::STORAGE,
        });
        let size = 60 * 3 * 2 * 16;
        let output = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point shading and actual query counts"), size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false,
        });
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("point query readback"), size, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: &pipeline.get_bind_group_layout(0), entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: owners.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: output.as_entire_binding() },
            ],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(60, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
        renderer.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let mapped = readback.slice(..).get_mapped_range();
        let rows: &[[u32; 4]] = bytemuck::cast_slice(&mapped);
        for case in 0..60 {
            let admitted = (case / 5) % 3 != 2;
            for brdf in 0..3 {
                let row = (case * 3 + brdf) * 2;
                assert_eq!(rows[row][..3], rows[row + 1][..3], "shading changed: clustered={clustered_lighting}, case={case}, brdf={brdf}");
                assert!(rows[row][..3].iter().all(|word| f32::from_bits(*word).is_finite()));
                assert_eq!(rows[row][3], u32::from(admitted && case % 5 == 0), "guarded query count: case={case}, brdf={brdf}");
                assert_eq!(rows[row + 1][3], u32::from(admitted), "reference query count: case={case}, brdf={brdf}");
                if !admitted || case % 5 != 0 || case / 15 == 2 || (brdf == 1 && case / 15 == 3) {
                    assert_eq!(rows[row][..3], [0; 3], "zero contribution retained");
                }
            }
        }
    }
}
