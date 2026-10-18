//! Adversarial production render-path coverage; no duplicate temporal algorithm.
use super::*;
use ao_scene::ActorFrame;
use std::sync::Arc;

struct Capture {
    targets: Targets,
    output: wgpu::Texture,
    size: (u32, u32),
}
impl Capture {
    fn new(r: &Renderer, size: (u32, u32)) -> Self {
        let output = r.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("post temporal regression"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self { targets: Targets::new(r, size.0, size.1), output, size }
    }
    fn pixels(&self, r: &mut Renderer, camera: &Camera) -> Vec<u8> {
        r.render(&self.output.create_view(&Default::default()), &self.targets, camera);
        texture_to_rgba(r, &self.output, self.size.0, self.size.1).unwrap()
    }
}

fn renderer() -> Renderer {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut r = Renderer::new(&instance, None).expect("post regression requires a headless GPU");
    r.set_msaa_samples(1);
    r.upload(&Scene::default());
    r.env.ambient = [0.8; 3];
    r.env.sun_color = [0.0; 3];
    r.env.fog_start = 100.0;
    r.env.fog_end = 120.0;
    r
}

fn model(color: [f32; 4]) -> Scene {
    let vertices = [[-0.8, -0.9, 0.0], [0.8, -0.9, 0.0], [0.8, 0.9, 0.0], [-0.8, 0.9, 0.0]]
        .map(|pos| ao_scene::Vertex { pos, normal: [0.0, 0.0, 1.0], uv: [0.0; 2], color }).to_vec();
    Scene {
        meshes: vec![ao_scene::Mesh { vertices, submeshes: vec![ao_scene::Submesh {
            two_sided: true, ..ao_scene::Submesh::new(vec![0, 1, 2, 0, 2, 3], None)
        }] }],
        ..Default::default()
    }
}

fn actor(model: u64, x: f32, skin: Option<Arc<Vec<ao_scene::Vertex>>>) -> ActorFrame {
    ActorFrame { id: 7, model, transform: Mat4::from_translation(Vec3::new(x, 0.0, 0.0)).to_cols_array_2d(), skin, ..Default::default() }
}

// Raster-boundary coverage legitimately changes under jitter. Uniform 5x5
// reference interiors still must carry this frame's surface, never an old actor.
fn assert_interiors(actual: &[u8], expected: &[u8], size: (u32, u32), label: &str) {
    let (w, h) = (size.0 as usize, size.1 as usize);
    let mut checked = 0;
    for y in 2..h - 2 {
        for x in 2..w - 2 {
            let i = (y * w + x) * 4;
            if (y - 2..=y + 2).all(|yy| (x - 2..=x + 2).all(|xx| {
                (0..3).all(|c| expected[(yy * w + xx) * 4 + c].abs_diff(expected[i + c]) <= 1)
            })) {
                checked += 1;
                for c in 0..3 {
                    assert!(actual[i + c].abs_diff(expected[i + c]) <= 2,
                        "{label}: stale surface at ({x},{y}) channel {c}: {} != {}", actual[i + c], expected[i + c]);
                }
            }
        }
    }
    assert!(checked > w * h / 2, "{label}: insufficient uniform interior coverage");
}

#[test]
fn post_target_uses_first_frame_settings_before_grade_and_bloom_allocation() {
    let camera = Camera::look_at(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO);
    let dof = EnhancedSettings {
        depth_of_field: true, dof_aperture: 2.0, dof_max_radius: 8.0, ..Default::default()
    };
    let mut r = renderer();
    for (settings, needs_target) in [
        (EnhancedSettings { hdr: true, ..dof }, true),
        (EnhancedSettings { emissive_bloom: true, bloom_strength: 0.5, ..dof }, true),
        (dof, false),
    ] {
        r.set_enhanced_settings(settings);
        r.post_target = None;
        assert!(r.grade.is_none() && r.bloom.is_none(), "effect resources have not been allocated yet");
        r.prepare_post_frame((64, 32), &camera, Mat4::IDENTITY, [0.0; 3]);
        assert_eq!(r.post_target.is_some(), needs_target,
            "first-frame target selection must use requested grade/bloom settings, not allocated resources");
    }
}

#[test]
fn post_temporal_moving_rigid_and_skinned_disappearance_and_model_identity() {
    let size = (192, 128);
    let camera = Camera::look_at(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO);
    let red = model([0.9, 0.12, 0.08, 1.0]);
    let green = model([0.08, 0.85, 0.15, 1.0]);
    let taa = EnhancedSettings { taa: true, ..Default::default() };
    for mode in ["rigid", "cpu_skin", "native_skin"] {
        let mut source = red.clone();
        if mode == "native_skin" {
            source.native_skin = Some(Arc::new(source.meshes[0].vertices.iter().map(|v| ao_scene::NativeSkinVertex {
                local0: v.pos, local1: [0.0; 3], weight: 1.0, bone0: 0, bone1: u32::MAX, normal: v.normal,
            }).collect()));
        }
        let mut r = renderer();
        r.add_actor_model(1, &source);
        r.add_actor_model(2, &green);
        r.set_enhanced_settings(taa);
        let capture = Capture::new(&r, size);
        let mut reference = renderer();
        reference.add_actor_model(1, &source);
        reference.add_actor_model(2, &green);
        reference.set_enhanced_settings(taa);
        let control = Capture::new(&reference, size);
        let mut previous = Vec::new();
        let mut changed = 0;
        for frame in 0..16 {
            let x = -1.2 + frame as f32 * 0.16;
            let skin = (mode == "cpu_skin").then(|| Arc::new(red.meshes[0].vertices.iter().map(|v| {
                let mut v = *v;
                v.pos[0] += v.pos[1] * (frame as f32 * 0.23).sin() * 0.35;
                v
            }).collect()));
            let mut current = actor(1, x, skin);
            if mode == "native_skin" {
                let shear = (frame as f32 * 0.23).sin() * 0.35;
                current.skin_pose = Some(Arc::new(vec![ao_scene::SkinBone {
                    rows: [[1.0, shear, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]],
                }]));
            }
            r.set_actors(vec![current.clone()]);
            let pixels = capture.pixels(&mut r, &camera);
            reference.reset_post_history();
            reference.set_actors(vec![current]);
            let expected = control.pixels(&mut reference, &camera);
            assert_interiors(&pixels, &expected, size, mode);
            changed += usize::from(!previous.is_empty() && previous != pixels);
            previous = pixels;
        }
        assert_eq!(changed, 15, "the rendered actor must actually move every frame");
        // Same actor ID now names a different model at the last location.
        r.set_actors(vec![actor(2, 1.2, None)]);
        reference.reset_post_history();
        reference.set_actors(vec![actor(2, 1.2, None)]);
        let replaced = capture.pixels(&mut r, &camera);
        let expected = control.pixels(&mut reference, &camera);
        assert_interiors(&replaced, &expected, size, "same-ID model replacement");
        assert_ne!(replaced, previous, "model replacement must change the actual rendered frame");
        // Reusing both logical ID and model key must still retire the old GPU identity.
        r.add_actor_model(2, &red);
        reference.add_actor_model(2, &red);
        reference.reset_post_history();
        r.set_actors(vec![actor(2, 1.2, None)]);
        reference.set_actors(vec![actor(2, 1.2, None)]);
        assert_interiors(&capture.pixels(&mut r, &camera), &control.pixels(&mut reference, &camera), size, "same-key model replacement");
        // Empty submission must remove every trace immediately, including edges.
        r.set_actors(vec![]);
        reference.reset_post_history();
        reference.set_actors(vec![]);
        assert_eq!(capture.pixels(&mut r, &camera), control.pixels(&mut reference, &camera), "disappeared actor must leave no ghost");
    }
}

#[test]
fn post_temporal_camera_cut_resize_reset_and_all_off_are_clean() {
    let mut r = renderer();
    let source = model([0.7, 0.4, 0.15, 1.0]);
    r.add_actor_model(1, &source);
    let taa = EnhancedSettings { taa: true, ..Default::default() };
    r.set_enhanced_settings(taa);
    let size = (192, 128);
    let capture = Capture::new(&r, size);
    let camera = Camera::look_at(Vec3::new(0.0, 0.0, 6.0), Vec3::ZERO);
    for frame in 0..12 {
        r.set_actors(vec![actor(1, frame as f32 * 0.03, None)]);
        capture.pixels(&mut r, &camera);
    }
    // Teleport beyond the temporal cut threshold while retaining a visible actor.
    let cut = Camera::look_at(Vec3::new(0.0, 0.0, 16.0), Vec3::ZERO);
    let mut cold = renderer();
    cold.add_actor_model(1, &source);
    cold.set_enhanced_settings(taa);
    cold.set_actors(vec![actor(1, 0.33, None)]);
    let control = Capture::new(&cold, size);
    assert_eq!(capture.pixels(&mut r, &cut), control.pixels(&mut cold, &cut), "camera teleport must discard old history");
    let resized = Capture::new(&r, (256, 160));
    let resized_control = Capture::new(&cold, (256, 160));
    cold.reset_post_history();
    assert_eq!(resized.pixels(&mut r, &cut), resized_control.pixels(&mut cold, &cut), "resize must start fresh history");
    for _ in 0..3 { resized.pixels(&mut r, &cut); }
    r.reset_post_history();
    cold.reset_post_history();
    assert_eq!(resized.pixels(&mut r, &cut), resized_control.pixels(&mut cold, &cut), "explicit history reset must start fresh");
    r.set_post_focus(Some([0.0, 0.0, 0.0]));
    r.set_enhanced_settings(EnhancedSettings { taa: true, fxaa: true, cas: true, motion_blur: 0.5, depth_of_field: true, ..Default::default() });
    resized.pixels(&mut r, &cut);
    r.set_enhanced_settings(EnhancedSettings::default());
    cold.set_enhanced_settings(EnhancedSettings::default());
    assert_eq!(resized.pixels(&mut r, &cut), resized_control.pixels(&mut cold, &cut), "all-off must be byte-identical after temporal/spatial effects");
    r.set_enhanced_settings(taa);
    cold.set_enhanced_settings(taa);
    assert_eq!(resized.pixels(&mut r, &cut), resized_control.pixels(&mut cold, &cut), "re-enabling must not resurrect history");
}

struct MotionProbe {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    output: wgpu::Texture,
    size: (u32, u32),
}

impl MotionProbe {
    fn new(r: &Renderer, size: (u32, u32)) -> Self {
        let layout = r.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("motion regression probe inputs"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            ],
        });
        let shader = r.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("motion regression attachment visualization"),
            source: wgpu::ShaderSource::Wgsl(r#"
@group(0) @binding(0) var motion: texture_2d<f32>;
@group(0) @binding(1) var identity: texture_2d<u32>;
@vertex fn vs(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    let x = f32((vertex << 1u) & 2u);
    let y = f32(vertex & 2u);
    return vec4<f32>(vec2<f32>(x, y) * 2.0 - vec2<f32>(1.0), 0.0, 1.0);
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(p.xy);
    let m = textureLoad(motion, pixel, 0);
    let id = textureLoad(identity, pixel, 0).x;
    // Preserve the low-byte surface identity, amplify subpixel velocity,
    // and expose the reactive flag independently. No temporal algorithm here.
    return vec4<f32>(abs(m.xy) * 4096.0, f32(id & 255u) / 255.0, m.w);
}
"#.into()),
        });
        let pipeline_layout = r.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("motion regression probe"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let pipeline = r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("motion regression probe"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"),
                compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(),
            multiview_mask: None, cache: None,
        });
        let output = r.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("motion regression diagnostic RGBA8"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        Self { layout, pipeline, output, size }
    }

    fn pixels(&self, r: &Renderer) -> Vec<u8> {
        let motion = r.motion_vectors.as_ref().expect("production motion attachments");
        let bindings = r.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("motion regression current attachments"), layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(motion.motion_view()) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(motion.identity_view()) },
            ],
        });
        let view = self.output.create_view(&Default::default());
        let mut encoder = r.device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("motion regression attachment readback"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view, resolve_target: None, depth_slice: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bindings, &[]);
            pass.draw(0..3, 0..1);
        }
        r.queue.submit([encoder.finish()]);
        texture_to_rgba(r, &self.output, self.size.0, self.size.1).unwrap()
    }
}

