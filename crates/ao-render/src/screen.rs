//! DS10023929/1002eec0: repeated native gamma-space viewport sprites.
use super::*;
use crate::pipeline_creation::RenderPipeline;
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
    pipes:HashMap<(u32,u32),RenderPipeline>,
    frames:Vec<(wgpu::Buffer,wgpu::BindGroup,Option<TextureKey>)>,
}
fn factor(value:u32)->wgpu::BlendFactor {
    use wgpu::BlendFactor::*;
    match value {1=>Zero,2=>One,3=>Src,4=>OneMinusSrc,5|12=>SrcAlpha,6|13=>OneMinusSrcAlpha,7=>DstAlpha,8=>OneMinusDstAlpha,9=>Dst,10=>OneMinusDst,11=>SrcAlphaSaturated,_=>Zero}
}
fn blend_key(source: u32, destination: u32) -> (u32, u32) {
    let destination = match source { 12 => 6, 13 => 5, _ => destination };
    let canonical = |value| match value { 2..=11 => value, 12 => 5, 13 => 6, _ => 1 };
    (canonical(source), canonical(destination))
}
impl ScreenPass {
    pub fn is_ready(&self) -> bool {
        self.gpu.as_ref().is_some_and(|gpu| gpu.pipes.len() == 121 && gpu.pipes.values().all(RenderPipeline::is_ready))
    }
    pub fn wait(&self) {
        let gpu = self.gpu.as_ref().expect("screen pipelines warmed at startup");
        for pipeline in gpu.pipes.values() { pipeline.get(); }
    }

