//! Focused source-capacity fairness regression; no installed assets required.
use super::*;

fn build_on_gpu(renderer: &crate::Renderer, records: &[[[f32; 4]; 5]], visible: bool) -> Vec<[f32; 4]> {
    let mut input = vec![[0.0; 4]; 2];
    input[0][0] = f32::from_bits(records.len() as u32);
    input.extend(records.iter().flatten().copied());
    let candidates = renderer.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
        label: Some("cluster overflow test candidates"), contents: bytemuck::cast_slice(&input), usage: wgpu::BufferUsages::STORAGE,
    });
    let owners = renderer.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor {
        label: Some("cluster overflow owner proof"), contents: bytemuck::cast_slice(&[0u32, u32::from(visible)]), usage: wgpu::BufferUsages::STORAGE,
    });
    let size = (OUTPUT_ROWS * 16) as u64;
    let output = renderer.device.create_buffer_tracked(&wgpu::BufferDescriptor {
        label: Some("cluster overflow test output"), size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
    });
    renderer.queue.write_buffer(&output, 0, bytemuck::cast_slice(&[[0.125f32, 0.25, 0.5, 1.0]; PREFIX]));
    let readback = renderer.device.create_buffer_tracked(&wgpu::BufferDescriptor {
        label: Some("cluster overflow test readback"), size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false,
    });
    let bind = renderer.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
        label: Some("cluster overflow test"), layout: &renderer.cluster_programs.layout,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: output.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: candidates.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 2, resource: owners.as_entire_binding() }],
    });
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("cluster overflow regression"), timestamp_writes: None });
        pass.set_pipeline(&renderer.cluster_programs.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, size);
    renderer.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    readback.slice(..).map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    renderer.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = readback.slice(..).get_mapped_range();
    bytemuck::cast_slice(&mapped).to_vec()
}

#[test]
fn gpu_hidden_owned_source_cannot_evict_visible_sixty_fourth_light() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let renderer = crate::Renderer::new(&instance, None).expect("cluster admission regression requires GPU");
    let mut lights = Vec::new();
    for index in 0..64u32 {
        lights.push([[10.0 + index as f32, 0.0, 0.0, 100.0], [0.5, 0.5, 0.5, 2.0], [1.0, 0.0, 0.0, 0.0], [0.0; 4],
            [0.0, f32::from_bits(index), 0.0, 0.0]]);
    }
    let absent = build_on_gpu(&renderer, &lights, false);
    let mut hidden = lights[0];
    hidden[0][0] = 0.0;
    hidden[3][3] = -64.0; // exact owner visibility group 1, no shadow slot
    hidden[4] = [f32::from_bits(1), 0.0, f32::from_bits(123), 0.0];
    lights.push(hidden);
    let occluded = build_on_gpu(&renderer, &lights, false);
    assert_eq!(bytemuck::cast_slice::<_, u8>(&absent), bytemuck::cast_slice::<_, u8>(&occluded), "hidden sources must not change selection, grid bounds, cells or local-light records");
    let admitted = build_on_gpu(&renderer, &lights, true);
    assert_eq!(admitted[COUNT_ROW][0].to_bits(), 64);
    assert_eq!(admitted[HEADER][0], 0.0, "positive control: the visible carried source is selected");
    assert_eq!(absent[HEADER + 63 * 4][0], 73.0);
    assert_eq!(admitted[HEADER + 63 * 4][0], 72.0, "only an admitted source may consume the final slot");
    let meta = admitted[PREFIX + 1].map(f32::to_bits);
    assert!(meta[0] * meta[1] * meta[2] <= MAX_CELLS as u32);
    assert_eq!(meta[3] as usize, CELL_ROW);
}

#[test]
fn gpu_hidden_first_sixteen_moving_sources_do_not_evict_later_visible_source() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let renderer = crate::Renderer::new(&instance, None).expect("moving-source admission regression requires GPU");
    let visible = [[30.0, 0.0, 0.0, 100.0], [0.4, 0.4, 0.4, 2.0], [1.0, 0.0, 0.0, 0.0], [0.0; 4],
        [f32::from_bits(1), f32::from_bits(16), 0.0, 0.0]];
    let absent = build_on_gpu(&renderer, &[visible], false);
    let mut sources = Vec::new();
    for index in 0..16u32 {
        let mut hidden = visible;
        hidden[0][0] = index as f32;
        hidden[3][3] = -64.0;
        hidden[4] = [f32::from_bits(1), f32::from_bits(index), f32::from_bits(123), f32::from_bits(index)];
        sources.push(hidden);
    }
    sources.push(visible);
    let occluded = build_on_gpu(&renderer, &sources, false);
    assert_eq!(bytemuck::cast_slice::<_, u8>(&absent), bytemuck::cast_slice::<_, u8>(&occluded), "hidden earlier moving sources cannot displace a later visible source or alter spatial clusters");
    assert_eq!(&occluded[..PREFIX], &[[0.125, 0.25, 0.5, 1.0]; PREFIX], "GPU cluster construction must preserve the faithful prefix verbatim");
    assert_eq!(occluded[HEADER][0], 30.0);
    assert_eq!(occluded[COUNT_ROW][0].to_bits(), 1);
    let admitted = build_on_gpu(&renderer, &sources, true);
    assert_eq!(admitted[COUNT_ROW][0].to_bits(), 17, "positive control: all admitted moving candidates survive beyond the old sixteen-source ceiling");
}

fn function(source: &str, name: &str) -> String {
    let start = source.find(&format!("fn {name}(")).unwrap();
    let end = start + source[start..].find("\n}").unwrap() + 2;
    source[start..end].to_owned()
}

