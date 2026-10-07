//! Using world objects: `N3Msg_DefaultActionOnDynel` [GC 0x100291da] for non-characters, `N3Msg_GetItem` [GC 0x10027beb], `N3Msg_UseItem` [GC 0x100286f8]
//! for objects and the "UseItem" confirmation dialog [GUI 0x1003012f]. Evidence and addresses: docs/zone/interact.md §8.

use super::dynels::CAN_STAT;
use super::hud_dialog::Dialogs;
use super::hud_pick::hits;
use super::hud_target::Ray;
use super::interact::{Action, Interact};
use super::interact_loot::LootUi;
use super::zone::Zone;
use ao_formats::screens::TextDb;
use ao_gui::{Event, Gui};
use ao_net::msg::Identity;
use ao_net::n3::inventory::{self, InventoryMsg, ANY_BAG_SLOT, BAG_FIRST, BAG_SLOTS};
use ao_net::n3::outgoing::DYNEL_CHAR;
use ao_net::n3::world::World;
use ao_net::n3::{Message, N3};
use std::collections::HashMap;
use std::path::PathBuf;

/// `Can` (stat 0x1e) bit 0: the object can be picked up -> `N3Msg_GetItem`.
pub const CAN_PICK_UP: i32 = 1;
/// `Can` bit 3: the object can be used -> `N3Msg_UseItem(id, false)`.
pub const CAN_USE: i32 = 8;
/// `Can` bit 4: `N3Msg_UseItem(id, false)` asks `ConfirmUseItemDialogue` (GlobalSignals `+0xa0`) instead of using at once.
pub const CAN_CONFIRM: i32 = 0x10;
/// The stat `ConfirmUseItemDialogue` reads (`GetSkill(0x2af, 2)`): an LDB text id of category 0x2715 for the dialog's question. The client's
/// "stat absent" marker is `0x499602d2`; here an absent stat is `None`.
const STAT_CONFIRM_TEXT: u32 = 0x2af;
/// LDB category of the confirmation question (stat `0x2af` = the text id) and of the fallback key `Item_ConfirmUse` (category 0x3e8).
const CAT_CONFIRM_TEXT: u32 = 0x2715;
const CAT_CONFIRM_DEFAULT: u32 = 0x3e8;

/// What `N3Msg_DefaultActionOnDynel` does for a non-character whose `Can` is `can`: bit 0 first (`GetItem`), else bit 3 (`UseItem`).
pub fn decide(can: i32) -> Action {
    if can & CAN_PICK_UP != 0 {
        Action::Get
    } else if can & CAN_USE != 0 {
        Action::Use
    } else {
        Action::None
    }
}

/// What a GUI event of the use layer asks for.
pub(super) enum UseOut {
    /// The event was consumed.
    Handled,
    /// The confirmation was answered Yes: `N3Msg_UseItem(id, true)`.
    Use(Identity),
    /// A loot window item was double-clicked: `MoveItemToInventory(item)`.
    Take(Identity),
}

/// The confirmation dialog, the loot windows and the refusals waiting for the chat.
#[derive(Default)]
pub struct UseUi {
    dialogs: Dialogs<Identity>,
    pub(super) loot: LootUi,
    /// The client directory (item records and icons of the loot window).
    dir: Option<PathBuf>,
    /// `Play::time`, for the double click of the loot window.
    now: f32,
    /// Names of the corpses seen (`Remains of ...`, the blob of `CorpseFullUpdateIIR_t`): the title of their window ([INFERENCE]: the original's title
    /// format string of `FUN_100cc2ca` was not read).
    names: HashMap<(i32, i32), String>,
    /// Objects whose confirmation dialog is to be opened (needs the text database, see [`Interact::show_confirms`]).
    pending: Vec<Identity>,
    feedback: Vec<&'static str>,
}

impl UseUi {
    /// The window of the open loot view of `container`.
    #[cfg(test)]
    pub(super) fn loot_window(&self, container: Identity) -> Option<ao_gui::WindowId> {
        self.loot.window_of(container)
    }

    pub(super) fn close_all(&mut self, gui: &mut Gui) {
        self.dialogs.close_all(gui);
        self.loot.close_all(gui);
        self.pending.clear();
    }

