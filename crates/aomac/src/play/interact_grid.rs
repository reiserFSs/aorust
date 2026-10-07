//! The grid / whompah / shuttle destination window (`TeleportTarget_c`, GUI.dll; layout `Views/TeleportTarget.xml`). Evidence: docs/zone/interact.md
//! ("Grid / whompah / shuttle").
//!
//! The server's `GridDestinationSelect` stores the list and the token in the engine (`N3Msg_SetGridDestinationList` [GC 0x100239da]) and then sets the
//! DValue `teleport_target_window` (`FUN_10128fd9`). The module behind that DValue (`FUN_1002e313`, the `GUIModuleBase_c` at `+0xc58`) opens the window
//! when the own character's `Features` (stat 0xe0, `N3Msg_GetSkill(0xe0, 2)`) has bit 0x800, resetting the DValue otherwise, and does nothing while a window
//! is already open (the list shown stays the old one, the engine's token is the new one).

use super::zone::Zone;
use ao_gui::widgets::{MultiCell, MultiKey};
use ao_gui::{Event, Gui, WindowId, WindowSize};
use ao_net::msg::Identity;
use ao_net::n3::grid::{self, Destination};
use ao_net::n3::outgoing::DYNEL_CHAR;
use std::collections::HashMap;

/// `Window(Rect(200, 180, 700, 600), "", "Vehicle", 0, 0x1000)` [GUI 0x100ffa7c] (`_DAT_101a959c/95a0/95a8`, `_DAT_101b3db8`): a 501 x 421 client (inclusive
/// corners, like the NPC chat window's `Rect(0, 0, 399, 299)`); the window is centred afterwards (`Window::MoveToCenter`).
pub const CLIENT: (u32, u32) = (501, 421);
/// `MultiListView_c(Rect(), 0x40, 0, 0)`: feature flag 0x40 = row selection.
const LIST_FLAGS: u32 = 0x40;
/// `AddColumn(id, label, width, 0xe)` flags: resizable | label | sortable. Default width `FindFloat("LocationColWidth" / "PlayfieldColWidth", 200.0)`.
const COL_FLAGS: u32 = 0xe;
const COL_WIDTH: f32 = 200.0;
/// Stat `Features` and the bit the module tests.
pub const STAT_FEATURES: u32 = 0xe0;
const FEATURE_OPEN: i32 = 0x800;
/// `N3Msg_GetPFName` returned null.
const UNKNOWN_PF: &str = "unknown pf";
/// `View name="TargetView"` of the XML, `Window::AppendTab` title.
const TAB_TITLE: &str = "Target List";

/// The destination window, the engine's stored list and token, and the DValue `teleport_target_window`.
#[derive(Default)]
pub struct GridUi {
    win: Option<WindowId>,
    /// The list the window was built from (row id = position, `Variant` of the item).
    entries: Vec<Destination>,
    /// `n3EngineClientAnarchy_t + 0xb4`: the token of the last `GridDestinationSelect` (copied into every `GridSelected`).
    token: Vec<u8>,
    /// DValue `teleport_target_window`.
    flag: bool,
    pf_names: Option<HashMap<u32, String>>,
}

/// Rank of every distinct string in byte order (`std::string::compare`), so the rows compare like the item's virtual `+4` does.
fn ranks<'a>(it: impl Iterator<Item = &'a str>) -> HashMap<&'a str, i64> {
    let mut v: Vec<&str> = it.collect();
    v.sort_unstable_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    v.dedup();
    v.into_iter().enumerate().map(|(i, s)| (s, i as i64)).collect()
}

impl GridUi {
    /// `GridDestinationSelectIIR_t::Activate` (`FUN_10128fd9`) followed by the module's show slot (`FUN_1002e313`). `target` = the header's dynel,
    /// which must be a known character (`GetDynel` + `RTDynamicCast<SimpleChar_t>`).
    pub fn activate(&mut self, gui: &mut Gui, screen: (u32, u32), zone: &Zone, target: Identity, destinations: Vec<Destination>, token: Vec<u8>) {
        let known = target.kind == DYNEL_CHAR && (target.instance as u32 == zone.char_id || zone.dynels.contains_key(&target.instance));
        if !known {
            return;
        }
        // `N3Msg_SetGridDestinationList`: list and token replace the engine's
        let list = destinations;
        self.token = token;
        self.flag = true;
        if zone.stat(STAT_FEATURES).unwrap_or(0) & FEATURE_OPEN == 0 {
            eprintln!("interact: grid window refused: own Features {:#x} lacks 0x800 (FUN_1002e313)", zone.stat(STAT_FEATURES).unwrap_or(0));
            self.flag = false;
            return;
        }
        if self.win.is_some() {
            return;
        }
        match self.open(gui, screen, list) {
            Ok(win) => self.win = Some(win),
            Err(e) => {
                eprintln!("interact: grid window: {e:#}");
                self.flag = false;
            }
        }
    }

