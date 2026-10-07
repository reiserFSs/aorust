//! Server-confirmed item use. Selection evidence: docs/zone/interact.md §8.8.

use ao_net::msg::Identity;
use ao_net::n3::{misc::{GenericArgs, Misc}, Message, N3};

/// Item identities are relative to `actor`, not to the receiving client's inventory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfirmedUse {
    pub actor: Identity,
    pub item: Identity,
    pub target: Option<Identity>,
    pub seq: i32,
    pub command: i32,
}

/// GC 1007c76a → 1003bac9 → 1003b947 → 1003b6e0: requests and aborts do not use the item.
/// The action-data actor is authoritative, including when the frame header names our own character.
pub fn confirmed(message: &Message) -> Option<ConfirmedUse> {
    let N3::Misc(Misc::GenericCmd(command)) = &message.body else { return None };
    if command.state != 1 { return None; }
    let (actor, item, target) = match (&command.args, command.cmd) {
        (GenericArgs::Item { actor, item, .. }, 3) => (*actor, *item, None),
        (GenericArgs::ItemOnItem { actor, item, target, .. }, 5 | 0x20) => (*actor, *item, Some(*target)),
        _ => return None,
    };
    Some(ConfirmedUse { actor, item, target, seq: command.seq, command: command.cmd })
}

/// Inspect an already-gated own request; GC 1007c8b7 starts its authored gesture
/// before acknowledgement. Never call this for a refused local interaction.
pub fn requested(message: &Message) -> Option<ConfirmedUse> {
    let N3::Misc(Misc::GenericCmd(command)) = &message.body else { return None };
    if command.state != 0 { return None; }
    let (actor, item, target) = match (&command.args, command.cmd) {
        (GenericArgs::Item { actor, item, .. }, 3) => (*actor, *item, None),
        (GenericArgs::ItemOnItem { actor, item, target, .. }, 5 | 0x20) => (*actor, *item, Some(*target)),
        _ => return None,
    };
    Some(ConfirmedUse { actor, item, target, seq: command.seq, command: command.cmd })
}

/// `SimpleItem::10081e74`: the receiver is the ITEM, not the character (ECX+0x78
/// at 10081e7f). The actor's own clip map resolves the selected AbstractAnimID later.
/// Pass a value selected with the existing CRT random picker from item key `command`.
pub fn gesture(selected: Option<u32>, crawling: bool) -> u16 {
    if crawling { return 0x9b; }
    selected.filter(|&id| id != 0 && id <= 2000).map_or(super::anim::WIELD_GESTURE, |id| id as u16)
}

