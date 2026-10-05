//! 2D quad pipeline that draws an `ao_gui::DrawList` (skin atlas + glyph coverage atlas).
//!
//! Pixel exact at 1:1: coordinates are window pixels, textures are sampled with the nearest filter
//! (UNRESOLVED: the client's D3D7 texture filter for the GUI is not confirmed; point filtering is the
//! D3D7 default), colours are the authored display-space values (an sRGB target gets the colour decoded
//! so it is re-encoded to the original value; blending still happens in the target's space).

use crate::Renderer;
use ao_gui::{DrawCmd, DrawList, Gui};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Globals {
    screen: [f32; 2],
    srgb: f32,
    _pad: f32,
}

pub struct GuiRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    gfx_view: wgpu::TextureView,
    page_px: Vec<(f32, f32)>,
    glyph_tex: wgpu::Texture,
    glyph_view: wgpu::TextureView,
    glyph_version: u64,
    glyph_size: (u32, u32),
}

impl GuiRenderer {
    /// Uploads the skin atlas of `gui` and builds the pipeline for `r.format`.
    pub fn new(r: &Renderer, gui: &Gui) -> Self {
        let device = &r.device;
        let atlas = gui.atlas();
        let pages = atlas.pages.len().max(1) as u32;
        let (pw, ph) = atlas.pages.first().map_or((1, 1), |p| (p.width, p.height));
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gui skin atlas"),
            size: wgpu::Extent3d { width: pw, height: ph, depth_or_array_layers: pages },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (i, p) in atlas.pages.iter().enumerate() {
            r.queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d { x: 0, y: 0, z: i as u32 }, aspect: wgpu::TextureAspect::All },
                &p.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(p.width * 4), rows_per_image: Some(p.height) },
                wgpu::Extent3d { width: p.width, height: p.height, depth_or_array_layers: 1 },
            );
        }
        let gfx_view = tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let ga = gui.glyph_atlas();
        let glyph_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gui glyph atlas"),
            size: wgpu::Extent3d { width: ga.width, height: ga.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let glyph_view = glyph_tex.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Nearest, min_filter: wgpu::FilterMode::Nearest, ..Default::default() });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gui"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2Array, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gui"), source: wgpu::ShaderSource::Wgsl(include_str!("gui.wgsl").into()) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("gui"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Uint32];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gui"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: r.format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            layout,
            sampler,
            gfx_view,
            page_px: atlas.pages.iter().map(|p| (p.width as f32, p.height as f32)).collect(),
            glyph_tex,
            glyph_view,
            glyph_version: 0,
            glyph_size: (ga.width, ga.height),
        }
    }

    /// Draws `list` over `target` (`size` = target pixels, `scale` = target pixels per GUI pixel, nearest-neighbour). The target is loaded, not cleared.
    pub fn draw(&mut self, r: &Renderer, target: &wgpu::TextureView, size: (u32, u32), scale: u32, gui: &Gui, list: &DrawList) {
        let sc = scale.max(1) as f32;
        let ga = gui.glyph_atlas();
        if ga.version != self.glyph_version {
            r.queue.write_texture(
                self.glyph_tex.as_image_copy(),
                &ga.data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(self.glyph_size.0), rows_per_image: Some(self.glyph_size.1) },
                wgpu::Extent3d { width: self.glyph_size.0, height: self.glyph_size.1, depth_or_array_layers: 1 },
            );
            self.glyph_version = ga.version;
        }
        // vertices + per-clip batches
        let mut verts: Vec<Vertex> = Vec::new();
        let mut batches: Vec<(Option<[i32; 4]>, std::ops::Range<u32>)> = Vec::new();
        let mut clip: Option<[i32; 4]> = None;
        let mut start = 0u32;
        let quad = |verts: &mut Vec<Vertex>, d: [f32; 4], uv: [f32; 4], color: [f32; 4], kind: u32| {
            let v = |x: f32, y: f32, u, v| Vertex { pos: [x * sc, y * sc], uv: [u, v], color, kind };
            verts.extend_from_slice(&[v(d[0], d[1], uv[0], uv[1]), v(d[2], d[1], uv[2], uv[1]), v(d[0], d[3], uv[0], uv[3]), v(d[2], d[1], uv[2], uv[1]), v(d[2], d[3], uv[2], uv[3]), v(d[0], d[3], uv[0], uv[3])]);
        };
        let (gw, gh) = (self.glyph_size.0 as f32, self.glyph_size.1 as f32);
        for c in &list.cmds {
            match *c {
                DrawCmd::Clip(nc) => {
                    if verts.len() as u32 > start {
                        batches.push((clip, start..verts.len() as u32));
                        start = verts.len() as u32;
                    }
                    clip = nc;
                }
                DrawCmd::Gfx { id, src, dst, tint, alpha } => {
                    let Some(e) = gui.atlas().entry(id) else { continue };
                    let (pw, ph) = self.page_px[e.page as usize];
                    let uv = [(e.x as f32 + src[0]) / pw, (e.y as f32 + src[1]) / ph, (e.x as f32 + src[0] + src[2]) / pw, (e.y as f32 + src[1] + src[3]) / ph];
                    quad(&mut verts, dst, uv, [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0, alpha], e.page);
                }
                DrawCmd::Glyph { src, dst, tint, alpha } => {
                    let d = [dst[0] as f32, dst[1] as f32, (dst[0] + src[2] as i32) as f32, (dst[1] + src[3] as i32) as f32];
                    let uv = [src[0] as f32 / gw, src[1] as f32 / gh, (src[0] + src[2]) as f32 / gw, (src[1] + src[3]) as f32 / gh];
                    quad(&mut verts, d, uv, [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0, alpha], 1 << 16);
                }
                DrawCmd::Solid { dst, color, alpha } => {
                    quad(&mut verts, dst, [0.0; 4], [color[0] as f32 / 255.0, color[1] as f32 / 255.0, color[2] as f32 / 255.0, alpha], 2 << 16);
                }
            }
        }
        if verts.len() as u32 > start {
            batches.push((clip, start..verts.len() as u32));
        }
        if verts.is_empty() {
            return;
        }
        let device = &r.device;
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("gui verts"), contents: bytemuck::cast_slice(&verts), usage: wgpu::BufferUsages::VERTEX });
        let globals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gui globals"),
            contents: bytemuck::bytes_of(&Globals { screen: [size.0 as f32, size.1 as f32], srgb: if r.format.is_srgb() { 1.0 } else { 0.0 }, _pad: 0.0 }),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gui"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.gfx_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.glyph_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("gui") });
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: target, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }})],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.set_vertex_buffer(0, vb.slice(..));
            for (c, range) in batches {
                match c {
                    Some([x0, y0, x1, y1]) => {
                        let (x0, y0, x1, y1) = (x0 * scale as i32, y0 * scale as i32, x1 * scale as i32, y1 * scale as i32);
                        let (x0, y0) = (x0.clamp(0, size.0 as i32), y0.clamp(0, size.1 as i32));
                        let (x1, y1) = (x1.clamp(x0, size.0 as i32), y1.clamp(y0, size.1 as i32));
                        pass.set_scissor_rect(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
                    }
                    None => pass.set_scissor_rect(0, 0, size.0, size.1),
                }
                pass.draw(range, 0..1);
            }
        }
        r.queue.submit([enc.finish()]);
    }
}
