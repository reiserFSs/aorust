//! Standalone GGX must gate owner admission before native-eight and 64-source caps.
use crate::{Camera, EnhancedSettings, Renderer, Targets, Vec3, texture_to_rgba};
use ao_scene::{ActorFrame, Instance, Light, Mesh, Scene, Submesh, Vertex, IDENTITY};

fn quad(corners: [[f32; 3]; 4]) -> Mesh {
    Mesh {
        vertices: corners.into_iter().map(|pos| Vertex { pos, normal: [0.0, 0.0, 1.0], color: [1.0; 4], ..Default::default() }).collect(),
        submeshes: vec![Submesh { two_sided: true, ..Submesh::new(vec![0, 1, 2, 0, 2, 3], None) }],
    }
}

#[test]
fn standalone_ggx_hidden_owners_cannot_emit_or_evict_visible_highlights() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut renderer = Renderer::new(&instance, None).expect("standalone GGX admission regression GPU");
    renderer.set_msaa_samples(1);
    let mut metal = quad([[-1.5, -1.5, -3.0], [1.5, -1.5, -3.0], [1.5, 1.5, -3.0], [-1.5, 1.5, -3.0]]);
    metal.submeshes[0].base_color = [0.0, 0.0, 0.0, 1.0];
    metal.submeshes[0].specular = [0.2; 3];
    metal.submeshes[0].shininess = 32.0;
    let scene = Scene { meshes: vec![metal], instances: vec![Instance { mesh: 0, transform: IDENTITY }], ..Default::default() };
    let load = |renderer: &mut Renderer, scene: &Scene| {
        renderer.upload(scene);
        renderer.env.sky_color = [0.0; 3];
        renderer.env.sun_color = [0.0; 3];
        renderer.env.ambient = [0.0; 3];
        renderer.env.fog_color = [0.0; 3];
        renderer.env.fog_start = 100.0;
        renderer.env.fog_end = 120.0;
    };
    load(&mut renderer, &scene);
    let body = Scene { meshes: vec![quad([[-0.8, -0.15, -2.0], [-0.6, -0.15, -2.0],
        [-0.6, 0.15, -2.0], [-0.8, 0.15, -2.0]])], ..Default::default() };
    renderer.add_actor_model(7, &body);
    let owner = ActorFrame { id: 7, model: 7, character: true, ..Default::default() };
    renderer.set_actors(vec![owner.clone()]);
    let admitted = Light { pos: [0.0, 0.0, -1.0], color: [0.4; 3], range: 10.0, owner: Some(7), ..Default::default() };
    renderer.set_dynamic_lights(&[admitted]);
    let targets = Targets::new(&renderer, 64, 64);
    let output = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("standalone GGX owner admission"), size: wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
    });
    let view = output.create_view(&Default::default());
    let camera = Camera::look_at(Vec3::ZERO, -Vec3::Z);
    let capture = |renderer: &mut Renderer| {
        renderer.render(&view, &targets, &camera);
        texture_to_rgba(renderer, &output, 64, 64).unwrap()
    };
    let center = |image: &[u8]| -> u32 {
        (30..34).flat_map(|y| (30..34).map(move |x| (y * 64 + x) * 4))
            .map(|pixel| image[pixel..pixel + 3].iter().map(|&v| u32::from(v)).sum::<u32>()).sum()
    };
    let faithful = capture(&mut renderer);
    let standalone = EnhancedSettings { material_specular: true, ..Default::default() };
    renderer.set_enhanced_settings(standalone);
    renderer.set_dynamic_lights(&[]);
    let absent = capture(&mut renderer);
    let hidden_owner = ActorFrame { id: 99, model: 7, character: true,
        transform: crate::Mat4::from_translation(Vec3::new(0.0, 0.0, -10.0)).to_cols_array_2d(), ..Default::default() };
    renderer.set_actors(vec![hidden_owner, owner.clone()]);
    let mut hidden_prefix = vec![Light { owner: Some(99), ..admitted }; 65];
    renderer.set_dynamic_lights(&hidden_prefix);
    assert_eq!(absent, capture(&mut renderer), "GGX-only hidden ownership leaked diffuse or specular light");
    hidden_prefix.push(admitted);
    renderer.set_dynamic_lights(&hidden_prefix);
    let prefixed = capture(&mut renderer);
    assert!(center(&prefixed) > center(&absent) + 16 * 3 * 8, "the admitted lamp must produce an actual authored highlight");
    renderer.set_actors(vec![owner]);
    renderer.set_dynamic_lights(&[admitted]);
    assert_eq!(prefixed, capture(&mut renderer), "hidden sources consumed the native-eight, moving-prefix or 64-source budget");
    renderer.set_enhanced_settings(EnhancedSettings::default());
    renderer.set_dynamic_lights(&[admitted]);
    assert_eq!(faithful, capture(&mut renderer), "GGX admission changed the all-off native roundtrip");
    assert!(renderer.point_shadows.is_none() && renderer.sun_shadows.is_none());
    assert_eq!(renderer.dyn_light_buf.size(), (crate::MAX_DYN_LIGHTS * 4 * 16) as u64, "GGX-only left the enhanced GPU pool resident");

    // Static owned lights must not evict a live source through the native grid's
    // sixteen-entry cell cap before the enhanced GGX pool sees it.
    renderer.set_actors(Vec::new());
    renderer.set_enhanced_settings(standalone);
    let visible_static = Light { owner: None, ..admitted };
    let mut static_scene = scene.clone();
    static_scene.lights = vec![visible_static];
    load(&mut renderer, &static_scene);
    renderer.set_dynamic_lights(&[]);
    let static_control = capture(&mut renderer);
    assert!(center(&static_control) > 16 * 3 * 8);
    static_scene.lights = vec![Light { owner: Some(99), ..visible_static }; 65];
    static_scene.lights.push(visible_static);
    load(&mut renderer, &static_scene);
    assert_eq!(static_control, capture(&mut renderer), "hidden static owners consumed the cell or 64-source cap");
    assert!(!renderer.enhanced_settings().clustered_lighting && !renderer.enhanced_settings().local_light_shadows);
    renderer.set_enhanced_settings(EnhancedSettings::default());
    assert!(renderer.point_shadows.is_none() && renderer.sun_shadows.is_none());
    assert_eq!(renderer.dyn_light_buf.size(), (crate::MAX_DYN_LIGHTS * 4 * 16) as u64);
}