    /// `FUN_100ffa7c`: the window from `TeleportTarget.xml`, the list view in `TargetView`, one row per entry, one tab "Target List", centred.
    fn open(&mut self, gui: &mut Gui, screen: (u32, u32), list: Vec<Destination>) -> anyhow::Result<WindowId> {
        let (w, h) = CLIENT;
        let pos = ((screen.0 as i32 - w as i32) / 2, (screen.1 as i32 - h as i32) / 2);
        let win = gui.open_tabbed_window("TeleportTarget", TAB_TITLE, pos, WindowSize::Fixed(w, h))?;
        gui.add_view_xml(win, "TargetView", "targets", &format!("<root><MultiListView name=\"targets\" feature_flags=\"{LIST_FLAGS}\" max_size=\"Point(16000,16000)\"/></root>"))?;
        gui.multi_add_column(win, "targets", 0, "Location", COL_WIDTH, COL_FLAGS);
        gui.multi_add_column(win, "targets", 1, "Playfield", COL_WIDTH, COL_FLAGS);
        self.entries = list;
        let names = self.pf_names.get_or_insert_with(|| ao_formats::screens::pfnr_names(&ao_gui::client_dir()).unwrap_or_default());
        let pf: Vec<String> = self.entries.iter().map(|d| names.get(&(d.playfield as u32)).cloned().unwrap_or_else(|| UNKNOWN_PF.into())).collect();
        // `TeleportTargetItem_c` virtual `+4` (0x10100340): column 0 compares the playfield name (`+0x64`), column 1 the location (`+0x48`) -- the opposite
        // of what the columns show; reproduced with rank keys. The first sortable column (0) is the sort column, rows are added with `sorted = true`.
        let (pf_rank, loc_rank) = (ranks(pf.iter().map(String::as_str)), ranks(self.entries.iter().map(|d| d.name.as_str())));
        for (i, d) in self.entries.iter().enumerate() {
            let cells = vec![
                MultiCell { text: d.name.clone(), key: MultiKey::Num(pf_rank[pf[i].as_str()]), image: None },
                MultiCell { text: pf[i].clone(), key: MultiKey::Num(loc_rank[d.name.as_str()]), image: None },
            ];
            gui.multi_add_row(win, "targets", i as i64, cells, true);
        }
        Ok(win)
    }

    /// GUI events of the window; `Some(frames)` (payloads to send, possibly none) when the event was for it.
    pub fn event(&mut self, gui: &mut Gui, ev: &Event, own: Identity) -> Option<Vec<Vec<u8>>> {
        let win = self.win?;
        match ev {
            // `FUN_100ff922`, the slot of the list's mouse signal: the row under the pointer is selected (`MultiListViewItem_c::Select(true, true)`)
            Event::MultiMouse { window, id: Some(i), .. } if *window == win => {
                gui.multi_select(win, "targets", *i, true, true);
                Some(vec![])
            }
            Event::Clicked { window, view, .. } if *window == win => match view.as_str() {
                "Go" => Some(self.go(gui, own)),
                // `Quit` is wired to a `Window` method [GUI 0x1012a778] = the close path; [INFERENCE]: it reaches the virtual `FUN_100ff939` like `Go` does.
                "Quit" => Some(self.close(gui, own)),
                _ => Some(vec![]),
            },
            // the frame's close button [INFERENCE: same virtual]
            Event::CloseRequested { window } if *window == win => Some(self.close(gui, own)),
            _ => None,
        }
    }

    /// `FUN_100ffa12` (Go): the selected row -> `N3Msg_GridDestinationSelected(row, playfield, identity)`, then the close virtual. Without a selection nothing happens.
    fn go(&mut self, gui: &mut Gui, own: Identity) -> Vec<Vec<u8>> {
        let Some(win) = self.win else { return vec![] };
        let Some(&row) = gui.multi_selected(win, "targets").first() else { return vec![] };
        let Some(d) = self.entries.get(row as usize) else { return vec![] };
        let mut out = vec![grid::selected(own, &self.token, row as i32, d.playfield as u32, d.target)];
        out.extend(self.close(gui, own));
        out
    }

