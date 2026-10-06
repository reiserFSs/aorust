//! Windows of the social layer: the Friends window (`FriendListView_c`, GUI 0x100a9c58), per-character tell windows, the private group
//! invitation / confirmation dialogs and the looking-for-team window (`LFTWindow_c`, GUI 0x100f03fb). Logic lives in [`super::social`];
//! evidence and GUESS labels in docs/chat/social.md.

use super::social::{self, Folder, Lft, Node, Social, LFT_DEFAULT, LFT_LOCATIONS, LFT_SIDES};
use ao_formats::screens::TextDb;
use ao_gui::{Event, Gui, GfxId, MenuItem, WindowId, WindowSize};
use ao_net::chat::LftReply;
use std::fmt::Write as _;
use std::path::PathBuf;

/// Namespace of this module's popup menu ids ([`Event::MenuPicked`]).
const MENU_BASE: u32 = 0x5C00_0000;

/// What the windows ask the hub to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Req {
    OpenTell { id: u32, name: String },
    /// Enter in a tell window's input bar.
    Tell { id: u32, name: String, text: String },
    Befriend(u32),
    RemoveFriend(u32),
    /// Menu "Invite X": `C2S_PRIVGRP_INVITE`.
    Invite(u32),
    Ignore { id: u32, name: String, on: bool },
    /// Yes / No of the private group invitation dialog.
    /// `windows` = `window_name`s whose check box was ticked (the chat windows the group is assigned to).
    Answer { id: u32, accept: bool, windows: Vec<String> },
    LftSearch { side: u32, profession: u32, location: u32 },
    LftSet { on: bool, description: String },
    /// LFT window "Invite to team" (`N3Msg_TeamJoinRequest`).
    LftInvite { id: u32, name: String },
}

// ------------------------------------------------------------------------------------------------ persistence

/// `<prefs dir>/<file>`: stand-in for the DValue (`FriendsWindowConfig`, `LFTWindowConfig`) the original keeps in the character prefs.
fn cfg_path(file: &str) -> Option<PathBuf> {
    super::super::prefs::dir().map(|d| d.join(file))
}

fn read_cfg(file: &str) -> Vec<(String, String)> {
    let Some(src) = cfg_path(file).and_then(|p| std::fs::read_to_string(p).ok()) else { return vec![] };
    let Ok(root) = ao_gui::xml::parse(&src) else { return vec![] };
    root.children.iter().filter_map(|c| Some((c.attr("name")?.to_owned(), c.attr("value")?.trim_matches('\'').trim_matches('"').to_owned()))).collect()
}

/// `Message::SaveToXML`-style archive (same schema as the chat window configs).
fn cfg_xml(items: &[(&str, &str, String)]) -> String {
    let mut o = String::from("<Archive code=\"0\">\n");
    for (ty, k, v) in items {
        if *ty == "String" {
            let _ = writeln!(o, "    <String name=\"{k}\" value='&quot;{}&quot;' />", v.replace('&', "&amp;").replace('<', "&lt;").replace('\'', "&apos;"));
        } else {
            let _ = writeln!(o, "    <{ty} name=\"{k}\" value=\"{v}\" />");
        }
    }
    o + "</Archive>\n"
}

fn write_cfg(file: &str, items: &[(&str, &str, String)]) {
    if let Some(p) = cfg_path(file) {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, cfg_xml(items));
    }
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

// ------------------------------------------------------------------------------------------------ texts

/// Text of category 10001 / 10000 / 100 by key (`GetText(0x2711 / 0x2710 / 0x64, key)`).
pub struct Texts<'a>(pub &'a TextDb);

impl Texts<'_> {
    pub fn social(&self, key: &str) -> String {
        self.0.by_key(10001, key).unwrap_or_else(|| key.to_owned())
    }
    pub fn msgbox(&self, key: &str) -> String {
        self.0.by_key(10000, key).or_else(|| self.0.by_key(10001, key)).unwrap_or_else(|| key.to_owned())
    }
    fn x(&self, key: &str, name: &str) -> String {
        self.social(key).replacen("%s", name, 1)
    }
}

// ------------------------------------------------------------------------------------------------ friends

/// `FriendListView_c` default frame `Rect(0, 0, 169, 215)` (`_DAT_101b9e70` / `_DAT_101b9e74`).
const FRIENDS_SIZE: (u32, u32) = (169, 215);
/// Folder order of the list (`FUN_100a9c58`: chat windows first, then online / offline / recent) and their text keys.
const FOLDERS: [&str; 4] = ["ChatWindows", "OnlineFriends", "OfflineFriends", "RecentMessages"];
/// `is_chat_window_list_open`, `is_online_list_open`, `is_offline_list_open`, `is_recent_list_open` (all default true).
const FOLDER_KEYS: [&str; 4] = ["is_chat_window_list_open", "is_online_list_open", "is_offline_list_open", "is_recent_list_open"];
/// Label colour pair of a normal item (**GUESS**: `StringListViewItem` default) and of red ones (`SetLabelColor(0xff6666, 0xc04c4c)`).
const LABEL: (u32, u32) = (0xc0c0c0, 0xffffff);
const LABEL_RED: (u32, u32) = (0xff6666, 0xc04c4c);

struct FriendsWin {
    win: WindowId,
    open: [bool; 4],
}

fn gfx_name(gui: &Gui, id: u32) -> String {
    gui.gfx().name(GfxId(id)).unwrap_or("").to_owned()
}

fn row(gui: &Gui, name: &str, icon: u32, text: &str, colors: (u32, u32), indent: u32) -> String {
    format!(
        "<View view_layout=\"horizontal\" layout_borders=\"Rect({indent},0,0,0)\"><BitmapView name=\"ico_{name}\" bitmap_id=\"{}\"/><TextButton name=\"{name}\" text=\"{}\" color=\"{}\" hover_color=\"{}\" pressed_color=\"{}\" layout_borders=\"Rect(3,0,0,0)\"/></View>",
        gfx_name(gui, icon),
        esc(text),
        colors.0,
        colors.1,
        colors.1
    )
}

// ------------------------------------------------------------------------------------------------ tells

struct TellWin {
    id: u32,
    name: String,
    win: WindowId,
    lines: Vec<String>,
}

const MAX_TELL_LINES: usize = 100;

// ------------------------------------------------------------------------------------------------ dialogs

enum Confirm {
    /// `windows[i]` is the `window_name` of check box `cw_i`.
    Invite { id: u32, windows: Vec<String> },
    Remove { id: u32 },
    Ignore { id: u32, name: String },
}

struct Dlg {
    win: WindowId,
    what: Confirm,
}

// ------------------------------------------------------------------------------------------------ LFT

