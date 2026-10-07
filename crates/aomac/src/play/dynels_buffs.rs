//! GC 100512af / 100515ce: active nano entries own target-attached category-4 effects.
use super::*;
use crate::play::own_nanos::VisualEvent;
use crate::play::combat::effects::{Binding, EffectConfig};

pub(super) struct BuffVisual {
    effect: i32,
    duration: f32,
    handle: Option<u32>,
}

impl Dynels {
    pub fn apply_buff_visuals(&mut self, who: i32, events: impl IntoIterator<Item = VisualEvent>) {
        for event in events {
            let nano = match event { VisualEvent::Add { nano, .. } | VisualEvent::Remove { nano } => nano };
            if let Some(old) = self.buff_visuals.remove(&(who, nano)) {
                if let (Some(renderer), Some(handle)) = (&mut self.effects, old.handle) {
                    renderer.terminate_gracefully(handle);
                }
            }
            if let VisualEvent::Add { effect, duration_cs, .. } = event {
                if effect != 49999 {
                    self.buff_visuals.insert((who, nano), BuffVisual { effect, duration: (duration_cs as u32 / 100) as f32, handle: None });
                }
            }
        }
    }

    pub(super) fn buff_message(&mut self, m: &Message) {
        let who = m.header.target;
        if who.kind != CHAR_KIND || who.instance == self.own { return; }
        let relevant = match &m.body {
            N3::Dynel(Dynel::SimpleCharFullUpdate(_)) | N3::Misc(Misc::Buff(_)) => true,
            N3::World(World::CharacterAction(a)) => matches!(a.action, 0x62 | 0xb1),
            _ => false,
        };
        if !relevant { return; }
        let nanos = self.buff_nanos.entry(who.instance).or_default();
        nanos.on_message(who, who, &m.body, 100);
        let events = nanos.take_visuals();
        self.apply_buff_visuals(who.instance, events);
    }

    pub(super) fn cancel_buff_visuals(&mut self, who: i32) {
        self.buff_nanos.remove(&who);
        let renderer = &mut self.effects;
        self.buff_visuals.retain(|&(target, _), visual| {
            if target != who { return true; }
            if let (Some(renderer), Some(handle)) = (renderer.as_mut(), visual.handle) { renderer.terminate_gracefully(handle); }
            false
        });
    }

    pub fn needs_effect_source_mesh(&self, identity: (u32, u32)) -> bool {
        self.effects.as_ref().is_some_and(|renderer| renderer.needs_source_mesh(identity))
    }

    pub fn prepare_effect_source_mesh(&mut self, identity: (u32, u32), scene: &ao_scene::Scene, actor: &ActorFrame) {
        if let Some(renderer) = &mut self.effects {
            renderer.prepare_source_mesh(identity, scene, actor);
        }
    }

    pub fn effect_source_model_changed(&mut self, identity: (u32, u32)) {
        if let Some(renderer) = &mut self.effects { renderer.source_model_changed(identity); }
    }

