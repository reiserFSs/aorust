//! Sprite controls: GC Fire 100dc296, Nano0 100e6e95, Nano1 100e827a,
//! Smoke 100f09e6, Sprite 100f62a7. DS Sprite2Type0 100267c1/10025391.
use super::{materials, quad, EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};
use ao_formats::character::CrtRand;
use ao_formats::weather::R250;
use std::sync::{Mutex, OnceLock};

fn display_uniform(random:&mut R250)->f32 {super::random_fraction(random)}

/// GC 10106259/10105c3d and rotations 100d25e4/100d26a9/100d276c.
/// Input/output are scene `F*M*F`, with native Z reflected.
pub(super) fn connector(template:&Template, source:Mat4)->Result<Mat4> {
    let x=Mat4::from_rotation_x(-template.float(4)?);
    let y=Mat4::from_rotation_y(-template.float(5)?);
    let z=Mat4::from_rotation_z(template.float(6)?);
    let rotation=if template.word(0)?&4==0 {x*y*z}else{z*y*x};
    let mut matrix=source*rotation;
    let offset=Vec3::new(template.float(1)?,template.float(2)?,-template.float(3)?);
    matrix.w_axis=(matrix.w_axis.truncate()+matrix.transform_vector3(offset)).extend(1.0);
    Ok(matrix)
}

/// GC 100d2ab0: one shared 2048-entry sphere table and the retail index walk.
pub(super) fn sphere_point(rng:&mut CrtRand) -> Vec3 {
    // Initialization consumes the live CRT stream, so LazyLock cannot supply it.
    static TABLE:OnceLock<[Vec3;2048]>=OnceLock::new();
    let table=TABLE.get_or_init(||std::array::from_fn(|_|loop {
        let v=Vec3::new(rng.rand() as f32*2.0/32768.0-1.0,rng.rand() as f32*2.0/32768.0-1.0,rng.rand() as f32*2.0/32768.0-1.0);
        if v.length_squared()>0.0 && v.length_squared()<1.0 {break v.normalize();}
    }));
    static WALK:Mutex<(u32,u32)>=Mutex::new((0,11));
    let mut walk=WALK.lock().unwrap_or_else(|e|e.into_inner());
    walk.0=walk.0.wrapping_add(walk.1)&0x7ff;
    walk.1=walk.1.wrapping_add(7)^walk.0;
    table[walk.0 as usize]
}

/// Native ARGB packing (GC bias 1016c8d0 / DS bias 10091410 = .49999).
pub(super) fn render_color(color:[f32;4])->[f32;4] {
    let byte=|v:f32|(v*255.0-0.49999).round_ties_even() as i32 as u32;
    let packed=((byte(color[3])<<8|byte(color[0]))<<8|byte(color[1]))<<8|byte(color[2]);
    let [a,r,g,b]=packed.to_be_bytes();
    [(r as f32/255.0).powf(2.2),(g as f32/255.0).powf(2.2),(b as f32/255.0).powf(2.2),a as f32/255.0]
}

const NANO_ANCHORS: [i32;14] = [1002,1003,1001,1000,1005,1011,1012,1007,1008,1006,1013,1014,1009,1010];

struct SpriteParticle {
    position: Vec3,
    velocity: Vec3,
    previous_velocity: Vec3,
    remaining: f32,
    width: f32,
    height: f32,
    width_rate: f32,
    height_rate: f32,
    color: [f32;4],
    color_rate: [f32;4],
    frame: f32,
    frame_rate: f32,
    mode: i32,
    low: f32,
    high: f32,
    wind_scale: f32,
}

pub(super) struct SpriteEffect {
    template: Template,
    source: Mat4,
    color: u32,
    effect: i32,
    particles: Vec<SpriteParticle>,
    capacity: usize,
    default_direction:Vec3,
    previous_time: f32,
    previous_source: Vec3,
    emitted: i32,
    wind: Vec3,
    smoke_wind: Vec3,
    anchors: Vec<Mat4>,
    stage: usize,
    next_stage: f32,
    config: EffectConfig,
}

impl SpriteEffect {
    pub(super) fn supports(kind: i32) -> bool { matches!(kind,1004|1007|1008|1009|1012|1018|1023) }

