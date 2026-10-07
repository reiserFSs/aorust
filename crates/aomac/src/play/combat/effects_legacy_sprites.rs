//! Legacy visuals: Drips GC ctor100d8c5b/load100d8a2f/init100d8aec/process100d8661.
use super::{materials, quad, EffectConfig, Template};
use anyhow::{ensure, Result};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Default)]
struct Drop { position: Vec3, render_position: Vec3, born: f32, velocity: f32, floor: f32, state: u8, frame: i32 }
pub(super) struct Drips {
    template: Template,
    source: Mat4,
    drops: [Drop; 8],
    elapsed: f32,
    last_spawn: f32,
    stop: Option<f32>,
    _terminating: bool,
}
impl Drips {
    pub fn new(template: &Template, source: Mat4) -> Result<Self> {
        ensure!(template.kind == 2009, "invalid Drips template");
        for i in 11..25 { template.float(i)?; }
        ensure!(template.float(23)? > 0.0 && template.float(24)? > 0.0, "invalid Drips animation period");
        ensure!((template.word(9)? as usize) < materials::MATERIALS.len(), "unknown Drips material");
        let duration = template.float(21)?;
        let source = super::sprites::connector(template,source)?;
        Ok(Self { template: template.clone(), source, drops: [Drop::default();8], elapsed: 0.0,
            last_spawn: 0.0, stop: (duration >= 0.0).then_some(duration), _terminating: false })
    }
    pub fn configure(&mut self, config: EffectConfig) { if let Some(d) = config.duration { self.stop = (d >= 0.0).then_some(d); } }
    pub fn update_source(&mut self, source: Mat4) -> Result<()> { if self.template.word(0).unwrap_or(0) & 1 != 0 { self.source = super::sprites::connector(&self.template,source)?; } Ok(()) }
    pub fn terminate_gracefully(&mut self) { self._terminating = true; }
    pub fn material(&self) -> usize { self.template.word(9).unwrap_or(0) as usize }
    pub fn blend(&self) -> Blend { Blend::AlphaBlend }
    /// Native acceleration is per Process, not dt-scaled (100d884a).
    pub fn advance(&mut self, dt: f32, valid: bool, mut ground: impl FnMut(Vec3)->f32) -> Result<bool> {
        self.elapsed += dt;
        if !valid { self.terminate_gracefully(); }
        let t = &self.template;
        let mut spawn = self.elapsed > self.last_spawn + t.float(22)?;
        // GC100d8661 extends the duration by five each expiry while DiaBill exists.
        // Graceful100d8902 only writes +120; Process does not read that byte.
        if let Some(stop) = &mut self.stop { if self.elapsed > *stop { self._terminating = true; *stop += 5.0; } }
        for drop in &mut self.drops {
            match drop.state {
                0 if spawn => { drop.position = self.source.w_axis.truncate(); drop.born = self.elapsed;
                    drop.state = 1; drop.frame = 0; self.last_spawn = self.elapsed; spawn = false; }
                0 => continue,
                _ => {}
            }
            if drop.state == 1 {
                let phase = (self.elapsed-drop.born)/t.float(23)?;
                if phase >= 1.0 { drop.frame = 5; drop.velocity = 0.0;
                    drop.floor = ground(drop.position)+t.float(12)?*0.5; drop.state = 2;
                } else { drop.position = self.source.w_axis.truncate(); drop.frame = (phase*5.0) as i32; }
            }
            drop.render_position = drop.position;
            if drop.state == 2 {
                if drop.position.y <= drop.floor { drop.position.y = drop.floor; drop.render_position = drop.position; drop.born = self.elapsed; drop.state = 3; }
                else { drop.velocity -= 0.01; drop.position.y += drop.velocity; }
            }
            if drop.state == 3 { let phase = (self.elapsed-drop.born)/t.float(24)?;
                if phase >= 1.0 { drop.state = 0; } else { drop.frame = 5-(phase*5.0) as i32; }
            }
        }
        Ok(true)
    }
    pub fn vertices(&self, right: Vec3, up: Vec3) -> Result<Vec<Vertex>> {
        let t = &self.template;
        let (_,columns,rows,_,_) = materials::MATERIALS[self.material()];
        let mut out = Vec::with_capacity(32);
        // Init100d8aec copies the gradient's start once; later color setters do not recolor sprites.
        let color = [t.float(14)?,t.float(15)?,t.float(16)?,t.float(13)?];
        let color = super::sprites::render_color(color);
        for drop in self.drops.iter().filter(|d|d.state != 0) {
            let x = right*t.float(12)?*0.5; let y = up*t.float(12)?*0.5;
            let p = drop.render_position; let frame = drop.frame;
            let u = (frame%columns as i32) as f32/columns as f32;
            let v = (frame/columns as i32) as f32/rows as f32;
            let du = 1.0/columns as f32; let dv = 1.0/rows as f32;
            quad(&mut out,[p-x-y,p+x-y,p-x+y,p+x+y],[[u,v+dv],[u+du,v+dv],[u,v],[u+du,v]],color);
        }
        out.resize(32,Vertex::default());
        Ok(out)
    }
}

