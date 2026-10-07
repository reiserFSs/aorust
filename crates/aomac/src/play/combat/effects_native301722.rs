//! GC GlobalSmoke3017: ctor100e0835, loader100e00ba, init100e01e3,
//! Process100df6e3, graceful100dff8d, delete100e0b52; DS Sol10020836.
//! GC Beam3022: ctor10109d70, loader101092b7, init10109199,
//! Process101096b7, graceful1010916d, delete1010953f; DS10008056/100087c1.
use super::{materials, quad, sprites, EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::{character::CrtRand, weather::R250};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Default)]
struct SmokeParticle { position:Vec3, velocity:Vec3, born:f32, active:bool }
pub(super) struct NativeEffect {
    template:Template, source:Mat4, anchor:Vec3, elapsed:f32, dt:f32,
    started:bool, duration:f32, repetitions:u32, fall:f32, random_rotation:f32,
    velocity:f32, particles:[SmokeParticle;64], emitted:i32,
    heading:f32, wander:Vec3, retarget:f32, trail:[Vec3;5], trail_write:usize,
    trail_read:usize, trail_time:f32, graceful:bool, start:Option<[f32;4]>, stop:Option<[f32;4]>,
    environment:Option<Vec3>, scroll:f32, reseed:bool, facets:u32, scale:f32, frames:u32, relative:Vec3, root:Option<Vec3>, track_root:bool, source_visible:bool,
}
fn indices(n:u32)->Vec<u32> {(0..n).flat_map(|i| {let a=i*4;[a,a+1,a+2,a+1,a+2,a+3]}).collect()}
fn packed(argb:u32)->[f32;4] {super::buff300x::packed_color(argb)}
fn color(a:u32,b:u32,f:f32)->[f32;4] {
    let a=a.to_be_bytes();let b=b.to_be_bytes();
    packed(u32::from_be_bytes(std::array::from_fn(|i|((a[i] as f32*(1.0-f)) as u32+(b[i] as f32*f) as u32) as u8)))
}
impl NativeEffect {
    pub(super) fn new(t:&Template,source:Mat4,c:EffectConfig,gc:&mut R250,ds:&mut R250)->Result<Self> {
        ensure!(matches!(t.kind,3017|3022),"not a GlobalSmoke/Beam template");
        t.word(if t.kind==3017 {31}else{41})?;
        for i in 1..=6 {t.float(i)?;}
        if t.kind==3017 {
            ensure!(t.word(10)?<=6,"invalid GlobalSmoke mode");
            for i in (11..=29).chain([8]) {t.float(i)?;}
            ensure!(t.float(26)?>0.0,"invalid GlobalSmoke particle lifetime");
            ensure!(t.word(10)?!=5 || t.word(31)?>0,"GlobalSmoke5 has zero rotation divisor");
        } else {
            for i in (11..=12).chain(15..=23).chain(30..=38).chain(40..=41).chain([8]) {t.float(i)?;}
            ensure!((1..=4096).contains(&t.word(14)?),"invalid Beam facet count");
        }
        ensure!((t.word(9)? as usize)<materials::MATERIALS.len(),"invalid native material");
        if t.kind==3022 && (t.word(39)? as i32)>=0 {ensure!((t.word(39)? as usize)<materials::MATERIALS.len(),"invalid Beam cap material");}
        let connector=sprites::connector(t,source)?;
        let root=if c.track_source {None}else{Some(Vec3::ZERO)};
        let relative=if t.word(0)?&2==0 {connector.w_axis.truncate()-root.unwrap_or(Vec3::ZERO)}else{Vec3::ZERO};
        let source=connector;
        let duration=c.duration.unwrap_or(if t.kind==3022 && t.float(8)?<0.0 {t.float(11)?+t.float(12)?}else{t.float(8)?});
        ensure!(t.kind!=3022 || duration>0.0,"invalid Beam duration");
        // Beam constructor consumes GC R250 even for fixed rotation; Sol does not.
        let random_rotation=if t.kind==3022 {super::random_fraction(gc)*2.0-1.0}else{0.0};
        let scroll=if t.kind==3022 && t.word(0)?&0x8000!=0 {super::random_fraction(ds)}else{0.0};
        let scale=c.scale.unwrap_or(1.0);
        ensure!(scale.is_finite() && scale>0.0,"invalid native scale");
        let facets=if t.kind==3022 {(t.word(14)? as f32*scale).trunc() as u32}else{64};
        ensure!((1..=4096).contains(&facets),"invalid scaled Beam facets");
        Ok(Self {template:t.clone(),source,anchor:source.w_axis.truncate(),elapsed:0.0,dt:0.0,started:false,duration,
            repetitions:c.repetitions.unwrap_or(if t.kind==3022 {t.word(13)?}else{0}),fall:if t.kind==3022 {t.float(12)?}else{0.0},
            random_rotation,velocity:0.0,particles:[SmokeParticle::default();64],emitted:0,heading:0.0,wander:Vec3::ZERO,
            retarget:0.0,trail:[Vec3::ZERO;5],trail_write:0,trail_read:0,trail_time:0.0,graceful:false,start:c.start_color,stop:c.stop_color,environment:None,scroll,reseed:false,facets,scale,frames:0,relative,root,track_root:c.track_source,source_visible:true})
    }
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {
        let connector=sprites::connector(&self.template,source)?;
        self.source=connector;Ok(())
    }
    pub(super) fn needs_source_root(&self)->bool {self.template.kind==3017 && self.template.word(0).unwrap_or(0)&2==0 && self.track_root}
    pub(super) fn set_source_root(&mut self,source:Mat4) {self.root=Some(source.w_axis.truncate());}
    pub(super) fn set_environment_position(&mut self,position:Vec3) {self.environment=Some(position);}
    pub(super) fn set_source_visible(&mut self,visible:bool) {self.source_visible=visible;}
    pub(super) fn graceful(&mut self) {
        if self.template.kind==3017 {self.graceful=true;}
        else if self.elapsed<self.duration-3.0 {self.duration=self.elapsed+3.0;self.fall=3.0;}
    }
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {
        ensure!(dt.is_finite() && dt>=0.0,"invalid native effect timestep");
        self.dt=if !self.started {0.0}else if self.frames==1 {dt.min(0.033)}else{dt};self.started=true;self.frames+=1;self.elapsed+=self.dt;
        if self.template.kind==3017 {
            if self.duration>0.0 && self.elapsed>self.duration {
                if self.graceful {self.graceful=false;self.duration+=5.0;}else{return Ok(false);}
            }
        } else if self.elapsed>=self.duration && self.repetitions==0 {return Ok(false);}
        else if self.elapsed>self.duration {
            if self.elapsed<self.duration+self.template.float(41)? {return Ok(true);}
            self.repetitions-=1;if self.repetitions==0 {return Ok(false);}self.elapsed=0.0;self.reseed=self.template.float(30)?==999.0;
        }
        Ok(true)
    }
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let n=self.facets;
        let mut models=vec![(Some(self.template.word(9).unwrap_or(0) as usize),indices(n),n as usize*4)];
        if self.template.kind==3022 && self.template.word(39).unwrap_or(0) as i32>=0 {models.push((Some(self.template.word(39).unwrap_or(0) as usize),indices(1),4));}
        models
    }
    pub(super) fn blends(&self)->Vec<Blend> {
        let additive=if self.template.kind==3017 {self.template.word(10).unwrap_or(0)!=5}else{self.template.word(0).unwrap_or(0)&0x200!=0};
        let count=1+usize::from(self.template.kind==3022 && self.template.word(39).unwrap_or(0) as i32>=0);
        vec![if additive {Blend::Additive}else{Blend::AlphaBlend};count]
    }
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,camera:Vec3,right:Vec3,up:Vec3,gc:&mut R250,crt:&mut CrtRand,terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.template.kind==3017 {self.smoke(right,up,crt,terrain)}else{self.beam(camera,gc,terrain)}
    }
    fn beam(&mut self,camera:Vec3,gc:&mut R250,mut terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.reseed {self.random_rotation=super::random_fraction(gc)*2.0-1.0;self.reseed=false;}
        let t=&self.template;let flags=t.word(0)?;
        if self.elapsed>self.duration {
            let mut groups=vec![vec![Vertex {color:[0.0;4],..Vertex::default()};self.facets as usize*4]];
            if t.word(39)? as i32>=0 {groups.push(vec![Vertex {color:[0.0;4],..Vertex::default()};4]);}
            return Ok(Some(groups));
        }
        let time=self.elapsed%self.duration;
        let mut height=t.float(17)?;
        if !self.started || flags&0x800!=0 {self.anchor=self.source.w_axis.truncate();}
        if flags&0x2000!=0 {
            let ground=terrain.as_mut().context("Beam requires terrain locator")?(self.anchor).context("Beam terrain anchor not found")?.0.y;
            if flags&0x4000!=0 {height=self.source.w_axis.y-ground;if height<0.0 || height>t.float(17)? {height=0.0;}}
            self.anchor.y=ground;
        }
        height*=self.scale;
        if t.word(10)?==0 {self.anchor.y+=self.velocity*self.dt;self.velocity+=-800.0*self.dt;}
        let rise=t.float(11)?;
        let (a,b,f)=if time<rise {(18,20,time/rise)}else if time>self.duration-self.fall {(20,22,(time-self.duration+self.fall)/self.fall)}else{(20,20,0.0)};
        let mut bottom=t.float(a)?*(1.0-f)+t.float(b)?*f;
        let mut top=t.float(a+1)?*(1.0-f)+t.float(b+1)?*f;
        let ca=24+(a-18);let cb=24+(b-18);
        let colors=[color(t.word(ca)?,t.word(cb)?,f),color(t.word(ca+1)?,t.word(cb+1)?,f)];
        if flags&0x400!=0 {
            bottom+=(std::f32::consts::TAU*t.float(35)?*time+t.float(37)?.to_radians()).sin()*t.float(33)?+(super::random_fraction(gc)-1.0)*t.float(31)?;
            top+=(std::f32::consts::TAU*t.float(36)?*time+t.float(38)?.to_radians()).sin()*t.float(34)?+(super::random_fraction(gc)-1.0)*t.float(32)?;
        }
        let angle=if t.float(30)?==999.0 {self.random_rotation}else{t.float(30)?*time};
        let mut rotation=Mat4::from_quat(self.source.to_scale_rotation_translation().1);
        if flags&0x20000!=0 {rotation*=Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2);}
        rotation=Mat4::from_rotation_y(-angle)*rotation;
        let n=self.facets;let mut vertices=Vec::with_capacity(n as usize*4);
        let scroll=self.scroll;
        for i in 0..n {
            let a=std::f32::consts::FRAC_PI_4+std::f32::consts::PI*i as f32/n as f32;
            let local=[Vec3::new(a.sin()*bottom,0.0,-a.cos()*bottom),Vec3::new(-a.sin()*bottom,0.0,a.cos()*bottom),Vec3::new(a.sin()*top,height,-a.cos()*top),Vec3::new(-a.sin()*top,height,a.cos()*top)];
            let points=local.map(|p|self.anchor+rotation.transform_vector3(p));
            let mut uv=[[t.float(15)?,scroll+t.float(16)?],[0.0,scroll+t.float(16)?],[t.float(15)?,scroll],[0.0,scroll]];
            if flags&0x1000!=0 {for (j,u) in uv.iter_mut().enumerate(){u[1]=scroll+if j>=2 {t.float(16)?}else{0.0};}}
            if flags&0x100!=0 {for u in &mut uv {u.swap(0,1);}}
            let mut c=colors;
            if flags&0x10000!=0 {
                let normal=(points[1]-points[0]).cross(points[2]-points[0]).normalize_or_zero();
                let alpha=normal.dot((camera-self.anchor).normalize_or_zero()).abs();
                for c in &mut c {c[3]=(c[3]*alpha*255.0).trunc()/255.0;}
            }
            for j in 0..4 {vertices.push(Vertex {pos:points[j].to_array(),normal:[0.0,0.0,1.0],uv:uv[j],color:c[j/2]});}
        }
        let mut out=vec![vertices];
        if t.word(39)? as i32>=0 {
            let y=t.float(40)?*height;let positions=[Vec3::new(bottom,y,-bottom),Vec3::new(-bottom,y,-bottom),Vec3::new(bottom,y,bottom),Vec3::new(-bottom,y,bottom)].map(|p|self.anchor+rotation.transform_vector3(p));
            let mut c=colors[0];if flags&0x10000!=0 {let alpha=rotation.transform_vector3(Vec3::Y).dot((camera-self.anchor).normalize_or_zero()).abs();c[3]=(c[3]*alpha*255.0).trunc()/255.0;}
            let mut v=Vec::with_capacity(4);quad(&mut v,positions,[[1.0,1.0],[0.0,1.0],[1.0,0.0],[0.0,0.0]],c);out.push(v);
        }
        Ok(Some(out))
    }
    fn smoke(&mut self,right:Vec3,up:Vec3,crt:&mut CrtRand,mut terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>)->Result<Option<Vec<Vec<Vertex>>>> {
        let t=&self.template;let mode=t.word(10)?;
        let origin=if t.word(0)?&2==0 {self.root.context("GlobalSmoke requires actual source root")?}else{self.source.w_axis.truncate()};
        self.relative=if t.word(0)?&2==0 {self.source.w_axis.truncate()-origin}else{Vec3::ZERO};
        if mode==3 && self.environment.context("GlobalSmoke3 requires VisualEnv camera anchor")?.distance_squared(origin)>6400.0 {return Ok(Some(vec![vec![Vertex {color:[0.0;4],..Vertex::default()};256]]));}
        let mut count=t.word(30)? as i32+(crt.rand()&t.word(31)?) as i32;
        if mode==5 {count=((self.elapsed*t.word(30)? as f32) as i32-self.emitted).min(3);}
        let life=t.float(26)?;
        if self.duration>0.0 && self.elapsed>self.duration-life {count=0;}
        if mode==1 {
            let ground=|p:Vec3,terrain:&mut Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>|->Result<f32>{Ok(terrain.as_mut().context("GlobalSmoke1 requires terrain")?(p).context("GlobalSmoke1 terrain not found")?.0.y)};
            if self.retarget<self.elapsed {
                self.wander=self.relative+Vec3::new((crt.rand() as f32/204.8)-80.0,0.0,-((crt.rand() as f32/204.8)-80.0));
                self.wander.y=ground(origin+self.wander,&mut terrain)?;self.retarget=self.elapsed+2.0;self.heading=0.0;self.trail.fill(self.wander);
            }
            let direction=Vec3::new(self.heading.sin(),0.0,-self.heading.cos());
            let h=ground(origin+self.wander,&mut terrain)?;
            let next=ground(origin+self.wander+direction,&mut terrain)?;
            self.heading+=if h<=next {-0.4}else{0.4};self.wander+=direction*self.dt*6.0;self.wander.y=h+0.8;
            if self.trail_time<self.elapsed {self.trail_time=self.elapsed+0.2;self.trail[self.trail_write]=self.wander;self.trail_write=(self.trail_write+1)%5;}
        }
        let rotation_time=if mode==5 {self.elapsed/t.word(31)? as f32}else{self.elapsed};
        let &(_,cols,rows,_,last)=&materials::MATERIALS[t.word(9)? as usize];
        let mut vertices=Vec::with_capacity(256);
        for (i,p) in self.particles.iter_mut().enumerate() {
            let mut phase=0.0;
            if p.active {
                phase=(self.elapsed-p.born)/life;
                if phase>1.0 {p.active=false;}else{p.position+=p.velocity*self.dt;if matches!(mode,2|4) {p.velocity.y+=t.float(29)?*self.dt;}}
            } else if count!=0 {
                let speed=t.float(13)?;
                let (position,velocity)=match mode {
                    1=>{let pos=self.trail[self.trail_read];self.trail_read=(self.trail_read+1)%5;(pos,Vec3::X*speed)},
                    2=>(self.relative,Vec3::new((crt.rand() as f32/655350.0+1.0)*speed,-0.4*speed,0.0)),
                    3=>{let forward=if t.word(0)?&2!=0 {Vec3::Z}else{self.source.z_axis.truncate()};let sideways=if t.word(0)?&2!=0 {Vec3::X}else{self.source.x_axis.truncate()};let velocity=forward*((crt.rand() as f32/655350.0+1.0)*speed);(self.relative+sideways*(crt.rand() as f32/21845.0-1.5),velocity)},
                    4|5=>(self.relative,-Vec3::Y*speed),
                    6=>{let up=if t.word(0)?&2!=0 {Vec3::Y}else{self.source.y_axis.truncate()};(self.relative,-up*((crt.rand() as f32/655350.0+1.0)*speed))},
                    _=>{let mut velocity=sprites::sphere_point(crt);velocity.y=velocity.y.abs();(self.relative+Vec3::new(0.0,-(crt.rand() as f32/65535.0),0.0),velocity*speed)},
                };
                *p=SmokeParticle {position,velocity,born:self.elapsed,active:true};self.emitted+=1;count-=1;
            }
            if !p.active {vertices.extend([Vertex {color:[0.0;4],..Vertex::default()};4]);continue;}
            let angle=(i as f32-4.0)*((i&1) as f32-0.5)*if phase==0.0 {self.elapsed}else{rotation_time};
            let a=right*(t.float(14)?+(t.float(15)?-t.float(14)?)*phase)*0.5*angle.sin();
            let b=up*(t.float(16)?+(t.float(17)?-t.float(16)?)*phase)*0.5*angle.cos();
            let x=a+b;let y=right*(-b.dot(up))+up*a.dot(right);
            let center=origin+p.position;
            let frame=(last as f32*phase) as u32;let u=(frame%cols) as f32/cols as f32;let v=(frame/cols) as f32/rows as f32;
            let mut rgba=[0.0;4];for (j,c) in rgba.iter_mut().enumerate(){let component=[3,0,1,2][j];*c=self.start.map_or(t.float(18+j)?,|c|c[component])*(1.0-phase)+self.stop.map_or(t.float(22+j)?,|c|c[component])*phase;}
            let byte=|v:f32|(v*255.0).trunc() as i32 as u32;
            let packed_color=((byte(rgba[0])<<8|byte(rgba[1]))<<8|byte(rgba[2]))<<8|byte(rgba[3]);
            quad(&mut vertices,[center-x-y,center+x-y,center-x+y,center+x+y],[[u,v+1.0/rows as f32],[u+1.0/cols as f32,v+1.0/rows as f32],[u,v],[u+1.0/cols as f32,v]],packed(packed_color));
        }
        if !self.source_visible {for v in &mut vertices {v.color=[0.0;4];}}
        Ok(Some(vec![vertices]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // Exact CMSBlock payloads from installed gfxtweak records12252/71109.
    fn smoke()->Template {Template {kind:3017,words:vec![0x5,0,0xbe4ccccd,0x3dcccccd,0,0,0,0x7d2,0x40400000,0x31,2,0x3ecccccd,0x3e19999a,0x3f19999a,0x3e99999a,0x3f800000,0x3e99999a,0x3f800000,0x3dcccccd,0x3f800000,0x3f800000,0x3f800000,0x3d4ccccd,0x3f800000,0x3f800000,0x3f800000,0x3fb33333,0,0x3f800000,0x3f000000,1,0]}}
    fn beam()->Template {Template {kind:3022,words:vec![0xe03,0,0,0,0,0,0,0,0x41200000,0x4a,1,0x40400000,0x40400000,1,8,0x3f800000,0x3f800000,0x40800000,0x40000000,0x40000000,0x40000000,0x40000000,0x40000000,0x40000000,0x2050a0,0x2050a0,0xa02050a0,0xa02050a0,0x2050a0,0x2050a0,0x3e4ccccd,0,0,0x3e800000,0x3e800000,0x3dcccccd,0x3dcccccd,0,0x42340000,8,0x3e4ccccd,0]}}
    #[test]
    fn authored_global_smoke_gravity_slots_and_graceful_extension() {
        let (mut gc,mut ds,mut crt)=(R250::new(1),R250::new(2),CrtRand::new(1));
        let mut n=NativeEffect::new(&smoke(),Mat4::IDENTITY,EffectConfig::default(),&mut gc,&mut ds).unwrap();
        assert!(n.frame(0.5).unwrap());
        let v=n.vertices(Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut crt,None).unwrap().unwrap();
        assert_eq!(v[0].len(),256);assert_eq!(n.emitted,1);
        assert_eq!(n.particles[0].position,n.relative);
        assert!((n.particles[0].velocity.y+0.24).abs()<1e-6);
        assert_eq!(n.models()[0].1.len(),384);
        n.frame(0.01).unwrap();n.vertices(Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut crt,None).unwrap();
        assert!((n.particles[0].velocity.y+0.235).abs()<1e-6);
        n.graceful();assert!(n.frame(3.0).unwrap());assert_eq!(n.duration,8.0);
        assert!(!n.frame(5.0).unwrap());
    }
    #[test]
    fn authored_beam_facets_cap_and_native_fade_termination() {
        let (mut gc,mut ds,mut crt)=(R250::new(1),R250::new(2),CrtRand::new(1));
        let mut n=NativeEffect::new(&beam(),Mat4::IDENTITY,EffectConfig::default(),&mut gc,&mut ds).unwrap();
        assert!(n.frame(0.0).unwrap());
        let v=n.vertices(Vec3::new(2.0,2.0,5.0),Vec3::X,Vec3::Y,&mut gc,&mut crt,None).unwrap().unwrap();
        assert_eq!((v[0].len(),v[1].len()),(32,4));
        assert!((v[0][0].pos[0]-2.0f32.sqrt()).abs()<1e-6);
        assert_eq!(v[0][2].pos[1],4.0);assert_eq!(v[1][0].pos[1],0.8);
        n.graceful();assert_eq!((n.duration,n.fall),(3.0,3.0));
        n.frame(0.01).unwrap();assert!(!n.frame(3.0).unwrap());
    }
    #[test]
    fn relative_smoke_births_follow_attractor_existing_particles_follow_root() {
        let (mut gc,mut ds,mut crt)=(R250::new(1),R250::new(2),CrtRand::new(1));
        let config=EffectConfig {track_source:true,..Default::default()};
        let mut n=NativeEffect::new(&smoke(),Mat4::from_translation(Vec3::new(10.0,2.0,3.0)),config,&mut gc,&mut ds).unwrap();
        assert!(n.needs_source_root());
        n.set_source_root(Mat4::from_translation(Vec3::new(10.0,0.0,3.0)));
        n.frame(0.0).unwrap();n.vertices(Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut crt,None).unwrap();
        let born=n.particles[0].position;
        n.update_source(Mat4::from_translation(Vec3::new(11.0,5.0,3.0))).unwrap();
        n.set_source_root(Mat4::from_translation(Vec3::new(11.0,0.0,3.0)));
        n.frame(0.0).unwrap();n.vertices(Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut crt,None).unwrap();
        assert_eq!(n.particles[0].position,born);
        assert_ne!(n.particles[1].position,born);
        assert!(n.needs_source_root(),"moving root must refresh every frame");
        n.set_source_visible(false);n.frame(0.0).unwrap();
        let hidden=n.vertices(Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut crt,None).unwrap().unwrap();
        assert!(hidden[0].iter().all(|v|v.color==[0.0;4]));
        assert_eq!(n.emitted,3,"hidden native Sol still simulates");
    }
    #[test]
    fn installed_all_global_smoke_and_beam_modes() {
        let templates=super::super::Templates::open(&ao_gui::client_dir()).unwrap();
        let mut smoke_modes=std::collections::BTreeSet::new();let mut count=0;
        let (mut gc,mut ds,mut crt)=(R250::new(1),R250::new(2),CrtRand::new(1));
        for (&id,t) in templates.by_id.iter().filter(|(_,t)|matches!(t.kind,3017|3022)) {
            count+=1;if t.kind==3017 {smoke_modes.insert(t.words[10]);}
            let mut n=NativeEffect::new(t,Mat4::IDENTITY,EffectConfig::default(),&mut gc,&mut ds).unwrap_or_else(|e|panic!("authored{id}: {e:#}"));
            n.set_environment_position(Vec3::ZERO);
            for _ in 0..60 {
                if !n.frame(1.0/60.0).unwrap() {break;}
                let mut flat=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
                let groups=n.vertices(Vec3::new(2.0,2.0,5.0),Vec3::X,Vec3::Y,&mut gc,&mut crt,Some(&mut flat)).unwrap();
                for (vertices,(_,_,capacity)) in groups.unwrap().iter().zip(n.models()) {
                    assert_eq!(vertices.len(),capacity,"authored{id}");
                    assert!(vertices.iter().all(|v|v.pos.iter().chain(v.uv.iter()).chain(v.color.iter()).all(|f|f.is_finite())),"authored{id}");
                }
            }
        }
        assert_eq!(smoke_modes,std::collections::BTreeSet::from([0,1,2,3,4,5,6]));
        assert_eq!(count,51);
    }
    #[test]
    #[ignore="installed retail assets and offscreen Metal rendering"]
    fn authored_native3017_3022_frames() {
        use super::super::{Binding,Renderer,MODEL_BASE};
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).unwrap_or_else(||"/tmp/FxClasses/frames".into());
        std::fs::create_dir_all(&out).unwrap();
        let mut renderer=Renderer::open(&ao_gui::client_dir()).unwrap();
        let mut ids:Vec<_>=renderer.templates.by_id.iter().filter(|(_,t)|matches!(t.kind,3017|3022)).map(|(&id,_)|id).collect();ids.sort_unstable();
        let origin=Vec3::new(5000.0,0.0,5000.0);let eye=origin+Vec3::new(4.0,4.0,10.0);
        let mut host=ao_render::Host::headless();host.camera=ao_render::Camera::look_at(eye,origin);
        for id in ids {
            renderer.clear();host.actor_models.clear();
            let handle=renderer.spawn_configured(Binding {group:0,attractor:0,effect:id,note:0,color:0},Mat4::from_translation(origin),origin,EffectConfig {creation:super::super::Creation::Matrix,..Default::default()}).unwrap();
            for frame in 1..=60 {
                host.actors.clear();let mut flat=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
                renderer.frame(1.0/60.0,&mut host,Some(&mut flat));
                if [1,6,30,60].contains(&frame) {
                    let mut models:Vec<_>=renderer.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                    models.extend(renderer.buff_models.iter().map(|(&id,m)|(0xfac2_0000_0000_0000|u64::from(id),m.scene.clone())));
                    ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,host.actors.clone(),eye.to_array(),origin.to_array(),640,480,&out.join(format!("native301722_{id}_{frame}.png")),frame as f32/60.0).unwrap();
                }
            }
            renderer.terminate_gracefully(handle);renderer.delete(handle);assert!(!renderer.is_active(handle));
        }
    }
}
