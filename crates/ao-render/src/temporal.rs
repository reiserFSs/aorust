//! Opt-in temporal colour history. Surface identity is checked at every reconstruction tap.
use crate::effect_gpu::EffectPassEncoder as _;
use crate::{gpu_stats::GpuStatsExt, pipeline_creation::RenderPipeline};
use glam::{Mat4, Vec3};

pub(crate) struct PostTarget {
    pub size: (u32, u32),
    pub view: wgpu::TextureView,
    pub source: wgpu::BindGroup,
}
impl PostTarget {
    pub fn new(device: &wgpu::Device, size: (u32,u32), format: wgpu::TextureFormat, layout: &wgpu::BindGroupLayout) -> Self {
        let view=device.create_texture_tracked(&wgpu::TextureDescriptor {label:Some("Enhanced postprocess input"),size:wgpu::Extent3d {width:size.0,height:size.1,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format,usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::TEXTURE_BINDING,view_formats:&[]}).create_view(&Default::default());
        let source=device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {label:Some("Enhanced postprocess source"),layout,entries:&[wgpu::BindGroupEntry {binding:0,resource:wgpu::BindingResource::TextureView(&view)}]});
        Self {size,view,source}
    }
}

struct History {
    colour: wgpu::TextureView,
    identity: wgpu::TextureView,
    depth: wgpu::TextureView,
    source: wgpu::BindGroup,
}
/// Compiled once when TAA is enabled; independent of viewport/history resources.
pub(crate) struct TemporalPrograms {
    layout: wgpu::BindGroupLayout,
    pipeline: RenderPipeline,
}
impl TemporalPrograms {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label:Some("Temporal resolve inputs"), entries:&std::array::from_fn::<_,10,_>(|i|wgpu::BindGroupLayoutEntry {
            binding:i as u32,visibility:wgpu::ShaderStages::FRAGMENT,count:None,
            ty:if i==7 { wgpu::BindingType::Buffer { ty:wgpu::BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None } }
            else if i==9 { wgpu::BindingType::Buffer { ty:wgpu::BufferBindingType::Storage {read_only:true},has_dynamic_offset:false,min_binding_size:None } }
            else { wgpu::BindingType::Texture { sample_type:if i==3 || i==4 {wgpu::TextureSampleType::Uint} else if i==5 {wgpu::TextureSampleType::Depth} else {wgpu::TextureSampleType::Float {filterable:false}},view_dimension:wgpu::TextureViewDimension::D2,multisampled:false } },
        }) });
        let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor { label:Some("YCoCg temporal resolve"),source:wgpu::ShaderSource::Wgsl(crate::shader_source::load("taa.wgsl",include_str!("taa.wgsl"))) });
        let pl=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {label:Some("Temporal resolve"),bind_group_layouts:&[Some(&layout)],immediate_size:0});
        let targets=[format,wgpu::TextureFormat::R32Uint,wgpu::TextureFormat::Rg32Float].map(|format|Some(wgpu::ColorTargetState {format,blend:None,write_mask:wgpu::ColorWrites::ALL}));
        let pipeline=crate::pipeline_creation::render_deferred(device,&wgpu::RenderPipelineDescriptor {
            label:Some("Temporal variance clipped Catmull-Rom"),layout:Some(&pl),vertex:wgpu::VertexState {module:&shader,entry_point:Some("vs"),compilation_options:Default::default(),buffers:&[]},
            fragment:Some(wgpu::FragmentState {module:&shader,entry_point:Some("resolve"),compilation_options:Default::default(),targets:&targets}),
            primitive:Default::default(),depth_stencil:None,multisample:Default::default(),multiview_mask:None,cache:None,
        });
        Self { layout, pipeline }
    }
    pub fn is_ready(&self) -> bool { self.pipeline.is_ready() }
    pub fn wait(&self) { self.pipeline.get(); }
}
pub(crate) struct Temporal {
    size: (u32,u32),
    layout: wgpu::BindGroupLayout,
    pipeline: RenderPipeline,
    params: wgpu::Buffer,
    history: [History;2],
    bindings: [Option<wgpu::BindGroup>;2],
    index: usize,
    valid: bool,
    frame: u32,
}
impl Temporal {
    pub fn new(device:&wgpu::Device, size:(u32,u32), format:wgpu::TextureFormat, source_layout:&wgpu::BindGroupLayout, programs:&TemporalPrograms)->Self {
        let layout = programs.layout.clone();
        let pipeline = programs.pipeline.clone();
        let params=device.create_buffer_tracked(&wgpu::BufferDescriptor { label:Some("Temporal parameters"),size:32,usage:wgpu::BufferUsages::UNIFORM|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false });
        let history=std::array::from_fn(|_|{
            let texture=|format,label|device.create_texture_tracked(&wgpu::TextureDescriptor { label:Some(label),size:wgpu::Extent3d {width:size.0,height:size.1,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format,usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::TEXTURE_BINDING,view_formats:&[] }).create_view(&Default::default());
            let colour=texture(format,"Temporal colour history"); let identity=texture(wgpu::TextureFormat::R32Uint,"Temporal identity history"); let depth=texture(wgpu::TextureFormat::Rg32Float,"Temporal depth and fog history");
            let source=device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {label:Some("Temporal resolved source"),layout:source_layout,entries:&[wgpu::BindGroupEntry {binding:0,resource:wgpu::BindingResource::TextureView(&colour)}]});
            History {colour,identity,depth,source}
        });
        Self {size,layout,pipeline,params,history,bindings:[None,None],index:0,valid:false,frame:0}
    }
    pub fn size(&self)->(u32,u32){self.size}
    pub fn bytes(&self)->u64{u64::from(self.size.0)*u64::from(self.size.1)*32+32}
    pub fn reset(&mut self){self.valid=false;self.frame=0;self.bindings=[None,None];}
    pub fn begin(&mut self)->[f32;2]{
        let jitter=halton_jitter(self.frame); self.frame=self.frame.wrapping_add(1); jitter
    }
    #[allow(clippy::too_many_arguments)]
    pub fn encode(&mut self,device:&wgpu::Device,queue:&wgpu::Queue,encoder:&mut wgpu::CommandEncoder,current:&wgpu::TextureView,motion:&wgpu::TextureView,identity:&wgpu::TextureView,depth:&wgpu::TextureView,fog:&wgpu::TextureView,fog_color:[f32;3],actor_visibility_changed:&wgpu::Buffer){
        let old=&self.history[1-self.index]; let out=&self.history[self.index];
        if self.bindings[self.index].is_none() {
            let views=[current,&old.colour,motion,identity,&old.identity,depth,&old.depth,fog];
            let entries=std::array::from_fn::<_,10,_>(|i|wgpu::BindGroupEntry {binding:i as u32,resource:if i==7 {self.params.as_entire_binding()} else if i==9 {actor_visibility_changed.as_entire_binding()} else {wgpu::BindingResource::TextureView(views[if i==8 {7}else{i}])}});
            self.bindings[self.index]=Some(device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {label:Some("Temporal current frame"),layout:&self.layout,entries:&entries}));
        }
        let bg=self.bindings[self.index].as_ref().expect("temporal inputs initialized");
        queue.write_buffer(&self.params,0,bytemuck::cast_slice(&[self.size.0 as f32,self.size.1 as f32,0.0,0.0,if self.valid {1.0}else{0.0},fog_color[0],fog_color[1],fog_color[2]]));
        let attachments=[&out.colour,&out.identity,&out.depth].map(|view|Some(wgpu::RenderPassColorAttachment {view,resolve_target:None,depth_slice:None,ops:wgpu::Operations {load:wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),store:wgpu::StoreOp::Store}}));
        let mut pass=encoder.begin_effect_render_pass(&wgpu::RenderPassDescriptor {label:Some("Enhanced TAA"),color_attachments:&attachments,depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None});
        pass.set_pipeline(&self.pipeline);pass.set_bind_group(0,bg,&[]);pass.draw(0..3,0..1);drop(pass);
        self.valid=true;self.index=1-self.index;
    }
    pub fn output(&self)->&wgpu::TextureView{&self.history[1-self.index].colour}
    pub fn source(&self)->&wgpu::BindGroup{&self.history[1-self.index].source}
}
pub(crate) fn halton_jitter(frame:u32)->[f32;2]{
    fn radical(mut index:u32,base:u32)->f32{let mut result=0.0;let mut scale=1.0;while index>0 {scale/=base as f32;result+=(index%base) as f32*scale;index/=base;}result}
    let index=frame%8+1;[radical(index,2)-0.5,radical(index,3)-0.5]
}

