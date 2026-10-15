//! The Mission window `mission_window` (`MissionView_c`, GUI.dll ctor `FUN_100d2c7e`; module slot `InventoryGUIModule_c::SlotMissionViewActivated` 0x100c6da1; Ctrl+4).
//! docs/gui.md §11.12.
//!
//! `DockableView_c(Rect(), "Missions", "mission_window", flags 5)` around a `MissionListView_c` (`FUN_100d3431`, a `MultiListView_c` in list layout, `SetLayoutMode(1)`):
//! columns Icon 16 (id 0, flags 8), Name 200 (id 1, text key `Name`), Remaining 55 (id 0xf, key `Tab_Remaning`), sorted by column 0xf. The rows are the quests of
//! `N3Msg_GetQuestIds` (the own `QuestFullUpdateIIR_t` list, `ao_net::n3::quest`); the Remaining cell is `N3Msg_QuestRemainingTime` (`FUN_100d2f92`, refreshed by a
//! 1 s `EventTimer_c`): seconds left to the deadline of the quest's actions against `GameTime_t+0xb4` (the server's unix time), 0 when none.
//! Double click = `InfoViewModule_c::ShowURL("itemid://kind/instance")` (`FUN_100d2334`); the right-click popup (`FUN_100d275d`) holds `Look@Mission` (the same page),
//! `Upload2Map` (only with `N3Msg_GetQuestWorldPos`: `FUN_100d2409` emits `GlobalSignals+0x158`, the map / compass marker), `UpdateTeamMembers` (team missions only:
//! `N3Msg_UpdateNearbyTeamMembers`) and `DeleteMission` (`FUN_100d2517`: the `ReallyWant2DeleteMission` box `Warning`, Yes / No; Yes = `N3Msg_RemoveQuest`).
//!
//! Initial size: `FUN_100d2c7e` calls `FUN_100cf72a`, returning `(167,112)`; the unsaved dock uses its normal centred placement.
//! Remaining uses hours:minutes (`FUN_1003cdf6`, case 0xf), from integer seconds, not NCU centiseconds.
//! Default list order is the retained grid position (`FUN_1003cc19`, column0).
//! Kept-quest feedback uses `GetText(0x66, "Quest_CannotDelete")`; single removals synchronize from Zone.

use super::hud::WindowKind;
use super::hud_bar::icon_image;
use super::hud_dialog::Dialogs;
use super::hud_listview::{self as lv, Column, Hit, ListView, ListWindow, MenuTexts, Mode, Row, Spec};
use super::zone::Zone;
use ao_formats::screens::{TextDb, CAT_GUI};
use ao_gui::{Event, GfxId, Gui, MenuItem};
use ao_net::frame::Frame;
use ao_net::n3::{self, outgoing, quest::{self, Quest}, world::World, N3};
use ao_rdb::RecordStore;
use std::collections::HashMap;
use std::path::Path;

const MENU_BASE: u32 = 0x6000;
/// Menu ids of the row popup (outside the window options menu's range `MENU_BASE .. +0x100`).
const POPUP_BASE: u32 = 0x6100;
const POPUP_LOOK: u32 = POPUP_BASE;
const POPUP_MAP: u32 = POPUP_BASE + 1;
const POPUP_TEAM: u32 = POPUP_BASE + 2;
const POPUP_DELETE: u32 = POPUP_BASE + 3;
/// The remaining-time column (`AddColumn(0xf, ...)`).
const COL_REMAINING: u32 = 0xf;
/// GUI `FUN_100cf72a`: `_DAT_101bd710=167`, `_DAT_101bdfa4=112`.
const CLIENT: (u32, u32) = (167, 112);

/// GlobalSignals+158 carries the quest identity as well as position/playfield.
pub(super) type Marker = (ao_net::msg::Identity, Option<(u32, [f32; 2])>);

