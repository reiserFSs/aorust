//! Retail Sequencer (3004): parameters GC100ec81f, Process100ec640,
//! graceful termination100ec600. Shield3003 and SkyFlash3006 are distinct visuals.
use super::{Binding, EffectConfig, Renderer, Template};
use anyhow::{ensure, Result};
use glam::{Mat4, Vec3};
use ao_scene::{Blend, Vertex};

pub(super) fn packed_color(argb:u32)->[f32;4] {
    let [a,r,g,b]=argb.to_be_bytes();
    [(r as f32/255.0).powf(2.2),(g as f32/255.0).powf(2.2),(b as f32/255.0).powf(2.2),a as f32/255.0]
}
fn mix_color(a:u32,b:u32,t:f32)->[f32;4] {
    let a=a.to_be_bytes();let b=b.to_be_bytes();
    let bytes=std::array::from_fn(|i|((a[i] as f32*(1.0-t)+b[i] as f32*t) as u32&255) as u8);
    packed_color(u32::from_be_bytes(bytes))
}

// SkyFlash is a stack of animated frustum strips, not a screen flash.
// GC100eea7d/100eebec; DS1000c196/1000bd0a/1000c0ec.
pub(super) struct SkyFlash {
    template:Template, anchor:Mat4, current_source:Mat4, elapsed:f32,
    duration:f32, started:bool, terminated:bool,
}
impl SkyFlash {
    pub(super) fn new(t:&Template,source:Mat4)->Result<Self> {
        ensure!(t.kind==3006,"not a SkyFlash template");
        t.word(29)?;
        for i in (1..=6).chain([8,11,12]).chain(15..=23) {t.float(i)?;}
        ensure!(t.word(13)?<=1024 && (1..=32766).contains(&t.word(14)?),"invalid SkyFlash geometry count");
        ensure!(t.float(11)?+t.float(12)?>0.0,"invalid SkyFlash cycle");
        let anchor=super::sprites::connector(t,source)?;
        Ok(Self {template:t.clone(),anchor,current_source:anchor,elapsed:0.0,duration:t.float(8)?,started:false,terminated:false})
    }
    pub(super) fn configure(&mut self,c:EffectConfig) {if let Some(d)=c.duration {self.duration=d;}}
    pub(super) fn requires_terrain(&self)->bool {self.template.words[0]&0x4000!=0}
    pub(super) fn update_source(&mut self,source:Mat4)->Result<()> {self.current_source=super::sprites::connector(&self.template,source)?;Ok(())}
    pub(super) fn terminate_gracefully(&mut self) {self.terminated=true;}
    pub(super) fn frame(&mut self,dt:f32)->Result<bool> {
        ensure!(dt.is_finite() && dt>=0.0,"invalid SkyFlash timestep");
        if self.started {self.elapsed+=dt;} else {self.started=true;}
        Ok(self.alive())
    }
    pub(super) fn alive(&self)->bool {
        !self.terminated && !(self.duration>0.0 && self.elapsed>self.duration)
            && self.elapsed<(self.template.words[10] as f32)*(f32::from_bits(self.template.words[11])+f32::from_bits(self.template.words[12]))
    }
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let n=self.template.words[14];
        let indices=(0..n*2).flat_map(|i|if i&1==0 {[i,i+1,i+2]}else{[i+1,i,i+2]}).collect::<Vec<_>>();
        (0..self.template.words[13]).map(|_|(Some(self.template.words[9] as usize),indices.clone(),(n as usize+1)*2)).collect()
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.template.words[0]&0x200!=0 {Blend::Additive}else{Blend::AlphaBlend};self.template.words[13] as usize]}
    pub(super) fn vertices(&mut self,terrain:&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>)->Result<Option<Vec<Vec<Vertex>>>> {
        if !self.alive() {return Ok(None);}
        let t=&self.template;
        let flags=t.word(0)?;
        let rise=t.float(11)?;let fall=t.float(12)?;let cycle=rise+fall;
        let phase=self.elapsed%cycle;
        let mut bottom=t.float(18)?+t.float(22)?*(phase/cycle);
        let mut top=t.float(19)?+t.float(23)?*(phase/cycle);
        let mut height=t.float(17)?;
        let mut bottom_step=t.float(20)?;
        let (colors,track)=if phase>=rise {
            let f=(phase-rise)/fall;
            ([mix_color(t.word(26)?,t.word(28)?,f),mix_color(t.word(27)?,t.word(29)?,f)],flags&0x1000!=0)
        } else {
            let f=phase/rise;
            if flags&0x400!=0 {bottom=top*(1.0-f)+bottom*f;bottom_step=t.float(21)?*(1.0-f)+bottom_step*f;height*=f;}
            ([mix_color(t.word(24)?,t.word(26)?,f),mix_color(t.word(25)?,t.word(27)?,f)],flags&0x800!=0)
        };
        if track {self.anchor=self.current_source;}
        let mut position=self.anchor.w_axis.truncate();
        if flags&0x4000!=0 {
            if let Some((ground,_))=terrain(position) {
                if flags&0x8000!=0 {
                    height=position.y-ground.y;
                    if height<0.0 || height>t.float(17)? {height=0.0;}
                }
                position.y=ground.y;
            }
        }
        if flags&0x8000==0 {position.y+=t.float(17)?-height;}
        let segments=t.word(14)?;
        let mut out=Vec::with_capacity(t.word(13)? as usize);
        for cone in 0..t.word(13)? {
            let rotation=if flags&0x2000!=0 {std::f32::consts::TAU*cone as f32/t.word(13)? as f32}else{0.0};
            let mut vertices=Vec::with_capacity((segments as usize+1)*2);
            for i in 0..=segments {
                let angle=std::f32::consts::TAU*i as f32/segments as f32+rotation;
                for (end,radius,color) in [(0,bottom,colors[0]),(1,top,colors[1])] {
                    let uv=if flags&0x100!=0 {[end as f32*t.float(15)?,i as f32*t.float(16)?/segments as f32]}else{[i as f32*t.float(15)?/segments as f32,end as f32*t.float(16)?]};
                    vertices.push(Vertex {pos:(position+Vec3::new(angle.cos()*radius,end as f32*height,-angle.sin()*radius)).to_array(),normal:[0.0,0.0,1.0],uv,color});
                }
            }
            out.push(vertices);
            bottom+=bottom_step;top+=t.float(21)?;
        }
        Ok(Some(out))
    }
}

