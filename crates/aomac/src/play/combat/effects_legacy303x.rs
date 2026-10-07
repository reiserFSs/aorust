//! GC Spiral2 1011194c/10111a8a/101116ef, AParticle 1010829c/10108a48/101084f5,
//! Toggle 10112109/10112401. DS Spiral2 1002270b/10022407 (x87 assembly).
use super::{buff303x::Curve, materials, quad, sprites, Binding, Creation, EffectConfig, Renderer, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::weather::R250;
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

const SPIRAL_TAU:f32=f32::from_bits(0x40c8f5c3); // GC1016b578: 6.28000020980835.

fn color(mut c:[f32;4])->[f32;4] { for v in &mut c[..3] {*v=v.clamp(0.0,1.0).powf(2.2);}c }
fn sample(t:&Template,a:usize,b:usize,r:&mut R250)->Result<f32> {let u=super::random_fraction(r);Ok(t.float(a)?+(t.float(b)?-t.float(a)?)*u)}
fn cube(radius:f32,r:&mut R250)->Vec3 {Vec3::new(super::random_fraction(r)*2.0-1.0,super::random_fraction(r)*2.0-1.0,-(super::random_fraction(r)*2.0-1.0))*radius}

pub(super) struct Spiral2 {
    template:Template, source:Mat4, duration:f32, elapsed:f32, angle:f32, speed:f32,
    initial:f32, scale:f32, scale_ready:bool, curves:[Curve;2], finish:Option<(f32,f32,[f32;4])>, removed:bool,
}
impl Spiral2 {
    pub(super) fn new(t:&Template,source:Mat4,c:EffectConfig,gc:&mut R250)->Result<Self> {
        ensure!(t.kind==3033,"not Spiral2");t.word(21)?;
        for i in (1..=6).chain([8]).chain(12..=21) {t.float(i)?;}
        ensure!(t.word(10)?>0 && t.word(11)?>1 && u64::from(t.word(10)?)*u64::from(t.word(11)?)*2<=u16::MAX as u64,"invalid Spiral2 capacity");
        materials::MATERIALS.get(t.word(9)? as usize).context("unknown Spiral2 material")?;
        let mut at=22;let curves=[Curve::parse(t,&mut at,true)?,Curve::parse(t,&mut at,false)?];
        let initial=if t.float(15)?==999.0 {super::random_fraction(gc)*SPIRAL_TAU}else{t.float(15)?};
        Ok(Self {template:t.clone(),source:sprites::connector(t,source)?,duration:c.duration.unwrap_or(t.float(8)?),elapsed:0.0,angle:0.0,speed:t.float(18)?,initial,scale:1.0,scale_ready:c.creation!=Creation::Dynel,curves,finish:None,removed:false})
    }
    pub(super) fn set_torso_factor(&mut self,scale:f32) {if !self.scale_ready {self.scale=scale;self.scale_ready=true;}}
    pub(super) fn source_removed(&mut self) {self.removed=true;}
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {self.source=sprites::connector(&self.template,source)?;Ok(())}
    pub(super) fn graceful(&mut self) {
        if self.elapsed<self.duration-1.0 {self.duration=self.elapsed+1.0;self.finish=Some((self.elapsed/self.duration,self.curves[1].scalar(0.0),self.curves[0].color(0.0)));}
    }
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let n=self.template.word(11).unwrap_or(0)*2;
        let indices=(0..n-2).flat_map(|i|if i&1==0 {[i,i+2,i+1]}else{[i,i+1,i+2]}).collect::<Vec<_>>();
        (0..self.template.word(10).unwrap_or(0)).map(|_|(Some(self.template.word(9).unwrap_or(0) as usize),indices.clone(),n as usize)).collect()
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.template.word(0).unwrap_or(0)&0x200!=0 {Blend::Additive}else{Blend::AlphaBlend};self.template.word(10).unwrap_or(0) as usize]}
    pub(super) fn vertices(&mut self,time:f32,_camera:Vec3,_right:Vec3,_up:Vec3,_gc:&mut R250)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.removed || (self.duration>=0.0 && time>self.duration) {return Ok(None);}
        if !self.scale_ready {return Ok(Some(Vec::new()));}
        let dt=(time-self.elapsed).max(0.0);self.elapsed=time;self.angle+=self.speed*dt;self.speed+=self.template.float(19)?*dt;
        let phase=time/self.duration;let (radius,c)=if let Some((start,r,c))=self.finish {let u=((phase-start)/(1.0-start)).clamp(0.0,1.0);let mut c=c;for v in &mut c {*v*=1.0-u;}(r+u,c)}else{(self.curves[1].scalar(phase),self.curves[0].color(phase))};
        let radius=radius*self.scale;let t=&self.template;let count=t.word(11).unwrap_or(0);
        let rotation=Mat4::from_rotation_y(-self.angle);let mut out=Vec::with_capacity(t.word(10).unwrap_or(0) as usize);
        for layer in 0..t.word(10).unwrap_or(0) {
            let mut vertices=Vec::with_capacity(count as usize*2);
            for i in 0..count {
                let u=i as f32/count as f32;let theta=self.initial+layer as f32*SPIRAL_TAU/t.word(10).unwrap_or(0) as f32+i as f32*t.float(16)?*self.scale/count as f32;
                let mut fade=1.0;
                if t.float(21)?>0.0 && u<t.float(21)? {fade=u/t.float(21)?;}
                if t.float(20)?>0.0 && u>1.0-t.float(20)? {fade=(1.0-u)/t.float(20)?;}
                if t.word(0).unwrap_or(0)&0x400!=0 {fade*=(1.0+(theta*4.0+phase*t.float(12)?).sin())*0.5;}
                let mut c=c;c[3]=(c[3]*fade*255.0) as u8 as f32/255.0;
                for (sign,v) in [(1.0,0.0),(-1.0,1.0)] {
                    let p=Vec3::new(theta.sin()*radius,u*t.float(14)?*self.scale+sign*t.float(13)?, -theta.cos()*radius);
                    vertices.push(Vertex {pos:(self.source.w_axis.truncate()+rotation.transform_vector3(p)).to_array(),normal:Vec3::Y.to_array(),uv:[u*t.float(17)?,v],color:color(c)});
                }
            }out.push(vertices);
        }Ok(Some(out))
    }
}

