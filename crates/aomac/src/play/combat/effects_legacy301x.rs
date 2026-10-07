//! GroundRing GC100e1d53/1dc2/1e31, load100e1ba1, init100e19cc,
//! process100e1729; DS update10016e84. Delay GC100d851e/857d,
//! load100d8499/process100d830b/delete100d82c5. Bubble GC100d48d4/
//! 49fd/4bab, load100d46a0/init100d476c/process100d41f9/delete100d4cd7.
use super::{Binding, Creation, EffectConfig, Renderer, Template};
use anyhow::{ensure, Result};
use ao_formats::character::CrtRand;
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

fn strip(segments:u32)->Vec<u32> {(0..segments*2).flat_map(|i|if i&1==0 {[i,i+1,i+2]}else{[i+1,i,i+2]}).collect()}
fn step(started:&mut u8,elapsed:&mut f32,dt:f32)->Result<f32> {
    ensure!(dt.is_finite() && dt>=0.0,"invalid legacy effect timestep");
    // GC100d2531: initial Process has dt0; first advancing frame caps at .033.
    let dt=match *started {0=>0.0,1=>dt.min(f32::from_bits(0x3d072b02)),_=>dt};*started=(*started+1).min(2);*elapsed+=dt;Ok(dt)
}
fn packed(t:&Template,at:usize)->Result<u32> {
    // GC1010810e: ARGB float range, multiplication by double255, truncation.
    let mut color=0u32;for i in 0..4 {color=(color<<8)|((t.float(at+i)? as f64*255.0) as i32 as u32);}Ok(color)
}

pub(super) struct GroundRing {t:Template,source:Mat4,center:Vec3,positions:Vec<Vec3>,elapsed:f32,started:u8,stopped:bool,duration:f32}
impl GroundRing {
    pub(super) fn new(t:&Template,source:Mat4)->Result<Self> {
        ensure!(t.kind==3012,"not GroundRing");t.word(19)?;
        for i in [8,11,12,13,14,15,16,19] {t.float(i)?;}
        ensure!((1..=32766).contains(&t.word(10)?),"invalid GroundRing segments");
        let source=super::sprites::connector(t,source)?;
        Ok(Self {t:t.clone(),source,center:source.w_axis.truncate(),positions:Vec::with_capacity((t.word(10).unwrap_or(0) as usize+1)*2),elapsed:0.0,started:0,stopped:false,duration:t.float(8)?})
    }
    // Native explicit position/matrix setters reject updates; dynel locator refresh is separate.
    pub(super) fn update_source(&mut self,m:Mat4)->Result<()> {self.source=super::sprites::connector(&self.t,m)?;Ok(())}
    pub(super) fn invalidate_source(&mut self) {if self.t.word(0).unwrap_or(0)&0x1000==0 {self.stopped=true;}}
    pub(super) fn configure(&mut self,c:EffectConfig) {if let Some(d)=c.duration {self.duration=d;}}
    pub(super) fn terminate_gracefully(&mut self) {self.stopped=true;}
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {step(&mut self.started,&mut self.elapsed,dt)?;Ok(!self.stopped && (self.duration<=0.0 || self.elapsed<=self.duration))}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        // GC10106f39 returns null for material IDs >127, including authored100000.
        vec![((self.t.word(9).unwrap_or(0)<=127).then_some(self.t.word(9).unwrap_or(0) as usize),strip(self.t.word(10).unwrap_or(0)),(self.t.word(10).unwrap_or(0) as usize+1)*2)]
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.t.word(0).unwrap_or(0)&0x400!=0 {Blend::AlphaBlend}else{Blend::Additive}]}
    pub(super) fn vertices(&mut self,ground:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.stopped {return Ok(None)}
        let t=&self.t;
        if self.positions.is_empty() || t.word(0).unwrap_or(0)&0x800==0 {
            if t.word(0).unwrap_or(0)&0x800==0 {self.center=self.source.w_axis.truncate();}self.positions.clear();
            for i in 0..=t.word(10).unwrap_or(0) {let a=std::f32::consts::TAU*i as f32/t.word(10).unwrap_or(0) as f32;
                for end in 0..2 {let mut p=self.center+Vec3::new(a.cos()*t.float(13+end)?,0.0,-a.sin()*t.float(13+end)?);
                    p.y=ground(p).map_or(0.0,|(p,_)|p.y)+t.float(19)?;self.positions.push(p);}
            }
        }
        let fade=if self.elapsed<t.float(15)? {self.elapsed/t.float(15)?}else if self.elapsed< t.float(8)?-t.float(16)? {1.0}else{1.0-(self.elapsed-(t.float(8)?-t.float(16)?))/t.float(16)?};
        let mut v=Vec::with_capacity(self.positions.len());
        for (i,&p) in self.positions.iter().enumerate() {
            let end=i&1;let mut c=super::buff300x::packed_color(t.word(17+end).unwrap_or(0));
            c[3]=((t.word(17+end).unwrap_or(0)>>24) as f32*fade) as u8 as f32/255.0;
            let uv=if t.word(0).unwrap_or(0)&0x100!=0 {[(i/2) as f32/t.word(10).unwrap_or(0) as f32*t.float(11)?,end as f32*t.float(12)?]}else{[p.x*t.float(11)?,-p.z*t.float(12)?]};
            v.push(Vertex {pos:p.to_array(),normal:[0.0,1.0,0.0],uv,color:c});
        }
        Ok(Some(vec![v]))
    }
}

