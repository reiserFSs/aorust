//! Primary-screen reflections; history contains static samples only, never actor/effect samples.
use crate::{gpu_stats::GpuStatsExt, pipeline_creation::RenderPipeline};
use glam::{Mat4, Vec3};
const PROBE: u32 = 128;
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    inverse_vp: [[f32; 4]; 4], vp: [[f32; 4]; 4], eye: [f32; 4],
    sky: [f32; 4], ground: [f32; 4], control: [f32; 4], wet: [f32; 4], area: [f32; 4], fog: [f32; 4],
}
pub(super) struct Reflections {
    layout: wgpu::BindGroupLayout, params: wgpu::Buffer,
    accumulate: RenderPipeline, reflection: RenderPipeline,
    probe: [wgpu::TextureView; 2], head: usize,
    snapshot: wgpu::TextureView,
    size: (u32,u32), previous: Option<(Vec3,f32)>,
    previous_direction: Option<Vec3>,
    bindings: Option<Bindings>,
}
struct Bindings {
    depth: wgpu::TextureView, surface: wgpu::TextureView, mask: wgpu::TextureView,
    roof: wgpu::TextureView, groups: [wgpu::BindGroup;2],
}
fn retention(previous: Option<(Vec3,f32)>, eye: Vec3, time: f32) -> (f32,bool) {
    let Some((old,then))=previous else {return (0.0,true)};
    let distance=eye.distance(old);let dt=time-then;
    if !distance.is_finite() || !dt.is_finite() || distance>=40.0 || dt<0.0 {return (0.0,true);}
    ((-distance*std::f32::consts::LN_2/140.0-dt/20.0).exp(),false)
}
impl Reflections {
    fn new(device:&wgpu::Device, format:wgpu::TextureFormat, size:(u32,u32))->Self {
        let tex=|binding,depth| wgpu::BindGroupLayoutEntry {binding,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Texture {sample_type:if depth {wgpu::TextureSampleType::Depth}else{wgpu::TextureSampleType::Float {filterable:false}},view_dimension:wgpu::TextureViewDimension::D2,multisampled:false},count:None};
        let layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {label:Some("material reflection inputs"),entries:&[
            wgpu::BindGroupLayoutEntry {binding:0,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Buffer {ty:wgpu::BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None},count:None},tex(1,false),tex(2,true),tex(3,false),tex(4,false),tex(5,false),tex(6,false)]});
        let params=device.create_buffer_tracked(&wgpu::BufferDescriptor {label:Some("reflection parameters"),size:std::mem::size_of::<Params>() as u64,usage:wgpu::BufferUsages::UNIFORM|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false});
        let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor {label:Some("observed material reflections"),source:wgpu::ShaderSource::Wgsl(crate::shader_source::load("material_reflections.wgsl",include_str!("material_reflections.wgsl")))});
        let pl=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {label:Some("material reflections"),bind_group_layouts:&[Some(&layout)],immediate_size:0});
        let pipeline=|entry,format,blend| crate::pipeline_creation::render_deferred(device,&wgpu::RenderPipelineDescriptor {
            label:Some(entry),layout:Some(&pl),vertex:wgpu::VertexState {module:&shader,entry_point:Some("vs"),compilation_options:Default::default(),buffers:&[]},fragment:Some(wgpu::FragmentState {module:&shader,entry_point:Some(entry),compilation_options:Default::default(),targets:&[Some(wgpu::ColorTargetState {format,blend,write_mask:wgpu::ColorWrites::ALL})]}),primitive:Default::default(),depth_stencil:None,multisample:Default::default(),multiview_mask:None,cache:None});
        let accumulate=pipeline("accumulate",crate::enhanced::AUX,None);
        let reflection=pipeline("reflection",format,Some(wgpu::BlendState::ALPHA_BLENDING));
        let image=|size:(u32,u32),format,label| device.create_texture_tracked(&wgpu::TextureDescriptor {label:Some(label),size:wgpu::Extent3d {width:size.0,height:size.1,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format,usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::TEXTURE_BINDING,view_formats:&[]}).create_view(&Default::default());
        let probe=[image((PROBE,PROBE),crate::enhanced::AUX,"static probe A"),image((PROBE,PROBE),crate::enhanced::AUX,"static probe B")];
        let snapshot=image(size,format,"primary reflection source");
        Self {layout,params,accumulate,reflection,probe,head:0,snapshot,size,previous:None,previous_direction:None,bindings:None}
    }
    pub(super) fn ready(&self)->bool {self.accumulate.is_ready() && self.reflection.is_ready()}
    pub(super) fn wait(&self) {self.accumulate.get();self.reflection.get();}
    #[cfg(test)]
    pub(super) fn history_view(&self)->&wgpu::TextureView {&self.probe[self.head]}
    #[cfg(test)]
    pub(super) fn forget_history(&mut self) {self.previous=None;self.previous_direction=None;}
    #[cfg(test)]
    pub(super) fn analytic_comparison(&self, r:&crate::Renderer, encoder:&mut wgpu::CommandEncoder) {
        let frame=r.enhanced_frame.as_ref().unwrap();
        let bg=r.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {label:Some("analytic-only oracle"),layout:r.enhanced_pipelines.source_layout(),entries:&[wgpu::BindGroupEntry {binding:0,resource:wgpu::BindingResource::TextureView(&self.snapshot)}]});
        frame.composite(encoder,&frame.colour,Some(&bg),None);
        r.queue.write_buffer(&self.params,(std::mem::offset_of!(Params,control)+4) as u64,bytemuck::bytes_of(&0.0f32));
        draw(encoder,&self.reflection,&self.bindings.as_ref().unwrap().groups[self.head],&frame.colour,true);
    }
    pub(super) fn bytes(&self)->u64 {u64::from(self.size.0)*u64::from(self.size.1)*4+u64::from(PROBE*PROBE)*16}
    fn bind(&self, device:&wgpu::Device, frame:&crate::enhanced::EnhancedFrame, history:&wgpu::TextureView, roof:&wgpu::TextureView)->wgpu::BindGroup {
        let view=|binding,v| wgpu::BindGroupEntry {binding,resource:wgpu::BindingResource::TextureView(v)};
        device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {label:Some("primary admitted reflection sources"),layout:&self.layout,entries:&[wgpu::BindGroupEntry {binding:0,resource:self.params.as_entire_binding()},view(1,&self.snapshot),view(2,&frame.resolved_depth),view(3,frame.reflection_surface.as_ref().expect("reflection surface")),view(4,frame.reflection_mask.as_ref().expect("reflection admission")),view(5,history),view(6,roof)]})
    }
    fn prepare_bindings(&mut self, device:&wgpu::Device, frame:&crate::enhanced::EnhancedFrame, roof:&wgpu::TextureView) {
        let surface=frame.reflection_surface.as_ref().expect("reflection surface");
        let mask=frame.reflection_mask.as_ref().expect("reflection admission");
        if self.bindings.as_ref().is_some_and(|b|b.depth==frame.resolved_depth && b.surface==*surface && b.mask==*mask && b.roof==*roof) {return;}
        self.bindings=Some(Bindings {depth:frame.resolved_depth.clone(),surface:surface.clone(),mask:mask.clone(),roof:roof.clone(),groups:std::array::from_fn(|i|self.bind(device,frame,&self.probe[i],roof))});
    }
}
impl crate::Renderer {
    pub(super) fn encode_material_reflections(&mut self, encoder:&mut wgpu::CommandEncoder, vp:Mat4, eye:Vec3, env:ao_scene::Environment) {
        let settings=self.enhanced_settings;
        if !settings.material_reflections && !settings.environment_probe {self.material_reflections=None;return;}
        if !vp.is_finite() || vp.determinant().abs()<1e-12 {return;}
        let Some(frame)=self.enhanced_frame.as_ref() else {return;};
        if self.material_reflections.as_ref().is_none_or(|r|r.size!=frame.size) {self.material_reflections=Some(Reflections::new(&self.device,self.format,frame.size));}
        let state=self.material_reflections.as_mut().expect("reflection resources");
        if !state.accumulate.is_ready() || !state.reflection.is_ready() {return;}
        // Snapshot precedes reflection/weather/particles: no feedback and no post-effect history.
        frame.composite(encoder,&state.snapshot,None,None);
        let (mut keep,reset)=retention(state.previous,eye,self.time);state.previous=Some((eye,self.time));
        let inverse=vp.inverse();
        let direction=(inverse.project_point3(Vec3::new(0.0,0.0,0.5))-eye).normalize_or_zero();
        if let Some(old)=state.previous_direction {keep*=(-old.angle_between(direction).abs()*0.5).exp();}
        state.previous_direction=Some(direction);
        let wet=self.rain_wet.as_ref().filter(|_|settings.wet_surfaces);
        let roof=wet.map_or(frame.reflection_mask.as_ref().expect("reflection metadata"),|r|&r.shelter);
        state.prepare_bindings(&self.device,frame,roof);
        let params=Params {
            inverse_vp:inverse.to_cols_array_2d(),vp:vp.to_cols_array_2d(),eye:eye.extend(self.time).to_array(),
            sky:Vec3::from_array(env.sky_color.map(crate::gamma)).extend(0.0).to_array(),
            ground:Vec3::from_array(env.ambient.map(crate::gamma)).extend(0.0).to_array(),
            control:[f32::from(u8::from(settings.material_reflections)),f32::from(u8::from(settings.environment_probe)),keep,f32::from(u8::from(reset))],
            wet:[wet.map_or(0.0,|r|r.reflection_wetness()),0.0,0.0,0.0],area:wet.map_or([0.0;4],|r|r.area),
            fog:Vec3::from_array(env.fog_color.map(crate::gamma)).extend(0.0).to_array(),
        };
        self.queue.write_buffer(&state.params,0,bytemuck::bytes_of(&params));
        if settings.environment_probe {
            let next=1-state.head;let bg=&state.bindings.as_ref().expect("prepared inputs").groups[state.head];
            draw(encoder,&state.accumulate,bg,&state.probe[next],false);state.head=next;
        }
        let bg=&state.bindings.as_ref().expect("prepared inputs").groups[state.head];
        draw(encoder,&state.reflection,bg,&frame.colour,true);
    }
}
fn draw(enc:&mut wgpu::CommandEncoder, pipeline:&RenderPipeline, bg:&wgpu::BindGroup, output:&wgpu::TextureView, load:bool) {
    let mut pass=enc.begin_render_pass(&wgpu::RenderPassDescriptor {label:Some("primary-only material reflections"),color_attachments:&[Some(wgpu::RenderPassColorAttachment {view:output,resolve_target:None,depth_slice:None,ops:wgpu::Operations {load:if load {wgpu::LoadOp::Load}else{wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)},store:wgpu::StoreOp::Store}})],depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None});
    pass.set_pipeline(pipeline);pass.set_bind_group(0,bg,&[]);pass.draw(0..3,0..1);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn history_resets_and_decays() {
        assert_eq!(retention(None,Vec3::ZERO,0.0),(0.0,true));
        assert_eq!(retention(Some((Vec3::ZERO,0.0)),Vec3::X*40.0,1.0),(0.0,true));
        assert_eq!(retention(Some((Vec3::ZERO,2.0)),Vec3::ZERO,1.0),(0.0,true));
        let (still,reset)=retention(Some((Vec3::ZERO,0.0)),Vec3::ZERO,1.0);assert!(!reset && still>0.0 && still<1.0);
        assert!(retention(Some((Vec3::ZERO,0.0)),Vec3::X,1.0).0<still);
    }
    #[test] fn history_requires_static_primary_and_unfiltered_sources() {
        let source=include_str!("material_reflections.wgsl");
        assert!(source.contains("m.b<0.999 || m.a<0.999"));
        assert!(source.contains("textureLoad(scene,p,0)"));
        assert!(!source.contains("textureSample"));
        assert!(source.contains("source.a>=0.999"));
        assert!(source.contains("(1.0-m.g)"));
    }
}
