//! GC1010eeef / N3 10007fe5,1000806d,10008085,10008021:
//! one n3GenericEffect per authored ABIFF eff_ connector.
use super::{Binding,Creation,EffectConfig,Renderer};
use anyhow::{Context,Result};
use ao_formats::mesh::MeshEffectAttr;
use glam::Mat4;
/// Cache this once by actual ABIFF record identity, never by an effect selector.
pub(super) fn load_attrs(store:&ao_rdb::RecordStore,record:u32)->Result<Vec<MeshEffectAttr>> {
    let bytes=store.get(ao_formats::mesh::MESH_TYPE,record)?.with_context(||format!("missing mesh effect ABIFF {record}"))?;
    ao_formats::mesh::mesh_effect_attrs(&bytes)
}

#[derive(Default)]
pub(super) struct MeshChildren { children:Vec<(MeshEffectAttr,u32)>,visible:bool }
impl MeshChildren {
    pub(super) fn new(attrs:&[MeshEffectAttr])->Self {
        Self {children:attrs.iter().cloned().map(|a|(a,0)).collect(),visible:false}
    }
    /// Supply actual sampled frame transforms; missing connectors are errors.
    /// Called after the parent mesh's animated frames have been sampled.
    pub(super) fn frame(&mut self,renderer:&mut Renderer,visible:bool,mut connector:impl FnMut(&MeshEffectAttr)->Option<Mat4>)->Result<()> {
        if !visible {
            if self.visible {for (_,handle) in &self.children {if *handle!=0 {renderer.set_enabled(*handle,false);}}}
            self.visible=false;
            return Ok(());
        }
        let enable=!self.visible;
        for (attr,handle) in &mut self.children {
            let matrix=connector(attr).context("missing authored mesh effect connector frame")?;
            if enable {
                // GC100cdb89 reuses a live disabled handle; it recreates only after
                // the native control has actually expired, not every visible frame.
                if *handle==0 || !renderer.is_active(*handle) {
                    *handle=renderer.spawn_configured(Binding {group:0,attractor:0,effect:attr.effect,note:0,color:0},matrix,matrix.w_axis.truncate(),EffectConfig {creation:Creation::RConnector,resource_connector:Some(matrix),start_color:attr.color,stop_color:attr.color,scale:attr.size,..Default::default()})?;
                }
                renderer.set_enabled(*handle,true);
            }
            if *handle!=0 {renderer.update_source(*handle,matrix);}
        }
        self.visible=visible;Ok(())
    }
    /// N3 destructor deletes even a disabled child; no orphaned looping effects.
    pub(super) fn delete(&mut self,renderer:&mut Renderer) {
        for (_,handle) in &mut self.children {if *handle!=0 {renderer.delete(*handle);*handle=0;}}
        self.visible=false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_mesh_children_start_disabled_without_handles() {
        let attr=ao_formats::mesh::parse_effect_name("eff_universal_72106",3).unwrap().unwrap();
        let children=MeshChildren::new(&[attr]);assert!(!children.visible);assert_eq!(children.children[0].1,0);
        assert_eq!(children.children[0].0.effect,72106);assert_eq!(children.children[0].0.frame,3);
    }
    #[test]
    #[ignore="installed3025/3027 mesh children and offscreen GPU; AOMAC_EFFECT_FRAMES required"]
    fn authored_mesh_effect_children_frames()->Result<()> {
        use ao_formats::mesh::{mesh_effect_attrs,load_animated_mesh,MESH_TYPE};
        use super::super::MODEL_BASE;
        let out=std::path::PathBuf::from(std::env::var_os("AOMAC_EFFECT_FRAMES").context("AOMAC_EFFECT_FRAMES required")?);
        std::fs::create_dir_all(&out)?;let mut renderer=Renderer::open(&ao_gui::client_dir())?;
        let templates:Vec<_>=renderer.templates.by_id.iter().filter(|(_,t)|matches!(t.kind,3025|3027)).map(|(&id,t)|(id,t.clone())).collect();
        let mut ids=Vec::new();
        for (id,t) in templates {
            if t.kind==3025 {
                let mut mesh=super::super::tracer_meshes::TracerMesh::new(&t,&renderer.store,&renderer.names,Mat4::IDENTITY,&mut renderer.mesh_resources,EffectConfig::default())?;
                if let Some(record)=mesh.record_id() {
                    ids.push((id,record));
                } else {
                    assert_eq!(id,71123,"unexpected installed mesh name hole");
                    assert!(mesh.scenes().is_empty());
                    assert!(!mesh.frame(1.0/60.0)?,"failed native name lookup terminates control");
                }
            } else {
                let mesh=super::super::legacy302x::MParticle::new(&t,Mat4::IDENTITY,&renderer.store,&renderer.names,&mut renderer.rng,&mut renderer.random)?;
                ids.extend(mesh.resources().iter().map(|(mesh,_)|(id,*mesh)));
            }
        }
        let mut exercised=[false;2];
        for (id,mesh) in ids {
            let bytes=renderer.store.get(MESH_TYPE,mesh)?.context("missing authored child-bearing mesh payload")?;
            let attrs=mesh_effect_attrs(&bytes)?;if attrs.is_empty(){continue}
            let (scene,mut rig)=match load_animated_mesh(&renderer.store,mesh)? {
                Some((scene,rig))=>(scene,Some(rig)),
                None=>(ao_formats::mesh::load_mesh(&renderer.store,mesh)?,None),
            };
            let kind=renderer.templates.by_id[&id].kind;exercised[usize::from(kind==3027)]=true;
            let mut children=MeshChildren::new(&attrs);let mut parts=Vec::new();
            let origin=glam::Vec3::new(5000.0,10.0,5000.0);let world=Mat4::from_translation(origin);let eye=origin+glam::Vec3::new(4.0,3.0,8.0);
            let mut host=ao_render::Host::headless();host.camera=ao_render::Camera::look_at(eye,origin);
            for frame in 1..=30 {
                if let Some(rig)=&mut rig {rig.pose_parts(frame as f32/60.0,&mut parts);}
                children.frame(&mut renderer,true,|attr|Some(world*Mat4::from_cols_array_2d(&rig.as_ref().and_then(|rig|rig.frame_transform(attr.frame)).unwrap_or(attr.transform))))?;
                host.actors.clear();host.actors.push(ao_scene::ActorFrame {id:1,model:1,transform:world.to_cols_array_2d(),parts:parts.clone(),..Default::default()});
                renderer.frame(1.0/60.0,&mut host,None);
                if [1,15,30].contains(&frame) {
                    let mut models=vec![(1,scene.clone())];models.extend(renderer.models.iter().map(|(&id,m)|(MODEL_BASE|id as u32 as u64,m.scene.clone())));
                    ao_render::render_to_png_actors(&ao_scene::Scene::default(),&models,host.actors.clone(),eye.to_array(),origin.to_array(),640,480,&out.join(format!("meshchildren_{kind}_{id}_{frame}.png")),frame as f32/60.0)?;
                }
            }
            let handles:Vec<_>=children.children.iter().map(|(_,h)|*h).collect();children.delete(&mut renderer);
            assert!(handles.into_iter().all(|h|!renderer.is_active(h)));
        }
        anyhow::ensure!(exercised.iter().all(|x|*x),"missing authored child-bearing3025/3027 fixtures");Ok(())
    }
}