#[test]
fn post_temporal_static_sloped_msaa_motion_excludes_jitter() {
    let size = (192, 128);
    let camera = Camera::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO);
    for samples in [1, 4] {
        let mut r = renderer();
        r.set_msaa_samples(samples);
        assert_eq!(r.msaa_samples(), samples, "motion regression requires actual {samples}x MSAA");
        let mut slope = model([0.3, 0.5, 0.7, 1.0]);
        for vertex in &mut slope.meshes[0].vertices {
            vertex.pos[0] *= 1.7;
            vertex.pos[1] *= 1.4;
            vertex.pos[2] = vertex.pos[0] * 0.7;
        }
        slope.instances.push(ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY });
        r.upload(&slope);
        r.set_enhanced_settings(EnhancedSettings { taa: true, ..Default::default() });
        let capture = Capture::new(&r, size);
        let probe = MotionProbe::new(&r, size);
        let interior: Vec<_> = (44..84).flat_map(|y| (72..120).map(move |x| ((y * size.0 + x) * 4) as usize)).collect();
        let mut baseline = Vec::new();
        for frame in 0..12 {
            capture.pixels(&mut r, &camera);
            let diagnostic = probe.pixels(&r);
            if frame == 0 { continue; }
            // This close plane slopes strongly enough that per-sample native
            // depths differ far more than the former 2e-6 resolved-depth guard.
            let admitted = interior.iter().filter(|&&i| diagnostic[i + 2] != 0).count();
            assert_eq!(admitted, interior.len(), "{samples}x frame {frame}: sloped interior lost identity");
            for &i in &interior {
                assert_eq!(diagnostic[i + 3], 0, "{samples}x frame {frame}: static interior became reactive");
                assert!(diagnostic[i] <= 1 && diagnostic[i + 1] <= 1,
                    "{samples}x frame {frame}: projection jitter leaked into motion: {:?}", &diagnostic[i..i + 4]);
            }
            baseline = diagnostic;
        }
        let covered = baseline.as_chunks::<4>().0.iter().filter(|pixel| pixel[2] != 0).count();
        assert!(covered > (size.0 * size.1 / 4) as usize, "{samples}x: insufficient sloped-quad motion coverage");
        let mut moving = model([0.9, 0.1, 0.2, 1.0]);
        for vertex in &mut moving.meshes[0].vertices {
            vertex.pos[0] *= 0.3;
            vertex.pos[1] *= 0.3;
        }
        r.add_actor_model(1, &moving);
        let moving_actor = |x| {
            let mut frame = actor(1, x, None);
            frame.transform = Mat4::from_translation(Vec3::new(x, 0.0, 1.2)).to_cols_array_2d();
            frame
        };
        r.set_actors(vec![moving_actor(-0.1)]);
        capture.pixels(&mut r, &camera);
        r.set_actors(vec![moving_actor(0.1)]);
        capture.pixels(&mut r, &camera);
        let moving_pixels = probe.pixels(&r);
        let moving_samples = interior.iter().filter(|&&i| moving_pixels[i + 2] != baseline[i + 2]
            && moving_pixels[i + 3] == 0 && moving_pixels[i] > 10).count();
        assert!(moving_samples > 100, "{samples}x: moving actor must have valid nonzero physical velocity");
        // Explicit retirement can respawn the same logical ID and model without
        // an intervening empty submission. Its history belongs to a new generation.
        let center = ((size.1 / 2 * size.0 + size.0 / 2) * 4) as usize;
        let old_identity = moving_pixels[center + 2];
        assert_ne!(old_identity, 0, "{samples}x: retirement probe must cover the actual actor");
        assert_ne!(old_identity, baseline[center + 2], "{samples}x: retirement probe hit static background");
        r.retire_actor_visuals(&[(7, 1)]);
        r.set_actors(vec![moving_actor(0.1)]);
        capture.pixels(&mut r, &camera);
        let respawned = probe.pixels(&r);
        let new_identity = respawned[center + 2];
        assert_ne!(new_identity, 0, "{samples}x: replacement must render a real surface");
        assert_ne!(new_identity, old_identity, "{samples}x: explicit same-ID/model retirement reused history identity");
        assert_eq!(respawned[center + 3], 255, "{samples}x: first replacement frame must reject old history");
        r.set_actors(vec![moving_actor(0.1)]);
        capture.pixels(&mut r, &camera);
        let established = probe.pixels(&r);
        assert_eq!(established[center + 2], new_identity, "{samples}x: replacement identity must remain stable");
        assert_eq!(established[center + 3], 0, "{samples}x: second replacement frame must have valid history");
        r.set_actors(vec![]);
        capture.pixels(&mut r, &camera);
        let disappeared = probe.pixels(&r);
        for &i in &interior {
            assert_eq!(disappeared[i + 2], baseline[i + 2], "{samples}x: disappeared actor left a surface identity");
            assert_eq!(disappeared[i + 3], 0, "{samples}x: exposed static surface became reactive");
            assert!(disappeared[i] <= 1 && disappeared[i + 1] <= 1, "{samples}x: hidden actor left velocity");
        }
    }
}