    /// Pending entries survive worker/model delays; only server lifecycle changes remove them.
    pub fn buff_visual_frame(&mut self, dt: f32, mut own_anchor: impl FnMut(i32) -> Option<[[f32; 4]; 4]>, mut stat: impl FnMut(i32, u32) -> Option<i32>) {
        for nanos in self.buff_nanos.values_mut() { nanos.tick(dt); }
        let Some(mut renderer) = self.effects.take() else { return };
        let mut visuals = std::mem::take(&mut self.buff_visuals);
        for (&(who, _), visual) in &mut visuals {
            if visual.handle.is_some() { continue; }
            if self.nano_effect_categories & 4 == 0 && renderer.effect_kind(visual.effect) != Some(1020) {
                visual.handle = Some(0);
                continue;
            }
            let Some(attractor) = renderer.attractor(visual.effect, 0) else {
                // Retail template lookup failure creates no handle, not replacement art.
                visual.handle = Some(0);
                continue;
            };
            let mut anchor = |id| {
                if who == self.own { own_anchor(id).map(|m| glam::Mat4::from_cols_array_2d(&m)) }
                else { self.effect_anchor(who, id, 0) }
            };
            let Some(source) = anchor(attractor) else { continue };
            let Some(target) = anchor(0) else { continue };
            let identity = (CHAR_KIND as u32, who as u32);
            renderer.prepare_anchors(identity, |_, id| anchor(id));
            let appearance = match [4, 59, 47, 360].map(|s| stat(who, s)) {
                [Some(a), Some(b), Some(c), Some(d)] => Some([a, b, c, d]),
                _ => None,
            };
            let config = EffectConfig { duration: Some(visual.duration), track_source: true, source_identity: Some(identity), target_identity: Some(identity), source_appearance: appearance, target_appearance: appearance, ..Default::default() };
            match renderer.spawn_configured(Binding { group: 0, attractor, effect: visual.effect, note: 0, color: 0 }, source, target.w_axis.truncate(), config) {
                Ok(handle) => visual.handle = Some(handle),
                Err(error) => { eprintln!("nano buff: {error:#}"); visual.handle = Some(0); }
            }
        }
        self.buff_visuals = visuals;
        self.effects = Some(renderer);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::own_nanos::OwnNanos;
    use ao_net::{frame::Frame, msg::Identity};
    use ao_net::n3::{action::simple, nano};

    fn installed() -> Option<std::path::PathBuf> {
        let dir = ao_gui::client_dir();
        dir.join("cd_image/rdb.db").exists().then_some(dir)
    }

    fn capture() -> Vec<Message> {
        include_str!("../../../../docs/captures/zone_ithaca.rec").lines().filter_map(|line| {
            let mut fields = line.split(' ');
            let (_, Some("<"), Some(hex)) = (fields.next(), fields.next(), fields.next()) else { return None };
            let bytes: Vec<_> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap()).collect();
            let (frame, _) = Frame::decode_with(&bytes, false).unwrap()?;
            ao_net::n3::decode(&frame).ok()
        }).collect()
    }

    /// Actual NPC Shadow Touch traffic: no authored stat413, hence no persistent effect.
    #[test]
    fn captured_npc_shadow_touch_has_no_persistent_visual() {
        let Some(dir) = installed() else { return };
        let mut world = Dynels { effects: Some(crate::play::combat::effects::Renderer::open(&dir).unwrap()), ..Default::default() };
        let mut adds = 0;
        let mut removes = 0;
        for message in capture() {
            if message.header.target.instance != 0x827a { continue; }
            match &message.body {
                N3::World(World::CharacterAction(a)) if a.action == 0x62 => {
                    world.on_message(&message);
                    assert_eq!(world.buff_visuals[&(0x827a, 163449)].effect, 1_234_567_890);
                    world.buff_visual_frame(0.0, |_| None, |_, _| None);
                    assert_eq!(world.buff_visuals[&(0x827a, 163449)].handle, Some(0));
                    adds += 1;
                }
                N3::Misc(Misc::Buff(b)) if b.kind == 0 => {
                    world.on_message(&message);
                    assert!(!world.buff_visuals.contains_key(&(0x827a, 163449)));
                    removes += 1;
                }
                _ => {}
            }
        }
        assert_eq!((adds, removes), (3, 3));
    }

