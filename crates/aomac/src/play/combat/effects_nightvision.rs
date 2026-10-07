//! NightVision1: GC ctor100eaddd, loader100ea6a1, init100ea813,
//! process100ea652, duration100ea694, graceful100a719a, delete100ea5c5.
//! DS sprite10023885/10023929; distortion10011597/1001165a is a render no-op.
use super::Template;
use anyhow::{ensure,Result};
use ao_render::ScreenLayer;

pub(super) struct NightVision {
    layers: Vec<ScreenLayer>,
    duration: f32,
    elapsed: f32,
    ticks: u32,
    terminating: bool,
    fog:Option<ao_render::NativeFogControl>,
}
impl NightVision {
    pub(super) fn new(template:&Template)->Result<Self> {
        ensure!(template.kind==1020 && template.words.len()>=26,"short NightVision template");
        // The loader reads all fields, including unused third-layer/distortion fields.
        for i in [1,2,3,4,8,9,10,11,15,16,17,18,24,25] {template.float(i)?;}
        let mut layers=Vec::with_capacity(3);
        for group in 0..3 {
            if (template.word(0)? as i32)<=group as i32 {break;}
            let start=1+group*7;
            let color=std::array::from_fn(|i| {
                // Native float*255 conversion and packed RGBA truncate to bytes.
                ((f32::from_bits(template.words[start+i])*255.0) as i32 as u8) as f32/255.0
            });
            let source_blend=template.word(start+5)?;
            let destination_blend=template.word(start+6)?;
            ensure!((1..=13).contains(&source_blend) && (1..=13).contains(&destination_blend),"invalid NightVision blend factors");
            let repetitions=(template.word(start+4)? as i32).max(0) as u32;
            layers.push(ScreenLayer {color,source_blend,destination_blend,repetitions,..Default::default()});
        }
        Ok(Self {layers,duration:-1.0,elapsed:0.0,ticks:0,terminating:false,fog:None})
    }
    pub(super) fn attach_fog(&mut self,state:ao_render::SharedNativeFog){self.fog=Some(ao_render::NativeFogControl::new(state,true,!self.layers.is_empty()));}
    pub(super) fn set_duration(&mut self,duration:f32) {self.duration=duration;}
    pub(super) fn terminate(&mut self) {self.terminating=true;}
    pub(super) fn frame(&mut self,dt:f32,host:&mut ao_render::Host)->bool {
        // Native1020 Process reasserts even when base Process just expired.
        if let Some(fog)=&mut self.fog {fog.process();}
        if self.terminating {return false;}
        let step=match self.ticks {0=>0.0,1=>dt.abs().min(0.033),_=>dt.abs()};
        self.ticks=self.ticks.saturating_add(1);self.elapsed+=step;
        if self.duration>0.0 && self.elapsed>self.duration {return false;}
        // SetFogMode(0) changes native mode/weight, not hardware fog enable.
        host.screen_layers.extend_from_slice(&self.layers);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn authored(id:i32)->Template {
        let one=1f32.to_bits();let half=0.5f32.to_bits();let q=0.75f32.to_bits();
        let words=match id {
            3400=>vec![2,one,0,one,0,2,9,3,one,0,one,0,1,10,1,one,half,one,0,1,0,0,0,0,0,0],
            3401=>vec![2,one,q,one,q,1,9,3,one,0,one,0,1,10,4,one,one,one,one,3,9,3,0,0,0,0],
            3410=>vec![2,one,one,one,0,2,9,3,one,one,one,0,1,10,1,one,half,one,0,1,0,0,0,0,0,0],
            3411=>vec![2,one,one,one,0,2,9,3,one,one,one,0,1,10,4,one,half,one,0,1,0,0,0,0,0,0],
            3422|3423=>vec![1,one,q,one,q,if id==3422 {2}else {3},9,3,one,one,one,0,1,10,4,one,half,one,0,1,0,0,0,0,0,0],
            3430=>vec![0,one,one,0,0,1,10,1,one,one,one,one,4,10,9,one,one,one,one,1,10,1,0,500,1022739087,1031127695],
            43652=>vec![1,one,0,0,0,1,1,1,one,0,0,0,1,3,1,one,0,0,0,1,3,1,0,0,0,0],
            43733=>vec![1,one,1065185444,0,1064682127,1,9,3,one,0,0,0,1,10,4,one,half,one,0,1,0,0,0,0,0,0],
            _=>unreachable!(),
        };
        Template {kind:1020,words}
    }
    #[test]
    fn authored_nightvision_layers_and_native_lifetime() {
        for id in [3400,3401,3410,3411,3422,3423,3430,43652,43733] {
            let template=authored(id);
            let mut effect=NightVision::new(&template).unwrap();
            assert_eq!(effect.layers.len(),template.words[0] as usize);
            for (group,layer) in effect.layers.iter().enumerate() {
                assert_eq!(layer.source_blend,template.words[6+group*7]);
                assert_eq!(layer.destination_blend,template.words[7+group*7]);
                assert_eq!(layer.repetitions,template.words[5+group*7]);
            }
            let mut host=ao_render::Host::headless();effect.set_duration(0.04);
            let fog=std::rc::Rc::new(std::cell::Cell::new(ao_render::NativeFog::default()));
            effect.attach_fog(fog.clone());
            assert!(effect.frame(1.0,&mut host));assert_eq!(fog.get().weight,0.1);assert!(fog.get().enabled);
            assert!(effect.frame(1.0,&mut host));assert!(!effect.frame(0.008,&mut host));
            let mut effect=NightVision::new(&authored(id)).unwrap();effect.terminate();assert!(!effect.frame(0.0,&mut host));
        }
        let mut bad=authored(3400);bad.words.truncate(25);assert!(NightVision::new(&bad).is_err());
    }
    #[test]
    #[ignore="installed retail configurations and offscreen Metal rendering"]
    fn retail_nightvision_frames() {
        let templates=super::super::Templates::open(&ao_gui::client_dir()).unwrap();
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).expect("AOMAC_EFFECT_FRAMES");
        std::fs::create_dir_all(&out).unwrap();
        for (&id,template) in templates.by_id.iter().filter(|(_,t)|t.kind==1020) {
            let mut effect=NightVision::new(template).unwrap();
            let fog=std::rc::Rc::new(std::cell::Cell::new(ao_render::NativeFog::default()));effect.attach_fog(fog.clone());
            let mut host=ao_render::Host::headless();effect.frame(0.0,&mut host);
            ao_render::render_to_png_screen_fog(&ao_scene::Scene::default(),effect.layers.clone(),fog.get(),640,480,&out.join(format!("nightvision_{id}.png"))).unwrap();
        }
    }
}