struct BubbleParticle {position:Vec3,velocity:Vec3,born:f32,size:f32,frame:u32,active:bool}
pub(super) struct Bubble {t:Template,source:Mat4,particles:[BubbleParticle;25],elapsed:f32,started:u8,stopped:bool,duration:f32,last_emit:f32,last_burst:f32,burst:i32,color:u32}
impl Bubble {
    pub(super) fn new(t:&Template,source:Mat4)->Result<Self> {
        ensure!(t.kind==3016,"not Bubble");t.word(24)?;
        for i in [8,11,12,13,14,15,16,17,18,19,20,21,22,23] {t.float(i)?;}
        ensure!(t.float(22)?>0.0 && t.float(23)?>0.0,"invalid Bubble timing");
        Ok(Self {t:t.clone(),source:super::sprites::connector(t,source)?,particles:std::array::from_fn(|_|BubbleParticle {position:Vec3::ZERO,velocity:Vec3::Y,born:0.0,size:0.0,frame:0,active:false}),elapsed:0.0,started:0,stopped:false,duration:t.float(21)?,last_emit:0.0,last_burst:0.0,burst:0,color:packed(t,13)?})
    }
    pub(super) fn update_source(&mut self,m:Mat4)->Result<()> {self.source=super::sprites::connector(&self.t,m)?;Ok(())}
    pub(super) fn invalidate_source(&mut self) {self.stopped=true;}
    pub(super) fn configure(&mut self,c:EffectConfig) {
        if let Some(d)=c.duration {self.duration=d;}
        for (at,color) in [(13,c.start_color),(17,c.stop_color)] {if let Some([r,g,b,a])=color {for (i,v) in [a,r,g,b].into_iter().enumerate() {{ self.t.words.resize(self.t.words.len().max((at+i) + 1), 0); *self.t.words.get_mut(at+i).unwrap() = v.to_bits(); };}}}
        // Native setters change the range only; existing sprites keep their creation colour.
    }
    pub(super) fn terminate_gracefully(&mut self) {self.stopped=true;}
    pub(super) fn frame(&mut self,dt:f32,crt:&mut CrtRand)->Result<bool> {
        let dt=step(&mut self.started,&mut self.elapsed,dt)?;
        if self.duration>0.0 && self.elapsed>self.duration {self.stopped=true;}
        if self.duration>0.0 && self.elapsed>self.duration+5.0 {return Ok(false)}
        let mode=self.t.words.get(25).copied().unwrap_or(0);
        let mut count=u32::from(self.last_emit+self.t.float(22)?<self.elapsed);
        if self.burst>0 {count+=3;self.burst-=3;}
        if self.last_burst+(if mode==1 {2.0}else{5.0})*self.t.float(22)?<self.elapsed {self.last_burst=self.elapsed;self.burst=self.t.word(24).unwrap_or(0) as i32;}
        if self.stopped {count=0;}
        for p in &mut self.particles {
            if !p.active {if count>0 {p.position=Vec3::ZERO;p.velocity=Vec3::Y;p.born=self.elapsed;p.size=crt.rand() as f32*self.t.float(12)?/32768.0;p.frame=5;p.active=true;count-=1;self.last_emit=self.elapsed;}}
            else {let phase=(self.elapsed-p.born)/self.t.float(23)?;
                if phase>=1.0 {p.active=false;continue;}
                p.velocity+=Vec3::new(crt.rand() as f32/32768.0-0.5,crt.rand() as f32/32768.0-0.5,-(crt.rand() as f32/32768.0-0.5))*dt;
                p.position+=p.velocity*dt;let frame=((phase as f64*16.0) as u32)&7;p.frame=if frame>3 {7-frame}else{frame};
            }
        }
        Ok(true)
    }
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {vec![(Some(self.t.word(9).unwrap_or(0) as usize),(0..25).flat_map(|i|[i*4,i*4+1,i*4+2,i*4+1,i*4+2,i*4+3]).collect(),100)]}
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::AlphaBlend]}
    pub(super) fn vertices(&self,right:Vec3,up:Vec3)->Result<Option<Vec<Vec<Vertex>>>> {
        let &(..,columns,rows,_,_)=super::materials::MATERIALS.get(self.t.word(9).unwrap_or(0) as usize).ok_or_else(||anyhow::anyhow!("unknown Bubble material"))?;
        let mut out=Vec::with_capacity(100);let mode=self.t.words.get(25).copied().unwrap_or(0);
        for p in &self.particles {
            if !p.active {continue;}
            let phase=(self.elapsed-p.born)/self.t.float(23)?;
            let f=2.0*phase-if mode==1 {0.75}else{1.0};let size=if p.frame==5 {0.01}else{(1.0-f*f)*p.size};
            let center=self.source.transform_point3(p.position);let x=right*size*0.5;let y=up*size*0.5;
            let u=(p.frame%columns) as f32/columns as f32;let v=(p.frame/columns) as f32/rows as f32;
            super::quad(&mut out,[center-x-y,center+x-y,center-x+y,center+x+y],[[u,v+1.0/rows as f32],[u+1.0/columns as f32,v+1.0/rows as f32],[u,v],[u+1.0/columns as f32,v]],super::buff300x::packed_color(self.color));
        }
        Ok(Some(vec![out]))
    }
}

