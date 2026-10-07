//! GC Vein3014 10102cde/10102ad0/101028c4/10102731/10102c39.
//! DS1002e885/1002def9/1002e6d1; cubic curves10004484/100043c3/100041ff.
use super::{materials, sprites, Binding, Creation, EffectConfig, Renderer, Template};
use anyhow::{ensure, Context, Result};
use ao_scene::{Blend, Vertex};
use glam::{Mat4, Vec3};

pub(super) struct Vein {
    template: Template,
    position: Vec3,
    config: EffectConfig,
    elapsed: f32,
    started: bool,
    advanced: bool,
    dead: bool,
    children: [u32; 2],
    weights: Vec<[f64; 9]>,
    rings: Vec<Vec3>,
    facing: Vec<f32>,
    heights: Vec<f32>,
    colors: Vec<[[f32; 4]; 2]>,
    sides: usize,
    layers: usize,
}
impl Vein {
    pub(super) fn new(t:&Template,source:Mat4,c:EffectConfig)->Result<Self> {
        ensure!(t.kind==3014,"not a native Vein template");
        t.word(29)?;
        for i in (1..=6).chain([8,11,12]).chain(15..=23) {t.float(i)?;}
        materials::MATERIALS.get(t.word(9)? as usize).context("unknown Vein material")?;
        let sides=t.word(14)? as usize;let layers=t.word(10)? as usize;
        ensure!((3..=4096).contains(&sides) && (1..=4096).contains(&layers),"invalid Vein strip dimensions");
        ensure!(sides.checked_mul(layers).is_some_and(|n|n<=1_000_000),"Vein dimensions exceed vertex limit");
        let samples=(sides as f64*f64::from_bits(0x3ff3333340000000)) as usize;
        let offset=(sides as f64*f64::from_bits(0x3fb99999a0000000)) as usize;
        let offset=offset.min(samples-sides);
        // Native evaluates the same clamped cubic basis at every timer update.
        // Its knots and sample indices never change, so cache only the weights.
        let mut weights=vec![[0.0;9];sides+1];let mut u=0.0;
        let increment=6.0/(samples-1) as f64;
        for sample in 0..samples-1 {
            if sample>offset && sample<=offset+sides {weights[sample-offset]=std::array::from_fn(|i|basis(i,4,u));}
            u+=increment;
        }
        if samples-1>offset && samples-1<=offset+sides {weights[samples-1-offset][8]=1.0;}
        weights[0]=weights[sides];
        let count=(layers+1)*(sides+1);
        let mut height= t.float(17)?;let mut total=height;let mut heights=vec![0.0;layers+1];
        for value in &mut heights[1..] {*value=total;height=(height as f64*f64::from_bits(0x3ff4ccccc0000000)) as f32;total+=height;}
        let mut colors=Vec::with_capacity(layers+1);
        for ring in 0..=layers {
            let phase=ring as f32/(layers+1) as f32;
            let byte=|a:u32,b:u32,shift:u32| -> f32 {((phase*((a>>shift)&255) as f32+(1.0-phase)*((b>>shift)&255) as f32) as u32&255) as f32/255.0};
            let rgba=|a:u32,b:u32|[byte(a,b,16).powf(2.2),byte(a,b,8).powf(2.2),byte(a,b,0).powf(2.2),phase*(a>>24) as f32+(1.0-phase)*(b>>24) as f32];
            colors.push([rgba(t.word(24).unwrap_or(0),t.word(25).unwrap_or(0)),rgba(t.word(26).unwrap_or(0),t.word(27).unwrap_or(0))]);
        }
        Ok(Self {template:t.clone(),position:sprites::connector(t,source)?.w_axis.truncate(),config:c,elapsed:0.0,started:false,advanced:false,dead:false,children:[0;2],weights,rings:vec![Vec3::ZERO;count],facing:vec![0.0;count],heights,colors,sides,layers})
    }
    // Native public UpdatePosition/UpdateMatrix slots reject the request.
    pub(super) fn refresh_source(&mut self,source:Option<Mat4>)->Result<()> {
        if self.template.word(0).unwrap_or(0)&0x1000!=0 {
            if let Some(source)=source {self.position=sprites::connector(&self.template,source)?.w_axis.truncate();}else{self.dead=true;}
        }
        Ok(())
    }
    pub(super) fn requires_terrain(&self)->bool {self.template.word(0).unwrap_or(0)&0x4000!=0}
    pub(super) fn set_ground_height(&mut self,height:f32) {self.position.y=height;}
    pub(super) fn graceful(&mut self) {self.dead=true;}
    pub(super) fn cancel(&mut self,renderer:&mut Renderer) {for child in &mut self.children {if *child!=0 {renderer.delete(*child);*child=0;}}}
    pub(super) fn frame(&mut self,dt:f32,renderer:&mut Renderer)->Result<bool> {
        ensure!(dt.is_finite()&&dt>=0.0,"invalid Vein frame time");
        if self.started {
            // GC base Process100d2531 caps only the first advancing delta.
            self.elapsed+=if self.advanced {dt}else{dt.min(f32::from_bits(0x3d072b02))};
            self.advanced=true;
        } else {
            self.started=true;
            for i in 0..2 {let id=self.template.word(28+i).unwrap_or(0) as i32;if id!=0 {
                let mut p=self.position;if i==1 {p.x*=0.5;p.z*=0.5;}
                self.children[i]=renderer.spawn_configured(Binding {group:0,attractor:0,effect:id,note:0,color:0},Mat4::from_translation(p),p,EffectConfig {creation:Creation::Vector,..EffectConfig::default()})?;
            }}
        }
        let lifetime=self.config.duration.unwrap_or(f32::from_bits(self.template.word(8).unwrap_or(0)));
        if self.dead || (lifetime>=0.0&&self.elapsed>lifetime) || self.elapsed>=self.template.float(11)?+self.template.float(12)? {self.cancel(renderer);return Ok(false);}
        Ok(true)
    }
    pub(super) fn models(&self)->Vec<(Option<usize>,Vec<u32>,usize)> {
        let per=(self.sides+1)*2;let count=per*self.layers;
        let indices:Vec<_>=(0..self.layers).flat_map(|layer|(0..self.sides).flat_map(move|i|{let a=(layer*per+i*2) as u32;[a,a+1,a+2,a+2,a+1,a+3]})).collect();
        vec![(Some(self.template.word(9).unwrap_or(0) as usize),indices.clone(),count),(Some(self.template.word(9).unwrap_or(0) as usize),indices,count)]
    }
    pub(super) fn blends(&self)->Vec<Blend> {vec![Blend::Additive;2]}
    pub(super) fn vertices(&mut self,time:f32,camera:Vec3,right:Vec3,up:Vec3)->Result<Option<Vec<Vec<Vertex>>>> {
        if self.dead {return Ok(None);}
        ensure!(time.is_finite()&&time>=0.0,"invalid Vein timer");
        let distance=self.position.distance(camera);
        let size=if distance>100.0 {(distance as f64/f64::from_bits(0x404264db5a52cb99)).ln() as f32}else{1.0};
        let forward=-right.cross(up);let mut look=Vec3::new(right.z,0.0,-forward.z);
        if look.x==0.0 {look.x=1.0;}else{look=look.normalize();}look.x= -look.x;
        let bottom=self.template.float(18)?;let top=self.template.float(19)?;
        for layer in 0..=self.layers {
            let timer=if layer==0 {time}else{(time as f64-layer as f64*f64::from_bits(0x3fc99999a0000000)) as f32};
            let controls=controls(timer);
            let radius=bottom*(1.0-layer as f32/self.layers as f32)+top*(layer as f32/self.layers as f32);
            let mut offset=Vec3::ZERO;
            if self.template.word(0).unwrap_or(0)&0x10000!=0 {
                let h=self.heights[layer];let scale=(self.template.float(20)? as f64*f64::from_bits(0x3f9eb851e0000000)*(1.0-layer as f64/self.layers as f64)) as f32;
                let a=(time as f64*f64::from_bits(0x4008ccccc0000000)+h as f64*f64::from_bits(0x3fb1eb8520000000)) as f32;
                let b=(time as f64*f64::from_bits(0x40059999a0000000)+h as f64*f64::from_bits(0x3fc0a3d700000000)) as f32;
                let wave=|x:f32,y:f32|(x as f64+y as f64*f64::from_bits(0x3fd3333333333333)) as f32;
                offset.x=bottom*scale*h*wave(a.sin(),b.sin());offset.z= -bottom*scale*h*wave(a.cos(),b.cos());
            }
            for i in 0..=self.sides {
                let mut p=[0.0f64;2];for (weight,control) in self.weights[i].iter().zip(controls) {p[0]+=weight*control[0];p[1]+=weight*control[1];}
                let p=Vec3::new(p[0] as f32,0.0,-(p[1] as f32));let index=layer*(self.sides+1)+i;
                if layer!=0 {self.facing[index]=look.dot(p).abs().min(1.0).powi(2);}
                self.rings[index]=Vec3::new(p.x*size*radius,self.heights[layer]*size,p.z*size*radius)+offset;
            }
        }
        let per=(self.sides+1)*2;let mut out=[Vec::with_capacity(per*self.layers),Vec::with_capacity(per*self.layers)];
        let (sin,cos)=time.sin_cos();let second_scale=f64::from_bits(0x3fe6666660000000) as f32;
        let vscroll=(time as f64*V_SCROLL) as f32;
        let uscale=self.template.float(15)?;let vscale=self.template.float(16)?;
        let swapped=self.template.word(0).unwrap_or(0)&0x100!=0;
        let uvstep=if swapped {vscale}else{uscale}/self.sides as f32;
        for layer in 0..self.layers {
            let mut uvphase=0.0;
            for i in 0..=self.sides {
                for side in 0..2 {
                    let ring=layer+side;let index=ring*(self.sides+1)+i;let p=self.rings[index];
                    let alpha=((self.colors[ring][0][3]*self.facing[index]) as u32&255) as f32/255.0;
                    let mut color=self.colors[ring][0];color[3]=alpha;
                    let uv=if !swapped {[time+uvphase,ring as f32*vscale+vscroll]}else{[time+side as f32*uscale,uvphase+vscroll]};
                    out[0].push(Vertex {pos:(self.position+p).to_array(),uv,color,..Vertex::default()});
                    let x=p.x*second_scale;let z= -p.z*second_scale;
                    let p=Vec3::new((sin*x-cos*z)*second_scale,p.y,-(sin*z+cos*x)*second_scale);
                    color=self.colors[ring][1];color[3]=alpha;
                    out[1].push(Vertex {pos:(self.position+p).to_array(),uv,color,..Vertex::default()});
                }
                uvphase+=uvstep;
            }
        }
        Ok(Some(out.into()))
    }
}
const V_SCROLL:f64=-3.0; // GC double1016d1d0=c008000000000000.
fn basis(i:usize,order:usize,t:f64)->f64 {
    const K:[f64;13]=[0.0,0.0,0.0,0.0,1.0,2.0,3.0,4.0,5.0,6.0,6.0,6.0,6.0];
    if order==1 {return if K[i]<=t&&t<K[i+1] {1.0}else{0.0};}
    let a=K[i+order-1]-K[i];let b=K[i+order]-K[i+1];
    (if a==0.0 {0.0}else{(t-K[i])/a*basis(i,order-1,t)})+(if b==0.0 {0.0}else{(K[i+order]-t)/b*basis(i+1,order-1,t)})
}
fn controls(time:f32)->[[f64;2];9] {
    let sine=|speed:f32|(speed*time).sin() as f64;
    let cosine=|speed:f32|(speed*time).cos() as f64;
    let amplitude=0.4f32 as f64;let half=0.5f64;let root=f64::from_bits(0x3febb67a00000000);
    let bump=|speed:f32|(0.3f32 as f64+0.99999994f32 as f64*(sine(speed)*half+half)) as f32 as f64;
    let mut c=[[0.0;2];9];
    c[0]=[1.0+sine(1.9)*amplitude,cosine(1.9)*amplitude];
    let a=bump(1.3);c[1]=[a*half,a*root];
    c[2]=[sine(1.7)*amplitude-half,cosine(1.7)*amplitude+root];
    c[3]=[-bump(3.1),0.0];
    c[4]=[sine(2.1)*amplitude-half,cosine(2.1)*amplitude-root];
    let a=bump(2.7);c[5]=[a*half,-a*root];
    c[6]=c[0];c[7]=c[1];c[8]=c[2];c
}

