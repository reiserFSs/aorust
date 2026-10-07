//! GroundGrid: GC1010e86a/1010e96a ctor,1010e66b loader,1010e3ff init,
//! 1010e1b2 process,100a719a graceful,1010e7ea delete; DS10016c68/100164b4/10016b9e.
use super::{materials, sprites, EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::weather::R250;
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};
fn f(t:&Template,i:usize)->f32 {f32::from_bits(t.words.get(i).copied().unwrap_or(0))}
pub(super) struct GroundGrid {
    template:Template, source:Mat4, origin:Vec3, rotation:Mat4, duration:f32,
    fade_start:f32, previous:f32, phase:f32, uv:[f32;4], points:Vec<Vec3>,
    cached:Vec<Vertex>, cached_origin:Vec3, initialized:bool, deleted:bool,
}
impl GroundGrid {
    pub(super) fn new(t:&Template,source:Mat4,c:EffectConfig,r:&mut R250)->Result<Self> {
        ensure!(t.kind==3030,"not GroundGrid");
        for i in [1,2,3,4,5,6,8,12,13,14,15,18,19,20,21,22,23,24,25,26] {ensure!(f(t,i).is_finite(),"nonfinite GroundGrid parameter {i}");}
        ensure!((2..=1024).contains(&t.word(10)?),"invalid GroundGrid resolution");
        ensure!(t.word(11)?<=2,"invalid GroundGrid attenuation mode");
        materials::MATERIALS.get(t.words.get(9).copied().unwrap_or(0) as usize).context("unknown GroundGrid material")?;
        let source=sprites::connector(t,source)?;
        let rotation=if t.words.first().copied().unwrap_or(0) & 0x20000!=0 {Mat4::from_rotation_y(-(super::random_fraction(r) as f64*f64::from_bits(0x401921fb60000000)) as f32)}else{Mat4::IDENTITY};
        let n=t.words.get(10).copied().unwrap_or(0) as usize;
        Ok(Self {template:t.clone(),source,origin:source.w_axis.truncate(),rotation,duration:c.duration.unwrap_or(f(t,8)),fade_start:f(t,8)-f(t,15),previous:0.0,phase:0.0,uv:[f(t,12),f(t,13),f(t,23),f(t,24)],points:vec![Vec3::ZERO;n*n],cached:Vec::new(),cached_origin:source.w_axis.truncate(),initialized:false,deleted:false})
    }
    pub(super) fn update_source(&mut self,m:Mat4)->Result<()> {self.source=sprites::connector(&self.template,m)?;Ok(())}
    pub(super) fn graceful(&mut self) {self.deleted=true;}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let n=self.template.words.get(10).copied().unwrap_or(0) as usize;let mut indices=Vec::with_capacity((n-1)*(n-1)*6);
        for row in 0..n-1 {let start=row*n*2;for i in 0..n*2-2 {let a=(start+i) as u32;indices.extend(if i&1==0 {[a,a+1,a+2]}else{[a+1,a,a+2]});}}
        vec![(Some(self.template.words.get(9).copied().unwrap_or(0) as usize),indices,(n-1)*n*2)]
    }
    pub(super) fn blends(&self)->Vec<Blend> {let flags=self.template.words.first().copied().unwrap_or(0);vec![if flags&0x200!=0 && flags&0x400==0 {Blend::Additive}else{Blend::AlphaBlend}]}
    fn locate(&mut self,terrain:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>,initial:bool)->Result<()> {
        let t=&self.template;let n=t.words.get(10).copied().unwrap_or(0) as usize;let half=(n-1) as f32*f(t,18)*0.5;
        if !initial {self.origin=self.source.w_axis.truncate();}
        for row in 0..n {for col in 0..n {
            let mut p=Vec3::new(col as f32*f(t,18)-half,0.0,-(row as f32*f(t,18)-half));
            if !initial {p=self.rotation.transform_vector3(p);}
            let ground=terrain(self.origin+p).context("GroundGrid requires actual terrain height")?;
            p.y=ground.0.y+f(t,19)-self.origin.y;self.points[row*n+col]=p;
        }}Ok(())
    }
    fn rebuild(&mut self) {
        let t=&self.template;let n=t.words.get(10).copied().unwrap_or(0) as usize;let center=(n-1) as f32*0.5;
        let uv_center=if t.words.first().copied().unwrap_or(0) & 0x8000!=0 {-center}else{0.0};
        let [a,r,g,b]=t.words.get(16).copied().unwrap_or(0).to_be_bytes();let rgb=[r,g,b].map(|x|(x as f32/255.0).powf(2.2));
        self.cached.clear();
        self.cached_origin=self.origin;
        for row in 0..n-1 {for col in 0..n {for y in [row,row+1] {
            let attenuation=match t.words.get(11).copied().unwrap_or(0) { 1=>((col as f32-n as f32*0.5).abs()+(y as f32-n as f32*0.5).abs())/(n as f32*0.5),2=>Vec3::new(col as f32-center,y as f32-center,0.0).length()/center,_=>0.0 };
            let wave=(((attenuation as f64*32.0+self.phase as f64)*0.5).sin() as f32+1.0)*0.5;
            let alpha=(1.0-attenuation).max(0.0)*a as f32/255.0*if t.words.first().copied().unwrap_or(0) & 0x4000!=0 {wave}else{1.0};
            let byte=(alpha*255.0) as i32 as u8;
            let mut p=self.points[y*n+col];if t.words.first().copied().unwrap_or(0) & 0x2000!=0 {p.y+=wave*0.5;}
            self.cached.push(Vertex {pos:(self.origin+p).to_array(),normal:[0.0,1.0,0.0],uv:[(col as f32+uv_center)/(n-1) as f32*self.uv[0]+self.uv[2],(y as f32+uv_center)/(n-1) as f32*self.uv[1]+self.uv[3]],color:[rgb[0],rgb[1],rgb[2],byte as f32/255.0]});
        }}} 
    }
    pub(super) fn vertices(&mut self,time:f32,terrain:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.deleted || (self.duration>=0.0 && time>self.duration) {return Ok(None);}
        let dt=(time-self.previous).max(0.0);self.previous=time;
        if !self.initialized {self.locate(terrain,self.template.words.first().copied().unwrap_or(0) & 0x800!=0)?;self.rebuild();self.initialized=true;}
        if self.template.words.first().copied().unwrap_or(0) & 0x800==0 {self.locate(terrain,false)?;}
        for (i,word) in [21,22,25,26].into_iter().enumerate() {self.uv[i]+=f(&self.template,word)*dt;}
        if [21,22,25,26].iter().any(|&i|f(&self.template,i)!=0.0) {self.rebuild();self.phase+=f(&self.template,20)*dt;}
        let alpha=if time<f(&self.template,14) {time/f(&self.template,14)}else if time>=self.fade_start {1.0-(time-self.fade_start)/f(&self.template,15)}else{1.0};
        let alpha=((alpha*255.0) as i32 as u8) as f32/255.0;
        let offset=self.origin-self.cached_origin;
        let mut vertices=self.cached.clone();for v in &mut vertices {v.pos=(Vec3::from_array(v.pos)+offset).to_array();v.color[3]*=alpha;}Ok(Some(vec![vertices]))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_71230_radial_ground_strip_lifecycle() {
        let t=Template {kind:3030,words:vec![57859,0,0,0,0,0,0,0,1092616192,58,20,2,1077936128,1077936128,1073741824,1073741824,4286628095,4286628095,1073741824,1048576000,1084227584,1036831949,1036831949,1056964608,1056964608,0,0]};
        let mut rng=R250::new(7);let mut grid=GroundGrid::new(&t,Mat4::IDENTITY,EffectConfig::default(),&mut rng).unwrap();
        let mut terrain=|p:Vec3|Some((Vec3::new(p.x,2.0,p.z),Vec3::Y));
        assert_eq!(grid.models()[0].2,760);assert_eq!(grid.models()[0].1.len(),2166);
        let vertices=grid.vertices(1.0,&mut terrain).unwrap().unwrap();assert_eq!(vertices[0].len(),760);
        assert!(vertices[0].iter().any(|v|v.color[3]>0.0));assert!(vertices[0].iter().all(|v|v.pos[1]>=2.25));
        grid.graceful();assert!(grid.vertices(1.0,&mut terrain).unwrap().is_none());
    }
    #[test]
    fn retail_groundgrid_authored_modes() {
        let dir=ao_gui::client_dir();
        if !dir.join("Setupf/gfxtweak.bin").exists() || !dir.join("cd_image/rdb.db").exists() {return;}
        let renderer=super::super::Renderer::open(&dir).unwrap();
        let mut rng=R250::new(7);
        for id in [71222,71230,71231,71232,71233,71234,71235,71236,71237,71303,71370] {
            let t=&renderer.templates.by_id[&id];
            let mut grid=GroundGrid::new(t,Mat4::IDENTITY,EffectConfig::default(),&mut rng).unwrap();
            let mut terrain=|p:Vec3|Some((Vec3::new(p.x,p.x*0.01,p.z),Vec3::Y));
            for time in [0.0,0.25,1.0,2.0] {
                let vertices=grid.vertices(time,&mut terrain).unwrap().unwrap();
                assert_eq!(vertices[0].len(),grid.models()[0].2);
                assert!(vertices[0].iter().all(|v|v.pos.iter().chain(v.uv.iter()).all(|x|x.is_finite())),"record {id}");
            }
            grid.graceful();assert!(grid.vertices(2.0,&mut terrain).unwrap().is_none());
        }
    }
    #[test]
    fn authored_groundgrid_clamp_material_state() {
        use super::super::{Binding,Creation,Renderer};
        let mut renderer=Renderer::open(&ao_gui::client_dir()).expect("authored clamp regression requires retail data");
        for id in [71222,71303,71370] {
            assert_ne!(renderer.templates.by_id[&id].words[0]&0x10000,0,"authored clamp flag {id}");
            renderer.spawn(Binding {group:0,attractor:0,effect:id,note:0,color:0},Creation::Vector,Mat4::IDENTITY,Vec3::ZERO).unwrap();
            let model=&renderer.models[&id].scene;
            assert!(model.meshes.iter().any(|m|!m.submeshes.is_empty()),"missing authored groundgrid material {id}");
            assert!(model.meshes.iter().flat_map(|m|&m.submeshes).all(|s|s.texture_clamp),"DS10016961 clamp state must reach actual resource materials {id}");
            renderer.clear();
        }
    }
    #[test]
    #[ignore = "retail assets and offscreen Metal rendering"]
    fn retail_groundgrid_frames() {
        use super::super::{Binding,Creation,Renderer,MODEL_BASE};
        let dir=ao_gui::client_dir();
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).expect("AOMAC_EFFECT_FRAMES output directory");
        std::fs::create_dir_all(&out).unwrap();
        let mut renderer=Renderer::open(&dir).unwrap();
        let mut rng=R250::new(7);
        let mut clamp_pixels=0;
        for id in [71222,71230,71231,71232,71233,71234,71235,71236,71237,71303,71370] {
            renderer.clear();
            renderer.spawn(Binding {group:0,attractor:0,effect:id,note:0,color:0},Creation::Vector,Mat4::IDENTITY,Vec3::ZERO).unwrap();
            let mut grid=GroundGrid::new(&renderer.templates.by_id[&id],Mat4::IDENTITY,EffectConfig::default(),&mut rng).unwrap();
            // Explicit test terrain isolates the native grid from playfield loading.
            let mut terrain=|p:Vec3|Some((Vec3::new(p.x,p.x*0.01,p.z),Vec3::Y));
            let models:Vec<_>=renderer.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
            let mut wrapped=models.clone();
            for (_,model) in &mut wrapped {for mesh in &mut model.meshes {for sub in &mut mesh.submeshes {sub.texture_clamp=false;}}}
            for time in [0.25,1.0,2.0] {
                let skin=grid.vertices(time,&mut terrain).unwrap().unwrap().remove(0);
                let actor=ao_scene::ActorFrame {id:1,model:MODEL_BASE|id as u32 as u64,skin:Some(skin),always:true,..Default::default()};
                let path=out.join(format!("groundgrid_{id}_{time:.2}.png"));
                ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,vec![actor.clone()],[30.0,40.0,30.0],[0.0,0.0,0.0],640,480,&path,time).unwrap();
                if [71222,71303,71370].contains(&id) {
                    let wrap_path=out.join(format!("groundgrid_wrap_reference_{id}_{time:.2}.png"));
                    ao_render::render_to_png_actors(&ao_scene::Scene::default(),&wrapped,vec![actor],[30.0,40.0,30.0],[0.0,0.0,0.0],640,480,&wrap_path,time).unwrap();
                    let clamp=image::open(path).unwrap().to_rgba8();let wrap=image::open(wrap_path).unwrap().to_rgba8();
                    let changed=clamp.pixels().zip(wrap.pixels()).filter(|(a,b)|a!=b).count();
                    clamp_pixels+=changed;
                    eprintln!("authored clamp {id} at{time}: {changed} clamp/wrap differing pixels");
                }
            }
        }
        assert!(clamp_pixels>0,"authored groundgrid clamp flags must change GPU sampling");
    }
}