    /// A button of the dialog (Yes = button 0, `ConfirmUseItemResult` [GUI 0x1002f759]) or an event of a loot window; `None` = not ours.
    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event) -> Option<UseOut> {
        let (mine, answer) = self.dialogs.event(gui, ev);
        if mine {
            return Some(answer.and_then(|(id, button)| (button == 0).then_some(UseOut::Use(id))).unwrap_or(UseOut::Handled));
        }
        let out = self.loot.event(gui, ev, self.now)?;
        Some(out.map_or(UseOut::Handled, UseOut::Take))
    }
}

/// Every non-character and character body under the pointer, nearest first (`n3Camera_t` hit list, docs/zone/interact.md §8): characters as
/// `{0xC350, instance}`, props (corpses, vending machines, doors, terminals, items on the ground) by their own identity.
pub fn pick_objects(ray: &Ray, zone: &Zone) -> Vec<Identity> {
    let mut bodies = zone.world.pick_bodies(zone.char_id as i32);
    let props = zone.world.pick_props();
    let by_id: HashMap<i32, Identity> = props.iter().map(|(b, id)| (b.id, *id)).collect();
    bodies.extend(props.iter().map(|(b, _)| *b));
    hits(&bodies, ray.origin, ray.dir, ray.len).into_iter().map(|id| by_id.get(&id).copied().unwrap_or(Identity { kind: DYNEL_CHAR, instance: id })).collect()
}

impl Interact {
    /// `Can` (stat 0x1e) of a non-character dynel; `None` for an unknown one (the original's `GetDynel` + `SimpleItem_t` cast fails, nothing happens).
    pub fn can_of(zone: &Zone, id: Identity) -> Option<i32> {
        (id.kind != DYNEL_CHAR).then(|| zone.world.stat_of(id.kind, id.instance, CAN_STAT)).flatten()
    }

    /// `N3Msg_DefaultActionOnDynel` [GC 0x100291da] on any dynel (double click; the right click on a character). Not the own character. A character
    /// Alive characters dialogue first, then player trade or NPC use; dead characters use. Fight gates read the target's controller, not ours.
    pub fn default_action_on(&mut self, zone: &Zone, id: Identity) -> Action {
        if id == self.own_id() {
            return Action::None;
        }
        if id.kind == DYNEL_CHAR {
            let Some(target) = zone.dynels.get(&id.instance) else { return Action::None };
            let flags = zone.stat_of(id.instance, 0).unwrap_or(0);
            if flags & 0x8000000 != 0 {
                return Action::None;
            }
            if zone.stat_of(self.own as i32, 0x296).unwrap_or(0) != 0 {
                self.use_ui.feedback.push("Feedback_NotInVehicle");
                return Action::Refused("Feedback_NotInVehicle");
            }
            let usable = flags & 0x200000 != 0;
            if !zone.world.is_dead(id.instance) {
                if zone.stat_of(id.instance, super::interact::STAT_TALK as u32).is_some_and(|v| v & 1 != 0) {
                    self.send(ao_net::n3::knubot::open_chat_window(self.own_id(), id));
                    return Action::Talk;
                }
                if !target.npc || !usable {
                    return if zone.fight_target.contains_key(&id.instance) { Action::None } else { self.trade_action(zone, id.instance) };
                }
            }
            if zone.fight_target.contains_key(&id.instance) {
                return Action::None;
            }
            if usable && zone.stat_of(self.own as i32, 0).unwrap_or(0) & 8 != 0 {
                self.send(ao_net::n3::trade::abort(self.own_id(), true));
                return Action::Abort;
            }
            return self.use_item(zone, id, false);
        }
        match Self::can_of(zone, id).map(decide) {
            Some(Action::Get) => self.get_item(zone, id),
            Some(Action::Use) => self.use_item(zone, id, false),
            _ => Action::None,
        }
    }