fn numeric_pixels(renderer: &crate::Renderer, source: String) -> Vec<u8> {
    let shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("clustered local-light numeric controls"), source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = renderer.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("clustered local-light numeric controls"), layout: None,
        vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"),
            targets: &[Some(wgpu::ColorTargetState { format: renderer.format, blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
    });
    let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("clustered numeric pixels"), size: wgpu::Extent3d { width: 2, height: 1, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let view = texture.create_view(&Default::default());
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })], ..Default::default()
        });
        pass.set_pipeline(&pipeline);
        pass.draw(0..3, 0..1);
    }
    renderer.queue.submit([encoder.finish()]);
    crate::texture_to_rgba(renderer, &texture, 2, 1).unwrap()
}

const NUMERIC_VERTEX: &str = r#"
@vertex fn vs(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}
"#;

#[test]
fn gpu_clustered_texture_alpha_emission_survives_local_override() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let renderer = crate::Renderer::new(&instance, None).expect("cluster emission regression requires GPU");
    let generated = crate::enhanced_shader::source(crate::EnhancedSettings { clustered_lighting: true, ..Default::default() });
    let shade = function(&generated, "shade");
    let source = format!(r#"
struct VOut {{ uv:vec2<f32>, color:vec4<f32>, fade:f32, effect:u32, light:vec4<f32>, spec:vec4<f32>, tint:vec4<f32>, fog:f32, env_uv:vec2<f32> }}
struct Material {{ scroll:vec4<f32>, color:vec4<f32>, emissive:vec4<f32>, native:vec4<f32>, flags:vec4<f32> }}
struct Globals {{ fog:vec4<f32>, fog_g:vec4<f32> }}
var<private> g:Globals;
var<private> mat:Material;
var<private> alpha_sample:f32;
const tex:u32=0u;
const env_tex:u32=1u;
fn sample_clamped(t:u32, uv:vec2<f32>)->vec4<f32>{{return vec4<f32>(1.0,1.0,1.0,alpha_sample);}}
fn native_unlit()->bool{{return false;}}
fn native_alpha_test()->bool{{return false;}}
fn native_alpha_pass(alpha:f32)->bool{{return true;}}
fn native_fog(factor:f32)->f32{{return factor;}}
fn to_g(value:vec3<f32>)->vec3<f32>{{return value;}}
fn clustered_receiver(i:VOut, light:vec3<f32>)->vec3<f32>{{return vec3<f32>(0.25);}}
fn clustered_receiver_spec(i:VOut, spec:vec3<f32>)->vec3<f32>{{return vec3<f32>(0.0);}}
{shade}
{NUMERIC_VERTEX}
@fragment fn fs(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32>{{
    alpha_sample=select(0.5,0.0,p.x>=1.0);
    mat.color=vec4<f32>(1.0); mat.emissive.w=1.0;
    var i:VOut; i.color=vec4<f32>(1.0); i.tint=vec4<f32>(1.0); i.fade=1.0;
    return shade(i,0u);
}}
"#);
    let pixels = numeric_pixels(&renderer, source);
    assert_eq!(&pixels[..4], &[191,191,191,255], "the native alpha self-illumination mask adds after the clustered light");
    assert_eq!(&pixels[4..], &[64,64,64,255], "zero-alpha positive control retains only the overridden local light");
}

#[test]
fn gpu_clustered_ground_subtracts_directional_shadow_once() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let renderer = crate::Renderer::new(&instance, None).expect("cluster ground regression requires GPU");
    let generated = crate::enhanced_shader::source(crate::EnhancedSettings { clustered_lighting: true, sun_shadows:true, ..Default::default() });
    let ground = function(&generated, "enhanced_original_fs_ground_fused");
    let helper = function(&generated, "clustered_ground");
    let source = format!(r#"
struct VOut {{ wpos:vec3<f32>, n:vec3<f32>, clip:vec4<f32>, color:vec4<f32>, light:vec4<f32>, sun_share:vec4<f32> }}
struct Globals {{ ambient_g:vec4<f32>, fog_g:vec4<f32> }}
var<private> g:Globals;
var<private> clustered_normal:vec3<f32>;
var<private> visibility:f32;
fn sun_visibility(p:vec3<f32>, n:vec3<f32>, pixel:vec2<f32>)->f32{{return visibility;}}
fn clustered_pixel_lights(p:vec3<f32>, n:vec3<f32>, footprint:f32)->vec3<f32>{{return vec3<f32>(0.2);}}
fn clustered_night_light(i:VOut,n:vec3<f32>,ground:bool,light:vec3<f32>,retained:vec3<f32>)->vec3<f32>{{return light;}}
fn ground_palette(i:VOut)->vec3<f32>{{return vec3<f32>(0.0);}}
fn ground_tile(i:VOut)->vec3<f32>{{return vec3<f32>(1.0);}}
fn ground_fog(i:VOut)->f32{{return 0.0;}}
{helper}
{ground}
{NUMERIC_VERTEX}
@fragment fn fs(@builtin(position) p:vec4<f32>)->@location(0) vec4<f32>{{
    clustered_normal=vec3<f32>(0.0,1.0,0.0); visibility=select(0.0,1.0,p.x>=1.0);
    var i:VOut; i.color=vec4<f32>(0.6); i.sun_share=vec4<f32>(0.4); i.n=clustered_normal;
    return enhanced_original_fs_ground_fused(i);
}}
"#);
    let pixels = numeric_pixels(&renderer, source);
    assert_eq!(&pixels[..4], &[102,102,102,255], "0.6 base minus 0.4 direct shadow plus 0.2 local = 0.4, not zero");
    assert_eq!(&pixels[4..], &[204,204,204,255], "unshadowed positive control retains the same local light");
}
