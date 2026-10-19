//! Final raster stage: TAA output -> CAS + tone map + grade + dither -> the output view. Also the bind-group / shader helpers the
//! bloom and TAA compute stages share, and the per-frame post controls.
use std::borrow::Cow;

/// Per-frame post controls (WGSL mirror: `struct Ctl` in `post_common.wgsl`).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub(super) struct Ctl {
    /// Bloom gain: the share of the blurred emission energy added back to the HDR image (`EnhancedSettings::bloom_strength`).
    pub bloom: f32,
    /// CAS sharpness 0..1 (`EnhancedSettings::cas_strength`).
    pub cas: f32,
    pub pad: [f32; 2],
    /// x = 1 discards the TAA history this frame.
    pub taa: [u32; 4],
}

pub(super) fn uniform_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }
}

pub(super) fn tex_entry(binding: u32, visibility: wgpu::ShaderStages, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None }
}

pub(super) fn depth_entry(binding: u32, visibility: wgpu::ShaderStages) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Depth, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None }
}

pub(super) fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: super::gbuffer::HDR, view_dimension: wgpu::TextureViewDimension::D2 },
        count: None }
}

pub(super) fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }
}

pub(super) fn view_bind(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) }
}

pub(super) fn buffer_bind(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
    wgpu::BindGroupEntry { binding, resource: buffer.as_entire_binding() }
}

/// A shader module of `own` behind the shared v2 prelude (frame constants, packing helpers, post helpers).
pub(super) fn module(device: &wgpu::Device, label: &str, own: Cow<'_, str>) -> wgpu::ShaderModule {
    let source = format!("{}\n{}\n{}\n{}",
        crate::shader_source::load("v2/common.wgsl", include_str!("common.wgsl")),
        crate::shader_source::load("v2/packing.wgsl", include_str!("packing.wgsl")),
        crate::shader_source::load("v2/post_common.wgsl", include_str!("post_common.wgsl")),
        own);
    device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(source.into()) })
}

pub(super) fn compute_pipeline(device: &wgpu::Device, label: &str, module: &wgpu::ShaderModule, entry: &str, layout: &wgpu::BindGroupLayout) -> wgpu::ComputePipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &[Some(layout)], immediate_size: 0 });
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label), layout: Some(&pipeline_layout), module, entry_point: Some(entry), compilation_options: Default::default(), cache: None,
    })
}

pub(super) struct Post {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

impl Post {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let f = wgpu::ShaderStages::FRAGMENT;
        let entries = [uniform_entry(0, f), tex_entry(1, f, false), depth_entry(2, f), uniform_entry(8, f)];
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("v2 post"), entries: &entries });
        let module = module(device, "v2 post", crate::shader_source::load("v2/post.wgsl", include_str!("post.wgsl")));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("v2 post"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("v2 post"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &module, entry_point: Some("fs"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        Self { pipeline, layout }
    }

    /// `input` is the TAA output of the frame, `depth` the G-buffer depth (fog gates for CAS and grade).
    pub(super) fn bind(&self, device: &wgpu::Device, consts: &wgpu::Buffer, ctl: &wgpu::Buffer, input: &wgpu::TextureView, depth: &wgpu::TextureView) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("v2 post"), layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: consts.as_entire_binding() },
                view_bind(1, input),
                view_bind(2, depth),
                wgpu::BindGroupEntry { binding: 8, resource: ctl.as_entire_binding() },
            ],
        })
    }

    pub(super) fn encode(&self, enc: &mut wgpu::CommandEncoder, bind: &wgpu::BindGroup, output: &wgpu::TextureView, timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>) {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("v2 post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: output, resolve_target: None, depth_slice: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None, timestamp_writes, occlusion_query_set: None, multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctl_layout_matches_wgsl() {
        assert_eq!(std::mem::size_of::<Ctl>(), 32);
        let wgsl = include_str!("post_common.wgsl");
        assert!(wgsl.contains("bloom: f32,") && wgsl.contains("cas: f32,") && wgsl.contains("pad: vec2<f32>,") && wgsl.contains("taa: vec4<u32>,"));
    }

    /// CPU mirror of `grade()` in post.wgsl (same constants, read back from the shader source below).
    fn grade_cpu(c: [f32; 3], t_native: f32, exposure: f32, saturation: f32, contrast: f32) -> [f32; 3] {
        let luma = |c: [f32; 3]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
        let smooth = |a: f32, b: f32, x: f32| { let t = ((x - a) / (b - a)).clamp(0.0, 1.0); t * t * (3.0 - 2.0 * t) };
        let k = smooth(0.5, 0.75, t_native);
        let mut c = c.map(|v| v * exposure);
        let peak = c[0].max(c[1]).max(c[2]);
        let l = luma(c);
        let roll = smooth(1.0, 3.0, peak) * 0.5;
        c = c.map(|v| v + (l - v) * roll);
        c = c.map(|x| { let kn = 0.72; x.min(kn) + (1.0 - kn) * (1.0 - (-(x - kn).max(0.0) / (1.0 - kn)).exp()) });
        let (s, ct) = (1.0 + (saturation - 1.0) * k, 1.0 + (contrast - 1.0) * k);
        let l = luma(c);
        c = c.map(|v| l + (v - l) * s);
        c.map(|v| (0.18 + (v - 0.18) * ct).max(v * 0.9))
    }

    #[test]
    fn grade_mirror_uses_the_shader_constants() {
        let wgsl = include_str!("post.wgsl");
        for needle in ["smoothstep(0.5, 0.75, t_native)", "let k = 0.72;", "smoothstep(1.0, 3.0, peak) * 0.5", "mix(1.0, fc.grade.y, k)", "mix(1.0, fc.grade.z, k)", "vec3<f32>(0.18)"] {
            assert!(wgsl.contains(needle), "post.wgsl changed ({needle}): update grade_cpu with it");
        }
    }

    /// Fairness (DESIGN section 8): over fogged pixels (T_native < 0.5) the grade is contrast- and saturation-neutral, so two fogged
    /// colours are never pushed further apart than the plain shoulder would; at full visibility the grade does boost.
    #[test]
    fn grade_is_contrast_neutral_over_fog() {
        let (a, b) = ([0.30, 0.34, 0.40], [0.36, 0.40, 0.44]);
        let grey = |c: [f32; 3]| c;
        let neutral = |t| { let (ga, gb) = (grade_cpu(a, t, 1.0, 1.08, 1.05), grade_cpu(b, t, 1.0, 1.08, 1.05)); (0..3).map(|i| (gb[i] - ga[i]).abs()).sum::<f32>() };
        let plain = { let (ga, gb) = (grade_cpu(grey(a), 0.0, 1.0, 1.0, 1.0), grade_cpu(grey(b), 0.0, 1.0, 1.0, 1.0)); (0..3).map(|i| (gb[i] - ga[i]).abs()).sum::<f32>() };
        for t in [0.0, 0.1, 0.25, 0.4, 0.49] {
            assert!((neutral(t) - plain).abs() < 1e-6, "fogged grade changed contrast at T={t}");
            assert_eq!(grade_cpu(a, t, 1.0, 1.08, 1.05), grade_cpu(a, t, 1.0, 1.0, 1.0));
        }
        assert!(neutral(1.0) > plain * 1.04, "visible geometry must still receive the contrast/saturation grade");
    }
}