    /// `N3Msg_GetItem(id)` [GC 0x10027beb]: `Feedback_InventoryFull` unless a bag slot (`0x40..0x5e`, `FUN_1002a1b0(0x40)`) is free, else
    /// `ClientGetItemIIR_t` (`FUN_10015258`; [`inventory::get_item`]). Not ported (docs/zone/interact.md §8): the item's own pick-up check (`vtable +0xc4`
    /// of `SimpleItem_t`, `FUN_10088529`: unique-item and own-building rules, `Feedback_CantCarryThat`) and the local pick-up animation (`FUN_10081e74`);
    /// the server enforces them.
    pub fn get_item(&mut self, zone: &Zone, id: Identity) -> Action {
        if Self::bag_full(zone) {
            self.use_ui.feedback.push("Feedback_InventoryFull");
            return Action::Refused("Feedback_InventoryFull");
        }
        let p = inventory::get_item(self.own as i32, id);
        self.send(p);
        Action::Get
    }

    /// `N3Msg_UseItem(id, confirmed)` [GC 0x100286f8] for what is not an inventory item: banks / corpse containers refuse with a feedback text, the own
    /// character and other characters pass, a world object with `Can` bit 4 asks the confirmation first (`confirmed` = the dialog's Yes), then
    /// `GenericCmd_t` 3 ([`Interact::use_object`]). Inventory items (kinds below 50000: wearing, unwearing, special actions) are the item windows' (`hud_stats`).
    /// [UNRESOLVED] `VisualMesh_t::AdvertisingUseAction` (billboards use client side): not ported, the server just ignores the command.
    pub fn use_item(&mut self, zone: &Zone, id: Identity, confirmed: bool) -> Action {
        let refuse = |s: &mut Self, key| {
            s.use_ui.feedback.push(key);
            Action::Refused(key)
        };
        match id.kind {
            0x69 => refuse(self, "Feedback_ItemCantBeUsedFromBank"),
            0x6a => refuse(self, "Feedback_ItemsCantBeUsedFromCorpse"),
            0x6e if id.instance != self.own as i32 => refuse(self, "Feedback_MoveItemToInventory"),
            DYNEL_CHAR if id.instance == self.own as i32 => Action::None,
            DYNEL_CHAR => {
                self.use_object(id);
                Action::Use
            }
            k if k > DYNEL_CHAR => {
                if !confirmed && Self::can_of(zone, id).is_some_and(|c| c & CAN_CONFIRM != 0) {
                    self.use_ui.pending.push(id);
                    return Action::Confirm;
                }
                self.use_object(id);
                Action::Use
            }
            _ => Action::None,
        }
    }

    /// The client directory for item records / icons, once per zone connection.
    pub fn set_client_dir(&mut self, dir: PathBuf) {
        self.use_ui.dir = Some(dir);
    }

    /// `Play::time` of this frame.
    pub fn tick(&mut self, now: f32) {
        self.use_ui.now = now;
    }

    /// What a GUI event of the use layer asked for ([`UseOut`]): the confirmed use, or taking an item out of a loot window (`FUN_100ca1e7`: a double click
    /// calls `MoveItemToInventory(item)` = `N3Msg_MoveItemToInventory(item, bag, any slot)`; `Feedback_InventoryFull` when no bag slot is free).
    pub(super) fn use_out(&mut self, zone: &Zone, out: UseOut) {
        match out {
            UseOut::Handled => {}
            UseOut::Use(id) => self.use_object(id),
            UseOut::Take(item) => {
                if Self::bag_full(zone) {
                    self.use_ui.feedback.push("Feedback_InventoryFull");
                } else {
                    let p = inventory::move_item_to_inventory(self.own as i32, item, ANY_BAG_SLOT);
                    self.send(p);
                }
            }
        }
    }

    fn bag_full(zone: &Zone) -> bool {
        (BAG_FIRST..BAG_FIRST + BAG_SLOTS).all(|s| zone.inventory.contains_key(&s))
    }