    pub(super) fn new_with_id(effect: i32, template: &Template, source: Mat4, _target: Mat4, color: u32, display_random:&mut R250) -> Result<Self> {
        ensure!(Self::supports(template.kind), "unsupported sprite class {}", template.kind);
        let last = match template.kind { 1004=>31,1007|1008|1023=>38,1009=>32,1018=>40,_=>26 };
        for i in 8..=last {
            let integer=i==9 || (template.kind==1012 && matches!(i,10|24))
                || (template.kind==1004 && matches!(i,29|31))
                || (template.kind==1009 && i==29)
                || (matches!(template.kind,1007|1008|1023) && matches!(i,24|31|37))
                || (template.kind==1018 && matches!(i,24|31|38|40));
            if !integer {template.float(i)?;}
        }
        if template.kind!=1012 {ensure!(template.float(10)? >= 0.0,"invalid sprite emission rate");}
        materials::MATERIALS.get(template.word(9)? as usize).context("unknown sprite material")?;
        let life = template.float(if template.kind == 1012 { 23 } else if matches!(template.kind,1007|1008|1018|1023) {35} else {26})?;
        ensure!(life >= 0.0, "invalid sprite lifetime");
        let capacity = if template.kind == 1012 {
            1
        } else {
            let initial = if matches!(template.kind,1007|1008|1018|1023) { template.word(31)? as usize } else {0};
            ((template.float(10)?*life*1.5) as usize).max(initial)
        };
        ensure!(capacity <= u16::MAX as usize/4, "sprite capacity exceeds native indices");
        let out = Self { template:template.clone(),source,color,effect,particles:Vec::with_capacity(capacity),capacity,
            default_direction:if matches!(template.kind,1012|1018|1023) {Vec3::ZERO}else{Vec3::new(display_uniform(display_random)*2.0-1.0,display_uniform(display_random)*0.89+0.1,-(display_uniform(display_random)*2.0-1.0))},previous_time:0.0,previous_source:source.w_axis.truncate(),emitted:0,
            wind:Vec3::ZERO,smoke_wind:Vec3::ZERO,anchors:vec![],stage:0,next_stage:0.0,config:EffectConfig::default() };
        // Nano0's initial burst waits for Process, after CreateEffect2 setters.
        Ok(out)
    }

    /// Nano3's burst is constructed in Init100e9ad7, before CreateEffect2 setters.
    pub(super) fn initialize_native(&mut self, random:&mut R250, crt:&mut CrtRand)->Result<()> {
        if self.template.kind==1023 {
            let color=self.color;
            self.color=0;
            for _ in 0..self.template.word(31)? {self.emit(self.emission_position(),random,crt)?;}
            self.color=color;
        }
        Ok(())
    }

    pub(super) fn configure(&mut self, config: EffectConfig) { self.config=config; }
    pub(super) fn update_wind(&mut self, current:Vec3, smoothed:Vec3) { self.wind=current*Vec3::new(1.0,1.0,-1.0); self.smoke_wind=smoothed*Vec3::new(1.0,1.0,-1.0); }
    pub(super) fn anchor_ids(&self) -> &[i32] { if self.template.kind==1008 { &NANO_ANCHORS } else { &[] } }
    pub(super) fn update_anchors(&mut self, anchors:&[Mat4])->Result<()> {
        self.anchors.clear();
        ensure!(anchors.iter().all(|m|m.is_finite()),"nonfinite Nano1 skeleton anchor");
        self.anchors.extend_from_slice(anchors);
        Ok(())
    }

    fn emission_matrix(&self)->Result<Mat4> {
        if self.template.word(0).unwrap_or(0)&2!=0 {return Ok(Mat4::IDENTITY);}
        let mut matrix=self.source;
        if matches!(self.template.kind,1004|1009) {
            let offset=Vec3::new(self.template.float(1)?,self.template.float(2)?,-self.template.float(3)?);
            matrix.w_axis=(matrix.w_axis.truncate()-matrix.transform_vector3(offset)).extend(1.0);
        }
        Ok(matrix)
    }