/// GC Hexagram100e2781/100e26ed, load100e1ed9/init100e2590/process100e2069.
pub(super) struct Hexagram { template: Template, source: Mat4, anchor: Vec3, elapsed: f32, _duration: f32, terminated: bool }
impl Hexagram {
    pub fn new(t: &Template, source: Mat4, mut ground: impl FnMut(Vec3)->f32) -> Result<Self> {
        ensure!(t.kind == 2008, "invalid Hexagram template");
        for i in [8,11,12,15,16,17,18,19,20,21,22,23,30,31] { t.float(i)?; }
        ensure!(t.word(10)? > 0 && t.word(10)? <= 1024 && t.word(13)? <= 1024 && t.word(14)? >= 1, "invalid Hexagram geometry");
        let source = super::sprites::connector(t,source)?;
        let mut anchor = source.w_axis.truncate();
        if t.word(0).unwrap_or(0)&0x4000 != 0 { anchor.y = ground(anchor); }
        Ok(Self {template:t.clone(),source,anchor,elapsed:0.0,_duration:t.float(8)?,terminated:false})
    }
    pub fn configure(&mut self, c: EffectConfig) { if let Some(d) = c.duration { self._duration = d; } }
    pub fn update_source(&mut self, source: Mat4) -> Result<()> { self.source = super::sprites::connector(&self.template,source)?; Ok(()) }
    pub fn terminate_gracefully(&mut self) { self.terminated = true; }
    pub fn frame(&mut self, dt: f32) -> bool {
        self.elapsed += dt;
        // Process100e2069 overrides the base duration-end byte while a ring remains.
        !self.terminated && self.elapsed < f32::from_bits(self.template.word(11).unwrap_or(0))+f32::from_bits(self.template.word(12).unwrap_or(0))+self.template.word(10).unwrap_or(0) as f32*f32::from_bits(self.template.word(31).unwrap_or(0))
    }
    pub fn models(&self) -> Vec<(Option<usize>,Vec<u32>,usize)> {
        let n = self.template.word(14).unwrap_or(0);
        let indices = (0..n*2).flat_map(|i|if i&1 == 0 {[i,i+1,i+2]}else{[i+1,i,i+2]}).collect::<Vec<_>>();
        (0..self.template.word(10).unwrap_or(0)*self.template.word(13).unwrap_or(0)).map(|_|(Some(self.template.word(9).unwrap_or(0) as usize),indices.clone(),(n as usize+1)*2)).collect()
    }
    pub fn blends(&self) -> Vec<Blend> { vec![if self.template.word(0).unwrap_or(0)&0x200 != 0 {Blend::Additive}else{Blend::AlphaBlend}; (self.template.word(10).unwrap_or(0)*self.template.word(13).unwrap_or(0)) as usize] }
    pub fn vertices(&mut self, valid: bool, mut ground: impl FnMut(Vec3)->f32) -> Result<Option<Vec<Vec<Vertex>>>> {
        let t = &self.template; let flags = t.word(0)?;
        let rise = t.float(11)?; let fall = t.float(12)?;
        let total = rise+fall+t.word(10)? as f32*t.float(31)?;
        let mut out = Vec::with_capacity((t.word(10)?*t.word(13)?) as usize);
        for ring in 0..t.word(10)? {
            let mut time = self.elapsed-ring as f32*t.float(31)?;
            if time < 0.0 { time = total+1.0; }
            let mut bottom = t.float(18)?+t.float(22)?*(time/total);
            let mut top = t.float(19)?+t.float(23)?*(time/total);
            let mut bottom_step = t.float(20)?; let top_step = t.float(21)?;
            let mut height = t.float(17)?;
            let (colors,track) = if time >= rise+fall {
                bottom=0.1;top=0.1;bottom_step=0.0;height=0.0;
                ([[0.0;4];2],false)
            } else if time >= rise {
                let phase=(time-rise)/fall;
                ([mix_packed(t.word(26)?,t.word(28)?,phase),mix_packed(t.word(27)?,t.word(29)?,phase)],flags&0x1000!=0)
            } else {
                let phase=time/rise;
                if flags&0x400!=0 {bottom=top*(1.0-phase)+bottom*phase;bottom_step=top_step*(1.0-phase)+bottom_step*phase;height*=phase;}
                ([mix_packed(t.word(24)?,t.word(26)?,phase),mix_packed(t.word(25)?,t.word(27)?,phase)],flags&0x800!=0)
            };
            if track { if !valid { return Ok(None); } self.anchor=self.source.w_axis.truncate(); }
            let angle=self.elapsed*0.2+ring as f32*f32::from_bits(0x40c8f5c3)/t.word(10)? as f32;
            let mut p=self.anchor+Vec3::new(angle.sin()*t.float(30)?,0.0,-angle.cos()*t.float(30)?);
            p.y=ground(self.anchor);
            if height==0.0 {height=0.1;p.y-=1.0;}
            for cone in 0..t.word(13)? {
                let rotation=if flags&0x2000!=0 {-f32::from_bits(0x40c8f5c3)*cone as f32/t.word(13)? as f32}else{0.0};
                let mut vertices=Vec::with_capacity((t.word(14)? as usize+1)*2);
                for i in 0..=t.word(14)? {
                    let a=std::f32::consts::TAU*i as f32/t.word(14)? as f32+rotation;
                    for (end,radius,color) in [(0,bottom,colors[0]),(1,top,colors[1])] {
                        let uv=if flags&0x100!=0 {[end as f32*t.float(15)?,i as f32*t.float(16)?/t.word(14)? as f32]}else{[i as f32*t.float(15)?/t.word(14)? as f32,end as f32*t.float(16)?]};
                        vertices.push(Vertex {pos:(p+Vec3::new(a.cos()*radius,end as f32*height,-a.sin()*radius)).to_array(),uv,color,..Default::default()});
                    }
                }
                out.push(vertices);bottom+=bottom_step;top+=top_step;
            }
        }
        Ok(Some(out))
    }
}
fn mix_packed(a:u32,b:u32,t:f32)->[f32;4] {
    let a=a.to_be_bytes();let b=b.to_be_bytes();
    super::buff300x::packed_color(u32::from_be_bytes(std::array::from_fn(|i|((a[i] as f32*(1.0-t)+b[i] as f32*t) as u32&255) as u8)))
}

