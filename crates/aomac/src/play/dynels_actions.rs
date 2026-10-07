//! Server-requested authored effects; retail overloads are documented in misc.md §16.
use super::{Dynels, CHAR_KIND, scene_pos};
use crate::play::combat::effects::{Binding, Creation, EffectConfig};
use ao_net::{msg::Identity, n3::effects::{Effects, GfxTrigger, Placement, PlaySound}};
use glam::{Mat4, Vec3};

struct Applied {
    source: Mat4,
    target: Vec3,
    source_identity: Option<Identity>,
    target_identity: Option<Identity>,
    argument: i32,
    form: u8,
}

fn placement(request: &GfxTrigger, mut actor: impl FnMut(Identity) -> Option<Mat4>) -> Option<Applied> {
    let vector = |p: &ao_net::n3::misc::Vec3| Vec3::from(scene_pos([p.x,p.y,p.z]));
    let character = |who: Identity, actor: &mut dyn FnMut(Identity) -> Option<Mat4>| {
        (who.kind == CHAR_KIND).then(|| actor(who)).flatten()
    };
    let mut out = Applied { source: Mat4::IDENTITY, target: Vec3::ZERO, source_identity: None, target_identity: None, argument: 0, form: 0 };
    match &request.placement {
        Placement::Position(p) => { out.source=Mat4::from_translation(vector(p)); out.target=vector(p); out.form=1; }
        Placement::Character { character: who, argument } => {
            out.source=character(*who,&mut actor)?; out.target=out.source.w_axis.truncate(); out.source_identity=Some(*who); out.argument = *argument; out.form=2;
        }
        Placement::Positions { source, target } => { out.source=Mat4::from_translation(vector(source)); out.target=vector(target); out.form=3; }
        Placement::PositionToCharacter { character: who, source } => {
            out.source=Mat4::from_translation(vector(source)); out.target=character(*who,&mut actor)?.w_axis.truncate(); out.target_identity=Some(*who); out.form=4;
        }
        Placement::CharacterToPosition { character: who, target, argument } => {
            out.source=character(*who,&mut actor)?; out.target=vector(target); out.source_identity=Some(*who); out.argument = *argument; out.form=5;
        }
        Placement::Characters { source, argument, .. } => {
            out.source=character(*source,&mut actor)?;
            // GC1003982e resolves +20 twice, not the separately decoded +28 identity.
            out.target=character(*source,&mut actor)?.w_axis.truncate();
            out.source_identity=Some(*source); out.target_identity=Some(*source); out.argument = *argument; out.form=6;
        }
        Placement::Unknown(_) => return None,
    }
    Some(out)
}