    fn emission_position(&self)->Vec3 {
        if self.template.word(0).unwrap_or(0)&2!=0 {Vec3::ZERO}else{self.source.w_axis.truncate()}
    }

    /// GC 100db97d,100e6475/100e674f,100eff68 create different native particles.
    fn emit(&mut self, at:Vec3,random:&mut R250,crt:&mut CrtRand) -> Result<()> {
        if self.particles.len()>=self.capacity { return Ok(()); }
        let nano=matches!(self.template.kind,1007|1008|1018|1023);
        let smoke=self.template.kind==1009;
        let mut life=if nano {self.template.float(34)?}else{self.template.float(26)?};
        if smoke { life*=1.0+(super::random_fraction(random))*0.29999995; }
        if !nano {ensure!(life>0.0,"invalid sprite particle lifetime");}
        let width_index=if nano {12}else{14};
        let color_index=if nano {16}else{18};
        let mut width=self.template.float(width_index)?;
        let mut height=self.template.float(width_index+2)?;
        let mut width_end=self.template.float(width_index+1)?;
        let mut height_end=self.template.float(width_index+3)?;
        let mut position=at;
        let matrix=self.emission_matrix()?;
        let mut height_base=None;
        let velocity;
        if nano {
            let az=self.template.float(25)?+(self.template.float(26)?-self.template.float(25)?)*(super::random_fraction(random));
            let el=self.template.float(27)?+(self.template.float(28)?-self.template.float(27)?)*(super::random_fraction(random));
            let speed=self.template.float(29)?+(self.template.float(30)?-self.template.float(29)?)*(super::random_fraction(random));
            velocity=matrix.transform_vector3(Vec3::new(az.cos()*el.cos(),el.sin(),-az.sin()*el.cos())*speed)*if self.template.kind==1018 {1.0}else{self.template.float(32)?};
            life=self.template.float(34)?+(self.template.float(35)?-self.template.float(34)?)*(super::random_fraction(random));
            ensure!(life>0.0,"invalid Nano particle lifetime");
            if self.template.kind==1018 {position=matrix.transform_point3(Vec3::ZERO);}
        } else if smoke {
            let az=(super::random_fraction(random))*std::f32::consts::TAU;
            let el=(super::random_fraction(random))*std::f32::consts::FRAC_PI_2;
            let radius=self.template.float(11)?+(self.template.float(12)?-self.template.float(11)?)*(super::random_fraction(random));
            let scale=1.0+(super::random_fraction(random))*0.29999995;
            let local=Vec3::new(az.cos()*el.cos()*radius,self.template.float(31)?+(self.template.float(32)?-self.template.float(31)?)*(super::random_fraction(random)),-az.sin()*el.cos()*radius);
            height_base=Some(local.y);
            position=matrix.transform_point3(local);
            velocity=matrix.transform_vector3(Vec3::new(local.x*0.3,0.0,local.z*0.3)*self.template.float(13)?/life);
            width*=scale; height*=scale; width_end*=scale; height_end*=scale;
        } else {
            let radius=self.template.float(11)?+(self.template.float(12)?-self.template.float(11)?)*(super::random_fraction(random));
            let sphere=sphere_point(crt);
            let local=Vec3::new(sphere.x*radius,sphere.z*radius,0.0);
            position=matrix.transform_point3(local);
            // Fire repeats word 2 as an unrotated Y offset (GC 100dbedb/100db97d).
            position.y+=self.template.float(2)?;
            let direction=if self.template.word(0).unwrap_or(0)&0x100==0 {matrix.z_axis.truncate()}else{Vec3::Y};
            velocity=direction*sphere.y.abs()*self.template.float(13)?/life;
        }
        let mut color=[self.template.float(color_index+1)?,self.template.float(color_index+2)?,self.template.float(color_index+3)?,self.template.float(color_index)?];
        let mut stop=[self.template.float(color_index+5)?,self.template.float(color_index+6)?,self.template.float(color_index+7)?,self.template.float(color_index+4)?];
        if self.color!=0 {
            let [a,r,g,b]=self.color.to_be_bytes();
            color=[r as f32/255.0,g as f32/255.0,b as f32/255.0,a as f32/255.0];
            stop=[color[0],color[1],color[2],0.0];
        }
        if let Some(c)=self.config.start_color {color=c;}
        if let Some(c)=self.config.stop_color {stop=c;}
        let scale=if self.template.kind==1023 {1.0}else{self.config.scale.unwrap_or(1.0)};
        width*=scale; height*=scale; width_end*=scale; height_end*=scale;
        let &(_,_,_,first,last)=&materials::MATERIALS[self.template.word(9).unwrap_or(0) as usize];
        let mode=self.template.word(if self.template.kind==1018 {38}else if nano{37}else{29})? as i32;
        ensure!((0..=7).contains(&mode),"unknown native sprite wind mode {mode}");
        let low=height_base.unwrap_or(position.y)+self.template.float(if self.template.kind==1018 {36}else if nano{35}else{27})?;
        let high=height_base.unwrap_or(position.y)+self.template.float(if self.template.kind==1018 {37}else if nano{36}else{28})?;
        let factor=if nano||smoke {super::random_fraction(random)*0.9+0.1}else{1.0};
        let wind_scale=factor*self.template.float(if self.template.kind==1018 {39}else if nano{38}else{30})?;
        let previous_velocity=self.default_direction;
        self.particles.push(SpriteParticle {position,velocity,previous_velocity,remaining:life,width,height,
            width_rate:(width_end-width)/life,height_rate:(height_end-height)/life,
            color,color_rate:std::array::from_fn(|i|(stop[i]-color[i])/life),frame:first as f32,frame_rate:(last-first) as f32/life,
            mode,low,high,wind_scale});
        Ok(())
    }

