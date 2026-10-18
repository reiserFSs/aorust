//! Standalone synthetic reflection evidence; no installed assets or live session.
use crate::*;
use std::fmt::Write;
fn quad(points:[[f32;3];4], normal:[f32;3], colour:[f32;3], glossy:bool)->ao_scene::Mesh {
    ao_scene::Mesh {vertices:points.into_iter().map(|pos|ao_scene::Vertex {pos,normal,color:[1.0;4],..Default::default()}).collect(),submeshes:vec![ao_scene::Submesh {two_sided:true,base_color:[colour[0],colour[1],colour[2],1.0],emissive:colour,specular:if glossy {[1.0;3]}else{[0.0;3]},shininess:64.0,..ao_scene::Submesh::new(vec![0,1,2,0,2,3],None)}]}
}
pub(super) fn room()->Scene {
    let mut scene=Scene::default();
    scene.meshes=vec![
        quad([[-8.0,0.0,3.0],[8.0,0.0,3.0],[8.0,0.0,-14.0],[-8.0,0.0,-14.0]],[0.0,1.0,0.0],[0.12;3],true),
        quad([[-8.0,0.0,-12.0],[8.0,0.0,-12.0],[8.0,8.0,-12.0],[-8.0,8.0,-12.0]],[0.0,0.0,1.0],[0.1,0.25,0.9],false),
        quad([[-8.0,0.0,3.0],[-8.0,0.0,-12.0],[-8.0,8.0,-12.0],[-8.0,8.0,3.0]],[1.0,0.0,0.0],[0.9,0.08,0.03],false)];
    scene.instances=(0..scene.meshes.len()).map(|mesh|ao_scene::Instance {mesh,transform:Mat4::IDENTITY.to_cols_array_2d()}).collect();
    scene.environment=Some(ao_scene::Environment {sky_color:[0.12,0.2,0.35],fog_color:[0.15;3],ambient:[0.1;3],fog_start:80.0,fog_end:120.0,sun_color:[0.0;3],sun_dir:[0.0,1.0,0.0],sun_specular:0.0});
    scene
}
#[test]
#[ignore = "GPU reflection fairness and optional AOMAC_GFX_OUT ABBA/contact evidence; run alone"]
fn observed_material_reflections_gpu_evidence()->anyhow::Result<()> {
    let instance=wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let camera=Camera::look_at(Vec3::new(0.0,2.5,4.0),Vec3::new(0.0,0.5,-7.0));
    let profiles=[EnhancedSettings::default(),EnhancedSettings {material_reflections:true,..Default::default()},EnhancedSettings {environment_probe:true,..Default::default()}];
    let names=["off","material-reflections","environment-probe"];
    let mut renderers=Vec::new();let mut outputs=Vec::new();
    let mut temporal=Vec::new();
    for settings in profiles {
        let mut r=Renderer::new(&instance,None)?;r.upload(&room());r.set_enhanced_settings(settings);r.wait_for_pipeline_warmup();
        let texture=r.device.create_texture_tracked(&wgpu::TextureDescriptor {label:Some("reflection evidence"),size:wgpu::Extent3d {width:256,height:192,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:r.format,usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::COPY_SRC,view_formats:&[]});
        let view=texture.create_view(&Default::default());let targets=Targets::new(&r,256,192);
        r.render(&view,&targets,&camera);r.wait_for_pipeline_warmup();
        if let Some(state)=&mut r.material_reflections {state.forget_history();}
        for frame in 0..40 {
            r.time=frame as f32/60.0;r.render(&view,&targets,&camera);
            if settings.environment_probe && [0,7,39].contains(&frame) {temporal.push(texture_to_rgba(&r,&texture,256,192)?);}
        }
        renderers.push(r);outputs.push((texture,view,targets));
    }
    let mut pixels=Vec::new();
    for (r,(texture,_,_)) in renderers.iter().zip(&outputs) {pixels.push(texture_to_rgba(r,texture,256,192)?);}
    assert_ne!(pixels[0],pixels[1],"glossy fixture must visibly exercise SSR/analytic fallback");
    assert_ne!(pixels[0],pixels[2],"probe alone must function as reflected environment");
    assert_ne!(temporal[0],temporal[2],"static probe must visibly learn between frames 1 and 40");
    let probe_pixels=read_probe(&renderers[2])?;
    assert!(probe_pixels.as_chunks::<4>().0.iter().any(|p|p[3]>128),"observed static directions must acquire substantial confidence");
    let mut encoder=renderers[2].device.create_command_encoder(&Default::default());
    renderers[2].material_reflections.as_ref().unwrap().analytic_comparison(&renderers[2],&mut encoder);
    renderers[2].enhanced_frame.as_ref().unwrap().composite(&mut encoder,&outputs[2].1,None,None);
    renderers[2].queue.submit(Some(encoder.finish()));
    let analytic=texture_to_rgba(&renderers[2],&outputs[2].0,256,192)?;
    assert_ne!(pixels[2],analytic,"observed reflected colours must differ from analytic fallback, not merely off");
    // Hidden actors cannot seed current reflection or accumulated history.
    let model=Scene {meshes:vec![quad([[-2.0,-2.0,0.0],[2.0,-2.0,0.0],[2.0,2.0,0.0],[-2.0,2.0,0.0]],[0.0,0.0,1.0],[0.0,1.0,0.0],false)],..Default::default()};
    for index in 1..3 {
        let r=&mut renderers[index];let (texture,view,targets)=&outputs[index];
        r.add_actor_model(99,&model);
        for (name,x,z,alpha) in [("offscreen",200.0,-8.0,1.0),("alpha-zero",0.0,-8.0,0.0),("occluded",0.0,-14.0,1.0)] {
            r.upload(&room());
            let mut actor=ao_scene::ActorFrame {id:99,model:99,alpha,..Default::default()};actor.transform[3][0]=x;actor.transform[3][2]=z;
            r.set_actors(vec![actor]);r.time=0.0;r.render(view,targets,&camera);r.wait_for_pipeline_warmup();
            r.material_reflections.as_mut().unwrap().forget_history();
            for frame in 0..40 {r.time=frame as f32/60.0;r.render(view,targets,&camera);}
            assert_eq!(pixels[index],texture_to_rgba(r,texture,256,192)?,"{name} actor must leave no reflected signal");
        }
        r.set_actors(Vec::new());r.remove_actor_model(99);
    }
    // All-off cutover retires attachments/history and restores exact native bytes.
    let r=&mut renderers[1];r.set_enhanced_settings(EnhancedSettings::default());r.render(&outputs[1].1,&outputs[1].2,&camera);
    assert!(r.material_reflections.is_none());assert!(r.enhanced_frame.is_none());
    assert_eq!(pixels[0],texture_to_rgba(r,&outputs[1].0,256,192)?);
    r.set_enhanced_settings(profiles[1]);r.render(&outputs[1].1,&outputs[1].2,&camera);r.wait_for_pipeline_warmup();
    if let Some(out)=std::env::var_os("AOMAC_GFX_OUT") {
        let out=std::path::PathBuf::from(out);std::fs::create_dir_all(&out)?;
        let mut strip=vec![0u8;256*3*192*4];
        for (index,rgba) in pixels.iter().enumerate() {
            save_pixels(&out.join(format!("{}.png",names[index])),rgba,256,192)?;
            for y in 0..192 {let dst=(y*768+index*256)*4;strip[dst..dst+1024].copy_from_slice(&rgba[y*1024..(y+1)*1024]);}
        }
        save_pixels(&out.join("reflection-contact-strip.png"),&strip,768,192)?;
        let mut learning=vec![0u8;768*192*4];
        for (index,rgba) in temporal.iter().enumerate() {save_pixels(&out.join(format!("probe-frame-{}.png",[1,8,40][index])),rgba,256,192)?;for y in 0..192 {let dst=(y*768+index*256)*4;learning[dst..dst+1024].copy_from_slice(&rgba[y*1024..(y+1)*1024]);}}
        save_pixels(&out.join("probe-learning-strip.png"),&learning,768,192)?;
        save_pixels(&out.join("probe-confidence.png"),&probe_pixels,128,128)?;
        save_pixels(&out.join("probe-analytic-fallback.png"),&analytic,256,192)?;
        let mut samples=String::from("feature,pair,profile,gpu_frame_ms\n");
        let mut totals=[[0.0f64;2];3];
        for r in &mut renderers {r.set_gpu_timing(true)?;}
        for feature in 1..3 {for pair in 0..80 {for index in if pair%2==0 {[0,feature]}else{[feature,0]} {
            let r=&mut renderers[index];let (_,view,targets)=&outputs[index];r.render(view,targets,&camera);
            let ms=r.read_frame_gpu_ms()?.expect("explicit GPU timing enabled");if pair>=16 {totals[feature][usize::from(index!=0)]+=ms;writeln!(samples,"{},{},{},{ms:.9}",names[feature],pair-16,names[index])?;}
        }}}
        std::fs::write(out.join("reflection-abba.csv"),samples)?;
        let mut deltas=String::from("feature,off_mean_gpu_ms,on_mean_gpu_ms,delta_gpu_ms\n");
        for feature in 1..3 {let off=totals[feature][0]/64.0;let on=totals[feature][1]/64.0;writeln!(deltas,"{},{off:.9},{on:.9},{:.9}",names[feature],on-off)?;}
        std::fs::write(out.join("reflection-abba-deltas.csv"),deltas)?;
    }
    Ok(())
}
fn save_pixels(path:&std::path::Path,pixels:&[u8],width:u32,height:u32)->anyhow::Result<()> {
    let mut enc=png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?),width,height);
    enc.set_color(png::ColorType::Rgba);enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(pixels)?;Ok(())
}
fn read_probe(r:&Renderer)->anyhow::Result<Vec<u8>> {
    let output=r.device.create_texture_tracked(&wgpu::TextureDescriptor {label:Some("static probe confidence evidence"),size:wgpu::Extent3d {width:128,height:128,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:r.format,usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::COPY_SRC,view_formats:&[]});
    let bind=r.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {label:Some("static probe readback"),layout:r.enhanced_pipelines.source_layout(),entries:&[wgpu::BindGroupEntry {binding:0,resource:wgpu::BindingResource::TextureView(r.material_reflections.as_ref().unwrap().history_view())}]});
    let mut encoder=r.device.create_command_encoder(&Default::default());r.enhanced_frame.as_ref().unwrap().composite(&mut encoder,&output.create_view(&Default::default()),Some(&bind),None);r.queue.submit(Some(encoder.finish()));
    texture_to_rgba(r,&output,128,128)
}