#[test]
fn post_temporal_native_fog_accumulates_and_color_density_change_rejects_history() {
    let size = (192, 128);
    let camera = Camera::look_at(Vec3::new(0.0, 0.0, 3.0), Vec3::ZERO);
    let mut vertices = Vec::new();
    let mut indices = Vec::new();
    for y in 0..12 {
        for x in 0..12 {
            let left = -1.4 + x as f32 * 2.8 / 12.0;
            let bottom = -1.1 + y as f32 * 2.2 / 12.0;
            let intensity = if (x + y) % 2 == 0 { 0.15 } else { 0.85 };
            let first = vertices.len() as u32;
            for [px, py] in [[left, bottom], [left + 2.8 / 12.0, bottom],
                [left + 2.8 / 12.0, bottom + 2.2 / 12.0], [left, bottom + 2.2 / 12.0]] {
                vertices.push(ao_scene::Vertex { pos: [px, py, px * 0.3],
                    normal: [0.0, 0.0, 1.0], uv: [0.0; 2], color: [intensity, intensity, intensity, 1.0] });
            }
            indices.extend([first, first + 1, first + 2, first, first + 2, first + 3]);
        }
    }
    let scene = Scene {
        meshes: vec![ao_scene::Mesh { vertices, submeshes: vec![ao_scene::Submesh {
            two_sided: true, ..ao_scene::Submesh::new(indices, None)
        }] }],
        instances: vec![ao_scene::Instance { mesh: 0, transform: ao_scene::IDENTITY }],
        ..Default::default()
    };
    let settings = EnhancedSettings { taa: true, ..Default::default() };
    let mut accumulated = renderer();
    let mut current_only = renderer();
    for r in [&mut accumulated, &mut current_only] {
        r.upload(&scene);
        r.env.fog_start = 0.1;
        r.env.fog_end = 10.0;
        r.env.fog_color = [0.18, 0.25, 0.32];
        r.set_enhanced_settings(settings);
    }
    let capture = Capture::new(&accumulated, size);
    let control = Capture::new(&current_only, size);
    let interior: Vec<_> = (40..88).flat_map(|y| (64..128).map(move |x| ((y * size.0 + x) * 4) as usize)).collect();
    let mut history_changes = 0;
    for frame in 0..16 {
        // Keep exactly the same projection-jitter phase, geometry and fog.
        // Invalidating only production motion poses forces per-surface current
        // colour without resetting the control's temporal jitter counter.
        current_only.reset_motion_vectors();
        let actual = capture.pixels(&mut accumulated, &camera);
        let cold = control.pixels(&mut current_only, &camera);
        if frame > 1 {
            history_changes += interior.iter().filter(|&&i|
                actual[i..i + 3] != cold[i..i + 3]).count();
        }
    }
    assert!(history_changes > 0, "native-fogged checker interiors must accumulate TAA, not silently bypass it");
    // Both the stronger extinction and new fog colour apply immediately. The
    // old weaker/differently coloured atmosphere must not survive this reset.
    for r in [&mut accumulated, &mut current_only] {
        r.env.fog_end = 4.0;
        r.env.fog_color = [0.45, 0.12, 0.28];
    }
    current_only.reset_motion_vectors();
    assert_eq!(capture.pixels(&mut accumulated, &camera), control.pixels(&mut current_only, &camera),
        "stronger native fog with a new colour must equal the current-only rendered control byte-for-byte");
}