    fn particle_vertices(&mut self,time:f32,right:Vec3,up:Vec3,random:&mut R250,display_random:&mut R250,crt:&mut CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        let dt=(time-self.previous_time).max(0.0);
        let nano=matches!(self.template.kind,1007|1008|1018|1023);
        let life=self.template.float(if nano{35}else{26})?;
        let duration=self.config.duration.unwrap_or(self.template.float(8)?);
        let emitting=duration<0.0 || time<duration-life;
        if matches!(self.template.kind,1007|1018) && self.previous_time==0.0 && self.particles.is_empty() && self.emitted==0 {
            for _ in 0..self.template.word(31)? {self.emit(self.emission_position(),random,crt)?;}
        }
        if self.template.kind==1008 {
            // Native Nano1 consumes the shared CRT mask draw but stages do not
            // depend on its result (GC 100e8421).
            crt.rand();
            ensure!(self.anchors.len()==NANO_ANCHORS.len(),"missing Nano1 skeleton anchors");
            if self.stage<5 && self.next_stage<time {
                let indices:&[usize]=match self.stage {0=>&[0],1=>&[1,2],2=>&[1,3],3=>&[4,5,6,7,8],_=>&[9,10,11,12,13]};
                for &i in indices {self.emit(self.anchors[i].w_axis.truncate(),random,crt)?;}
                self.next_stage+=0.01;
                let boundary=0.05*(self.stage+1) as f32;
                let transition=if matches!(self.stage,2|4) {self.next_stage>boundary}else{self.next_stage>=boundary};
                if transition {self.next_stage=boundary;self.stage+=1;}
            }
        } else {
            let mask=if nano{self.template.word(24)?}else if self.template.kind==1004{self.template.word(31)?}else{0};
            if emitting && (self.template.kind==1009 || (crt.rand() & mask)==0) {
                let count=(self.template.float(10)?*time) as i32;
                let old=self.emitted;
                // GC100e9855 consumes the ring-radius draw even with authored rate0.
                let radius=if self.template.kind==1023 {super::random_fraction(random)+2.0}else{0.0};
                for n in old..count {
                    let mut at=if matches!(self.template.kind,1007|1023) && self.template.word(0).unwrap_or(0)&2==0 {self.previous_source.lerp(self.emission_position(),(n-old+1) as f32/(count-old) as f32)}else{self.emission_position()};
                    if self.template.kind==1023 {
                        let angle=super::random_fraction(random)*f32::from_bits(0x40c8f5c3);
                        at+=Vec3::new(angle.cos()*radius,1.0,-angle.sin()*radius);
                    }
                    self.emit(at,random,crt)?;
                }
                self.emitted=count;
            }
        }
        let wind=if self.template.kind==1009 {self.smoke_wind}else{self.wind};
        for p in &mut self.particles {
            p.remaining-=dt;
            if p.remaining<0.0 {continue;}
            if p.mode==7 {p.velocity=p.previous_velocity*p.high;}
            else if p.mode!=0 {
                let phase=((p.position.y-p.low)/(p.high-p.low)).clamp(0.0,1.0);
                let strength=match p.mode {1=>1.0,2=>phase,3=>phase*phase,4=>(phase*phase+phase)*0.5,5=>1.0,_=>phase};
                if p.mode==5 {p.position.y+=dt*1.5;}
                if p.mode==6 {p.position.y+=(display_uniform(display_random)-0.5)*dt;p.position.x+=(display_uniform(display_random)-0.5)*dt;}
                p.position+=wind*strength*p.wind_scale*dt;
            }
            p.position+=p.velocity*dt;
            if p.mode==7 {p.previous_velocity=p.velocity.normalize_or_zero();}
            p.width+=p.width_rate*dt; p.height+=p.height_rate*dt;
            for i in 0..4 {p.color[i]+=p.color_rate[i]*dt;}
            p.frame+=p.frame_rate*dt;
        }
        self.particles.retain(|p|p.remaining>=0.0);
        self.previous_time=time;self.previous_source=self.source.w_axis.truncate();
        if self.particles.is_empty() && (!emitting || (nano && self.template.word(0).unwrap_or(0)&0x200!=0)) {return Ok(None);}
        let &(_,columns,rows,_,_)=&materials::MATERIALS[self.template.word(9).unwrap_or(0) as usize];
        let mut vertices=Vec::with_capacity(self.capacity*4);
        for p in &self.particles {
            let frame=p.frame as i32;
            let u=(frame%columns as i32) as f32/columns as f32;let v=(frame/rows as i32) as f32/rows as f32;
            let x=right*p.width*0.5;let y=up*p.height*0.5;
            let center=if self.template.word(0).unwrap_or(0)&2!=0 {self.source.transform_point3(p.position)}else{p.position};
            quad(&mut vertices,[center-x-y,center+x-y,center-x+y,center+x+y],[[u,v+1.0/rows as f32],[u+1.0/columns as f32,v+1.0/rows as f32],[u,v],[u+1.0/columns as f32,v]],render_color(p.color));
        }
        vertices.resize(self.capacity*4,Vertex::default());
        Ok(Some(vec![vertices]))
    }