impl crate::Renderer {
    pub fn set_post_focus(&mut self, focus: Option<[f32; 3]>) {
        self.post_focus = focus.filter(|p| p.iter().all(|v| v.is_finite()));
    }
    pub(crate) fn reset_post_history(&mut self) {
        if let Some(temporal) = &mut self.temporal { temporal.reset(); }
        self.reset_motion_vectors();
        self.post_previous_view = None;
    }
    pub(crate) fn prepare_post_frame(&mut self, size: (u32,u32), camera: &crate::Camera, native_vp: Mat4, fog_color: [f32;3]) -> Mat4 {
        let settings = self.enhanced_settings;
        if !settings.post_enabled() { return native_vp; }
        let room = self.fog.as_ref().and_then(|fog| fog.camera_room(camera.pos.to_array()));
        let cut = self.post_previous_view.is_none_or(|(_,eye)| eye.distance_squared(camera.pos) > 64.0);
        if cut || room != self.temporal_room || self.post_fog_color != Some(fog_color) { self.reset_post_history(); }
        self.temporal_room = room;
        self.post_fog_color = Some(fog_color);
        if self.post_target.as_ref().is_none_or(|target| target.size != size) {
            self.reset_post_history();
            self.post_target = Some(PostTarget::new(&self.device, size, self.format, self.enhanced_pipelines.source_layout()));
        }
        if !settings.taa { return native_vp; }
        if self.temporal.as_ref().is_none_or(|temporal| temporal.size() != size) {
            self.temporal = Some(Temporal::new(&self.device,size,self.format,self.enhanced_pipelines.source_layout(),self.temporal_programs.as_ref().expect("TAA programs allocated before frame")));
        }
        let jitter = self.temporal.as_mut().expect("TAA allocated").begin();
        Mat4::from_translation(Vec3::new(2.0*jitter[0]/size.0 as f32,-2.0*jitter[1]/size.1 as f32,0.0))*native_vp
    }
    pub(crate) fn encode_post_frame(&mut self, encoder: &mut wgpu::CommandEncoder, output: &wgpu::TextureView, camera: &crate::Camera, vp: Mat4, far: f32) {
        if self.post_target.is_none() { return; }
        if self.enhanced_settings.taa || self.enhanced_settings.motion_blur > 0.0 {
            if let Some(frame) = &self.enhanced_frame {
                let depth = frame.depth.clone();
                self.encode_motion_vectors(encoder, &depth);
            }
        }
        self.finish_motion_frame(encoder);
        let Some(frame) = &self.enhanced_frame else { return; };
        let target = self.post_target.as_ref().expect("post target allocated");
        if let (Some(temporal), Some(motion)) = (&mut self.temporal, &self.motion_vectors) {
            temporal.encode(&self.device,&self.queue,encoder,&target.view,motion.motion_view(),motion.identity_view(),&frame.resolved_depth,frame.sun.as_ref().expect("postprocessing owns native fog mask"),self.post_fog_color.unwrap_or_default().map(crate::gamma),motion.visibility_changed());
        }
        let (view, source) = self.temporal.as_ref().map_or((&target.view,&target.source),|temporal|(temporal.output(),temporal.source()));
        let settings = self.enhanced_settings;
        let spatial_settings = crate::spatial_post::SpatialSettings {
            fxaa: settings.fxaa && !settings.taa, cas: settings.cas, cas_strength: settings.cas_strength,
            motion_blur: settings.motion_blur, depth_of_field: settings.depth_of_field,
            dof_aperture: settings.dof_aperture, dof_max_radius: settings.dof_max_radius,
        };
        let projection = crate::spatial_post::Projection {
            near: self.lens.near, far,
            focus_depth: self.post_focus.map(|p|(Vec3::from_array(p)-camera.pos).dot(camera.forward())).filter(|z|*z>self.lens.near),
            fog_color: self.post_fog_color.unwrap_or_default().map(crate::gamma),
        };
        let motion = self.motion_vectors.as_ref().map(|motion|(motion.motion_view(),motion.identity_view()));
        let resolved = match self.spatial_post.as_mut() {
            Some(spatial) => spatial.encode(&self.device,&self.queue,encoder,view,&frame.resolved_depth,frame.sun.as_ref().expect("postprocessing owns native fog mask"),motion,frame.size,spatial_settings,projection),
            None => None,
        };
        let timestamps = if self.heat_haze.source().is_none() && self.screen.layers.is_empty() { self.gpu_timer.as_mut().map(crate::FrameGpuTimer::end_pass) } else { None };
        frame.composite(encoder,self.heat_haze.source().unwrap_or(output),Some(resolved.unwrap_or(source)),timestamps);
        self.post_previous_view=Some((vp,camera.pos));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn jitter_is_bounded_periodic_and_subpixel() {
        for i in 0..8 {
            let j=halton_jitter(i);
            assert!(j.into_iter().all(|x|(-0.5..0.5).contains(&x)));
            assert_eq!(j,halton_jitter(i+8));
        }
        assert_ne!(halton_jitter(0),halton_jitter(1));
    }
    #[test]
    fn surface_delta_keeps_native_fog_and_quantization_residual() {
        let fog_color=Vec3::new(0.35,0.45,0.55);
        let authored=Vec3::new(0.2,0.6,0.8);
        let replacement=Vec3::new(0.9,0.1,0.4);
        for fog in [0.0_f32,0.25,0.8,1.0] {
            let native=((authored*(1.0-fog)+fog_color*fog)*255.0).round()/255.0;
            let recovered=((native-fog_color*fog)/(1.0-fog).max(0.00001)).clamp(Vec3::ZERO,Vec3::ONE);
            let residual=native-(recovered*(1.0-fog)+fog_color*fog);
            let resolved=native+(replacement-recovered)*(1.0-fog);
            let expected=replacement*(1.0-fog)+fog_color*fog+residual;
            assert!((resolved-expected).abs().max_element()<0.000001);
            if fog==1.0 { assert_eq!(resolved,native); }
        }
        let shader=include_str!("taa.wgsl");
        assert!(shader.contains("for(var y=0;y<4;y++)"));
        assert!(!shader.contains("if fog>0.0"));
        assert!(shader.contains("d.g<1.0"));
        assert!(shader.contains("resolved_surface-current_surface"));
    }
}