#[test]
fn post_temporal_actor_admission_change_erases_receiver_shadow_history() {
    let quad = |points: [[f32; 3]; 4], normal| ao_scene::Mesh {
        vertices: points.into_iter().map(|pos| ao_scene::Vertex {
            pos, normal, uv: [0.0; 2], color: [1.0; 4],
        }).collect(),
        submeshes: vec![ao_scene::Submesh { two_sided: true,
            ..ao_scene::Submesh::new(vec![0, 1, 2, 0, 2, 3], None) }],
    };
    let mut scene = Scene {
        meshes: vec![
            quad([[-8.0, 0.0, -8.0], [8.0, 0.0, -8.0], [8.0, 0.0, 8.0], [-8.0, 0.0, 8.0]], [0.0, 1.0, 0.0]),
            quad([[-2.0, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 4.0, 0.0], [-2.0, 4.0, 0.0]], [0.0, 0.0, 1.0]),
        ],
        instances: (0..2).map(|mesh| ao_scene::Instance { mesh, transform: ao_scene::IDENTITY }).collect(),
        ..Default::default()
    };
    // A uniform ground could clip stale shadows away without an admission reset.
    // Fine native texture variation keeps current receiver variance nonzero.
    let checker = ao_scene::TextureKey { rdb_type: 0, id: 0x706f_7374 };
    scene.textures.insert(checker, ao_scene::Texture {
        source: Default::default(), width: 64, height: 64,
        rgba: (0..64).flat_map(|y| (0..64).flat_map(move |x| {
            let v = if (x + y) % 2 == 0 { 80 } else { 240 };
            [v, v, v, 255]
        })).collect(),
    });
    scene.meshes[0].submeshes[0].texture = Some(checker);
    for (vertex, uv) in scene.meshes[0].vertices.iter_mut().zip([[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]]) {
        vertex.uv = uv;
    }
    let caster = Scene { meshes: vec![
        quad([[-0.4, 0.0, -1.0], [0.4, 0.0, -1.0], [0.4, 1.8, -1.0], [-0.4, 1.8, -1.0]], [0.0, 0.0, 1.0]),
    ], ..Default::default() };
    let settings = EnhancedSettings { taa: true, sun_shadows: true, ..Default::default() };
    let mut accumulated = renderer();
    let mut current_only = renderer();
    for r in [&mut accumulated, &mut current_only] {
        r.upload(&scene);
        r.add_actor_model(1, &caster);
        r.shadow_sun = Some([-0.8, 0.5, -0.3]);
        r.env.sun_dir = [-0.8, 0.5, -0.3];
        r.env.sun_color = [0.6; 3];
        r.env.ambient = [0.2; 3];
        r.set_enhanced_settings(settings);
    }
    let size = (384, 256);
    let camera = Camera::look_at(Vec3::new(0.0, 3.0, 8.0), Vec3::new(0.0, 1.0, 0.0));
    let capture = Capture::new(&accumulated, size);
    let control = Capture::new(&current_only, size);
    let mut absent_frames = Vec::new();
    // Hidden-only allocation/slot growth must not affect visible history. Each
    // renderer follows the same jitter sequence from its very first frame.
    for frame in 0..16 {
        accumulated.set_actors((0..=frame).map(|n| ActorFrame {
            id: 100 + n, model: 1, ..Default::default()
        }).collect());
        current_only.set_actors(vec![]);
        let hidden = capture.pixels(&mut accumulated, &camera);
        let absent = control.pixels(&mut current_only, &camera);
        assert_eq!(hidden, absent, "hidden-only actor slot growth changed temporal frame {frame}");
        absent_frames.push(absent);
    }
    let aspect = size.0 as f32 / size.1 as f32;
    let projection = Mat4::perspective_rh(accumulated.lens.vertical_fov(aspect), aspect,
        accumulated.lens.near, accumulated.lens.far.unwrap_or(132.0));
    let receiver = (projection * Mat4::look_to_rh(camera.pos, camera.forward(), camera.up()))
        .project_point3(Vec3::new(-3.5, 0.0, -0.4375));
    let x = ((receiver.x * 0.5 + 0.5) * size.0 as f32) as usize;
    let y = ((0.5 - receiver.y * 0.5) * size.1 as f32) as usize;
    assert!(x > 2 && x + 2 < size.0 as usize && y > 2 && y + 2 < size.1 as usize);
    let center = (y * size.0 as usize + x) * 4;
    let visible = ActorFrame { id: 7, model: 1,
        transform: Mat4::from_translation(Vec3::new(-5.0, 0.0, 0.0)).to_cols_array_2d(), ..Default::default() };
    let mut shadowed = Vec::new();
    for _ in 0..16 {
        accumulated.set_actors(vec![visible.clone()]);
        current_only.set_actors(vec![visible.clone()]);
        current_only.reset_motion_vectors();
        shadowed = capture.pixels(&mut accumulated, &camera);
        control.pixels(&mut current_only, &camera);
    }
    let energy = |image: &[u8]| image[center..center + 3].iter().map(|&v| u32::from(v)).sum::<u32>();
    assert!(energy(&shadowed) < energy(&absent_frames[15]),
        "visible actor must really cast a shadow on an unchanged world receiver");
    // One case removes the actor; the other moves it behind the opaque wall.
    // Rewarm before each event so both begin with real receiver shadow history.
    for hidden_instead_of_removed in [false, true] {
        if hidden_instead_of_removed {
            for _ in 0..16 {
                accumulated.set_actors(vec![visible.clone()]);
                current_only.set_actors(vec![visible.clone()]);
                current_only.reset_motion_vectors();
                capture.pixels(&mut accumulated, &camera);
                control.pixels(&mut current_only, &camera);
            }
        }
        let actors = if hidden_instead_of_removed {
            vec![ActorFrame { id: 7, model: 1, ..Default::default() }]
        } else { vec![] };
        accumulated.set_actors(actors.clone());
        current_only.set_actors(actors);
        current_only.reset_motion_vectors();
        let cleared = capture.pixels(&mut accumulated, &camera);
        let expected = control.pixels(&mut current_only, &camera);
        for yy in y - 2..=y + 2 {
            for xx in x - 2..=x + 2 {
                let i = (yy * size.0 as usize + xx) * 4;
                assert_eq!(&cleared[i..i + 4], &expected[i..i + 4],
                    "admission event hidden={hidden_instead_of_removed} left actor shadow history on receiver ({xx},{yy})");
            }
        }
    }
}
