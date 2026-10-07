//! LavaBall3015: GC100e435c/100e4031/100e413e/100e3005/100e3f04.
//! Two 128-entry DiaBill arrays; DS10010d70/1001105e owns atlas geometry.
use super::{materials,quad,sprites,AuxSound,EffectConfig,Template};
use anyhow::{ensure,Context,Result};
use ao_formats::character::CrtRand;
use ao_scene::{Blend,Vertex};
use glam::{Mat4,Vec3};
const CAPACITY:usize=128;
const REFLECT:Vec3=Vec3::new(1.0,1.0,-1.0);
const PI:f32=f64::from_bits(0x400921fb60000000) as f32;
#[derive(Clone,Copy,Default)]
struct Sprite {position:Vec3,velocity:Vec3,born:f32,frame:i32,active:bool}
pub(super) struct LavaBallEffect {
    template:Template,source:Mat4,relative:Vec3,smoke:[Sprite;CAPACITY],fire:[Sprite;CAPACITY],
    state:u8,time:f32,previous:f32,wait_until:f32,emitted:i32,impact_left:i32,
    position:Vec3,velocity:Vec3,azimuth:f32,angle:f32,hit:bool,stopping:bool,done:bool,
    duration:f32,sounds:Vec<AuxSound>,
}
impl LavaBallEffect {
    pub(super) fn new(t:&Template,source:Mat4,relative:Vec3,c:EffectConfig)->Result<Self> {
        ensure!(t.kind==3015,"unsupported LavaBall class {}",t.kind);
        for i in [8,14,15,16,17,18,19,26,27] {t.float(i)?;}
        for i in [9,10] {materials::MATERIALS.get(t.word(i)? as usize).context("unknown LavaBall material")?;}
        for i in [15,18,26] {ensure!(t.float(i)?>0.0,"nonpositive LavaBall lifetime field {i}");}
        ensure!(c.duration.is_none_or(f32::is_finite),"nonfinite LavaBall duration");
        Ok(Self {template:t.clone(),source,relative,smoke:[Sprite::default();CAPACITY],fire:[Sprite::default();CAPACITY],state:4,time:0.0,previous:0.0,wait_until:0.0,emitted:0,impact_left:0,position:Vec3::ZERO,velocity:Vec3::ZERO,azimuth:0.0,angle:0.0,hit:false,stopping:false,done:false,duration:c.duration.unwrap_or(-1.0),sounds:Vec::new()})
    }
    // Color setters100e3f19/100e3f4c write unused interpolation fields;
    // process100e3005 always writes white fire and opaque black smoke.
    pub(super) fn update_source(&mut self,source:Mat4) {self.source=source;}
    pub(super) fn terminate_gracefully(&mut self) {self.stopping=true;}
    pub(super) fn source_lost(&mut self) {self.stopping=true;}
    pub(super) fn priority(&self)->i32 {if self.template.word(11).unwrap_or(0)!=0 {1}else{6}}
    // DS100078c8 uses +1a0 as the render-list bucket for non-world coordinates.
    pub(super) fn render_bucket(&self)->Option<i32> {if self.template.word(11).unwrap_or(0)!=0 {Some(100)}else{None}}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        [9,10].map(|i|(Some(self.template.word(i).unwrap_or(0) as usize),(0..CAPACITY as u32).flat_map(|i|[4*i,4*i+1,4*i+2,4*i+1,4*i+2,4*i+3]).collect(),CAPACITY*4)).into()
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.template.word(13).unwrap_or(0)==1 {Blend::AlphaBlend}else{Blend::Additive},Blend::Additive]}
    pub(super) fn take_sounds(&mut self)->Vec<AuxSound> {std::mem::take(&mut self.sounds)}
    fn sound(&mut self,name:&str,p:Vec3) {self.sounds.push(AuxSound {id:ao_audio::sbf::sound_id(name),pos:p.to_array(),velocity:[0.0;3],parameters:[1.0,0.0,0.0,0.0],probability:100});}
    fn f(&self,i:usize)->f32 {f32::from_bits(self.template.word(i).unwrap_or(0))}
    fn rate_count(&self)->i32 {(self.f(16) as f64*self.time as f64) as i32}
    fn age_smoke(&mut self)->(Option<usize>,usize) {
        let life=self.f(18);let mut free=None;let mut alive=0;
        for (i,p) in self.smoke.iter_mut().enumerate() {if !p.active {free=Some(i);}else{let phase=(self.time-p.born)/life;if phase>1.0 {p.active=false;}else{p.frame=(phase*64.0) as i32;alive+=1;}}}
        (free,alive)
    }
    fn smoke_at(&mut self,index:usize) {self.smoke[index]=Sprite {position:self.position,born:self.time,active:true,..Sprite::default()};}
    #[allow(clippy::too_many_arguments,clippy::type_complexity)]
    pub(super) fn vertices(&mut self,time:f32,camera:Vec3,right:Vec3,up:Vec3,crt:&mut CrtRand,mut terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>,mut collision:Option<&mut dyn FnMut(Vec3,Vec3)->Option<Vec3>>)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.done {return Ok(None);}
        let dt=(time-self.previous).max(0.0);self.previous=time;self.time=time;
        if self.duration>=0.0 && time>self.duration {self.done=true;return Ok(None);}
        let mode=self.template.word(11).unwrap_or(0);let origin=self.source.w_axis.truncate();
        match self.state {
            4=>{for p in self.smoke.iter_mut().chain(self.fire.iter_mut()) {p.active=false;}self.done=self.stopping;self.state=0;},
            0=>{self.wait_until=time+(crt.rand() as f64*self.f(27) as f64/16384.0) as f32;self.state=if mode==0 {1}else{5};},
            1=>{
                if time<self.wait_until {return Ok(Some(self.draw(camera,right,up)));}
                self.sound("SM_Sandy_EP01_fireball_launch_1",origin);self.sound("SM_Sandy_EP01_fireball_loop_1",origin);
                self.position=Vec3::Y*(self.relative.y+f64::from_bits(0x3fc99999a0000000) as f32);
                self.velocity=Vec3::Y*15.0+sprites::sphere_point(crt)*4.0;
                self.emitted=self.rate_count()-5;self.hit=false;self.state=2;
                for i in 0..20 {let velocity=Vec3::Y*1.5+sprites::sphere_point(crt)*2.0;sprites::sphere_point(crt);self.fire[i]=Sprite {position:self.position,velocity,born:time,active:true,..Sprite::default()};}
            },
            5=>{
                if time<self.wait_until {return Ok(Some(self.draw(camera,right,up)));}
                self.azimuth=((crt.rand() as i32-0x4000) as f64*f64::from_bits(0x400921fb60000000)/16384.0) as f32;
                self.emitted=self.rate_count()-5;self.state=6;self.angle=crt.rand() as f32/32768.0;self.hit=false;
            },
            2|6=>{
                let (free,mut alive)=self.age_smoke();let mut budget=(self.rate_count()-self.emitted).max(0);
                if self.state==2 {
                    let old=origin+self.position*REFLECT;
                    if !self.hit {
                        self.sound("SM_Sandy_EP01_fireball_loop_1",old);
                        self.position+=self.velocity*dt;self.velocity.y-=dt*(f64::from_bits(0x40239999a0000000) as f32);
                        if let Some(index)=free {self.smoke_at(index);}
                    }
                    let ground=terrain.as_mut().context("LavaBall mode0 requires terrain")?(origin+self.position*REFLECT).context("LavaBall trajectory has no ground")?.0.y;
                    // Native compares local Y to absolute ground Y, without subtracting source Y.
                    if self.position.y<ground {self.position.y=ground;self.hit=true;self.impact_left=30;self.sound("SM_Sandy_EP01_fireball_ground_1",origin+self.position*REFLECT);}
                    if !self.hit {if let Some(hit)=collision.as_mut().context("LavaBall mode0 requires segment collision")?(old,origin+self.position*REFLECT) {self.position=(hit-origin)*REFLECT;self.hit=true;self.impact_left=30;self.sound("SM_Sandy_EP01_fireball_ground_1",hit);}}
                    if self.hit {budget=budget.min(self.impact_left);}
                } else {
                    let phase=(time-self.wait_until)/self.f(26);
                    if phase>1.0 {budget=0;self.hit=true;}
                    let a=if mode==1 {phase*PI}else{self.angle};let x=a.cos()*950.0;let y=a.sin()*950.0;
                    self.position=Vec3::new(self.azimuth.cos()*x,y*0.5,self.azimuth.sin()*x);
                    self.velocity=if mode==1 {Vec3::new(-self.azimuth.cos()*y,x*0.5,-self.azimuth.sin()*y)}else{sprites::sphere_point(crt);Vec3::ZERO};
                    if !self.hit {if let Some(index)=free {self.smoke_at(index);}}
                }
                let life=self.f(15);let spread=self.f(14);
                for i in 0..CAPACITY {
                    if self.fire[i].active {
                        let p=&mut self.fire[i];p.velocity*=0.8;p.position+=p.velocity*dt;
                        let phase=(time-p.born)/life;if phase>1.0 {p.active=false;}else{p.frame=(phase*64.0) as i32;alive+=1;}
                    } else if budget>0 {
                        let v=sprites::sphere_point(crt);let velocity=if self.state==2&&self.hit {Vec3::Y*1.5+v*2.0}else{self.velocity+v};
                        let mut position=self.position;if self.state==2&&self.hit {position+=Vec3::Y*0.2;}
                        position+=sprites::sphere_point(crt)*spread;
                        self.fire[i]=Sprite {position,velocity,born:time,active:true,..Sprite::default()};
                        self.emitted+=1;self.impact_left-=1;budget-=1;alive+=1;
                    }
                }
                if self.hit&&alive==0 {self.state=4;}
            },
            _=>{},
        }
        Ok(Some(self.draw(camera,right,up)))
    }
    fn draw(&self,camera:Vec3,right:Vec3,up:Vec3)->Vec<Vec<Vertex>> {
        let origin=if self.template.word(11).unwrap_or(0)==0 {self.source.w_axis.truncate()}else{camera};
        [(9,&self.smoke,self.f(19),[0.0,0.0,0.0,1.0]),(10,&self.fire,self.f(17),[1.0;4])].map(|(material,particles,size,color)|{
            let m=materials::MATERIALS[self.template.word(material).unwrap_or(0) as usize];let mut vertices=Vec::with_capacity(CAPACITY*4);
            for p in particles {let pos=origin+p.position*REFLECT;let x=right*(size*0.5);let y=up*(size*0.5);let u=(p.frame%m.1 as i32) as f32/m.1 as f32;let v=(p.frame/m.1 as i32) as f32/m.2 as f32;
                quad(&mut vertices,[pos-x-y,pos+x-y,pos-x+y,pos+x+y],[[u,v+1.0/m.2 as f32],[u+1.0/m.1 as f32,v+1.0/m.2 as f32],[u,v],[u+1.0/m.1 as f32,v]],if p.active {color}else{[0.0;4]});
            }vertices
        }).into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_lavaball_wait_field_is_zero_in_frame() {
        let mut t=authored(0);t.words.truncate(27);
        let mut e=LavaBallEffect::new(&t,Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).unwrap();
        assert_eq!(e.f(27),0.0);e.models();e.blends();assert_eq!(e.priority(),6);assert_eq!(e.render_bucket(),None);
        e.state=0;
        e.vertices(0.1,Vec3::ZERO,Vec3::X,Vec3::Y,&mut CrtRand::new(1),None,None).unwrap();
        assert_eq!(e.wait_until,0.1);assert_eq!(e.state,1);
    }
    fn authored(mode:u32)->Template {
        let words=match mode {
            0=>vec![5,0,0,0,0,0,0,2001,3212836864,49,9,0,4278190080,1,1047233823,1063675494,1103626240,1074161254,1066192077,1067030938,0,0,0,0,0,0,1085276160,1065353216],
            1=>vec![5,0,0,0,0,0,0,2001,3212836864,49,9,1,4278190080,1,1047233823,1063675494,1103626240,1101004800,1065353216,1101004800,0,0,0,0,0,0,1089470464,1097859072],
            _=>vec![5,0,0,0,0,0,0,2001,3212836864,49,9,2,4278190080,1,1075838976,1045220557,1097859072,1106247680,1053609165,1106247680,0,0,0,0,0,0,1060320051,1065353216],
        };
        Template {kind:3015,words}
    }
    #[test]
    fn authored_lavaball_lifecycle_and_atlas() {
        // Authored12300,12301,12560 (12580 duplicates12560).
        for mode in 0..=2 {
            let t=authored(mode);let mut e=LavaBallEffect::new(&t,Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).unwrap();
            let mut crt=CrtRand::new(1);let mut terrain=|p:Vec3|Some((Vec3::new(p.x,-100.0,p.z),Vec3::Y));let mut collision=|_:Vec3,_:Vec3|None;
            e.vertices(0.01,Vec3::ZERO,Vec3::X,Vec3::Y,&mut crt,Some(&mut terrain),Some(&mut collision)).unwrap();
            assert_eq!(e.state,0);
            e.vertices(0.02,Vec3::ZERO,Vec3::X,Vec3::Y,&mut crt,Some(&mut terrain),Some(&mut collision)).unwrap();
            assert_eq!(e.state,if mode==0 {1}else{5});e.wait_until=0.02;
            e.vertices(0.03,Vec3::ZERO,Vec3::X,Vec3::Y,&mut crt,Some(&mut terrain),Some(&mut collision)).unwrap();
            assert_eq!(e.state,if mode==0 {2}else{6});
            if mode==0 {assert_eq!(e.fire.iter().filter(|p|p.active).count(),20);assert_eq!(e.take_sounds().len(),2);}
            let groups=e.vertices(0.04,Vec3::ZERO,Vec3::X,Vec3::Y,&mut crt,Some(&mut terrain),Some(&mut collision)).unwrap().unwrap();
            assert_eq!(groups.len(),2);assert!(groups.iter().all(|g|g.len()==512));
            assert!(groups.iter().flatten().all(|v|v.pos.iter().chain(v.color.iter()).all(|x|x.is_finite())));
            e.terminate_gracefully();e.state=4;
            e.vertices(0.05,Vec3::ZERO,Vec3::X,Vec3::Y,&mut crt,Some(&mut terrain),Some(&mut collision)).unwrap();
            assert!(e.vertices(0.06,Vec3::ZERO,Vec3::X,Vec3::Y,&mut crt,Some(&mut terrain),Some(&mut collision)).unwrap().is_none());
        }
    }
    #[test]
    fn ground_collision_caps_native_impact_emission() {
        let t=authored(0);let mut e=LavaBallEffect::new(&t,Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).unwrap();
        e.state=2;e.position=Vec3::Y;e.velocity=Vec3::ZERO;e.emitted= -5;
        let mut crt=CrtRand::new(1);let mut terrain=|p:Vec3|Some((Vec3::new(p.x,2.0,p.z),Vec3::Y));let mut collision=|_:Vec3,_:Vec3|None;
        e.vertices(0.01,Vec3::ZERO,Vec3::X,Vec3::Y,&mut crt,Some(&mut terrain),Some(&mut collision)).unwrap();
        assert!(e.hit);assert_eq!(e.position.y,2.0);assert_eq!(e.impact_left,25);
        assert_eq!(e.take_sounds().last().unwrap().id,ao_audio::sbf::sound_id("SM_Sandy_EP01_fireball_ground_1"));
    }
    #[test]
    #[ignore="installed retail records and offscreen GPU"]
    fn all_authored_lavaball_frames()->Result<()> {
        use super::super::{Binding,Creation,Renderer,Templates,MODEL_BASE};
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).context("set AOMAC_EFFECT_FRAMES")?;
        std::fs::create_dir_all(&out)?;
        let table=Templates::open(&ao_gui::client_dir())?;let mut renderer=Renderer::open(&ao_gui::client_dir())?;let mut host=ao_render::Host::headless();
        for (&id,t) in table.by_id.iter().filter(|(_,t)|t.kind==3015) {
            renderer.clear();let eye=if t.words[11]==0 {Vec3::new(2.0,2.0,5.0)}else{Vec3::ZERO};
            let target=if t.words[11]==0 {Vec3::Y}else{Vec3::Z* -950.0};host.camera=ao_render::Camera::look_at(eye,target);
            renderer.spawn(Binding {group:0,attractor:0,effect:id,note:0,color:0},Creation::Matrix,Mat4::IDENTITY,Vec3::X)?;
            for frame in 1..=1200 {
                let mut terrain=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
                host.actors.clear();renderer.frame(1.0/60.0,&mut host,Some(&mut terrain));
                if ![60,120,300,600,1200].contains(&frame) {continue;}
                let models:Vec<_>=renderer.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,host.actors.clone(),eye.to_array(),target.to_array(),640,480,&out.join(format!("lavaball3015_{id}_{frame}.png")),frame as f32/60.0)?;
            }
        }
        Ok(())
    }
}
