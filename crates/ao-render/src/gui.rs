//! 2D quad pipeline that draws an `ao_gui::DrawList` (skin atlas + glyph coverage atlas).
//!
//! Pixel exact at 1:1: coordinates are window pixels, textures are sampled with the nearest filter
//! (UNRESOLVED: the client's D3D7 texture filter for the GUI is not confirmed; point filtering is the
//! D3D7 default), colours are the authored display-space values.
//!
//! **Blending happens in display (gamma) space like the original**: D3D7 has no sRGB framebuffer (`D3DRS_SRGBWRITE` does not exist), so every
//! `SRCALPHA / INVSRCALPHA` blend of the GUI works on the stored 8-bit values. World and GUI now share the UNORM gamma-space
//! target, so the GUI leaves scene bytes untouched and blends straight alpha over them, without an output conversion.

use crate::Renderer;
use ao_gui::{DrawCmd, DrawList, Gui};
use bytemuck::{Pod, Zeroable};
use crate::gpu_stats::{GpuStatsExt, TextureViewStatsExt};
use crate::pipeline_creation::RenderPipeline;
use std::collections::HashMap;

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
    _pad: [f32; 2],
}

pub struct GuiRenderer {
    pipeline: RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    linear: wgpu::Sampler,
    extras: Vec<(wgpu::Texture, wgpu::TextureView)>,
    gfx_view: wgpu::TextureView,
    page_px: Vec<(f32, f32)>,
    glyph_tex: wgpu::Texture,
    glyph_view: wgpu::TextureView,
    glyph_version: u64,
    glyph_size: (u32, u32),
    /// Persistent capacity-based vertex buffer (grown in place, like `actors`/`set_sky`): every `draw` call has a
    /// different vertex count (text/window layout changes every frame), but the GPU buffer object itself only needs
    /// to grow, never be recreated, once the screen's largest frame has been seen.
    vb: Option<wgpu::Buffer>,
    vb_cap: u64,
    /// Persistent globals uniform buffer, created once in `new` and updated in place via `queue.write_buffer`.
    globals: wgpu::Buffer,
    /// Bind groups cached by extra-image slot (`None` = glyph atlas): the glyph/extra-image textures, the glyph
    /// sampler and the globals buffer are all stable GPU objects for the renderer's whole lifetime, so a bind group
    /// only needs to be created once per distinct key, not every `draw` call.
    bind_groups: HashMap<Option<usize>, wgpu::BindGroup>,
    /// Vertex buffers actually (re)created by capacity growth, since the layer started: a regression counter, not
    /// drained. In steady state (a busy screen whose vertex count has a stable ceiling) this must stop climbing.
    pub(crate) vb_created: u64,
    /// Bind groups actually created (new cache entries), since the layer started: a regression counter. Converges
    /// once every distinct texture (glyph atlas + each extra image) a screen uses has been seen once.
    pub(crate) bg_created: u64,
    /// Persistent per-frame scratch (cleared, not reallocated, at the start of every `draw` call): the vertex
    /// quads and clip/extra-image batches built from `list.cmds` never need a fresh `Vec` once the screen's
    /// largest frame has grown these to their steady-state capacity.
    verts: Vec<Vertex>,
    batches: Vec<Batch>,
}

/// (scissor, extra-image slot, vertex range)
type Batch = (Option<[i32; 4]>, Option<usize>, std::ops::Range<u32>);

impl GuiRenderer {
    pub fn is_ready(&self) -> bool { self.pipeline.is_ready() }
    pub fn wait(&self) { self.pipeline.get(); }

    /// View of the shared UNORM gamma-space framebuffer.
    pub fn view(tex: &wgpu::Texture) -> wgpu::TextureView {
        tex.create_view_tracked(&Default::default())
    }

