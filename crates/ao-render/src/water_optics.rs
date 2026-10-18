//! Opt-in current-frame optics; faithful liquid draw and opacity remain untouched.
use crate::effect_gpu::EffectPassEncoder as _;
use crate::{gpu_stats::GpuStatsExt, pipeline_creation::RenderPipeline};
use ao_scene::liquid::Surface;
use glam::{Mat4, Vec3};

#[derive(Clone, Copy)]
pub(super) struct WaterOpticsSwitches {
    pub refraction: bool,
    pub reflections: bool,
    pub underwater: bool,
    pub ripples: bool,
    pub caustics: bool,
    pub sun_glint: bool,
    pub waterfalls: bool,
    pub foam: bool,
    pub waves: bool,
    pub wakes: bool,
    pub rain_mask: bool,
    pub realism: f32,
}
impl Default for WaterOpticsSwitches {
    fn default() -> Self {
        Self { refraction: false, reflections: false, underwater: false, ripples: false, caustics: false, sun_glint: false, waterfalls: false, foam: false, waves: false, wakes: false, rain_mask: false, realism: 0.5 }
    }
}
#[derive(Clone, Copy)]
pub(super) struct WaterEligibility {
    pub daylight: bool,
    pub outdoors: bool,
    pub fog_start: f32,
    pub fog_end: f32,
    pub sun_direction: [f32; 3],
    pub sun_colour: [f32; 3],
    pub sky_colour: [f32; 3],
    pub fog_colour: [f32; 3],
    pub samples: u32,
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Lamp {
    position_range: [f32; 4],
    colour: [f32; 4],
    attenuation: [f32; 4],
    direction: [f32; 4],
    cone: [f32; 4],
}
impl Lamp {
    const ZERO: Self = Self { position_range: [0.0; 4], colour: [0.0; 4], attenuation: [0.0; 4], direction: [0.0; 4], cone: [0.0; 4] };
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    vp: [[f32; 4]; 4],
    inverse_vp: [[f32; 4]; 4],
    eye_time: [f32; 4],
    control: [f32; 4],
    fog: [f32; 4],
    effects: [f32; 4],
    sun_direction: [f32; 4],
    sun_colour: [f32; 4],
    sky_colour: [f32; 4],
    fog_colour: [f32; 4],
    raster: [f32; 4],
    lamps: [Lamp; 64],
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex { position: [f32; 3], normal: [f32; 3], shore: f32, foam: f32 }
struct Targets {
    size: (u32, u32),
    position: wgpu::TextureView,
    normal: wgpu::TextureView,
    depth: wgpu::TextureView,
    scratch: wgpu::TextureView,
    inputs: Option<Inputs>,
    bottom: wgpu::TextureView,
}
struct Inputs {
    colour: wgpu::TextureView,
    depth: wgpu::TextureView,
    mask: wgpu::BindGroup,
    composite: wgpu::BindGroup,
    copy: wgpu::BindGroup,
}
pub(super) struct WaterOptics {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    mask_layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    mask: RenderPipeline,
    composite: RenderPipeline,
    copy: RenderPipeline,
    vertices: Option<wgpu::Buffer>,
    vertex_count: u32,
    vertex_capacity: u64,
    triangles: Vec<[Vec3; 3]>,
    targets: Option<Targets>,
    active: bool,
    lamps: [Lamp; 64],
    lamp_count: usize,
    membership: Option<bool>,
    needs_bottom: bool,
}

/// Vertical intersection of a finite loaded surface triangle, not its infinite plane.
fn surface_height(triangle: [Vec3; 3], point: Vec3) -> Option<f32> {
    if !point.is_finite() { return None; }
    let [a, b, c] = triangle.map(|v| v.as_dvec3());
    let point = point.as_dvec3();
    let ab = b - a;
    let ac = c - a;
    let ap = point - a;
    let determinant = ab.x * ac.z - ab.z * ac.x;
    if determinant.abs() <= 1e-12 { return None; }
    let u = (ap.x * ac.z - ap.z * ac.x) / determinant;
    let v = (ab.x * ap.z - ab.z * ap.x) / determinant;
    if u < 0.0 || v < 0.0 || u + v > 1.0 { return None; }
    Some((a.y + ab.y * u + ac.y * v) as f32)
}
impl WaterOptics {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let uniform = wgpu::BindGroupLayoutEntry {
            binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None,
        };
        let texture = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None,
        };
        let depth_entry = texture(1, wgpu::TextureSampleType::Depth);
        let mask_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("water exact mask inputs"), entries: &[uniform, depth_entry] });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("water optics inputs"), entries: &[
            uniform, depth_entry,
            texture(2, wgpu::TextureSampleType::Float { filterable: false }),
            texture(3, wgpu::TextureSampleType::Float { filterable: false }),
            texture(4, wgpu::TextureSampleType::Float { filterable: false }),
            texture(5, wgpu::TextureSampleType::Depth),
        ] });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("water optics"), source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("water_optics.wgsl", include_str!("water_optics.wgsl"))) });
        let mask_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("water exact mask"), source: wgpu::ShaderSource::Wgsl(crate::shader_source::load("water_mask.wgsl", include_str!("water_mask.wgsl"))) });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("water optics"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let mask_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("water mask"), bind_group_layouts: &[Some(&mask_layout)], immediate_size: 0 });
        let target = |format| Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL });
        let make = |entry| crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some(entry), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some(entry), compilation_options: Default::default(), targets: &[target(format)] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let composite = make("fs");
        let copy = make("copy");
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32, 3 => Float32];
        let mask = crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
            label: Some("water exact triangle mask"), layout: Some(&mask_pipeline_layout),
            vertex: wgpu::VertexState { module: &mask_shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attributes }] },
            fragment: Some(wgpu::FragmentState { module: &mask_shader, entry_point: Some("fs"), compilation_options: Default::default(), targets: &[target(wgpu::TextureFormat::Rgba32Float), target(wgpu::TextureFormat::Rgba16Float)] }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::LessEqual), stencil: Default::default(), bias: Default::default() }),
            multisample: Default::default(), multiview_mask: None, cache: None,
        });
        let params = device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("water optics parameters"), size: std::mem::size_of::<Params>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        Self { format, layout, mask_layout, params, mask, composite, copy, vertices: None, vertex_count: 0, vertex_capacity: 0, triangles: Vec::new(), targets: None, active: false, lamps: [Lamp::ZERO; 64], lamp_count: 0, membership: None, needs_bottom: false }
    }
    pub(super) fn is_ready(&self) -> bool { self.mask.is_ready() && self.composite.is_ready() && self.copy.is_ready() }
    pub(super) fn wait(&self) { self.mask.get(); self.composite.get(); self.copy.get(); }
    pub(super) fn target_bytes(&self) -> u64 {
        self.targets.as_ref().map_or(0, |targets| {
            let colour = u64::from(self.format.block_copy_size(None).unwrap_or(8));
            u64::from(targets.size.0) * u64::from(targets.size.1) * (16 + 8 + 4 + 4 + colour)
        })
    }
    pub(super) fn release_targets(&mut self) { self.targets = None; self.active = false; }
    /// Live collision query includes authored lower depth and room membership.
    /// None is reserved for offline scenes without the native collision data.
    pub(super) fn set_membership(&mut self, membership: Option<bool>) { self.membership = membership; }
    fn camera_underwater(&self, eye: Vec3) -> bool {
        self.membership.unwrap_or_else(|| self.contains(eye))
    }
    /// Caller supplies only currently admitted, active native lights; no actor
    /// registry or stale previous-frame lights are consulted here.
    pub(super) fn set_lights(&mut self, _device: &wgpu::Device, _queue: &wgpu::Queue, lights: &[ao_scene::Light]) {
        self.lamp_count = 0;
        self.lamps.fill(Lamp::ZERO);
        for light in lights.iter().filter(|light| light.range.is_finite() && light.range > 0.0 && light.pos.iter().chain(&light.color).chain(&light.atten).all(|value| value.is_finite())).take(64) {
            let (direction, cone) = if let Some(spot) = light.spot {
                if !spot.dir.iter().all(|v| v.is_finite()) || !spot.theta.is_finite() || !spot.phi.is_finite() { continue; }
                ([spot.dir[0], spot.dir[1], spot.dir[2], 1.0], [(spot.theta * 0.5).cos(), (spot.phi * 0.5).cos(), 0.0, 0.0])
            } else { ([0.0; 4], [0.0; 4]) };
            self.lamps[self.lamp_count] = Lamp { position_range: [light.pos[0], light.pos[1], light.pos[2], light.range], colour: [light.color[0], light.color[1], light.color[2], 0.0], attenuation: [light.atten[0], light.atten[1], light.atten[2], 0.0], direction, cone };
            self.lamp_count += 1;
        }
    }

    /// Upload ordinary and shallow water only; acid/lava/star are excluded.
    pub(super) fn set_surfaces<'a>(&mut self, device: &wgpu::Device, surfaces: impl Iterator<Item = &'a Surface>) {
        self.triangles.clear();
        self.lamps.fill(Lamp::ZERO);
        self.lamp_count = 0;
        self.membership = None;
        let mut vertices = Vec::new();
        for surface in surfaces.filter(|surface| matches!(surface.style, 0 | 4)) {
            for triangle in surface.triangles() {
                self.triangles.push(triangle.map(|v| Vec3::from_array(v.pos)));
                vertices.extend(triangle.map(|v| Vertex { position: v.pos, normal: v.normal, shore: 1000.0, foam: 0.0 }));
            }
        }
        self.vertex_count = vertices.len() as u32;
        self.vertex_capacity = std::mem::size_of_val(vertices.as_slice()) as u64;
        self.vertices = (!vertices.is_empty()).then(|| device.create_buffer_init_tracked(&wgpu::util::BufferInitDescriptor { label: Some("exact loaded water triangles"), contents: bytemuck::cast_slice(&vertices), usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST }));
        self.release_targets();
    }
    /// Triangle-expanded displaced mask only. Original topology still owns membership.
    /// Packed pos3, normal3, shore-distance (or steep-sheet along), foam.
    pub(super) fn set_geometry(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, vertices: &[[f32; 8]]) {
        let bytes = bytemuck::cast_slice(vertices);
        self.vertex_count = vertices.len() as u32;
        if bytes.is_empty() { return; }
        if bytes.len() as u64 > self.vertex_capacity {
            self.vertex_capacity = (bytes.len() as u64).next_power_of_two();
            self.vertices = Some(device.create_buffer_tracked(&wgpu::BufferDescriptor { label: Some("enhanced water triangle mask"), size: self.vertex_capacity, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }));
        }
        queue.write_buffer(self.vertices.as_ref().expect("water vertex allocation"), 0, bytes);
    }
    pub(super) fn contains(&self, eye: Vec3) -> bool {
        // ponytail: once-per-frame linear scan; spatial bins if loaded triangle counts make it measurable.
        self.triangles.iter().any(|&triangle| surface_height(triangle, eye).is_some_and(|height| eye.y < height))
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32), vp: Mat4, inverse_vp: Mat4, eye: Vec3, time: f32, eligibility: WaterEligibility, switches: WaterOpticsSwitches) -> bool {
        let underwater = switches.underwater && self.camera_underwater(eye);
        self.needs_bottom = switches.refraction || switches.caustics || switches.foam;
        self.active = self.vertex_count > 0 && size.0 > 0 && size.1 > 0 && vp.is_finite() && inverse_vp.is_finite() && eye.is_finite() && (underwater || switches.refraction || switches.reflections || switches.caustics || switches.sun_glint || switches.waterfalls || switches.foam || switches.waves || switches.wakes || switches.ripples || switches.rain_mask);
        if !self.active { self.release_targets(); return false; }
        if self.targets.as_ref().is_none_or(|t| t.size != size) {
            let view = |label, format, usage| device.create_texture_tracked(&wgpu::TextureDescriptor { label: Some(label), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format, usage, view_formats: &[] }).create_view(&Default::default());
            let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
            self.targets = Some(Targets { size, position: view("water surface positions", wgpu::TextureFormat::Rgba32Float, usage), normal: view("water surface normals shore", wgpu::TextureFormat::Rgba16Float, usage), depth: view("nearest water surface", wgpu::TextureFormat::Depth32Float, wgpu::TextureUsages::RENDER_ATTACHMENT), scratch: view("water optics scratch", self.format, usage), inputs: None, bottom: view("water static bottom depth", wgpu::TextureFormat::Depth32Float, usage) });
        }
        let eligible = (eligibility.outdoors || self.lamp_count > 0) && eligibility.fog_start.is_finite() && eligibility.fog_end.is_finite() && eligibility.fog_end > eligibility.fog_start;
        queue.write_buffer(&self.params, 0, bytemuck::bytes_of(&Params {
            vp: vp.to_cols_array_2d(), inverse_vp: inverse_vp.to_cols_array_2d(), eye_time: [eye.x, eye.y, eye.z, if time.is_finite() { time } else { 0.0 }],
            control: [u32::from(switches.refraction) as f32, u32::from(switches.reflections) as f32, if switches.realism.is_finite() { switches.realism.clamp(0.0, 1.0) } else { 0.5 }, u32::from(switches.ripples) as f32],
            fog: [eligibility.fog_start, eligibility.fog_end, u32::from(eligible) as f32, u32::from(underwater) as f32],
            effects: [u32::from(switches.caustics) as f32, u32::from(switches.sun_glint) as f32, u32::from(switches.waterfalls) as f32, u32::from(switches.foam) as f32],
            sun_direction: [eligibility.sun_direction[0], eligibility.sun_direction[1], eligibility.sun_direction[2], u32::from(switches.waves || switches.wakes || switches.ripples || switches.waterfalls) as f32],
            sun_colour: [eligibility.sun_colour[0], eligibility.sun_colour[1], eligibility.sun_colour[2], u32::from(eligibility.daylight && eligibility.outdoors) as f32],
            sky_colour: [eligibility.sky_colour[0], eligibility.sky_colour[1], eligibility.sky_colour[2], u32::from(eligibility.outdoors) as f32],
            fog_colour: [eligibility.fog_colour[0], eligibility.fog_colour[1], eligibility.fog_colour[2], self.lamp_count as f32],
            raster: [eligibility.samples.max(1) as f32, 0.0, 0.0, 0.0],
            lamps: self.lamps,
        }));
        true
    }
    pub(super) fn needs_bottom_depth(&self) -> bool { self.active && self.needs_bottom }
    pub(super) fn rain_masks(&self) -> Option<(&wgpu::TextureView, &wgpu::TextureView)> {
        if !self.active { return None; }
        self.targets.as_ref().map(|targets| (&targets.position, &targets.normal))
    }

    /// Target for the parent's static-only bottom raster. Actors and liquids
    /// must never be submitted here; native opacity is preserved in scene colour.
    pub(super) fn bottom_depth(&self) -> Option<&wgpu::TextureView> {
        if !self.needs_bottom_depth() { return None; }
        self.targets.as_ref().map(|targets| &targets.bottom)
    }

    /// Encode after native liquid and depth resolve, before other compositors.
    /// Both inputs are single-sampled. Colour needs TEXTURE_BINDING|RENDER_ATTACHMENT.
    /// Scratch is owned here; colour is never sampled while attached for writing.
    pub(super) fn encode(&mut self, device: &wgpu::Device, encoder: &mut wgpu::CommandEncoder, colour: &wgpu::TextureView, depth: &wgpu::TextureView) {
        if !self.active { return; }
        let targets = self.targets.as_mut().expect("prepared water targets");
        if targets.inputs.as_ref().is_none_or(|i| i.colour != *colour || i.depth != *depth) {
            let tex = |binding, view| wgpu::BindGroupEntry { binding, resource: wgpu::BindingResource::TextureView(view) };
            let bind = |source| device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("water optics frame inputs"), layout: &self.layout, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() }, tex(1, depth), tex(2, source), tex(3, &targets.position), tex(4, &targets.normal), tex(5, &targets.bottom),
            ] });
            let mask = device.create_bind_group_tracked(&wgpu::BindGroupDescriptor { label: Some("water mask frame inputs"), layout: &self.mask_layout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: self.params.as_entire_binding() }, tex(1, depth)] });
            targets.inputs = Some(Inputs { colour: colour.clone(), depth: depth.clone(), mask, composite: bind(colour), copy: bind(&targets.scratch) });
        }
        let inputs = targets.inputs.as_ref().expect("prepared water input bindings");
        let attachment = |view| Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } });
        {
            let mut pass = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor { label: Some("exact water triangle mask"), color_attachments: &[attachment(&targets.position), attachment(&targets.normal)], depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &targets.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }), timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            pass.set_pipeline(&self.mask);
            pass.set_bind_group(0, &inputs.mask, &[]);
            pass.set_vertex_buffer(0, self.vertices.as_ref().expect("loaded water triangles").slice(..));
            pass.draw(0..self.vertex_count, 0..1);
        }
        for (label, pipeline, bind, output) in [("water optics", &self.composite, &inputs.composite, &targets.scratch), ("water optics copy back", &self.copy, &inputs.copy, colour)] {
            let mut pass = encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor { label: Some(label), color_attachments: &[attachment(output)], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optics_are_independently_off_by_default() {
        let switches = WaterOpticsSwitches::default();
        assert!(!switches.refraction && !switches.reflections && !switches.underwater && !switches.ripples && !switches.caustics && !switches.sun_glint && !switches.waterfalls && !switches.foam && !switches.waves && !switches.wakes);
        assert_eq!(switches.realism, 0.5);
        assert!(!switches.rain_mask);
    }
    #[test]
    fn finite_triangle_membership_handles_shore_holes_slopes_and_edges() {
        let triangle = [Vec3::new(0.0, 2.0, 0.0), Vec3::new(10.0, 4.0, 0.0), Vec3::new(0.0, 2.0, 10.0)];
        assert_eq!(surface_height(triangle, Vec3::new(5.0, -100.0, 0.0)), Some(3.0));
        assert!(surface_height(triangle, Vec3::new(8.0, 0.0, 8.0)).is_none(), "bounding box is not membership");
        assert!(surface_height(triangle, Vec3::new(-0.01, 0.0, 1.0)).is_none());
        assert!(surface_height(triangle, Vec3::new(f32::NAN, 0.0, 1.0)).is_none());
        assert!(surface_height([Vec3::ZERO, Vec3::Y, Vec3::Z], Vec3::ZERO).is_none(), "vertical falls are not infinite volumes");
        let contains = |point: Vec3| surface_height(triangle, point).is_some_and(|height| point.y < height);
        assert!(contains(Vec3::new(1.0, 1.0, 1.0)));
        assert!(!contains(Vec3::new(1.0, 5.0, 1.0)));
    }
    #[test]
    fn shader_modules_validate() {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let renderer = crate::Renderer::new(&instance, None).expect("water shader validation requires offscreen GPU");
        let scope = renderer.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let mut optics = WaterOptics::new(&renderer.device, wgpu::TextureFormat::Rgba16Float);
        optics.wait();
        optics.set_membership(Some(true));
        assert!(optics.camera_underwater(Vec3::ZERO), "native collision overrides missing offline triangles");
        optics.triangles.push([Vec3::new(-1.0, 2.0, -1.0), Vec3::new(1.0, 2.0, -1.0), Vec3::new(0.0, 2.0, 1.0)]);
        optics.set_membership(Some(false));
        assert!(!optics.camera_underwater(Vec3::ZERO), "room/depth rejection overrides the surface footprint");
        optics.set_membership(None);
        assert!(optics.camera_underwater(Vec3::ZERO), "offline fallback remains finite-triangle based");
        optics.set_geometry(&renderer.device, &renderer.queue, &[
            [-1.0, 2.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
            [1.0, 2.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
            [0.0, 2.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0],
        ]);
        let eligibility = WaterEligibility { daylight: true, outdoors: true, fog_start: 0.0, fog_end: 100.0,
            sun_direction: [0.0, 1.0, 0.0], sun_colour: [1.0; 3], sky_colour: [0.5; 3], fog_colour: [0.5; 3], samples: 1 };
        assert!(optics.prepare(&renderer.device, &renderer.queue, (16, 16), Mat4::IDENTITY, Mat4::IDENTITY,
            Vec3::ZERO, 0.0, eligibility, WaterOpticsSwitches { rain_mask: true, ..Default::default() }),
            "rain alone requests the exact native liquid mask without enhanced water shading");
        assert!(optics.rain_masks().is_some() && !optics.needs_bottom_depth());
        assert!(!optics.prepare(&renderer.device, &renderer.queue, (16, 16), Mat4::IDENTITY, Mat4::IDENTITY,
            Vec3::ZERO, 0.0, eligibility, WaterOpticsSwitches::default()));
        assert!(optics.rain_masks().is_none(), "all-off releases the optical mask");
        assert!(pollster::block_on(scope.pop()).is_none());
    }

    #[test]
    fn rain_only_native_sloped_plane_has_complete_mask_coverage() {
        use ao_scene::{liquid::SourceTriangle, Blend, Instance, Mesh, Scene, Submesh, Vertex, IDENTITY};
        let source = Surface::new(0, 0, [[0, 1, 2], [0, 2, 3]].map(|indices| SourceTriangle {
            kind: 0xff20_3041,
            vertices: indices.map(|i| Vertex {
                pos: [[960.0, 18.6, -720.0], [1060.0, 19.6, -720.0],
                    [1060.0, 21.6, -620.0], [960.0, 20.6, -620.0]][i],
                ..Default::default()
            }),
        }).to_vec()).unwrap();
        let camera = crate::Camera { pos: Vec3::new(1010.4, 24.0, -663.1),
            yaw: 2.4288, pitch: -0.8, roll: 0.0 };
        let (mut vertices, mut indices, mut colours) = (Vec::new(), Vec::new(), Vec::new());
        source.geometry(camera.pos.to_array(), true, &mut vertices, &mut indices, &mut colours);
        assert!(indices.len() > 6, "exercise the native tessellated plane, not two trivial triangles");
        let count = vertices.len() as u32;
        vertices.extend_from_within(..);
        let submeshes = (0..2).map(|layer| Submesh { liquid: true, two_sided: true,
            blend: Blend::AlphaBlend, emissive: [0.3; 3],
            ..Submesh::new(indices.iter().map(|i| i + count * layer).collect(), None) }).collect();
        let scene = Scene { meshes: vec![Mesh { vertices, submeshes }], liquids: vec![source],
            instances: vec![Instance { mesh: 0, transform: IDENTITY }], daylight: Some(1.0),
            ..Default::default() };
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let mut renderer = crate::Renderer::new(&instance, None).expect("native liquid coverage GPU");
        renderer.upload(&scene);
        renderer.set_enhanced_settings(crate::EnhancedSettings { enhanced_rain: true, ..Default::default() });
        let output = renderer.device.create_texture_tracked(&wgpu::TextureDescriptor {
            label: Some("native mask complete coverage"), size: wgpu::Extent3d { width: 48, height: 48, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: renderer.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
        });
        let view = output.create_view(&Default::default());
        let shader = renderer.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native mask coverage readback"), source: wgpu::ShaderSource::Wgsl(r#"
@group(0) @binding(0) var positions: texture_2d<f32>;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    return vec4<f32>(vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0, 0.0, 1.0);
}
@fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(vec3<f32>(select(0.0, 1.0, textureLoad(positions, vec2<i32>(p.xy), 0).w > 0.0)), 1.0);
}
"#.into()),
        });
        let layout = renderer.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native mask coverage"), entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None,
            }],
        });
        let pipeline_layout = renderer.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native mask coverage"), bind_group_layouts: &[Some(&layout)], immediate_size: 0,
        });
        let pipeline = renderer.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("native mask coverage readback"), layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &shader, entry_point: Some("fs"), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: renderer.format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
        });
        for samples in [1, 4] {
            renderer.set_msaa_samples(samples);
            if renderer.msaa_samples() != samples { continue; }
            let targets = crate::Targets::new(&renderer, 48, 48);
            renderer.wait_for_pipeline_warmup();
            renderer.render(&view, &targets, &camera);
            renderer.wait_for_pipeline_warmup();
            renderer.render(&view, &targets, &camera);
            let (positions, _) = renderer.water.rain_masks().expect("rain-only native mask");
            let bindings = renderer.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native mask coverage"), layout: &layout,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(positions) }],
            });
            let mut encoder = renderer.device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("native mask coverage"), color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view, resolve_target: None, depth_slice: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &bindings, &[]);
                pass.draw(0..3, 0..1);
            }
            renderer.queue.submit([encoder.finish()]);
            let pixels = crate::texture_to_rgba(&renderer, &output, 48, 48).expect("native mask readback");
            assert!(pixels.as_chunks::<4>().0.iter().all(|pixel| *pixel == [255; 4]),
                "native liquid triangles lost mask coverage at {samples}x MSAA");
        }
    }
}