/// Column widths of the candidate list (`FUN_100f03fb`: `_DAT_101ae4d8`, `101ae5c8`, `101b20a8`, `101aec64`, `101a95a0` x2).
const LFT_COLS: [u32; 6] = [120, 30, 50, 60, 180, 180];
/// `LFTWindow_c` frame `Rect(200, 180, 849, 700)` (`_DAT_101a959c`, `_DAT_101a95a0`, `_DAT_101c10b0`, `_DAT_101a95a8`).
const LFT_FRAME: (i32, i32, u32, u32) = (200, 180, 650, 521);

struct LftWin {
    win: WindowId,
    side: usize,
    location: usize,
    profession: usize,
    professions: Vec<u32>,
    selected: Option<usize>,
    /// `LFTCandidateItem_c` rows (the view of each row's cells).
    rows: Vec<ao_gui::ViewHandle>,
}

#[derive(Default)]
pub struct SocialWin {
    screen: (u32, u32),
    friends: Option<FriendsWin>,
    tells: Vec<TellWin>,
    dialogs: Vec<Dlg>,
    lft: Option<LftWin>,
    /// Popup menu entries of the open friend menu: `(user id, name, action)`.
    menu: Vec<(u32, String, Action)>,
    folder_state: Option<[bool; 4]>,
    /// HUD dvalues of windows the user closed with the frame button (`friends_window`, `lft_window`): the HUD button pops out.
    pub closed: Vec<&'static str>,
    flash: f32,
    /// `N3Msg_GetPFName` names (`pfnrmap.dat`), loaded with the LFT window.
    pf_names: std::collections::HashMap<u32, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Befriend,
    Invite,
    Delete,
    Ignore,
    Unignore,
}

fn fit(gui: &mut Gui, text: &str, w: u32) -> String {
    let font = ao_gui::FontId::Normal;
    if gui.text_width(font, text) <= w as i32 {
        return text.to_owned();
    }
    let mut s: String = text.to_owned();
    while !s.is_empty() && gui.text_width(font, &s) > w as i32 {
        s.pop();
    }
    s
}

impl SocialWin {
    pub fn new(screen: (u32, u32)) -> Self {
        Self { screen, ..Default::default() }
    }

    pub fn resize(&mut self, screen: (u32, u32)) {
        self.screen = screen;
    }

    pub fn owns(&self, w: WindowId) -> bool {
        self.friends.as_ref().is_some_and(|f| f.win == w) || self.tells.iter().any(|t| t.win == w) || self.dialogs.iter().any(|d| d.win == w) || self.lft.as_ref().is_some_and(|l| l.win == w)
    }

    pub fn lft_window(&self) -> Option<WindowId> {
        self.lft.as_ref().map(|l| l.win)
    }

    // -------------------------------------------------------------------------------------------- friends

    /// `SlotFriendWindowActivated(window, true/false)` 0x100871a0: opens the window if there is none / closes it.
    pub fn set_friends(&mut self, gui: &mut Gui, open: bool, soc: &Social, tx: &Texts, chat_windows: &[String]) {
        match (open, self.friends.is_some()) {
            (true, false) => {
                let cfg = read_cfg("FriendsWindowConfig.xml");
                let open = FOLDER_KEYS.map(|k| cfg.iter().find(|(n, _)| n == k).is_none_or(|(_, v)| v == "true"));
                let src = "<root><View view_layout=\"vertical\"><ScrollView name=\"scroll\" v_scrollbar_mode=\"auto\" h_scrollbar_mode=\"auto\" max_size=\"Point(16000,16000)\"><ScrollViewChild view_layout=\"vertical\" max_size=\"Point(16000,16000)\"><View name=\"rows\" view_layout=\"vertical\" h_alignment=\"left\"/></ScrollViewChild></ScrollView></View></root>";
                let Ok(win) = gui.open_tabbed_window_xml("Friends", "Friends", src, (0, 0), WindowSize::Fixed(FRIENDS_SIZE.0, FRIENDS_SIZE.1)) else { return };
                // `Window::MoveToCenter` (no saved frame): GUESS, the position is not in the binary's defaults
                let (w, h) = gui.outer_size(win);
                gui.set_window_pos(win, ((self.screen.0 as i32 - w as i32) / 2, (self.screen.1 as i32 - h as i32) / 2));
                gui.set_window_context(win, true);
                self.friends = Some(FriendsWin { win, open });
                self.refresh_friends(gui, soc, tx, chat_windows);
            }
            (false, true) => {
                if let Some(f) = self.friends.take() {
                    self.save_friends(&f);
                    gui.close_window(f.win);
                }
            }
            _ => {}
        }
    }

    fn save_friends(&mut self, f: &FriendsWin) {
        let items: Vec<(&str, &str, String)> = FOLDER_KEYS.iter().zip(f.open).map(|(k, o)| ("Bool", *k, o.to_string())).collect();
        write_cfg("FriendsWindowConfig.xml", &items);
        self.folder_state = Some(f.open);
    }

    /// Rebuilds the item rows: folders in list order, items sorted by name without case (`CompareNoCase`, `FUN_100a8141`).
    pub fn refresh_friends(&mut self, gui: &mut Gui, soc: &Social, tx: &Texts, chat_windows: &[String]) {
        let Some(f) = &self.friends else { return };
        let (win, open) = (f.win, f.open);
        gui.remove_children(win, "rows");
        let mut by_folder: [Vec<&Node>; 3] = Default::default();
        for n in soc.nodes.values() {
            by_folder[match n.folder() {
                Folder::Online => 0,
                Folder::Offline => 1,
                Folder::Recent => 2,
            }]
            .push(n);
        }
        for v in &mut by_folder {
            v.sort_by_key(|n| n.name.to_lowercase());
        }
        let mut cw: Vec<&String> = chat_windows.iter().collect();
        cw.sort_by_key(|n| n.to_lowercase());
        for (i, key) in FOLDERS.iter().enumerate() {
            let count = if i == 0 { cw.len() } else { by_folder[i - 1].len() };
            // folder icons 0xd5 / 0xd4 (`StringListViewItem(.., 0xd5, 0xd4)`); which is open is a GUESS (0xd4 open)
            let head = row(gui, &format!("folder_{i}"), if open[i] { 0xd4 } else { 0xd5 }, &format!("{} ({count})", tx.social(key)), (0xffffff, 0xffffff), 0);
            let _ = gui.add_view_xml(win, "rows", &format!("folder_row_{i}"), &format!("<root>{head}</root>"));
            if !open[i] {
                continue;
            }
            if i == 0 {
                for (k, n) in cw.iter().enumerate() {
                    let r = row(gui, &format!("cw_{k}"), 0xd8, n, LABEL, 14);
                    let _ = gui.add_view_xml(win, "rows", &format!("cw_row_{k}"), &format!("<root>{r}</root>"));
                }
                continue;
            }
            for n in &by_folder[i - 1] {
                let name = if n.name.is_empty() { format!("#{}", n.id) } else { n.name.clone() };
                let colors = if n.red() { LABEL_RED } else { LABEL };
                let r = row(gui, &format!("fr_{}", n.id), n.icon(), &name, colors, 14);
                let _ = gui.add_view_xml(win, "rows", &format!("fr_row_{}", n.id), &format!("<root>{r}</root>"));
            }
        }
    }

