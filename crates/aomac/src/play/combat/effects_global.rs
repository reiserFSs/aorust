//! Native global controls: GC ctors 100d37ed/10104a94/10104d5c/101158d8.
//! Process 100d3494/101046c8/10104b44/10115394; DS 10023929/1002eec0.
use super::Template;
use anyhow::{ensure, Result};
use ao_render::ScreenLayer;

pub(super) struct TintLayer {
    pub material: i32,
    pub flags: u32,
    pub scale: [f32;2],
    pub offset: [f32;2],
    velocity: [f32;2],
    pub layer: ScreenLayer,
}
pub(super) struct GlobalEffect {
    kind:i32,
    duration:f32,
    elapsed:f32,
    delay:f32,
    fade:f32,
    ticks:u32,
    terminating:bool,
    fog:Option<ao_render::NativeFogControl>,
    pub layers:Vec<TintLayer>,
}
fn byte(v:f32)->f32 { ((v*255.0-0.49999).round_ties_even() as i32 as u8) as f32/255.0 }
fn layer(color:[f32;4],source_blend:u32,destination_blend:u32,repetitions:u32)->ScreenLayer {
    ScreenLayer {color,source_blend,destination_blend,repetitions,..Default::default()}
}
impl GlobalEffect {
    pub(super) fn new(t:&Template)->Result<Self> {
        ensure!(matches!(t.kind,1000|1014|1015|3037),"not a global control");
        // CMSBlock float getter10106893 returns zero for absent parameters.
        let get=|i|t.float(i);
        let mut layers=Vec::new();
        if t.kind==3037 {
            let count=t.word(0)? as usize;
            ensure!(count<=t.words.len().saturating_sub(1)/11,"short VisionTint record");
            for i in 0..count {
                let at=1+i*11;
                let color=t.word(at+6)?.to_be_bytes();
                let source=t.word(at+8)?;let destination=t.word(at+9)?;
                ensure!((1..=13).contains(&source)&&(1..=13).contains(&destination),"invalid tint blend");
                // at+10 is authored but never consumed by init or process.
                t.word(at+10)?;
                layers.push(TintLayer {material:t.word(at+1)? as i32,flags:t.word(at)?,scale:[t.float(at+2)?,t.float(at+3)?],offset:[0.0;2],velocity:[t.float(at+4)?,t.float(at+5)?],layer:layer([color[1] as f32/255.0,color[2] as f32/255.0,color[3] as f32/255.0,color[0] as f32/255.0],source,destination,(t.word(at+7)? as i32).max(0) as u32)});
            }
        }
        Ok(Self {kind:t.kind,duration:-1.0,elapsed:0.0,delay:if t.kind==1014{get(1)?}else{0.0},fade:if matches!(t.kind,1014|1015){get(0)?}else{0.0},ticks:0,terminating:false,layers,fog:None})
    }
    pub(super) fn attach_fog(&mut self,state:ao_render::SharedNativeFog){if self.kind==3037 {self.fog=Some(ao_render::NativeFogControl::new(state,false,true));}}
    pub(super) fn set_duration(&mut self,duration:f32){self.duration=duration;}
    pub(super) fn terminate(&mut self){
        if self.kind==1014 {if self.delay!=0.0 {self.delay=0.0;self.elapsed=0.0;}}
        else {self.terminating=true;}
    }
    pub(super) fn screen_tints(&self,host:&mut ao_render::Host,random:&mut ao_formats::weather::R250) {
        for tint in &self.layers {
            let mut layer=tint.layer;
            let mut offset=tint.offset;
            if tint.flags&0x400!=0 {for v in &mut offset {*v+=super::random_fraction(random)-0.5;}}
            let mut span=tint.scale;
            if tint.flags&0x800!=0 {offset[1]+=span[1];span[1]= -span[1];}
            layer.uv=[offset[0],offset[1],span[0],span[1]];
            host.screen_layers.push(layer);
        }
    }
    pub(super) fn frame(&mut self,dt:f32,host:&mut ao_render::Host)->bool {
        if self.terminating{return false;}
        let step=match self.ticks {0=>0.0,1=>dt.abs().min(0.033),_=>dt.abs()};
        self.ticks=self.ticks.saturating_add(1);self.elapsed+=step;
        if self.duration>0.0&&self.elapsed>self.duration{return false;}
        match self.kind {
            1000=>{
                let r=self.elapsed.clamp(0.0,1.0);let g=r;let b=(self.elapsed-0.25).clamp(0.0,1.0);
                host.screen_layers.push(layer([byte(0.5),byte(0.5),byte((r+1.0)*0.5),1.0],9,3,2));
                host.screen_layers.push(layer([byte((1.0-r)*0.5),byte((1.0-g)*0.5),byte((1.0-b)*0.5),1.0],9,3,1));
            }
            1014=>{
                self.delay-=step;if self.delay>0.0 {self.elapsed=0.0;}
                let progress=if self.elapsed<=self.fade {self.elapsed/self.fade}else{1.0};
                let f=(1.0-progress)*(1.0-progress)*1.25;
                let rg=byte(f.min(0.5)+0.5);let b=byte((f-0.125).clamp(0.0,0.5)+0.5);
                host.screen_layers.push(layer([rg,rg,b,1.0],9,3,4));
                let white=byte((f-0.25).clamp(0.0,1.0));host.screen_layers.push(layer([white,white,white,1.0],5,2,1));
                if self.elapsed>self.fade{return false;}
            }
            1015=>host.screen_layers.push(layer([1.0,1.0,1.0,byte((self.elapsed/self.fade).clamp(0.0,1.0))],5,6,1)),
            3037=>{
                if let Some(fog)=&mut self.fog {fog.process();}
                for tint in &mut self.layers {for i in 0..2 {tint.offset[i]+=tint.velocity[i]*step;}}
            }
            _=>unreachable!(),
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn authored_global_fades() {
        for (kind,words) in [(1000,vec![]),(1015,vec![]),(1014,vec![2f32.to_bits(),(-1f32).to_bits()])] {
            let mut effect=GlobalEffect::new(&Template{kind,words}).unwrap();
            let mut host=ao_render::Host::headless();assert!(effect.frame(1.0,&mut host));
            assert_eq!(host.screen_layers.len(),if kind==1015{1}else{2});
            effect.terminate();if kind!=1014 {assert!(!effect.frame(0.1,&mut host));}
        }
    }
    #[test]
    fn authored_tint_layers_and_uv_motion() {
        // Installed records71121/71122, copied as CMSBlock words.
        for (count,material,words) in [
            (3,13,vec![3,0,4294967295,1065353216,1065353216,0,0,4289396650,2,9,5,5,1024,13,1082130432,1082130432,0,0,279642026,1,5,6,4,0,4294967295,1065353216,1065353216,0,0,1353383850,1,5,6,4]),
            (5,58,vec![5,0,4294967295,1065353216,1065353216,0,0,4292730282,2,9,5,5,0,58,1077936128,1077936128,1050253722,1036831949,551411114,1,5,6,4,0,58,1077936128,1077936128,3201092813,1045220557,551411114,1,5,6,4,0,58,1077936128,1077936128,1036831949,3197737370,551411114,1,5,6,4,0,4294967295,1065353216,1065353216,0,0,1356717482,1,5,6,4]),
        ] {
            let mut effect=GlobalEffect::new(&Template{kind:3037,words}).unwrap();
            let mut host=ao_render::Host::headless();
            assert!(effect.frame(0.1,&mut host));assert!(effect.frame(0.1,&mut host));
            assert_eq!(effect.layers.len(),count as usize);
            assert_eq!(effect.layers[1].material,material);
            // Native NoFog changes hardware enable, not saved logical mode.
            assert_eq!(effect.layers[0].offset,[0.0;2]);
            if count==5 {assert!((effect.layers[1].offset[0]-0.0099).abs()<1e-6);}
            effect.terminate();assert!(!effect.frame(0.1,&mut host));
        }
    }
    #[test]
    #[ignore="installed authored records and offscreen Metal rendering"]
    fn retail_global_frames() {
        let dir=ao_gui::client_dir();
        let table=super::super::Templates::open(&dir).unwrap();
        let store=ao_rdb::RecordStore::open(&dir).unwrap();
        let names=ao_formats::character::NameTable::load(&store).unwrap();
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).expect("AOMAC_EFFECT_FRAMES");
        std::fs::create_dir_all(&out).unwrap();
        for (&id,t) in table.by_id.iter().filter(|(_,t)|matches!(t.kind,1000|1014|1015|3037)) {
            let mut e=GlobalEffect::new(t).unwrap();let mut host=ao_render::Host::headless();
            let fog=std::rc::Rc::new(std::cell::Cell::new(ao_render::NativeFog::default()));e.attach_fog(fog.clone());
            let mut scene=ao_scene::Scene::default();
            for tint in &mut e.layers {
                if tint.material<0 {continue;}
                let &(name,cols,rows,_,_)=super::super::materials::MATERIALS.get(tint.material as usize).unwrap();
                let key=ao_scene::TextureKey{rdb_type:1010004,id:names.id(1010004,name).unwrap()};
                tint.layer.texture=Some(key);tint.scale[0]/=cols as f32;tint.scale[1]/=rows as f32;
                scene.textures.insert(key,ao_formats::texture::load_texture(&store,key).unwrap().unwrap());
            }
            let mut random=ao_formats::weather::R250::new(0xe6f1);
            for _ in 0..20 {host.screen_layers.clear();if e.frame(0.05,&mut host){e.screen_tints(&mut host,&mut random);}}
            ao_render::render_to_png_screen_fog(&scene,host.screen_layers,fog.get(),640,480,&out.join(format!("global_{id}.png"))).unwrap();
        }
    }
}