    /// `FUN_100ff939` (virtual slot 7): while the DValue is set `N3Msg_GridDestinationSelected(-1, 0, Identity(0, 0))` and the DValue is cleared, which makes
    /// the module delete the window (`FUN_1002e313(false)`); a window that is closed without a flag just goes away.
    fn close(&mut self, gui: &mut Gui, own: Identity) -> Vec<Vec<u8>> {
        let mut out = vec![];
        if self.flag {
            out.push(grid::cancel(own, &self.token));
            self.flag = false;
        }
        self.destroy(gui);
        out
    }

    fn destroy(&mut self, gui: &mut Gui) {
        if let Some(win) = self.win.take() {
            gui.close_window(win);
        }
    }

    /// The zone changes or the connection ends: the window goes away without a word to the server ([INFERENCE]: the original's behaviour on a zone change was not found).
    pub fn close_all(&mut self, gui: &mut Gui) {
        self.destroy(gui);
        self.flag = false;
    }

    /// The rows as shown, top to bottom: `"<location> | <playfield>"` (live harness / tests).
    #[cfg(test)]
    pub fn dump(&self, gui: &Gui) -> String {
        let Some(win) = self.win else { return "grid: no window".into() };
        let names = self.pf_names.as_ref();
        let mut s = format!("grid window, {} entries (selected {:?})", self.entries.len(), gui.multi_selected(win, "targets"));
        for id in gui.multi_row_ids(win, "targets") {
            let Some(d) = self.entries.get(id as usize) else { continue };
            let pf = names.and_then(|n| n.get(&(d.playfield as u32))).map_or(UNKNOWN_PF, String::as_str);
            s.push_str(&format!("\n  [{id}] {} | {pf} (pf {}, {:?})", d.name, d.playfield, d.target));
        }
        s
    }

    /// Select row `index` (a list position) and press Go; the payloads to send, `None` when there is no such row or no window.
    #[cfg(test)]
    pub fn select(&mut self, gui: &mut Gui, own: Identity, index: usize) -> Option<Vec<Vec<u8>>> {
        let win = self.win?;
        if !gui.multi_row_ids(win, "targets").contains(&(index as i64)) {
            return None;
        }
        gui.multi_select(win, "targets", index as i64, true, true);
        Some(self.go(gui, own))
    }

    #[cfg(test)]
    pub fn is_open(&self) -> bool {
        self.win.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_gui::{InputEvent, MouseButton};
    use ao_net::n3::grid::Grid;

    const OWN: u32 = 0x82e8;
    const ME: Identity = Identity { kind: DYNEL_CHAR, instance: OWN as i32 };

    fn rig() -> Option<Gui> {
        let client = ao_gui::client_dir();
        client.join("cd_image/gui").exists().then(|| {
            let mut g = Gui::new(&client, None).unwrap();
            g.set_screen_size(1280, 800);
            g
        })
    }

    fn dest(pf: i32, name: &str) -> Destination {
        Destination { playfield: pf, target: Identity { kind: 0x9C50, instance: pf }, name: name.into(), v2c: 0, v28: 0 }
    }

    fn zone(features: i32) -> Zone {
        let mut z = Zone::new(OWN);
        z.stats.insert(STAT_FEATURES, features);
        z
    }

    fn click(gui: &mut Gui, win: WindowId, view: &str) -> Vec<Event> {
        let r = gui.view_rect(win, view).unwrap_or_else(|| panic!("no view {view}"));
        let (x, y) = ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0);
        gui.input(InputEvent::MouseMove { x, y });
        gui.input(InputEvent::MouseDown { x, y, button: MouseButton::Left });
        gui.input(InputEvent::MouseUp { x, y, button: MouseButton::Left })
    }

    fn feed(g: &mut GridUi, gui: &mut Gui, evs: Vec<Event>) -> Vec<Vec<u8>> {
        evs.iter().flat_map(|e| g.event(gui, e, ME).unwrap_or_default()).collect()
    }

    fn press(g: &mut GridUi, gui: &mut Gui, win: WindowId, view: &str) -> Vec<Vec<u8>> {
        let evs = click(gui, win, view);
        feed(g, gui, evs)
    }