    fn friends_event(&mut self, gui: &mut Gui, ev: &Event, soc: &Social, tx: &Texts, chat_windows: &[String]) -> Vec<Req> {
        let Some(f) = &mut self.friends else { return vec![] };
        let win = f.win;
        match ev {
            Event::Clicked { window, view, .. } if *window == win => {
                if let Some(i) = view.strip_prefix("folder_").and_then(|n| n.parse::<usize>().ok()) {
                    f.open[i] = !f.open[i];
                    self.refresh_friends(gui, soc, tx, chat_windows);
                } else if let Some(id) = view.strip_prefix("fr_").and_then(|n| n.parse::<u32>().ok()) {
                    // selecting a friend opens its tell window (`FriendListView_c` selection signal -> `OpenTellWindow`)
                    let name = soc.nodes.get(&id).map(|n| n.name.clone()).unwrap_or_default();
                    return vec![Req::OpenTell { id, name }];
                }
                vec![]
            }
            Event::CloseRequested { window } if *window == win => {
                if let Some(f) = self.friends.take() {
                    self.save_friends(&f);
                    gui.close_window(f.win);
                }
                self.closed.push("friends_window");
                vec![]
            }
            Event::ContextMenu { window, x, y, .. } if *window == win => {
                let hit = soc.nodes.keys().copied().find(|id| gui.view_rect(win, &format!("fr_{id}")).is_some_and(|r| (*x as f32) >= r.l && (*x as f32) <= r.r && (*y as f32) >= r.t && (*y as f32) <= r.b));
                if let Some(id) = hit {
                    self.open_friend_menu(gui, soc, tx, id, (*x, *y));
                }
                vec![]
            }
            _ => vec![],
        }
    }

    /// `FUN_100a930e`: "Befriend X" (recent entries only), "Invite X" (greyed for ignored), "Delete X", "Ignore X" / "Unignore X".
    /// ("Mail X" only exists while the MailWindow is open: it is not ported.)
    fn open_friend_menu(&mut self, gui: &mut Gui, soc: &Social, tx: &Texts, id: u32, at: (i32, i32)) {
        let Some(n) = soc.nodes.get(&id) else { return };
        let mut entries: Vec<(Action, &str, bool)> = vec![];
        if n.state == 2 {
            entries.push((Action::Befriend, "BefriendX", true));
        }
        entries.push((Action::Invite, "InviteX", n.state != 3));
        entries.push((Action::Delete, "DeleteX", true));
        entries.push(if n.state == 3 { (Action::Unignore, "UnignoreX", true) } else { (Action::Ignore, "IgnoreX", true) });
        self.menu.clear();
        let mut items = vec![];
        for (k, (a, key, on)) in entries.into_iter().enumerate() {
            let mut it = MenuItem::entry(MENU_BASE + k as u32, &tx.x(key, &n.name));
            it.enabled = on;
            items.push(it);
            self.menu.push((id, n.name.clone(), a));
        }
        gui.open_menu(at, (self.screen.0 as i32, self.screen.1 as i32), items);
    }

    /// `StringListViewItem_c::FlashIcon`: icons of nodes with unread messages / a pending invitation blink (**GUESS**: 0.5 s period).
    pub fn update(&mut self, gui: &mut Gui, dt: f32, soc: &Social) {
        self.flash += dt;
        let on = ((self.flash / 0.5) as u32).is_multiple_of(2);
        let Some(f) = &self.friends else { return };
        for n in soc.nodes.values().filter(|n| n.flashing()) {
            gui.set_visible(f.win, &format!("ico_fr_{}", n.id), on);
        }
    }

    // -------------------------------------------------------------------------------------------- tell windows

    /// `FUN_100a75e8(1, 1)`: show and focus the tell window of `id` (created on first use).
    pub fn open_tell(&mut self, gui: &mut Gui, id: u32, name: &str) {
        if let Some(t) = self.tells.iter().find(|t| t.id == id) {
            gui.set_window_visible(t.win, true);
            gui.focus(t.win, "input");
            return;
        }
        // GUESS: TellWindow_c (per-id `<id>.xml` config / `<id>.log` in the Friends directory) was not decompiled; the layout is the chat view's
        // text area + input bar (docs/chat/gui.md section 3) in a style-0 window titled with the character name.
        let lh = gui.font_height(ao_gui::FontId::Chat) as u32;
        let flags = "TVF_ENABLE_SHADOW|TVF_FILL_BOTTOM_UP|TVF_DISABLE_RC_MENU|TVF_WORD_WRAP|TVF_MULTILINE|TVF_ALLOW_TEXT_SELECTION|TVF_ACCEPT_MOUSE_INPUT";
        let src = format!(
            r#"<root><View name="tell" view_layout="vertical" layout_borders="Rect(3,3,3,3)">
                 <BorderView name="text_border" max_size="Point(16000,16000)">
                   <ScrollView name="scroll" v_scrollbar_mode="always" layout_borders="Rect(5,5,5,5)" max_size="Point(16000,16000)">
                     <ScrollViewChild view_layout="vertical" max_size="Point(16000,16000)">
                       <TextView name="text" max_size="Point(16000,-1)" font="CHAT" feature_flags="{flags}"/>
                     </ScrollViewChild>
                   </ScrollView>
                 </BorderView>
                 <BorderView name="input_border" layout_borders="Rect(0,5,0,0)" min_size="Point(-1,{ih})" max_size="Point(16000,{ih})">
                   <TextView name="input" max_size="Point(16000,-1)" layout_borders="Rect(5,5,5,5)" font="CHAT" feature_flags="TVF_ACCEPT_TXT_INPUT|TVF_ACCEPT_MOUSE_INPUT|TVF_ALLOW_TEXT_SELECTION|TVF_ENABLE_SHADOW"/>
                 </BorderView>
               </View></root>"#,
            ih = lh + 10
        );
        let k = self.tells.len() as i32;
        let Ok(win) = gui.open_tabbed_window_xml("TellWindow", name, &src, (220 + 24 * k, 160 + 24 * k), WindowSize::Fixed(300, 170)) else { return };
        gui.set_text_shadow_offset(1, 1);
        gui.focus(win, "input");
        self.tells.push(TellWin { id, name: name.to_owned(), win, lines: vec![] });
    }

    /// `FUN_100a6cd5(text)`: a line of the tell window (the window only exists once opened: earlier lines are kept by the hub).
    pub fn tell_text(&mut self, gui: &mut Gui, id: u32, html: &str) {
        let Some(t) = self.tells.iter_mut().find(|t| t.id == id) else { return };
        t.lines.push(format!("<div indent=wrapped>{html}</div>"));
        if t.lines.len() > MAX_TELL_LINES {
            t.lines.remove(0);
        }
        let all = t.lines.concat();
        gui.set_text(t.win, "text", &all);
        gui.scroll_to_bottom(t.win, "scroll");
    }