pub(super) struct HudMission {
    store: Option<RecordStore>,
    texts: TextDb,
    screen: (u32, u32),
    win: Option<ListWindow>,
    quests: Vec<Quest>,
    /// Counts list changes, to rebuild the rows.
    serial: u32,
    /// `(server unix time of the last GameTimeIIR_t, [`Self::clock`] then)`: `GameTime_t+0xb4`.
    sync: Option<(i64, f32)>,
    clock: f32,
    icons: HashMap<i32, Option<(GfxId, u32, u32)>>,
    dialogs: Dialogs<i32>,
    closed: Vec<WindowKind>,
    outbox: Vec<Frame>,
    urls: Vec<String>,
    lines: Vec<String>,
    press: Option<i32>,
    last_click: Option<(i32, f32)>,
    marker: Option<Marker>,
    marker_source: Option<ao_net::msg::Identity>,
    waypoint_lines: Vec<(String, u32)>,
    /// The quest the open popup belongs to.
    popup: Option<i32>,
    built: Option<(u32, i64, Mode, usize)>,
    grid_slots: Vec<Option<i32>>,
}

/// GUI100d2ab7 adds missing quests (100d2275/GetFirstFreePos), then deletes stale rows.
fn refresh_grid_slots(slots: &mut Vec<Option<i32>>, quests: &[Quest]) {
    for quest in quests {
        let key = Some(quest.id.instance);
        if !slots.contains(&key) {
            if let Some(slot) = slots.iter_mut().find(|slot| slot.is_none()) { *slot = key; }
            else { slots.push(key); }
        }
    }
    for slot in slots {
        if slot.is_some_and(|key| !quests.iter().any(|quest| quest.id.instance == key)) { *slot = None; }
    }
}

/// The frames [`HudMission::on_frame`] reads: `QuestFullUpdateIIR_t` and `GameTimeIIR_t`, by message key.
pub(super) fn wants(f: &Frame) -> bool {
    f.ptype == n3::outgoing::PT_N3
        && f.payload.get(..4).is_some_and(|k| {
            let k = u32::from_be_bytes([k[0], k[1], k[2], k[3]]);
            k == n3::world::QUEST_FULL_UPDATE || k == n3::world::GAME_TIME
        })
}

impl HudMission {
    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> anyhow::Result<Self> {
        Ok(Self {
            store: RecordStore::open(dir).ok(),
            texts: TextDb::load(dir)?,
            screen,
            win: None,
            quests: vec![],
            serial: 0,
            sync: None,
            clock: 0.0,
            icons: HashMap::new(),
            dialogs: Dialogs::default(),
            closed: vec![],
            outbox: vec![],
            urls: vec![],
            lines: vec![],
            press: None,
            last_click: None,
            marker: None,
            marker_source: None,
            waypoint_lines: vec![],
            popup: None,
            built: None,
            grid_slots: vec![],
        })
    }

    pub(super) fn handles(kind: WindowKind) -> bool {
        kind == WindowKind::Mission
    }

    /// Whether `(x, y)` sits over a quest icon in the Mission list (`MP_Quest_Icon`, shift-to-inspect cursor).
    pub(super) fn icon_at(&self, gui: &Gui, x: f32, y: f32) -> bool {
        self.win.as_ref().is_some_and(|w| w.view.row_at(gui, w.win, x, y, lv::ICON_32).is_some())
    }

    pub(super) fn set_screen(&mut self, screen: (u32, u32)) {
        self.screen = screen;
        if let Some(w) = &mut self.win {
            w.screen = screen;
        }
    }

    pub(super) fn take_closed(&mut self) -> Vec<WindowKind> {
        std::mem::take(&mut self.closed)
    }

    pub(super) fn take_outbox(&mut self) -> Vec<Frame> {
        std::mem::take(&mut self.outbox)
    }

