//! Wet-floor reflection evidence uses the native accumulated wetness and collision shelter.
use crate::*;
#[test]
#[ignore = "GPU optional wet-floor reflection and native collision exposure regression"]
fn wet_reflections_require_accumulated_wetness_and_exposed_floor()->anyhow::Result<()> {
    let instance=wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let mut r=Renderer::new(&instance,None)?;
    let mut scene=super::material_reflection_tests::room();
    for material in scene.meshes.iter_mut().flat_map(|m|&mut m.submeshes) {material.specular=[0.0;3];material.shininess=0.0;}
    let camera=Camera::look_at(Vec3::new(0.0,2.5,4.0),Vec3::new(0.0,0.5,-7.0));
    let output=r.device.create_texture_tracked(&wgpu::TextureDescriptor {label:Some("wet reflection exposure"),size:wgpu::Extent3d {width:128,height:128,depth_or_array_layers:1},mip_level_count:1,sample_count:1,dimension:wgpu::TextureDimension::D2,format:r.format,usage:wgpu::TextureUsages::RENDER_ATTACHMENT|wgpu::TextureUsages::COPY_SRC,view_formats:&[]});let view=output.create_view(&Default::default());
    for (name,roof_height,rain,visible) in [("exposed wet",-1000.0,1.0,true),("roof sheltered",1000.0,1.0,false),("exposed dry",-1000.0,0.0,false)] {
        let mut pixels=Vec::new();
        for enabled in [false,true] {
            r.set_enhanced_settings(EnhancedSettings::default());r.upload(&scene);
            r.set_weather_input(WeatherInput {rain,outdoor:true,surface:Some(std::rc::Rc::new(move |_|Some([roof_height,0.0]))),..Default::default()});
            r.set_enhanced_settings(EnhancedSettings {wet_surfaces:enabled,material_reflections:true,..Default::default()});
            let targets=Targets::new(&r,128,128);r.time=0.0;r.render(&view,&targets,&camera);r.wait_for_pipeline_warmup();
            for frame in 0..8 {r.time=frame as f32/60.0;r.render(&view,&targets,&camera);}
            if enabled {
                let wet=r.rain_wet.as_ref().unwrap();
                assert_eq!(wet.reflection_wetness()>0.0,rain>0.0,"{name}: concrete native wetness state");
            }
            pixels.push(texture_to_rgba(&r,&output,128,128)?);
        }
        if visible {assert_ne!(pixels[0],pixels[1],"{name}: exposed native wetness must visibly contribute reflection");}
        else {assert_eq!(pixels[0],pixels[1],"{name}: dry/sheltered floor must not reflect from wet option");}
    }
    Ok(())
}