    #[cfg(test)]
    pub fn dialog_windows(&self) -> Vec<WindowId> {
        self.dialogs.iter().map(|d| d.win).collect()
    }

    #[cfg(test)]
    pub fn tell_window_for_test(&self, id: u32) -> Option<WindowId> {
        self.tells.iter().find(|t| t.id == id).map(|t| t.win)
    }

    pub fn tell_is_open(&self, id: u32) -> bool {
        self.tells.iter().any(|t| t.id == id)
    }

    // -------------------------------------------------------------------------------------------- dialogs

    fn dialog(&mut self, gui: &mut Gui, body: &str, what: Confirm, tx: &Texts, title: &str, boxes: &[(String, bool)]) {
        let extra: String = boxes.iter().enumerate().map(|(i, (n, _))| format!("<CheckBox name=\"cw_{i}\" label=\"{}\" layout_borders=\"Rect(25,0,15,2)\"/>", esc(n))).collect();
        let w = ((2.0 * body.chars().count() as f32 * 7.0 * 14.0).sqrt() as u32).clamp(200, 520);
        let src = format!(
            "<root><View view_layout=\"vertical\"><TextView name=\"text\" feature_flags=\"TVF_MULTILINE|TVF_WORD_WRAP\" min_size=\"Point({w},-1)\" max_size=\"Point({w},-1)\" layout_borders=\"Rect(15,5,15,{})\"/>{extra}<View view_layout=\"horizontal\" layout_borders=\"Rect(0,0,0,5)\"><HLayoutSpacer/><Button name=\"btn0\" label=\"{}\" layout_borders=\"Rect(8,0,8,0)\"/><Button name=\"btn1\" label=\"{}\" layout_borders=\"Rect(8,0,8,0)\"/></View></View></root>",
            if extra.is_empty() { 20 } else { 5 },
            esc(&tx.msgbox("MsgBox_Yes")),
            esc(&tx.msgbox("MsgBox_No"))
        );
        let Ok(win) = gui.open_tabbed_window_xml("DialogBox", title, &src, (0, 0), WindowSize::Preferred) else { return };
        gui.set_text(win, "text", body);
        for (i, (_, on)) in boxes.iter().enumerate() {
            gui.set_checked(win, &format!("cw_{i}"), *on);
        }
        gui.set_default_button(win, "btn0");
        gui.resize_window(win, WindowSize::Preferred); // the body is set after the window exists
        let (ow, oh) = gui.outer_size(win);
        gui.set_window_pos(win, ((self.screen.0 as i32 - ow as i32) / 2, (self.screen.1 as i32 - oh as i32) / 2));
        self.dialogs.push(Dlg { win, what });
    }

    /// `FUN_100a700a`: "PrivateGroupInvitation" dialog with `ChatFriendList_InvitedToPrivateGroupDialogText`; Yes joins, No declines.
    /// (The original lists the chat windows as check boxes to assign the group to: see docs/chat/social.md, gap.)
    /// `windows` = `(window_name, name)` of the chat windows, one check box each (default ticked: the first window, **GUESS**: `FUN_100a7d13`'s default is not decoded).
    pub fn invite_dialog(&mut self, gui: &mut Gui, id: u32, name: &str, tx: &Texts, windows: &[(String, String)]) {
        let body = tx.x("ChatFriendList_InvitedToPrivateGroupDialogText", name).replace("\\r\\n", "<br>").replace("\r\n", "<br>");
        let boxes: Vec<(String, bool)> = windows.iter().enumerate().map(|(i, (_, n))| (n.clone(), i == 0)).collect();
        let what = Confirm::Invite { id, windows: windows.iter().map(|w| w.0.clone()).collect() };
        self.dialog(gui, &body, what, tx, &tx.msgbox("PrivateGroupInvitation"), &boxes);
    }

    fn answer(&mut self, gui: &mut Gui, win: WindowId, yes: bool) -> Vec<Req> {
        let Some(i) = self.dialogs.iter().position(|d| d.win == win) else { return vec![] };
        let d = self.dialogs.remove(i);
        let ticked: Vec<bool> = match &d.what {
            Confirm::Invite { windows, .. } => (0..windows.len()).map(|k| gui.checked(d.win, &format!("cw_{k}"))).collect(),
            _ => vec![],
        };
        gui.close_window(d.win);
        match d.what {
            Confirm::Invite { id, windows } => vec![Req::Answer { id, accept: yes, windows: windows.into_iter().zip(ticked).filter(|(_, t)| *t).map(|(w, _)| w).collect() }],
            Confirm::Remove { id } if yes => vec![Req::RemoveFriend(id)],
            Confirm::Ignore { id, name } if yes => vec![Req::Ignore { id, name, on: true }],
            _ => vec![],
        }
    }

    // -------------------------------------------------------------------------------------------- LFT

