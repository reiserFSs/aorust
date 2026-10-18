//! Enhanced two-pass foliage with blended, non-depth-writing silhouette coverage.
use super::{Blend, PipelineSpec, PipelineSlot, Pipelines};

fn eligible(spec: PipelineSpec) -> bool {
    spec.blend == Blend::AlphaBlend && spec.descriptor().depth_write
        && spec.effect == 0 && !spec.sky && !spec.env && !spec.fade && !spec.actor
}

/// Preserve every native index; paired slots occupy a separate suffix.
pub(super) fn extend(pipes: &mut Pipelines) {
    let native = pipes.len();
    for index in 0..native {
        let spec = pipes.spec(index);
        for soft in [false, true] {
            pipes.push(if eligible(spec) {
                PipelineSlot::new(PipelineSpec {
                    effect: if spec.sprite { 20 } else { 18 } + u32::from(soft),
                    ..spec
                })
            } else { PipelineSlot::alias(index) });
        }
    }
}

pub(super) fn pipeline(pipes: &Pipelines, native: usize, soft: bool) -> Option<usize> {
    let base = pipes.len() / 3;
    (native < base && eligible(pipes.spec(native)))
        .then_some(base + native * 2 + usize::from(soft))
}

/// Called after Enhanced attachment wrappers are generated. Reuse their complete
/// output calculation; only paired leaf silhouettes relax hard alpha thresholds.
pub(super) fn append(source: &mut String) {
    source.push_str(&crate::shader_source::load("foliage_edges.wgsl", include_str!("foliage_edges.wgsl")));
    // A native GREATER/GREATEREQUAL alpha test otherwise hard-cuts the very
    // edge this pass is meant to soften. Only paired plant draws use this
    // generated copy; unusual alpha comparisons and every other shader stay native.
    let start = source.find("fn shade(").expect("native shaded foliage");
    let end = start + source[start..].find("\n}").expect("native shade end") + 2;
    let shade = source[start..end].replacen("fn shade(", "fn foliage_edge_shade(", 1)
        .replace(
            "if native_alpha_test() && !native_alpha_pass(alpha) {",
            "if native_alpha_test() && select(!native_alpha_pass(alpha), alpha <= 0.0, u32(mat.native.w) == 5u || u32(mat.native.w) == 7u) {",
        );
    source.push('\n');
    source.push_str(&shade);
    // The root guard must use the same native comparison relaxation as shade;
    // otherwise positive soft-edge alpha would be killed before it reaches it.
    let start = source.find("fn early_reject(").expect("generated early coverage guard");
    let end = start + source[start..].find("\n}").expect("early coverage guard end") + 2;
    let reject = source[start..end].replacen("fn early_reject(", "fn foliage_edge_reject(", 1)
        .replace(
            "if native_alpha_test() && !native_alpha_pass(alpha) {",
            "if native_alpha_test() && select(!native_alpha_pass(alpha), alpha <= 0.0, u32(mat.native.w) == 5u || u32(mat.native.w) == 7u) {",
        );
    source.push('\n');
    source.push_str(&reject);
    for (original, stem) in [("fs_blend", "foliage_blend"), ("fs_sprite", "foliage_sprite")] {
        let marker = format!("@fragment fn {original}(");
        let start = source.find(&marker).expect("generated native foliage wrapper");
        let end = start + source[start..].find("\n}").expect("generated foliage wrapper end") + 2;
        let mode = if original == "fs_sprite" { 6 } else { 2 };
        let mut wrapper = source[start..end].to_owned();
        for input in ["i", "surface", "moon_receiver"] {
            wrapper = wrapper.replace(&format!("enhanced_original_{original}({input})"), &format!("foliage_edge_shade({input}, {mode}u)"));
        }
        for (suffix, reject) in [("core", "!foliage_core(foliage_alpha(i))"), ("soft", "foliage_core(foliage_alpha(i))")] {
            // The inherited guard follows all samples/derivatives but precedes
            // receiver visibility, emission and beauty. Reject the other half here.
            let wrapper = wrapper.replacen(&marker, &format!("@fragment fn fs_{stem}_{suffix}("), 1)
                .replace("early_reject(i, early_mode);",
                    &format!("foliage_edge_reject(i, early_mode);\n    if {reject} {{ discard; }}"));
            source.push('\n');
            source.push_str(&wrapper);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn partition_follows_derivatives_and_precedes_every_receiver() {
        for materials in [false, true] {
            for clustered in [false, true] {
                for night in [false, true] {
                    let mut source = crate::enhanced_shader::source(crate::EnhancedSettings {
                        generated_normals: materials, material_maps: materials, material_pbr: materials,
                        environment_probe: materials, material_specular: true,
                        clustered_lighting: clustered, sun_shadows: true, moon_shadows: night,
                        emissive_bloom: true,
                        night_mode: if night { crate::NightMode::Moonlit } else { crate::NightMode::AsGame },
                        ..Default::default()
                    });
                    if night { source = crate::moon_shadows::shader(source, true, true); }
                    crate::foliage::append_shader(&mut source);
                    crate::foliage_detail::append_shader(&mut source, true, true);
                    super::append(&mut source);
                    for stem in ["blend", "sprite"] {
                        for suffix in ["core", "soft"] {
                            let marker = format!("@fragment fn fs_foliage_{stem}_{suffix}(");
                            let body = source.split(&marker).nth(1).unwrap().split("\n}").next().unwrap();
                            let partition = body.find("foliage_core(foliage_alpha(i))").unwrap();
                            for derivative in ["early_primary = sample_clamped(", "early_geometric_normal = cross(dpdx(",
                                "early_footprint = max(length(fwidth(", "early_normal_dy = dpdy("] {
                                assert!(body.find(derivative).unwrap() < partition, "{marker}: {derivative}");
                            }
                            if materials {
                                assert!(body.find("material_prepare_derivatives(i);").unwrap() < partition);
                                assert!(body.find("early_normal_dy = dpdy(material_normal_value").unwrap() < partition);
                                assert!(partition < body.find("material_prepare(i);").unwrap());
                            }
                            for expensive in ["let emission_sample =", "let beauty =", "foliage_edge_shade("] {
                                assert!(partition < body.find(expensive).unwrap(), "{marker}: {expensive}");
                            }
                            if night && !clustered && stem == "blend" {
                                assert!(partition < body.find("let moon_receiver = authored_moon_receiver(").unwrap());
                                assert!(body.contains("foliage_edge_shade(moon_receiver, 2u)"));
                            }
                            assert!(!body.contains("enhanced_original_fs_"));
                        }
                    }
                    let shade = source.split("fn foliage_edge_shade(").nth(1).unwrap().split("\n}").next().unwrap();
                    assert!(shade.contains("let t = early_primary;"));
                    assert!(shade.contains("let mode = select(select(fade_mode, fade_mode - 4u, faded), 2u, sprite);"));
                    assert!(shade.contains("var a = select(select(1.0, alpha, one_one || mode == 2u || (mode == 1u && a2c)), alpha * (1.0 - f), add);"));
                    let alpha = source.split("fn foliage_alpha(").nth(1).unwrap().split("\n}").next().unwrap();
                    for declaration in alpha.lines().filter(|line| line.trim_start().starts_with("let ")) {
                        // Beauty aliases the same cached sample as `t` above.
                        let beauty_declaration = declaration.replace("early_primary.a", "t.a");
                        assert!(shade.contains(&beauty_declaration), "early alpha must use the exact beauty expression: {beauty_declaration}");
                    }
                    assert!(shade.contains("let alpha = texture_alpha * i.color.a * material_alpha;"));
                }
            }
        }
    }

    #[test]
    fn cached_alpha_partition_matches_post_beauty_pixels_msaa1_and_4() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let r = crate::Renderer::new(&instance, None).expect("foliage partition parity GPU");
        let native = include_str!("shader.wgsl").split("fn shade(").nth(1).unwrap().split("\n}").next().unwrap();
        let alpha = native[native.find("    let sprite =").unwrap()..native.find("    // Audit item 4").unwrap()]
            .replace("sample_clamped(tex, i.uv + mat.scroll.xy * g.fog.z)", "early_primary");
        let output = &native[native.find("    var a =").unwrap()..native.find("    return vec4<f32>").unwrap()];
        // Probe production alpha arithmetic/mode mapping and both partition
        // locations; the source contract above covers the complete wrappers.
        let source = format!(r#"
struct VOut {{ color: vec4<f32>, fade: f32 }}
struct Material {{ color: vec4<f32>, emissive: vec4<f32> }}
var<private> mat: Material;
var<private> early_primary: vec4<f32>;
{}
fn beauty_alpha(i: VOut, fade_mode: u32) -> f32 {{
{alpha}
    let a2c = false;
    let f = 0.73;
    let add = mode == 3u;
{output}
    return a;
}}
@vertex fn vs(@builtin(vertex_index) n: u32) -> @builtin(position) vec4<f32> {{
    return vec4<f32>(vec2<f32>(f32((n << 1u) & 2u), f32(n & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}}
fn pixel(p: vec4<f32>, optimized: bool) -> vec4<f32> {{
    let x = u32(p.x);
    let y = u32(p.y);
    let samples = array<f32, 8>(0.0, 0.001, 0.5, 0.9499999, 0.95, 0.9500001, 0.999, 1.0);
    early_primary = vec4<f32>(1.0, 1.0, 1.0, samples[x % 8u]);
    mat.color = vec4<f32>(1.0, 1.0, 1.0, select(1.0, 0.95, (x / 8u) % 2u != 0u));
    mat.emissive = vec4<f32>(0.0, 0.0, 0.0, f32((x / 16u) % 3u));
    let i = VOut(vec4<f32>(1.0, 1.0, 1.0, select(1.0, 0.95, y % 8u >= 4u)),
        select(1.0, 0.95, y % 4u >= 2u));
    let derivative = abs(dpdx(p.x)) + abs(dpdy(p.y));
    let soft = y % 2u != 0u;
    let mode = select(2u, 6u, y >= 8u);
    if optimized && foliage_core(foliage_alpha(i)) == soft {{ discard; }}
    let a = beauty_alpha(i, mode);
    if !optimized && foliage_core(a) == soft {{ discard; }}
    return vec4<f32>(0.25 * derivative, 0.75, 0.25, a);
}}
@fragment fn reference(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{ return pixel(p, false); }}
@fragment fn optimized(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{ return pixel(p, true); }}
"#, include_str!("foliage_edges.wgsl"));
        let module = r.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("foliage early partition parity"), source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        for samples in [1, 4] {
            assert!(r.msaa_supported.contains(&samples), "regression requires real {samples}x MSAA");
            let mut captures = Vec::new();
            for entry in ["reference", "optimized"] {
                let pipeline = r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: None, layout: None,
                    vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), buffers: &[], compilation_options: Default::default() },
                    fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some(entry), targets: &[Some(wgpu::ColorTargetState {
                        format: r.format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL,
                    })], compilation_options: Default::default() }),
                    primitive: Default::default(), depth_stencil: None,
                    multisample: wgpu::MultisampleState { count: samples, ..Default::default() }, multiview_mask: None, cache: None,
                });
                let texture = |count| r.device.create_texture(&wgpu::TextureDescriptor {
                    label: None, size: wgpu::Extent3d { width: 48, height: 16, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: count, dimension: wgpu::TextureDimension::D2, format: r.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | if count == 1 { wgpu::TextureUsages::COPY_SRC } else { wgpu::TextureUsages::empty() },
                    view_formats: &[],
                });
                let output = texture(1);
                let view = output.create_view(&Default::default());
                let msaa = (samples > 1).then(|| texture(samples));
                let msaa_view = msaa.as_ref().map(|t| t.create_view(&Default::default()));
                let mut encoder = r.device.create_command_encoder(&Default::default());
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: msaa_view.as_ref().unwrap_or(&view), resolve_target: msaa_view.as_ref().map(|_| &view), depth_slice: None,
                            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                        })], ..Default::default()
                    });
                    pass.set_pipeline(&pipeline);
                    pass.draw(0..3, 0..1);
                }
                r.queue.submit([encoder.finish()]);
                captures.push(crate::texture_to_rgba(&r, &output, 48, 16).unwrap());
            }
            assert_eq!(captures[0], captures[1], "core+soft alpha partition exact pixel parity at {samples}x MSAA");
            assert!(captures[1].as_chunks::<4>().0.iter().any(|p| p[3] > 0 && p[3] < 240), "soft coverage must survive");
            assert!(captures[1].as_chunks::<4>().0.iter().any(|p| p[3] >= 240), "core coverage must survive");
            assert!(captures[1].as_chunks::<4>().0.iter().any(|p| p[3] == 0), "partition must reject pixels");
        }
    }

    #[test]
    fn partition_softens_greater_tests_without_touching_native_shader() {
        let mut source = super::super::enhanced_shader::source(crate::EnhancedSettings::default());
        let native_shade = source.split("fn shade(").nth(1).unwrap().split("\n}").next().unwrap().to_owned();
        super::append(&mut source);
        for stem in ["blend", "sprite"] {
            for suffix in ["core", "soft"] {
                let start = source.find(&format!("@fragment fn fs_foliage_{stem}_{suffix}(")).unwrap();
                let body = &source[start..start + source[start..].find("\n}").unwrap()];
                assert!(body.contains("foliage_edge_shade("));
                assert!(body.contains("foliage_edge_reject(i, early_mode);"));
                assert!(body.find("foliage_edge_reject(").unwrap() < body.find("foliage_edge_shade(").unwrap());
                assert!(body.find("foliage_core(foliage_alpha(i))").unwrap() < body.find("foliage_edge_shade(").unwrap());
                assert!(!body.contains("foliage_core(beauty.a)"));
            }
        }
        assert!(source.contains("if native_alpha_test() && !native_alpha_pass(alpha)"));
        assert!(source.contains("return alpha >= 0.95;"));
        assert_eq!(source.split("fn shade(").nth(1).unwrap().split("\n}").next().unwrap(), native_shade);
        let edge = source.split("fn foliage_edge_shade(").nth(1).unwrap().split("\n}").next().unwrap();
        assert!(edge.contains("alpha <= 0.0, u32(mat.native.w) == 5u || u32(mat.native.w) == 7u"));
        let reject = source.split("fn foliage_edge_reject(").nth(1).unwrap().split("\n}").next().unwrap();
        assert!(reject.contains("alpha <= 0.0, u32(mat.native.w) == 5u || u32(mat.native.w) == 7u"),
            "root coverage and native shade must relax precisely the same comparisons");
        for value in 0..=255 {
            let alpha = value as f32 / 255.0;
            let native_pass = alpha > 0.5;
            let soft_pass = alpha > 0.0 && alpha < 0.95;
            if alpha > 0.0 && !native_pass {
                assert!(soft_pass, "fractional silhouette coverage must reach the non-depth-writing pass");
            }
            if alpha == 0.0 { assert!(!soft_pass); }
        }
        for value in 0..=255 {
            let alpha = value as f32 / 255.0;
            for shaded_pass in [false, true] {
                let core = shaded_pass && alpha >= 0.95;
                let soft = shaded_pass && alpha < 0.95;
                assert_eq!(core || soft, shaded_pass);
                assert!(!(core && soft));
            }
        }
    }
}
