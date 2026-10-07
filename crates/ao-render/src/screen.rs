//! DS10023929/1002eec0: repeated native gamma-space viewport sprites.
use super::*;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenLayer {
    pub color: [f32;4],
    pub source_blend:u32,
    pub destination_blend:u32,
    pub repetitions:u32,
    pub texture:Option<TextureKey>,
    /// UV offset and signed span; negative V span implements native V flip.
    pub uv:[f32;4],
}
impl Default for ScreenLayer {
    fn default()->Self {Self {color:[1.0;4],source_blend:2,destination_blend:1,repetitions:0,texture:None,uv:[0.0,0.0,1.0,1.0]}}
}
/// Native depth-only viewport clear (DS1000bc0b, Randy1004b75a).
#[derive(Clone,Copy,Debug,Default)]
pub struct ViewportDepthClear {pub priority:i32,pub position:[f32;3]}
#[derive(Default)]
pub(super) struct ScreenPass {
    pub layers:Vec<ScreenLayer>,
    pub fog_disabled:bool,
    pub depth_clears:Vec<ViewportDepthClear>,
    gpu:Option<ScreenGpu>,
}
struct ScreenGpu {
    layout:wgpu::BindGroupLayout,
    shader:wgpu::ShaderModule,
    sampler:wgpu::Sampler,
    textures:HashMap<Option<TextureKey>,wgpu::TextureView>,
    pipes:HashMap<(u32,u32),wgpu::RenderPipeline>,
    frames:Vec<(wgpu::Buffer,wgpu::BindGroup,Option<TextureKey>)>,
}
fn factor(value:u32)->wgpu::BlendFactor {
    use wgpu::BlendFactor::*;
    match value {1=>Zero,2=>One,3=>Src,4=>OneMinusSrc,5|12=>SrcAlpha,6|13=>OneMinusSrcAlpha,7=>DstAlpha,8=>OneMinusDstAlpha,9=>Dst,10=>OneMinusDst,11=>SrcAlphaSaturated,_=>Zero}
}
impl ScreenPass {
    pub fn upload(&mut self,device:&wgpu::Device,queue:&wgpu::Queue,key:TextureKey,texture:&ao_scene::Texture) {
        self.init(device,queue);
        let gpu=self.gpu.as_mut().unwrap();
        if gpu.textures.contains_key(&Some(key)) {return;}
        gpu.textures.insert(Some(key),upload(device,queue,texture));
    }
    fn init(&mut self,device:&wgpu::Device,queue:&wgpu::Queue) {
        if self.gpu.is_some() {return;}
        let layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {label:None,entries:&[
            wgpu::BindGroupLayoutEntry {binding:0,visibility:wgpu::ShaderStages::VERTEX_FRAGMENT,ty:wgpu::BindingType::Buffer {ty:wgpu::BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None},count:None},
            wgpu::BindGroupLayoutEntry {binding:1,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Texture {sample_type:wgpu::TextureSampleType::Float {filterable:true},view_dimension:wgpu::TextureViewDimension::D2,multisampled:false},count:None},
            wgpu::BindGroupLayoutEntry {binding:2,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),count:None}]});
        let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor {label:None,source:wgpu::ShaderSource::Wgsl(include_str!("screen.wgsl").into())});
        let sampler=device.create_sampler(&wgpu::SamplerDescriptor {address_mode_u:wgpu::AddressMode::Repeat,address_mode_v:wgpu::AddressMode::Repeat,mag_filter:wgpu::FilterMode::Linear,min_filter:wgpu::FilterMode::Linear,mipmap_filter:wgpu::MipmapFilterMode::Linear,..Default::default()});
        let white=ao_scene::Texture {width:1,height:1,rgba:vec![255;4]};
        let mut textures=HashMap::new();textures.insert(None,upload(device,queue,&white));
        self.gpu=Some(ScreenGpu {layout,shader,sampler,textures,pipes:HashMap::new(),frames:vec![]});
    }
    pub fn prepare(&mut self,device:&wgpu::Device,queue:&wgpu::Queue,format:wgpu::TextureFormat,_size:(u32,u32)) {
        if self.layers.is_empty() {return;}
        self.init(device,queue);
        let gpu=self.gpu.as_mut().unwrap();
        for (index,layer) in self.layers.iter().enumerate() {
            let key=(layer.source_blend,layer.destination_blend);
            gpu.pipes.entry(key).or_insert_with(|| {
                let destination=match key.0 {12=>6,13=>5,_=>key.1};
                let component=wgpu::BlendComponent {src_factor:factor(key.0),dst_factor:factor(destination),operation:wgpu::BlendOperation::Add};
                let pl=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {label:None,bind_group_layouts:&[Some(&gpu.layout)],immediate_size:0});
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {label:Some("native viewport blend"),layout:Some(&pl),vertex:wgpu::VertexState {module:&gpu.shader,entry_point:Some("vs"),compilation_options:Default::default(),buffers:&[]},fragment:Some(wgpu::FragmentState {module:&gpu.shader,entry_point:Some("fs"),compilation_options:Default::default(),targets:&[Some(wgpu::ColorTargetState {format,blend:Some(wgpu::BlendState {color:component,alpha:component}),write_mask:wgpu::ColorWrites::ALL})]}),primitive:Default::default(),depth_stencil:None,multisample:Default::default(),multiview_mask:None,cache:None})
            });
            let data=[layer.color,layer.uv];
            if index>=gpu.frames.len() {
                let buffer=device.create_buffer(&wgpu::BufferDescriptor {label:None,size:32,usage:wgpu::BufferUsages::UNIFORM|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false});
                let bind=bind(device,gpu,&buffer,layer.texture);
                gpu.frames.push((buffer,bind,layer.texture));
            } else if gpu.frames[index].2!=layer.texture {
                gpu.frames[index].1=bind(device,gpu,&gpu.frames[index].0,layer.texture);
                gpu.frames[index].2=layer.texture;
            }
            queue.write_buffer(&gpu.frames[index].0,0,bytemuck::cast_slice(&data));
        }
    }
    pub fn draw(&self,enc:&mut wgpu::CommandEncoder,resolve:&wgpu::TextureView) {
        if self.layers.is_empty() {return;}
        let Some(gpu)=&self.gpu else {return};
        let mut pass=enc.begin_render_pass(&wgpu::RenderPassDescriptor {label:Some("native viewport sprites"),color_attachments:&[Some(wgpu::RenderPassColorAttachment {view:resolve,resolve_target:None,depth_slice:None,ops:wgpu::Operations {load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store}})],depth_stencil_attachment:None,timestamp_writes:None,occlusion_query_set:None,multiview_mask:None});
        for (index,layer) in self.layers.iter().enumerate() {
            pass.set_pipeline(&gpu.pipes[&(layer.source_blend,layer.destination_blend)]);pass.set_bind_group(0,&gpu.frames[index].1,&[]);
            for _ in 0..layer.repetitions {pass.draw(0..3,0..1);}
        }
    }
}
fn bind(device:&wgpu::Device,gpu:&ScreenGpu,buffer:&wgpu::Buffer,key:Option<TextureKey>)->wgpu::BindGroup {
    let texture=gpu.textures.get(&key).expect("authored screen texture must be uploaded");
    device.create_bind_group(&wgpu::BindGroupDescriptor {label:None,layout:&gpu.layout,entries:&[wgpu::BindGroupEntry {binding:0,resource:buffer.as_entire_binding()},wgpu::BindGroupEntry {binding:1,resource:wgpu::BindingResource::TextureView(texture)},wgpu::BindGroupEntry {binding:2,resource:wgpu::BindingResource::Sampler(&gpu.sampler)}]})
}
fn upload(device:&wgpu::Device,queue:&wgpu::Queue,image:&ao_scene::Texture)->wgpu::TextureView {
    let texture=device.create_texture(&wgpu::TextureDescriptor {label:Some("native gamma screen texture"),size:wgpu::Extent3d {width:image.width,height:image.height,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:wgpu::TextureFormat::Rgba8Unorm,usage:wgpu::TextureUsages::TEXTURE_BINDING|wgpu::TextureUsages::COPY_DST,view_formats:&[]});
    queue.write_texture(wgpu::TexelCopyTextureInfo {texture:&texture,mip_level:0,origin:wgpu::Origin3d::ZERO,aspect:wgpu::TextureAspect::All},&image.rgba,wgpu::TexelCopyBufferLayout {offset:0,bytes_per_row:Some(image.width*4),rows_per_image:Some(image.height)},wgpu::Extent3d {width:image.width,height:image.height,depth_or_array_layers:1});
    texture.create_view(&Default::default())
}

pub(super) fn world_pass<'a>(enc:&'a mut wgpu::CommandEncoder,t:&'a Targets,resolve:&'a wgpu::TextureView,clear_depth:bool)->wgpu::RenderPass<'a> {
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {label:Some("ordered native viewport"),color_attachments:&[Some(wgpu::RenderPassColorAttachment {view:&t.msaa,resolve_target:Some(resolve),depth_slice:None,ops:wgpu::Operations {load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store}})],depth_stencil_attachment:Some(wgpu::RenderPassDepthStencilAttachment {view:&t.depth,depth_ops:Some(wgpu::Operations {load:if clear_depth {wgpu::LoadOp::Clear(1.0)}else {wgpu::LoadOp::Load},store:wgpu::StoreOp::Store}),stencil_ops:None}),timestamp_writes:None,occlusion_query_set:None,multiview_mask:None})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_viewport_blend_factors_preserve_authored_destination_color() {
        assert_eq!(factor(9),wgpu::BlendFactor::Dst);
        assert_eq!(factor(3),wgpu::BlendFactor::Src);
        assert_eq!(factor(10),wgpu::BlendFactor::OneMinusDst);
        assert_eq!(ScreenLayer::default().uv,[0.0,0.0,1.0,1.0]);
    }
    #[test]
    #[ignore="offscreen Metal viewport texture regression"]
    fn native_textured_viewport_modulates_gamma_rgba() {
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).expect("AOMAC_EFFECT_FRAMES");
        std::fs::create_dir_all(&out).unwrap();
        let path=out.join("screen-texture-modulate.png");
        let key=TextureKey {rdb_type:1010004,id:13};
        let mut scene=Scene::default();
        scene.textures.insert(key,ao_scene::Texture {width:1,height:1,rgba:vec![255,0,0,255]});
        super::super::render_to_png_screen(&scene,vec![ScreenLayer {texture:Some(key),color:[0.5,1.0,1.0,1.0],repetitions:1,..Default::default()}],false,64,64,&path).unwrap();
        let mut reader=png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap())).read_info().unwrap();
        let mut pixels=vec![0;reader.output_buffer_size()];
        let info=reader.next_frame(&mut pixels).unwrap();
        assert_eq!(info.color_type,png::ColorType::Rgba);
        let center=(32*64+32)*4;
        assert!((i32::from(pixels[center])-128).abs()<=1);
        assert_eq!(&pixels[center+1..center+4],&[0,0,255]);
    }
}