    /// `FUN_100f03fb`: the window from `Views/LFTView.xml`, tab "Team Search" (`GetText(100, "Team Search")`), config `LFTWindowConfig`.
    pub fn open_lft(&mut self, gui: &mut Gui, lft: &Lft, tx: &Texts, in_team_blocked: bool) {
        if let Some(l) = &self.lft {
            gui.set_window_visible(l.win, true);
            return;
        }
        let cfg = read_cfg("LFTWindowConfig.xml");
        let get = |k: &str| cfg.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let title = tx.0.by_key(100, "Team Search").unwrap_or_else(|| "Team Search".into());
        let (x, y, w, h) = LFT_FRAME;
        // `Window::MoveToCenter` then `LoadWndConfig` (saved frame): centre
        let _ = (x, y);
        let (cw, ch) = (w - 10, h - 31); // client = outer minus the frame insets (5, 26, 5, 5)
        let Ok(win) = gui.open_tabbed_window("LFTView", &title, (0, 0), WindowSize::Fixed(cw, ch)) else { return };
        let (ow, oh) = gui.outer_size(win);
        gui.set_window_pos(win, ((self.screen.0 as i32 - ow as i32) / 2 + 5, (self.screen.1 as i32 - oh as i32) / 2 + 26));
        let professions: Vec<u32> = social::lft_professions().chain([0x10]).collect();
        // dropdown contents (the original inserts every item at its id as index: the list order is the id order)
        let sides: Vec<String> = LFT_SIDES.iter().map(|(_, t)| t.and_then(|t| tx.0.by_id(2005, t)).unwrap_or_else(|| "any".into())).collect();
        let locs: Vec<String> = LFT_LOCATIONS.iter().map(|(_, n)| n.to_string()).collect();
        let profs: Vec<String> = professions.iter().map(|p| if *p == 0x10 { tx.0.by_key(100, "any").unwrap_or_else(|| "any".into()) } else { tx.0.by_id(2004, *p).unwrap_or_default() }).collect();
        gui.combo_set_items(win, "Side", sides.clone());
        gui.combo_set_items(win, "Location", locs.clone());
        gui.combo_set_items(win, "Profession", profs.clone());
        let sel = |k: &str, default: u32, ids: &[u32]| -> usize {
            let v = get(k).and_then(|v| v.parse::<u32>().ok()).unwrap_or(default);
            ids.iter().position(|i| *i == v).or_else(|| ids.iter().position(|i| *i == default)).unwrap_or(0)
        };
        let side = sel("SelectedSide", LFT_DEFAULT.0, &LFT_SIDES.map(|s| s.0));
        let location = sel("SelectedLocation", LFT_DEFAULT.1, &LFT_LOCATIONS.map(|s| s.0));
        let profession = sel("SelectedProfession", LFT_DEFAULT.2, &professions);
        gui.set_text(win, "Side", &sides[side]);
        gui.set_text(win, "Location", &locs[location]);
        gui.set_text(win, "Profession", &profs[profession]);
        let desc = if lft.description.is_empty() { get("TeamDesc").unwrap_or_default() } else { lft.description.clone() };
        gui.set_text(win, "Description", &desc);
        gui.set_checked(win, "LFT", lft.on);
        gui.set_enabled(win, "Invite", !in_team_blocked);
        // `CandidateView` holds the `MultiListView_c` (6 columns); ao-gui has none: header + rows of fixed-width cells (GUESS look)
        let cols = [tx.0.by_key(100, "Name").unwrap_or_default(), tx.0.by_id(2003, 0x36).unwrap_or_default(), tx.0.by_id(2003, 0x21).unwrap_or_default(), tx.0.by_id(2003, 0x3c).unwrap_or_default(), tx.0.by_key(100, "Location").unwrap_or_default(), tx.0.by_key(100, "Description").unwrap_or_default()];
        let mut head = String::new();
        for (c, w) in cols.iter().zip(LFT_COLS) {
            let _ = write!(head, "<TextView value=\"{}\" min_size=\"Point({w},-1)\" max_size=\"Point({w},-1)\"/>", esc(c));
        }
        let list = format!(
            "<root><View view_layout=\"vertical\" max_size=\"Point(16000,16000)\"><View view_layout=\"horizontal\">{head}</View><ScrollView name=\"cand_scroll\" v_scrollbar_mode=\"auto\" h_scrollbar_mode=\"auto\" max_size=\"Point(16000,16000)\"><ScrollViewChild view_layout=\"vertical\" max_size=\"Point(16000,16000)\"><View name=\"cand_rows\" view_layout=\"vertical\" h_alignment=\"left\"/></ScrollViewChild></ScrollView></View></root>"
        );
        let _ = gui.add_view_xml(win, "CandidateView", "candidates", &list);
        gui.set_default_button(win, "Search");
        self.pf_names = ao_formats::screens::pfnr_names(&ao_gui::client_dir()).unwrap_or_default();
        self.lft = Some(LftWin { win, side, location, profession, professions, selected: None, rows: vec![] });
        self.lft_rows(gui, lft, tx);
    }

    /// Candidate rows (`FUN_100efe4b` -> `LFTCandidateItem_c`: name, level, side, profession, playfield name, description).
    pub fn lft_rows(&mut self, gui: &mut Gui, lft: &Lft, tx: &Texts) {
        let Some(l) = &mut self.lft else { return };
        let win = l.win;
        gui.remove_children(win, "cand_rows");
        l.rows.clear();
        l.selected = None;
        for (i, r) in lft.results.iter().enumerate() {
            let cells = lft_cells(r, tx, &self.pf_names);
            let mut x = String::from("<View view_layout=\"horizontal\">");
            for (c, (text, w)) in cells.iter().zip(LFT_COLS).enumerate() {
                let t = fit(gui, text, w);
                let _ = write!(x, "<TextButton name=\"cand_{i}_{c}\" text=\"{}\" color=\"{}\" hover_color=\"{}\" pressed_color=\"{}\" min_size=\"Point({w},-1)\" max_size=\"Point({w},-1)\"/>", esc(&t), LABEL.0, LABEL.1, LABEL.1);
            }
            x += "</View>";
            if let Ok(h) = gui.add_view_xml(win, "cand_rows", &format!("cand_row_{i}"), &format!("<root>{x}</root>")) {
                for c in 0..6 {
                    gui.set_toggle_in(h, &format!("cand_{i}_{c}"), true, false);
                }
                l.rows.push(h);
            }
        }
    }

    fn lft_event(&mut self, gui: &mut Gui, ev: &Event, lft: &Lft, tx: &Texts) -> Vec<Req> {
        let Some(l) = &mut self.lft else { return vec![] };
        let win = l.win;
        match ev {
            Event::ComboChanged { window, view, index, .. } if *window == win => {
                match view.as_str() {
                    "Side" => l.side = *index,
                    "Location" => l.location = *index,
                    "Profession" => l.profession = *index,
                    _ => {}
                }
                vec![]
            }
            Event::Clicked { window, view, .. } if *window == win => match view.as_str() {
                "Search" => vec![Req::LftSearch { side: LFT_SIDES[l.side.min(3)].0, profession: l.professions[l.profession.min(l.professions.len() - 1)], location: LFT_LOCATIONS[l.location.min(3)].0 }],
                "LFT" => vec![Req::LftSet { on: gui.checked(win, "LFT"), description: gui.text(win, "Description") }],
                "Invite" | "Tell" => {
                    let Some(r) = l.selected.and_then(|i| lft.results.get(i)) else { return vec![] };
                    let (id, name) = (r.id, r.name.clone());
                    vec![if view == "Invite" { Req::LftInvite { id, name } } else { Req::OpenTell { id, name } }]
                }
                v => {
                    // a cell of row i: select the row (`MultiListViewItem_c::Select`), pressed look = highlight
                    if let Some(i) = v.strip_prefix("cand_").and_then(|r| r.split('_').next()).and_then(|n| n.parse::<usize>().ok()) {
                        if let Some(old) = l.selected {
                            for c in 0..6 {
                                self.lft_cell_toggle(gui, old, c, false);
                            }
                        }
                        for c in 0..6 {
                            self.lft_cell_toggle(gui, i, c, true);
                        }
                        if let Some(l) = &mut self.lft {
                            l.selected = Some(i);
                        }
                    }
                    vec![]
                }
            },
            Event::EnterPressed { window, view } if *window == win && view == "Description" => {
                if gui.checked(win, "LFT") {
                    vec![Req::LftSet { on: true, description: gui.text(win, "Description") }]
                } else {
                    vec![]
                }
            }
            Event::CloseRequested { window } if *window == win => {
                self.close_lft(gui, tx);
                self.closed.push("lft_window");
                vec![]
            }
            _ => vec![],
        }
    }

    fn lft_cell_toggle(&mut self, gui: &mut Gui, row: usize, col: usize, on: bool) {
        let Some(h) = self.lft.as_ref().and_then(|l| l.rows.get(row).copied()) else { return };
        gui.set_toggle_in(h, &format!("cand_{row}_{col}"), true, on);
    }