impl Dynels {
    /// Apply after the own avatar has animated connectors, before refreshing effect anchors.
    pub fn server_effect_frame(&mut self, mut own_anchor: impl FnMut(i32) -> Option<Mat4>, mut stat: impl FnMut(i32,u32) -> Option<i32>) -> Vec<PlaySound> {
        let mut named = Vec::new();
        let mut renderer=self.effects.take();
        for request in std::mem::take(&mut self.pending_effects) {
            let Effects::GfxTrigger(request)=request else {
                if let Effects::PlaySound(sound)=request { named.push(sound); }
                continue;
            };
            if request.effect==49999 { continue; }
            let Some(renderer)=renderer.as_mut() else { continue };
            let Some(applied)=placement(&request, |who| {
                let c=self.chars.get(&who.instance)?;
                Some(Mat4::from_scale_rotation_translation(Vec3::splat(c.scale),glam::Quat::from_rotation_y(super::scene_yaw(c.pose.yaw)),Vec3::from(scene_pos(c.pose.pos))))
            }) else { continue };
            let kind=renderer.effect_kind(request.effect);
            if !match applied.form { 3=>matches!(kind,Some(1011|3007)),4=>kind==Some(1011),5|6=>kind==Some(1010),_=>true } { continue; }
            let identity=|who: Identity| (who.kind as u32,who.instance as u32);
            for who in [applied.source_identity,applied.target_identity].into_iter().flatten() {
                let mut resolve=|_: (u32,u32),id| if who.instance==self.own { own_anchor(id) } else { self.effect_anchor(who.instance,id,0) };
                renderer.prepare_anchors(identity(who),&mut resolve);
                if applied.argument!=0 { let matrix=resolve(identity(who),applied.argument); renderer.prepare_anchor(identity(who),applied.argument,matrix); }
            }
            let appearance=|who: Option<Identity>, stat: &mut dyn FnMut(i32,u32)->Option<i32>| -> Option<[i32;4]> {
                let who=who?; Some([stat(who.instance,4)?,stat(who.instance,59)?,stat(who.instance,47)?,stat(who.instance,360)?])
            };
            let config=EffectConfig {
                creation: match applied.form {1=>Creation::Vector,2=>Creation::Dynel,3=>Creation::VectorVector,4=>Creation::VectorDynel,5=>Creation::DynelVector,6=>Creation::DynelDynel,_=>unreachable!()},
                source_identity:applied.source_identity.map(identity),target_identity:applied.target_identity.map(identity),
                source_attractor:(applied.argument!=0).then_some(applied.argument),
                source_appearance:appearance(applied.source_identity,&mut stat),target_appearance:appearance(applied.target_identity,&mut stat),
                track_source:applied.source_identity.is_some(),..EffectConfig::default()
            };
            let attractor=if kind==Some(1010) && applied.argument==0 {2001} else {applied.argument};
            let binding=Binding {group:0,attractor,effect:request.effect,note:0,color:0};
            if let Err(error)=renderer.spawn_configured(binding,applied.source,applied.target,config) { eprintln!("server effect: {error:#}"); }
        }
        self.effects=renderer;
        named
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::{n3::{N3Header,effects},wire::{Reader,Writer}};
    #[test]
    fn decoded_layout_application_source_twice_and_missing_actor() {
        let source=Identity {kind:CHAR_KIND,instance:42};
        let target=Identity {kind:CHAR_KIND,instance:43};
        let mut wire=Writer::default(); wire.i32(6); wire.i32(12000); source.write(&mut wire); target.write(&mut wire); wire.i32(2007);
        let header=N3Header {msg_type:effects::GFX_TRIGGER,target,flag:0};
        let Some(Effects::GfxTrigger(request))=effects::decode(&header,&mut Reader::new(&wire.0)).unwrap() else { panic!("gfx") };
        let mut lookups=Vec::new();
        let applied=placement(&request,|who| {lookups.push(who); Some(Mat4::from_translation(Vec3::X))}).unwrap();
        assert_eq!(lookups,[source,source]); assert_eq!(applied.target_identity,Some(source)); assert_eq!(applied.argument,2007);
        assert!(placement(&request,|_| None).is_none());
        assert!(placement(&GfxTrigger {effect:1,placement:Placement::Unknown(7)},|_| panic!("unknown must not resolve")).is_none());
        for form in 1..=5 {
            let mut wire=Writer::default(); wire.i32(form); wire.i32(12000);
            let vector=|wire:&mut Writer| { wire.f32(1.0); wire.f32(2.0); wire.f32(3.0); };
            match form {
                1=>vector(&mut wire),
                2=>{source.write(&mut wire);wire.i32(1007);},
                3=>{vector(&mut wire);wire.f32(4.0);wire.f32(5.0);wire.f32(6.0);},
                4=>{source.write(&mut wire);vector(&mut wire);},
                5=>{source.write(&mut wire);vector(&mut wire);wire.i32(1007);},
                _=>unreachable!(),
            }
            let Some(Effects::GfxTrigger(request))=effects::decode(&header,&mut Reader::new(&wire.0)).unwrap() else { panic!("gfx") };
            let applied=placement(&request,|_| Some(Mat4::from_translation(Vec3::X))).unwrap();
            assert_eq!(applied.form,form as u8);
            assert_eq!(applied.argument,if matches!(form,2|5) {1007} else {0});
            assert_eq!(applied.source_identity,if matches!(form,2|5) {Some(source)} else {None});
            assert_eq!(applied.target_identity,if form==4 {Some(source)} else {None});
            let a=Vec3::from(scene_pos([1.0,2.0,3.0]));
            assert_eq!(applied.source.w_axis.truncate(),if matches!(form,2|5) {Vec3::X} else {a});
            assert_eq!(applied.target,match form {2|4=>Vec3::X,3=>Vec3::from(scene_pos([4.0,5.0,6.0])),_=>a});
            assert_eq!(placement(&request,|_| None).is_none(),matches!(form,2|4|5));
        }
    }
    #[test]
    fn named_signal_preserves_decoded_body_identity_without_actor_or_renderer() {
        let body_identity=Identity {kind:0x9c50,instance:77};
        let header=N3Header {msg_type:effects::PLAY_SOUND,target:Identity {kind:CHAR_KIND,instance:42},flag:0};
        let mut wire=Writer::default(); wire.i32(8); wire.bytes(b"foo.wav\0"); body_identity.write(&mut wire);
        let effects=effects::decode(&header,&mut Reader::new(&wire.0)).unwrap().unwrap();
        let mut world=Dynels::default();
        world.on_message(&ao_net::n3::Message {header,sender:0,body:ao_net::n3::N3::Effects(effects)});
        assert_eq!(world.server_effect_frame(|_| panic!("sound must not resolve connectors"),|_,_| None),
            [PlaySound {name:"foo.wav".into(),identity:body_identity}]);
        assert!(world.server_effect_frame(|_| None,|_,_| None).is_empty());
    }
}
