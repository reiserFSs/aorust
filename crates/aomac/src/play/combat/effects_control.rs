//! Retail controllers: class1001 is BPHFSM, not the nano-cast Spell1 class1010.
//! GC dispatch100d0102 ->100d3cce; parameters100d39ba; process100d3b0e.
//! Cast CreateEffect2 100d1de5 ->100d11b1 exclusively dispatches class1010.
use super::{Binding, EffectConfig, Renderer, Template};
use anyhow::{ensure, Context, Result};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};
use ao_formats::weather::R250;

pub(super) struct Controller {
    template: Template,
    config: EffectConfig,
    anchors: [Mat4; 3],
    anchor_ids: [i32;3],
    elapsed: f32,
    delay: f32,
    remaining: i32,
    registered: bool,
    spell: Option<Spell>,
    started: bool,
}

impl Controller {
    pub(super) fn new(template: &Template, source: Mat4, target: Vec3, config: EffectConfig) -> Result<Self> {
        ensure!(matches!(template.kind,1001|1010), "unsupported controller class {}", template.kind);
        ensure!(template.words.len() >= if template.kind==1001 {22} else {33}, "short controller template");
        ensure!(config.source_identity.is_some(), "controller requires a source dynel identity");
        for i in (1..=6).chain([8]).chain(if template.kind==1001 {12..=20} else {10..=26}) { template.float(i)?; }
        let remaining = config.repetitions.map(|n| n as i32).unwrap_or(if template.kind==1001 {template.word(21)? as i32} else {-1});
        let spell=if template.kind==1010 {Some(Spell::new(template)?)} else {None};
        let anchor_ids=if template.kind==1010 {
            let explicit=config.source_attractor.filter(|id| *id != 0);
            [explicit.unwrap_or(2001),explicit.unwrap_or(2000),1003]
        } else {[config.source_attractor.filter(|id| *id != 0).unwrap_or(template.word(7)? as i32),0,0]};
        Ok(Self { template: template.clone(), config, anchors: [source,source,Mat4::from_translation(target)], elapsed:0.0, delay:0.0, remaining, registered:false,spell,started:false,anchor_ids })
    }

    pub(super) fn anchor_ids(&self)->&[i32] {if self.spell.is_some() {&self.anchor_ids} else {&self.anchor_ids[..1]}}
    pub(super) fn update_anchors(&mut self, anchors: &[Mat4]) {for (to,from) in self.anchors.iter_mut().zip(anchors) {*to = *from;}}
    pub(super) fn update_source(&mut self, source: Mat4) { self.anchors[0]=source; }
    pub(super) fn update_target(&mut self, target: Vec3) { self.anchors[2]=Mat4::from_translation(target); }

    pub(super) fn next_state(&mut self) {
        if let Some(spell)=&mut self.spell { spell.next_state(self.elapsed); }
    }

    pub(super) fn cancel(&mut self, renderer: &mut Renderer) {
        // BPHFSM100d385e/100d3a93 stops only the controller: emitted handles
        // are not retained by100d3b0e and finish their authored lifetimes.
        if let Some(spell)=&mut self.spell { spell.cancel(renderer); }
        if !self.registered { return; }
        let key=self.config.source_identity.unwrap();
        if std::env::var_os("AOMAC_COMBAT_LOG").is_some() { eprintln!("BPHFSM cancel source={key:?} elapsed={} clock={} remaining={}",self.elapsed,renderer.elapsed,self.remaining); }
        if let Some((count,_))=renderer.bph_last.get_mut(&key) {
            *count-=1;
            if *count==0 { renderer.bph_last.remove(&key); }
        }
        self.registered=false;
    }