    fn open(gui: &mut Gui, g: &mut GridUi, z: &Zone) {
        // sorted by playfield name (column 0 compares the playfield, `pfnrmap` names where known, "unknown pf" otherwise): ids 9990.. are unknown
        let list = vec![dest(9992, "Zeta"), dest(9991, "Alpha"), dest(9990, "Mid")];
        g.activate(gui, (1280, 800), z, ME, list, vec![5, 6, 7]);
    }

    #[test]
    fn window_lists_the_destinations_and_go_sends_the_selected_one() {
        let Some(mut gui) = rig() else { return };
        let mut g = GridUi::default();
        open(&mut gui, &mut g, &zone(0x8807));
        let win = g.win.expect("window");
        // every playfield is unknown -> equal keys on column 0: insertion order; column 1 shows the playfield name
        assert_eq!(gui.multi_row_ids(win, "targets"), [0, 1, 2]);
        assert!(g.dump(&gui).contains("Alpha | unknown pf"));
        // Go without a selection does nothing
        assert!(press(&mut g, &mut gui, win, "Go").is_empty());
        assert!(g.is_open());
        // a click on the list row selects it (`FUN_100ff922`)
        let r = gui.view_rect(win, "targets").unwrap();
        let (x, y) = (r.l + 40.0, r.t + 19.0 + 5.0); // row pitch 19 (`ROW_H` 15 + 1 + 3): the second row
        let mut evs = vec![];
        gui.input(InputEvent::MouseMove { x, y });
        evs.extend(gui.input(InputEvent::MouseDown { x, y, button: MouseButton::Left }));
        evs.extend(gui.input(InputEvent::MouseUp { x, y, button: MouseButton::Left }));
        feed(&mut g, &mut gui, evs);
        assert_eq!(gui.multi_selected(win, "targets").len(), 1, "a row is selected");
        let row = gui.multi_selected(win, "targets")[0];
        let out = press(&mut g, &mut gui, win, "Go");
        // `Go`: GridSelected(row, playfield, identity), then the close virtual sends the cancel while the DValue is still set
        let d = &g.entries[row as usize];
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], grid::selected(ME, &[5, 6, 7], row as i32, d.playfield as u32, d.target));
        assert_eq!(out[1], grid::cancel(ME, &[5, 6, 7]));
        assert!(!g.is_open());
    }

    #[test]
    fn quit_and_the_close_button_send_the_cancel_once() {
        let Some(mut gui) = rig() else { return };
        let mut g = GridUi::default();
        open(&mut gui, &mut g, &zone(0x800));
        let win = g.win.unwrap();
        let out = press(&mut g, &mut gui, win, "Quit");
        assert_eq!(out, [grid::cancel(ME, &[5, 6, 7])]);
        assert!(!g.is_open());
        open(&mut gui, &mut g, &zone(0x800));
        let win = g.win.unwrap();
        let out = feed(&mut g, &mut gui, vec![Event::CloseRequested { window: win }]);
        assert_eq!(out, [grid::cancel(ME, &[5, 6, 7])]);
        // a zone change closes without a message
        open(&mut gui, &mut g, &zone(0x800));
        g.close_all(&mut gui);
        assert!(!g.is_open() && !g.flag);
    }

    #[test]
    fn harness_select_and_the_features_gate() {
        let Some(mut gui) = rig() else { return };
        let mut g = GridUi::default();
        // Features without bit 0x800: no window, the DValue is reset
        open(&mut gui, &mut g, &zone(0x8007));
        assert!(g.win.is_none() && !g.flag);
        let mut g = GridUi::default();
        open(&mut gui, &mut g, &zone(0x800));
        assert!(g.select(&mut gui, ME, 7).is_none());
        let out = g.select(&mut gui, ME, 1).unwrap();
        assert_eq!(out[0], grid::selected(ME, &[5, 6, 7], 1, 9991, Identity { kind: 0x9C50, instance: 9991 }));
        // the sent bytes decode as GridSelected
        let b = &out[0];
        let (h, mut r) = ao_net::n3::N3Header::parse(b).unwrap();
        assert!(matches!(grid::decode(&h, &mut r).unwrap(), Some(Grid::Selected { index: 1, playfield: 9991, .. })));
    }

    #[test]
    fn a_second_list_keeps_the_window_but_replaces_the_token() {
        let Some(mut gui) = rig() else { return };
        let mut g = GridUi::default();
        let z = zone(0x800);
        open(&mut gui, &mut g, &z);
        g.activate(&mut gui, (1280, 800), &z, ME, vec![dest(1, "Only")], vec![9]);
        assert_eq!(g.entries.len(), 3, "the open window keeps its list");
        assert_eq!(g.token, [9]);
        // an unknown dynel is ignored
        let mut g = GridUi::default();
        g.activate(&mut gui, (1280, 800), &z, Identity { kind: DYNEL_CHAR, instance: 1 }, vec![], vec![]);
        assert!(g.win.is_none() && !g.flag);
    }

    /// The whole path through [`Interact`]: a `GridDestinationSelect` frame opens the window, `grid_select` answers with the `GridSelected` frame.
    #[test]
    fn frames_in_frames_out_through_interact() {
        use super::super::interact::Interact;
        use ao_net::n3::{self, N3};
        let Some(mut gui) = rig() else { return };
        let mut z = zone(0x800);
        let mut it = Interact::new(OWN, (1280, 800));
        let list = Grid::DestinationSelect { destinations: vec![dest(9991, "Alpha"), dest(9990, "Mid")], token: vec![1, 2] };
        it.on_frame(&mut gui, &ao_net::n3::outgoing::n3_frame(0, 1, list.encode(ME)), &mut z);
        assert!(it.grid_dump(&gui).contains("Mid | unknown pf"), "{}", it.grid_dump(&gui));
        assert!(!it.grid_select(&mut gui, 5));
        assert!(it.grid_select(&mut gui, 0));
        let sent: Vec<_> = it.take_outbox().iter().map(|f| n3::decode(f).unwrap().body).collect();
        let Some(N3::Grid(Grid::Selected { index: 0, playfield: 9991, token, .. })) = sent.first().cloned() else { panic!("{sent:?}") };
        assert_eq!(token, [1, 2]);
        assert_eq!(sent.len(), 2, "selection, then the close path's cancel");
        assert_eq!(it.grid_dump(&gui), "grid: no window");
    }

    /// Known playfields show their `pfnrmap.dat` name and the list starts sorted by that name (column 0 compares the playfield), ties in list order.
    #[test]
    fn rows_start_sorted_by_playfield_name() {
        let Some(mut gui) = rig() else { return };
        if !ao_gui::client_dir().join("cd_image/data/launcher/pfnrmap.dat").exists() {
            return;
        }
        let mut g = GridUi::default();
        // 560 Mort, 4582 ICC Shuttleport, 550 Athen Shire, 551 Wailing Wastes, 550 again
        let list = vec![dest(560, "a"), dest(4582, "b"), dest(550, "c"), dest(551, "d"), dest(550, "e")];
        g.activate(&mut gui, (1280, 800), &zone(0x800), ME, list, vec![]);
        let win = g.win.unwrap();
        assert_eq!(gui.multi_row_ids(win, "targets"), [2, 4, 1, 0, 3]);
        assert!(g.dump(&gui).contains("[2] c | Athen Shire"), "{}", g.dump(&gui));
        // a click on the "Playfield" header sorts by the other column's compare (the location)
        assert!(gui.multi_sort(win, "targets", 1, false));
        assert_eq!(gui.multi_row_ids(win, "targets"), [0, 1, 2, 3, 4]);
    }

    /// What the capture says about the module's gate: the own character's `Features` in the new-character capture.
    #[test]
    fn new_character_features_lack_the_gate_bit() {
        let mut z = Zone::new(33512);
        for l in include_str!("../../../../docs/captures/zone_newchar_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (Some(_), Some(dir), Some(hex)) = (p.next(), p.next(), p.next()) else { continue };
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            if dir == "<" {
                if let Ok(Some((f, _))) = ao_net::frame::Frame::decode_with(&b, false) {
                    z.on_frame(&f);
                }
            }
        }
        // [LIVE] a stock character has Features 0x6: the original's module would refuse a grid list for it (the server must grant bit 0x800 first)
        assert_eq!(z.stat(STAT_FEATURES), Some(6));
        assert_eq!(6 & FEATURE_OPEN, 0);
    }

    #[test]
    fn rank_keys_order_like_string_compare() {
        let r = ranks(["b", "a", "b", "C"].into_iter());
        assert_eq!((r["C"], r["a"], r["b"]), (0, 1, 2));
    }
}
