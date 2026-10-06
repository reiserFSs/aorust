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
/// `ReallyTrainPerk` / ... colours of the perk cells: UNRESOLVED GUESS (the `PerkNode` art `FUN_1005fe12` is not decoded); palette of the skill window.
const KNOWN: u32 = 0x7dcc5e;
const OPEN: u32 = 0xffffff;
const LOCKED: u32 = 0x888888;
/// The window's client size: UNRESOLVED GUESS (`PerkWindowConfig` has no saved frame, the list widths come from `GetListViewWidths`).
const CLIENT: (u32, u32) = (440, 320);

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
}

#[derive(Default)]
pub(super) struct PerkDb {
    defs: Vec<PerkDef>,
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
                Some(PerkDef { id, item: w(0)?, precursor: w(1)?, flags: w(2)?, mask: w(3)? })
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
    /// A line is the chain `root -> next perk whose precursor is it` (`FUN_100538c1`); only perks the character may see are chained. The lines of a tab are
    /// ordered by their first perk's id (**UNRESOLVED GUESS**: the original sorts with `FUN_10064147` by a text, which is not decoded).
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
    tabs: ViewHandle,
    bottom: ViewHandle,
    /// What the list was last built from: `(tab, known, points)`; rebuilt when it changes.
    drawn: Option<(usize, Known, [i32; 4], bool)>,
}

pub(super) struct HudPerks {
    db: PerkDb,
    store: Option<RecordStore>,
    names: HashMap<u32, String>,
    texts: Option<TextDb>,
    pub(super) known: Known,
    win: Option<Win>,
    /// `SelectedTab` of `PerkWindowConfig`.
    tab: usize,
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
            texts: TextDb::load(dir).ok(),
            known: Known::default(),
            win: None,
            tab: 0,
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

    /// Own perk map / perk updates from the zone stream (`FUN_100537a5`, `FUN_10063942`).
    pub(super) fn on_frame(&mut self, f: &Frame, own: i32) {
        let Ok(m) = n3::decode(f) else { return };
        match &m.body {
            // `FullCharacter::rest` starts at the first block the decoder could not read: the perk map when the spell list before it is empty
            N3::World(World::FullCharacter(c)) if m.header.target.instance == own && !c.rest.is_empty() => {
                if let Ok(p) = team::parse_perk_map(&c.rest) {
                    self.known = Known(p.into_iter().map(|k| (k.perk, k.timer)).collect());
                }
            }
            N3::Unknown(b) if m.header.msg_type == team::PERK_UPDATE && m.header.target.instance == own => match team::parse_perk_update(b) {
                // `FUN_10063942`: the entry is created when missing (`FUN_100533b3`), its timer set (`FUN_10052842`: negative = 0)
                Ok(u) => {
                    self.known.0.insert(u.perk, u.remaining.max(0));
                }
                Err(e) => eprintln!("perks: PerkUpdate: {e:#}"),
            },
            _ => {}
        }
    }

    /// `N3Msg_GetPerkName(id, true)` (`FUN_1005366e`): the item's name, then a space and the perk's position in its line.
    fn name(&mut self, id: u32) -> String {
        if let Some(n) = self.names.get(&id) {
            return n.clone();
        }
        let Some(d) = self.db.get(id).copied() else { return format!("Missing perk: {id}") };
        let base = self.store.as_ref().and_then(|s| item_template(s, d.item).ok().flatten()).and_then(|t| t.name).unwrap_or_default();
        let mut level = 1;
        let mut cur = d;
        while cur.precursor != 0 && level < 64 {
            let Some(p) = self.db.get(cur.precursor).copied() else { break };
            cur = p;
            level += 1;
        }
        let n = format!("{base} {level}");
        self.names.insert(id, n.clone());
        n
    }