    pub(super) fn frame(&mut self, dt: f32, renderer: &mut Renderer) -> Result<bool> {
        ensure!(dt.is_finite() && dt>=0.0, "invalid controller frame time");
        if self.started { self.elapsed+=dt; } else { self.started=true; }
        let duration=self.config.duration.unwrap_or(if self.spell.is_some() {60.0} else {self.template.float(8)?});
        if duration>0.0 && self.elapsed>duration { self.cancel(renderer); return Ok(false); }
        if let Some(spell)=&mut self.spell {
            return spell.frame(self.elapsed,dt,self.anchors,self.config,renderer);
        }
        let key=self.config.source_identity.context("missing BPHFSM source")?;
        if !self.registered {
            renderer.bph_last.entry(key).and_modify(|v|v.0+=1).or_insert((1,0.0));
            self.registered=true;
            if std::env::var_os("AOMAC_COMBAT_LOG").is_some() { eprintln!("BPHFSM register source={key:?} duration={duration} clock={} throttle={:?} remaining={} children={:?} origin={:?} appearance={:?}",renderer.elapsed,renderer.bph_last[&key],self.remaining,&self.template.words[10..12],self.anchors[0].w_axis.truncate(),self.config.source_appearance); }
        }
        self.delay-=dt;
        if self.delay>0.0 { return Ok(true); }
        let last=renderer.bph_last[&key].1;
        // GC doubles101601f8=5,1016b338=1/16384, narrowed to float by x87 stores.
        if renderer.elapsed<=last+5.0 {
            self.delay=renderer.rng.rand() as f32/16384.0;
            return Ok(true);
        }
        renderer.bph_last.get_mut(&key).unwrap().1=renderer.elapsed;
        if std::env::var_os("AOMAC_COMBAT_LOG").is_some() { eprintln!("BPHFSM pulse source={key:?} elapsed={} clock={} remaining={} children={:?} origin={:?} appearance={:?}",self.elapsed,renderer.elapsed,self.remaining,&self.template.words[10..12],self.anchors[0].w_axis.truncate(),self.config.source_appearance); }
        self.delay=self.template.float(20)?;
        if self.remaining!=0 {
            let argb=|start:usize| -> Result<[f32;4]> { Ok([self.template.float(start+1)?,self.template.float(start+2)?,self.template.float(start+3)?,self.template.float(start)?]) };
            let config=EffectConfig { creation:super::Creation::Dynel,track_source:true,start_color:Some(self.config.start_color.unwrap_or(argb(12)?)), stop_color:Some(self.config.stop_color.unwrap_or(argb(16)?)),source_identity:self.config.source_identity,source_appearance:self.config.source_appearance,..EffectConfig::default() };
            for i in [10,11] {
                let effect=self.template.word(i)? as i32;
                if effect!=0 {
                    renderer.spawn_configured(Binding {group:0,attractor:0,effect,note:0,color:0},self.anchors[0],self.anchors[0].w_axis.truncate(),config)?;
                }
            }
        }
        if self.remaining>0 {
            self.remaining-=1;
            if self.remaining==0 { self.cancel(renderer); return Ok(false); }
        }
        Ok(true)
    }

    pub(super) fn models(&self) -> Vec<(Option<usize>,Vec<u32>,usize)> {
        if self.spell.is_none() {return Vec::new();}
        vec![(Some(self.template.words.get(9).copied().unwrap_or(0) as usize),(0..100u32).flat_map(|i|[i*4,i*4+2,i*4+1,i*4+1,i*4+2,i*4+3]).collect(),400)]
    }
    pub(super) fn blends(&self) -> Vec<Blend> { if self.spell.is_some() {vec![Blend::Additive]} else {Vec::new()} }
    pub(super) fn vertices(&mut self, _time:f32, _camera:Vec3, right:Vec3, up:Vec3) -> Result<Option<Vec<Vec<Vertex>>>> {
        if let Some(spell)=&self.spell {
            let mut vertices=Vec::with_capacity(spell.sprites.len()*4);
            for &(position,size,color) in &spell.sprites {
                for (x,y,uv) in [(-1.0,-1.0,[0.0,1.0]),(1.0,-1.0,[1.0,1.0]),(-1.0,1.0,[0.0,0.0]),(1.0,1.0,[1.0,0.0])] {
                    vertices.push(Vertex {pos:(position+right*(x*size*0.5)+up*(y*size*0.5)).to_array(),normal:[0.0,0.0,1.0],uv,color});
                }
            }
            vertices.resize(400,Vertex {pos:[0.0;3],normal:[0.0;3],uv:[0.0;2],color:[0.0;4]});
            return Ok(Some(vec![vertices]));
        }
        Ok(Some(Vec::new()))
    }
}

// GC Spell1 phase helpers100f2acd/100f3992/100f3944/100f2c5c.
// Native sprites100f2cb0; DS10027df8/10028206: current-frame additive billboards.
struct Spell {
    times: [f32;9],
    ids: [i32;6],
    colors: [[f32;4];2],
    mode: u32,
    entered: [bool;4],
    ended: [bool;4],
    terminated: bool,
    children: [u32;6],
    projectile: Vec3,
    speed: f32,
    sprites: Vec<(Vec3,f32,[f32;4])>,
}