struct AmbientParticle {position:Vec3,angle:f32,spin:f32,size:f32,frame:f32,phase:f32,frequency:f32,amplitude:f32}
pub(super) struct AParticle {template:Template,source:Mat4,duration:f32,elapsed:f32,particles:Vec<AmbientParticle>,curve:Curve,center:Option<Vec3>,wind:Option<Vec3>,frame_rate:f32,removed:bool}
impl AParticle {
    pub(super) fn new(t:&Template,source:Mat4,c:EffectConfig,gc:&mut R250)->Result<Self> {
        ensure!(t.kind==3035,"not AParticle");t.word(25)?;for i in (1..=6).chain([8]).chain(12..=23).chain([25]) {t.float(i)?;}
        ensure!(t.word(9)?<=2 && t.word(24)?<=3 && t.word(11)?<=u16::MAX as u32/4 && t.float(25)?>0.0,"invalid AParticle parameters");
        let material=materials::MATERIALS.get(t.word(10)? as usize).context("unknown AParticle material")?;
        let mut at=26;let curve=Curve::parse(t,&mut at,true)?;let mut particles=Vec::with_capacity(t.word(11).unwrap_or(0) as usize);
        for _ in 0..t.word(11).unwrap_or(0) {
            let size=sample(t,17,18,gc)?;let frame=if t.word(0).unwrap_or(0)&0x20000!=0 {material.3 as f32+(material.4-material.3) as f32*super::random_fraction(gc)}else{0.0};
            let phase=super::random_fraction(gc)*std::f32::consts::TAU;let frequency=sample(t,19,20,gc)?;let amplitude=sample(t,21,22,gc)?;
            let position=cube(t.float(14)?,gc);let angle=if t.word(0).unwrap_or(0)&0x4000!=0 {0.0}else{super::random_fraction(gc)*std::f32::consts::TAU};let spin=sample(t,15,16,gc)?.to_radians();
            particles.push(AmbientParticle {position,angle,spin,size,frame,phase,frequency,amplitude});
        }
        Ok(Self {template:t.clone(),source:sprites::connector(t,source)?,duration:c.duration.unwrap_or(t.float(8)?),elapsed:0.0,particles,curve,center:None,wind:None,frame_rate:t.float(23)?,removed:false})
    }
    pub(super) fn set_environment(&mut self,center:Vec3,wind:Vec3) {if self.center.is_none() {for p in &mut self.particles {p.position+=center;}}self.center=Some(center);self.wind=Some(wind);}
    pub(super) fn source_removed(&mut self) {self.removed=true;}
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {self.source=sprites::connector(&self.template,source)?;Ok(())}
    pub(super) fn graceful(&mut self) {} // base 100a719a is a no-op.
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {vec![(Some(self.template.word(10).unwrap_or(0) as usize),(0..self.particles.len() as u32).flat_map(|i|{let b=i*4;[b,b+1,b+2,b+2,b+1,b+3]}).collect(),self.particles.len()*4)]}
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.template.word(0).unwrap_or(0)&0x200!=0 {Blend::Additive}else{Blend::AlphaBlend}]}
    pub(super) fn vertices(&mut self,time:f32,_camera:Vec3,right:Vec3,up:Vec3,gc:&mut R250)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.removed || (self.duration>=0.0 && time>self.duration) {return Ok(None);}
        let center=self.center.context("AParticle needs actual native camera position")?;let wind=self.wind.context("AParticle needs actual environment oscillator channels")?;
        let dt=(time-self.elapsed).max(0.0);self.elapsed=time;let t=&self.template;let m=&materials::MATERIALS[t.word(10).unwrap_or(0) as usize];let mut vertices=Vec::with_capacity(self.particles.len()*4);
        for p in &mut self.particles {
            let bound=p.amplitude+t.float(14)?+5.0;let delta=p.position-center;
            if delta.abs().max_element()>bound+10.0 {p.position=center+cube(t.float(14)?,gc);}else{for axis in 0..3 {
                if delta[axis]< -bound {p.position[axis]=center[axis]+bound;}
                if delta[axis]>bound {p.position[axis]=center[axis]-bound;}
            }}
            let mut pos=p.position;
            if (1..=2).contains(&t.word(9).unwrap_or(0)) {pos+=Vec3::new(p.phase.sin()+1.2*wind.x,p.phase.sin()*0.4,-(p.phase.cos()*0.7+2.0*wind.z*wind.x))*p.amplitude;if t.word(9).unwrap_or(0)==2 {super::random_fraction(gc);}}
            p.phase=(p.phase+p.frequency*dt)%std::f32::consts::TAU;
            let near=t.float(12)?.powi(2);let far=t.float(14)?.powi(2);let phase=((pos-center).length_squared()-near)/(far-near);let c=self.curve.color(phase.clamp(0.0,1.0));
            p.angle+=p.spin*dt;let x=(right*(-p.angle).cos()+up*(-p.angle).sin())*p.size;let y=(-right*(-p.angle).sin()+up*(-p.angle).cos())*(p.size/t.float(25)?);
            if t.word(24).unwrap_or(0)!=0 {p.frame+=dt*self.frame_rate;
                if p.frame>m.4 as f32 {match t.word(24).unwrap_or(0) {1=>p.frame=m.3 as f32,2=>p.frame=m.4 as f32,3=>{self.frame_rate= -self.frame_rate;p.frame+=dt*self.frame_rate;},_=>{}}}
                if p.frame<m.3 as f32 && t.word(24).unwrap_or(0)==3 {self.frame_rate= -self.frame_rate;p.frame+=dt*self.frame_rate;}
            }
            let frame=p.frame as i32;let u=frame.rem_euclid(m.1 as i32) as f32/m.1 as f32;let v=(frame/m.1 as i32) as f32/m.2 as f32;let du=1.0/m.1 as f32;let dv=1.0/m.2 as f32;
            let uv=if t.word(0).unwrap_or(0)&0x100!=0 {[[u,v],[u,v+dv],[u+du,v],[u+du,v+dv]]}else{[[u,v],[u+du,v],[u,v+dv],[u+du,v+dv]]};
            quad(&mut vertices,[pos+x+y,pos+y-x,pos+x-y,pos-x-y],uv,color(c));
        }Ok(Some(vec![vertices]))
    }
}