    pub(super) fn open(&mut self, gui: &mut Gui) {
        if self.win.is_some() {
            return;
        }
        let xml = "<root><View view_layout=\"vertical\" name=\"perk_root\">\
                   <View view_layout=\"horizontal\" name=\"tabs\" layout_borders=\"Rect(5,5,5,0)\"/>\
                   <View view_layout=\"vertical\" name=\"points\" layout_borders=\"Rect(5,5,5,0)\"/>\
                   <BorderView name=\"lines_border\" layout_borders=\"Rect(5,5,5,5)\"><ScrollView name=\"lines_scroll\" v_scrollbar_mode=\"auto\" h_scrollbar_mode=\"auto\" min_size=\"Point(1,1)\" max_size=\"Point(16000,16000)\">\
                   <ScrollViewChild><View view_layout=\"vertical\" name=\"lines\"/></ScrollViewChild></ScrollView></BorderView>\
                   <View view_layout=\"horizontal\" name=\"bottom\" layout_borders=\"Rect(5,0,5,5)\"/></View></root>";
        let title = self.text("PerkWindow");
        let pos = ((self.screen.0 as i32 - CLIENT.0 as i32) / 2, (self.screen.1 as i32 - CLIENT.1 as i32) / 2);
        match gui.open_tabbed_window_xml("PerkWindow", &title, xml, pos, WindowSize::Fixed(CLIENT.0, CLIENT.1)) {
            Ok(window) => {
                let keys = ["ProfessionPerks", "GroupPerks", "GeneralPerks"];
                let mut tabs = String::from("<root><View view_layout=\"horizontal\">");
                for (i, k) in keys.iter().enumerate() {
                    tabs += &format!("<TextButton name=\"tab{i}\" text=\"{}\" color=\"0xFFFFFF\" hover_color=\"TEXT_HOVER\" pressed_color=\"TEXT_SELECTED\" layout_borders=\"Rect(0,0,12,0)\"/>", self.text(k));
                }
                tabs += "</View></root>";
                // "Remove Perk" toggle (`FUN_100611dc`, tooltip `UntrainPerkInfo`) and the untrain timeout (`UntrainPerkTimeout`, `FUN_10060553`)
                let bottom = format!(
                    "<root><View view_layout=\"horizontal\"><TextButton name=\"untrain\" text=\"{}\" color=\"0xFFFFFF\" hover_color=\"TEXT_HOVER\" pressed_color=\"TEXT_SELECTED\"/><HLayoutSpacer/><TextView name=\"timeout\" value=\"--:--:--\"/></View></root>",
                    self.text("RemovePerk")
                );
                match (gui.add_view_xml(window, "tabs", "PerkTabs", &tabs), gui.add_view_xml(window, "bottom", "PerkBottom", &bottom)) {
                    (Ok(tabs), Ok(bottom)) => self.win = Some(Win { window, tabs, bottom, drawn: None }),
                    (a, b) => {
                        eprintln!("hud: perk_window: {:?} {:?}", a.err(), b.err());
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
        let points = self.points(zone);
        let expansion = zone.stat(EXPANSION).unwrap_or(0);
        let alien = expansion & 8 != 0 && zone.stat(ALIEN_LEVEL).unwrap_or(0) >= 1;
        let sig = (self.tab, self.known.clone(), points, alien);
        if w.drawn.as_ref() != Some(&sig) {
            self.rebuild(gui, &mut w, zone, points, alien);
            w.drawn = Some(sig);
        }
        for i in 0..3 {
            gui.set_color_in(w.tabs, &format!("tab{i}"), if i == self.tab { 0xFFFFFF } else { 0x9999aa });
        }
        gui.set_toggle_in(w.bottom, "untrain", true, self.untrain);
        // `FUN_10060553`: the time left until the next untrain is possible as `%02u:%02u:%02u`, `--:--:--` when it is free; the stat holds the last reset's time
        let timeout = self.timeout(zone);
        gui.set_text(w.window, "timeout", &timeout);
        self.win = Some(w);
    }

    /// Seconds until the next perk may be removed: `LastPerkResetTime` (stat 0x241, seconds of the day clock) + `HOURS` hours - now
    /// (UNRESOLVED GUESS: the epoch of the stat is not decoded; it is taken as the server's unix time).
    fn timeout(&self, zone: &Zone) -> String {
        let last = zone.stat(LAST_RESET).unwrap_or(0);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
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
            let src = format!("<root><TextView value=\"{}{}\" color=\"0xCCFFFF\"/></root>", self.text(k), v);
            let _ = gui.add_view_xml(w.window, "points", "PerkPoints", &src);
        }
        let lines = self.db.lines(zone.stat(EXPANSION).unwrap_or(0), zone.stat(BREED).unwrap_or(0), zone.stat(PROFESSION).unwrap_or(0));
        gui.remove_children(w.window, "lines");
        for (k, line) in lines[self.tab.min(2)].clone().iter().enumerate() {
            let mut src = format!("<root><View view_layout=\"horizontal\" name=\"line{k}\" layout_borders=\"Rect(3,2,3,2)\">");
            for &id in line {
                let known = self.known.0.contains_key(&(id as i32));
                let precursor_known = self.db.get(id).is_some_and(|d| d.precursor == 0 || self.known.0.contains_key(&(d.precursor as i32)));
                let color = if known { KNOWN } else if precursor_known { OPEN } else { LOCKED };
                let name = self.name(id).replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;");
                src += &format!("<TextButton name=\"perk_{id}\" text=\"{name}\" color=\"{color:#08X}\" hover_color=\"TEXT_HOVER\" pressed_color=\"TEXT_SELECTED\" layout_borders=\"Rect(0,0,10,0)\"/>");
            }
            src += "<HLayoutSpacer/></View></root>"; // left-aligned
            let _ = gui.add_view_xml(w.window, "lines", "PerkLine", &src);
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
        // `FUN_10052904` (`MeetsPerkCriteria`, the item template's requirement check) is UNRESOLVED: the server refuses a perk the character may not train
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
            Event::Clicked { window: x, view, .. } if *x == window => {
                if let Some(i) = view.strip_prefix("tab").and_then(|n| n.parse::<usize>().ok()).filter(|i| *i < 3) {
                    self.tab = i;
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
        PerkDef { id, item: 0, precursor, flags, mask }
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
    fn perk_map_from_a_frame() {
        let mut h = HudPerks::new(&ao_gui::client_dir(), (1024, 768));
        let p = ao_net::n3::team::perk_update(ao_net::msg::Identity { kind: 50000, instance: 7 }, &ao_net::n3::team::PerkUpdate { perk: 5, stat_109: 0, remaining: -3 });
        h.on_frame(&ao_net::n3::outgoing::n3_frame(0, 7, p), 7);
        assert_eq!(h.known.0.get(&5), Some(&0));
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