pub(super) struct Delay {remaining:f32,effect:i32,position:Vec3,child:u32,elapsed:f32,started:u8,duration:f32}
impl Delay {
    pub(super) fn new(t:&Template,source:Mat4,c:EffectConfig,crt:&mut CrtRand)->Result<Self> {
        ensure!(t.kind==3013,"not Delay");let low=t.float(0)?;let high=t.float(1)?;
        Ok(Self {remaining:low+crt.rand() as f32/32768.0*(high-low),effect:t.word(2)? as i32,position:source.w_axis.truncate(),child:0,elapsed:0.0,started:0,duration:c.duration.map_or(90.0,|d|d+15.0)})
    }
    pub(super) fn initialize(&mut self,r:&Renderer,c:EffectConfig)->Result<()> {
        if c.creation!=Creation::Dynel {return Ok(())}
        let Some(t)=r.templates.by_id.get(&self.effect) else {self.remaining=0.0;return Ok(())};
        // GC100d857d ignores InitDynelTemplate failure; the fresh locator's
        // local position stays zero and the delayed child still uses Vector.
        if c.source_nonvisual {self.position=Vec3::ZERO;return Ok(())}
        let identity=c.source_identity.ok_or_else(||anyhow::anyhow!("Delay dynel identity missing"))?;
        let attractor=c.source_attractor.unwrap_or(t.word(7)? as i32);
        let source=r.anchors.get(&(identity,attractor)).copied().flatten()
            .or_else(||r.anchors.get(&(identity,0)).copied().flatten())
            .ok_or_else(||anyhow::anyhow!("Delay child source anchor missing"))?;
        self.position=super::sprites::connector(t,source)?.w_axis.truncate();
        Ok(())
    }
    pub(super) fn child(&self)->u32 {self.child}
    // Native UpdatePosition/Matrix forward only after child creation, without changing stored position.
    pub(super) fn frame(&mut self,dt:f32,r:&mut Renderer)->Result<bool> {
        let dt=step(&mut self.started,&mut self.elapsed,dt)?;
        if self.duration>0.0 && self.elapsed>self.duration {return Ok(false)}
        if self.remaining<=0.0 {return Ok(self.child!=0 && r.is_active(self.child))}
        self.remaining-=dt;
        if self.remaining<0.0 && self.effect!=0 {self.child=r.spawn_configured(Binding {group:0,attractor:0,effect:self.effect,note:0,color:0},Mat4::from_translation(self.position),self.position,EffectConfig {creation:Creation::Vector,..Default::default()})?;}
        Ok(true)
    }
    pub(super) fn terminate_gracefully(&mut self,r:&mut Renderer) {if self.child!=0 {r.terminate_gracefully(self.child);}}
    pub(super) fn cancel(&mut self,r:&mut Renderer) {if self.child!=0 {r.delete(self.child);self.child=0;}}
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_initial_process_timestep_cap()->Result<()> {
        let mut count=0;let mut elapsed=0.0;
        assert_eq!(step(&mut count,&mut elapsed,1.0)?,0.0);
        assert_eq!(step(&mut count,&mut elapsed,1.0)?,f32::from_bits(0x3d072b02));
        assert_eq!(step(&mut count,&mut elapsed,1.0)?,1.0);Ok(())
    }
    fn ring()->Template {Template {kind:3012,words:vec![771,0,0,0,0,0,0,0,1092616192,34,16,1097859072,3212836864,0,1090519040,1056964608,1065353216,4294967295,255,1048576000]}}
    fn bubble()->Template {Template {kind:3016,words:vec![5,0,0,0,0,0,0,2002,3212836864,51,0,1053609165,1028443341,1065353216,1050253722,1053609165,1065353216,1065353216,1050253722,1053609165,1065353216,3212836864,1063675494,1080872141,20]}}
    #[test]
    fn authored_60005_groundring_terrain_uv_envelope()->Result<()> {
        let mut s=GroundRing::new(&ring(),Mat4::from_translation(Vec3::new(10.0,3.0,20.0)))?;
        s.frame(0.0)?;s.frame(0.0)?;s.frame(0.25)?;
        let mut ground=|p:Vec3|Some((Vec3::new(p.x,7.0,p.z),Vec3::Y));
        let v=s.vertices(&mut ground)?.unwrap();
        assert_eq!(v[0].len(),34);assert_eq!(v[0][0].pos[1],7.25);
        assert_eq!(v[0][0].uv,[0.0,0.0]);assert_eq!(v[0][1].uv,[0.0,-1.0]);
        // DS10016e84 advances U by authored repeat15 / segments16.
        assert_eq!(v[0][2].uv,[15.0/16.0,0.0]);assert_eq!(v[0][32].uv,[15.0,0.0]);
        assert_eq!(v[0][0].color[3],127.0/255.0);
        s.terminate_gracefully();assert!(!s.frame(0.0)?);Ok(())
    }
    #[test]
    fn authored_61101_groundring_null_material_frozen_terrain()->Result<()> {
        let t=Template {kind:3012,words:vec![2819,0,0,0,0,0,0,0,1086744166,100000,16,1097859072,3212836864,0,1086324736,1058642330,1050253722,4294967295,255,1048576000]};
        let mut s=GroundRing::new(&t,Mat4::IDENTITY)?;
        assert_eq!(s.models()[0].0,None);s.frame(0.0)?;
        let mut ground=|p:Vec3|Some((Vec3::new(p.x,4.0,p.z),Vec3::Y));
        let first=s.vertices(&mut ground)?.unwrap();
        s.update_source(Mat4::from_translation(Vec3::X*10.0))?;
        let next=s.vertices(&mut ground)?.unwrap();assert_eq!(first[0][0].pos,next[0][0].pos);Ok(())
    }
    #[test]
    fn authored_12400_bubble_emission_atlas_and_drain()->Result<()> {
        let mut s=Bubble::new(&bubble(),Mat4::IDENTITY)?;let mut rng=CrtRand::new(1);
        s.frame(0.0,&mut rng)?;s.frame(0.0,&mut rng)?;s.frame(0.81,&mut rng)?;
        // GC100d41f9 uses strict last_emit + interval22 < elapsed;
        // authored12400 interval is 0.9 (0x3f666666), not 0.8.
        assert!(s.particles.iter().all(|p|!p.active));
        s.frame(s.t.float(22)?-s.elapsed,&mut rng)?;
        assert!(s.particles.iter().all(|p|!p.active));
        s.frame(0.01,&mut rng)?;
        assert_eq!(s.particles.iter().filter(|p|p.active).count(),1);
        assert_eq!(s.particles[0].frame,5);
        s.frame(0.5,&mut rng)?;
        let phase=(s.elapsed-s.particles[0].born)/s.t.float(23)?;
        let tick=(phase as f64*16.0) as u32&7;
        assert_eq!(s.particles[0].frame,if tick>3 {7-tick}else{tick});
        s.terminate_gracefully();s.frame(4.0,&mut rng)?;
        assert!(s.particles.iter().all(|p|!p.active));Ok(())
    }
    #[test]
    fn authored_100385_bubble_mode_one_and_optional_mode_zero()->Result<()> {
        let t=Template {kind:3016,words:vec![5,0,3192704205,1036831949,0,0,0,2002,3212836864,51,0,1036831949,1025758986,1065353216,1050253722,1065353216,1053609165,1065353216,1050253722,1061997773,1050253722,3212836864,1074580685,1080872141,30,1]};
        let mut s=Bubble::new(&t,Mat4::IDENTITY)?;let mut rng=CrtRand::new(1);
        s.frame(0.0,&mut rng)?;s.frame(0.0,&mut rng)?;s.frame(4.5,&mut rng)?;assert_eq!(s.burst,30);
        s.frame(0.1,&mut rng)?;assert_eq!(s.particles.iter().filter(|p|p.active).count(),4);
        assert_eq!(bubble().words.get(25),None);Ok(())
    }
    #[test]
    fn authored_62010_delay_random_interval_and_capture()->Result<()> {
        let t=Template {kind:3013,words:vec![0,1092616192,80013]};let mut rng=CrtRand::new(1);
        let source=Mat4::from_translation(Vec3::new(1.0,2.0,3.0));
        let d=Delay::new(&t,source,EffectConfig {creation:Creation::Vector,..Default::default()},&mut rng)?;
        assert!(d.remaining>=0.0 && d.remaining<10.0);assert_eq!(d.effect,80013);assert_eq!(d.position,source.w_axis.truncate());assert_eq!(d.duration,90.0);Ok(())
    }
    #[test]
    #[ignore="installed authored3012/3013/3016 assets and offscreen GPU"]
    fn retail_legacy301x_authored_frames()->Result<()> {
        use super::super::{MODEL_BASE,MESH_MODEL_BASE};
        use anyhow::Context;
        let out=std::path::PathBuf::from(std::env::var_os("AOMAC_EFFECT_FRAMES").context("AOMAC_EFFECT_FRAMES required")?);
        std::fs::create_dir_all(&out)?;
        let mut r=Renderer::open(&ao_gui::client_dir())?;
        let origin=Vec3::new(5000.0,10.0,5000.0);let source=Mat4::from_translation(origin);
        let eye=origin+Vec3::new(12.0,10.0,16.0);
        let mut host=ao_render::Host::headless();host.camera=ao_render::Camera::look_at(eye,origin);
        let ids:Vec<_>=r.templates.by_id.iter().filter_map(|(&id,t)|matches!(t.kind,3012|3013|3016).then_some(id)).collect();
        ensure!(ids.len()==16,"authored legacy301x census changed");
        for id in ids {
            r.clear();
            let handle=r.spawn_configured(Binding {group:0,attractor:0,effect:id,note:0,color:0},source,origin,EffectConfig {creation:Creation::Vector,..Default::default()})?;
            ensure!(handle!=0,"authored legacy301x did not spawn: {id}");
            for frame in 1..=660 {
                host.actors.clear();
                let mut ground=|p:Vec3|Some((Vec3::new(p.x,10.0,p.z),Vec3::Y));
                r.frame(1.0/60.0,&mut host,Some(&mut ground));
                if [1,15,30,60,120,240,600,660].contains(&frame) {
                    let mut models:Vec<_>=r.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                    models.extend(r.buff_models.iter().map(|(&id,m)|(0xfac2_0000_0000_0000|u64::from(id),m.scene.clone())));
                    models.extend(r.mesh_resources.iter().map(|(&id,m)|(MESH_MODEL_BASE|u64::from(id),(**m).clone())));
                    ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,host.actors.clone(),eye.to_array(),origin.to_array(),640,480,&out.join(format!("legacy301x_{id}_{frame}.png")),frame as f32/60.0)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[test]
fn short_ring_defaults_missing_colors_to_zero() {
    let mut words=vec![0;11];words[10]=3;
    let mut effect=GroundRing::new(&Template {kind:3012,words},Mat4::IDENTITY).unwrap();
    let vertices=effect.vertices(&mut |p|Some((p,Vec3::Y))).unwrap().unwrap();
    assert!(vertices[0].iter().all(|v|v.color==[0.0;4]));
    assert_eq!(effect.t.float(19).unwrap(),0.0);
    assert_eq!(effect.models()[0].0,Some(0));
}
