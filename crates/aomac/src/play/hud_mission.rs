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
//! UNRESOLVED / GUESS: the window's size and place (`MissionViewConfig` holds nothing, the ctor's size `FUN_100cf72a` is not decoded: 300 x 180 at the screen centre), the
//! format of the Remaining cell (the item stores an integer; `h:m:s` like the NCU window's cell), the order of the rows (sort column 0xf, ascending), the text the original
//! emits when a kept quest (`Quest_t+0xa4` bit 10) is removed (`GetText(0x66)`), which of the action position fields is the marker position (we take the integer `x`, `z`
//! of `WorldPos_c`), the server messages that add / remove single quests (only `QuestFullUpdateIIR_t` replaces the list).

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
/// UNRESOLVED GUESS: client size of the window.
const CLIENT: (u32, u32) = (300, 180);

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
    marker: Option<(u32, [f32; 2])>,
    /// The quest the open popup belongs to.
    popup: Option<i32>,
    built: Option<(u32, i64, Mode, usize)>,
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
            marker: None,
            popup: None,
            built: None,
        })
    }

    pub(super) fn handles(kind: WindowKind) -> bool {
        kind == WindowKind::Mission
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

    /// The marker `Upload2Map` emitted (`GlobalSignals+0x158`): playfield and world X / Z.
    pub(super) fn take_marker(&mut self) -> Option<(u32, [f32; 2])> {
        self.marker.take()
    }

    fn text(&self, key: &str) -> String {
        self.texts.by_key(CAT_GUI, key).unwrap_or_else(|| key.to_string())
    }

    /// The own quest list and the server clock.
    pub(super) fn on_frame(&mut self, f: &Frame, own: i32) {
        let Ok(m) = n3::decode(f) else { return };
        match &m.body {
            N3::World(World::Quests(q)) if m.header.target.instance == own => {
                if q.quests.len() == q.quest_count {
                    self.quests = q.quests.clone();
                    self.serial += 1;
                } else {
                    eprintln!("mission: QuestFullUpdate of {} quests could not be decoded", q.quest_count);
                }
            }
            N3::World(World::GameTime(t)) => self.sync = Some((i64::from(t.arg4), self.clock)),
            _ => {}
        }
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
        let all = self.columns();
        let view = ListView::new(Mode::List, all.clone(), 1, 0, (COL_REMAINING, false));
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
        let quests = std::mem::take(&mut self.quests);
        let mut rows = vec![];
        for q in &quests {
            // `N3Msg_QuestRemainingTime`: -1 (no deadline / past) shows as 0
            let left = now.and_then(|n| q.remaining_secs(n)).unwrap_or(0);
            let cells = columns.iter().map(|c| if c.id == COL_REMAINING { lv::hms(left as i32 * 100) } else { q.name.clone() }).collect();
            let icon = self.icon(gui, q.icon);
            rows.push(Row { key: q.id.instance, icon, cells, tip_title: q.name.clone(), tip_body: String::new() });
        }
        self.quests = quests;
        let win = self.win.as_mut().expect("open");
        win.view.set_rows(rows);
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

    /// `N3Msg_RemoveQuest` [GC 0x10019e5a]: a quest with bit 10 set is kept (the original prints a feedback text, UNRESOLVED), otherwise the message goes out.
    fn remove(&mut self, zone: &Zone, key: i32) {
        if let Some(q) = self.quest(key).filter(|q| q.removable()) {
            self.outbox.push(outgoing::n3_frame(0, zone.char_id, quest::remove_quest(zone.char_id as i32, q.id)));
        }
    }

    fn picked(&mut self, gui: &mut Gui, zone: &Zone, id: u32) {
        let Some(key) = self.popup.take() else { return };
        match id {
            POPUP_LOOK => self.look(key),
            POPUP_MAP => {
                // `FUN_100d2409`: the marker of the quest's first action position
                if let Some(p) = self.quest(key).and_then(Quest::world_pos) {
                    self.marker = Some((p.playfield.instance as u32, [p.x as f32, p.z as f32]));
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
        let now = self.clock;
        match win.view.event(gui, w, ev, now, lv::ICON_32) {
            Some(Hit::Double(key)) => {
                self.look(key);
                true
            }
            Some(Hit::Click(_)) => {
                self.built = None;
                true
            }
            Some(Hit::Context(key, at)) => {
                self.popup(gui, key, at);
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
impl HudMission {
    pub(super) fn shown(&self) -> Vec<(i32, Vec<String>)> {
        self.win.as_ref().map(|w| w.view.rows().iter().map(|r| (r.key, r.cells.clone())).collect()).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::msg::Identity;

    fn quest(instance: i32, name: &str, flags: i32, deadline: i32) -> Quest {
        let action = quest::QuestAction { id: 1, identities: [Identity::default(); 8], floats: [0.0; 8], ints: [deadline, 0], pos: quest::WorldPos { playfield: Identity { kind: 0xC79C, instance: 4605 }, x: 1500, z: 2500, local: [0.0; 3] } };
        Quest { id: Identity { kind: quest::QUEST_KIND, instance }, name: name.into(), value_a4: flags, actions: vec![action], ..Default::default() }
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
        m.quests = vec![quest(1, "Rat Hunt", 0, 10_000 + 90), quest(2, "Team Job", 1 << 8, 0)];
        m.sync = Some((10_000, 0.0));
        m.serial += 1;
        m.update(&mut gui, 0.0);
        assert_eq!(m.shown(), vec![(2, vec!["Team Job".into(), "00:00:00".into()]), (1, vec!["Rat Hunt".into(), "00:01:30".into()])]);
        // the remaining time counts down with the server clock
        m.update(&mut gui, 30.0);
        assert_eq!(m.shown()[1].1[1], "00:01:00");
        // Upload2Map: the first action's playfield and position
        m.popup = Some(1);
        m.picked(&mut gui, &zone, POPUP_MAP);
        assert_eq!(m.take_marker(), Some((4605, [1500.0, 2500.0])));
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
    }
}