    /// The `itemid://` pages the user asked for (`InfoViewModule_c::ShowURL`).
    pub(super) fn take_urls(&mut self) -> Vec<String> {
        std::mem::take(&mut self.urls)
    }
    pub(super) fn take_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.lines)
    }


    /// Native MultiListView release semantics apply to every cell, not just the name text.
    pub(super) fn input(&mut self, gui: &Gui, ev: &ao_gui::InputEvent, mods: ao_gui::Modifiers) {
        use ao_gui::{InputEvent, MouseButton};
        let row = |x, y| self.win.as_ref().filter(|w| gui.pointer_owner(x, y) == Some(w.win))
            .and_then(|w| w.view.row_at(gui, w.win, x, y, lv::ICON_32));
        match *ev {
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => self.press = row(x, y),
            InputEvent::MouseUp { x, y, button: MouseButton::Left } => {
                let released = row(x, y);
                let Some(key) = self.press.take().filter(|key| Some(*key) == released) else { return };
                let double = self.last_click.is_some_and(|(old, at)| old == key && self.clock - at <= ao_gui::DOUBLE_CLICK_TIME);
                self.last_click = (!mods.shift && !double).then_some((key, self.clock));
                if let Some(w) = &mut self.win {
                    w.view.selected = Some(key);
                }
                self.built = None;
                if mods.shift || double {
                    self.look(key);
                }
            }
            _ => {}
        }
    }

    /// Pending GlobalSignals+158/+15c: quest identity, and a location or removal.
    pub(super) fn take_marker(&mut self) -> Option<Marker> {
        self.marker.take()
    }

    pub(super) fn waypoint_changed(&mut self, marker: Option<(u32, [f32; 2])>, source: Option<ao_net::msg::Identity>, current_pf: Option<u32>, announce: bool) {
        self.marker_source = source;
        if !announce { return; }
        let keys: &[&str] = match marker {
            Some((0, _)) | None => &["NoWptInfo"],
            Some((pf, _)) if Some(pf) == current_pf => &["MissionWptUploaded", "MissionShownAsDot"],
            Some(_) => &["MissionWptUploaded", "MissionNotInArea"],
        };
        let color = if marker.is_some_and(|m| m.0 != 0) { 16 } else { 12 };
        for key in keys {
            if let Some(text) = self.texts.by_key(CAT_GUI, key) {
                self.waypoint_lines.push((text, color));
            }
        }
    }

    pub(super) fn take_waypoint_lines(&mut self) -> Vec<(String, u32)> {
        std::mem::take(&mut self.waypoint_lines)
    }

    fn text(&self, key: &str) -> String {
        self.texts.by_key(CAT_GUI, key).unwrap_or_else(|| key.to_string())
    }

    /// The own quest list and the server clock.
    pub(super) fn on_frame(&mut self, f: &Frame, own: i32) -> std::iter::RepeatN<(&'static str, &'static str)> {
        let Ok(m) = n3::decode(f) else { return std::iter::repeat_n(("OnReceiveMission", ""), 0) };
        self.on_message(&m, own)
    }

    fn on_message(&mut self, m: &n3::Message, own: i32) -> std::iter::RepeatN<(&'static str, &'static str)> {
        match &m.body {
            N3::World(World::Quests(q)) if m.header.target.kind == outgoing::DYNEL_CHAR && m.header.target.instance == own => self.on_quests(q),
            N3::World(World::GameTime(t)) => {
                self.sync = Some((i64::from(t.arg4), self.clock));
                std::iter::repeat_n(("OnReceiveMission", ""), 0)
            }
            _ => std::iter::repeat_n(("OnReceiveMission", ""), 0),
        }
    }

    /// GC100aca88 ->10056796 emits GlobalSignals+0x154 for each quest only
    /// with the message's trailing flag and the main-character byte+0x140
    /// (SetMainDynel10019f77). GUI100c69e2 forwards an empty tip payload.
    fn on_quests(&mut self, q: &n3::world::QuestFullUpdate) -> std::iter::RepeatN<(&'static str, &'static str)> {
        let count = if q.quests.len() == q.quest_count {
            if let Some(source) = self.marker_source.filter(|source| self.quests.iter().any(|old| old.id == *source) && !q.quests.iter().any(|new| new.id == *source)) {
                self.marker = Some((source, None));
            }
            self.quests = q.quests.clone();
            self.serial += 1;
            if q.flag != 0 { q.quests.len() } else { 0 }
        } else {
            eprintln!("mission: QuestFullUpdate of {} quests could not be decoded", q.quest_count);
            0
        };
        std::iter::repeat_n(("OnReceiveMission", ""), count)
    }

    /// `GameTime_t+0xb4`: the server's unix time now.
    fn server_now(&self) -> Option<u32> {
        self.sync.map(|(unix, at)| (unix + (self.clock - at) as i64).max(0) as u32)
    }

    fn columns(&self) -> Vec<Column> {
        vec![Column { id: 1, label: self.text("Name"), width: 200.0 }, Column { id: COL_REMAINING, label: self.text("Tab_Remaning"), width: 55.0 }]
    }

    pub(super) fn open(&mut self, gui: &mut Gui) {
        if self.win.is_some() {
            return;
        }
        self.grid_slots.clear();
        let all = self.columns();
        let view = ListView::new(Mode::List, all.clone(), 1, 0, (0, false));
        let texts = MenuTexts { list_mode: self.text("ListMode"), auto_arrange: self.text("AutoArrange") };
        let spec = Spec { name: "MissionView", title: "Missions", pos: (0, 0), client: CLIENT, top_xml: "", top_h: 0.0, texts, menu_base: MENU_BASE, screen: self.screen };
        match ListWindow::open(gui, spec, view, all, None) {
            Ok(mut w) => {
                let (ow, oh) = gui.outer_size(w.win);
                w.place(gui, ((self.screen.0 as i32 - ow as i32) / 2, (self.screen.1 as i32 - oh as i32) / 2));
                self.win = Some(w);
                self.built = None;
            }
            Err(e) => eprintln!("hud: mission window: {e:#}"),
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui) {
        if let Some(w) = self.win.take() {
            w.close(gui);
        }
        self.dialogs.close_all(gui);
        self.built = None;
        self.press = None;
        self.last_click = None;
    }

    /// Quest removal actions update the canonical Zone list without a full-list packet.
    pub(super) fn sync_removed(&mut self, zone: &Zone) {
        let previous = self.quests.len();
        if let Some(source) = self.marker_source.filter(|source| self.quests.iter().any(|old| old.id == *source) && !zone.quests.contains_key(source)) {
            self.marker = Some((source, None));
        }
        self.quests.retain(|q| zone.quests.contains_key(&q.id));
        if self.quests.len() != previous {
            self.serial += 1;
        }
    }

    fn icon(&mut self, gui: &mut Gui, id: i32) -> Option<(GfxId, u32, u32)> {
        let store = self.store.as_ref();
        *self.icons.entry(id).or_insert_with(|| u32::try_from(id).ok().filter(|&i| i > 0).and_then(|i| icon_image(gui, store?, i)))
    }

    pub(super) fn update(&mut self, gui: &mut Gui, dt: f32) {
        self.clock += dt;
        let Some(win) = self.win.as_ref() else { return };
        let w = win.win;
        let now = self.server_now();
        let sig = (self.serial, now.map_or(-1, i64::from), win.view.mode, win.view.columns.len() * 31 + win.view.sort.0 as usize + usize::from(win.view.sort.1));
        if self.built == Some(sig) {
            return;
        }
        let columns = win.view.columns.clone();
        refresh_grid_slots(&mut self.grid_slots, &self.quests);
        let quests = std::mem::take(&mut self.quests);
        let mut rows = vec![];
        for q in &quests {
            // `N3Msg_QuestRemainingTime`: -1 (no deadline / past) shows as 0
            let left = now.and_then(|n| q.remaining_secs(n)).unwrap_or(0);
            let cells = columns.iter().map(|c| if c.id == COL_REMAINING { remaining(left) } else { q.name.clone() }).collect();
            let icon = self.icon(gui, q.icon);
            rows.push((left, Row { key: q.id.instance, icon, cells, tip_title: q.name.clone(), tip_body: String::new() }));
        }
        self.quests = quests;
        let win = self.win.as_mut().expect("open");
        let (column, descending) = win.view.sort;
        rows.sort_by_key(|(_, row)| self.grid_slots.iter().position(|slot| *slot == Some(row.key)));
        if win.view.mode == Mode::List && column == 0 && descending { rows.reverse(); }
        if win.view.mode == Mode::List && column != 0 {
            rows.sort_by(|(left, a), (right, b)| {
                let order = if column == COL_REMAINING { left.cmp(right) } else { a.tip_title.to_lowercase().cmp(&b.tip_title.to_lowercase()) };
                if descending { order.reverse() } else { order }
            });
        }
        win.view.set_rows_presorted(rows.into_iter().map(|(_, row)| row).collect());
        win.view.paint(gui, w, lv::ICON_32);
        self.built = Some(sig);
    }

    fn quest(&self, key: i32) -> Option<&Quest> {
        self.quests.iter().find(|q| q.id.instance == key)
    }

    /// `FUN_100d275d`: the row popup.
    fn popup(&mut self, gui: &mut Gui, key: i32, at: (i32, i32)) {
        let Some(q) = self.quest(key) else { return };
        let mut items = vec![MenuItem::entry(POPUP_LOOK, &self.text("Look@Mission"))];
        if q.world_pos().is_some() {
            items.push(MenuItem::entry(POPUP_MAP, &self.text("Upload2Map")));
        }
        if q.is_team() {
            items.push(MenuItem::entry(POPUP_TEAM, &self.text("UpdateTeamMembers")));
        }
        items.push(MenuItem::entry(POPUP_DELETE, &self.text("DeleteMission")));
        self.popup = Some(key);
        gui.open_menu(at, (self.screen.0 as i32, self.screen.1 as i32), items);
    }

    fn look(&mut self, key: i32) {
        if let Some(q) = self.quest(key) {
            self.urls.push(format!("itemid://{}/{}", q.id.kind, q.id.instance));
        }
    }

    /// GC `0x10019e5a`: kept quests emit category0x66 feedback instead of a request.
    fn remove(&mut self, zone: &Zone, key: i32) {
        if let Some(q) = self.quest(key) {
            if q.removable() {
                self.outbox.push(outgoing::n3_frame(0, zone.char_id, quest::remove_quest(zone.char_id as i32, q.id)));
            } else if let Some(text) = self.texts.by_key(0x66, "Quest_CannotDelete") {
                self.lines.push(text);
            }
        }
    }

    fn picked(&mut self, gui: &mut Gui, zone: &Zone, id: u32) {
        let Some(key) = self.popup.take() else { return };
        match id {
            POPUP_LOOK => self.look(key),
            POPUP_MAP => {
                // `FUN_100d2409`: the marker of the quest's first action position
                if let Some(q) = self.quest(key) {
                    if let Some(p) = q.world_pos() {
                        self.marker = Some((q.id, Some((p.playfield.instance as u32, [p.local[0], p.local[2]]))));
                    }
                }
            }
            POPUP_TEAM => {
                if let Some(q) = self.quest(key) {
                    self.outbox.push(outgoing::n3_frame(0, zone.char_id, quest::give_quest_to_members(zone.char_id as i32, q.id)));
                }
            }
            POPUP_DELETE => {
                let buttons = [self.text("MsgBox_Yes"), self.text("MsgBox_No")];
                let body = self.text("ReallyWant2DeleteMission");
                self.dialogs.go(gui, self.screen, key, &body, &buttons);
            }
            _ => {}
        }
    }

    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        match self.dialogs.event(gui, ev) {
            (_, Some((key, 0))) => {
                self.remove(zone, key);
                return true;
            }
            (true, _) => return true,
            _ => {}
        }
        let Some(win) = self.win.as_mut() else { return false };
        let w = win.win;
        match ev {
            Event::CloseRequested { window } if *window == w => {
                self.close(gui);
                self.closed.push(WindowKind::Mission);
                return true;
            }
            Event::FrameIcon { window, x, y } if *window == w => {
                win.open_menu(gui, (*x, *y));
                return true;
            }
            Event::WindowFrame { window } if *window == w => {
                if win.reflow_grid(gui, lv::ICON_32) {
                    self.built = None;
                }
                return true;
            }
            Event::MenuPicked { id } if win.owns_menu(*id) => {
                win.picked(*id);
                self.built = None;
                return true;
            }
            Event::MenuPicked { id } if (POPUP_BASE..POPUP_BASE + 4).contains(id) => {
                self.picked(gui, zone, *id);
                return true;
            }
            _ => {}
        }
        // Raw input above handles the complete row on release. Name TextButtons
        // also emit Clicked: do not inspect twice or seed another double click.
        if matches!(ev, Event::Clicked { .. } | Event::CanvasPress { .. }) {
            return false;
        }
        match win.view.event(gui, w, ev, self.clock, lv::ICON_32) {
            Some(Hit::Context(key, at)) => {
                self.popup(gui, key, at);
                true
            }
            _ => false,
        }
    }
}