    /// Every decoded zone frame: corpse names, and `InventoryUpdateIIR_t` for a container that is not the own character's (`FUN_100a040e` [GC 0x100a040e]:
    /// the header must be a character, the container a `Chest_t`; the contents are replaced, the window opens with the message's flag).
    pub(super) fn watch_objects(&mut self, gui: &mut Gui, m: &Message) {
        match &m.body {
            N3::Inventory(InventoryMsg::Bank(entries)) if m.header.target == self.own_id() => {
                let container = Identity { kind: 0xdead, instance: self.own as i32 };
                let u = inventory::InventoryUpdate { capacity: 0x66, kind: 3, entries: entries.clone(), container, word: 0, flag: true };
                let ui = &mut self.use_ui;
                ui.loot.update(gui, ui.dir.as_deref(), self.screen, container, "Bank", &u);
            }
            N3::World(World::Corpse(c)) => {
                let name = c.base.blob.split(|&b| b == 0).next().map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
                self.use_ui.names.insert((m.header.target.kind, m.header.target.instance), name);
            }
            N3::Inventory(InventoryMsg::Update(u)) if u.container.kind > DYNEL_CHAR && m.header.target.kind == DYNEL_CHAR => {
                let title = self.use_ui.names.get(&(u.container.kind, u.container.instance)).cloned().unwrap_or_default();
                let ui = &mut self.use_ui;
                ui.loot.update(gui, ui.dir.as_deref(), self.screen, u.container, &title, u);
            }
            // the server's answer to the take: `{0x6b, word << 16 | slot}` goes into our bag, the corpse list loses it (zone side: `Zone::apply_inventory`)
            N3::Inventory(InventoryMsg::ContainerAdd { item, container, .. })
                if item.kind == inventory::KIND_IN_CONTAINER && *container == (Identity { kind: DYNEL_CHAR, instance: self.own as i32 }) =>
            {
                self.use_ui.loot.taken(gui, *item);
            }
            _ => {}
        }
    }

    /// The `Feedback_*` keys refused actions produced since the last call (the chat prints them from category 110).
    pub fn take_feedback(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.use_ui.feedback)
    }

    /// Opens the "UseItem" dialog of every pending confirmation (`GuiSystem_c::ConfirmUseItemDialogue` [GUI 0x1003012f]): question = LDB text
    /// `(0x2715, stat 0x2af of the object)`, or `Item_ConfirmUse` of category 0x3e8 without that stat; buttons `MsgBox_Yes` / `MsgBox_No` (category 10000).
    pub fn show_confirms(&mut self, gui: &mut Gui, text: &TextDb, zone: &Zone) {
        for id in std::mem::take(&mut self.use_ui.pending) {
            let question = match zone.world.stat_of(id.kind, id.instance, STAT_CONFIRM_TEXT) {
                Some(v) => text.by_id(CAT_CONFIRM_TEXT, v as u32),
                None => text.by_key(CAT_CONFIRM_DEFAULT, "Item_ConfirmUse"),
            };
            let buttons = ["MsgBox_Yes", "MsgBox_No"].map(|k| text.by_key(10000, k).unwrap_or_else(|| k.to_string()));
            self.use_ui.dialogs.go(gui, self.screen, id, &question.unwrap_or_default(), &buttons);
        }
    }
}

/// Live-harness helpers (`flow/live.rs`).
#[cfg(test)]
impl Interact {
    /// `MoveItemToInventory(item)` to any bag slot with an arbitrary item identity (live probing of what the server accepts for a corpse item).
    pub fn move_to_bag(&mut self, item: Identity) {
        let p = inventory::move_item_to_inventory(self.own as i32, item, ANY_BAG_SLOT);
        self.send(p);
    }

    /// The `id` of entry `n` of the first loot window.
    pub fn loot_entry_id(&self, n: usize) -> Option<Identity> {
        self.use_ui.loot.entry_id(n)
    }

    /// Double click on cell `n` of the open loot window: the item goes to the bag ([`Interact::use_out`]).
    pub fn loot_take(&mut self, gui: &mut Gui, zone: &Zone, n: usize) -> bool {
        let Some(item) = self.use_ui.loot.take(gui, n, self.use_ui.now) else { return false };
        self.use_out(zone, UseOut::Take(item));
        true
    }