#[test]
#[ignore = "GPU static-only history admission, scene reset and full MRT combination"]
fn reflection_history_excludes_current_visible_actors_and_effects()->anyhow::Result<()> {
    let instance=wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut r=Renderer::new(&instance,None)?;
    r.set_enhanced_settings(EnhancedSettings {environment_probe:true,..Default::default()});
    let model=Scene {meshes:vec![quad([[-4.0,-4.0,0.0],[4.0,-4.0,0.0],[4.0,4.0,0.0],[-4.0,4.0,0.0]],[0.0,0.0,1.0],[0.0,1.0,0.0],true)],..Default::default()};
    r.add_actor_model(9,&model);
    let output=r.device.create_texture_tracked(&wgpu::TextureDescriptor {label:Some("probe actor exclusion"),size:wgpu::Extent3d {width:128,height:128,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:r.format,usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::COPY_SRC,view_formats:&[]});
    let view=output.create_view(&Default::default());let camera=Camera::look_at(Vec3::ZERO,Vec3::NEG_Z);
    for effect in [0u32,1,5,7] {
        r.upload(&Scene::default());
        let mut actor=ao_scene::ActorFrame {id:9,model:9,rendering_effect:effect,..Default::default()};actor.transform[3][2] = -5.0;r.set_actors(vec![actor]);
        let targets=Targets::new(&r,128,128);r.render(&view,&targets,&camera);r.wait_for_pipeline_warmup();
        for frame in 0..40 {r.time=frame as f32/60.0;r.render(&view,&targets,&camera);}
        let state=r.material_reflections.as_ref().expect("probe allocated");
        let bind=r.device.create_bind_group_tracked(&wgpu::BindGroupDescriptor {label:Some("probe fairness readback"),layout:r.enhanced_pipelines.source_layout(),entries:&[wgpu::BindGroupEntry {binding:0,resource:wgpu::BindingResource::TextureView(state.history_view())}]});
        let mut encoder=r.device.create_command_encoder(&Default::default());
        r.enhanced_frame.as_ref().unwrap().composite(&mut encoder,&view,Some(&bind),None);r.queue.submit(Some(encoder.finish()));
        let pixels=texture_to_rgba(&r,&output,128,128)?;
        assert!(pixels.as_chunks::<4>().0.iter().all(|p|p[0]==0 && p[1]==0 && p[2]==0 && p[3]==0),"current visible actor/effect {effect} must never enter probe history");
    }
    r.upload(&room());assert!(r.material_reflections.is_none(),"scene replacement must discard history");
    r.set_actors(Vec::new());
    r.set_enhanced_settings(EnhancedSettings {material_reflections:true,environment_probe:true,ambient_occlusion:true,emissive_bloom:true,cloud_shadows:true,hdr:true,..Default::default()});
    r.wait_for_pipeline_warmup();
    let targets=Targets::new(&r,128,128);r.render(&view,&targets,&camera);r.wait_for_pipeline_warmup();r.render(&view,&targets,&camera);
    assert_eq!(r.enhanced_frame.as_ref().unwrap().target_count(),7);
    // Full native receiver extinction stays byte-identical even with reflection/probe active.
    let mut fogged=room();let env=fogged.environment.as_mut().unwrap();env.fog_start=0.0;env.fog_end=1.0;
    r.lens.far=Some(200.0);r.upload(&fogged);r.clear_actors();
    let mut fog_pixels=Vec::new();
    for enabled in [false,true] {
        r.set_enhanced_settings(EnhancedSettings {material_reflections:enabled,environment_probe:enabled,..Default::default()});
        let targets=Targets::new(&r,128,128);r.render(&view,&targets,&camera);r.wait_for_pipeline_warmup();
        for _ in 0..4 {r.render(&view,&targets,&camera);}
        fog_pixels.push(texture_to_rgba(&r,&output,128,128)?);
    }
    assert_eq!(fog_pixels[0],fog_pixels[1],"reflection cannot reveal receivers under full native fog");
    Ok(())
}
