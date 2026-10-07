//! DS VisualFog10058011/10058015/10058409; GC1020/3037 save/restore.
use std::{cell::Cell,rc::Rc};
#[derive(Clone,Copy,Debug,PartialEq)]
pub struct NativeFog {pub mode:i32,pub weight:f32,pub enabled:bool}
impl Default for NativeFog {fn default()->Self {Self {mode:3,weight:1.0,enabled:true}}}
impl NativeFog {
    pub fn set_mode(&mut self,mode:i32) {
        self.mode=if mode==0{3}else{mode};self.weight=if mode==0{0.1}else{1.0};
        // SetFogMode leaves hardware enable unchanged. DS process calls
        // Randy10041876, which always enables linear fog.
    }
    pub fn process(&mut self){if self.mode!=0 {self.enabled=true;}}
    pub fn no_fog(&mut self){self.enabled=false;}
}
pub type SharedNativeFog=Rc<Cell<NativeFog>>;
pub struct NativeFogControl {state:SharedNativeFog,saved:i32,nightvision:bool}
impl NativeFogControl {
    pub fn new(state:SharedNativeFog,nightvision:bool,initialize:bool)->Self {
        let mut saved=0;
        if initialize {let mut fog=state.get();saved=fog.mode;if nightvision {fog.set_mode(0);}else{fog.no_fog();}state.set(fog);}
        Self {state,saved,nightvision}
    }
    pub fn process(&mut self){
        let mut fog=self.state.get();
        if fog.mode!=0 {self.saved=fog.mode;if self.nightvision{fog.set_mode(0);}else{fog.no_fog();}self.state.set(fog);}
    }
}
impl Drop for NativeFogControl {
    fn drop(&mut self){let mut fog=self.state.get();fog.set_mode(self.saved);self.state.set(fog);}
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_overlap_preference_and_destructor_order() {
        let state=Rc::new(Cell::new(NativeFog::default()));
        let mut night=NativeFogControl::new(state.clone(),true,true);assert_eq!(state.get().weight,0.1);
        let mut tint=NativeFogControl::new(state.clone(),false,true);assert!(!state.get().enabled);
        let mut fog=state.get();fog.set_mode(2);state.set(fog);
        night.process();tint.process();
        drop(night);assert_eq!(state.get().mode,2);
        drop(tint);assert_eq!(state.get().mode,3);assert_eq!(state.get().weight,1.0);
        assert!(!state.get().enabled);
        let mut fog=state.get();fog.process();state.set(fog);assert!(state.get().enabled);
        let mut host=crate::Host::headless();host.bind_native_fog(state.clone());host.set_native_fog_preference(1);
        let mut night=NativeFogControl::new(state.clone(),true,true);
        host.set_native_fog_preference(1);assert_eq!(state.get().weight,0.1);
        let model=ao_scene::FogModel{base_density:1.0,near:0.1,far:20.0,..Default::default()};
        assert!((model.at_scaled([0.0;3],state.get().weight).1-18.51).abs()<1e-5);
        host.set_native_fog_preference(2);night.process();drop(night);
        assert_eq!(state.get().mode,2);assert_eq!(state.get().weight,1.0);
        let mut tint=NativeFogControl::new(state.clone(),false,true);
        let mut night=NativeFogControl::new(state.clone(),true,true);
        tint.process();night.process();drop(tint);assert_eq!(state.get().mode,3);
        drop(night);assert_eq!(state.get().mode,3);
    }
    #[test]
    #[ignore="offscreen Metal native fog mode regression"]
    fn native_fog_mode_frames() {
        use ao_scene::{Scene,Mesh,Vertex,Submesh,Instance,IDENTITY,FogModel};
        let out=std::env::var_os("AOMAC_EFFECT_FRAMES").map(std::path::PathBuf::from).expect("AOMAC_EFFECT_FRAMES");
        std::fs::create_dir_all(&out).unwrap();
        let vertices=[(-2.0,-2.0),(2.0,-2.0),(2.0,2.0),(-2.0,2.0)].into_iter().map(|(x,y)|Vertex{pos:[x,y,-8.0],normal:[0.0,0.0,1.0],..Default::default()}).collect();
        let scene=Scene {meshes:vec![Mesh{vertices,submeshes:vec![Submesh{two_sided:true,emissive:[1.0,0.0,0.0],..Submesh::new(vec![0,1,2,0,2,3],None)}]}],instances:vec![Instance{mesh:0,transform:IDENTITY}],fog_model:Some(FogModel{base_color:[0.0,0.0,1.0],base_density:1.0,near:0.1,far:20.0,..Default::default()}),environment:Some(ao_scene::Environment{sky_color:[0.0;3],fog_color:[0.0,0.0,1.0],fog_start:0.1,fog_end:20.0,ambient:[0.0;3],sun_color:[0.0;3],sun_dir:[0.0,0.0,1.0],sun_specular:0.0}),..Default::default()};
        for mode in 0..=3 {
            let mut fog=NativeFog::default();fog.set_mode(mode);fog.process();
            crate::render_to_png_screen_fog(&scene,vec![],fog,640,480,&out.join(format!("native_fog_mode{mode}.png"))).unwrap();
        }
        let mut fog=NativeFog::default();fog.no_fog();
        crate::render_to_png_screen_fog(&scene,vec![],fog,640,480,&out.join("native_fog_disabled.png")).unwrap();
    }
}
