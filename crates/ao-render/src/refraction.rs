//! Randy offscreen technology (pref `UseOffscreenSurfaceTechnology`) and VisualLiquid's `RenderRefraction` band.
//! DS `100793b8` renders into render target 1, copies it into target 2 (list 9, `10058669`) after the far buckets, then
//! `RViewPort::RenderRefraction(list 4)` (`1004c4ea`) calls `VisualLiquid` visual-vtable slot `+0x3c` = DS `1003d13a`
//! = `1003b9c9(…, 0)` (water) and `(…, 4)` (shallow). See docs/formats.md "RenderRefraction".
use super::{Renderer, REFRACTION_PIPE};
use crate::{effect_gpu::EffectPassEncoder as _, gpu_stats::GpuStatsExt};

/// Randy `10042732` case 2: target 2 is `pow2 <= width` by `pow2 < height` (starting at 2; both at least 2).
pub(super) fn target_size((width, height): (u32, u32)) -> (u32, u32) {
    let mut w = 1;
    let mut next = 2;
    while width > 1 && next <= width { w = next; next *= 2; }
    let (mut h, mut s) = (1, 2);
    if height > 2 {
        loop { let doubled = s * 2; h = s; s = doubled; if doubled >= height { break; } }
    }
    (w, h)
}

/// DS `1003b9c9` water branch: `y_up = 1 - y/H`, `fy` = camera forward Y. Mirrors `refraction_band_alpha` in shader.wgsl.
#[cfg(test)]
pub(super) fn band_alpha(y_up: f32, fy: f32) -> u8 {
    let centre = 0.5 - 0.5 * (fy * 1.414);
    let t = (y_up - centre) * 5.0;
    let m = if t <= 0.0 { t + 1.0 } else { 1.0 - t };
    if m > 0.2 { (f64::from(m * (1.0 - fy.abs()) * (1.0 - fy.abs()) * m * 255.0) - 0.49999).round_ties_even().clamp(0.0, 255.0) as u8 } else { 0 }
}

#[derive(Default)]
pub(super) struct Refraction {
    gpu: Option<Gpu>,
}

struct Gpu {
    copy_layout: wgpu::BindGroupLayout,
    copy_sampler: wgpu::Sampler,
    copy: wgpu::RenderPipeline,
    material_buffer: wgpu::Buffer,
    target: Option<Target>,
}

struct Target { size: (u32, u32), view: wgpu::TextureView, material: wgpu::BindGroup }

impl Renderer {
    /// Randy `m_bUseOffscreenTechnology` plus a liquid that requested feature 0x20 (DS `10040d3d`) in an earlier frame; the
    /// path models the faithful single-sample frame only.
    pub(super) fn offscreen_active(&self) -> bool {
        self.offscreen_surface_technology && self.offscreen_armed && !self.enhanced_settings.any_enabled() && self.world.msaa == 1
    }