/// GUI 0x1003cf68: seconds / 3600 and (seconds / 60) % 60, `%02d:%02d`.
fn remaining(seconds: u32) -> String {
    format!("{:02}:{:02}", seconds / 3600, seconds / 60 % 60)
}

#[cfg(test)]
impl HudMission {
    pub(super) fn set_quests(&mut self, quests: &[Quest]) {
        self.quests = quests.to_vec();
        self.serial += 1;
    }

    pub(super) fn row_point(&self, gui: &mut Gui, id: ao_net::msg::Identity, icon: bool) -> Option<(f32, f32)> {
        self.quests.iter().find(|q| q.id == id)?;
        let w = self.win.as_ref()?;
        for name in gui.view_names(w.win).into_iter().filter(|n| n.starts_with("lv_row")) {
            let r = gui.view_rect(w.win, &name)?;
            let (x, y) = ((r.l + r.r) * 0.5, (r.t + r.b) * 0.5);
            if w.view.row_at(gui, w.win, x, y, lv::ICON_32) != Some(id.instance) {
                continue;
            }
            let name = if icon { name.replacen("lv_row", "lv_icon", 1) } else { name };
            let r = gui.view_rect(w.win, &name)?;
            let scroll = gui.view_rect(w.win, "lv_scroll")?;
            let offset = gui.scroll_offset(w.win, "lv_scroll");
            if r.t < scroll.t {
                gui.set_scroll_offset(w.win, "lv_scroll", offset + r.t - scroll.t);
            } else if r.b > scroll.b {
                gui.set_scroll_offset(w.win, "lv_scroll", offset + r.b - scroll.b);
            }
            let r = gui.view_rect(w.win, &name)?;
            return Some(((r.l + r.r) * 0.5, (r.t + r.b) * 0.5));
        }
        None
    }