    /// Real Body Boost metadata in a templated wire replay, not a captured player cast.
    #[test]
    fn real_body_boost_own_foreign_refresh_login_and_pending() {
        let Some(dir) = installed() else { return };
        let own = Identity { kind: CHAR_KIND, instance: 7 };
        let other = Identity { kind: CHAR_KIND, instance: 8 };
        let mut nanos = OwnNanos::default();
        let mut world = Dynels { own: own.instance, effects: Some(crate::play::combat::effects::Renderer::open(&dir).unwrap()), ..Default::default() };
        let add = |op, duration| N3::World(World::CharacterAction(simple(op, nano::nano(29091), Identity { kind: 42, instance: duration })));
        nanos.on_message(own, own, &add(0x62, 10099), 150);
        assert_eq!(nanos.buffs[0].total_cs, 15148);
        let events = nanos.take_visuals();
        assert_eq!(events, [VisualEvent::Add { nano: 29091, effect: 1070, duration_cs: 10099 }]);
        world.apply_buff_visuals(own.instance, events);
        world.buff_visual_frame(0.0, |_| None, |_, _| None);
        assert_eq!(world.buff_visuals[&(7, 29091)].duration, 100.0);
        assert_eq!(world.buff_visuals[&(7, 29091)].handle, None, "model delay must retain the entry");
        world.buff_visual_frame(0.0, |_| Some(glam::Mat4::IDENTITY.to_cols_array_2d()), |_, _| None);
        let handle = world.buff_visuals[&(7, 29091)].handle.expect("Body Boost creates its authored effect");
        assert_ne!(handle, 0);
        assert!(world.effects.as_ref().unwrap().is_active(handle));
        let mut foreign = capture().into_iter().find(|m| matches!(&m.body, N3::World(World::CharacterAction(a)) if a.action == 0x62)).unwrap();
        foreign.header.target = other;
        foreign.body = add(0x62, 10099);
        world.on_message(&foreign);
        nanos.on_message(own, own, &add(0xb1, 2050), 100);
        let refresh = nanos.take_visuals();
        assert_eq!(refresh, [VisualEvent::Remove { nano: 29091 }, VisualEvent::Add { nano: 29091, effect: 1070, duration_cs: 2050 }]);
        world.apply_buff_visuals(7, refresh);
        assert!(!world.effects.as_ref().unwrap().is_active(handle), "refresh deletes native class1001 immediately");
        world.buff_visual_frame(0.0, |_| Some(glam::Mat4::IDENTITY.to_cols_array_2d()), |_, _| None);
        let refreshed_handle = world.buff_visuals[&(7, 29091)].handle.unwrap();
        assert_ne!(refreshed_handle, 0);
        let mut update = capture().into_iter().find_map(|message| match message.body {
            N3::Dynel(Dynel::SimpleCharFullUpdate(update)) => Some(update),
            _ => None,
        }).unwrap();
        update.effects = vec![ao_net::n3::dynel::EffectEntry { source: nano::nano(29091), a: 0, b: 10000, c: 499 }];
        foreign.body = N3::Dynel(Dynel::SimpleCharFullUpdate(update.clone()));
        world.on_message(&foreign);
        assert_eq!(world.buff_visuals[&(8, 29091)].duration, 4.0);
        nanos.on_message(own, own, &N3::Dynel(Dynel::SimpleCharFullUpdate(update)), 100);
        world.apply_buff_visuals(7, nanos.take_visuals());
        assert!(!world.effects.as_ref().unwrap().is_active(refreshed_handle), "login-list replacement terminates the old visual");
        assert_eq!(world.buff_visuals[&(7, 29091)].duration, 4.0);
        world.buff_visual_frame(0.0, |_| Some(glam::Mat4::IDENTITY.to_cols_array_2d()), |_, _| None);
        let login_handle = world.buff_visuals[&(7, 29091)].handle.unwrap();
        assert_ne!(login_handle, 0);
        let remove = N3::Misc(Misc::Buff(ao_net::n3::misc::Buff { kind: 0, nano: Some(nano::nano(29091)), rest: vec![] }));
        nanos.on_message(own, own, &remove, 100);
        world.apply_buff_visuals(7, nanos.take_visuals());
        assert!(!world.buff_visuals.contains_key(&(7, 29091)));
        assert!(!world.effects.as_ref().unwrap().is_active(login_handle), "BuffIIR removes the renderer actor, not just lifecycle state");
        assert!(world.buff_visuals.contains_key(&(8, 29091)));
        foreign.body = remove;
        world.on_message(&foreign);
        assert!(!world.buff_visuals.contains_key(&(8, 29091)));
        nanos.on_message(own, own, &add(0x62, 10099), 100);
        world.apply_buff_visuals(7, nanos.take_visuals());
        world.buff_visual_frame(0.0, |_| Some(glam::Mat4::IDENTITY.to_cols_array_2d()), |_, _| None);
        let death_handle = world.buff_visuals[&(7, 29091)].handle.unwrap();
        assert_ne!(death_handle, 0);
        world.cancel_nano_visuals(7);
        assert!(!world.effects.as_ref().unwrap().is_active(death_handle), "death/teardown removes the renderer actor");
        nanos.on_message(own, own, &add(0x62, 10099), 100);
        world.apply_buff_visuals(7, nanos.take_visuals());
        world.buff_visual_frame(0.0, |_| Some(glam::Mat4::IDENTITY.to_cols_array_2d()), |_, _| None);
        let quit_handle = world.buff_visuals[&(7, 29091)].handle.unwrap();
        assert_ne!(quit_handle, 0);
        foreign.header.target = own;
        foreign.body = N3::Misc(Misc::ToClientQuit);
        world.on_message(&foreign);
        assert!(!world.effects.as_ref().unwrap().is_active(quit_handle), "actual dynel deletion removes attached renderer actors");
    }
}
