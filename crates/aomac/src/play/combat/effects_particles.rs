//! Class 3020: GC 101125fd/1011277c/10112bb0; DS 10029de9/10029c81/1002a350.
//! These are crossed, tapered local-space trails, not camera-facing sprites.
use super::{materials, quad, EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::{character::CrtRand, weather::R250};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

struct Particle { position: Vec3, velocity: Vec3, end: Vec3 }
pub(super) struct ParticleEffect {
    template: Template,
    source: Mat4,
    particles: Vec<Particle>,
    previous_time: f32,
    elapsed: f32,
}

fn packed_color(word:u32)->[f32;4] {
    let [a,r,g,b]=word.to_be_bytes();
    [r as f32/255.0,g as f32/255.0,b as f32/255.0,a as f32/255.0]
}
fn interpolate_color(a:u32,b:u32,t:f32)->[f32;4] {
    let a=packed_color(a);let b=packed_color(b);
    // Color_t::Interpolate truncates each channel back to its packed byte.
    std::array::from_fn(|i| ((a[i]+(b[i]-a[i])*t)*255.0) as u8 as f32/255.0)
}

impl ParticleEffect {
    pub(super) fn supports(kind:i32)->bool {kind==3020}
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    pub(super) fn new(template:&Template,mut source:Mat4,_target:Mat4,_color:u32,_random:&mut R250,display:&mut R250,_crt:&mut CrtRand,terrain:Option<&mut dyn FnMut(Vec3)->Option<(Vec3,Vec3)>>)->Result<Self> {
        ensure!(Self::supports(template.kind),"unsupported TParticle class {}",template.kind);
        for i in 0..=40 {
            if !matches!(i,0|7|9|10|11|13|14|23..=28|34) {template.float(i)?;}
        }
        let mode=template.word(10)?;
        ensure!(mode<=3,"unknown native TParticle emission mode {mode}");
        if template.word(0)?&0x400!=0 {
            let terrain=terrain.context("TParticle ground flag requires terrain")?;
            let (ground,_)=terrain(source.w_axis.truncate()).context("TParticle source has no ground")?;
            source.w_axis.y=ground.y;
        }
        let count=template.word(11)? as usize;
        ensure!(count<=u16::MAX as usize/8,"TParticle capacity exceeds native indices");
        ensure!(template.float(8)?!=0.0,"zero TParticle control lifetime");
        materials::MATERIALS.get(template.word(9)? as usize).context("unknown TParticle material")?;
        let mut particles=Vec::with_capacity(count);
        for _ in 0..count {
            let uniform=|r:&mut R250|super::random_fraction(r);
            let (position,velocity,end)=match mode {
                0=>{
                    let direction=Vec3::new(uniform(display)*2.0-1.0,uniform(display)*2.0-1.0,uniform(display)*2.0-1.0).normalize_or_zero();
                    let position=direction*(uniform(display)*template.float(12)?);
                    let velocity=Vec3::new(
                        template.float(17)?+(template.float(18)?-template.float(17)?)*uniform(display),
                        template.float(19)?+(template.float(20)?-template.float(19)?)*uniform(display),
                        template.float(21)?+(template.float(22)?-template.float(21)?)*uniform(display));
                    // Angular fields are unused by the crossed-strip renderer.
                    uniform(display);uniform(display);
                    (position,velocity,Vec3::ZERO)
                },
                1=>(Vec3::ZERO,Vec3::ZERO,Vec3::new(
                    uniform(display)*2.0-1.0,uniform(display)*2.0-1.0,uniform(display)*2.0-1.0)*template.float(12)?),
                2=>{
                    // DS doubles 10089d30=0x401921fb60000000,
                    // 1008b898=600, 1008a128=2 (radians, not PI).
                    let angle=(uniform(display) as f64*f64::from_bits(0x401921fb60000000)) as f32;
                    let position=Vec3::new(angle.sin()*template.float(12)?,uniform(display)*600.0,angle.cos()*template.float(12)?);
                    let angle=angle+2.0;
                    let velocity=Vec3::new(angle.sin()*template.float(18)?,template.float(20)?,angle.cos()*template.float(22)?);
                    (position,velocity,Vec3::ZERO)
                },
                _=>(Vec3::ZERO,Vec3::Z,Vec3::ZERO),
            };
            particles.push(Particle {position,velocity,end});
        }
        Ok(Self {template:template.clone(),source,particles,previous_time:0.0,elapsed:0.0})
    }
    // The retail class inherits no-op effect setters (GC vtable 1016e99c).
    pub(super) fn configure(&mut self,_config:EffectConfig)->Result<()> {Ok(())}
    pub(super) fn update_source(&mut self,source:Mat4) {self.source=source;}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let n=self.particles.len()*2;
        vec![(Some(self.template.word(9).unwrap_or(0) as usize),(0..n as u32).flat_map(|i|[4*i,4*i+1,4*i+2,4*i+1,4*i+2,4*i+3]).collect(),n*4)]
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.template.word(0).unwrap_or(0)&0x200!=0 && self.template.word(0).unwrap_or(0)&0x1000==0 {Blend::Additive}else{Blend::AlphaBlend}]}
    // Preserve the native frame inputs and separate Gamecode/DisplaySystem/CRT RNG streams.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,_camera:Vec3,_right:Vec3,_up:Vec3,_random:&mut R250,_display:&mut R250,_crt:&mut CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        let dt=(time-self.previous_time).clamp(0.0,0.033);
        self.previous_time=time;self.elapsed=time;
        let life=self.template.float(8)?;
        let phase=if life<0.0 {0.0}else{self.elapsed/life};
        if phase>1.0 {return Ok(None);}
        let (stage,part)=if phase<0.5 {(0,phase*2.0)}else{(1,phase*2.0-1.0)};
        let head=interpolate_color(self.template.word(24+stage*2).unwrap_or(0),self.template.word(26+stage*2).unwrap_or(0),part);
        let tail=interpolate_color(self.template.word(23+stage*2).unwrap_or(0),self.template.word(25+stage*2).unwrap_or(0),part);
        let size_phase=if self.template.word(34).unwrap_or(0)==4 {part.powi(8)}else{part};
        let size=|axis|->Result<f32> {
            let i=35+stage*2+axis;
            Ok(self.template.float(i)?+(self.template.float(i+2)?-self.template.float(i)?)*size_phase)
        };
        let head_width=size(1)?;let tail_width=size(0)?;
        let trail=self.template.float(15)?;
        let gravity=self.template.float(16)?;
        let reflect=Vec3::new(1.0,1.0,-1.0);
        // GC10112aa0 removes locator scale before extracting its rotation.
        let mut matrix=self.source;
        matrix.x_axis=matrix.x_axis.truncate().normalize_or_zero().extend(0.0);
        matrix.y_axis=matrix.y_axis.truncate().normalize_or_zero().extend(0.0);
        matrix.z_axis=matrix.z_axis.truncate().normalize_or_zero().extend(0.0);
        let mut vertices=Vec::with_capacity(self.particles.len()*8);
        let v0=if self.template.word(0).unwrap_or(0)&0x100!=0 {1.0}else{0.0};let v1=1.0-v0;
        for particle in &mut self.particles {
            if matches!(self.template.word(10).unwrap_or(0),0|2) {
                particle.position+=particle.velocity*dt;
                particle.velocity.y+=gravity*dt;
                particle.end=particle.position-particle.velocity*trail;
            }
            let end=particle.end;
            let direction=(end-particle.position).normalize_or_zero();
            if direction==Vec3::ZERO {vertices.extend([Vertex {color:[0.0;4],..Vertex::default()};8]);continue;}
            let side=direction.cross(Vec3::Y).normalize_or_zero();
            let side=if side==Vec3::ZERO {Vec3::X}else{side};
            let other=side.cross(direction).normalize_or_zero();
            for axis in [other,side] {
                let positions=[particle.position-axis*head_width,particle.position+axis*head_width,end-axis*tail_width,end+axis*tail_width].map(|p|matrix.transform_point3(p*reflect));
                quad(&mut vertices,positions,[[0.0,v0],[1.0,v0],[0.0,v1],[1.0,v1]],[1.0;4]);
                let base=vertices.len()-4;
                for (i,color) in [head,head,tail,tail].into_iter().enumerate() {
                    vertices[base+i].color=[color[0].powf(2.2),color[1].powf(2.2),color[2].powf(2.2),color[3]];
                }
            }
        }
        Ok(Some(vec![vertices]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_particle_record_zeroes_absent_frame_fields() {
        let mut t=template();t.words.truncate(12);
        let mut gc=R250::new(1);let mut ds=R250::new(1);let mut crt=CrtRand::new(1);
        let mut e=ParticleEffect::new(&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt,None).unwrap();
        e.configure(EffectConfig::default()).unwrap();e.blends();assert_eq!(e.models()[0].2,160);
        for time in [0.25,0.75] {
            let groups=e.vertices(time,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().unwrap();
            assert!(groups[0].iter().all(|v|v.pos==[0.0;3] && v.color==[0.0;4]));
        }
    }
    fn template()->Template {
        let mut words=vec![0;41];words[0]=0x203;words[11]=20;
        for (i,f) in [(8,1.0f32),(12,0.1),(15,0.2),(17,-1.0),(18,1.0),(19,10.0),(20,15.0),(21,-1.0),(22,1.0),(35,0.2),(36,0.2),(37,0.5),(38,0.5),(39,0.8),(40,0.8)] {words[i]=f.to_bits();}
        for (i,c) in [(23,0x0080ff30),(24,0x0080ff30),(25,0x8080ff30),(26,0x8080ff30),(27,0x0080ff30),(28,0x0080ff30)] {words[i]=c;}
        Template {kind:3020,words}
    }
    #[test]
    fn crossed_trails_preserve_rng_capacity_and_control_lifecycle() {
        let mut gc=R250::new(0xe6f1);let mut ds=R250::new(0xe6f1);let mut crt=CrtRand::new(1);
        let mut expected=ds.clone();for _ in 0..180 {expected.next_u32();}
        let mut effect=ParticleEffect::new(&template(),Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt,None).unwrap();
        assert_eq!(ds.next_u32(),expected.next_u32());
        assert_eq!(effect.models()[0].2,160);assert_eq!(effect.models()[0].1.len(),240);
        let vertices=effect.vertices(0.01,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().unwrap();
        assert_eq!(vertices[0].len(),160);assert!(vertices[0].iter().all(|v|v.pos.iter().all(|x|x.is_finite())));
        for frame in 2..=100 {effect.vertices(frame as f32/60.0,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap();}
        assert!(effect.vertices(2.0,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().is_none());
        let mut short=template();short.words.pop();
        let effect=ParticleEffect::new(&short,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt,None).unwrap();
        assert_eq!(short.float(40).unwrap(),0.0,"native absent final trail width defaults to zero");
        assert_eq!(effect.models()[0].2,160);
        short.words[39]=f32::NAN.to_bits();
        assert!(ParticleEffect::new(&short,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt,None).is_err());
    }
    #[test]
    fn authored_modes_preserve_native_rng_and_motion() {
        // Installed records 71103, 71104, 71105 (class3020).
        let records=[
            vec![515,0,0,0,0,0,0,0,1065353216,55,1,20,1092616192,0,0,1045220557,3212836864,3259498496,1112014848,1084227584,1092616192,3259498496,1112014848,16777215,16777215,16777215,4294967295,16777215,16777215,0,1097859072,1112014848,1077936128,1045220557,0,1033476506,1033476506,1033476506,1033476506,1033476506,1033476506],
            vec![515,0,0,0,0,0,0,2011,1065353216,55,2,20,1082130432,0,0,1053609165,0,3212836864,1073741824,3212836864,1073741824,3212836864,1073741824,16777215,16777215,16777215,4294967295,16777215,16777215,0,1097859072,1112014848,1077936128,1045220557,0,1033476506,1033476506,1033476506,1033476506,1033476506,1033476506],
            vec![514,0,0,0,0,0,0,0,3212836864,56,3,20,1084227584,0,0,1045220557,3212836864,1036831949,1036831949,3212836864,3225419776,3212836864,1065353216,1621819306,1621819306,16777215,4294967295,16777215,16777215,0,1097859072,1112014848,1077936128,1045220557,0,1077936128,1077936128,1077936128,1077936128,1077936128,1077936128],
        ];
        for words in records {
            let t=Template {kind:3020,words};let mode=t.words[10];
            let mut gc=R250::new(0xe6f1);let mut ds=R250::new(0xe6f1);let mut crt=CrtRand::new(1);
            let mut expected=ds.clone();
            for _ in 0..t.words[11]*match mode {1=>3,2=>2,_=>0} {expected.next_u32();}
            let mut effect=ParticleEffect::new(&t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt,None).unwrap();
            assert_eq!(ds.next_u32(),expected.next_u32());
            let position=effect.particles[0].position;let end=effect.particles[0].end;
            if mode==2 {assert!((Vec3::new(position.x,0.0,position.z).length()-4.0).abs()<0.00001);assert!((0.0..600.0).contains(&position.y));}
            let vertices=effect.vertices(0.02,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().unwrap();
            if mode==2 {assert_ne!(effect.particles[0].position,position);}
            else {assert_eq!(effect.particles[0].position,position);assert_eq!(effect.particles[0].end,end);}
            if mode==3 {assert!(vertices[0].iter().all(|v|v.color==[0.0;4]));}
        }
    }
    #[test]
    fn ground_flag_snaps_only_initial_position() {
        let mut t=template();t.words[0]|=0x400;
        let mut gc=R250::new(1);let mut ds=R250::new(1);let mut crt=CrtRand::new(1);
        let mut terrain=|p:Vec3|Some((Vec3::new(p.x,17.0,p.z),Vec3::Y));
        let mut effect=ParticleEffect::new(&t,Mat4::from_translation(Vec3::ONE),Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt,Some(&mut terrain)).unwrap();
        assert_eq!(effect.source.w_axis.y,17.0);
        effect.update_source(Mat4::from_translation(Vec3::ONE));
        assert_eq!(effect.source.w_axis.y,1.0);
    }
    #[test]
    #[ignore="installed retail authored effect table"]
    fn all_authored_class3020_records() {
        let templates=super::super::Templates::open(&ao_gui::client_dir()).unwrap();
        let mut count=0;let mut modes=[false;4];
        for (id,t) in templates.by_id.iter().filter(|(_,t)|t.kind==3020) {
            let mut gc=R250::new(0xe6f1);let mut ds=R250::new(0xe6f1);let mut crt=CrtRand::new(1);
            let mut terrain=|p:Vec3|Some((Vec3::new(p.x,0.0,p.z),Vec3::Y));
            let mut effect=ParticleEffect::new(t,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt,Some(&mut terrain)).unwrap_or_else(|e|panic!("record {id}: {e:#}"));
            modes[t.words[10] as usize]=true;count+=1;
            let vertices=effect.vertices(1.0/60.0,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap();
            if let Some(vertices)=vertices {assert!(vertices[0].iter().all(|v|v.pos.iter().all(|x|x.is_finite())),"record {id}");}
        }
        assert!(count>0);assert_eq!(modes,[true;4]);
    }
    #[test]
    #[ignore="installed retail assets and offscreen Metal rendering"]
    fn retail_class3020_frames() {
        use super::super::{Binding,Creation,Renderer,MODEL_BASE};
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).unwrap_or_else(||"/tmp/FxRest/frames".into());
        std::fs::create_dir_all(&out).unwrap();
        let mut renderer=Renderer::open(&ao_gui::client_dir()).unwrap();
        let origin=Vec3::new(5000.0,10.0,5000.0);
        let mut host=ao_render::Host::headless();
        let mut ids:Vec<_>=renderer.templates.by_id.iter().filter_map(|(&id,t)|(t.kind==3020).then_some(id)).collect();
        ids.sort_unstable();
        assert!(!ids.is_empty(),"missing authored TParticle records");
        for id in ids {
            let t=&renderer.templates.by_id[&id];
            let mode=t.word(10).unwrap();
            let time=t.float(8).unwrap();
            let time=if time<0.0 {1.0}else{time.min(1.0)};
            let velocity=if mode==2 {
                Vec3::new(t.float(18).unwrap(),t.float(20).unwrap(),t.float(22).unwrap()).length()
            } else {
                Vec3::new(t.float(17).unwrap().abs().max(t.float(18).unwrap().abs()),t.float(19).unwrap().abs().max(t.float(20).unwrap().abs()),t.float(21).unwrap().abs().max(t.float(22).unwrap().abs())).length()
            };
            let width=(35..=40).map(|i|t.float(i).unwrap().abs()).fold(0.0f32,f32::max);
            let emission=t.float(12).unwrap().abs();
            let (center,mut radius)=match mode {
                1=>(origin,emission*3.0f32.sqrt()),
                // DS1008b898: the authored ring occupies Y=0..600.
                2=>(origin+Vec3::Y*300.0,(300.0f32.powi(2)+emission.powi(2)).sqrt()),
                _=>(origin,emission),
            };
            if mode==0 || mode==2 {
                let trail=t.float(15).unwrap().abs();
                radius+=velocity*(time+trail)+t.float(16).unwrap().abs()*time*(time*0.5+trail);
            }
            radius+=width+Vec3::new(t.float(1).unwrap(),t.float(2).unwrap(),t.float(3).unwrap()).length();
            let eye=center+Vec3::new(2.0,2.0,5.0).normalize()*radius.max(1.0)*2.4;
            host.camera=ao_render::Camera::look_at(eye,center);
            renderer.clear();renderer.spawn(Binding {group:0,attractor:0,effect:id,note:0,color:0},Creation::Matrix,Mat4::from_translation(origin),origin+Vec3::X).unwrap();
            for frame in 1..=60 {
                host.actors.clear();
                let mut terrain=|p:Vec3|Some((Vec3::new(p.x,10.0,p.z),Vec3::Y));
                renderer.frame(1.0/60.0,&mut host,Some(&mut terrain));
                if [1,6,15,30,45].contains(&frame) {
                    let models:Vec<_>=renderer.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                    ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,host.actors.clone(),eye.to_array(),center.to_array(),640,480,&out.join(format!("particle3020_{id}_{frame}.png")),frame as f32/60.0).unwrap();
                    if mode==2 {
                        // The 600-unit overview cannot resolve authored 0.075-unit
                        // strips; retain a geometry-derived close-up as well.
                        if let Some(vertices)=host.actors.first().and_then(|actor|actor.skin.as_ref()).and_then(|skin|skin.get(..8)) {
                            let mut lo=Vec3::splat(f32::INFINITY);let mut hi=Vec3::splat(f32::NEG_INFINITY);
                            for vertex in vertices {let p=Vec3::from_array(vertex.pos);lo=lo.min(p);hi=hi.max(p);}
                            let target=(lo+hi)*0.5;
                            let detail_eye=target+Vec3::new(2.0,2.0,5.0).normalize()*(hi-lo).length().max(0.1)*1.2;
                            ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,host.actors.clone(),detail_eye.to_array(),target.to_array(),640,480,&out.join(format!("particle3020_{id}_{frame}_detail.png")),frame as f32/60.0).unwrap();
                        }
                    }
                }
            }
        }
    }
}