    pub(super) fn shown(&self) -> Vec<(i32, Vec<String>)> {
        self.win.as_ref().map(|w| w.view.rows().iter().map(|r| (r.key, r.cells.clone())).collect()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::msg::Identity;

    fn quest(instance: i32, name: &str, flags: i32, deadline: i32) -> Quest {
        let action = quest::QuestAction { id: 1, identities: [Identity::default(); 8], floats: [0.0; 8], ints: [deadline, 0], pos: quest::WorldPos { playfield: Identity { kind: 0xC79C, instance: 4605 }, x: 1500, z: 2500, local: [123.5, 45.0, 678.25] } };
        Quest { id: Identity { kind: quest::QUEST_KIND, instance }, name: name.into(), value_a4: flags, actions: vec![action], ..Default::default() }
    }

    #[test]
    fn mission_refresh_retains_grid_slots_before_deleting_stale_rows() {
        let mut slots = vec![];
        refresh_grid_slots(&mut slots, &[quest(1, "First", 0, 0), quest(2, "Second", 0, 0)]);
        assert_eq!(slots, [Some(1), Some(2)]);
        refresh_grid_slots(&mut slots, &[quest(2, "Second updated", 0, 0), quest(3, "Third", 0, 0)]);
        assert_eq!(slots, [None, Some(2), Some(3)], "new items precede stale-item deletion");
        refresh_grid_slots(&mut slots, &[quest(3, "Third", 0, 0), quest(4, "Fourth", 0, 0), quest(2, "Second", 0, 0)]);
        assert_eq!(slots, [Some(4), Some(2), Some(3)], "existing positions survive server order changes; new items reuse prior holes");
    }

    #[test]
    fn waypoint_feedback_and_removal_retain_native_quest_identity() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/text").exists() { return; }
        let mut m = HudMission::new(&dir, (1024, 768)).unwrap();
        let q = quest(9, "Marker", 0, 0);
        let marker = Some((4605, [12.0, 34.0]));
        m.waypoint_changed(marker, Some(q.id), Some(4605), true);
        let text = |key| m.texts.by_key(CAT_GUI, key).expect("native waypoint text");
        let uploaded = text("MissionWptUploaded");
        let shown = text("MissionShownAsDot");
        let elsewhere = text("MissionNotInArea");
        let missing = text("NoWptInfo");
        assert_eq!(m.take_waypoint_lines(), [(uploaded.clone(), 16), (shown, 16)]);
        m.waypoint_changed(marker, Some(q.id), Some(1), true);
        assert_eq!(m.take_waypoint_lines(), [(uploaded, 16), (elsewhere, 16)]);
        m.quests = vec![q.clone()];
        m.sync_removed(&Zone::new(7));
        assert_eq!(m.take_marker(), Some((q.id, None)));
        m.waypoint_changed(None, Some(q.id), Some(4605), false);
        assert!(m.take_waypoint_lines().is_empty(), "+15c removal has no +158 feedback");
        m.waypoint_changed(None, None, Some(4605), true);
        assert_eq!(m.take_waypoint_lines(), [(missing, 12)]);
        m.quests = vec![q];
        m.sync_removed(&Zone::new(7));
        assert!(m.take_marker().is_none(), "a manual waypoint no longer belongs to the old quest");
    }

    #[test]
    fn remaining_cell_uses_native_hours_and_minutes_without_centisecond_overflow() {
        for (seconds, text) in [(0, "00:00"), (59, "00:00"), (60, "00:01"), (3599, "00:59"), (3600, "01:00"), (360_061, "100:01"), (u32::MAX, "1193046:28")] {
            assert_eq!(remaining(seconds), text);
        }
    }

    #[test]
    fn received_mission_native_flag_emits_one_empty_event_per_quest() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/text").exists() { return; }
        let mut m = HudMission::new(&dir, (1024, 768)).unwrap();
        let mut update = n3::world::QuestFullUpdate { quest_count: 2, quests: vec![quest(1, "First", 0, 0), quest(2, "Second", 0, 0)], flag: 0, rest: vec![] };
        assert_eq!(m.on_quests(&update).count(), 0, "initial list has no new-mission flag");
        update.flag = 1;
        assert_eq!(m.on_quests(&update).collect::<Vec<_>>(), vec![("OnReceiveMission", ""), ("OnReceiveMission", "")]);
        let mut message = n3::Message { header: n3::N3Header { msg_type: n3::world::QUEST_FULL_UPDATE, target: Identity { kind: outgoing::DYNEL_CHAR, instance: 7 }, flag: 0 }, sender: 0, body: N3::World(World::Quests(update.clone())) };
        assert_eq!(m.on_message(&message, 7).collect::<Vec<_>>(), vec![("OnReceiveMission", ""), ("OnReceiveMission", "")]);
        message.header.target.instance = 9;
        assert_eq!(m.on_message(&message, 7).count(), 0, "other characters have no main-dynel signal");
        message.header.target = Identity { kind: 0xC79C, instance: 7 };
        assert_eq!(m.on_message(&message, 7).count(), 0, "matching instance is not enough without character kind");
        update.quests.clear();
        assert_eq!(m.on_quests(&update).count(), 0, "undecoded records must not produce a tip");
        update.quest_count = 0;
        assert_eq!(m.on_quests(&update).count(), 0, "empty flagged list emits no quest signal");
    }