#[cfg(test)]
mod tests {
    use super::*;
    fn authored()->Template {Template {kind:3014,words:vec![0x203,0,0,0,0,0,0,0,0xbf800000,48,6,0x3f99999a,0x48afc800,1,30,0x40800000,0x3e4ccccd,0x40800000,0x40400000,0x3dcccccd,0x3f800000,0x3ca3d70a,0x3dcccccd,0,0x7ed6,0xff007ed6,0x7ed6,0xff00c27e,0,0]}}
    fn fixture_view(n:&Vein)->(Vec3,Vec3) {
        let height=n.heights.iter().copied().map(f32::abs).fold(0.0,f32::max);
        let bottom=n.template.float(18).unwrap().abs();
        let radial=bottom.max(n.template.float(19).unwrap().abs())*1.4*2.0f32.sqrt();
        let sway=if n.template.word(0).unwrap()&0x10000!=0 {
            bottom*n.template.float(20).unwrap().abs()*0.03*height*2.0*2.0f32.sqrt()
        } else {0.0};
        // Cubic points stay in their control hull; include both rotated passes.
        let radius=Vec3::new(radial+sway,height,radial+sway).length().max(1.0);
        let mut distance=radius*2.4;
        // DS grows distant geometry logarithmically; frame that native growth too.
        for _ in 0..16 {
            let scale=if distance>100.0 {(distance/f64::from_bits(0x404264db5a52cb99) as f32).ln().max(1.0)}else{1.0};
            distance=distance.max(radius*scale*2.4);
        }
        (n.position+Vec3::new(2.0,2.0,5.0).normalize()*distance,n.position)
    }
    #[test]
    fn vein_fixture_camera_contains_authored_geometry()->Result<()> {
        let mut n=Vein::new(&authored(),Mat4::IDENTITY,EffectConfig::default())?;
        let (eye,target)=fixture_view(&n);
        for time in [0.0,0.25,0.5,1.0,2.0] {
            for v in n.vertices(time,eye,Vec3::X,Vec3::Y)?.unwrap().iter().flatten() {
                assert!(Vec3::from_array(v.pos).distance(target)<eye.distance(target)/2.4+0.001);
            }
        }
        Ok(())
    }
    #[test]
    fn authored_12201_vein_cubic_periodicity_and_two_passes()->Result<()> {
        let mut n=Vein::new(&authored(),Mat4::IDENTITY,EffectConfig::default())?;
        assert_eq!(n.weights[0],n.weights[30]);
        for row in &n.weights {assert!((row.iter().sum::<f64>()-1.0).abs()<1e-12);}
        let c=controls(0.0);assert_eq!(c[0],c[6]);assert_eq!(c[1],c[7]);assert_eq!(c[2],c[8]);
        assert_eq!(basis(0,4,0.0),1.0);assert_eq!(basis(8,4,0.0),0.0);
        assert_eq!(n.models()[0].2,372);assert_eq!(n.models()[0].1.len(),1080);
        let v=n.vertices(0.25,Vec3::new(2.0,2.0,5.0),Vec3::X,Vec3::Y)?.unwrap();
        assert_eq!(v.len(),2);assert_eq!(v[0].len(),372);
        assert!(v.iter().flatten().all(|v|v.pos.iter().all(|x|x.is_finite())));
        assert_eq!(v[0][0].color[3],0.0);assert_eq!(v[0][0].uv[0],0.25);assert_eq!(v[0][0].uv[1],-0.75);
        n.graceful();assert!(n.vertices(0.5,Vec3::Z,Vec3::X,Vec3::Y)?.is_none());
        Ok(())
    }
    #[test]
    #[ignore="installed retail assets and offscreen Metal rendering"]
    fn retail_vein3014_frames()->Result<()> {
        use super::super::MODEL_BASE;
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).unwrap_or_else(||"/tmp/FxClasses/frames".into());std::fs::create_dir_all(&out)?;
        let mut renderer=Renderer::open(&ao_gui::client_dir())?;
        let origin=Vec3::new(5000.0,10.0,5000.0);
        let mut host=ao_render::Host::headless();
        for id in [12200,12201,12202,12203,12206,96002,96003,96004,96005] {
            let fixture=Vein::new(&renderer.templates.by_id[&id],Mat4::from_translation(origin),EffectConfig::default())?;
            let (eye,target)=fixture_view(&fixture);
            host.camera=ao_render::Camera::look_at(eye,target);
            renderer.clear();renderer.spawn(Binding {group:0,attractor:0,effect:id,note:0,color:0},Creation::Vector,Mat4::from_translation(origin),origin)?;
            for frame in 1..=120 {
                host.actors.clear();let mut terrain=|p:Vec3|Some((Vec3::new(p.x,origin.y,p.z),Vec3::Y));
                renderer.frame(1.0/60.0,&mut host,Some(&mut terrain));
                if [1,15,30,60,120].contains(&frame) {
                    let models:Vec<_>=renderer.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())).collect();
                    ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,host.actors.clone(),eye.to_array(),target.to_array(),640,480,&out.join(format!("vein3014_{id}_{frame}.png")),frame as f32/60.0)?;
                }
            }
        }
        Ok(())
    }
}