    /// Creates (or resizes) render target 2 and its sampled material binding. Must run before the world pass is encoded.
    pub(super) fn prepare_refraction(&mut self, size: (u32, u32)) {
        let size = target_size(size);
        if self.refraction.gpu.is_none() {
            let copy_layout = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("refraction copy"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ] });
            let copy_sampler = self.device.create_sampler_tracked(&wgpu::SamplerDescriptor { label: Some("refraction copy linear"), address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, min_filter: wgpu::FilterMode::Linear, mag_filter: wgpu::FilterMode::Linear, ..Default::default() });
            let shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("refraction copy"), source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("refraction.wgsl", include_str!("refraction.wgsl"))) });
            let layout = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&copy_layout)], immediate_size: 0 });
            let copy = crate::pipeline_creation::render(&self.device, &wgpu::RenderPipelineDescriptor {
                label: Some("refraction copy"), layout: Some(&layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format: self.format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
                primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
            });
            let material_buffer = self.device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor { label: Some("refraction material"), contents: bytemuck::bytes_of(&[0f32; 32]), usage: wgpu::BufferUsages::UNIFORM });
            self.refraction.gpu = Some(Gpu { copy_layout, copy_sampler, copy, material_buffer, target: None });
        }
        if self.refraction.gpu.as_ref().and_then(|gpu| gpu.target.as_ref()).is_some_and(|target| target.size == size) { return; }
        let texture = self.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("Randy render target 2 (refraction)"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let maps = self.material_fallbacks();
        let gpu = self.refraction.gpu.as_ref().expect("created above");
        let material = self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
            label: Some("refraction material"), layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: gpu.material_buffer.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&maps.generated) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&maps.normal) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(&maps.orm) },
                wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::TextureView(&maps.emissive) },
            ],
        });
        self.refraction.gpu.as_mut().expect("created above").target = Some(Target { size, view, material });
    }

    /// List 9: render target 1 as texture, stretched over render target 2.
    pub(super) fn encode_refraction_copy(&self, encoder: &mut wgpu::CommandEncoder, source: &wgpu::TextureView) {
        let Some(gpu) = &self.refraction.gpu else { return };
        let Some(target) = &gpu.target else { return };
        let bind = self.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("refraction copy source"), layout: &gpu.copy_layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(source) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&gpu.copy_sampler) },
        ] });
        let mut pass = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("VisualOffscreen2Refraction list 9"), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(&gpu.copy);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }

    /// `RenderRefraction(list 4)`: water (style 0) then shallow (style 4) band over the already drawn liquid triangles.
    pub(super) fn draw_refraction(&self, pass: &mut wgpu::RenderPass, instances: &wgpu::Buffer) -> usize {
        let Some(target) = self.refraction.gpu.as_ref().and_then(|gpu| gpu.target.as_ref()) else { return 0 };
        let mut calls = 0;
        pass.set_bind_group(0, &self.globals_bg, &[]);
        pass.set_bind_group(1, &target.material, &[]);
        pass.set_vertex_buffer(1, instances.slice(..));
        for style in [0, 4] {
            for run in self.liquids.iter().filter(|run| run.source.style == style) {
                let mesh = run.source.mesh;
                let range = self.vis_range[mesh].clone();
                let Some(draw) = self.gpu.liquid.iter().find(|draw| draw.mesh == mesh) else { continue };
                let Some((vertices, indices)) = self.gpu.meshes[mesh].as_ref() else { continue };
                if range.is_empty() || draw.count == 0 { continue; }
                pass.set_pipeline(&self.world.pipes[REFRACTION_PIPE + usize::from(style == 4)]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(draw.first_index..draw.first_index + draw.count, 0, range);
                calls += 1;
            }
        }
        calls
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_target_2_size_follows_randy_10042732() {
        assert_eq!(target_size((1920, 1080)), (1024, 1024));
        assert_eq!(target_size((1920, 1200)), (1024, 1024));
        assert_eq!(target_size((1024, 1024)), (1024, 512));
        assert_eq!(target_size((800, 600)), (512, 512));
        assert_eq!(target_size((1280, 720)), (1024, 512));
    }

    #[test]
    fn horizon_band_follows_1003b9c9() {
        // Level camera: centre y_up .5, peak alpha 255 at the horizon, zero beyond |y_up - .5| >= .16 (m <= .2).
        assert_eq!(band_alpha(0.5, 0.0), 255);
        assert_eq!(band_alpha(0.7, 0.0), 0);
        assert_eq!(band_alpha(0.5 - 0.17, 0.0), 0);
        // tent m = 1 - 5|dy|: dy = .08 -> m .6, alpha = round(.36 * 255 - .49999) = 91
        assert_eq!(band_alpha(0.58, 0.0), 91);
        // Looking up shifts the band down by .707*fy and scales by (1 - |fy|)^2.
        let fy = 0.2;
        let centre = 0.5 - 0.5 * 1.414 * fy;
        assert_eq!(band_alpha(centre, fy), (255.0f32 * 0.64 - 0.49999).round() as u8);
    }
}