    pub(super) fn models(&self) -> Vec<(Option<usize>, Vec<u32>, usize)> {
        let n=self.capacity;
        vec![(Some(self.template.word(9).unwrap_or(0) as usize),(0..n as u32).flat_map(|i|[i*4,i*4+1,i*4+2,i*4+1,i*4+2,i*4+3]).collect(),n*4)]
    }

    pub(super) fn blends(&self) -> Vec<Blend> {
        let flags = self.template.word(10).unwrap_or(0);
        vec![if self.template.kind==1009 { if self.effect==80005 {Blend::Additive} else {Blend::AlphaBlend} }
            else if matches!(self.template.kind,1007|1018|1023) && self.template.word(0).unwrap_or(0)&0x800!=0 {Blend::AlphaBlend}
            else if self.template.kind!=1012 {Blend::Additive}
            else if flags&3==0 {
                if flags&0x1000!=0 {Blend::Opaque}else if flags&4==0 {Blend::Additive}else{Blend::AlphaBlend}
            } else if flags&0x1000!=0 {Blend::DestinationColorSourceColor}
            else if flags&4==0 {Blend::Additive}else{Blend::ZeroSourceColor}]
    }

    pub(super) fn update_source(&mut self, source: Mat4) {
        if self.template.word(0).unwrap_or(0)&1==0 || (self.template.kind==1012 && self.template.word(10).unwrap_or(0)&0x10==0) {return;}
        self.source=source;
    }

