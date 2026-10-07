//! GC Spiral 2001: ctors100f4ab6/100f4e53, loader100f499c, init100f49f0,
//! process100f4760, graceful100f488e, delete100f4f87. DS10021eb4/10021924.
use super::{materials, sprites, EffectConfig, Template};
use anyhow::{ensure, Context, Result};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

pub(super) struct Spiral {
    template: Template,
    position: Vec3,
    duration: f32,
    elapsed: f32,
    visible: bool,
    bounds: [f32; 2],
    circles: [[(f32, f32); 13]; 2],
}
impl Spiral {
    pub(super) fn supports(kind: i32) -> bool { kind == 2001 }
    #[allow(clippy::approx_constant)]
    pub(super) fn new(t: &Template, source: Mat4, c: EffectConfig) -> Result<Self> {
        ensure!(Self::supports(t.kind), "not a native Spiral template");
        for i in (1..=6).chain([8]).chain(10..=17) { t.float(i)?; }
        materials::MATERIALS.get(t.word(9)? as usize).context("unknown Spiral material")?;
        let mut circles = [[(0.0, 0.0); 13]; 2];
        for (layer, circle) in circles.iter_mut().enumerate() {
            let mut angle = layer as f32 * 6.28 / 2.0;
            for point in circle { *point = (angle.cos(), angle.sin()); angle += 6.28 / 12.0; }
        }
        let mut n = Self { template: t.clone(), position: sprites::connector(t, source)?.w_axis.truncate(), duration: t.float(8)?, elapsed: 0.0, visible: true, bounds: [0.0, 1.0], circles };
        n.configure(c)?;
        Ok(n)
    }
    pub(super) fn configure(&mut self, c: EffectConfig) -> Result<()> {
        if let Some(d) = c.duration { ensure!(d.is_finite(), "nonfinite Spiral duration"); self.duration = d; }
        // Native CMS setters store colours, but Spiral never reads them.
        Ok(())
    }
    pub(super) fn update_source(&mut self, source: Mat4) -> Result<()> {
        self.position = sprites::connector(&self.template, source)?.w_axis.truncate(); Ok(())
    }
    pub(super) fn set_visible(&mut self, visible: bool) { self.visible = visible; }
    pub(super) fn source_removed(&mut self) { self.duration = self.elapsed; }
    pub(super) fn graceful(&mut self) { self.duration = self.elapsed; }
    pub(super) fn models(&self) -> Vec<(Option<usize>, Vec<u32>, usize)> {
        vec![(Some(self.template.words[9] as usize), (0..2u32).flat_map(|layer| (0..12u32).flat_map(move |i| {
            let a = layer * 26 + i * 2; [a, a + 1, a + 2, a + 2, a + 1, a + 3]
        })).collect(), 52)]
    }
    pub(super) fn blends(&self) -> Vec<Blend> { vec![Blend::Additive] }
    #[allow(clippy::approx_constant)]
    pub(super) fn vertices(&mut self, time: f32) -> Result<Option<Vec<Vec<Vertex>>>> {
        ensure!(time.is_finite() && time >= 0.0, "invalid Spiral time");
        self.elapsed = time;
        if self.duration >= 0.0 && time > self.duration { return Ok(None); }
        let phase = (time + time) / self.duration;
        if phase < 1.0 { self.bounds = [0.0, phase]; }
        else if phase < 2.0 { self.bounds = [phase - 1.0, 1.0]; }
        // DS clamps the start even for indefinite GC duration (-1).
        let mut start = self.bounds[0];
        if !(0.0..=1.0).contains(&start) { start = 0.0; }
        let end = self.bounds[1].max(start).min(1.0);
        let first = (12.0 * start).floor() as usize;
        let last = if end < 1.0 && start < end { (12.0 * end).ceil().max(1.0) as usize } else { 12 };
        let rotation = Mat4::from_rotation_y(-((time as f64 * f64::from_bits(0x400d9999a0000000)) as f32));
        let mut out = Vec::with_capacity(52);
        for circle in &self.circles {
            let mut strip = [Vertex::default(); 26];
            for (i, &(cos, sin)) in circle.iter().enumerate() {
                let arc = i as f32 * 6.28_f32 / 12.0;
                for side in 0..2 {
                    strip[i * 2 + side] = Vertex { pos: [cos * 0.6, arc * 0.3 - side as f32 * 0.3, -sin * 0.6], uv: [-time + i as f32 * ((6.28_f32 * 0.6 / 1.5) / 12.0), 1.0 - side as f32], ..Vertex::default() };
                }
            }
            if start > 0.0 && first < 12 { interpolate_pair(&mut strip, first, first + 1, 12.0 * start - first as f32); }
            if end < 1.0 && start < end { interpolate_pair(&mut strip, last, last - 1, last as f32 - 12.0 * end); }
            for i in 0..13 {
                let clamped = i.clamp(first.min(12), last);
                for side in 0..2 {
                    let mut v = strip[clamped * 2 + side];
                    v.pos = (self.position + rotation.transform_vector3(Vec3::from_array(v.pos))).to_array();
                    // DS sets the complete endpoint ARGB to zero, not alpha alone.
                    if clamped == first || clamped == last || !self.visible { v.color = [0.0; 4]; }
                    out.push(v);
                }
            }
        }
        Ok(Some(vec![out]))
    }
}
fn interpolate_pair(strip: &mut [Vertex; 26], to: usize, from: usize, fraction: f32) {
    for side in 0..2 {
        let a = strip[to * 2 + side]; let b = strip[from * 2 + side];
        strip[to * 2 + side].pos = Vec3::from_array(a.pos).lerp(Vec3::from_array(b.pos), fraction).to_array();
        strip[to * 2 + side].uv = std::array::from_fn(|i| a.uv[i] * (1.0 - fraction) + b.uv[i] * fraction);
    }
}
// Plasma 2002 is the integer hit-location overload only (100d145c).
// GC100ec059/100ebf73/100ebfcd/100ebd91/100ebe66/100ec194;
// DS1001b8f7/1001bbf2, 75 segments and four authored sine-cubed waves.
pub(super) struct Plasma {
    template: Template,
    hit: Option<(Vec3, Vec3)>,
    duration: f32,
    phases: [f32; 4],
    start: [f32; 4],
    stop: [f32; 4],
}
impl Plasma {
    pub(super) fn new(t: &Template, hit: Option<(Vec3, Vec3)>, c: EffectConfig) -> Result<Self> {
        ensure!(t.kind == 2002, "not a native Plasma template");
        for i in (1..=6).chain([8]).chain(10..=18) { t.float(i)?; }
        materials::MATERIALS.get(t.word(9)? as usize).context("unknown Plasma material")?;
        let color = |base:usize| -> Result<[f32;4]> { Ok([t.float(base+1)?,t.float(base+2)?,t.float(base+3)?,t.float(base)?]) };
        let duration = c.duration.unwrap_or(t.float(18)?);
        ensure!(duration.is_finite(), "nonfinite Plasma duration");
        if let Some((a,b)) = hit { ensure!(a.is_finite() && b.is_finite(), "nonfinite hit location"); }
        Ok(Self {template:t.clone(),hit,duration,phases:[0.0;4],start:c.start_color.unwrap_or(color(10)?),stop:c.stop_color.unwrap_or(color(14)?)})
    }
    pub(super) fn update_hit_location(&mut self, hit: Option<(Vec3, Vec3)>) -> Result<()> {
        if let Some((a,b)) = hit { ensure!(a.is_finite() && b.is_finite(), "nonfinite hit location"); }
        self.hit = hit; Ok(())
    }
    pub(super) fn graceful(&mut self) { self.duration = 0.0; }
    pub(super) fn models(&self) -> Vec<(Option<usize>,Vec<u32>,usize)> {
        vec![(Some(self.template.words[9] as usize),(0..75u32).flat_map(|i| {let a=i*2;[a,a+1,a+2,a+2,a+1,a+3]}).collect(),152)]
    }
    pub(super) fn blends(&self) -> Vec<Blend> { vec![Blend::Additive] }
    pub(super) fn vertices(&mut self,time:f32,forward:Vec3,crt:&mut ao_formats::character::CrtRand)->Result<Option<Vec<Vec<Vertex>>>> {
        ensure!(time.is_finite() && time>=0.0,"invalid Plasma time");
        if self.duration>=0.0 && time>self.duration {return Ok(None);}
        let Some((start,end))=self.hit else {return Ok(None)};
        let mut step=(end-start)*(1.0/75.0);let distance=step.length();
        if distance==0.0 {step=Vec3::X*0.01;}
        // Native camera-space +Z cross direction; reflection reverses cross products.
        let side=(-forward.cross(step)).normalize_or_zero()*0.1;
        let random=crt.rand();
        if random&0x7c0==0 {self.phases[(random&2) as usize]+=u32::from(random&0xfffffffc!=0) as f32*0.5;}
        let phase=time/self.duration;
        let color=super::buff200x::render_color(std::array::from_fn(|i|self.start[i]+(self.stop[i]-self.start[i])*phase));
        let amplitude=[0.15,0.3,0.2,0.1];let wavelength=[0.2,0.3,0.4,0.5];let speed=[-0.8,1.0,1.2,0.7];
        let mut centre=Vec3::ZERO;let mut out=Vec::with_capacity(152);
        for i in 0..=75 {
            let mut wave=0.0;
            for j in 0..4 {
                let frequency=(f64::from_bits(0x40191eb860000000)/wavelength[j]) as f32;
                let angle=(speed[j]*time+i as f32*distance)*frequency+self.phases[j];
                let sine=angle.sin();wave+=sine*sine*sine*amplitude[j];
            }
            let point=centre+side*wave;
            let u=(wave as f64*f64::from_bits(0x3fb99999a0000000)) as f32;
            out.push(Vertex {pos:(start+point+side).to_array(),uv:[u,0.0],color,..Vertex::default()});
            out.push(Vertex {pos:(start+point-side).to_array(),uv:[1.0,1.0-u],color,..Vertex::default()});
            centre+=step;
        }
        Ok(Some(vec![out]))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn authored(offset: u32) -> Template { Template { kind: 2001, words: vec![5,offset,0x3e800000,0,0,0,0,0,0x40800000,32,0x3f800000,0x3f800000,0x3f800000,0x3f800000,0x3f800000,0x3f800000,0x3f800000,0x3f800000] } }
    #[test]
    fn authored_43010_43011_spiral_lifecycle() -> Result<()> {
        for offset in [0, 0x3f000000] {
            let mut n = Spiral::new(&authored(offset), Mat4::IDENTITY, EffectConfig::default())?;
            assert_eq!(n.models()[0].2, 52); assert_eq!(n.models()[0].1.len(), 144);
            let v = n.vertices(1.0)?.unwrap(); assert_eq!(v[0].len(), 52);
            assert_eq!(n.bounds, [0.0, 0.5]); assert_eq!(n.position.y, 0.25);
            assert_eq!(n.position.x, f32::from_bits(offset));
            n.vertices(3.0)?; assert_eq!(n.bounds, [0.5, 1.0]);
            n.graceful(); assert!(n.vertices(3.01)?.is_none());
        }
        let mut t = authored(0); t.words[8] = (-1.0f32).to_bits();
        let mut n = Spiral::new(&t, Mat4::IDENTITY, EffectConfig::default())?;
        assert!(n.vertices(100.0)?.is_some()); n.source_removed(); assert!(n.vertices(100.01)?.is_none());
        t.words.truncate(17);
        let short=Spiral::new(&t,Mat4::IDENTITY,EffectConfig::default())?;
        assert_eq!(short.template.float(17)?,0.0,"native absent final Spiral field defaults to zero");
        t.words[16]=f32::NAN.to_bits();
        assert!(Spiral::new(&t,Mat4::IDENTITY,EffectConfig::default()).is_err());
        Ok(())
    }
    #[test]
    fn authored_17500_plasma_hit_lifetime_and_rng() -> Result<()> {
        let t=Template {kind:2002,words:vec![5,0,0x3e800000,0,0,0,0,0,0xbf800000,8,0x3f800000,0x3e99999a,0x3f800000,0x3e99999a,0x3f800000,0x3e99999a,0x3f800000,0x3e99999a,0x3f800000]};
        let mut n=Plasma::new(&t,Some((Vec3::ZERO,Vec3::X)),EffectConfig::default())?;
        assert_eq!(n.duration,1.0); assert_eq!(n.models()[0].2,152);
        let mut crt=ao_formats::character::CrtRand::new(7);
        let mut expected=ao_formats::character::CrtRand::new(7);expected.rand();
        let v=n.vertices(0.5,Vec3::Z,&mut crt)?.unwrap();
        assert_eq!(v[0].len(),152);assert_eq!(crt.rand(),expected.rand());
        assert!(v[0].iter().all(|v|v.pos.iter().all(|x|x.is_finite())));
        n.update_hit_location(None)?;assert!(n.vertices(0.6,Vec3::Z,&mut crt)?.is_none());
        let mut n=Plasma::new(&t,Some((Vec3::ZERO,Vec3::X)),EffectConfig::default())?;
        n.graceful();assert!(n.vertices(0.1,Vec3::Z,&mut crt)?.is_none());
        Ok(())
    }
    #[test]
    #[ignore = "installed retail assets and offscreen Metal rendering"]
    fn retail_native2002_hit_frames() -> Result<()> {
        use super::super::Templates;
        use ao_scene::{Mesh,Submesh,Scene,ActorFrame,TextureKey};
        let templates=Templates::open(&ao_gui::client_dir())?;
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).unwrap_or_else(||"/tmp/FxClasses/frames".into());
        std::fs::create_dir_all(&out)?;
        // Renderer supplies the real material texture; bypass only the located
        // factory, which natively cannot construct this integer-overload class.
        let renderer=super::super::Renderer::open(&ao_gui::client_dir())?;
        let origin=Vec3::new(5000.0,10.0,5000.0);let eye=origin+Vec3::new(2.0,2.0,5.0);
        for id in [11201,17500,17600,17912,17913,17914] {
            let t=&templates.by_id[&id];let mut n=Plasma::new(t,Some((origin,origin+Vec3::Y*2.0)),EffectConfig::default())?;
            let mut crt=ao_formats::character::CrtRand::new(1);
            let name=materials::MATERIALS[t.words[9] as usize].0;
            let key=TextureKey {rdb_type:1010004,id:renderer.names.id(1010004,name).context("missing Plasma texture name")?};
            let texture=ao_formats::texture::load_texture(&renderer.store,key)?.context("missing Plasma texture")?;
            for frame in [1,6,15,30,45] {
                let time=frame as f32/60.0;
                let Some(mut vertices)=n.vertices(time,(origin-eye).normalize(),&mut crt)? else {continue};
                let mut sub=Submesh::new(n.models().remove(0).1,Some(key));
                sub.blend=Blend::Additive;sub.two_sided=true;sub.emissive=[1.0;3];
                let mut scene=Scene::default();
                scene.textures.insert(key,texture.clone());
                scene.meshes.push(Mesh {vertices:vertices.remove(0),submeshes:vec![sub]});
                ao_render::render_to_png_actors(&scene,&[],Vec::<ActorFrame>::new(),eye.to_array(),origin.to_array(),640,480,&out.join(format!("native2002_{id}_{frame}.png")),time)?;
            }
        }
        Ok(())
    }
    #[test]
    #[ignore = "installed retail assets and offscreen Metal rendering"]
    fn retail_native2001_frames() -> Result<()> {
        use super::super::{Binding, Renderer, MODEL_BASE};
        let out = std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).unwrap_or_else(|| "/tmp/FxClasses/frames".into());
        std::fs::create_dir_all(&out)?;
        let mut renderer = Renderer::open(&ao_gui::client_dir())?;
        let origin = Vec3::new(5000.0, 10.0, 5000.0); let eye = origin + Vec3::new(2.0, 2.0, 5.0);
        let mut host = ao_render::Host::headless(); host.camera = ao_render::Camera::look_at(eye, origin);
        for id in [11200, 43010, 43011] {
            renderer.clear(); renderer.spawn(Binding {group:0,attractor:0,effect:id,note:0,color:0}, super::super::Creation::Vector, Mat4::from_translation(origin), origin)?;
            for frame in 1..=240 {
                host.actors.clear(); renderer.frame(1.0/60.0, &mut host, None);
                if [1,30,60,120,180,239].contains(&frame) {
                    let models: Vec<_> = renderer.models.iter().map(|(&id,m)|(MODEL_BASE | id as u32 as u64,m.scene.clone())).collect();
                    ao_render::render_to_png_actors(&ao_scene::Scene::default(), &models, host.actors.clone(), eye.to_array(), origin.to_array(), 640,480,&out.join(format!("native2001_{id}_{frame}.png")),frame as f32/60.0)?;
                }
            }
        }
        Ok(())
    }
}
