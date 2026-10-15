//! `perk_window` (`PerkWindowModule_c` 0x10063373, `PerkView_c` `FUN_100611dc`, window ctor `FUN_1006372d`): the Perks window, Shift+P / `CharMenu.xml`
//! entry `perk_window`. Data: the perk catalogue rdb 1000036, the own perk map (`FullCharacterIIR_t`, `PerkUpdateIIR`), train / untrain requests.
//! Evidence, addresses and what is unresolved: docs/gui.md §11.13.

use super::hud_dialog::Dialogs;
use super::zone::Zone;
use ao_formats::dynel_visual::item_template;
use ao_formats::screens::TextDb;
use ao_gui::{Event, Gui, ViewHandle, WindowId, WindowSize};
use ao_net::frame::Frame;
use ao_net::n3::{self, team, world::World, N3};
use ao_rdb::RecordStore;
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

/// The perk catalogue resource (`FUN_1002bdd0`: `GetIdentityVec(0xf4264)`).
const PERK_LIST: u32 = 1_000_036;
/// Perk flag bits (`Perk_t+0xc`): 0 special (never listed, `FUN_10062f21`), 1 alien (`IsAlienPerk`), 2..5 breed 1..4 only, 6 / 7 other kinds the window skips (`& 0xc0`).
mod flag {
    pub const SPECIAL: u32 = 1;
    pub const ALIEN: u32 = 2;
    pub const BREEDS: [u32; 4] = [4, 8, 0x10, 0x20];
    pub const SKIPPED: u32 = 0xc0;
}
/// Stats the window reads: `Breed` 4, `Level` 0x36, `Profession` 0x3c, `AlienLevel` 0xa9, `Expansion` 0x185, `LastPerkResetTime` 0x241.
const BREED: u32 = 4;
const LEVEL: u32 = 0x36;
const PROFESSION: u32 = 0x3c;
const ALIEN_LEVEL: u32 = 0xa9;
const EXPANSION: u32 = 0x185;
const LAST_RESET: u32 = 0x241;
/// "You can only remove a perk you have chosen once every %d hours": the literal `2` pushed in `FUN_100607f6` and `FUN_10060ea7`.
const HOURS: i32 = 2;
/// `StatusIcon_c` 0x1005f7de and `FUN_1005e2bf`: state tints, not label colours.
const STATUS_ICON: ao_gui::GfxId = ao_gui::GfxId(0x121);
fn status_color(eligible: bool, known: bool) -> u32 {
    if known { 0x006e00 } else if eligible { 0x646464 } else { 0x960000 }
}
/// `PerkWindow_c` 0x1006372d: client Rect(200,300,600,500), before MoveToCenter.
const CLIENT: (u32, u32) = (401, 201);

/// One catalogue record (`Perk_t`, 36 bytes little endian: `item, precursor, flags, profession mask, 5 more words`; `FUN_1002ba03` reads the first six).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PerkDef {
    pub id: u32,
    /// rdb 1000020 item whose name / description the perk shows.
    pub item: u32,
    /// Previous perk of the line (0 = first).
    pub precursor: u32,
    pub flags: u32,
    /// Profession bit set: 0 general, one bit = profession perk, several = group perk (`IsProfessionPerk` / `IsGroupPerk` / `IsGeneralPerk`).
    pub mask: u32,
    /// RDB word 4 / runtime +0x14: granted Action_e, not an item template (`GC 10052aca`).
    pub action: u32,
    /// RDB word 5 / Perk_t+0x18: research goal's required contribution (GC 1002ba03).
    pub research_total: u32,
}

#[derive(Default)]
pub(super) struct PerkDb {
    pub(super) defs: Vec<PerkDef>,
    index: HashMap<u32, usize>,
}

impl PerkDb {
    pub(super) fn from_defs(defs: Vec<PerkDef>) -> Self {
        let index = defs.iter().enumerate().map(|(i, d)| (d.id, i)).collect();
        Self { defs, index }
    }

    pub(super) fn load(store: &RecordStore) -> Self {
        let defs = store
            .ids(PERK_LIST)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|id| {
                let r = store.get(PERK_LIST, id).ok().flatten()?;
                let w = |i: usize| r.get(4 * i..4 * i + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
                Some(PerkDef { id, item: w(0)?, precursor: w(1)?, flags: w(2)?, mask: w(3)?, action: w(4)?, research_total: w(5)? })
            })
            .collect();
        Self::from_defs(defs)
    }

    pub(super) fn get(&self, id: u32) -> Option<&PerkDef> {
        self.index.get(&id).map(|&i| &self.defs[i])
    }

    /// `N3Msg_IsProfessionPerk` / `IsGroupPerk` / `IsGeneralPerk`: tab 0, 1 or 2 of a perk for a character of `profession`.
    pub(super) fn tab_of(&self, id: u32, profession: i32) -> Option<usize> {
        let d = self.get(id)?;
        let bit = 1u32.checked_shl(profession as u32 & 0x1f).unwrap_or(0);
        if d.mask != 0 && bit & d.mask == d.mask {
            Some(0)
        } else if bit & d.mask != 0 {
            Some(1)
        } else if d.mask == 0 {
            Some(2)
        } else {
            None
        }
    }

    /// `PerkWindowModule_c::LoadPerks` (`FUN_10062f21`): the perk lines of the three tabs for a character with `expansion` (stat 0x185), `breed`, `profession`.
    /// A line is the chain `root -> next perk whose precursor is it` (`FUN_100538c1`); only perks the character may see are chained.
    /// Presentation ordering depends on live eligibility/known state and root names (`HudPerks::lines`).
    pub(super) fn lines(&self, expansion: i32, breed: i32, profession: i32) -> [Vec<Vec<u32>>; 3] {
        let shown = |d: &PerkDef| {
            if d.flags & flag::SPECIAL != 0 {
                return false;
            }
            let expansion_ok = if d.flags & flag::ALIEN != 0 {
                expansion & 8 != 0
            } else {
                d.flags & flag::SKIPPED == 0 && expansion & 2 != 0
            };
            expansion_ok && flag::BREEDS.iter().enumerate().all(|(i, f)| d.flags & f == 0 || breed == i as i32 + 1)
        };
        let mut next: HashMap<u32, u32> = HashMap::new();
        for d in self.defs.iter().filter(|d| shown(d) && d.precursor != 0) {
            next.insert(d.precursor, d.id);
        }
        let mut out: [Vec<Vec<u32>>; 3] = Default::default();
        for root in self.defs.iter().filter(|d| shown(d) && d.precursor == 0) {
            let Some(tab) = self.tab_of(root.id, profession) else { continue };
            let mut line = vec![root.id];
            while let Some(&n) = next.get(line.last().unwrap()) {
                if line.len() > 64 || line.contains(&n) {
                    break;
                }
                line.push(n);
            }
            out[tab].push(line);
        }
        out
    }
}