impl Spell {
    fn new(t:&Template)->Result<Self> {
        let mut times=[0.0;9];
        for (i,v) in times.iter_mut().enumerate() { *v=t.float(18+i)?; }
        let mut ids=[0;6];
        for (i,v) in ids.iter_mut().enumerate() { *v=t.word(27+i)? as i32; }
        let color=|i| -> Result<[f32;4]> {Ok([t.float(i+1)?,t.float(i+2)?,t.float(i+3)?,t.float(i)?])};
        // CMSBlock::GetInt10106872 returns zero for missing optional mode word33.
        Ok(Self {times,ids,colors:[color(10)?,color(14)?],mode:t.words.get(33).copied().unwrap_or(0),entered:[false;4],ended:[false;4],terminated:false,children:[0;6],projectile:Vec3::ZERO,speed:0.0,sprites:Vec::with_capacity(100)})
    }

    // GC100f20c2, constants10155f00=.5,1016cba0=.35.
    fn next_state(&mut self,t:f32) {
        if self.mode==0 && t<self.times[2] {
            self.times[2]=self.times[1]+t-self.times[2];
            self.times[1]=t;
            self.times[3]=t;
            self.times[4]=t+0.5;
            self.times[5]=t+0.35;
        }
    }

    fn cancel(&mut self,r:&mut Renderer) {
        for child in &mut self.children {if *child!=0 {r.delete(*child);*child=0;}}
        self.sprites.clear();
    }

    fn child(&self,index:usize,position:Vec3,identity:Option<(u32,u32)>,config:EffectConfig,r:&mut Renderer)->Result<u32> {
        // GC100f2495/100f280e use Vector; GC100f290c uses the target Dynel.
        let config=EffectConfig {creation:if identity.is_some() {super::Creation::Dynel} else {super::Creation::Vector},track_source:identity.is_some(),start_color:Some(config.start_color.unwrap_or(self.colors[0])),stop_color:Some(config.stop_color.unwrap_or(self.colors[1])),source_identity:identity,source_appearance:if identity.is_some() {config.target_appearance} else {None},..EffectConfig::default()};
        r.spawn_configured(Binding {group:0,attractor:0,effect:self.ids[index],note:0,color:0},Mat4::from_translation(position),position,config)
    }

    fn frame(&mut self,t:f32,dt:f32,anchors:[Mat4;3],config:EffectConfig,r:&mut Renderer)->Result<bool> {
        let a=anchors[0].w_axis.truncate();
        let b=anchors[1].w_axis.truncate();
        let target=anchors[2].w_axis.truncate();
        self.sprites.clear();
        if t==0.0 {self.speed=(target-(a+b)*0.5).length();}
        if t>=self.times[0] {
            if t<=self.times[2] {
                if !self.entered[0] {
                    self.entered[0]=true;
                    for (child,id,position) in [(0,2,a),(1,3,b),(2,0,a),(3,1,b)] {
                        self.children[child]=self.child(id,position,None,config,r)?;
                    }
                } else {
                    if t>=self.times[1] && !self.terminated {
                        self.terminated=true;
                        for &child in &self.children[..4] {r.terminate_gracefully(child);}
                    }
                    for (i,position) in [a,b,a,b].into_iter().enumerate() {r.update_source(self.children[i],Mat4::from_translation(position));}
                }
            } else if !self.ended[0] {
                self.ended[0]=true;
                for child in &mut self.children[..4] {r.delete(*child);*child=0;}
            }
        }
        if t>=self.times[3] && t<=self.times[4] {
            if !self.entered[1] {self.entered[1]=true;} else {
                let length=self.times[4]-self.times[3];
                let since=t-self.times[3];
                let color=config.start_color.unwrap_or(self.colors[0]).map(|v|(v*255.0).clamp(0.0,255.0).trunc()/255.0);
                if length>0.0 && since<length {
                    if since<length*0.5 {
                        let q=since/length;
                        self.strip(a,b,0.0,q,0.75-q,0.25,color,&mut r.random);
                        self.strip(a,b,1.0,1.0-q,0.75-q,0.25,color,&mut r.random);
                    } else {
                        let q=(since-length*0.5)/(length*0.5);
                        self.strip(a,b,q*0.5,0.5,0.25,q*0.75+0.25,color,&mut r.random);
                        self.strip(a,b,1.0-q*0.5,0.5,0.25,q*0.75+0.25,color,&mut r.random);
                    }
                }
            }
        }
        if self.mode!=0 {return Ok(true);}
        if t>=self.times[5] {
            if t<=self.times[6] {
                if !self.entered[2] {
                    self.entered[2]=true;
                    self.projectile=(a+b)*0.5;
                    self.children[4]=self.child(4,self.projectile,None,config,r)?;
                } else {
                    let delta=target-self.projectile;
                    let distance=delta.length();
                    if distance<self.speed*dt {
                        self.times[6]=t;
                        self.times[8]=self.times[8]+t-self.times[7];
                        self.times[7]=t;
                    }
                    self.speed=self.speed.max((5.0*distance).min(10.0));
                    self.projectile+=delta.normalize_or_zero()*dt*self.speed;
                    r.update_source(self.children[4],Mat4::from_translation(self.projectile));
                }
            } else if !self.ended[2] {self.ended[2]=true;r.delete(self.children[4]);self.children[4]=0;}
        }
        if t>=self.times[7] {
            if t<=self.times[8] {
                if !self.entered[3] {
                    self.entered[3]=true;
                    self.children[5]=self.child(5,target,config.target_identity,config,r)?;
                }
            } else if !self.ended[3] {self.ended[3]=true;r.delete(self.children[5]);self.children[5]=0;}
        }
        Ok(true)
    }