    /// `~LFTWindow_c` (`FUN_100efb97`): saves `LFTWindowConfig` (selections, team description, column widths).
    pub fn close_lft(&mut self, gui: &mut Gui, _tx: &Texts) {
        let Some(l) = self.lft.take() else { return };
        let items = [
            ("Int32", "SelectedSide", LFT_SIDES[l.side.min(3)].0.to_string()),
            ("Int32", "SelectedLocation", LFT_LOCATIONS[l.location.min(3)].0.to_string()),
            ("Int32", "SelectedProfession", l.professions[l.profession.min(l.professions.len() - 1)].to_string()),
            ("String", "TeamDesc", gui.text(l.win, "Description")),
            ("Float", "NameColWidth", LFT_COLS[0].to_string()),
            ("Float", "LevelColWidth", LFT_COLS[1].to_string()),
            ("Float", "SideColWidth", LFT_COLS[2].to_string()),
            ("Float", "ProfColWidth", LFT_COLS[3].to_string()),
            ("Float", "LocationColWidth", LFT_COLS[4].to_string()),
            ("Float", "DescColWidth", LFT_COLS[5].to_string()),
        ];
        write_cfg("LFTWindowConfig.xml", &items);
        gui.close_window(l.win);
    }

    pub fn lft_busy_changed(&mut self, gui: &mut Gui, busy: bool) {
        if let Some(l) = &self.lft {
            gui.set_enabled(l.win, "Search", !busy);
        }
    }

    // -------------------------------------------------------------------------------------------- events

    /// `(handled, requests)`.
    pub fn event(&mut self, gui: &mut Gui, ev: &Event, soc: &Social, lft: &Lft, tx: &Texts, chat_windows: &[String]) -> (bool, Vec<Req>) {
        let window = match ev {
            Event::Clicked { window, .. }
            | Event::EnterPressed { window, .. }
            | Event::ComboChanged { window, .. }
            | Event::CloseRequested { window }
            | Event::Escape { window }
            | Event::ContextMenu { window, .. }
            | Event::TextChanged { window, .. }
            | Event::LinkClicked { window, .. } => Some(*window),
            _ => None,
        };
        if let Event::MenuPicked { id } = ev {
            if *id >= MENU_BASE && ((*id - MENU_BASE) as usize) < self.menu.len() {
                let (uid, name, a) = self.menu.swap_remove((*id - MENU_BASE) as usize);
                self.menu.clear();
                return (true, self.menu_action(gui, uid, name, a, tx));
            }
            return (false, vec![]);
        }
        let Some(w) = window else { return (false, vec![]) };
        if !self.owns(w) {
            return (false, vec![]);
        }
        // dialogs
        if self.dialogs.iter().any(|d| d.win == w) {
            let r = match ev {
                Event::Clicked { view, .. } => self.answer(gui, w, view == "btn0"),
                Event::EnterPressed { .. } => self.answer(gui, w, true),
                Event::Escape { .. } | Event::CloseRequested { .. } => self.answer(gui, w, false),
                _ => vec![],
            };
            return (true, r);
        }
        if self.friends.as_ref().is_some_and(|f| f.win == w) {
            return (true, self.friends_event(gui, ev, soc, tx, chat_windows));
        }
        if self.lft.as_ref().is_some_and(|l| l.win == w) {
            return (true, self.lft_event(gui, ev, lft, tx));
        }
        if let Some(i) = self.tells.iter().position(|t| t.win == w) {
            return (true, self.tell_event(gui, i, ev));
        }
        (false, vec![])
    }

    fn menu_action(&mut self, gui: &mut Gui, id: u32, name: String, a: Action, tx: &Texts) -> Vec<Req> {
        match a {
            Action::Befriend => vec![Req::Befriend(id)],
            Action::Invite => vec![Req::Invite(id)],
            Action::Unignore => vec![Req::Ignore { id, name, on: false }],
            // `FUN_100a8d40`: DialogBox (Warning, `ReallyWantToRemoveXfromFriends`), Yes -> `C2S_REM_BUDDY`
            Action::Delete => {
                let body = tx.x("ReallyWantToRemoveXfromFriends", &name);
                self.dialog(gui, &body, Confirm::Remove { id }, tx, &tx.social("Warning"), &[]);
                vec![]
            }
            Action::Ignore => {
                let body = tx.x("ReallyWant2IgnoreX", &name);
                self.dialog(gui, &body, Confirm::Ignore { id, name }, tx, &tx.social("Warning"), &[]);
                vec![]
            }
        }
    }

    fn tell_event(&mut self, gui: &mut Gui, i: usize, ev: &Event) -> Vec<Req> {
        let t = &self.tells[i];
        match ev {
            Event::EnterPressed { view, .. } if view == "input" => {
                let text = gui.text(t.win, "input").trim_end().to_owned();
                gui.set_text(t.win, "input", "");
                if text.is_empty() {
                    vec![]
                } else {
                    vec![Req::Tell { id: t.id, name: t.name.clone(), text }]
                }
            }
            Event::CloseRequested { .. } => {
                let t = self.tells.remove(i);
                gui.close_window(t.win);
                vec![]
            }
            Event::LinkClicked { href, .. } => href.strip_prefix("user://").map(|n| vec![Req::OpenTell { id: 0, name: n.to_owned() }]).unwrap_or_default(),
            _ => vec![],
        }
    }

    /// Ids of tell windows closed since the last call (so the hub clears `window_open`).
    pub fn closed_tells(&mut self, soc: &Social) -> Vec<u32> {
        soc.nodes.values().filter(|n| n.window_open && !self.tell_is_open(n.id)).map(|n| n.id).collect()
    }
}