/// The own perk map: perk id -> timer (`entry+0x1c`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct Known(pub BTreeMap<i32, i32>);

impl Known {
    /// `GC 10052aca`: learned catalogue entries grant their nonzero Action_e.
    #[cfg(test)]
    pub(super) fn actions<'a>(&'a self, db: &'a PerkDb) -> impl Iterator<Item = u32> + 'a {
        self.0.keys().filter_map(|&id| db.get(id as u32)).map(|d| d.action).filter(|&a| a != 0)
    }

    /// `FUN_10052bc9` / `FUN_10052c6d`: the perks of `alien` (flag bit 1) or normal kind that count as used (`flags & 0xc2 == 0`, resp. bit 1).
    pub(super) fn used(&self, db: &PerkDb, alien: bool) -> i32 {
        self.0.keys().filter_map(|&id| db.get(id as u32)).filter(|d| if alien { d.flags & flag::ALIEN != 0 } else { d.flags & (flag::SKIPPED | flag::ALIEN) == 0 }).count() as i32
    }
}

/// `FUN_10052bfc`: all points of a character = `level / 10` below level 200, `level - 180` from there (`FUN_10052bfc`).
pub(super) fn total_points(level: i32) -> i32 {
    if level < 200 { level / 10 } else { level - 0xb4 }
}

/// Which request the player is confirming.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ask {
    Train(u32),
    Untrain(u32),
    Info,
}

struct Win {
    window: WindowId,
    bottom: ViewHandle,
    /// What the list was last built from: `(tab, known, points)`; rebuilt when it changes.
    drawn: Option<(usize, Known, [i32; 4], bool)>,
}

pub(super) struct HudPerks {
    pub(super) db: PerkDb,
    pub(super) store: Option<RecordStore>,
    names: HashMap<u32, String>,
    criteria: HashMap<u32, super::chat::info::PerkCriteria>,
    equipment: super::chat::info::PerkEquipment,
    eligibility_stats: HashMap<u32, i32>,
    texts: Option<TextDb>,
    pub(super) known: Known,
    win: Option<Win>,
    /// `SelectedTab` of `PerkWindowConfig`.
    tab: usize,
    /// Native line selection is retained independently in each of the three tabs (`0x10060702`).
    selected: [Option<u32>; 3],
    /// The "Remove Perk" toggle (`UntrainPerk`): the next perk click untrains.
    untrain: bool,
    screen: (u32, u32),
    dialogs: Dialogs<Ask>,
    pub(super) outbox: Vec<Frame>,
    closed: bool,
}

impl HudPerks {
    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> Self {
        let store = RecordStore::open(dir).ok();
        Self {
            db: store.as_ref().map(PerkDb::load).unwrap_or_default(),
            store,
            names: HashMap::new(),
            criteria: HashMap::new(),
            equipment: super::chat::info::PerkEquipment::default(),
            eligibility_stats: HashMap::new(),
            texts: TextDb::load(dir).ok(),
            known: Known::default(),
            win: None,
            tab: 0,
            selected: [None; 3],
            untrain: false,
            screen,
            dialogs: Dialogs::default(),
            outbox: vec![],
            closed: false,
        }
    }

    pub(super) fn set_screen(&mut self, screen: (u32, u32)) {
        self.screen = screen;
    }

    pub(super) fn take_closed(&mut self) -> bool {
        std::mem::take(&mut self.closed)
    }

    fn text(&self, key: &str) -> String {
        self.texts.as_ref().and_then(|t| t.by_key(10000, key)).unwrap_or_else(|| key.to_string())
    }

    /// Native alien markup's colour, expressed separately because Button labels are plain text.
    fn label_attrs(&self, id: u32, label: &str) -> String {
        let color = if self.db.get(id).is_some_and(|d| d.flags & flag::ALIEN != 0) { " label_color=\"0x30ff30\"" } else { "" };
        format!("label=\"{}\"{color}", super::hud_listview::esc(label))
    }