struct TestRock { selector: usize, position: Vec3, velocity: Vec3, axis: Vec3, angle: f32, spin: f32 }
/// MeshTest1028 GC100e564b/init100e54ad/process100e52cf; DS1001c435.
pub(super) struct MeshTest {
    pub resources: Vec<(u32,std::sync::Arc<ao_scene::Scene>)>,
    rocks: Vec<TestRock>,
    terminated: bool,
}
impl MeshTest {
    pub fn new(t:&Template,source:Mat4,store:&ao_rdb::RecordStore,names:&ao_formats::character::NameTable,random:&mut ao_formats::weather::R250,crt:&mut ao_formats::character::CrtRand,cache:&mut std::collections::HashMap<u32,std::sync::Arc<ao_scene::Scene>>)->Result<Self> {
        ensure!(t.kind==1028,"invalid MeshTest template");
        let source=super::sprites::connector(t,source)?;
        let mut resources=Vec::with_capacity(8);
        for selector in 11..=18 {
            let scene=match cache.entry(selector) {
                std::collections::hash_map::Entry::Occupied(entry)=>std::sync::Arc::clone(entry.get()),
                std::collections::hash_map::Entry::Vacant(entry)=>std::sync::Arc::clone(entry.insert(std::sync::Arc::new(super::meshes::load_resource(store,names,selector)?))),
            };
            resources.push((selector,scene));
        }
        let mut rocks=Vec::with_capacity(32);
        for _ in 0..32 {
            let selector=(crt.rand()&7) as usize;
            let velocity=random_vector(random)*15.0;
            let axis=random_vector(random).normalize_or_zero();
            let axis=if axis==Vec3::ZERO {Vec3::Y}else{axis};
            let angle=super::random_fraction(random)*std::f32::consts::TAU-std::f32::consts::PI;
            let spin=(super::random_fraction(random)*std::f32::consts::TAU-std::f32::consts::PI)*8.0;
            rocks.push(TestRock {selector,position:source.w_axis.truncate(),velocity,axis,angle,spin});
        }
        // Native constructor overwrites authored parameter8 with -1 after Init.
        Ok(Self {resources,rocks,terminated:false})
    }
    /// MeshTest's setters all dispatch to native100793e8 (including duration).
    pub fn configure(&mut self,_c:EffectConfig) {}
    pub fn terminate_gracefully(&mut self) {self.terminated=true;}
    pub fn frame(&mut self,dt:f32,random:&mut ao_formats::weather::R250,mut ground:impl FnMut(Vec3)->f32)->bool {
        if self.terminated {return false;}
        for rock in &mut self.rocks {
            rock.velocity.y-=dt*9.8;rock.position+=rock.velocity*dt;
            let floor=ground(rock.position);
            if rock.position.y<floor {
                rock.position.y=2.0*floor-rock.position.y;
                rock.velocity.y= -rock.velocity.y;rock.velocity*=0.5;
                if rock.velocity.length()<0.5 {rock.position.y=floor;rock.velocity=Vec3::ZERO;rock.spin=0.0;}
                else {
                    rock.axis=random_vector(random).normalize_or_zero();
                    if rock.axis==Vec3::ZERO {rock.axis=Vec3::Y;}
                    rock.angle=super::random_fraction(random)*std::f32::consts::TAU-std::f32::consts::PI;
                    rock.spin=(super::random_fraction(random)*std::f32::consts::TAU-std::f32::consts::PI)*8.0;
                }
            }
            rock.angle+=rock.spin*dt;
        }
        true
    }
    pub fn actors(&self,actor:u32,model:u64)->Vec<ao_scene::ActorFrame> {
        self.rocks.iter().enumerate().map(|(i,r)|ao_scene::ActorFrame {
            id:actor+i as u32,model:model|self.resources[r.selector].0 as u64,
            transform:Mat4::from_rotation_translation(glam::Quat::from_axis_angle(r.axis,r.angle),r.position).to_cols_array_2d(),
            alpha:1.0,..Default::default()
        }).collect()
    }
}
fn random_vector(random:&mut ao_formats::weather::R250)->Vec3 {
    Vec3::new(super::random_fraction(random)*2.0-1.0,super::random_fraction(random)*2.0-1.0,-(super::random_fraction(random)*2.0-1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn drip_record() -> Template {
        Template {kind:2009,words:vec![5,0,0,0,0,0,0,2000,0xbf800000,38,0,0x3ecccccd,0x3d23d70a,0x3f800000,0x3f333333,0x3e4ccccd,0x3e4ccccd,0x3f800000,0x3f800000,0x3e99999a,0x3e99999a,0xbf800000,0x3ecccccd,0x3e99999a,0x3e4ccccd]}
    }
    #[test]
    fn authored_25318_drip_four_state_lifecycle() -> Result<()> {
        let mut d=Drips::new(&drip_record(),Mat4::from_translation(Vec3::Y))?;
        assert!(d.advance(0.4,true,|_|0.0)?);assert_eq!(d.drops[0].state,0);
        d.advance(0.01,true,|_|0.0)?;assert_eq!(d.drops[0].state,1);
        d.advance(0.31,true,|_|0.0)?;assert_eq!(d.drops[0].state,2);
        assert_eq!(d.drops[0].velocity,-0.01);
        assert_eq!(d.drops[0].render_position.y,1.0);
        for _ in 0..20 {d.advance(0.016,true,|_|0.0)?;}
        assert!(d.drops.iter().any(|drop|drop.state==3));
        d.terminate_gracefully();assert!(d.advance(10.0,true,|_|0.0)?);
        assert!(d._terminating);
        Ok(())
    }
    #[test]
    fn installed_legacy_authored_record_shapes() -> Result<()> {
        let dir=ao_gui::client_dir();
        if !dir.join("Setupf/gfxtweak.bin").exists() {return Ok(());}
        let table=super::super::Templates::open(&dir)?;
        for (id,kind,length) in [(4000,1028,10),(8020,1023,40),(15400,2008,32),(25318,2009,25)] {
            let t=&table.by_id[&id];assert_eq!(t.kind,kind);assert_eq!(t.words.len(),length);
        }
        let mut h=Hexagram::new(&table.by_id[&15400],Mat4::IDENTITY,|_|0.0)?;
        assert_eq!(h.models().len(),24);assert_eq!(h.models()[0].2,22);
        h.frame(0.5);assert_eq!(h.vertices(true,|_|0.0)?.unwrap().len(),24);
        assert!(!h.frame(10.0));
        let mut random=ao_formats::weather::R250::new(7);let mut crt=ao_formats::character::CrtRand::new(7);
        let mut s=super::super::sprites::SpriteEffect::new_with_id(8020,&table.by_id[&8020],Mat4::IDENTITY,Mat4::IDENTITY,0,&mut random)?;
        s.initialize_native(&mut random,&mut crt)?;
        assert_eq!(s.models()[0].2,400);
        assert_eq!(s.blends(),vec![Blend::AlphaBlend]);
        assert!(s.vertices(0.01,Vec3::Z,Vec3::X,Vec3::Y,&mut random,&mut ao_formats::weather::R250::new(8),&mut crt)?.is_some());
        let store=ao_rdb::RecordStore::open(&dir)?;
        let names=ao_formats::character::NameTable::load(&store)?;
        let mut rocks=MeshTest::new(&table.by_id[&4000],Mat4::IDENTITY,&store,&names,&mut random,&mut crt,&mut std::collections::HashMap::new())?;
        assert_eq!(rocks.resources.len(),8);assert_eq!(rocks.actors(100,1000).len(),32);
        assert!(rocks.frame(1.0/60.0,&mut random,|_|0.0));
        rocks.terminate_gracefully();assert!(!rocks.frame(0.0,&mut random,|_|0.0));
        let sprite_template=table.by_id.values().find(|t|t.kind==1012).unwrap();
        for selector in [1,2] {
            let mut t=sprite_template.clone();{ t.words.resize(t.words.len().max((10) + 1), 0); *t.words.get_mut(10).unwrap() = (t.word(10).unwrap_or(0)&!3)|selector; };
            let mut sprite=super::super::sprites::SpriteEffect::new_with_id(0,&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut random)?;
            assert!(sprite.vertices(0.0,Vec3::Z,Vec3::X,Vec3::Y,&mut random,&mut ao_formats::weather::R250::new(8),&mut crt)?.is_none());
        }
        Ok(())
    }
    #[test]
    #[ignore="installed assets and offscreen GPU; set AOMAC_EFFECT_FRAMES"]
    fn retail_legacy_sprite_authored_frames() -> Result<()> {
        use super::super::{Binding,Creation,Renderer,MODEL_BASE,MESH_MODEL_BASE};
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").ok_or_else(||anyhow::anyhow!("AOMAC_EFFECT_FRAMES required"))?;
        let out=std::path::PathBuf::from(out);std::fs::create_dir_all(&out)?;
        let mut renderer=Renderer::open(&ao_gui::client_dir())?;
        let origin=Vec3::new(5000.0,10.0,5000.0);let eye=origin+Vec3::new(4.0,3.0,8.0);
        let mut host=ao_render::Host::headless();host.camera=ao_render::Camera::look_at(eye,origin);
        for id in [4000,8020,15400,25318] {
            renderer.clear();
            renderer.spawn(Binding {group:0,attractor:0,effect:id,note:0,color:0},Creation::Vector,Mat4::from_translation(origin),origin)?;
            for frame in 1..=120 {
                host.actors.clear();let mut terrain=|p:Vec3|Some((Vec3::new(p.x,9.0,p.z),Vec3::Y));
                renderer.frame(1.0/60.0,&mut host,Some(&mut terrain));
                if [1,6,30,60,120].contains(&frame) {
                    let mut models:Vec<_>=renderer.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                    models.extend(renderer.mesh_resources.iter().map(|(&id,m)|(MESH_MODEL_BASE|id as u64,m.as_ref().clone())));
                    ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,host.actors.clone(),eye.to_array(),origin.to_array(),640,480,&out.join(format!("legacy_{id}_{frame}.png")),frame as f32/60.0)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[test]
fn short_hexagram_defaults_missing_fields_to_zero() {
    let mut words=vec![0;15];words[10]=1;words[13]=1;words[14]=3;
    let mut effect=Hexagram::new(&Template {kind:2008,words},Mat4::IDENTITY,|_|0.0).unwrap();
    assert_eq!(effect.template.float(31).unwrap(),0.0);
    assert!(!effect.frame(0.0));
    assert_eq!(effect.models().len(),1);
    assert_eq!(effect.blends(),vec![Blend::AlphaBlend]);
}
