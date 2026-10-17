//! Offscreen pixel comparison against the former split-pass attachment clear.
use super::*;

#[test]
fn ordered_depth_clear_matches_split_pass_pixels() {
    let instance=wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut r=Renderer::new(&instance,None).expect("depth-clear regression requires an offscreen GPU");
    let shader=r.device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label:Some("depth clear pixel fixture"),
        source:wgpu::ShaderSource::Wgsl(r#"
struct Out { @builtin(position) p: vec4<f32>, @location(0) color: vec4<f32> }
@vertex fn vs(@builtin(vertex_index) v:u32,@builtin(instance_index) stage:u32)->Out {
    var p=array<vec2<f32>,6>(vec2(-1.,-1.),vec2(1.,-1.),vec2(1.,1.),vec2(-1.,-1.),vec2(1.,1.),vec2(-1.,1.))[v];
    let depths=array<f32,5>(0.2,0.8,0.6,0.4,0.9);
    let colors=array<vec4<f32>,5>(vec4(1.,0.,0.,1.),vec4(0.,1.,0.,1.),vec4(0.,0.,1.,1.),vec4(1.,1.,1.,0.5),vec4(1.,1.,0.,1.));
    if stage==2u {p.x=(p.x+1.)*0.5;}
    if stage==3u {p.y=(p.y+1.)*0.5;}
    if stage==4u {p=p*0.25;}
    var out:Out;out.p=vec4(p,depths[stage],1.);out.color=colors[stage];return out;
}
@fragment fn fs(in:Out)->@location(0) vec4<f32> {return in.color;}
"#.into()),
    });
    // Exercise the actual native clear commands, including the no-actor-buffer case.
    r.screen.depth_clears=vec![
        ViewportDepthClear {priority:2,position:[0.0;3]},
        ViewportDepthClear {priority:6,position:[0.0;3]},
    ];
    r.prepare_actors(Vec3::ZERO,-Vec3::Z,&[Vec4::ZERO;6]);
    let client=ao_gui::client_dir();
    let gui_fixture=if client.join("cd_image/gui").exists() {
        let mut gui=ao_gui::Gui::new(&client,None).expect("installed native GUI");
        gui.set_screen_size(64,64);
        let win=gui.open_window_xml("depth-clear-pixels",
            "<root><TextView name=\"text\" min_size=\"Point(16,16)\" max_size=\"Point(16,16)\"/></root>",
            (0,0),ao_gui::WindowSize::Fixed(16,16)).unwrap();
        gui.set_text(win,"text","M");
        let mut list=gui.frame(0.0);
        assert!(list.cmds.iter().any(|cmd|matches!(cmd,ao_gui::DrawCmd::Glyph {..})),
            "native TextView must emit glyphs for composed pixel coverage");
        // Constrain every native clip transition to the reserved top-left fixture area.
        // All depth/blend expected-pixel probes remain outside this area.
        list.cmds.insert(0,ao_gui::DrawCmd::Clip(Some([0,0,16,16])));
        for cmd in &mut list.cmds {
            if let ao_gui::DrawCmd::Clip(clip)=cmd {
                let [l,t,r,b]=clip.unwrap_or([0,0,16,16]);
                *clip=Some([l.clamp(0,16),t.clamp(0,16),r.clamp(0,16),b.clamp(0,16)]);
            }
        }
        Some((gui,list))
    } else {
        eprintln!("native GUI pixel overlay skipped: installed cd_image/gui unavailable; asset-free depth/MSAA comparison still runs");
        None
    };
    for samples in r.msaa_supported.clone() {
        r.set_msaa_samples(samples);
        let layout=r.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label:None,bind_group_layouts:&[],immediate_size:0,
        });
        let pipe=r.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label:Some("depth clear pixel fixture"),layout:Some(&layout),
            vertex:wgpu::VertexState {module:&shader,entry_point:Some("vs"),compilation_options:Default::default(),buffers:&[]},
            fragment:Some(wgpu::FragmentState {module:&shader,entry_point:Some("fs"),compilation_options:Default::default(),
                targets:&[Some(wgpu::ColorTargetState {format:r.format,blend:Some(wgpu::BlendState::ALPHA_BLENDING),write_mask:wgpu::ColorWrites::ALL})]}),
            primitive:Default::default(),
            depth_stencil:Some(wgpu::DepthStencilState {format:DEPTH,depth_write_enabled:Some(true),depth_compare:Some(wgpu::CompareFunction::LessEqual),stencil:Default::default(),bias:Default::default()}),
            multisample:wgpu::MultisampleState {count:samples,..Default::default()},multiview_mask:None,cache:None,
        });
        let mut gui_renderer=gui_fixture.as_ref().map(|(gui,_)|GuiRenderer::new(&r,gui));
        let mut render=|reference:bool,overlay:bool| {
            let out=r.device.create_texture_tracked(&wgpu::TextureDescriptor {
                label:None,size:wgpu::Extent3d {width:64,height:64,depth_or_array_layers:1},
                mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:r.format,
                usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::COPY_SRC,view_formats:&[],
            });
            let resolve=out.create_view_tracked(&Default::default());
            let mut targets=Targets::new(&r,64,64);
            if reference {
                // Unlike the new single pass, the old reference needs backed attachments.
                let attachment=|format| r.device.create_texture_tracked(&wgpu::TextureDescriptor {
                    label:Some("split-pass reference"),size:wgpu::Extent3d {width:64,height:64,depth_or_array_layers:1},
                    mip_level_count:1,sample_count:samples,dimension:wgpu::TextureDimension::D2,format,
                    usage:wgpu::TextureUsages::RENDER_ATTACHMENT,view_formats:&[],
                }).create_view_tracked(&Default::default());
                targets.depth=attachment(DEPTH);
                targets.msaa=(samples>1).then(||attachment(r.format));
            }
            let mut enc=r.device.create_command_encoder(&Default::default());
            for segment in 0..if reference {3}else{1} {
                let last=!reference||segment==2;
                let mut pass=enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label:Some("depth clear reference comparison"),
                    color_attachments:&[Some(wgpu::RenderPassColorAttachment {
                        view:targets.msaa.as_ref().unwrap_or(&resolve),
                        resolve_target:if last&&samples>1 {Some(&resolve)}else{None},depth_slice:None,
                        ops:wgpu::Operations {load:if segment==0 {wgpu::LoadOp::Clear(wgpu::Color::BLACK)}else{wgpu::LoadOp::Load},
                            store:if last&&samples>1 {wgpu::StoreOp::Discard}else{wgpu::StoreOp::Store}},
                    })],
                    depth_stencil_attachment:Some(wgpu::RenderPassDepthStencilAttachment {
                        view:&targets.depth,depth_ops:Some(wgpu::Operations {load:wgpu::LoadOp::Clear(1.0),
                            store:if last {wgpu::StoreOp::Discard}else{wgpu::StoreOp::Store}}),stencil_ops:None,
                    }),timestamp_writes:None,occlusion_query_set:None,multiview_mask:None,
                });
                let stages=if !reference {0..5}else{match segment {0=>0..2,1=>2..4,_=>4..5}};
                for stage in stages {
                    if !reference {
                        if stage==2 {r.draw_actors(&mut pass,2);}
                        if stage==4 {r.draw_actors(&mut pass,6);}
                    }
                    // The clear replaces pipeline state; subsequent draws must rebind.
                    pass.set_pipeline(&pipe);
                    pass.draw(0..if stage==4 {3}else{6},stage..stage+1);
                }
            }
            r.queue.submit([enc.finish()]);
            if overlay {
                if let (Some((gui,list)),Some(renderer))=(&gui_fixture,&mut gui_renderer) {
                    renderer.draw(&r,&resolve,(64,64),1,gui,list);
                }
            }
            texture_to_rgba(&r,&out,64,64).unwrap()
        };
        let pixels=render(false,true);
        assert_eq!(pixels,render(true,true),"single-pass vs attachment clear plus native GUI at {samples}x MSAA");
        if gui_fixture.is_some() {
            let without_gui=render(false,false);
            assert!(pixels!=without_gui,"native GUI overlay must change actual RGBA pixels");
        }
        let pixel=|x:usize,y:usize| &pixels[(y*64+x)*4..(y*64+x+1)*4];
        assert_eq!(pixel(8,48),[255,0,0,255],"rejected green and clear must preserve red");
        assert_eq!(pixel(56,48),[0,0,255,255],"blue behind former red depth must pass after clear");
        for (actual,expected) in pixel(56,8).iter().zip([128u8,128,255,255]) {
            assert!(actual.abs_diff(expected)<=1,"ordered alpha blend: {:?}",pixel(56,8));
        }
    }
}