    /// The loot windows as shown: `(container, [(container slot, item low id, name)])`.
    pub fn loot_dump(&mut self, gui: &mut Gui) -> super::interact_loot::LootRows {
        self.use_ui.loot.dump(gui)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::misc::{GenericArgs, Misc};
    use ao_net::n3::outgoing::{message_key, n3_frame};

    const DOOR: Identity = Identity { kind: 0xC748, instance: 0x1234 };

    fn payloads(i: &mut Interact) -> Vec<Vec<u8>> {
        i.take_outbox().into_iter().map(|f| f.payload).collect()
    }

    #[test]
    fn character_default_action_follows_original_branch_order() {
        use crate::play::zone::DynelState;
        let own = 1;
        let npc = Identity { kind: DYNEL_CHAR, instance: 2 };
        let mut zone = Zone::new(own);
        for (id, is_npc) in [(1, false), (2, true)] {
            zone.dynels.insert(id, DynelState { name: id.to_string(), pos: [0.0; 3], yaw: None, npc: is_npc, side: 0, level: 1, health: 10, max_health: 10 });
        }
        let mut i = Interact::new(own, (800, 600));
        zone.character_stats.entry(2).or_default().insert(0x300, 1);
        zone.character_stats.entry(2).or_default().insert(0, 0x8000000);
        assert_eq!(i.default_action_on(&zone, npc), Action::None);
        zone.character_stats.entry(2).or_default().insert(0, 0x200000);
        zone.stats.insert(0x296, 1);
        assert_eq!(i.default_action_on(&zone, npc), Action::Refused("Feedback_NotInVehicle"));
        assert!(payloads(&mut i).is_empty());
        zone.stats.insert(0x296, 0);
        zone.fight_target.insert(2, 1);
        assert_eq!(i.default_action_on(&zone, npc), Action::Talk, "dialogue precedes the target fight gate");
        assert_eq!(i.take_outbox()[0].payload, ao_net::n3::knubot::open_chat_window(i.own_id(), npc));
        zone.character_stats.entry(2).or_default().insert(0x300, 0);
        assert_eq!(i.default_action_on(&zone, npc), Action::None);
        zone.fight_target.clear();
        assert_eq!(i.default_action_on(&zone, npc), Action::Use);
        assert!(matches!(ao_net::n3::decode(&i.take_outbox()[0]).unwrap().body, N3::Misc(Misc::GenericCmd(_))));
        zone.stats.insert(0, 8);
        assert_eq!(i.default_action_on(&zone, npc), Action::Abort);
        assert_eq!(i.take_outbox()[0].payload, ao_net::n3::trade::abort(i.own_id(), true));
        zone.character_stats.entry(2).or_default().insert(0, 0);
        assert_eq!(i.default_action_on(&zone, npc), Action::Trade, "NPCs without the use flag take the trade branch");
        assert_eq!(i.take_outbox()[0].payload, ao_net::n3::trade::start(i.own_id(), npc));
        assert_eq!(i.default_action_on(&zone, i.own_id()), Action::None);
        assert_eq!(i.default_action_on(&zone, Identity { kind: DYNEL_CHAR, instance: 99 }), Action::None);
    }

    #[test]
    fn default_action_decision_table() {
        // bit 0 wins over bit 3, bit 4 alone does nothing by default action
        let table = [(0, Action::None), (1, Action::Get), (8, Action::Use), (9, Action::Get), (0x10, Action::None), (0x18, Action::Use), (0x11, Action::Get), (2, Action::None)];
        for (can, want) in table {
            assert_eq!(decide(can), want, "Can {can:#x}");
        }
    }

    #[test]
    fn world_object_use_is_a_generic_cmd_3() {
        let zone = Zone::new(0x6584);
        let mut i = Interact::new(0x6584, (800, 600));
        assert_eq!(i.use_item(&zone, DOOR, false), Action::Use);
        let p = payloads(&mut i);
        assert_eq!(p.len(), 1);
        let (h, mut r) = ao_net::n3::N3Header::parse(&p[0]).unwrap();
        let Some(Misc::GenericCmd(c)) = ao_net::n3::misc::decode(&h, &mut r).unwrap() else { panic!("not a GenericCmd") };
        assert_eq!((c.state, c.cmd), (0, 3));
        let own = Identity { kind: DYNEL_CHAR, instance: 0x6584 };
        assert_eq!(c.args, GenericArgs::Item { flag: 0, actor: own, item: DOOR });
    }

    #[test]
    fn use_item_refusals_and_characters() {
        let zone = Zone::new(0x6584);
        let mut i = Interact::new(0x6584, (800, 600));
        assert_eq!(i.use_item(&zone, Identity { kind: 0x6a, instance: 3 }, false), Action::Refused("Feedback_ItemsCantBeUsedFromCorpse"));
        assert_eq!(i.use_item(&zone, Identity { kind: 0x69, instance: 3 }, false), Action::Refused("Feedback_ItemCantBeUsedFromBank"));
        assert_eq!(i.take_feedback(), ["Feedback_ItemsCantBeUsedFromCorpse", "Feedback_ItemCantBeUsedFromBank"]);
        // never on oneself, other characters pass (the `Use` key on a target)
        assert_eq!(i.use_item(&zone, Identity { kind: DYNEL_CHAR, instance: 0x6584 }, false), Action::None);
        assert!(payloads(&mut i).is_empty());
        assert_eq!(i.use_item(&zone, Identity { kind: DYNEL_CHAR, instance: 7 }, false), Action::Use);
        assert_eq!(payloads(&mut i).len(), 1);
    }

    #[test]
    fn get_item_message_and_full_bag() {
        let mut zone = Zone::new(0x6584);
        let mut i = Interact::new(0x6584, (800, 600));
        let item = Identity { kind: 0xC74E, instance: 99 };
        assert_eq!(i.get_item(&zone, item), Action::Get);
        let p = payloads(&mut i);
        // key of the class name, header {0xC350, own}, pass-on byte 0, the item identity
        assert_eq!(p[0][..4], message_key("ClientGetItemIIR_t").to_be_bytes());
        assert_eq!(p[0][4..], [0, 0, 0xc3, 0x50, 0, 0, 0x65, 0x84, 0, 0, 0, 0xc7, 0x4e, 0, 0, 0, 99][..]);
        // every bag slot taken: Feedback_InventoryFull and nothing sent
        let entry = ao_net::n3::world::InventoryEntry {
            slot: 0,
            a: 0,
            b: 0,
            id: Identity::default(),
            item: ao_net::n3::world::AcgItem { low_id: 1, high_id: 1, level: 1 },
        };
        for s in BAG_FIRST..BAG_FIRST + BAG_SLOTS {
            zone.inventory.insert(s, ao_net::n3::world::InventoryEntry { slot: s, ..entry });
        }
        assert_eq!(i.get_item(&zone, item), Action::Refused("Feedback_InventoryFull"));
        assert!(payloads(&mut i).is_empty());
    }

    #[test]
    fn default_action_on_objects_follows_can() {
        let mut zone = Zone::new(0x6584);
        let mut i = Interact::new(0x6584, (800, 600));
        let at = |n: i32| Identity { kind: 0xC73D, instance: n };
        for (n, can) in [(1, 1), (2, 8), (3, 0x18), (4, 0x10), (5, 0)] {
            zone.world.test_prop(at(n), vec![(CAN_STAT, can)]);
        }
        assert_eq!(i.default_action_on(&zone, at(1)), Action::Get);
        assert_eq!(i.default_action_on(&zone, at(2)), Action::Use);
        // bit 3 with bit 4: `UseItem(id, false)` asks the confirmation instead of sending
        assert_eq!(i.default_action_on(&zone, at(3)), Action::Confirm);
        assert_eq!(i.default_action_on(&zone, at(4)), Action::None);
        assert_eq!(i.default_action_on(&zone, at(5)), Action::None);
        assert_eq!(payloads(&mut i).len(), 2, "get + use only");
        // the dialog's Yes: `UseItem(id, true)` skips the confirmation
        assert_eq!(i.use_item(&zone, at(3), true), Action::Use);
        assert_eq!(payloads(&mut i).len(), 1);
    }

    /// `InventoryUpdateIIR_t` (header = the own character) for the corpse `{0xC76A, 5}`: capacity 0x15, kind 0, `items` = (slot, item id), word 0x70, flag.
    fn loot_frame(own: u32, items: &[(u32, i32)], flag: bool) -> ao_net::frame::Frame {
        let mut b = vec![];
        let put = |b: &mut Vec<u8>, v: u32| b.extend(v.to_be_bytes());
        put(&mut b, inventory::INVENTORY_UPDATE);
        b.extend([0, 0, 0xc3, 0x50]);
        put(&mut b, own);
        b.push(0);
        put(&mut b, 0x15);
        put(&mut b, 0);
        put(&mut b, (items.len() as u32 + 1) * 0x3f1);
        for &(slot, id) in items {
            put(&mut b, slot);
            b.extend([0, 0, 0, 0]);
            put(&mut b, 0);
            put(&mut b, 0);
            [id, id, 20, 0].iter().for_each(|&v| put(&mut b, v as u32));
        }
        put(&mut b, 0xC76A);
        put(&mut b, 5);
        put(&mut b, 0x70);
        put(&mut b, u32::from(flag));
        n3_frame(1, own, b)
    }

    #[test]
    fn corpse_loot_window_opens_with_the_flag_and_a_double_click_takes_an_item() {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() {
            return;
        }
        let mut gui = Gui::new(&client, None).unwrap();
        gui.set_screen_size(1280, 800);
        let mut zone = Zone::new(0x6584);
        let mut i = Interact::new(0x6584, (1280, 800));
        i.set_client_dir(client);
        let corpse = Identity { kind: 0xC76A, instance: 5 };
        i.on_frame(&mut gui, &loot_frame(0x6584, &[(0, 21_797), (4, 21_797)], false), &mut zone);
        assert!(i.loot_dump(&mut gui).is_empty(), "no window without the open flag");
        i.on_frame(&mut gui, &loot_frame(0x6584, &[(0, 21_797), (4, 21_797)], true), &mut zone);
        let dump = i.loot_dump(&mut gui);
        assert_eq!(dump.len(), 1);
        assert_eq!((dump[0].0, dump[0].1.len()), (corpse, 2));
        assert!(!dump[0].1[0].2.is_empty(), "the item has its record name: {dump:?}");
        // the cell of the second item; a double click sends the move to any bag slot with `{0x6b, word << 16 | slot}` (`FUN_1007de99`)
        let w = i.use_ui.loot_window(corpse).unwrap();
        let (ox, oy) = super::super::hud_stats::inv_grid::cell_origin(1, 0, (10.0, 10.0));
        let click = Event::CanvasClick { window: w, view: "grid".into(), x: ox + 2.0, y: oy + 2.0 };
        i.tick(1.0);
        assert!(i.event(&mut gui, &click, &zone));
        assert!(payloads(&mut i).is_empty());
        i.tick(1.2);
        assert!(i.event(&mut gui, &click, &zone));
        let p = payloads(&mut i);
        assert_eq!(p, [inventory::move_item_to_inventory(0x6584, Identity { kind: 0x6b, instance: 0x0070_0004 }, ANY_BAG_SLOT)]);
    }

    /// The same capture through the loot window: the window opens with the update and the server's `ContainerAddItemIIR_t` empties the taken cell.
    #[test]
    fn captured_take_empties_the_loot_window() {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() {
            return;
        }
        let mut gui = Gui::new(&client, None).unwrap();
        gui.set_screen_size(1280, 800);
        let mut zone = Zone::new(0x830e);
        let mut i = Interact::new(0x830e, (1280, 800));
        i.set_client_dir(client);
        let mut rows = vec![];
        for l in include_str!("../../../../docs/captures/zone_loot_take_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
            if dir == "<" {
                let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
                i.on_frame(&mut gui, &ao_net::frame::Frame::decode_with(&b, false).unwrap().unwrap().0, &mut zone);
                rows.push(i.loot_dump(&mut gui).iter().map(|(_, r)| r.len()).sum::<usize>());
            }
        }
        assert!(rows.contains(&1), "{rows:?}");
        assert_eq!(rows.last(), Some(&0), "{rows:?}");
    }

    #[test]
    fn unknown_objects_do_nothing() {
        let zone = Zone::new(0x6584);
        let mut i = Interact::new(0x6584, (800, 600));
        assert_eq!(Interact::can_of(&zone, DOOR), None);
        assert_eq!(i.default_action_on(&zone, DOOR), Action::None);
        assert!(payloads(&mut i).is_empty());
    }
}