    /// Uploads the skin atlas of `gui` and queues the pipeline for `r.format`.
    pub fn new(r: &Renderer, gui: &Gui) -> Self {
        let device = &r.device;
        let atlas = gui.atlas();
        let pages = atlas.pages.len().max(1) as u32;
        let (pw, ph) = atlas.pages.first().map_or((1, 1), |p| (p.width, p.height));
        let tex = device.create_texture_tracked(&wgpu::TextureDescriptor {
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
        let gfx_view = tex.create_view_tracked(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        let ga = gui.glyph_atlas();
        let glyph_tex = device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("gui glyph atlas"),
            size: wgpu::Extent3d { width: ga.width, height: ga.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let glyph_view = glyph_tex.create_view_tracked(&Default::default());
        let sampler = device.create_sampler_tracked(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Nearest, min_filter: wgpu::FilterMode::Nearest, ..Default::default() });
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
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("gui"), source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("gui.wgsl", include_str!("gui.wgsl"))) });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("gui"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Uint32];
        let pipeline = crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
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
            linear: device.create_sampler_tracked(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, ..Default::default() }),
            extras: Vec::new(),
            gfx_view,
            page_px: atlas.pages.iter().map(|p| (p.width as f32, p.height as f32)).collect(),
            glyph_tex,
            glyph_view,
            glyph_version: 0,
            glyph_size: (ga.width, ga.height),
            vb: None,
            vb_cap: 0,
            globals: device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("gui globals"), size: std::mem::size_of::<Globals>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
            bind_groups: HashMap::new(),
            vb_created: 0,
            bg_created: 0,
            verts: Vec::new(),
            batches: Vec::new(),
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
        for img in gui.extra_images().iter().skip(self.extras.len()) {
            let t = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
                label: Some("gui image"),
                size: wgpu::Extent3d { width: img.w, height: img.h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            r.queue.write_texture(
                t.as_image_copy(),
                &img.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(img.w * 4), rows_per_image: Some(img.h) },
                wgpu::Extent3d { width: img.w, height: img.h, depth_or_array_layers: 1 },
            );
            let v = t.create_view_tracked(&Default::default());
            self.extras.push((t, v));
        }
        // vertices + per-clip batches
        self.verts.clear();
        self.batches.clear();
        let mut cur_extra: Option<usize> = None;
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
                    if self.verts.len() as u32 > start {
                        self.batches.push((clip, cur_extra, start..self.verts.len() as u32));
                        start = self.verts.len() as u32;
                    }
                    clip = nc;
                }
                DrawCmd::Gfx { id, src, dst, tint, alpha } => {
                    if id.0 >= ao_gui::EXTRA_BASE {
                        let k = (id.0 - ao_gui::EXTRA_BASE) as usize;
                        if k >= self.extras.len() {
                            continue;
                        }
                        if cur_extra != Some(k) {
                            if self.verts.len() as u32 > start {
                                self.batches.push((clip, cur_extra, start..self.verts.len() as u32));
                                start = self.verts.len() as u32;
                            }
                            cur_extra = Some(k);
                        }
                        let img = &gui.extra_images()[k];
                        let uv = [src[0] / img.w as f32, src[1] / img.h as f32, (src[0] + src[2]) / img.w as f32, (src[1] + src[3]) / img.h as f32];
                        quad(&mut self.verts, dst, uv, [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0, alpha], 3 << 16);
                        continue;
                    }
                    if cur_extra.is_some() {
                        if self.verts.len() as u32 > start {
                            self.batches.push((clip, cur_extra, start..self.verts.len() as u32));
                            start = self.verts.len() as u32;
                        }
                        cur_extra = None;
                    }
                    let Some(e) = gui.atlas().entry(id) else { continue };
                    let (pw, ph) = self.page_px[e.page as usize];
                    let uv = [(e.x as f32 + src[0]) / pw, (e.y as f32 + src[1]) / ph, (e.x as f32 + src[0] + src[2]) / pw, (e.y as f32 + src[1] + src[3]) / ph];
                    quad(&mut self.verts, dst, uv, [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0, alpha], e.page);
                }
                DrawCmd::Glyph { src, dst, scale, tint, alpha } => {
                    if cur_extra.is_some() {
                        if self.verts.len() as u32 > start {
                            self.batches.push((clip, cur_extra, start..self.verts.len() as u32));
                            start = self.verts.len() as u32;
                        }
                        cur_extra = None;
                    }
                    // aomac Enhanced "Anti-aliased Text" (audit item 13, not retail): `scale` > 1 means the atlas
                    // bitmap (`src` w/h) was rasterized that many times bigger for AA; dividing the destination
                    // box back down to the logical glyph size keeps layout identical while this per-command
                    // `scale` upscale (combined with the interface's own physical backing-scale upscale of the
                    // whole GUI) ends up sampling the atlas near 1:1 at the physical pixel grid. `scale` 1 (every
                    // bitmap font, and the option off) is exactly the old 1:1 behaviour.
                    let sc = u32::from(scale.max(1)) as f32;
                    let (bw, bh) = (src[2] as f32 / sc, src[3] as f32 / sc);
                    let d = [dst[0] as f32, dst[1] as f32, dst[0] as f32 + bw, dst[1] as f32 + bh];
                    let uv = [src[0] as f32 / gw, src[1] as f32 / gh, (src[0] + src[2]) as f32 / gw, (src[1] + src[3]) as f32 / gh];
                    quad(&mut self.verts, d, uv, [tint[0] as f32 / 255.0, tint[1] as f32 / 255.0, tint[2] as f32 / 255.0, alpha], 1 << 16);
                }
                DrawCmd::Solid { dst, color, alpha } => {
                    if cur_extra.is_some() {
                        if self.verts.len() as u32 > start {
                            self.batches.push((clip, cur_extra, start..self.verts.len() as u32));
                            start = self.verts.len() as u32;
                        }
                        cur_extra = None;
                    }
                    quad(&mut self.verts, dst, [0.0; 4], [color[0] as f32 / 255.0, color[1] as f32 / 255.0, color[2] as f32 / 255.0, alpha], 2 << 16);
                }
            }
        }
        if self.verts.len() as u32 > start {
            self.batches.push((clip, cur_extra, start..self.verts.len() as u32));
        }
        if self.verts.is_empty() {
            return;
        }
        let device = &r.device;
        // Bind groups are cached by extra-image slot: the glyph/extra textures, samplers, layout and the globals
        // buffer below are all stable for the renderer's lifetime, so each distinct key only needs one bind group
        // ever, not one per `draw` call (leak follow-up: this used to rebuild every batch's bind group every frame).
        for i in 0..self.batches.len() {
            let extra = self.batches[i].1;
            if !self.bind_groups.contains_key(&extra) {
                let (view, samp) = match extra {
                    Some(k) => (&self.extras[k].1, if gui.extra_images()[k].smooth { &self.linear } else { &self.sampler }),
                    None => (&self.glyph_view, &self.sampler),
                };
                let bg = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {
                    label: Some("gui"),
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: self.globals.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.gfx_view) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&self.glyph_view) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(samp) },
                        wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(view) },
                    ],
                });
                self.bind_groups.insert(extra, bg);
                self.bg_created += 1;
            }
        }
        // Persistent capacity-based vertex buffer (like `actors`/`set_sky`): grown in place only when the batch
        // outgrows the current capacity, written in place every call otherwise (leak follow-up: this used to
        // recreate a brand-new vertex buffer and globals uniform buffer every `draw` call).
        let needed = (self.verts.len() * std::mem::size_of::<Vertex>()) as u64;
        let (vb, created) = crate::ensure_buf(device, self.vb.take(), &mut self.vb_cap, needed, wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, "gui verts");
        self.vb_created += created as u64;
        r.queue.write_buffer(&vb, 0, bytemuck::cast_slice(&self.verts));
        r.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&Globals { screen: [size.0 as f32, size.1 as f32], _pad: [0.0; 2] }));
        let bgs: Vec<&wgpu::BindGroup> = self.batches.iter().map(|b| self.bind_groups.get(&b.1).expect("bind group cached above")).collect();
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
            pass.set_vertex_buffer(0, vb.slice(..));
            for ((c, _, range), bg) in self.batches.drain(..).zip(bgs) {
                pass.set_bind_group(0, bg, &[]);
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
        self.vb = Some(vb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Renderer;

    /// Leak follow-up (the real window renders the GUI every frame; the offscreen harness used to verify the sky
    /// fix does not): `GuiRenderer::draw` used to create a brand-new vertex buffer, globals uniform buffer and one
    /// bind group per clip/texture batch on *every* call instead of just once, a second steady per-frame GPU-object
    /// leak alongside the sky rebuild. Drives the real `draw()` path against a busy synthetic screen (five windows,
    /// ~2000 characters of varied text each: as many distinct glyphs/clip batches as a busy chat window plus
    /// several open HUD windows) and counts actual GPU object creations via the renderer's own regression counters
    /// (not an RSS bound). After warm-up (the steady vertex-count ceiling and every distinct texture seen once),
    /// further frames must create zero further vertex buffers or bind groups.
    #[test]
    fn gui_draw_creates_zero_gpu_objects_in_steady_state_with_a_busy_screen() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = ao_gui::Gui::new(&dir, None).unwrap();
        let blob: String = (0..2000).map(|i| char::from(b'!' + (i % 94) as u8)).collect();
        for i in 0..5 {
            let win = gui.open_window_xml(&format!("busy{i}"), "<root><ScrollView name=\"sv\" min_size=\"Point(300,500)\" max_size=\"Point(300,500)\"><ScrollViewChild name=\"svc\"><TextView name=\"text\" min_size=\"Point(280,4000)\" max_size=\"Point(280,4000)\"/></ScrollViewChild></ScrollView></root>", (i * 40, i * 40), ao_gui::WindowSize::Fixed(300, 500)).unwrap();
            gui.set_text(win, "text", &blob);
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).expect("resource regression requires GPU");
        r.upload(&ao_scene::Scene::default());
        let (w, h) = (1280u32, 800u32);
        let out = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = out.create_view_tracked(&Default::default());
        let mut gr = GuiRenderer::new(&r, &gui);
        for _ in 0..30 {
            let list = gui.frame(1.0 / 60.0);
            gr.draw(&r, &view, (w, h), 1, &gui, &list);
            let _ = r.device.poll(wgpu::PollType::Poll);
        }
        let (vb_warm, bg_warm) = (gr.vb_created, gr.bg_created);
        let (verts_cap_warm, batches_cap_warm) = (gr.verts.capacity(), gr.batches.capacity());
        eprintln!("warm-up (30 frames): vb_created={vb_warm} bg_created={bg_warm}");
        for _ in 0..30 {
            let list = gui.frame(1.0 / 60.0);
            gr.draw(&r, &view, (w, h), 1, &gui, &list);
            let _ = r.device.poll(wgpu::PollType::Poll);
        }
        assert_eq!(gr.vb_created, vb_warm, "steady-state busy-screen draws created {} more vertex buffers instead of reusing the capacity buffer", gr.vb_created - vb_warm);
        assert_eq!(gr.bg_created, bg_warm, "steady-state busy-screen draws created {} more bind groups instead of reusing the cache", gr.bg_created - bg_warm);
        assert_eq!(gr.verts.capacity(), verts_cap_warm, "steady-state busy-screen draws reallocated the CPU-side vertex scratch buffer");
        assert_eq!(gr.batches.capacity(), batches_cap_warm, "steady-state busy-screen draws reallocated the CPU-side batch scratch buffer");
    }

    /// Leak follow-up: `GuiRenderer::draw` creates a brand-new vertex buffer, globals uniform buffer and one bind
    /// group per clip/texture batch *every call* (not just once); a busy screen (many windows, heavy chat text, lots
    /// of distinct glyphs/clip regions -> many batches) could plausibly allocate far more per frame than the earlier
    /// lightning-actor-only measurement did. Drives the real `draw()` path in a tight loop against a busy synthetic
    /// screen (five windows, ~2000 characters of varied text each, so many distinct glyphs and clip batches) with a
    /// long-lived `Renderer`/`GuiRenderer` (like the live session) and samples real process RSS.
    #[test]
    #[ignore = "real GPU + client data; run explicitly: cargo test --release -p ao-render -- --ignored gui_draw_loop_rss_stays_flat_with_busy_text --nocapture"]
    fn gui_draw_loop_rss_stays_flat_with_busy_text() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() { return; }
        let mut gui = ao_gui::Gui::new(&dir, None).unwrap();
        let blob: String = (0..2000).map(|i| char::from(b'!' + (i % 94) as u8)).collect();
        for i in 0..5 {
            let win = gui.open_window_xml(&format!("busy{i}"), "<root><ScrollView name=\"sv\" min_size=\"Point(300,500)\" max_size=\"Point(300,500)\"><ScrollViewChild name=\"svc\"><TextView name=\"text\" min_size=\"Point(280,4000)\" max_size=\"Point(280,4000)\"/></ScrollViewChild></ScrollView></root>", (i * 40, i * 40), ao_gui::WindowSize::Fixed(300, 500)).unwrap();
            gui.set_text(win, "text", &blob);
        }
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut r = Renderer::new(&instance, None).unwrap();
        r.upload(&ao_scene::Scene::default());
        let (w, h) = (1280u32, 800u32);
        let out = r.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: r.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = out.create_view_tracked(&Default::default());
        let mut gr = GuiRenderer::new(&r, &gui);
        let rss = |pid: u32| -> u64 {
            let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
        };
        let pid = std::process::id();
        let mut samples = Vec::new();
        const FRAMES: usize = 3000; // 50 simulated seconds at 60 fps: a lot of GUI-only draws, run as fast as possible
        for i in 0..FRAMES {
            let list = gui.frame(1.0 / 60.0);
            gr.draw(&r, &view, (w, h), 1, &gui, &list);
            let _ = r.device.poll(wgpu::PollType::Poll);
            if i % 300 == 0 { samples.push((i, rss(pid))); }
        }
        samples.push((FRAMES, rss(pid)));
        eprintln!("rss samples (frame, KB): {samples:?}");
        let (warm, last) = (samples[1].1 as f64, samples.last().unwrap().1 as f64);
        let growth = last - warm;
        let per_frame = growth / (FRAMES - 300) as f64;
        eprintln!("growth after warm-up: {growth:.0} KB over {} frames ({per_frame:.2} KB/frame)", FRAMES - 300);
        assert!(growth < 50_000.0, "GUI draw-loop RSS grew {growth:.0} KB ({per_frame:.2} KB/frame) over {} frames: real leak in GuiRenderer::draw's per-frame buffer/bind-group creation", FRAMES - 300);
    }
}