    /// The window lists the quests with their remaining time and the popup sends / uploads what the original does.
    #[test]
    fn mission_window_lists_quests_and_acts() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui").exists() {
            eprintln!("skipping: no client");
            return;
        }
        let labels = TextDb::load(&dir).unwrap();
        let mut gui = Gui::new(&dir, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).unwrap();
        let mut m = HudMission::new(&dir, (1100, 760)).unwrap();
        let zone = Zone::new(7);
        m.open(&mut gui);
        assert_eq!(CLIENT, (167, 112), "MissionView's native initial ResizeTo");
        assert!(gui.window_size(m.win.as_ref().unwrap().win).0 < 300, "do not retain the reconstructed 300px viewport");
        m.quests = vec![quest(1, "Rat Hunt", 0, 10_000 + 90), quest(2, "Team Job", 1 << 8, 0)];
        m.sync = Some((10_000, 0.0));
        m.serial += 1;
        m.update(&mut gui, 0.0);
        assert_eq!(m.shown(), vec![(1, vec!["Rat Hunt".into(), "00:01".into()]), (2, vec!["Team Job".into(), "00:00".into()])]);
        m.win.as_mut().unwrap().view.sort = (COL_REMAINING, false);
        m.update(&mut gui, 0.0);
        assert_eq!(m.shown()[0].0, 2, "explicit Remaining sort, not the initial grid-position order");
        let win = m.win.as_ref().unwrap().win;
        for (width, height) in [CLIENT, (640, 400), (900, 600)] {
            gui.resize_window(win, ao_gui::WindowSize::Fixed(width, height));
            let scroll = gui.view_rect(win, "lv_scroll").unwrap();
            let header = gui.view_rect(win, "lv_head").unwrap();
            let icon = gui.view_rect(win, "lv_icon0").unwrap();
            let name = gui.view_rect(win, "lv_row0").unwrap();
            let remaining = gui.view_rect(win, "lv_cell0_1").unwrap();
            assert_eq!((header.l, header.t), (scroll.l, scroll.t));
            assert_eq!(header.height(), gui.font_height(ao_gui::FontId::Normal) as f32 - 1.0 + 6.0);
            assert_eq!((icon.l, icon.t), (scroll.l, header.b + 1.0));
            assert_eq!((name.l, name.t), (icon.l + lv::ICON_COL + 6.0, icon.t));
            assert_eq!((remaining.l, remaining.t), (name.l + 200.0 + 6.0, icon.t));
            assert_eq!(gui.view_rect(win, "lv_row1").unwrap().t - name.t, 19.0);
            assert!(m.icon_at(&gui, icon.l + 2.0, icon.t + 2.0));
        }
        // the remaining time counts down with the server clock
        m.update(&mut gui, 30.0);
        assert_eq!(m.shown()[1].1[1], "00:01");
        // Upload2Map: the first action's playfield and position
        m.popup = Some(1);
        m.picked(&mut gui, &zone, POPUP_MAP);
        assert_eq!(m.take_marker(), Some((m.quests[0].id, Some((4605, [123.5, 678.25])))));
        // UpdateTeamMembers: sent for the quest
        m.popup = Some(2);
        m.picked(&mut gui, &zone, POPUP_TEAM);
        assert_eq!(m.take_outbox().len(), 1);
        // Look@Mission
        m.popup = Some(1);
        m.picked(&mut gui, &zone, POPUP_LOOK);
        assert_eq!(m.take_urls(), vec![format!("itemid://{}/1", quest::QUEST_KIND)]);
        // a removable quest goes out, one with bit 10 set is kept
        m.remove(&zone, 1);
        assert_eq!(m.take_outbox().len(), 1);
        m.quests.push(quest(3, "Keep", 1 << 10, 0));
        m.remove(&zone, 3);
        assert!(m.take_outbox().is_empty());
        assert_eq!(m.take_lines(), vec![m.texts.by_key(0x66, "Quest_CannotDelete").expect("native kept-quest feedback")]);
        assert!(m.take_lines().is_empty(), "feedback is consumed once");
        // A removal action does not resend QuestFullUpdate: discard the stale row
        // when the canonical Zone list no longer contains that quest.
        let mut zone = Zone::new(7);
        zone.quests.insert(m.quests[1].id, m.quests[1].clone());
        m.sync_removed(&zone);
        m.update(&mut gui, 0.0);
        assert_eq!(m.shown(), vec![(2, vec!["Team Job".into(), "00:00".into()])]);
        m.set_quests(&[quest(4, "Later", 0, 10_120), quest(5, "Earlier", 0, 10_119)]);
        m.update(&mut gui, 0.0);
        assert_eq!(m.shown().iter().map(|r| r.0).collect::<Vec<_>>(), [5, 4], "raw seconds sort even when both cells display00:01");
        m.win.as_mut().unwrap().view.sort.1 = true;
        m.update(&mut gui, 0.0);
        assert_eq!(m.shown().iter().map(|r| r.0).collect::<Vec<_>>(), [4, 5]);
    }

    /// `MP_Quest_Icon`: `icon_at` finds a quest row and misses empty space (regression for the shift-to-inspect cursor).
    #[test]
    fn icon_at_finds_quest_row() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/rdb.db").exists() || !dir.join("cd_image/gui").exists() {
            eprintln!("skipping: no client");
            return;
        }
        let labels = TextDb::load(&dir).unwrap();
        let mut gui = Gui::new(&dir, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).unwrap();
        let mut m = HudMission::new(&dir, (1100, 760)).unwrap();
        m.open(&mut gui);
        m.quests = vec![quest(1, "Rat Hunt", 0, 0)];
        m.sync = Some((0, 0.0));
        m.serial += 1;
        m.update(&mut gui, 0.0);
        let win = m.win.as_ref().unwrap();
        let r = gui.view_rect(win.win, "lv_line0").unwrap();
        assert!(m.icon_at(&gui, r.l + 2.0, r.t + 2.0));
        assert!(!m.icon_at(&gui, 0.0, 0.0));
    }
}
