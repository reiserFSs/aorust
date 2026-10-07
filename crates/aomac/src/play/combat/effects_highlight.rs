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
    suppress_root: bool,
}
impl Highlight {
    pub(super) fn new(t:&Template,c:EffectConfig)->Result<Self> {
        let mode=t.word(1)?;
        ensure!(mode<=3,"invalid native Highlight mode");
        let ramp=t.float(2)?;
        ensure!(ramp>0.0,"invalid Highlight envelope duration");
        let read=|at|->Result<[f32;4]> {Ok([t.float(at)?,t.float(at+1)?,t.float(at+2)?,t.float(at+3)?])};
        Ok(Self {identity:c.source_identity.context("Highlight requires source dynel")?,mode,
            start:read(3)?,stop:read(7)?,ramp,duration:c.duration.unwrap_or(if mode==2 {-1.0}else{ramp}),
            elapsed:0.0,stopping:false,actor:None,suppress_root:t.word(0)?&0x400!=0})
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
            1|3=>{let p=self.elapsed/self.duration*2.0-1.0;1.0-p*p},
            _=>if self.stopping {(self.duration-self.elapsed)/self.ramp}else{(self.elapsed/self.ramp).min(1.0)},
        };
        std::array::from_fn(|i|self.start[i]+phase*(self.stop[i]-self.start[i]))
    }
    pub(super) fn apply(&self,actors:&mut[ao_scene::ActorFrame]) {
        let Some(actor)=self.actor.and_then(|id|actors.iter_mut().find(|a|a.id==id)) else {return};
        let c=self.color();
        if self.suppress_root {
            if actor.part_priorities.is_empty() {actor.part_priorities.push(Some(-1));}
            else {actor.part_priorities[0]=Some(-1);}
        }
        let rgb=[c[1].powf(2.2),c[2].powf(2.2),c[3].powf(2.2)];
        if self.mode!=3 {actor.alpha=c[0];actor.emissive=Some(rgb);}
        actor.part_materials.resize_with(actor.part_attractors.len(),Default::default);
        for (place,material) in actor.part_attractors.iter().zip(&mut actor.part_materials) {
            // GC100e29a7 excludes place0 (the head) only in mode3.
            if place.is_some_and(|place|self.mode!=3 || place!=0) {
                material.alpha=Some(c[0]);material.emissive=Some(rgb);
                if self.mode==3 {material.specular=Some(rgb);}
            }
        }
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
    #[test]
    fn held_highlight_changes_weapon_material_not_body_or_head() {
        let mut words=vec![0;12];words[1]=3;words[2]=2.0f32.to_bits();
        words[3]=1.0f32.to_bits();words[7]=0.5f32.to_bits();
        for word in &mut words[8..11] {*word=0.5f32.to_bits();}
        let mut h=Highlight::new(&Template {kind:2011,words},EffectConfig {source_identity:Some((50000,1)),..Default::default()}).unwrap();
        h.prepare_actor(1);h.advance(1.0);
        let mut actor=ao_scene::ActorFrame {id:1,part_attractors:vec![None,Some(0),Some(1)],..Default::default()};
        h.apply(std::slice::from_mut(&mut actor));
        assert_eq!(actor.alpha,1.0);assert!(actor.emissive.is_none());
        assert!(actor.part_materials[0].alpha.is_none());assert!(actor.part_materials[1].specular.is_none());
        assert_eq!(actor.part_materials[2].alpha,Some(0.5));
        assert_eq!(actor.part_materials[2].specular,Some([0.5f32.powf(2.2);3]));
    }
    #[test]
    fn authored_11507_held_specular_has_parabolic_envelope() {
        let t=Template {kind:2011,words:vec![3,3,1065353216,1065353216,0,0,0,1065353216,1065353216,1065353216,1065353216,0]};
        let mut h=Highlight::new(&t,EffectConfig {source_identity:Some((50000,1)),..Default::default()}).unwrap();
        assert_eq!(h.color(),[1.0,0.0,0.0,0.0]);h.advance(0.5);
        assert_eq!(h.color(),[1.0;4]);h.advance(0.5);
        assert_eq!(h.color(),[1.0,0.0,0.0,0.0]);assert!(!h.advance(0.001));
    }
    #[test]
    fn authored_61110_suppresses_only_root_visual_priority() {
        let t=Template {kind:2011,words:vec![1027,0,1077936128,0,0,0,0,0,0,0,0,0]};
        let mut h=Highlight::new(&t,EffectConfig {source_identity:Some((50000,1)),..Default::default()}).unwrap();
        h.prepare_actor(1);
        let mut actor=ao_scene::ActorFrame {id:1,part_attractors:vec![None,Some(0),Some(1)],..Default::default()};
        h.apply(std::slice::from_mut(&mut actor));
        assert_eq!(actor.priority,None);assert_eq!(actor.part_priorities,[Some(-1)]);
    }
    #[test]
    #[ignore="installed authored assets and offscreen GPU regression"]
    fn retail_highlight_held_authored_frames()->Result<()> {
        use ao_formats::character::{actor::{ActorAssets,ActorRig,PlayerLook},Breed,Gender,Skin,Equipment};
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").context("AOMAC_EFFECT_FRAMES required")?;
        let out=std::path::PathBuf::from(out);std::fs::create_dir_all(&out)?;
        let store=ao_rdb::RecordStore::open(&ao_gui::client_dir())?;
        let assets=ActorAssets::new(&store)?;
        let look=PlayerLook {breed:Breed::Solitus,gender:Gender::Male,skin:Skin::Caucasian,build:1,head:None,equipment:Equipment::default()};
        let rig=ActorRig::player(&store,&assets,&look,&[(1,15839)])?;
        let templates=super::super::Templates::open(&ao_gui::client_dir())?;
        for id in [11507,11508] {
            let t=templates.by_id.get(&id).context("missing authored held highlight")?;
            let mut h=Highlight::new(t,EffectConfig {source_identity:Some((50000,1)),..Default::default()})?;
            h.prepare_actor(1);
            for frame in 0..=60 {
                if frame>0 {h.advance(1.0/60.0);}
                let (skin,parts)=rig.pose(None);
                let mut actor=ao_scene::ActorFrame {id:1,model:1,skin:Some(skin),parts,part_attractors:rig.part_attractors(),..Default::default()};
                h.apply(std::slice::from_mut(&mut actor));
                assert!(actor.emissive.is_none());assert_eq!(actor.alpha,1.0);
                if [0,15,30,45,60].contains(&frame) {
                    ao_render::render_to_png_actors(&ao_scene::Scene::default(),&[(1,rig.model().clone())],vec![actor],[2.0,2.0,5.0],[0.0,1.0,0.0],640,480,&out.join(format!("highlight2011_{id}_{frame}.png")),frame as f32/60.0)?;
                }
            }
        }
        Ok(())
    }
}