fn lft_cells(r: &LftReply, tx: &Texts, pf: &std::collections::HashMap<u32, String>) -> [String; 6] {
    [
        r.name.clone(),
        r.level.to_string(),
        tx.0.by_id(2005, r.side as u32).unwrap_or_default(),
        tx.0.by_id(2004, r.profession as u32).unwrap_or_default(),
        // `N3Msg_GetPFName(playfield)`, "Not found" when unknown (`FUN_100efe4b`)
        pf.get(&r.playfield).cloned().unwrap_or_else(|| "Not found".into()),
        r.description.clone(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::chat::social::{Env, InviteAction};
    use ao_gui::{InputEvent, MouseButton};

    fn rig() -> Option<(Gui, TextDb)> {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() {
            return None;
        }
        std::env::set_var("AOMAC_PREFS_DIR", std::env::temp_dir().join("aomac-social-test-prefs"));
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("aomac-social-test-prefs"));
        let labels = TextDb::load(&client).ok()?;
        let db = TextDb::load(&client).ok()?;
        Some((Gui::new(&client, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).ok()?, db))
    }

    fn click(gui: &mut Gui, win: WindowId, view: &str) -> Vec<Event> {
        let r = gui.view_rect(win, view).unwrap_or_else(|| panic!("no view {view}"));
        let (x, y) = ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0);
        gui.input(InputEvent::MouseMove { x, y });
        gui.input(InputEvent::MouseDown { x, y, button: MouseButton::Left });
        gui.input(InputEvent::MouseUp { x, y, button: MouseButton::Left })
    }

    fn names(id: u32) -> String {
        match id {
            5 => "Bob".into(),
            6 => "Eve".into(),
            7 => "Zed".into(),
            _ => String::new(),
        }
    }

    fn filled() -> Social {
        let mut s = Social::default();
        let txt = |_: &str| String::new();
        let no = |_: u32| false;
        let env = Env { own_id: 1, connected: true, name_of: &names, text: &txt, invite_pref: InviteAction::Dialog, ignored: &no };
        for (id, online, data) in [(5, 1, vec![1]), (6, 0, vec![1]), (7, 1, vec![0])] {
            s.on_event(&ao_net::chat::ChatEvent::BuddyAdd { id, online, data }, &env);
        }
        s
    }

    #[test]
    fn friends_window_lists_folders_and_acts() {
        let Some((mut gui, db)) = rig() else { return };
        let tx = Texts(&db);
        let soc = filled();
        let mut w = SocialWin::new((1280, 800));
        w.set_friends(&mut gui, true, &soc, &tx, &["Default Window".into()]);
        let win = w.friends.as_ref().unwrap().win;
        // folders in the original's order, entries inside their folder
        for v in ["folder_0", "folder_1", "folder_2", "folder_3", "fr_5", "fr_6", "fr_7", "cw_0"] {
            assert!(gui.has_view(win, v), "{v}");
        }
        assert_eq!(tx.social("OnlineFriends"), "Online Friends");
        let y = |n: &str| gui.view_rect(win, n).unwrap().t;
        assert!(y("folder_0") < y("folder_1") && y("folder_1") < y("fr_5") && y("fr_5") < y("folder_2") && y("folder_2") < y("fr_6") && y("fr_6") < y("folder_3") && y("folder_3") < y("fr_7"));
        // a click on a folder closes it (its entries disappear)
        let lft = Lft::default();
        for e in click(&mut gui, win, "folder_1") {
            w.event(&mut gui, &e, &soc, &lft, &tx, &[]);
        }
        assert!(!gui.has_view(win, "fr_5") && gui.has_view(win, "fr_6"));
        for e in click(&mut gui, win, "folder_1") {
            w.event(&mut gui, &e, &soc, &lft, &tx, &[]);
        }
        // a click on a friend opens its tell window
        let mut reqs = vec![];
        for e in click(&mut gui, win, "fr_6") {
            reqs.extend(w.event(&mut gui, &e, &soc, &lft, &tx, &[]).1);
        }
        assert_eq!(reqs, [Req::OpenTell { id: 6, name: "Eve".into() }]);
        // right click -> menu "Delete Eve" -> confirmation -> Yes removes the buddy
        let r = gui.view_rect(win, "fr_6").unwrap();
        w.event(&mut gui, &Event::ContextMenu { window: win, x: r.l as i32 + 3, y: r.t as i32 + 3, link: None }, &soc, &lft, &tx, &[]);
        assert!(gui.menu_open());
        assert_eq!(w.menu.iter().map(|m| m.2).collect::<Vec<_>>(), [Action::Invite, Action::Delete, Action::Ignore]);
        let (_, r1) = w.event(&mut gui, &Event::MenuPicked { id: MENU_BASE + 1 }, &soc, &lft, &tx, &[]);
        assert!(r1.is_empty() && w.dialogs.len() == 1);
        gui.close_menu(); // the engine closes it on a real pick
        let dw = w.dialogs[0].win;
        assert!(gui.text(dw, "text").contains("remove user"), "{}", gui.text(dw, "text"));
        let mut reqs = vec![];
        for e in click(&mut gui, dw, "btn0") {
            reqs.extend(w.event(&mut gui, &e, &soc, &lft, &tx, &[]).1);
        }
        assert_eq!(reqs, [Req::RemoveFriend(6)]);
        // a recent entry also offers "Befriend"
        let r = gui.view_rect(win, "fr_7").unwrap();
        w.event(&mut gui, &Event::ContextMenu { window: win, x: r.l as i32 + 3, y: r.t as i32 + 3, link: None }, &soc, &lft, &tx, &[]);
        assert_eq!(w.menu[0].2, Action::Befriend);
        // frame close button
        let (hit, _) = w.event(&mut gui, &Event::CloseRequested { window: win }, &soc, &lft, &tx, &[]);
        assert!(hit && w.friends.is_none() && w.closed == ["friends_window"]);
    }

    #[test]
    fn invite_dialog_answers() {
        let Some((mut gui, db)) = rig() else { return };
        let tx = Texts(&db);
        let (soc, lft) = (Social::default(), Lft::default());
        let mut w = SocialWin::new((1280, 800));
        let cws = [("Window1".to_string(), "Default Window".to_string()), ("Window2".to_string(), "Combat".to_string())];
        w.invite_dialog(&mut gui, 5, "Bob", &tx, &cws);
        let dw = w.dialogs[0].win;
        assert!(gui.has_view(dw, "cw_0") && gui.has_view(dw, "cw_1") && gui.checked(dw, "cw_0") && !gui.checked(dw, "cw_1"));
        // ticking the second window's box through a real click
        click(&mut gui, dw, "cw_1");
        assert!(gui.checked(dw, "cw_1"));
        assert!(gui.text(dw, "text").starts_with("You were invited to a private chat group by Bob"));
        assert_eq!((tx.msgbox("MsgBox_Yes"), tx.msgbox("MsgBox_No")), ("Yes".to_string(), "No".to_string()));
        let mut reqs = vec![];
        for e in click(&mut gui, dw, "btn1") {
            reqs.extend(w.event(&mut gui, &e, &soc, &lft, &tx, &[]).1);
        }
        assert_eq!(reqs, [Req::Answer { id: 5, accept: false, windows: vec!["Window1".into(), "Window2".into()] }]);
        w.invite_dialog(&mut gui, 5, "Bob", &tx, &cws);
        let dw = w.dialogs[0].win;
        let (_, r) = w.event(&mut gui, &Event::EnterPressed { window: dw, view: "btn0".into() }, &soc, &lft, &tx, &[]);
        assert_eq!(r, [Req::Answer { id: 5, accept: true, windows: vec!["Window1".into()] }]);
    }

    #[test]
    fn tell_window_sends_and_shows_lines() {
        let Some((mut gui, db)) = rig() else { return };
        let tx = Texts(&db);
        let (soc, lft) = (Social::default(), Lft::default());
        let mut w = SocialWin::new((1280, 800));
        w.open_tell(&mut gui, 5, "Bob");
        w.tell_text(&mut gui, 5, "[Bob]: hi");
        let win = w.tells[0].win;
        assert!(gui.text(win, "text").contains("[Bob]: hi"));
        gui.set_text(win, "input", "hello  ");
        let (hit, r) = w.event(&mut gui, &Event::EnterPressed { window: win, view: "input".into() }, &soc, &lft, &tx, &[]);
        assert!(hit);
        assert_eq!(r, [Req::Tell { id: 5, name: "Bob".into(), text: "hello".into() }]);
        assert_eq!(gui.text(win, "input"), "");
        // opening it again reuses the window
        w.open_tell(&mut gui, 5, "Bob");
        assert_eq!(w.tells.len(), 1);
        let (_, r) = w.event(&mut gui, &Event::LinkClicked { window: win, view: "text".into(), href: "user://Eve".into() }, &soc, &lft, &tx, &[]);
        assert_eq!(r, [Req::OpenTell { id: 0, name: "Eve".into() }]);
        w.event(&mut gui, &Event::CloseRequested { window: win }, &soc, &lft, &tx, &[]);
        assert!(!w.tell_is_open(5));
    }

    #[test]
    fn lft_window_searches_and_lists() {
        let Some((mut gui, db)) = rig() else { return };
        let tx = Texts(&db);
        let soc = Social::default();
        let mut lft = Lft::default();
        let mut w = SocialWin::new((1280, 800));
        w.open_lft(&mut gui, &lft, &tx, false);
        let win = w.lft.as_ref().unwrap().win;
        // the client's own LFTView.xml
        for v in ["Side", "Location", "Profession", "Search", "Invite", "Tell", "LFT", "Description", "CandidateView"] {
            assert!(gui.has_view(win, v), "{v}");
        }
        assert_eq!((gui.text(win, "Side"), gui.text(win, "Location"), gui.text(win, "Profession")), ("any".into(), "Rubi-Ka".into(), "any".into()));
        // Search with the defaults: side any, profession any, location Rubi-Ka
        let mut reqs = vec![];
        for e in click(&mut gui, win, "Search") {
            reqs.extend(w.event(&mut gui, &e, &soc, &lft, &tx, &[]).1);
        }
        assert_eq!(reqs, [Req::LftSearch { side: 7, profession: 0x10, location: 2 }]);
        // two candidates arrive; the window shows name / level / side / profession / playfield / description
        for (id, name) in [(5, "Bob"), (6, "Eve")] {
            lft.on_reply(&LftReply { status: 0, id, name: name.into(), level: 100 + id, playfield: 4001, side: 2, profession: 6, description: "need heals".into() });
        }
        w.lft_rows(&mut gui, &lft, &tx);
        assert!(gui.has_view(win, "cand_0_0") && gui.has_view(win, "cand_1_5") && !gui.has_view(win, "cand_2_0"));
        assert_eq!(lft_cells(&lft.results[1], &tx, &[(4001, "Newland".to_string())].into())[..], ["Eve", "106", "omni", "Adventurer", "Newland", "need heals"]);
        assert_eq!(lft_cells(&lft.results[0], &tx, &Default::default())[4], "Not found");
        // selecting a row enables Tell / Invite for it
        let mut reqs = vec![];
        for e in click(&mut gui, win, "cand_1_0") {
            reqs.extend(w.event(&mut gui, &e, &soc, &lft, &tx, &[]).1);
        }
        for e in click(&mut gui, win, "Tell") {
            reqs.extend(w.event(&mut gui, &e, &soc, &lft, &tx, &[]).1);
        }
        for e in click(&mut gui, win, "Invite") {
            reqs.extend(w.event(&mut gui, &e, &soc, &lft, &tx, &[]).1);
        }
        assert_eq!(reqs, [Req::OpenTell { id: 6, name: "Eve".into() }, Req::LftInvite { id: 6, name: "Eve".into() }]);
        // the check box toggles on a click and reports the team description
        gui.set_text(win, "Description", "raid ubs");
        let mut reqs = vec![];
        for e in click(&mut gui, win, "LFT") {
            reqs.extend(w.event(&mut gui, &e, &soc, &lft, &tx, &[]).1);
        }
        assert_eq!(reqs, [Req::LftSet { on: true, description: "raid ubs".into() }]);
        assert!(gui.checked(win, "LFT"));
        // closing saves the config; reopening restores the description
        w.close_lft(&mut gui, &tx);
        let mut lft2 = Lft::default();
        lft2.set(true, "");
        w.open_lft(&mut gui, &lft2, &tx, true);
        let win = w.lft.as_ref().unwrap().win;
        assert_eq!(gui.text(win, "Description"), "raid ubs");
        assert!(gui.checked(win, "LFT") && !gui.is_enabled(win, "Invite"));
    }

    /// Offscreen render: `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac social_win_shot -- --nocapture` -> `social.png`.
    #[test]
    fn social_win_shot() {
        let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(PathBuf::from) else { return };
        let Some((gui, db)) = rig() else { return };
        let tx = Texts(&db);
        let mut gui = gui;
        let mut soc = filled();
        soc.nodes.get_mut(&6).unwrap().unread = 2;
        let mut lft = Lft::default();
        for (id, name, lvl, pf, side, prof) in [(5, "Bob", 187, 4001, 2, 6), (6, "Eve", 42, 4005, 1, 3)] {
            lft.on_reply(&LftReply { status: 0, id, name: name.into(), level: lvl, playfield: pf, side, profession: prof, description: "need heals for ubs".into() });
        }
        let mut w = SocialWin::new((1280, 828));
        w.set_friends(&mut gui, true, &soc, &tx, &["Default Window".into(), "Combat".into()]);
        w.open_lft(&mut gui, &lft, &tx, false);
        let fw = w.friends.as_ref().unwrap().win;
        gui.set_window_pos(fw, (20, 200));
        w.open_tell(&mut gui, 5, "Bob");
        w.tell_text(&mut gui, 5, &social::tell_prefix("Bob", false).replace("]: ", "]: hello there"));
        w.invite_dialog(&mut gui, 6, "Eve", &tx, &[("Window1".into(), "Default Window".into()), ("Window2".into(), "Combat".into())]);
        struct Fe(Gui);
        impl ao_render::Frontend for Fe {
            fn gui(&self) -> &Gui {
                &self.0
            }
            fn input(&mut self, ev: InputEvent, _: &mut ao_render::Host) {
                self.0.input(ev);
            }
            fn frame(&mut self, dt: f32, _: (u32, u32), _: &mut ao_render::Host) -> ao_gui::DrawList {
                self.0.frame(dt)
            }
        }
        let mut fe = Fe(gui);
        let mut o = ao_render::Offscreen::new(&fe, (1280, 828)).unwrap();
        let list = o.frame(&mut fe, 0.016);
        std::fs::create_dir_all(&out).unwrap();
        o.png(&fe, &list, &out.join("social.png")).unwrap();
        eprintln!("wrote social.png");
    }
}
