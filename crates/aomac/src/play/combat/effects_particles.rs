//! Class 3020: GC 10112e94/1011277c/10112bb0; DS 10029de9/10029c81/1002a350.
//! These are crossed, tapered local-space trails, not camera-facing sprites.
use super::{materials, quad, EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_formats::{character::CrtRand, weather::R250};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

struct Particle { position: Vec3, velocity: Vec3 }
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
    pub(super) fn new(template:&Template,source:Mat4,_target:Mat4,_color:u32,_random:&mut R250,display:&mut R250,_crt:&mut CrtRand)->Result<Self> {
        ensure!(Self::supports(template.kind),"unsupported TParticle class {}",template.kind);
        template.word(40)?;
        for i in 0..=40 {
            if !matches!(i,0|7|9|10|11|13|14|23..=28|34) {template.float(i)?;}
        }
        ensure!(template.word(10)?==0,"unsupported native TParticle emission mode");
        let count=template.word(11)? as usize;
        ensure!(count<=u16::MAX as usize/8,"TParticle capacity exceeds native indices");
        ensure!(template.float(8)?!=0.0,"zero TParticle control lifetime");
        materials::MATERIALS.get(template.word(9)? as usize).context("unknown TParticle material")?;
        let mut particles=Vec::with_capacity(count);
        for _ in 0..count {
            let uniform=|r:&mut R250|super::random_fraction(r);
            let direction=Vec3::new(uniform(display)*2.0-1.0,uniform(display)*2.0-1.0,uniform(display)*2.0-1.0).normalize_or_zero();
            let position=direction*(uniform(display)*template.float(12)?);
            let velocity=Vec3::new(
                template.float(17)?+(template.float(18)?-template.float(17)?)*uniform(display),
                template.float(19)?+(template.float(20)?-template.float(19)?)*uniform(display),
                template.float(21)?+(template.float(22)?-template.float(21)?)*uniform(display));
            // DS initializes angular position and velocity even though this draw
            // path does not use them; preserve the shared DisplaySystem RNG walk.
            uniform(display);uniform(display);
            particles.push(Particle {position,velocity});
        }
        Ok(Self {template:template.clone(),source,particles,previous_time:0.0,elapsed:0.0})
    }
    // The retail class inherits no-op effect setters (GC vtable 1016e99c).
    pub(super) fn configure(&mut self,_config:EffectConfig)->Result<()> {Ok(())}
    pub(super) fn update_source(&mut self,source:Mat4) {self.source=source;}
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let n=self.particles.len()*2;
        vec![(Some(self.template.words[9] as usize),(0..n as u32).flat_map(|i|[4*i,4*i+1,4*i+2,4*i+1,4*i+2,4*i+3]).collect(),n*4)]
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![if self.template.words[0]&0x200!=0 && self.template.words[0]&0x1000==0 {Blend::Additive}else{Blend::AlphaBlend}]}
    // Preserve the native frame inputs and separate Gamecode/DisplaySystem/CRT RNG streams.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn vertices(&mut self,time:f32,_camera:Vec3,_right:Vec3,_up:Vec3,_random:&mut R250,_display:&mut R250,_crt:&mut CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        let dt=(time-self.previous_time).clamp(0.0,0.033);
        self.previous_time=time;self.elapsed=time;
        let life=self.template.float(8)?;
        let phase=if life<0.0 {0.0}else{self.elapsed/life};
        if phase>1.0 {return Ok(None);}
        let (stage,part)=if phase<0.5 {(0,phase*2.0)}else{(1,phase*2.0-1.0)};
        let head=interpolate_color(self.template.words[24+stage*2],self.template.words[26+stage*2],part);
        let tail=interpolate_color(self.template.words[23+stage*2],self.template.words[25+stage*2],part);
        let size_phase=if self.template.words[34]==4 {part.powi(8)}else{part};
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
        let v0=if self.template.words[0]&0x100!=0 {1.0}else{0.0};let v1=1.0-v0;
        for particle in &mut self.particles {
            particle.position+=particle.velocity*dt;
            particle.velocity.y+=gravity*dt;
            let end=particle.position-particle.velocity*trail;
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
        let mut effect=ParticleEffect::new(&template(),Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt).unwrap();
        assert_eq!(ds.next_u32(),expected.next_u32());
        assert_eq!(effect.models()[0].2,160);assert_eq!(effect.models()[0].1.len(),240);
        let vertices=effect.vertices(0.01,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().unwrap();
        assert_eq!(vertices[0].len(),160);assert!(vertices[0].iter().all(|v|v.pos.iter().all(|x|x.is_finite())));
        for frame in 2..=100 {effect.vertices(frame as f32/60.0,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap();}
        assert!(effect.vertices(2.0,Vec3::Z,Vec3::X,Vec3::Y,&mut gc,&mut ds,&mut crt).unwrap().is_none());
        let mut malformed=template();malformed.words.pop();
        assert!(ParticleEffect::new(&malformed,Mat4::IDENTITY,Mat4::IDENTITY,0,&mut gc,&mut ds,&mut crt).is_err());
    }
    #[test]
    #[ignore="installed retail assets and offscreen Metal rendering"]
    fn retail_class3020_frames() {
        use super::super::{Binding,Renderer,MODEL_BASE};
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).unwrap_or_else(||"/tmp/FxRest/frames".into());
        std::fs::create_dir_all(&out).unwrap();
        let mut renderer=Renderer::open(&ao_gui::client_dir()).unwrap();
        let origin=Vec3::new(5000.0,10.0,5000.0);let eye=origin+Vec3::new(2.0,2.0,5.0);
        let mut host=ao_render::Host::headless();host.camera=ao_render::Camera::look_at(eye,origin);
        for id in [71512,71342] {
            renderer.clear();renderer.spawn(Binding {group:0,attractor:0,effect:id,note:0,color:0},Mat4::from_translation(origin),origin+Vec3::X).unwrap();
            for frame in 1..=60 {
                host.actors.clear();renderer.frame(1.0/60.0,&mut host,None);
                if [1,6,15,30,45].contains(&frame) {
                    let models:Vec<_>=renderer.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                    ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,host.actors.clone(),eye.to_array(),origin.to_array(),640,480,&out.join(format!("particle3020_{id}_{frame}.png")),frame as f32/60.0).unwrap();
                }
            }
        }
    }
}