    #[allow(clippy::too_many_arguments)]
    fn strip(&mut self,a:Vec3,b:Vec3,start:f32,end:f32,start_size:f32,end_size:f32,color:[f32;4],rng:&mut R250) {
        let n=(((b-a).length()*(end-start)).abs()*50.0).trunc().min(50.0) as usize;
        for i in 0..n {
            let fraction=if n==1 {0.0} else {i as f32/(n-1) as f32};
            // GC1013d97c uses the process-wide R250, not the CRT emission-gate stream.
            let jitter=(rng.next_f64()*0.02-0.01) as f32;
            self.sprites.push((a+(b-a)*(start+(end-start)*fraction+jitter),start_size+(end_size-start_size)*fraction,color));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bphfsm_is_not_a_cast_alias() {
        let t=Template {kind:1010,words:vec![0;34]};
        assert!(Controller::new(&t,Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).is_err());
        let t=Template {kind:1001,words:vec![0;22]};
        assert!(Controller::new(&t,Mat4::IDENTITY,Vec3::ZERO,EffectConfig::default()).is_err());
    }
    #[test]
    fn body_boost_cancel_preserves_emitted_children() {
        let dir=ao_gui::client_dir();
        if !dir.join("Setupf/gfxtweak.bin").exists() || !dir.join("cd_image/rdb.db").exists() {return;}
        let mut renderer=Renderer::open(&dir).unwrap();
        let template=renderer.templates.by_id[&1070].clone();
        assert_eq!(template.kind,1001);
        assert_eq!([template.word(10).unwrap(),template.word(11).unwrap()],[20091,20096]);
        assert_eq!(template.float(8).unwrap(),-1.0);
        for id in [20091,20096] {
            assert_eq!(renderer.templates.by_id[&id].float(8).unwrap(),1.5);
        }
        let identity=(50000,1);
        let origin=Vec3::new(930.0051,24.21451,-759.66864);
        // Synthetic connector for this ownership-only regression; the captured
        // Zone regression resolves the actual avatar's authored Spine3 matrix.
        renderer.prepare_anchor(identity,1004,Some(Mat4::from_translation(origin)));
        let handle=renderer.spawn_configured(Binding {group:0,attractor:0,effect:1070,note:0,color:0},Mat4::from_translation(origin),origin,EffectConfig {source_identity:Some(identity),source_appearance:Some([1,2,1,100]),..EffectConfig::default()}).unwrap();
        renderer.elapsed=6.0;
        let mut host=ao_render::Host::headless();
        host.camera=ao_render::Camera::look_at(origin+Vec3::new(2.0,3.0,4.0),origin+Vec3::Y);
        renderer.frame(0.0,&mut host,None);
        let children:Vec<_>=renderer.active.iter().filter(|a|matches!(a.effect,20091|20096)).map(|a|a.actor).collect();
        assert_eq!(children.len(),2);
        renderer.terminate_gracefully(handle);
        assert!(!renderer.is_active(handle),"buff removal must retire the periodic controller immediately");
        assert!(!renderer.bph_last.contains_key(&identity));
        assert!(children.iter().all(|handle|renderer.active.iter().any(|a|a.actor==*handle)));
        renderer.terminate_gracefully(handle);
        assert!(children.iter().all(|handle|renderer.active.iter().any(|a|a.actor==*handle)));
        let mut visible=false;
        for _ in 0..120 {
            host.actors.clear();
            renderer.frame(1.0/60.0,&mut host,None);
            visible|=host.actors.iter().any(|a|a.skin.as_ref().is_some_and(|vertices|vertices.iter().any(|v|v.color[3]>0.0)));
        }
        assert!(visible,"an emitted Body Boost pulse must submit nontransparent geometry before orbit expiry");
        assert!(renderer.active.is_empty(),"orbit expiry must delete the owned flare and cord, not leave a twenty-second flare");
        assert!(host.actors.is_empty(),"expired pulse must no longer submit visual actors");
        for _ in 0..360 {
            host.actors.clear();
            renderer.frame(1.0/60.0,&mut host,None);
        }
        assert!(renderer.active.is_empty(),"cancelled controller must not emit another five-second pulse");
    }
    #[test]
    fn cast_hands_and_target_refresh_independently() {
        let config=EffectConfig {source_identity:Some((50000,1)),target_identity:Some((50000,2)),..EffectConfig::default()};
        let mut controller=Controller::new(&Template {kind:1010,words:vec![0;34]},Mat4::IDENTITY,Vec3::Z,config).unwrap();
        assert_eq!(controller.anchor_ids(),&[2001,2000,1003]);
        controller.update_anchors(&[Mat4::from_translation(Vec3::X),Mat4::from_translation(Vec3::Y)]);
        assert_eq!(controller.anchors[2].w_axis.truncate(),Vec3::Z);
        controller.update_target(Vec3::Z*2.0);
        assert_eq!(controller.anchors.map(|m|m.w_axis.truncate()),[Vec3::X,Vec3::Y,Vec3::Z*2.0]);
        let config=EffectConfig {source_identity:Some((50000,1)),source_attractor:Some(1007),..EffectConfig::default()};
        let controller=Controller::new(&Template {kind:1010,words:vec![0;34]},Mat4::IDENTITY,Vec3::Z,config).unwrap();
        assert_eq!(controller.anchor_ids(),&[1007,1007,1003]);
        assert_eq!(controller.anchors[2].w_axis.truncate(),Vec3::Z);
    }
    #[test]
    fn release_retimes_only_retail_cast_mode() {
        let mut words=vec![0;34];
        for (i,v) in [0.0f32,8.0,6.0,50.3,56.0,56.0,61.0,61.0,70.0].into_iter().enumerate() {words[18+i]=v.to_bits();}
        let mut spell=Spell::new(&Template {kind:1010,words}).unwrap();
        spell.next_state(2.0);
        assert_eq!(spell.times,[0.0,2.0,4.0,2.0,2.5,2.35,61.0,61.0,70.0]);
        spell.mode=1;
        let before=spell.times;
        spell.next_state(3.0);
        assert_eq!(spell.times,before);
    }
    #[test]
    fn native_strip_caps_each_half_at_fifty_sprites() {
        let mut spell=Spell::new(&Template {kind:1010,words:vec![0;34]}).unwrap();
        spell.strip(Vec3::ZERO,Vec3::X*10.0,0.0,0.5,0.25,0.75,[1.0;4],&mut R250::new(0xe6f1));
        assert_eq!(spell.sprites.len(),50);
        assert_eq!(spell.sprites.first().unwrap().1,0.25);
        assert!((spell.sprites.last().unwrap().1-0.75).abs()<0.00001);
        assert!(spell.sprites.iter().all(|s|s.0.is_finite()));
        spell.sprites.clear();
        spell.strip(Vec3::ZERO,Vec3::X*0.025,0.0,1.0,0.25,0.75,[1.0;4],&mut R250::new(0xe6f1));
        assert_eq!(spell.sprites.len(),1);
        assert!(spell.sprites[0].0.is_finite());
        assert_eq!(spell.sprites[0].1,0.25);
        assert_eq!(Spell::new(&Template {kind:1010,words:vec![0;33]}).unwrap().mode,0);
    }
}