struct Entry { effect:i32, start:f32, end:f32, entered:bool, child:u32 }
pub(super) struct Sequencer {
    entries:Vec<Entry>, source:Mat4, target:Vec3, config:EffectConfig,
    flags:u32, elapsed:f32, cycle_start:f32, started:bool,
}
impl Sequencer {
    pub(super) fn new(t:&Template,source:Mat4,target:Vec3,config:EffectConfig)->Result<Self> {
        ensure!(t.kind==3004,"not a sequencer template");
        let count=t.word(1)? as usize;
        ensure!(count<=4096,"sequencer count exceeds bounded storage");
        // CMSBlock GetInt/GetFloat return zero for absent authored fields;
        // template12250 explicitly declares two entries but stores only one.
        let entries=(0..count).map(|i| {
            let word=|n|t.words.get(n).copied().unwrap_or(0);
            let start=f32::from_bits(word(3+i*3));let end=f32::from_bits(word(4+i*3));
            ensure!(start.is_finite() && end.is_finite(),"nonfinite sequencer entry");
            Ok(Entry {effect:word(2+i*3) as i32,start,end,entered:false,child:0})
        }).collect::<Result<Vec<_>>>()?;
        Ok(Self {entries,source,target,config,flags:t.word(0)?,elapsed:0.0,cycle_start:0.0,started:false})
    }
    // GC vtable100d25df rejects UpdateMatrix; retain the constructor anchor.
    pub(super) fn update_source(&mut self,_source:Mat4) {}
    pub(super) fn terminate_gracefully(&mut self,r:&mut Renderer) {
        self.flags&=!0x400;
        for entry in &self.entries {if entry.child!=0 {r.terminate_gracefully(entry.child);}}
    }
    pub(super) fn cancel(&mut self,r:&mut Renderer) {
        for entry in &mut self.entries {if entry.child!=0 {r.delete(entry.child);entry.child=0;}}
    }
    pub(super) fn frame(&mut self,dt:f32,r:&mut Renderer)->Result<bool> {
        ensure!(dt.is_finite() && dt>=0.0,"invalid sequencer timestep");
        if self.started {self.elapsed+=dt;} else {self.started=true;}
        let time=self.elapsed-self.cycle_start;
        let mut finished=true;
        for entry in &mut self.entries {
            if entry.child!=0 && !r.is_active(entry.child) {entry.child=0;}
            if entry.child==0 {
                if time<entry.start {finished=false;}
                else if !entry.entered && (time<entry.end || entry.end<entry.start) {
                    let config=EffectConfig {duration:(entry.end>0.0).then_some(entry.end-entry.start),source_identity:self.config.source_identity,source_attractor:self.config.source_attractor,source_appearance:self.config.source_appearance,track_source:self.config.track_source,..Default::default()};
                    entry.child=r.spawn_configured(Binding {group:0,attractor:0,effect:entry.effect,note:0,color:0},self.source,self.target,config)?;
                    entry.entered=true;
                }
            }
            if entry.child!=0 {finished=false;}
        }
        if finished && self.flags&0x400!=0 {
            for entry in &mut self.entries {entry.entered=false;}
            self.cycle_start=self.elapsed;
            return Ok(true);
        }
        Ok(!finished)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_sequence_layout_and_immutable_anchor() {
        let t=Template {kind:3004,words:vec![0x400,1,14,1.0f32.to_bits(),3.0f32.to_bits()]};
        let mut s=Sequencer::new(&t,Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).unwrap();
        assert_eq!((s.entries[0].effect,s.entries[0].start,s.entries[0].end),(14,1.0,3.0));
        s.update_source(Mat4::from_translation(Vec3::ONE));
        assert_eq!(s.source,Mat4::IDENTITY);
        let short=Sequencer::new(&Template {kind:3004,words:vec![0,2]},Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).unwrap();
        assert_eq!((short.entries[1].effect,short.entries[1].start,short.entries[1].end),(0,0.0,0.0));
    }
    #[test]
    fn skyflash_authored_frustum_and_cycle_lifetime() {
        let mut words=vec![0;30];words[8]=(-1.0f32).to_bits();words[9]=13;words[10]=1;words[13]=2;words[14]=4;
        for (i,v) in [(11,1.0f32),(12,1.0),(15,1.0),(16,1.0),(17,3.0),(18,1.0),(19,2.0),(20,0.5),(21,0.5)] {words[i]=v.to_bits();}
        for color in &mut words[24..30] {*color=u32::MAX;}
        let mut f=SkyFlash::new(&Template {kind:3006,words},Mat4::IDENTITY).unwrap();
        assert!(f.frame(0.0).unwrap());
        let vertices=f.vertices(&mut |_|None).unwrap().unwrap();
        assert_eq!(vertices.len(),2);
        assert_eq!(vertices[0][0].pos,[1.0,0.0,0.0]);
        assert_eq!(vertices[0][1].pos,[2.0,3.0,0.0]);
        assert_eq!(vertices[1][0].pos,[1.5,0.0,0.0]);
        assert_eq!(f.models()[0].1.len(),24);
        assert!(!f.frame(2.0).unwrap());
    }
}