pub(super) struct Toggle {template:Template,source:Mat4,target:Vec3,config:EffectConfig,elapsed:f32,child:u32,terminating:bool,moving:bool,motion:Option<(f32,i32)>,playfield:Option<(u32,u32)>,removed:bool}
impl Toggle {
    pub(super) fn new(t:&Template,source:Mat4,target:Vec3,c:EffectConfig)->Result<Self> {ensure!(t.kind==3036,"not Toggle");t.float(1)?;Ok(Self {template:t.clone(),source,target,config:c,elapsed:0.0,child:0,terminating:false,moving:false,motion:None,playfield:None,removed:false})}
    pub(super) fn update_source(&mut self,source:Mat4) {self.source=source;}
    pub(super) fn set_playfield(&mut self,id:u32,flags:u32) {self.playfield=Some((id,flags));}
    pub(super) fn set_motion(&mut self,speed:f32,direction:i32) {self.motion=Some((speed,direction));}
    pub(super) fn source_removed(&mut self) {self.removed=true;}
    fn permitted(&self)->Result<bool> {let (id,flags)=self.playfield.context("Toggle needs actual playfield resource")?;let t=&self.template;if t.word(3)?!=0 && flags&t.word(3)?==0 {return Ok(false);}let count=t.word(4)? as usize;let list=t.words.get(5..).unwrap_or(&[]);let listed=list.iter().take(count).any(|&word|word==id)||(id==0&&count>list.len());Ok(if t.word(0)?&0x800!=0 {!listed}else{listed})}
    fn update_motion(&mut self)->Result<bool> {let old=self.moving;if self.template.word(0).unwrap_or(0)&0x6000!=0 {let (speed,direction)=self.motion.context("Toggle needs actual vehicle motion")?;if self.template.word(0).unwrap_or(0)&0x2000!=0 && direction>0 {self.moving=speed>0.0;}}Ok(old)}
    pub(super) fn graceful(&mut self,renderer:&mut Renderer)->Result<()> {self.template.words.resize(self.template.words.len().max(1),0);*self.template.words.get_mut(0).unwrap() &= !0x400;if self.child!=0 {renderer.terminate_gracefully(self.child);}self.terminating=true;Ok(())}
    pub(super) fn cancel(&mut self,renderer:&mut Renderer) {if self.child!=0 {renderer.delete(self.child);self.child=0;}}
    pub(super) fn frame(&mut self,dt:f32,renderer:&mut Renderer)->Result<bool> {
        self.elapsed+=dt;if self.removed || (self.template.float(1)?>=0.0 && self.elapsed>self.template.float(1)?) {self.cancel(renderer);return Ok(false);}
        if self.child!=0 && !renderer.is_active(self.child) {self.child=0;if self.template.word(0).unwrap_or(0)&0x400==0 || self.terminating {return Ok(false);}}
        if self.child==0 && !self.terminating {
            if !self.permitted()? {return Ok(true);}
            if self.config.creation==Creation::Dynel {let old=self.update_motion()?;if self.template.word(0).unwrap_or(0)&0x2000!=0 && (!self.moving || old) {return Ok(true);}}
            let mut c=self.config;c.duration=None;c.start_color=None;c.stop_color=None;c.repetitions=None;
            if self.template.word(0).unwrap_or(0)&0x1000!=0 {c.creation=Creation::Unlocated;c.source_identity=None;c.track_source=false;}
            self.child=renderer.spawn_configured(Binding {group:0,attractor:0,effect:self.template.word(2).unwrap_or(0) as i32,note:0,color:0},self.source,self.target,c)?;
            return Ok(true);
        }
        if self.child!=0 && self.template.word(0).unwrap_or(0)&0x8000!=0 {let old=self.update_motion()?;if !self.moving && old {renderer.terminate_gracefully(self.child);}}
        Ok(self.child!=0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_legacy303x_records_and_lifecycle()->Result<()> {
        let table=super::super::Templates::open(&ao_gui::client_dir())?;
        let mut gc=R250::new(0xe6f1);
        let counts=[(3033,8),(3035,3),(3036,5)];
        for (kind,count) in counts {assert_eq!(table.by_id.values().filter(|t|t.kind==kind).count(),count);}
        for t in table.by_id.values().filter(|t|t.kind==3033) {
            let mut n=Spiral2::new(t,Mat4::IDENTITY,EffectConfig::default(),&mut gc)?;
            let models=n.models();assert_eq!(models.len(),t.words[10] as usize);
            let v=n.vertices(0.25,Vec3::Z,Vec3::X,Vec3::Y,&mut gc)?.unwrap();
            assert_eq!(v.len(),models.len());assert!(v.iter().flatten().all(|v|v.pos.iter().all(|p|p.is_finite())));
            assert_eq!(v[0].len(),t.words[11] as usize*2);n.graceful();
            assert!(n.vertices(1.26,Vec3::Z,Vec3::X,Vec3::Y,&mut gc)?.is_none());
        }
        let mut n=Spiral2::new(&table.by_id[&71066],Mat4::IDENTITY,EffectConfig {creation:Creation::Dynel,..Default::default()},&mut gc)?;
        assert!(n.vertices(0.0,Vec3::Z,Vec3::X,Vec3::Y,&mut gc)?.unwrap().is_empty());
        n.set_torso_factor(2.0);n.set_torso_factor(3.0);assert_eq!(n.scale,2.0);
        assert!(!n.vertices(0.1,Vec3::Z,Vec3::X,Vec3::Y,&mut gc)?.unwrap().is_empty());
        for t in table.by_id.values().filter(|t|t.kind==3035) {
            let mut n=AParticle::new(t,Mat4::IDENTITY,EffectConfig::default(),&mut gc)?;
            n.set_environment(Vec3::ZERO,Vec3::ZERO);
            let v=n.vertices(0.1,Vec3::Z,Vec3::X,Vec3::Y,&mut gc)?.unwrap();
            assert_eq!(v[0].len(),t.words[11] as usize*4);
            assert!(n.particles.iter().all(|p|p.size>=t.float(17).unwrap() && p.size<=t.float(18).unwrap()));
            assert!(v[0].iter().all(|v|v.pos.iter().all(|p|p.is_finite())));
            n.graceful();assert!(n.vertices(100.0,Vec3::Z,Vec3::X,Vec3::Y,&mut gc)?.is_some());
            n.source_removed();assert!(n.vertices(100.1,Vec3::Z,Vec3::X,Vec3::Y,&mut gc)?.is_none());
        }
        for id in [3421,72106,72412,72416,73100] {
            let mut n=Toggle::new(&table.by_id[&id],Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default())?;
            n.set_playfield(153,0);
            assert_eq!(n.permitted()?,matches!(id,72412|72416));
        }
        let mut n=Toggle::new(&table.by_id[&72416],Mat4::IDENTITY,Vec3::ZERO,EffectConfig {creation:Creation::Dynel,..Default::default()})?;
        n.set_motion(1.0,1);assert!(!n.update_motion()?);assert!(n.moving);
        n.set_motion(0.0,1);assert!(n.update_motion()?);assert!(!n.moving);
        Ok(())
    }
    #[test]
    #[ignore = "installed retail assets and offscreen Metal rendering"]
    fn retail_legacy303x_frames()->Result<()> {
        use ao_scene::{ActorFrame,Mesh,Scene,Submesh,TextureKey};
        let renderer=Renderer::open(&ao_gui::client_dir())?;
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).unwrap_or_else(||"/tmp/FxClasses/frames".into());
        std::fs::create_dir_all(&out)?;
        let origin=Vec3::new(5000.0,10.0,5000.0);let eye=origin+Vec3::new(8.0,6.0,12.0);
        for (&id,t) in renderer.templates.by_id.iter().filter(|(_,t)|matches!(t.kind,3033|3035)) {
            let mut gc=R250::new(0xe6f1);
            let mut spiral=if t.kind==3033 {Some(Spiral2::new(t,Mat4::from_translation(origin),EffectConfig::default(),&mut gc)?)}else{None};
            let mut ambient=if t.kind==3035 {let mut n=AParticle::new(t,Mat4::from_translation(origin),EffectConfig::default(),&mut gc)?;n.set_environment(origin,Vec3::ZERO);Some(n)}else{None};
            let models=if let Some(n)=&spiral {n.models()}else{ambient.as_ref().unwrap().models()};
            let blends=if let Some(n)=&spiral {n.blends()}else{ambient.as_ref().unwrap().blends()};
            for frame in [1,15,30,60] {
                let time=frame as f32/60.0;
                let Some(vertices)=(if let Some(n)=&mut spiral {n.vertices(time,eye,Vec3::X,Vec3::Y,&mut gc)?}else{ambient.as_mut().unwrap().vertices(time,eye,Vec3::X,Vec3::Y,&mut gc)?}) else {continue};
                let mut scene=Scene::default();
                for (i,vertices) in vertices.into_iter().enumerate() {
                    let material=materials::MATERIALS[models[i].0.unwrap()];
                    let key=TextureKey {rdb_type:1010004,id:renderer.names.id(1010004,material.0).context("missing legacy texture name")?};
                    scene.textures.insert(key,ao_formats::texture::load_texture(&renderer.store,key)?.context("missing legacy texture")?);
                    let mut sub=Submesh::new(models[i].1.clone(),Some(key));sub.blend=blends[i];sub.two_sided=true;sub.emissive=[1.0;3];
                    scene.meshes.push(Mesh {vertices,submeshes:vec![sub]});
                }
                scene.instances.extend((0..scene.meshes.len()).map(|mesh|ao_scene::Instance {mesh,transform:Mat4::IDENTITY.to_cols_array_2d()}));
                assert_eq!(scene.instances.len(),scene.meshes.len(),"every legacy303x fixture mesh must be drawable");
                ao_render::render_to_png_actors(&scene,&[],Vec::<ActorFrame>::new(),eye.to_array(),origin.to_array(),640,480,&out.join(format!("legacy303x_{id}_{frame}.png")),time)?;
            }
        }Ok(())
    }
}

#[cfg(test)]
#[test]
fn short_toggle_defaults_missing_fields_to_zero() {
    let mut effect=Toggle::new(&Template {kind:3036,words:Vec::new()},Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).unwrap();
    effect.set_playfield(1,0);
    assert!(!effect.permitted().unwrap());
    assert!(!effect.update_motion().unwrap());
    assert_eq!(effect.template.word(2).unwrap(),0);
}