    /// GC 100f67d6: period, repetition, atlas, colour and size flags are independent.
    // Keep the native camera inputs and three independent RNG streams explicit.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self, time: f32, camera: Vec3, right: Vec3, up: Vec3,random:&mut R250,display_random:&mut R250,crt:&mut CrtRand) -> Result<Option<Vec<Vec<Vertex>>>> {
        if self.template.kind!=1012 { return self.particle_vertices(time,right,up,random,display_random,crt); }
        let t = &self.template;
        let flags = t.word(10)?;
        let period = if t.float(23)? == 0.0 { 1.0 } else { t.float(23)? };
        // GC100f5f2e: selectors1/2 terminate natively; they are not missing planes.
        if matches!(flags&3,1|2) {return Ok(None);}
        ensure!(period > 0.0, "invalid sprite period");
        let cycle = (time / period) as i32;
        let repetitions=self.config.repetitions.map(|n|n as i32).unwrap_or(if flags&0x40!=0 {t.word(24)? as i32}else{0});
        if repetitions >= 0 && cycle > repetitions { return Ok(None); }
        let phase = time / period - cycle as f32;
        let sample = |a: usize, b: usize| -> Result<f32> { Ok(t.float(a)? + (t.float(b)?-t.float(a)?)*phase) };
        let mut width = t.float(11)?;
        let mut height = t.float(13)?;
        if flags & 0x100 != 0 {
            width = sample(11,12)?;
            height = sample(13,14)?;
            if flags & 0x800 != 0 {
                let pulse = (time * std::f32::consts::TAU * t.float(26)?).sin() * t.float(25)?;
                width += pulse;
                height += pulse;
            }
        }
        let scale=self.config.scale.unwrap_or(1.0);
        width*=scale; height*=scale;
        let authored = if flags & 0x80 != 0 {
            [sample(16,20)?,sample(17,21)?,sample(18,22)?,sample(15,19)?]
        } else { [t.float(16)?,t.float(17)?,t.float(18)?,t.float(15)?] };
        let mut color = if self.color == 0 { authored } else {
            let [a,r,g,b] = self.color.to_be_bytes();
            [r as f32/255.0,g as f32/255.0,b as f32/255.0,a as f32/255.0*(1.0-phase)]
        };
        if let Some(start)=self.config.start_color {
            color=if let Some(stop)=self.config.stop_color {std::array::from_fn(|i|start[i]+(stop[i]-start[i])*phase)}else{start};
        } else if let Some(stop)=self.config.stop_color {
            color=std::array::from_fn(|i|authored[i]+(stop[i]-authored[i])*phase);
        }
        let &(_,columns,rows,first,last) = &materials::MATERIALS[t.word(9)? as usize];
        let frame = if flags & 0x20 != 0 { (first as f32+(last-first) as f32*phase) as i32 } else { first as i32 };
        let u = (frame % columns as i32) as f32 / columns as f32;
        // Retail deliberately divides by row count, not column count (DS10023ea4/10028606).
        let v = (frame / rows as i32) as f32 / rows as f32;
        let du = 1.0/columns as f32;
        let dv = 1.0/rows as f32;
        let p = if t.word(0)?&2!=0 {self.source.w_axis.truncate()}else{Vec3::ZERO};
        let (x,y) = if flags & 3 == 0 { (right*width*0.5,up*height*0.5) }
            else if flags & 0x2000 != 0 {
                let delta = camera-p;
                let side=Vec3::new(delta.z,0.0,-delta.x);
                let side=if side.length_squared()<=0.001 {Vec3::X}else{side.normalize()};
                (side*width*0.5,Vec3::Y*height*0.5)
            } else if flags&8!=0 && t.word(0)?&2!=0 {
                (self.source.x_axis.truncate()*width*0.5,self.source.y_axis.truncate()*height*0.5)
            } else {(Vec3::X*width*0.5,Vec3::Y*height*0.5)};
        let mut vertices = Vec::with_capacity(4);
        quad(&mut vertices,[p-x-y,p+x-y,p-x+y,p+x+y],[[u,v+dv],[u+du,v+dv],[u,v],[u+du,v]],render_color(color));
        Ok(Some(vec![vertices]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absent_sprite_fields_remain_zero_through_frames() {
        let mut gc=R250::new(1);let mut ds=R250::new(1);let mut crt=CrtRand::new(1);
        for kind in [1004,1007,1008,1009,1012,1018,1023] {
            let t=Template {kind,words:vec![]};
            let mut e=SpriteEffect::new_with_id(1,&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut ds).unwrap();
            e.configure(EffectConfig::default());e.update_source(Mat4::from_translation(Vec3::ONE));
            assert_eq!(e.models()[0].0,Some(0));e.blends();
            assert_eq!(e.emission_position(),Vec3::ZERO);
            assert_eq!(e.emission_matrix().unwrap(),Mat4::IDENTITY);
            e.initialize_native(&mut gc,&mut crt).unwrap();
            let groups=e.vertices(0.0,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap();
            if kind==1012 {
                let vertices=groups.unwrap();assert!(vertices[0].iter().all(|v|v.pos==[0.0;3] && v.color==[0.0;4]));
            }
        }
    }
    fn template() -> Template {
        let mut words = vec![0;27];
        words[10] = 0x100 | 0x80 | 0x20;
        for (i,f) in [(11,2.0f32),(12,4.0),(13,4.0),(14,8.0),(15,1.0),(16,1.0),(20,0.0),(23,2.0)] { words[i]=f.to_bits(); }
        Template { kind:1012,words }
    }
    #[test]
    fn sprite_period_size_color_and_plane_are_authored() {
        let mut effect = SpriteEffect::new_with_id(1,&template(),Mat4::IDENTITY,Mat4::IDENTITY,0, &mut R250::new(0xe6f1)).unwrap();
        let groups = effect.vertices(1.0,Vec3::Z,Vec3::X,Vec3::Y, &mut R250::new(0xe6f1), &mut R250::new(0xe6f1), &mut CrtRand::new(1)).unwrap().unwrap();
        assert_eq!(groups[0][0].pos,[-1.5,-3.0,0.0]);
        assert_eq!(groups[0][0].color,[(127.0f32/255.0).powf(2.2),0.0,0.0,127.0/255.0]);
        assert!(effect.vertices(2.1,Vec3::Z,Vec3::X,Vec3::Y,&mut R250::new(0xe6f1),&mut R250::new(0xe6f1),&mut CrtRand::new(1)).unwrap().is_none());
        // GC 100f5f2e accepts selectors 1/2 but terminates without a visual.
        for selector in [1,2] {
            let mut t=template(); t.words[10]=selector;
            let mut effect=SpriteEffect::new_with_id(1,&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut R250::new(0xe6f1)).unwrap();
            assert!(effect.vertices(0.0,Vec3::Z,Vec3::X,Vec3::Y,&mut R250::new(0xe6f1),&mut R250::new(0xe6f1),&mut CrtRand::new(1)).unwrap().is_none());
        }
        let mut t=template(); t.words[11]=f32::NAN.to_bits();
        assert!(SpriteEffect::new_with_id(1,&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut R250::new(0xe6f1)).is_err());
    }
    #[test]
    fn native_family_blends_and_nano_burst_do_not_alias() {
        let mut words=vec![0;39];
        for (i,f) in [(8,-1.0f32),(10,4.0),(12,2.0),(13,4.0),(14,3.0),(15,5.0),(16,1.0),(17,1.0),(32,1.0),(34,1.0),(35,2.0)] {words[i]=f.to_bits();}
        words[31]=3;
        let t=Template{kind:1007,words};
        let mut nano=SpriteEffect::new_with_id(1,&t,Mat4::IDENTITY,Mat4::IDENTITY,0, &mut R250::new(0xe6f1)).unwrap();
        nano.configure(EffectConfig {scale:Some(2.0),..EffectConfig::default()});
        nano.vertices(0.0,Vec3::Z,Vec3::X,Vec3::Y, &mut R250::new(0xe6f1), &mut R250::new(0xe6f1), &mut CrtRand::new(1)).unwrap();
        assert_eq!(nano.particles.len(),3);
        assert_eq!(nano.particles[0].width,4.0);
        assert_eq!(nano.blends(),[Blend::Additive]);
        let mut smoke=t.clone(); smoke.kind=1009; smoke.words[26]=1.0f32.to_bits();smoke.words[31]=0;smoke.words[32]=0;
        let s=SpriteEffect::new_with_id(80005,&smoke,Mat4::IDENTITY,Mat4::IDENTITY,0, &mut R250::new(0xe6f1)).unwrap();
        assert_eq!(s.blends(),[Blend::Additive]);
        let s=SpriteEffect::new_with_id(80006,&smoke,Mat4::IDENTITY,Mat4::IDENTITY,0, &mut R250::new(0xe6f1)).unwrap();
        assert_eq!(s.blends(),[Blend::AlphaBlend]);
        assert!(!SpriteEffect::supports(1010));
        for _ in 0..32 {assert!((sphere_point(&mut CrtRand::new(1)).length()-1.0).abs()<1e-5);}
    }

    #[test]
    fn sparks_uses_native_unscaled_velocity_and_wind_word38() {
        let mut words=vec![0;41];
        for (i,f) in [(8,-1.0f32),(12,1.0),(13,1.0),(14,1.0),(15,1.0),(16,1.0),(17,1.0),(29,2.0),(30,2.0),(32,99.0),(34,1.0),(35,1.0)] {words[i]=f.to_bits();}
        words[31]=1;words[38]=0;words[37]=2.0f32.to_bits();
        let t=Template {kind:1018,words};
        let mut effect=SpriteEffect::new_with_id(2710,&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut R250::new(1)).unwrap();
        effect.vertices(0.0,Vec3::Z,Vec3::X,Vec3::Y,&mut R250::new(1),&mut R250::new(1),&mut CrtRand::new(1)).unwrap();
        assert_eq!(effect.particles.len(),1);
        assert_eq!(effect.particles[0].velocity,Vec3::X*2.0);
        assert_eq!(effect.particles[0].mode,0);
        assert_eq!(effect.particles[0].high,2.0);
    }

    #[test]
    fn connector_rotates_authored_offset_in_scene_coordinates() {
        let mut t=template();
        t.words[1]=1.0f32.to_bits(); t.words[3]=2.0f32.to_bits();
        t.words[6]=std::f32::consts::FRAC_PI_2.to_bits();
        let m=connector(&t,Mat4::from_translation(Vec3::X)).unwrap();
        assert!((m.w_axis.truncate()-Vec3::new(1.0,1.0,-2.0)).length()<1e-5);
    }

    #[test]
    fn fire_disk_and_nano1_stages_use_their_native_axes_and_bones() {
        let mut words=vec![0;39];
        for (i,f) in [(8,-1.0f32),(10,100.0),(11,1.0),(12,1.0),(13,1.0),(14,1.0),(16,1.0),(18,1.0),(19,1.0),(26,2.0)] {words[i]=f.to_bits();}
        let mut gc=R250::new(0xe6f1);let mut ds=R250::new(0xe6f1);let mut crt=CrtRand::new(1);
        let mut fire=SpriteEffect::new_with_id(1,&Template{kind:1004,words:words.clone()},Mat4::IDENTITY,Mat4::IDENTITY,0,&mut ds).unwrap();
        fire.emit(Vec3::ZERO,&mut gc,&mut crt).unwrap();
        assert_eq!(fire.particles[0].position.z,0.0);
        assert_eq!(fire.particles[0].velocity.x,0.0);
        assert_eq!(fire.particles[0].velocity.y,0.0);
        assert!(fire.particles[0].velocity.z>=0.0);
        words[26]=0;words[32]=1.0f32.to_bits();words[34]=1.0f32.to_bits();words[35]=2.0f32.to_bits();
        let mut nano=SpriteEffect::new_with_id(1,&Template{kind:1008,words},Mat4::IDENTITY,Mat4::IDENTITY,0,&mut ds).unwrap();
        let anchors=std::array::from_fn::<_,14,_>(|i|Mat4::from_translation(Vec3::X*(i+1) as f32));
        nano.update_anchors(&anchors).unwrap();
        nano.vertices(0.01,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap();
        assert_eq!(nano.particles.len(),1);
        assert_eq!(nano.particles[0].position,Vec3::X);
        assert_eq!(nano.source,Mat4::IDENTITY);
    }
}
