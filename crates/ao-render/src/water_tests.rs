//! Enhanced water must never replace the faithful liquid draw or retain disabled targets.
use crate::{gpu_stats::GpuStatsExt, *};

#[test]
fn water_switch_roundtrip_preserves_exact_faithful_pixels() {
    use ao_scene::{liquid::{SourceTriangle, Surface}, Blend, Instance, Mesh, Submesh, Vertex, IDENTITY};
    let source = Surface::new(0, 0, vec![SourceTriangle {
        kind: 0xff20_3041,
        vertices: [[100.0, 5.0, -100.0], [140.0, 5.0, -100.0], [120.0, 5.0, -140.0]]
            .map(|pos| Vertex { pos, ..Default::default() }),
    }]).unwrap();
    let camera = Camera::look_at(Vec3::new(120.0, 13.0, -108.0), Vec3::new(120.0, 5.0, -120.0));
    let (mut vertices, mut indices, mut colors) = (Vec::new(), Vec::new(), Vec::new());
    source.geometry(camera.pos.to_array(), false, &mut vertices, &mut indices, &mut colors);
    assert!(vertices.iter().all(|vertex| vertex.color[3] == 1.0), "the hidden-actor oracle requires genuinely opaque native coverage");
    let count = vertices.len() as u32;
    vertices.extend_from_within(..);
    let submeshes = (0..2).map(|layer| Submesh { liquid: true, two_sided: true, blend: Blend::AlphaBlend,
        emissive: [0.3; 3], ..Submesh::new(indices.iter().map(|i| i + count * layer).collect(), None) }).collect();
    let mut environment = default_environment(100.0);
    environment.fog_start = 100.0;
    environment.fog_end = 200.0;
    let scene = Scene { meshes: vec![Mesh { vertices, submeshes }], liquids: vec![source],
        instances: vec![Instance { mesh: 0, transform: IDENTITY }], environment: Some(environment), daylight: Some(1.0), ..Default::default() };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).unwrap();
    renderer.set_msaa_samples(4);
    assert_eq!(renderer.msaa_samples(), 4, "the resolved-min-depth regression requires real 4x MSAA");
    renderer.upload(&scene);
    // Native midpoint ARGB averaging loses low bits even from 255. Use the
    // faithful coarse-water option so this occlusion oracle really is opaque.
    renderer.set_realistic_water(false);
    let target = renderer.device.create_texture_tracked(&wgpu::TextureDescriptor {
        label: Some("water faithful boundary regression"), size: wgpu::Extent3d { width: 48, height: 48, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let capture = |renderer: &mut Renderer| {
        renderer.render(&target.create_view(&Default::default()), &Targets::new(renderer, 48, 48), &camera);
        texture_to_rgba(renderer, &target, 48, 48).unwrap()
    };
    let faithful = capture(&mut renderer);
    renderer.set_enhanced_settings(EnhancedSettings { water_ripples: true, ..Default::default() });
    assert_ne!(capture(&mut renderer), faithful, "flat-water detail ripples must survive the 4x minimum-depth resolve");
    renderer.set_enhanced_settings(EnhancedSettings::default());
    assert_eq!(capture(&mut renderer), faithful);
    renderer.set_enhanced_settings(EnhancedSettings { water_waves: true, water_ripples: true, water_refraction: true,
        water_reflections: true, water_foam: true, water_wakes: true, waterfalls: true, underwater: true,
        water_caustics: true, water_sun_glint: true, ..Default::default() });
    let actor = Scene { meshes: vec![Mesh {
        vertices: [[-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [1.0, 0.0, 1.0], [-1.0, 0.0, 1.0]]
            .map(|pos| Vertex { pos, normal: [0.0, 1.0, 0.0], ..Default::default() }).to_vec(),
        submeshes: vec![Submesh { two_sided: true, emissive: [1.0, 0.0, 0.0],
            ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) }],
    }], ..Default::default() };
    renderer.add_actor_model(1, &actor);
    let hidden = ao_scene::ActorFrame { id: 1, model: 1, swimming: true,
        transform: Mat4::from_translation(Vec3::new(120.0, 4.0, -120.0)).to_cols_array_2d(), ..Default::default() };
    for quality in [1, 2] {
        let settings = EnhancedSettings { water_quality: quality, ..renderer.enhanced_settings() };
        renderer.set_enhanced_settings(settings);
        let baseline = capture(&mut renderer);
        renderer.set_actors(vec![hidden.clone()]);
        assert_eq!(capture(&mut renderer), baseline, "neither water quality may reveal a non-admitted swimmer via reflections, bottom depth or wakes");
        renderer.set_actors(Vec::new());
    }
    assert!(renderer.water.target_bytes() > 0);
    renderer.set_enhanced_settings(EnhancedSettings::default());
    assert_eq!(renderer.water.target_bytes(), 0);
    assert_eq!(capture(&mut renderer), faithful, "disabling every water switch restores exact native bytes");
}