    /// Own perk map / perk updates from the zone stream (`FUN_100537a5`, `FUN_10063942`).
    pub(super) fn on_frame(&mut self, f: &Frame, zone: &mut Zone) {
        let own = zone.char_id as i32;
        let Ok(m) = n3::decode(f) else { return };
        let mut changed = false;
        match &m.body {
            // A fully decoded empty perk map leaves rest empty; earlier unsupported blocks retain their size word.
            N3::World(World::FullCharacter(c)) if m.header.target.kind == 50000 && m.header.target.instance == own && c.rest.is_empty() => {
                self.known.0.clear();
                changed = true;
            }
            // `FullCharacter::rest` starts at the first block the decoder could not read: the perk map when the spell list before it is empty
            N3::World(World::FullCharacter(c)) if m.header.target.kind == 50000 && m.header.target.instance == own && !c.rest.is_empty() => {
                if let Ok(p) = team::parse_perk_map(&c.rest) {
                    self.known = Known(p.into_iter().map(|k| (k.perk, k.timer)).collect());
                    changed = true;
                }
            }
            N3::Unknown(b) if m.header.msg_type == team::PERK_UPDATE && m.header.target.kind == 50000 && m.header.target.instance == own => match team::parse_perk_update(b) {
                // `FUN_10063942`: the entry is created when missing (`FUN_100533b3`), its timer set (`FUN_10052842`: negative = 0)
                Ok(u) => {
                    zone.stats.insert(0x109, u.stat_109);
                    if !self.known.0.contains_key(&u.perk) {
                        if let Err(e) = self.train_relay(u.perk, zone) { eprintln!("perks: update {}: {e:#}", u.perk); }
                    }
                    if let Some(timer) = self.known.0.get_mut(&u.perk) {
                        *timer = u.remaining.max(0);
                        changed = true;
                    }
                }
                Err(e) => eprintln!("perks: PerkUpdate: {e:#}"),
            },
            N3::World(World::CharacterAction(a)) if m.header.target.kind == 50000 && m.header.target.instance == own => {
                let id = a.identity_b.instance;
                match a.action {
                    // GC1005e4f9/1005e512 push [identity_b+4], not param or identity_a.
                    0xbb if !self.known.0.contains_key(&id) => {
                        if let Err(e) = self.train_relay(id, zone) { eprintln!("perks: train {id}: {e:#}"); }
                        changed = self.known.0.contains_key(&id);
                    }
                    0xbc => {
                        if let Some(&timer) = self.known.0.get(&id) {
                            if let Some(now) = zone.server_now() { zone.stats.insert(LAST_RESET, now as i32); }
                            if timer == 0 {
                                if let Err(e) = self.wear_effects(id as u32, zone, false) { eprintln!("perks: untrain {id} effects: {e:#}"); }
                            }
                            self.known.0.remove(&id);
                            changed = true;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        if changed { self.publish_completed(zone); }
    }

    fn publish_completed(&self, zone: &mut Zone) {
        if zone.global_perk_ids.is_none() {
            zone.global_perk_ids = Some(self.db.defs.iter().filter(|d| d.flags & 0x80 != 0).map(|d| d.id).collect());
        }
        let completed = zone.personal_completed_perks.get_or_insert_with(Vec::new);
        completed.clear();
        completed.extend(self.known.0.iter().filter(|(id, timer)|
            **timer == 0 && self.db.get(**id as u32).is_some_and(|d| d.flags & 0x80 == 0)).map(|(&id, _)| id as u32));
        zone.refresh_completed_perks();
    }

    /// GC100533b3: catalogue, expansion, point availability and item wear requirements precede insertion.
    fn train_relay(&mut self, id: i32, zone: &mut Zone) -> anyhow::Result<()> {
        let d = *self.db.get(id as u32).ok_or_else(|| anyhow::anyhow!("missing catalogue perk {id}"))?;
        let alien = d.flags & flag::ALIEN != 0;
        let expansion = zone.skill_value(EXPANSION).unwrap_or(0);
        if d.flags & flag::SPECIAL != 0
            || (!alien && d.flags & flag::SKIPPED == 0 && expansion & 2 == 0)
            || (alien && expansion & 8 == 0)
            || (alien && self.known.used(&self.db, true) >= zone.skill_value(ALIEN_LEVEL).unwrap_or(0))
            || (d.flags & (flag::SKIPPED | flag::ALIEN) == 0 && self.known.used(&self.db, false) >= total_points(zone.skill_value(LEVEL).unwrap_or(0))) {
            return Ok(());
        }
        if let Some(store) = &self.store { self.equipment.refresh(store, zone)?; }
        if !self.eligible(id as u32, zone)? { return Ok(()); }
        let timer = if d.flags & flag::SKIPPED != 0 { d.research_total as i32 } else { 0 };
        self.known.0.insert(id, timer);
        if timer == 0 {
            self.publish_completed(zone);
            self.wear_effects(id as u32, zone, true)?;
        }
        Ok(())
    }

    /// GC10080a79 / 10080cd0 execute the same item event14, with apply / undo.
    fn wear_effects(&self, id: u32, zone: &mut Zone, apply: bool) -> anyhow::Result<()> {
        let d = self.db.get(id).ok_or_else(|| anyhow::anyhow!("missing catalogue perk {id}"))?;
        let store = self.store.as_ref().ok_or_else(|| anyhow::anyhow!("perk effects require client RDB"))?;
        let raw = store.get(1_000_020, d.item)?.ok_or_else(|| anyhow::anyhow!("missing perk item {}", d.item))?;
        let mut spells = super::chat::item_template_spells(&raw, 14)?;
        anyhow::ensure!(zone.completed_perks.is_some() || !spells.iter().any(|s| s.criteria.iter().any(|c| matches!(c[2], 93 | 103))),
            "perk spell criteria require authoritative completed-perk state");
        let mut error = None;
        spells.retain(|spell| match super::chat::info::item_eligibility::evaluate(&spell.criteria, zone, zone.completed_perks.as_deref().unwrap_or_default()) {
            Ok(accept) => accept,
            Err(e) => { error = Some(e); false }
        });
        if let Some(e) = error { return Err(e); }
        zone.apply_effects(&spells, apply);
        zone.own_events.push(super::zone::OwnEvent::Spells { spells, apply });
        // UNRESOLVED: native item class flags / ownership side effects of 10080a79/10080cd0
        // are not represented by the shared spell executor; see docs/gui.md §11.13.
        Ok(())
    }

    fn eligible(&mut self, id: u32, zone: &Zone) -> anyhow::Result<bool> {
        let def = self.db.get(id).ok_or_else(|| anyhow::anyhow!("missing perk {id}"))?;
        let store = self.store.as_ref().ok_or_else(|| anyhow::anyhow!("perk eligibility requires client RDB"))?;
        if let std::collections::hash_map::Entry::Vacant(entry) = self.criteria.entry(id) {
            let raw = store.get(1_000_020, def.item)?.ok_or_else(|| anyhow::anyhow!("missing perk item {}", def.item))?;
            entry.insert(super::chat::info::PerkCriteria::from_record(&raw)?);
        }
        self.criteria[&id].eligible(zone, self.equipment.armor(), zone.completed_perks.as_deref().unwrap_or_default())
    }

    /// `N3Msg_GetPerkName(id, true)` (`FUN_1005366e`): the item's name, then a space and the perk's position in its line.
    pub(super) fn name(&mut self, id: u32) -> String {
        if let Some(n) = self.names.get(&id) {
            return n.clone();
        }
        let Some(d) = self.db.get(id).copied() else { return format!("Missing perk: {id}") };
        // `1005f708` obtains every rank's base name recursively from its root.
        let mut level = 1;
        let mut cur = d;
        while cur.precursor != 0 && level < 64 {
            let Some(p) = self.db.get(cur.precursor).copied() else { break };
            cur = p;
            level += 1;
        }
        let base = self.store.as_ref().and_then(|s| item_template(s, cur.item).ok().flatten()).and_then(|t| t.name).unwrap_or_default();
        let n = format!("{base} {level}");
        self.names.insert(id, n.clone());
        n
    }

    fn state(&mut self, id: u32, zone: &Zone) -> (bool, bool) {
        let eligible = match self.eligible(id, zone) {
            Ok(value) => value,
            Err(e) => { eprintln!("perks item{id}: {e:#}"); false }
        };
        (eligible, self.known.0.contains_key(&(id as i32)))
    }

    /// GUI `10063ad6`: criteria first, then learned, then bytewise root name.
    fn lines(&mut self, zone: &Zone) -> [Vec<Vec<u32>>; 3] {
        let mut lines = self.db.lines(zone.skill_value(EXPANSION).unwrap_or(0), zone.stat(BREED).unwrap_or(0), zone.stat(PROFESSION).unwrap_or(0));
        for tab in &mut lines {
            tab.sort_by_cached_key(|line| {
                let (eligible, known) = self.state(line[0], zone);
                let name = self.name(line[0]);
                let base = name.rsplit_once(' ').map_or(name.as_str(), |(base, _)| base);
                (!eligible, !known, base.to_owned())
            });
        }
        lines
    }

    pub(super) fn open(&mut self, gui: &mut Gui) {
        if self.win.is_some() {
            return;
        }
        // `PerkView` 0x100611dc: horizontal line-list and selected-line columns.
        let xml = "<root><View view_layout=\"vertical\" name=\"perk_root\">\
                   <View view_layout=\"horizontal\">\
                   <View view_layout=\"vertical\"><TextView name=\"line_heading\" font=\"BOLD\" layout_borders=\"Rect(5,10,5,5)\"/>\
                   <BorderView layout_borders=\"Rect(5,0,0,5)\"><ScrollView name=\"lines_scroll\" v_scrollbar_mode=\"always\" max_size=\"Point(16000,16000)\"><ScrollViewChild><View view_layout=\"vertical\" name=\"lines\"/></ScrollViewChild></ScrollView></BorderView></View>\
                   <View view_layout=\"vertical\"><TextView name=\"selected_name\" font=\"BOLD\" layout_borders=\"Rect(5,10,5,5)\"/>\
                   <BorderView layout_borders=\"Rect(5,0,5,5)\"><View name=\"ranks\" view_layout=\"horizontal\"/></BorderView>\
                   <View view_layout=\"vertical\" name=\"points\"/>\
                   <VLayoutSpacer min_size=\"Point(-1,5)\"/><View view_layout=\"horizontal\" name=\"bottom\" layout_borders=\"Rect(5,5,5,5)\"/></View>\
                   </View></View></root>";
        let title = self.text("PerkWindow");
        let pos = ((self.screen.0 as i32 - CLIENT.0 as i32 - 10) / 2, (self.screen.1 as i32 - CLIENT.1 as i32 - 31) / 2);
        match gui.open_tabbed_window_xml("PerkWindow", &title, xml, pos, WindowSize::Fixed(CLIENT.0, CLIENT.1)) {
            Ok(window) => {
                gui.set_window_preferred_size_limits(window, true);
                let tabs = ["ProfessionPerks", "GroupPerks", "GeneralPerks"].map(|k| self.text(k));
                gui.set_window_tabs(window, &tabs, self.tab);
                gui.set_text(window, "line_heading", &self.text("PerkLines"));
                // "Remove Perk" toggle (`FUN_100611dc`, tooltip `UntrainPerkInfo`) and the untrain timeout (`UntrainPerkTimeout`, `FUN_10060553`)
                let bottom = format!(
                    "<root><View view_layout=\"horizontal\"><Button name=\"untrain\" label=\"{}\"/><TextView name=\"timeout\" value=\"--:--:--\" layout_borders=\"Rect(5,0,10,0)\"/></View></root>",
                    self.text("RemovePerk")
                );
                match gui.add_view_xml(window, "bottom", "PerkBottom", &bottom) {
                    Ok(bottom) => self.win = Some(Win { window, bottom, drawn: None }),
                    Err(e) => {
                        eprintln!("hud: perk_window: {e:#}");
                        gui.close_window(window);
                    }
                }
            }
            Err(e) => eprintln!("hud: perk_window: {e:#}"),
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui) {
        if let Some(w) = self.win.take() {
            gui.close_window(w.window);
        }
        self.dialogs.close_all(gui);
    }

    /// The points of the `PerkView_c` labels: `(used, available, alien used, alien available)`.
    pub(super) fn points(&self, zone: &Zone) -> [i32; 4] {
        let level = zone.stat(LEVEL).unwrap_or(0);
        let (used, alien_used) = (self.known.used(&self.db, false), self.known.used(&self.db, true));
        [used, total_points(level) - used, alien_used, zone.stat(ALIEN_LEVEL).unwrap_or(0) - alien_used]
    }

    /// Per-frame refresh: tab, point labels (`FUN_100611dc` label groups), the perk lines, the untrain timeout (`FUN_10060553`).
    pub(super) fn update(&mut self, gui: &mut Gui, zone: &Zone) {
        let Some(mut w) = self.win.take() else { return };
        let stats_changed = self.eligibility_stats.len() != zone.stats.len() + zone.skill_values.keys().filter(|key| !zone.stats.contains_key(key)).count()
            || zone.stats.iter().chain(zone.skill_values.iter()).any(|(&stat, _)| self.eligibility_stats.get(&stat).copied() != zone.skill_value(stat));
        let equipment_changed = self.store.as_ref().map(|store| self.equipment.refresh(store, zone)).transpose();
        if stats_changed || equipment_changed.as_ref().is_ok_and(|changed| *changed == Some(true)) {
            self.eligibility_stats.clone_from(&zone.stats);
            self.eligibility_stats.extend(zone.skill_values.iter().map(|(&stat, &value)| (stat, value)));
            w.drawn = None;
        }
        if let Err(e) = equipment_changed { eprintln!("perks equipment: {e:#}"); self.win = Some(w); return; }
        let points = self.points(zone);
        let expansion = zone.skill_value(EXPANSION).unwrap_or(0);
        let alien = expansion & 8 != 0 && zone.stat(ALIEN_LEVEL).unwrap_or(0) >= 1;
        let sig = (self.tab, self.known.clone(), points, alien);
        if w.drawn.as_ref() != Some(&sig) {
            self.rebuild(gui, &mut w, zone, points, alien);
            w.drawn = Some(sig);
        }
        gui.set_toggle_in(w.bottom, "untrain", true, self.untrain);
        // `FUN_10060553`: the time left until the next untrain is possible as `%02u:%02u:%02u`, `--:--:--` when it is free; the stat holds the last reset's time
        let timeout = self.timeout(zone);
        gui.set_text(w.window, "timeout", &timeout);
        self.win = Some(w);
    }

    /// Native reset timestamps and countdown use GameTime, never the host Unix clock.
    fn timeout(&self, zone: &Zone) -> String {
        let last = zone.stat(LAST_RESET).unwrap_or(0);
        let Some(now) = zone.server_now().map(i64::from) else { return "--:--:--".to_string() };
        let left = if last > 0 { i64::from(last) + i64::from(HOURS) * 3600 - now } else { 0 };
        if left <= 0 { "--:--:--".to_string() } else { format!("{:02}:{:02}:{:02}", left / 3600, left / 60 % 60, left % 60) }
    }

    fn rebuild(&mut self, gui: &mut Gui, w: &mut Win, zone: &Zone, points: [i32; 4], alien: bool) {
        // point labels: "Used Perk Points: 3" (`FUN_100611dc`: GetText key, then the number), colour 0xccffff
        gui.remove_children(w.window, "points");
        let mut labels = vec![("UsedPerkPoints", points[0]), ("AvailablePerkPoints", points[1])];
        if alien {
            labels.extend([("UsedAlienPerkPoints", points[2]), ("AvailableAlienPerkPoints", points[3])]);
        }
        for (k, v) in labels {
            let top = if k.starts_with("Available") { 5 } else { 3 };
            let src = format!("<root><TextView value=\"{}{}\" color=\"0xCCFFFF\" layout_borders=\"Rect(10,{top},10,0)\"/></root>", self.text(k), v);
            let _ = gui.add_view_xml(w.window, "points", "PerkPoints", &src);
        }
        let lines = self.lines(zone);
        // `LoadPerks` 0x10062f21 measures all tabs: root labels and numbered rank labels,
        // using normal-font string width + 35 (0x1005e320, double at 0x101b3300).
        let mut widths = [1, 1];
        for line in lines.iter().flatten() {
            let name = self.name(line[0]);
            let base = name.rsplit_once(' ').map_or(name.as_str(), |(base, _)| base);
            widths[0] = widths[0].max(gui.fonts().font(ao_gui::FontId::Normal).text_width(base) + 36);
            for n in 1..=line.len() {
                widths[1] = widths[1].max(gui.fonts().font(ao_gui::FontId::Normal).text_width(&format!("{base} {n}")) + 36);
            }
        }
        let row_height = gui.fonts().font(ao_gui::FontId::Normal).height + 9;
        gui.remove_children(w.window, "lines");
        for (k, line) in lines[self.tab.min(2)].iter().enumerate() {
            let name = self.name(line[0]);
            let base = name.rsplit_once(' ').map_or(name.as_str(), |(base, _)| base);
            let label = self.label_attrs(line[0], base);
            let src = format!("<root><Button name=\"line{k}\" {label} min_size=\"Point({},-1)\" max_size=\"Point(16000,-1)\" layout_borders=\"Rect(0,0,0,4)\"/></root>", widths[0] - 1);
            if let Ok(handle) = gui.add_view_xml(w.window, "lines", "PerkLine", &src) {
                let (eligible, known) = self.state(line[0], zone);
                gui.set_button_icon_view(w.window, &format!("line{k}"), STATUS_ICON, status_color(eligible, known));
                gui.set_toggle_in(handle, &format!("line{k}"), true, self.selected[self.tab] == Some(line[0]));
            }
        }
        gui.set_view_pref_size(w.window, "lines_scroll", (widths[0] as f32 + 9.0, row_height as f32 + 3.0), (16000.0, 16000.0));
        gui.remove_children(w.window, "ranks");
        let selected = self.selected[self.tab].and_then(|id| lines[self.tab].iter().find(|line| line[0] == id));
        let base = selected.map(|line| self.name(line[0])).unwrap_or_default();
        let base = base.rsplit_once(' ').map_or(base.as_str(), |(base, _)| base);
        gui.set_text(w.window, "selected_name", base);
        // `PerkListView` 0x1005e4b1: fixed two columns / five rows; SetFrame 0x1005ec59
        // places the first ceil(n/2) ranks in the left column, remaining ranks in the right.
        let count = selected.map_or(0, Vec::len);
        let split = count.div_ceil(2);
        for col in 0..2 {
            let mut src = format!("<root><View view_layout=\"vertical\" layout_borders=\"Rect(0,0,{},0)\">", if col == 0 { 5 } else { 0 });
            for row in 0..5 {
                let index = col * split + row;
                if row < split && index < count {
                    let id = selected.unwrap()[index];
                    let label = format!("{} {}", base, index + 1);
                    let label = self.label_attrs(id, &label);
                    src += &format!("<Button name=\"perk_{id}\" {label} min_size=\"Point({}, {})\" max_size=\"Point({}, {})\" layout_borders=\"Rect(0,0,0,{})\"/>", widths[1] - 1, row_height - 1, widths[1] - 1, row_height - 1, if row < 4 { 5 } else { 0 });
                } else {
                    src += &format!("<View min_size=\"Point({}, {})\" max_size=\"Point({}, {})\" layout_borders=\"Rect(0,0,0,{})\"/>", widths[1] - 1, row_height - 1, widths[1] - 1, row_height - 1, if row < 4 { 5 } else { 0 });
                }
            }
            src += "</View></root>";
            if let Ok(handle) = gui.add_view_xml(w.window, "ranks", "PerkRanks", &src) {
                if let Some(line) = selected {
                    for &id in line.iter().skip(col * split).take(split) {
                        let (eligible, known) = self.state(id, zone);
                        gui.set_button_icon_view(w.window, &format!("perk_{id}"), STATUS_ICON, status_color(eligible, known));
                        gui.set_toggle_in(handle, &format!("perk_{id}"), true, known);
                    }
                }
            }
        }
        gui.relayout_window(w.window);
    }

    fn info_box(&mut self, gui: &mut Gui, key: &str, how: &str) {
        let body = format!("{}\n{}", self.text(key), self.text(how));
        let ok = [self.text("MsgBox_OK")];
        self.dialogs.go(gui, self.screen, Ask::Info, &body, &ok);
    }

    /// `FUN_100607f6` (click on a perk to train it) and `FUN_10060ea7` (the same with the "Remove Perk" toggle on).
    fn click(&mut self, gui: &mut Gui, id: u32, zone: &Zone) {
        let Some(d) = self.db.get(id).copied() else { return };
        let known = self.known.0.contains_key(&(id as i32));
        if self.untrain {
            if !known {
                return self.info_box(gui, "MustKnowPerk2Untrain", "Info");
            }
            let body = self.text("ReallyRemovePerk").replacen("%s", &self.name(id), 1).replacen("%d", &HOURS.to_string(), 1);
            let buttons = [self.text("MsgBox_Yes"), self.text("MsgBox_No")];
            return self.dialogs.go(gui, self.screen, Ask::Untrain(id), &body, &buttons);
        }
        let points = self.points(zone);
        if known {
            return self.info_box(gui, "YouKnowThisPerk", "Info");
        }
        let alien = d.flags & flag::ALIEN != 0;
        if (alien && points[3] <= 0) || (!alien && points[1] <= 0) {
            return if alien { self.info_box(gui, "NoAPerkPts", "HowToGetAPerks") } else { self.info_box(gui, "NoPerkPts", "HowToGetPerks") };
        }
        if d.precursor != 0 && !self.known.0.contains_key(&(d.precursor as i32)) {
            return self.info_box(gui, "MustTrainPrecursoryPerk", "Info");
        }
        if let Some(store) = &self.store {
            if let Err(e) = self.equipment.refresh(store, zone) { eprintln!("perks equipment: {e:#}"); return; }
        }
        match self.eligible(id, zone) {
            Ok(true) => {}
            Ok(false) => return self.info_box(gui, "DoNotMeetPerkReq", "Info"),
            Err(e) => { eprintln!("perks item{id}: {e:#}"); return; }
        }
        let body = self.text("ReallyTrainPerk").replacen("%s", &self.name(id), 1).replacen("%d", &HOURS.to_string(), 1);
        let buttons = [self.text("MsgBox_Yes"), self.text("MsgBox_No")];
        self.dialogs.go(gui, self.screen, Ask::Train(id), &body, &buttons);
    }

    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        let (mine, ans) = self.dialogs.event(gui, ev);
        if mine {
            let own = zone.char_id as i32;
            match ans {
                // Yes is button 0: `N3Msg_TrainPerk` / `N3Msg_UntrainPerk`
                Some((Ask::Train(id), 0)) => self.outbox.push(n3::outgoing::n3_frame(0, zone.char_id, team::train_perk(own, id as i32))),
                Some((Ask::Untrain(id), 0)) => {
                    self.outbox.push(n3::outgoing::n3_frame(0, zone.char_id, team::untrain_perk(own, id as i32)));
                    self.untrain = false;
                }
                _ => {}
            }
            return true;
        }
        let Some(w) = &self.win else { return false };
        let window = w.window;
        match ev {
            Event::CloseRequested { window: x } if *x == window => {
                self.close(gui);
                self.closed = true;
                true
            }
            Event::TabSelected { window: x, index } if *x == window && *index < 3 => {
                self.tab = *index;
                true
            }
            Event::Clicked { window: x, view, .. } if *x == window => {
                if let Some(i) = view.strip_prefix("line").and_then(|n| n.parse::<usize>().ok()) {
                    let lines = self.lines(zone);
                    if let Some(line) = lines[self.tab].get(i) {
                        self.selected[self.tab] = Some(line[0]);
                        if let Some(w) = self.win.as_mut() { w.drawn = None; }
                    }
                } else if view == "untrain" {
                    self.untrain = !self.untrain;
                } else if let Some(id) = view.strip_prefix("perk_").and_then(|n| n.parse::<u32>().ok()) {
                    self.click(gui, id, zone);
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(id: u32, precursor: u32, flags: u32, mask: u32) -> PerkDef {
        PerkDef { id, item: 0, precursor, flags, mask, action: 0, research_total: 0 }
    }

    #[test]
    fn real_catalogue_native_order_and_status_icons() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let mut h = HudPerks::new(&dir, (1280, 800));
        let mut z = Zone::new(1);
        z.stats.extend([(PROFESSION, 7), (BREED, 1), (EXPANSION, 0xff), (LEVEL, 10)]);
        for tab in h.lines(&z) {
            let keys: Vec<_> = tab.iter().map(|line| {
                let (eligible, known) = h.state(line[0], &z);
                (!eligible, !known, h.name(line[0]))
            }).collect();
            assert!(keys.windows(2).all(|pair| pair[0] <= pair[1]), "{keys:?}");
        }
        // Catalogue100/item210830 is eligible at profession7/level10.
        h.known.0.insert(100, 0);
        assert_eq!(h.lines(&z)[0][0][0], 100);
        h.selected[0] = Some(100);
        let mut gui = Gui::new(&dir, None).unwrap();
        h.open(&mut gui);
        h.update(&mut gui, &z);
        let icons: Vec<_> = gui.frame(0.0).cmds.into_iter().filter_map(|cmd| match cmd {
            ao_gui::DrawCmd::Gfx { id, tint, .. } if id == STATUS_ICON => Some(tint),
            _ => None,
        }).collect();
        assert!(icons.contains(&[0, 110, 0]), "learned uses native green status art: {icons:?}");
        assert!(icons.contains(&[150, 0, 0]), "unmet ranks use native red status art: {icons:?}");
        assert_eq!((status_color(true, false), status_color(false, true)), (0x646464, 0x006e00));
        let alien = h.db.defs.iter().find(|d| d.precursor == 0 && d.flags & flag::ALIEN != 0 && h.db.tab_of(d.id, 7) == Some(0)).unwrap().id;
        let label = h.name(alien);
        assert_eq!(h.label_attrs(alien, &label), format!("label=\"{}\" label_color=\"0x30ff30\"", super::super::hud_listview::esc(&label)));
        assert!(!h.label_attrs(alien, &label).contains("font"));
        h.selected[0] = Some(alien);
        h.win.as_mut().unwrap().drawn = None;
        h.update(&mut gui, &z);
        assert!(gui.frame(0.0).cmds.iter().any(|cmd| matches!(cmd, ao_gui::DrawCmd::Glyph { tint: [0x30, 0xff, 0x30], .. })), "alien labels emit green glyphs, not literal font tags");
    }

    #[test]
    fn learned_actions_are_catalogue_action_ids_not_perk_items() {
        let mut a = def(111, 110, 0, 32852);
        a.item = 211656;
        a.action = 10111;
        let db = PerkDb::from_defs(vec![a, def(100, 0, 0, 128)]);
        let known = Known(BTreeMap::from([(111, 0), (100, 0), (999, 0)]));
        assert_eq!(known.actions(&db).collect::<Vec<_>>(), [10111]);
    }

    #[test]
    fn perk_training_uses_native_item_requirements() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let mut perks = HudPerks::new(&dir, (1280, 800));
        let mut zone = Zone::new(1);
        // Real catalogue100/item210830 requires profession7 and level>9.
        zone.stats.extend([(PROFESSION, 7), (LEVEL, 9)]);
        assert!(!perks.eligible(100, &zone).unwrap());
        zone.stats.insert(LEVEL, 10);
        assert!(perks.eligible(100, &zone).unwrap());
        zone.stats.insert(PROFESSION, 1);
        assert!(!perks.eligible(100, &zone).unwrap());
    }

    /// The perk lines stack below each other (they were all drawn at the top: a `ScrollViewChild` needs its one layout view inside) and start at the left.
    #[test]
    fn perk_lines_stack_in_the_scroll_view() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() {
            return;
        }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut h = HudPerks::new(&dir, (1280, 800));
        h.open(&mut gui);
        let mut z = Zone::default();
        for (k, v) in [(PROFESSION, 6), (BREED, 1), (EXPANSION, 0xff), (LEVEL, 30)] {
            z.stats.insert(k, v);
        }
        h.update(&mut gui, &z);
        let w = h.win.as_ref().unwrap().window;
        let (a, b) = (gui.view_rect(w, "line0").unwrap(), gui.view_rect(w, "line1").unwrap());
        assert!(b.t > a.t + 5.0 && a.l == b.l, "{a:?} {b:?}");
        let lines = h.lines(&z);
        let (index, line) = lines[0].iter().enumerate().find(|(_, line)| line.len() > 1).unwrap();
        let ids = line.clone();
        assert!(h.event(&mut gui, &Event::Clicked { window: w, view: format!("line{index}"), item: None }, &z));
        h.update(&mut gui, &z);
        let first = gui.view_rect(w, &format!("perk_{}", ids[0])).unwrap();
        let second_column = gui.view_rect(w, &format!("perk_{}", ids[ids.len().div_ceil(2)])).unwrap();
        assert!(second_column.l > first.r);
        let frame = gui.window_outer_frame(w).unwrap();
        gui.set_window_outer_frame(w, (frame.0, frame.1, frame.2 + 100, frame.3 + 80));
        let grown = gui.view_rect(w, &format!("perk_{}", ids[0])).unwrap();
        assert_eq!((grown.width(), grown.height()), (first.width(), first.height()), "rank grid stays preferred-sized while the line list absorbs resize");
    }

    #[test]
    fn lines_tabs_and_points() {
        // line 1-2-3 for profession 6 only, line 10-11 for a group containing profession 6, general 20, alien 30, special 40
        let db = PerkDb::from_defs(vec![def(1, 0, 0, 1 << 6), def(2, 1, 0, 1 << 6), def(3, 2, 0, 1 << 6), def(10, 0, 0, 0b1100_0000), def(11, 10, 0, 0b1100_0000), def(20, 0, 0, 0), def(30, 0, 2, 0), def(40, 0, 1, 0), def(50, 0, 0x40, 0)]);
        let l = db.lines(2, 1, 6);
        assert_eq!(l[0], vec![vec![1, 2, 3]]);
        assert_eq!(l[1], vec![vec![10, 11]]);
        assert_eq!(l[2], vec![vec![20]]);
        // another profession sees only the general perk; alien perks need expansion bit 8
        assert_eq!(db.lines(2, 1, 3), [vec![], vec![], vec![vec![20]]]);
        assert_eq!(db.lines(10, 1, 3)[2], vec![vec![20], vec![30]]);
        let mut k = Known::default();
        k.0.insert(1, 0);
        k.0.insert(30, 0);
        assert_eq!((k.used(&db, false), k.used(&db, true)), (1, 1));
        assert_eq!((total_points(95), total_points(199), total_points(205)), (9, 19, 25));
    }

    #[test]
    fn completed_projection_tracks_authoritative_timer_frames() {
        use ao_net::{msg::Identity, n3::outgoing::n3_frame};
        let mut h = HudPerks::new(&ao_gui::client_dir(), (1024, 768));
        h.db = PerkDb::from_defs(vec![def(100, 0, 0, 0), def(101, 0, 0, 0), def(200, 0, 0x80, 0)]);
        let mut zone = Zone::new(7);
        assert_eq!(zone.completed_perks, None);
        h.known.0.extend([(100, 20), (101, 0), (200, 0), (999, 0)]);
        let update = |own, remaining| n3_frame(0, 7, team::perk_update(Identity { kind: 50000, instance: own },
            &team::PerkUpdate { perk: 100, stat_109: 0, remaining }));
        h.on_frame(&update(8, 0), &mut zone);
        assert_eq!(zone.completed_perks, None, "foreign frames cannot manufacture a snapshot");
        h.on_frame(&update(7, 0), &mut zone);
        assert_eq!(zone.completed_perks.as_deref(), Some(&[100, 101][..]));
        h.on_frame(&update(7, 10), &mut zone);
        assert_eq!(zone.completed_perks.as_deref(), Some(&[101][..]));
        h.on_frame(&n3_frame(0, 7, team::untrain_perk(7, 100)), &mut zone);
        assert_eq!(zone.completed_perks.as_deref(), Some(&[101][..]));
        let header = |key| {
            let mut w = ao_net::Writer::default();
            w.u32(key);
            Identity { kind: 50000, instance: 7 }.write(&mut w);
            w.u8(0);
            w
        };
        let research = |timers: [i32; 3]| {
            let mut w = header(ao_net::n3::research::UPDATE);
            w.u8(1);
            w.i32(200);
            for timer in timers { w.i32(timer); }
            for wrong_source in [101, 999] {
                w.i32(wrong_source);
                for _ in 0..3 { w.i32(0); }
            }
            w.i32(0);
            n3_frame(0, 7, w.0)
        };
        zone.on_frame(&research([0, 10, -1]));
        assert_eq!(zone.completed_perks.as_deref(), Some(&[101, 200][..]));
        let mut side = header(ao_net::n3::dynel::STAT);
        side.i32(1);
        side.i32(33);
        side.i32(1);
        zone.on_frame(&n3_frame(0, 7, side.0));
        assert_eq!(zone.completed_perks.as_deref(), Some(&[101][..]), "side change removes other-side completion");
        zone.on_frame(&research([0, 0, -1]));
        assert_eq!(zone.completed_perks.as_deref(), Some(&[101, 200][..]), "global update completes current-side goal");
        let personal = team::perk_update(Identity { kind: 50000, instance: 7 }, &team::PerkUpdate { perk: 101, stat_109: 0, remaining: 10 });
        h.on_frame(&n3_frame(0, 7, personal), &mut zone);
        assert_eq!(zone.completed_perks.as_deref(), Some(&[200][..]), "personal changes preserve global completion");
    }

    #[test]
    fn perk_map_from_a_frame() {
        let mut h = HudPerks::new(&ao_gui::client_dir(), (1024, 768));
        let p = ao_net::n3::team::perk_update(ao_net::msg::Identity { kind: 50000, instance: 7 }, &ao_net::n3::team::PerkUpdate { perk: 5, stat_109: 0, remaining: -3 });
        h.known.0.insert(5, 99);
        let mut zone = Zone::new(7);
        h.on_frame(&ao_net::n3::outgoing::n3_frame(0, 7, p), &mut zone);
        assert_eq!(h.known.0.get(&5), Some(&0));
    }

    #[test]
    fn authoritative_perk_relays_update_points_effects_and_reset_time() {
        use ao_net::n3::{action, outgoing::n3_frame};
        use ao_net::msg::Identity;
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() { return; }
        let mut h = HudPerks::new(&dir, (1024, 768));
        let mut zone = Zone::new(7);
        zone.stats.extend([(PROFESSION, 7), (LEVEL, 10), (EXPANSION, 2)]);
        let train = n3_frame(0, 7, team::train_perk(7, 100));
        let untrain = n3_frame(0, 7, team::untrain_perk(7, 100));
        assert!(super::super::hud_winb::wants(&train));
        let before = zone.active_spells.clone();
        h.on_frame(&n3_frame(0, 7, team::train_perk(8, 100)), &mut zone);
        let foreign_kind = action::character_action_for(Identity { kind: 1, instance: 7 }, &action::simple(0xbb, Identity::default(), Identity { kind: 0, instance: 100 }));
        h.on_frame(&n3_frame(0, 7, foreign_kind), &mut zone);
        h.on_frame(&n3_frame(0, 7, team::train_perk(7, i32::MAX)), &mut zone);
        assert!(h.known.0.is_empty());
        zone.stats.insert(LEVEL, 9);
        h.on_frame(&train, &mut zone);
        assert!(h.known.0.is_empty(), "native eligibility must reject unmet requirements");
        zone.stats.insert(LEVEL, 10);
        h.on_frame(&train, &mut zone);
        assert_eq!(h.known.0.get(&100), Some(&0));
        assert_eq!(zone.completed_perks.as_deref(), Some(&[100][..]));
        assert_eq!(h.known.used(&h.db, false), 1);
        assert_eq!(total_points(10) - h.known.used(&h.db, false), 0);
        let trained = zone.active_spells.clone();
        assert!(trained.len() > before.len(), "catalogue100 wear event14 must reach modifier storage");
        h.on_frame(&train, &mut zone);
        assert_eq!(zone.active_spells, trained, "duplicate train must not stack effects");
        h.on_frame(&n3_frame(0, 7, team::untrain_perk(8, 100)), &mut zone);
        assert!(h.known.0.contains_key(&100));
        h.on_frame(&untrain, &mut zone);
        assert_eq!(zone.stat(LAST_RESET), None, "no invented clock before server time");
        assert_eq!(zone.active_spells, before);
        assert_eq!(zone.completed_perks.as_deref(), Some(&[][..]));
        assert_eq!(h.known.used(&h.db, false), 0);
        zone.server_unix = Some(100_000.0);
        zone.clock = Some(500.0);
        h.on_frame(&train, &mut zone);
        h.on_frame(&untrain, &mut zone);
        assert_eq!(zone.stat(LAST_RESET), Some(100_000), "GameTime+b4, not the sky/day clock");
        assert_eq!(h.timeout(&zone), "02:00:00");
        zone.server_unix = Some(100_010.0);
        h.on_frame(&untrain, &mut zone);
        assert_eq!(zone.stat(LAST_RESET), Some(100_000), "duplicate untrain must not restart timeout");
        let update = team::perk_update(Identity { kind: 50000, instance: 7 }, &team::PerkUpdate { perk: 100, stat_109: 3, remaining: -1 });
        h.on_frame(&n3_frame(0, 7, update), &mut zone);
        assert_eq!(zone.stat(0x109), Some(3));
        assert_eq!(h.known.0.get(&100), Some(&0));
    }

    #[test]
    fn real_catalogue() {
        let dir = ao_gui::client_dir();
        let Ok(store) = RecordStore::open(&dir) else { return };
        let db = PerkDb::load(&store);
        if db.defs.is_empty() {
            return;
        }
        assert_eq!(db.defs.len(), 2062);
        assert_eq!(db.get(101).map(|d| (d.item, d.precursor)), Some((210831, 100)));
    }
}
