//! GC Highlight2011: load100e286a, process100e29a7, graceful100e283c,
//! cleanup100e2bde. Colour envelope10108089/101081a5; mode2 infinite=-1 at10155dd0.
use super::{EffectConfig, Template};
use anyhow::{ensure, Context, Result};

pub(super) struct Highlight {
    identity: (u32,u32),
    mode: u32,
    start: [f32;4],
    stop: [f32;4],
    ramp: f32,
    duration: f32,
    elapsed: f32,
    stopping: bool,
    actor: Option<u32>,
}
impl Highlight {
    pub(super) fn new(t:&Template,c:EffectConfig)->Result<Self> {
        let mode=t.word(1)?;
        ensure!(mode<=2,"Highlight mode3 requires separate held-attractor material overrides");
        let ramp=t.float(2)?;
        ensure!(ramp>0.0,"invalid Highlight envelope duration");
        let read=|at|->Result<[f32;4]> {Ok([t.float(at)?,t.float(at+1)?,t.float(at+2)?,t.float(at+3)?])};
        Ok(Self {identity:c.source_identity.context("Highlight requires source dynel")?,mode,
            start:read(3)?,stop:read(7)?,ramp,duration:c.duration.unwrap_or(if mode==2 {-1.0}else{ramp}),
            elapsed:0.0,stopping:false,actor:None})
    }
    pub(super) fn identity(&self)->(u32,u32) {self.identity}
    pub(super) fn prepare_actor(&mut self,id:u32) {self.actor=Some(id);}
    pub(super) fn terminate_gracefully(&mut self) {
        if self.mode==2 {self.duration=self.elapsed+self.ramp;}
        self.stopping=true;
    }
    pub(super) fn advance(&mut self,dt:f32)->bool {
        self.elapsed+=dt;
        self.duration<0.0 || self.elapsed<=self.duration
    }
    fn color(&self)->[f32;4] {
        let phase=match self.mode {
            0=>self.elapsed/self.duration,
            1=>{let p=self.elapsed/self.duration*2.0-1.0;1.0-p*p},
            _=>if self.stopping {(self.duration-self.elapsed)/self.ramp}else{(self.elapsed/self.ramp).min(1.0)},
        };
        std::array::from_fn(|i|self.start[i]+phase*(self.stop[i]-self.start[i]))
    }
    pub(super) fn apply(&self,actors:&mut[ao_scene::ActorFrame]) {
        let Some(actor)=self.actor.and_then(|id|actors.iter_mut().find(|a|a.id==id)) else {return};
        let c=self.color();
        actor.alpha=c[0];
        actor.emissive=Some([c[1].powf(2.2),c[2].powf(2.2),c[3].powf(2.2)]);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistent_highlight_ramps_then_gracefully_restores_start() {
        let mut t=Template {kind:2011,words:vec![0;12]};
        t.words[1]=2;t.words[2]=2.0f32.to_bits();t.words[3]=1.0f32.to_bits();
        for i in 7..=10 {t.words[i]=0.2f32.to_bits();}
        let mut h=Highlight::new(&t,EffectConfig {source_identity:Some((50000,1)),..Default::default()}).unwrap();
        h.advance(1.0);assert_eq!(h.color(),[0.6,0.1,0.1,0.1]);
        h.advance(1.0);assert_eq!(h.color(),[0.19999999,0.2,0.2,0.2]);
        h.terminate_gracefully();h.advance(1.0);assert_eq!(h.color(),[0.6,0.1,0.1,0.1]);
        h.advance(1.0);assert_eq!(h.color(),[1.0,0.0,0.0,0.0]);assert!(!h.advance(0.01));
    }
}