    pub fn upload(&mut self,device:&wgpu::Device,queue:&wgpu::Queue,key:TextureKey,texture:&ao_scene::Texture,policy:crate::native_texture::Policy) {
        self.init(device,queue);
        let gpu=self.gpu.as_mut().unwrap();
        if gpu.textures.contains_key(&Some(key)) {return;}
        gpu.textures.insert(Some(key),upload(device,queue,texture,policy));
    }
    fn init(&mut self,device:&wgpu::Device,queue:&wgpu::Queue) {
        if self.gpu.is_some() {return;}
        let layout=device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {label:None,entries:&[
            wgpu::BindGroupLayoutEntry {binding:0,visibility:wgpu::ShaderStages::VERTEX_FRAGMENT,ty:wgpu::BindingType::Buffer {ty:wgpu::BufferBindingType::Uniform,has_dynamic_offset:false,min_binding_size:None},count:None},
            wgpu::BindGroupLayoutEntry {binding:1,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Texture {sample_type:wgpu::TextureSampleType::Float {filterable:true},view_dimension:wgpu::TextureViewDimension::D2,multisampled:false},count:None},
            wgpu::BindGroupLayoutEntry {binding:2,visibility:wgpu::ShaderStages::FRAGMENT,ty:wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),count:None}]});
        let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor {label:None,source:wgpu::ShaderSource::Wgsl(crate::shader_source::load("screen.wgsl", include_str!("screen.wgsl")))});
        let sampler=device.create_sampler_tracked(&wgpu::SamplerDescriptor {address_mode_u:wgpu::AddressMode::Repeat,address_mode_v:wgpu::AddressMode::Repeat,mag_filter:wgpu::FilterMode::Linear,min_filter:wgpu::FilterMode::Linear,mipmap_filter:wgpu::MipmapFilterMode::Linear,..Default::default()});
        let white=ao_scene::Texture { source: Default::default(), width:1,height:1,rgba:vec![255;4] };
        let mut textures=HashMap::new();textures.insert(None,upload(device,queue,&white,Default::default()));
        self.gpu=Some(ScreenGpu {layout,shader,sampler,textures,pipes:HashMap::new(),frames:vec![]});
    }
    pub fn warm_pipelines(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) {
        self.init(device, queue);
        let gpu = self.gpu.as_mut().unwrap();
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(&gpu.layout)], immediate_size: 0 });
        for source in 1..=11 {
            for destination in 1..=11 {
                gpu.pipes.entry((source, destination)).or_insert_with(|| {
                    let component = wgpu::BlendComponent { src_factor: factor(source), dst_factor: factor(destination), operation: wgpu::BlendOperation::Add };
                    crate::pipeline_creation::render_deferred(device, &wgpu::RenderPipelineDescriptor {
                        label: Some("native viewport blend"), layout: Some(&pl),
                        vertex: wgpu::VertexState { module: &gpu.shader, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
                        fragment: Some(wgpu::FragmentState { module: &gpu.shader, entry_point: Some("fs"), compilation_options: Default::default(),
                            targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState { color: component, alpha: component }), write_mask: wgpu::ColorWrites::ALL })] }),
                        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview_mask: None, cache: None,
                    })
                });
            }
        }
    }
    pub fn prepare(&mut self,device:&wgpu::Device,queue:&wgpu::Queue,_format:wgpu::TextureFormat,_size:(u32,u32)) {
        if self.layers.is_empty() {return;}
        self.init(device,queue);
        let gpu=self.gpu.as_mut().unwrap();
        for (index,layer) in self.layers.iter().enumerate() {
            let data=[layer.color,layer.uv];
            if index>=gpu.frames.len() {
                let buffer=device.create_buffer_tracked(&wgpu::BufferDescriptor {label:None,size:32,usage:wgpu::BufferUsages::UNIFORM|wgpu::BufferUsages::COPY_DST,mapped_at_creation:false});
                let bind=bind(device,gpu,&buffer,layer.texture);
                gpu.frames.push((buffer,bind,layer.texture));
            } else if gpu.frames[index].2!=layer.texture {
                gpu.frames[index].1=bind(device,gpu,&gpu.frames[index].0,layer.texture);
                gpu.frames[index].2=layer.texture;
            }
            queue.write_buffer(&gpu.frames[index].0,0,bytemuck::cast_slice(&data));
        }
    }
    pub fn draw(&self,enc:&mut wgpu::CommandEncoder,resolve:&wgpu::TextureView,timestamps:Option<wgpu::RenderPassTimestampWrites<'_>>) {
        if self.layers.is_empty() {return;}
        let Some(gpu)=&self.gpu else {return};
        let mut pass=enc.begin_effect_render_pass(&wgpu::RenderPassDescriptor {label:Some("native viewport sprites"),color_attachments:&[Some(wgpu::RenderPassColorAttachment {view:resolve,resolve_target:None,depth_slice:None,ops:wgpu::Operations {load:wgpu::LoadOp::Load,store:wgpu::StoreOp::Store}})],depth_stencil_attachment:None,timestamp_writes:timestamps,occlusion_query_set:None,multiview_mask:None});
        for (index,layer) in self.layers.iter().enumerate() {
            pass.set_pipeline(&gpu.pipes[&blend_key(layer.source_blend,layer.destination_blend)]);pass.set_bind_group(0,&gpu.frames[index].1,&[]);
            for _ in 0..layer.repetitions {pass.draw(0..3,0..1);}
        }
    }
}
fn bind(device:&wgpu::Device,gpu:&ScreenGpu,buffer:&wgpu::Buffer,key:Option<TextureKey>)->wgpu::BindGroup {
    let texture=gpu.textures.get(&key).expect("authored screen texture must be uploaded");
    device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {label:None,layout:&gpu.layout,entries:&[wgpu::BindGroupEntry {binding:0,resource:buffer.as_entire_binding()},wgpu::BindGroupEntry {binding:1,resource:wgpu::BindingResource::TextureView(texture)},wgpu::BindGroupEntry {binding:2,resource:wgpu::BindingResource::Sampler(&gpu.sampler)}]})
}
fn upload(device:&wgpu::Device,queue:&wgpu::Queue,image:&ao_scene::Texture,policy:crate::native_texture::Policy)->wgpu::TextureView {
    let encoding=policy.encoding(image.source,device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC),image.width,image.height);
    let block=encoding.block_edge();
    let mut encoded=Vec::new();
    let bytes=if encoding==crate::native_texture::Encoding::Rgba8 {image.rgba.as_slice()} else {encoding.encode(&image.rgba,image.width,image.height,&mut encoded);encoded.as_slice()};
    let texture=device.create_texture_tracked(&wgpu::TextureDescriptor {label:Some("native gamma screen texture"),size:wgpu::Extent3d {width:image.width,height:image.height,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:encoding.format().remove_srgb_suffix(),usage:wgpu::TextureUsages::TEXTURE_BINDING|wgpu::TextureUsages::COPY_DST,view_formats:&[]});
    queue.write_texture(wgpu::TexelCopyTextureInfo {texture:&texture,mip_level:0,origin:wgpu::Origin3d::ZERO,aspect:wgpu::TextureAspect::All},bytes,wgpu::TexelCopyBufferLayout {offset:0,bytes_per_row:Some(image.width.div_ceil(block)*encoding.block_bytes()),rows_per_image:Some(image.height.div_ceil(block))},wgpu::Extent3d {width:image.width.div_ceil(block)*block,height:image.height.div_ceil(block)*block,depth_or_array_layers:1});
    texture.create_view_tracked(&Default::default())
}