/// Sound-list key of the runtime item-use callback. Absence means retail makes no
/// PlayGameSound call, not a missing sound to replace with a generic effect.
/// GC 10080f1c: plain use is own-only; depleted charges use key 0x32.
/// GC 1008983a: use-on-item plays key 5 for both own and foreign actors.
/// GC 1008035a: use-on-character has no sound-map call.
pub fn sound_key(command: i32, own_actor: bool, charges: Option<i32>) -> Option<u32> {
    match command {
        3 if own_actor => Some(if charges == Some(0) { 0x32 } else { 3 }),
        5 => Some(5),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Playback {
    pub actor: Identity,
    pub animation: Option<u16>,
    pub sound: Option<u32>,
    pub sound_at_origin: bool,
    pub visuals: Option<ao_net::n3::spells::ApplySpells>,
}

struct Pending {
    action: ConfirmedUse,
    left: f32,
    sound: Option<u32>,
    sound_at_origin: bool,
    visuals: Option<ao_net::n3::spells::ApplySpells>,
}


#[derive(Default)]
struct ItemData {
    animations: Vec<(u32, Vec<u32>)>,
    sounds: Vec<(u32, Vec<u32>)>,
    charges: Option<i32>,
    delay: Option<i32>,
    actor_visuals: Vec<ao_net::n3::spells::Spell>,
    item_visuals: Vec<ao_net::n3::spells::Spell>,
}
/// Item-action lifetime, independent of GUI and rendering.
pub struct Uses {
    store: Option<ao_rdb::RecordStore>,
    requested: Vec<ConfirmedUse>,
    pending: Vec<Pending>,
    playback: Vec<Playback>,
}

impl Default for Uses {
    fn default() -> Self {
        Self { store: ao_rdb::RecordStore::open(&ao_gui::client_dir()).ok(), requested: Vec::new(), pending: Vec::new(), playback: Vec::new() }
    }
}

impl Uses {
    pub fn clear(&mut self) {
        self.requested.clear();
        self.pending.clear();
        self.playback.clear();
    }

    pub fn request(&mut self, message: &Message) {
        if let Some(action) = requested(message) { self.requested.push(action); }
    }

    fn data(&self, action: ConfirmedUse, zone: &crate::play::zone::Zone, callback: bool) -> Option<ItemData> {
        let own = action.actor.instance == zone.char_id as i32 && action.actor.kind == 50000;
        let inventory = (0x65..=0xf9).contains(&action.item.kind);
        let template = if inventory {
            if !own { return None; }
            u32::try_from(zone.inventory.get(&(action.item.instance as u32))?.item.low_id).ok()?
        } else {
            zone.world.item_class_of(action.item.kind, action.item.instance)?;
            zone.world.stat_of(action.item.kind, action.item.instance, 23).unwrap_or(0).max(0) as u32
        };
        let record = self.store.as_ref().and_then(|store| store.get(ao_formats::dynel_visual::ITEM_TEMPLATE_TYPE, template).ok().flatten());
        let animations = record.as_ref().map(|record| ao_formats::dynel_visual::animation_map(record)).unwrap_or_default();
        if !callback { return Some(ItemData { animations, ..Default::default() }); }
        let parsed = record.as_ref().and_then(|record| ao_formats::dynel_visual::parse_item_template(record).ok());
        let stat = |key| (!inventory).then(|| zone.world.stat_of(action.item.kind, action.item.instance, key)).flatten().or_else(|| parsed.as_ref().and_then(|item| item.stat(key)));
        let charges = stat(26);
        let attack = stat(294).or_else(|| parsed.as_ref().map(|_| 0));
        let recharge = stat(210).or_else(|| parsed.as_ref().map(|_| 0));
        let delay = if action.command == 3 && action.item.kind == 0xc749 { Some(0) } else {
            attack.zip(recharge).map(|(attack, recharge)| if action.command == 3 && i64::from(attack) + i64::from(recharge) < 31 { 0 } else { attack })
        };
        let sounds = parsed.map(|item| item.sounds).unwrap_or_else(|| record.as_ref().map(|record| ao_formats::dynel_visual::sound_map(record)).unwrap_or_default());
        let event = match action.command {
            3 if charges != Some(0) => Some(0),
            5 if action.target != Some(action.item) => Some(4),
            0x20 => Some(4),
            _ => None,
        };
        let (actor_visuals, item_visuals) = event.and_then(|event| record.as_ref().and_then(|record| crate::play::chat::item_template_spells(record, event).ok())).unwrap_or_default().into_iter().filter(|spell| spell.function == 0xcf26 && !matches!(spell.stat(0x20), 0xe | 0x17)).partition(|spell| spell.stat(0x20) == 2 || (spell.stat(0x20) == 3 && action.command == 3));
        Some(ItemData { animations, sounds, charges, delay, actor_visuals, item_visuals })
    }

    fn pick(zone: &mut crate::play::zone::Zone, map: &[(u32, Vec<u32>)], key: u32) -> Option<u32> {
        let values = &map.iter().find(|entry| entry.0 == key)?.1;
        zone.world.pick_variant(values)
    }

    fn animate(&mut self, action: ConfirmedUse, zone: &mut crate::play::zone::Zone) {
        if !Self::available(action, zone) { return; }
        let Some(item) = self.data(action, zone, false) else { return };
        self.animate_with(action, zone, &item.animations);
    }

    fn animate_with(&mut self, action: ConfirmedUse, zone: &mut crate::play::zone::Zone, animations: &[(u32, Vec<u32>)]) {
        let selected = Self::pick(zone, animations, action.command as u32);
        let crawling = zone.stat_of(action.actor.instance, 0x1ae) == Some(0xe);
        self.playback.push(Playback { actor: action.actor, animation: Some(gesture(selected, crawling)), sound: None, sound_at_origin: false, visuals: None });
    }

    pub fn on_message(&mut self, message: &Message, zone: &mut crate::play::zone::Zone) {
        if let N3::Misc(Misc::GenericCmd(command)) = &message.body {
            if command.state == 2 {
                let actor = match &command.args {
                    GenericArgs::Item { actor, .. } | GenericArgs::ItemOnItem { actor, .. } => *actor,
                    GenericArgs::Raw(_) => message.header.target,
                };
                self.pending.retain(|pending| pending.action.seq != command.seq || pending.action.actor != actor);
                return;
            }
        }
        let Some(action) = confirmed(message) else { return };
        let own = action.actor.kind == 50000 && action.actor.instance == zone.char_id as i32;
        let Some(item) = self.data(action, zone, true) else { return };
        if !Self::available(action, zone) { return; }
        if !own { self.animate_with(action, zone, &item.animations); }
        let Some(delay) = item.delay else { return };
        let sound = if action.target == Some(action.item) { None } else { sound_key(action.command, own, item.charges).and_then(|key| Self::pick(zone, &item.sounds, key)) };
        let visuals = (!item.actor_visuals.is_empty()).then_some(ao_net::n3::spells::ApplySpells { target: action.actor, apply: true, spells: item.actor_visuals });
        let sound_at_origin = action.command == 3 && item.charges == Some(0);
        self.callback(action, delay, sound, sound_at_origin, visuals);
        if !item.item_visuals.is_empty() {
            self.callback(action, delay, None, false, Some(ao_net::n3::spells::ApplySpells { target: action.item, apply: true, spells: item.item_visuals }));
        }
    }

    fn callback(&mut self, action: ConfirmedUse, delay: i32, sound: Option<u32>, sound_at_origin: bool, visuals: Option<ao_net::n3::spells::ApplySpells>) {
        if sound.is_none() && visuals.is_none() { return; }
        if delay <= 0 {
            self.playback.push(Playback { actor: action.actor, animation: None, sound, sound_at_origin, visuals });
        } else {
            self.pending.push(Pending { action, left: delay as f32, sound, sound_at_origin, visuals });
        }
    }

    fn available(action: ConfirmedUse, zone: &crate::play::zone::Zone) -> bool {
        if zone.world.is_dead(action.actor.instance) || zone.stat_of(action.actor.instance, 0x296).is_some_and(|value| value != 0) { return false; }
        if matches!(action.item.kind, 0x69 | 0x6a) { return false; }
        if let Some(target) = action.target {
            if target.kind == 50000 {
                if target.instance != zone.char_id as i32 && !zone.dynels.contains_key(&target.instance) { return false; }
            } else if (0x65..=0xf9).contains(&target.kind) {
                if action.actor.instance != zone.char_id as i32 || !zone.inventory.contains_key(&(target.instance as u32)) { return false; }
            } else if zone.world.item_class_of(target.kind, target.instance).is_none() {
                return false;
            }
        }
        if (0x65..=0xf9).contains(&action.item.kind) {
            action.actor.kind == 50000 && action.actor.instance == zone.char_id as i32 && zone.inventory.contains_key(&(action.item.instance as u32))
        } else {
            zone.world.item_class_of(action.item.kind, action.item.instance).is_some()
        }
    }

    /// `dt` is seconds; the retail action timer uses centiseconds.
    pub fn update(&mut self, zone: &mut crate::play::zone::Zone, dt: f32) {
        for action in std::mem::take(&mut self.requested) { self.animate(action, zone); }
        for pending in &mut self.pending { pending.left -= dt * 100.0; }
        let playback = &mut self.playback;
        self.pending.retain_mut(|pending| {
            if pending.left > 0.0 { return true; }
            if Self::available(pending.action, zone) {
                playback.push(Playback { actor: pending.action.actor, animation: None, sound: pending.sound, sound_at_origin: pending.sound_at_origin, visuals: pending.visuals.take() });
            }
            false
        });
    }

    pub fn take(&mut self) -> Vec<Playback> { std::mem::take(&mut self.playback) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::{frame::Frame, n3::{self, misc::GenericCmd, outgoing::n3_frame}};

    #[test]
    fn remote_sound_timer_and_abort_keep_actor_identity() {
        let actor = Identity { kind: 50000, instance: 77 };
        let action = ConfirmedUse { actor, item: Identity { kind: 0xc73d, instance: 9 }, target: None, seq: 10, command: 5 };
        let mut uses = Uses { store: None, requested: Vec::new(), pending: vec![Pending { action, left: 100.0, sound: Some(42), sound_at_origin: false, visuals: None }], playback: Vec::new() };
        let mut zone = crate::play::zone::Zone::default();
        uses.update(&mut zone, 0.5);
        assert!(uses.take().is_empty());
        assert_eq!(uses.pending[0].left, 50.0);
        uses.update(&mut zone, 0.5);
        assert!(uses.take().is_empty(), "an unresolved runtime item cannot run its callback");
        assert!(uses.pending.is_empty());
        uses.pending.push(Pending { action, left: 100.0, sound: Some(42), sound_at_origin: false, visuals: None });
        let abort = Misc::GenericCmd(GenericCmd { state: 2, seq: 10, cmd: 5, args: GenericArgs::ItemOnItem { flag: 0, actor, item: action.item, target: actor } });
        let frame = n3_frame(0, 1, abort.encode(Identity { kind: 50000, instance: 1 }, 0));
        uses.on_message(&n3::decode(&frame).unwrap(), &mut zone);
        uses.update(&mut zone, 2.0);
        assert!(uses.take().is_empty());
    }

    #[test]
    fn starter_laboratory_confirmed_callback_queues_authored_stars_for_own_actor() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let mut uses = Uses::default();
        let mut zone = crate::play::zone::Zone::new(77);
        let actor = Identity { kind: 50000, instance: 77 };
        let slot = 0x40;
        let item = ao_net::n3::inventory::item_identity(slot);
        zone.inventory.insert(slot, ao_net::n3::world::InventoryEntry {
            slot, a: 0, b: 1, id: item,
            item: ao_net::n3::world::AcgItem { low_id: 116628, high_id: 116628, level: 1 },
        });
        let packet = Misc::GenericCmd(GenericCmd { state: 1, seq: 1, cmd: 3, args: GenericArgs::Item { flag: 0, actor, item } });
        let message = n3::decode(&n3_frame(0, 77, packet.encode(actor, 0))).unwrap();
        uses.on_message(&message, &mut zone);
        let playback = uses.take();
        assert_eq!(playback.len(), 1);
        assert_eq!(playback[0].actor, actor);
        assert_eq!(playback[0].animation, None, "own acknowledgement must not replay the request gesture");
        assert_eq!(playback[0].sound, None, "the lab has no authored use sound map");
        let application = playback[0].visuals.as_ref().unwrap();
        assert_eq!(application.target, actor);
        assert!(application.apply);
        assert_eq!(application.spells.len(), 1);
        assert_eq!(application.spells[0].function, 0xcf26);
        assert_eq!(application.spells[0].stat(0x27), 13600);
        assert_eq!(application.spells[0].stat(0x31), 100);
    }

    #[test]
    fn foreign_laboratory_confirmation_uses_actor_not_frame_target() {
        if !ao_gui::client_dir().join("cd_image/rdb.db").exists() { return; }
        let mut uses = Uses::default();
        let mut zone = crate::play::zone::Zone::new(1);
        let actor = Identity { kind: 50000, instance: 77 };
        let own = Identity { kind: 50000, instance: 1 };
        let item = Identity { kind: 0xc73d, instance: 99 };
        zone.world.test_template_prop(item, 116628, uses.store.as_ref().unwrap(), vec![]).unwrap();
        let packet = Misc::GenericCmd(GenericCmd { state: 1, seq: 1, cmd: 3, args: GenericArgs::Item { flag: 0, actor, item } });
        let message = n3::decode(&n3_frame(0, 1, packet.encode(own, 0))).unwrap();
        uses.on_message(&message, &mut zone);
        let playback = uses.take();
        assert!(playback.iter().any(|action| action.actor == actor && action.animation == Some(0x6d)));
        let application = playback.iter().find_map(|action| action.visuals.as_ref()).unwrap();
        assert_eq!(application.target, actor);
        assert_eq!(application.spells[0].stat(0x27), 13600);
        assert!(playback.iter().all(|action| action.sound.is_none()), "foreign cmd3 skips only the own sound branch");
    }

    #[test]
    fn crystal_use_on_item_keeps_authored_source_item_receiver() {
        if !ao_gui::client_dir().join("cd_image/rdb.db").exists() { return; }
        let mut uses = Uses::default();
        let mut zone = crate::play::zone::Zone::new(77);
        let actor = Identity { kind: 50000, instance: 77 };
        let item = Identity { kind: 0xc73d, instance: 98 };
        let target = Identity { kind: 0xc73d, instance: 99 };
        zone.world.test_template_prop(item, 275468, uses.store.as_ref().unwrap(), vec![]).unwrap();
        zone.world.test_template_prop(target, 116628, uses.store.as_ref().unwrap(), vec![]).unwrap();
        let action = ConfirmedUse { actor, item, target: Some(target), seq: 1, command: 5 };
        let data = uses.data(action, &zone, true).unwrap();
        assert!(data.actor_visuals.is_empty());
        assert_eq!(data.item_visuals.len(), 1);
        assert_eq!(data.item_visuals[0].stat(0x20), 3);
        assert_eq!(data.item_visuals[0].stat(0x27), 73001);
        assert!(uses.data(ConfirmedUse { target: Some(item), ..action }, &zone, true).unwrap().item_visuals.is_empty());
        let delay = data.delay.unwrap().max(0) as f32 / 100.0;
        let packet = Misc::GenericCmd(GenericCmd { state: 1, seq: 1, cmd: 5, args: GenericArgs::ItemOnItem { flag: 0, actor, item, target } });
        let message = n3::decode(&n3_frame(0, 77, packet.encode(actor, 0))).unwrap();
        uses.on_message(&message, &mut zone);
        uses.update(&mut zone, delay + 0.01);
        let playback = uses.take();
        let application = playback.iter().find_map(|action| action.visuals.as_ref()).unwrap();
        assert_eq!(application.target, item, "std Target3 is the source item's Beholder, not its owner or target item");
    }

    #[test]
    fn beacon_use_on_item_keeps_authored_source_receiver() {
        if !ao_gui::client_dir().join("cd_image/rdb.db").exists() { return; }
        let mut uses = Uses::default();
        let mut zone = crate::play::zone::Zone::new(77);
        let actor = Identity { kind: 50000, instance: 77 };
        let item = Identity { kind: 0xc73d, instance: 98 };
        let target = Identity { kind: 0xc73d, instance: 99 };
        zone.world.test_template_prop(item, 288073, uses.store.as_ref().unwrap(), vec![]).unwrap();
        zone.world.test_template_prop(target, 116628, uses.store.as_ref().unwrap(), vec![]).unwrap();
        let action = ConfirmedUse { actor, item, target: Some(target), seq: 1, command: 5 };
        let data = uses.data(action, &zone, true).unwrap();
        assert!(data.actor_visuals.is_empty());
        assert_eq!(data.item_visuals.len(), 1);
        assert_eq!(data.item_visuals[0].stat(0x27), 71214);
        let delay = data.delay.unwrap().max(0) as f32 / 100.0;
        let packet = Misc::GenericCmd(GenericCmd { state: 1, seq: 1, cmd: 5, args: GenericArgs::ItemOnItem { flag: 0, actor, item, target } });
        let message = n3::decode(&n3_frame(0, 77, packet.encode(actor, 0))).unwrap();
        uses.on_message(&message, &mut zone);
        uses.update(&mut zone, delay + 0.01);
        let playback = uses.take();
        let application = playback.iter().find_map(|action| action.visuals.as_ref()).unwrap();
        assert_eq!(application.target, item);
        zone.world.apply_nano_visuals(application.clone());
    }

    #[test]
    fn depleted_laboratory_returns_before_authored_visual_callback() {
        if !ao_gui::client_dir().join("cd_image/rdb.db").exists() { return; }
        let mut uses = Uses::default();
        let mut zone = crate::play::zone::Zone::new(77);
        let actor = Identity { kind: 50000, instance: 77 };
        let item = Identity { kind: 0xc73d, instance: 99 };
        zone.world.test_template_prop(item, 116628, uses.store.as_ref().unwrap(), vec![(26, 0)]).unwrap();
        let packet = Misc::GenericCmd(GenericCmd { state: 1, seq: 1, cmd: 3, args: GenericArgs::Item { flag: 0, actor, item } });
        let message = n3::decode(&n3_frame(0, 77, packet.encode(actor, 0))).unwrap();
        uses.on_message(&message, &mut zone);
        assert!(uses.take().is_empty());
    }

    #[test]
    fn authored_selection_and_retail_fallbacks() {
        assert_eq!(gesture(Some(0x12), false), 0x12);
        assert_eq!(gesture(Some(0x97), false), 0x97);
        assert_eq!(gesture(Some(0x29), false), 0x29);
        for selected in [None, Some(0), Some(2001)] {
            assert_eq!(gesture(selected, false), 0x6d);
        }
        assert_eq!(gesture(Some(0x97), true), 0x9b);
        assert_eq!(sound_key(3, true, None), Some(3));
        assert_eq!(sound_key(3, true, Some(0)), Some(0x32));
        assert_eq!(sound_key(3, false, None), None);
        assert_eq!(sound_key(5, false, None), Some(5));
        assert_eq!(sound_key(0x20, true, None), None);
    }

    #[test]
    fn installed_authored_use_categories() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let store = ao_rdb::RecordStore::open(&dir).unwrap();
        for (id, expected) in [(88395, 0x12), (215423, 0x97), (154327, 0x29)] {
            let record = store.get(ao_formats::dynel_visual::ITEM_TEMPLATE_TYPE, id).unwrap().unwrap();
            let map = ao_formats::dynel_visual::animation_map(&record);
            assert_eq!(map.iter().find(|entry| entry.0 == 3).and_then(|entry| entry.1.first()).copied(), Some(expected), "template {id}");
        }
        for id in [23314, 25812, 26522] {
            let record = store.get(ao_formats::dynel_visual::ITEM_TEMPLATE_TYPE, id).unwrap().unwrap();
            assert!(ao_formats::dynel_visual::animation_map(&record).is_empty(), "template {id}");
        }
    }

    #[test]
    fn capture_confirmation_and_remote_actor_replay() {
        let text = include_str!("../../../../../docs/captures/zone_use_object_ithaca.rec");
        let mut confirmations = 0;
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            let _ = fields.next().unwrap();
            let from_server = fields.next().unwrap() == "<";
            let hex = fields.next().unwrap();
            let bytes: Vec<_> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
            let frame = Frame::decode_with(&bytes, false).unwrap().unwrap().0;
            let message = n3::decode(&frame).unwrap();
            let Some(action) = confirmed(&message) else { continue };
            assert!(from_server);
            confirmations += 1;
            let N3::Misc(Misc::GenericCmd(mut command)) = message.body else { unreachable!() };
            let remote = Identity { kind: 50000, instance: 123456 };
            match &mut command.args {
                GenericArgs::Item { actor, .. } | GenericArgs::ItemOnItem { actor, .. } => *actor = remote,
                _ => unreachable!(),
            }
            let replay = |command: GenericCmd| n3::decode(&n3_frame(0, 1, Misc::GenericCmd(command).encode(message.header.target, 0))).unwrap();
            assert_eq!(confirmed(&replay(command.clone())), Some(ConfirmedUse { actor: remote, ..action }));
            for state in [0, 2] {
                command.state = state;
                assert_eq!(confirmed(&replay(command.clone())), None);
            }
        }
        assert!(confirmations > 0);
    }
}