/// DS1000bc0b/Randy1004b75a clears depth at an ordered render-list position.
/// Writing every sample at depth 1 with Always preserves that operation inside one world pass.
pub(super) fn depth_clear_pipeline(device:&wgpu::Device,format:wgpu::TextureFormat,samples:u32,settings:crate::EnhancedSettings)->wgpu::RenderPipeline {
    let shader=device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label:Some("native viewport depth clear"),
        source:wgpu::ShaderSource::Wgsl(crate::shader_source::load("depth_clear.wgsl", include_str!("depth_clear.wgsl"))),
    });
    let layout=device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label:Some("native viewport depth clear"),bind_group_layouts:&[],immediate_size:0,
    });
    let target = wgpu::ColorTargetState {format,blend:None,write_mask:wgpu::ColorWrites::empty()};
    let auxiliary=wgpu::ColorTargetState {format:crate::enhanced::AUX,blend:None,write_mask:wgpu::ColorWrites::empty()};
    let reflections=settings.material_reflections || settings.environment_probe;
    let mut targets = [Some(target), (settings.ambient_occlusion || settings.indirect_light).then(|| wgpu::ColorTargetState { format: crate::enhanced::ambient_format(&settings), ..auxiliary.clone() }), settings.emissive_bloom.then(|| auxiliary.clone()), settings.needs_sun_mask().then(|| wgpu::ColorTargetState { format: crate::enhanced::sun_format(&settings), ..auxiliary.clone() }), settings.cloud_shadows.then_some(wgpu::ColorTargetState { format: wgpu::TextureFormat::R16Float, blend: None, write_mask: wgpu::ColorWrites::empty() }), reflections.then(|| auxiliary.clone()), reflections.then_some(auxiliary), settings.indirect_light.then_some(wgpu::ColorTargetState { format: crate::enhanced::ALBEDO, blend: None, write_mask: wgpu::ColorWrites::empty() })];
    if crate::enhanced::compact_indirect(&settings) { targets.swap(2, 7); }
    crate::pipeline_creation::render(device, &wgpu::RenderPipelineDescriptor {
        label:Some("native viewport depth clear"),layout:Some(&layout),
        vertex:wgpu::VertexState {module:&shader,entry_point:Some("vs"),compilation_options:Default::default(),buffers:&[]},
        fragment:Some(wgpu::FragmentState {module:&shader,entry_point:Some("fs"),compilation_options:Default::default(),
            targets:&targets[..crate::enhanced::target_count(&settings)]}),
        primitive:Default::default(),
        depth_stencil:Some(wgpu::DepthStencilState {format:DEPTH,depth_write_enabled:Some(true),depth_compare:Some(wgpu::CompareFunction::Always),stencil:Default::default(),bias:Default::default()}),
        multisample:wgpu::MultisampleState {count:samples,..Default::default()},multiview_mask:None,cache:None,
    })
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
        for source in 0..=15 {
            for destination in 0..=15 {
                let (canonical_source, canonical_destination) = blend_key(source, destination);
                let authored_destination = match source { 12 => 6, 13 => 5, _ => destination };
                assert_eq!(factor(canonical_source), factor(source));
                assert_eq!(factor(canonical_destination), factor(authored_destination));
            }
        }
    }
    #[test]
    #[ignore="offscreen Metal viewport texture regression"]
    fn native_textured_viewport_modulates_gamma_rgba() {
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).expect("AOMAC_EFFECT_FRAMES");
        std::fs::create_dir_all(&out).unwrap();
        let path=out.join("screen-texture-modulate.png");
        let key=TextureKey {rdb_type:1010004,id:13};
        let mut scene=Scene::default();
        scene.textures.insert(key,ao_scene::Texture { source: Default::default(), width:1,height:1,rgba:vec![255,0,0,255] });
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
