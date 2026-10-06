//! `team_view` (`TeamView_c`, GUI.dll `FUN_100785f0`; `TeamViewModule_c` 0x1007b9be): the Team window, Ctrl+5 / RightMenu entry `team_view`.
//! Evidence, addresses and what is unresolved: docs/gui.md §11.13.

use super::zone::Zone;
use ao_formats::screens::TextDb;
use ao_gui::{CanvasItem, Event, Gui, MenuItem, MouseButton, ViewHandle, WindowId, WindowSize};
use ao_net::frame::Frame;
use ao_net::msg::Identity;
use ao_net::n3::{self, team, world::World, N3};
use std::path::Path;

/// The window shows six `TeamBar_c` rows (`FUN_100785f0` loop `< 6`, `FUN_10077bf2`).
const ROWS: usize = 6;
/// Row height of a `TeamBar_c`, inclusive pixels (UNRESOLVED guess: two 5 px bars + borders; the original size was not decoded).
const ROW_H: u32 = 16;
/// `SetDefaultColor`: leader `0x11eeaa`; otherwise `(!inTree - 1 & 0x777777) + 0x888888` = white when the character is known, grey when not (`FUN_100775ac`).
const LEADER: u32 = 0x11eeaa;
const KNOWN: u32 = 0xffffff;
const UNKNOWN: u32 = 0x888888;
/// Target highlight of a row: `ViewSurface_c::SetColor(0x88aadd)` (`FUN_10078cad`), left edge 20 px (`_DAT_101b4e08`).
const HIGHLIGHT: u32 = 0x88aadd;
const HIGHLIGHT_LEFT: f32 = 20.0;
/// Popup menu ids (`Event::MenuPicked`): `BASE | action << 4 | row`.
const MENU_BASE: u32 = 0x7e00_0000;
const MENU_INFO: u32 = 1;
const MENU_LEADER: u32 = 2;
const MENU_KICK: u32 = 3;
/// `DockArea1.xml` (`docked_view_identities "team_view"`): saved `WindowFrame` Rect(1836,893,2030,1050) of the template.
const DOCK_FALLBACK: (i32, i32, u32, u32) = (1836, 893, 195, 158);

/// One entry of the member list (`TeamMemberNode_c`: identity + name) with what the bars read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Member {
    pub id: Identity,
    pub name: String,
    pub level: u16,
    /// `(current, max)` from `TeamMemberInfoIIR_t`.
    pub health: (i32, i32),
    pub nano: (i32, i32),
}

/// The client's team data (`TeamViewModule_c` + the own character's `TeamManager`), built from the server messages.
#[derive(Debug, Default)]
pub(super) struct TeamState {
    /// The own identity `{0xC350, char id}`.
    pub own: Identity,
    pub team: Option<Identity>,
    pub members: Vec<Member>,
    pub leader: Option<Identity>,
    /// A pending `TeamInviteIIR_t` (`flag == 0`): who asked and under which name.
    pub invite: Option<(Identity, String)>,
    /// System lines the window module would print (category 0x2710 texts): `(key, argument)`.
    pub notices: Vec<(&'static str, String)>,
}

impl TeamState {
    pub(super) fn new(char_id: i32) -> Self {
        Self { own: Identity { kind: n3::outgoing::DYNEL_CHAR, instance: char_id }, ..Self::default() }
    }

    pub(super) fn in_team(&self) -> bool {
        self.team.is_some()
    }

    /// `N3Msg_IsTeamLeader(own)`.
    pub(super) fn is_leader(&self) -> bool {
        self.in_team() && self.leader == Some(self.own)
    }

    pub(super) fn is_member(&self, id: Identity) -> bool {
        self.members.iter().any(|m| m.id == id)
    }

    /// `TeamViewModule_c::GetTeamLevel`: UNRESOLVED GUESS, the highest member level. The module stores the value of its team event `0x10` (`identity.instance`,
    /// `FUN_1007a5f2`), whose emitter in Gamecode was not found; no capture has a team.
    pub(super) fn level(&self) -> i32 {
        self.members.iter().map(|m| i32::from(m.level)).max().unwrap_or(0)
    }

    fn clear(&mut self) {
        self.team = None;
        self.members.clear();
        self.leader = None;
    }

    /// One zone N3 message (`FUN_1007a033` / `FUN_1007a294` / `FUN_1005d0d8` cases 4, 8, 9 / `FUN_100a3f26`).
    pub(super) fn on_message(&mut self, m: &n3::Message) {
        let N3::Unknown(body) = &m.body else {
            if let N3::World(World::CharacterAction(a)) = &m.body {
                self.on_action(m.header.target, a);
            }
            return;
        };
        match m.header.msg_type {
            team::TEAM_MEMBER => match team::parse_team_member(body) {
                Ok(t) => self.on_member(&t),
                Err(e) => eprintln!("team: TeamMember: {e:#}"),
            },
            team::TEAM_MEMBER_INFO => match team::parse_team_member_info(body) {
                Ok(i) => {
                    if let Some(x) = self.members.iter_mut().find(|x| x.id == i.member) {
                        x.health = (i.health, i.max_health);
                        x.nano = (i.nano, i.max_nano);
                    }
                }
                Err(e) => eprintln!("team: TeamMemberInfo: {e:#}"),
            },
            team::TEAM_INVITE => match team::parse_team_invite(body) {
                // `FUN_100a3f26`: acts for flag 0 only, and not when the own character already has a team (`FUN_100657d1`)
                Ok(i) if i.flag == 0 && !self.in_team() => self.invite = Some((i.from, i.name)),
                Ok(i) if i.flag == 0 => self.notices.push(("WouldYouLike2TeamWithX", i.name)),
                Ok(_) => {}
                Err(e) => eprintln!("team: TeamInvite: {e:#}"),
            },
            _ => {}
        }
    }

    /// `FUN_1007a033`: a member record. `team.instance == 0` clears that dynel's team (`FUN_10065745`); otherwise the entry is created / replaced
    /// (`FUN_10065d09`). The own record carries the own team; the leader defaults to the first member of a new team (**UNRESOLVED GUESS**: the
    /// leader comes with action `0x23`, see [`TeamState::on_action`]).
    fn on_member(&mut self, t: &team::TeamMember) {
        if t.team.instance == 0 {
            self.remove(t.member);
            return;
        }
        if t.member == self.own {
            self.team = Some(t.team);
            self.invite = None;
        }
        let old = self.members.iter().position(|m| m.id == t.member);
        let m = Member { id: t.member, name: t.name.clone(), level: t.level, health: (0, 0), nano: (0, 0) };
        match old {
            Some(i) => {
                let keep = self.members[i].clone();
                self.members[i] = Member { health: keep.health, nano: keep.nano, ..m };
            }
            None => {
                if self.leader.is_none() {
                    self.leader = Some(t.member);
                }
                self.members.push(m);
            }
        }
    }

    fn remove(&mut self, id: Identity) {
        if id == self.own {
            self.clear();
            return;
        }
        if let Some(i) = self.members.iter().position(|m| m.id == id) {
            let m = self.members.remove(i);
            self.notices.push(("XleftTeam", m.name));
        }
        if self.leader == Some(id) {
            self.leader = None;
        }
    }

    /// `FUN_1005d0d8`: `0x20` (case 8) removes `identity_a`; `0x18` (case 4) clears the team of the dynel it is relayed for; `0x23` (case 9) stores
    /// `identity_a` as the leader.
    fn on_action(&mut self, target: Identity, a: &n3::world::CharacterAction) {
        match a.action {
            team::action::MEMBER_LEFT => self.remove(a.identity_a),
            team::action::LEAVE => self.remove(target),
            team::action::LEADER if self.in_team() => self.leader = Some(a.identity_a),
            _ => {}
        }
    }
}

/// One `TeamBar_c` row of the window.
struct Row {
    handle: ViewHandle,
    /// The member shown (`+0x128/+0x12c`), `None` = empty row.
    id: Option<Identity>,
}

struct Win {
    window: WindowId,
    rows: Vec<Row>,
    /// Which button set is built: `(in team, leader)`.
    buttons: (bool, bool),
}

pub(super) struct HudTeam {
    pub(super) state: TeamState,
    win: Option<Win>,
    texts: Option<TextDb>,
    /// Outer frame (x, y, width, height) from `DockArea1.xml` `WindowFrame` (inclusive `Rect`).
    frame: (i32, i32, u32, u32),
    screen: (u32, u32),
    /// Zone frames to send (leave, kick, transfer, join request, request reply).
    pub(super) outbox: Vec<Frame>,
    /// Set when the user closes the frame: the caller clears the menu state.
    closed: bool,
    /// The member the user clicked: becomes the target (`AFCM::Send(0x1e, 0x127, id)`, `TargetMember`).
    select: Option<i32>,
    dialog: super::hud_dialog::Dialogs<()>,
    /// Text lines for the System window of the chat (`GlobalSignals+0x17c`).
    pub(super) lines: Vec<String>,
    /// `InfoViewModule_c::ShowURL` requests ("Info on <name>").
    pub(super) info_urls: Vec<String>,
}

/// `Rect(l,t,r,b)` of the first `WindowFrame` in a `prefs/NewChar` archive.
fn dock_frame(dir: &Path, file: &str) -> Option<(i32, i32, u32, u32)> {
    let t = std::fs::read_to_string(dir.join("prefs/NewChar/DockAreas").join(file)).ok()?;
    let root = ao_gui::xml::parse(&t).ok()?;
    fn find(e: &ao_gui::xml::Element) -> Option<String> {
        e.children.iter().find_map(|c| if c.name == "Rect" && c.attr("name") == Some("WindowFrame") { c.attr("value").map(String::from) } else { find(c) })
    }
    let v = find(&root)?;
    let n: Vec<f32> = v.trim_start_matches("Rect(").trim_end_matches(')').split(',').filter_map(|p| p.trim().parse().ok()).collect();
    (n.len() == 4).then(|| (n[0] as i32, n[1] as i32, (n[2] - n[0]) as u32 + 1, (n[3] - n[1]) as u32 + 1))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `%u` / `%s` of an `LDBformat` text, one argument at a time.
fn fmt(text: &str, args: &[&str]) -> String {
    let mut out = String::new();
    let mut it = args.iter();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' && matches!(chars.peek(), Some('s' | 'u' | 'd')) {
            chars.next();
            out += it.next().copied().unwrap_or("");
        } else {
            out.push(c);
        }
    }
    out
}

impl HudTeam {
    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> Self {
        Self {
            state: TeamState::new(0),
            win: None,
            texts: TextDb::load(dir).ok(),
            frame: dock_frame(dir, "DockArea1.xml").unwrap_or(DOCK_FALLBACK),
            screen,
            outbox: vec![],
            closed: false,
            select: None,
            dialog: Default::default(),
            lines: vec![],
            info_urls: vec![],
        }
    }

    pub(super) fn set_screen(&mut self, screen: (u32, u32)) {
        self.screen = screen;
    }

    fn text(&self, key: &str) -> String {
        self.texts.as_ref().and_then(|t| t.by_key(10000, key)).unwrap_or_else(|| key.to_string())
    }

    pub(super) fn take_closed(&mut self) -> bool {
        std::mem::take(&mut self.closed)
    }

    pub(super) fn on_frame(&mut self, f: &Frame, own: i32) {
        if f.ptype != ao_net::frame::PT_N3 {
            return;
        }
        if self.state.own.instance != own {
            self.state = TeamState::new(own);
        }
        if let Ok(m) = n3::decode(f) {
            self.state.on_message(&m);
        }
        let texts = self.texts.as_ref();
        for (key, arg) in std::mem::take(&mut self.state.notices) {
            if let Some(t) = texts.and_then(|t| t.by_key(10000, key)) {
                self.lines.push(fmt(&t, &[&arg]));
            }
        }
    }

    pub(super) fn open(&mut self, gui: &mut Gui) {
        if self.win.is_some() {
            return;
        }
        match self.build(gui) {
            Ok(w) => self.win = Some(w),
            Err(e) => eprintln!("hud: team_view: {e:#}"),
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui) {
        if let Some(w) = self.win.take() {
            gui.close_window(w.window);
        }
        gui.close_menu();
    }

    /// `TeamView_c` ctor `FUN_100785f0`: header `BorderView` (client borders 4, 2, 4, 2), the button row (borders 0, 5, 0, 5), six `TeamBar_c`s.
    fn build(&mut self, gui: &mut Gui) -> anyhow::Result<Win> {
        let xml = "<root><View view_layout=\"vertical\" name=\"team_root\">\
                   <BorderView name=\"header_border\"><TextView name=\"header\" layout_borders=\"Rect(4,2,4,2)\" max_size=\"Point(16000,-1)\"/></BorderView>\
                   <View name=\"buttons\" view_layout=\"horizontal\" layout_borders=\"Rect(0,5,0,5)\"/>\
                   <View name=\"rows\" view_layout=\"vertical\"/></View></root>";
        let title = self.text("Team");
        let (x, y, fw, fh) = self.frame;
        let window = gui.open_tabbed_window_xml("TeamView", &title, xml, (x, y), WindowSize::Preferred)?;
        let gfx = |id: u32| gui.gfx().name(ao_gui::GfxId(id)).unwrap_or_default().to_string();
        // `TeamBar_c` (`FUN_100791f7`): bars `PowerbarView_c(bounds, 0x1b, 0x1a, 0, 0, right)` (health) and `(0x1d, 0x1c)` (nano); separators `BitmapView_c(0xd6)`
        let (hp, hp_bg, nano, nano_bg, sep) = (gfx(0x1a), gfx(0x1b), gfx(0x1c), gfx(0x1d), gfx(0xd6));
        let mut rows = vec![];
        for i in 0..ROWS {
            let top = if i == 0 { format!("<BitmapView bitmap_id=\"{sep}\" max_size=\"Point(16000,-1)\"/>") } else { String::new() };
            let src = format!(
                "<root><View view_layout=\"vertical\" name=\"row{i}\">{top}\
                 <View view_layout=\"stacked\" min_size=\"Point(-1,{ROW_H})\" max_size=\"Point(16000,{ROW_H})\"><CanvasView name=\"hl{i}\"/>\
                 <View view_layout=\"horizontal\"><TextView name=\"idx{i}\" value=\"{n}\" layout_borders=\"Rect(3,2,0,2)\"/><HLayoutSpacer/>\
                 <TextView name=\"name{i}\" value=\"\" layout_borders=\"Rect(5,2,5,2)\"/><HLayoutSpacer/>\
                 <View view_layout=\"vertical\" layout_borders=\"Rect(2,2,2,2)\">\
                 <PowerBar name=\"hp{i}\" bg_gfx=\"{hp_bg}\" full_gfx=\"{hp}\" direction=\"right\"/>\
                 <PowerBar name=\"nano{i}\" bg_gfx=\"{nano_bg}\" full_gfx=\"{nano}\" direction=\"right\"/></View></View></View>\
                 <BitmapView bitmap_id=\"{sep}\" max_size=\"Point(16000,-1)\"/></View></root>",
                n = i + 1
            );
            rows.push(Row { handle: gui.add_view_xml(window, "rows", &format!("TeamBar{i}"), &src)?, id: None });
        }
        let mut w = Win { window, rows, buttons: (false, false) };
        self.build_buttons(gui, &mut w, true);
        // `LoadWndConfig` applies the saved `WindowFrame` (the template's size) and ends in `Window::MoveInsideScreen`: the origin comes from a bigger screen
        // The template's height (157) is the client's own default; a stacked row has no layout node, so the rows carry `min_size`
        // (UNRESOLVED: the original `TeamBar_c` height) and the window grows to hold six of them.
        gui.resize_window(w.window, WindowSize::Preferred);
        let fh = fh.max(gui.outer_size(w.window).1);
        let (px, py) = (x.min(self.screen.0.saturating_sub(fw) as i32).max(0), y.min(self.screen.1.saturating_sub(fh) as i32).max(0));
        gui.set_window_outer_frame(w.window, (px, py, fw, fh));
        gui.set_window_size_limits(w.window, (fw.saturating_sub(10), fh.saturating_sub(31)), (0, 0));
        Ok(w)
    }

    /// `FUN_10077fb3`: not in a team = spacer + `Recruit`; in a team = spacer, `Leave` (borders 5, 0, 5, 0) and for the leader `Recruit` (0, 0, 5, 0).
    fn build_buttons(&self, gui: &mut Gui, w: &mut Win, force: bool) {
        let want = (self.state.in_team(), self.state.is_leader());
        if !force && w.buttons == want {
            return;
        }
        w.buttons = want;
        gui.remove_children(w.window, "buttons");
        let (recruit, leave) = (esc(&self.text("Recruit")), esc(&self.text("Leave")));
        let mut src = String::from("<root><View view_layout=\"horizontal\" name=\"button_row\"><HLayoutSpacer name=\"spacer\"/>");
        if !want.0 {
            src += &format!("<Button name=\"recruit\" label=\"{recruit}\"/>");
        } else {
            src += &format!("<Button name=\"leave\" label=\"{leave}\" layout_borders=\"Rect(5,0,5,0)\"/>");
            if want.1 {
                src += &format!("<Button name=\"recruit\" label=\"{recruit}\" layout_borders=\"Rect(0,0,5,0)\"/>");
            }
        }
        src += "</View></root>";
        if let Err(e) = gui.add_view_xml(w.window, "buttons", "TeamButtons", &src) {
            eprintln!("hud: team buttons: {e:#}");
        }
        gui.relayout_window(w.window);
    }

    /// Per-frame refresh (`FUN_100779db` header text, `FUN_10077bf2` rows, `FUN_100775ac` bars, `FUN_1007791c` Recruit state).
    pub(super) fn update(&mut self, gui: &mut Gui, zone: &mut Zone, _dt: f32) {
        if self.state.own.instance != zone.char_id as i32 {
            self.state = TeamState::new(zone.char_id as i32);
        }
        if let Some(id) = self.select.take() {
            if zone.dynels.contains_key(&id) || id == zone.char_id as i32 {
                zone.set_target(Some(ao_net::msg::Identity { kind: 50000, instance: id }));
            }
        }
        let Some(mut w) = self.win.take() else { return };
        self.build_buttons(gui, &mut w, false);
        let st = &self.state;
        let header = if st.in_team() {
            let side = self.texts.as_ref().and_then(|t| t.by_id(2005, zone.stat(ao_formats::stats::SIDE).unwrap_or(0).max(0) as u32)).unwrap_or_default();
            fmt(&self.text("TeamLvlTeamSide"), &[&st.level().to_string(), &side])
        } else {
            self.text("NoTeam")
        };
        gui.set_text(w.window, "header", &header);
        let own = zone.char_id as i32;
        for (i, row) in w.rows.iter_mut().enumerate() {
            let m = st.members.get(i);
            row.id = m.map(|m| m.id);
            gui.set_text(w.window, &format!("name{i}"), m.map_or("", |m| m.name.as_str()));
            let color = match m {
                None => KNOWN,
                Some(m) if st.leader == Some(m.id) => LEADER,
                Some(m) if zone.dynels.contains_key(&m.id.instance) || m.id.instance == own => KNOWN,
                Some(_) => UNKNOWN,
            };
            gui.set_color_in(row.handle, &format!("name{i}"), color);
            // bars: `GetSkill(0x1b) / GetSkill(1)` and `(0xd6) / (0xdd)`; empty rows 0 (`FUN_100775ac`, `FUN_10079b30`)
            let frac = |c: i32, m: i32| if m > 0 { (c as f32 / m as f32).clamp(0.0, 1.0) } else { 0.0 };
            let (hp, nano) = match m {
                None => (0.0, 0.0),
                Some(m) if m.id.instance == own => (frac(zone.skill_value(27).unwrap_or(0), zone.skill_value(1).unwrap_or(0)), frac(zone.skill_value(214).unwrap_or(0), zone.skill_value(221).unwrap_or(0))),
                Some(m) => {
                    let known = zone.dynels.get(&m.id.instance).filter(|d| d.max_health > 0).map(|d| (d.health, d.max_health));
                    let h = known.unwrap_or(m.health);
                    (frac(h.0, h.1), frac(m.nano.0, m.nano.1))
                }
            };
            gui.set_progress(w.window, &format!("hp{i}"), hp);
            gui.set_progress(w.window, &format!("nano{i}"), nano);
            // the selected member's highlight (`FUN_10078cad`)
            let selected = m.is_some_and(|m| zone.target == Some(m.id.instance));
            let (cw, ch) = gui.canvas_size(w.window, &format!("hl{i}"));
            let bars = gui.view_frame(w.window, &format!("hp{i}")).map_or(0.0, |r| r.r - r.l);
            let items = if selected { vec![CanvasItem::Solid { dst: [HIGHLIGHT_LEFT, 0.0, (cw as f32 - bars - 4.0).max(HIGHLIGHT_LEFT), ch as f32], color: HIGHLIGHT, alpha: 1.0 }] } else { vec![] };
            gui.set_canvas(w.window, &format!("hl{i}"), items);
        }
        // `FUN_1007791c`: Recruit is enabled for a selected character that is not the own one, not an NPC and not yet a member
        let target = zone.target.and_then(|t| zone.dynels.get(&t).map(|d| (t, d.npc)));
        let can = target.is_some_and(|(t, npc)| t != own && !npc && !st.is_member(Identity { kind: n3::outgoing::DYNEL_CHAR, instance: t }));
        gui.set_enabled(w.window, "recruit", can);
        self.win = Some(w);
        self.invite_dialog(gui);
    }

    /// `SlotJoinTeamRequest` (`FUN_1007a1f6`): outside a team the invitation is a Yes / No `DialogBox_c` with "Want to join %s's team?".
    fn invite_dialog(&mut self, gui: &mut Gui) {
        if self.dialog.is_open() {
            return;
        }
        let Some((_, name)) = self.state.invite.clone() else { return };
        let body = fmt(&self.text("Want2JoinXsTeam"), &[&name]);
        let buttons = [self.text("MsgBox_Yes"), self.text("MsgBox_No")];
        self.dialog.go(gui, self.screen, (), &body, &buttons);
    }

    fn send(&mut self, payload: Vec<u8>, zone: &Zone) {
        self.outbox.push(n3::outgoing::n3_frame(0, zone.char_id, payload));
    }

    /// `true` when the event belonged to the window / its dialog or menu.
    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone) -> bool {
        let own = zone.char_id as i32;
        let (mine, ans) = self.dialog.event(gui, ev);
        if mine {
            if let (Some((_, button)), Some((from, _))) = (ans, self.state.invite.take()) {
                // `N3Msg_RequestReply(from, accept)`: Yes is button 0 (action 0x1c)
                let accept = button == 0;
                let p = team::request_reply(own, from, accept);
                self.send(p, zone);
            }
            return true;
        }
        if let Event::MenuPicked { id } = ev {
            if *id & 0xff00_0000 != MENU_BASE {
                return false;
            }
            let (action, row) = ((*id >> 4) & 0xff, (*id & 0xf) as usize);
            let Some(m) = self.state.members.get(row).cloned() else { return true };
            match action {
                // `FUN_10079a2a`: `InfoViewModule_c::ShowURL("charid://kind/instance")`
                MENU_INFO => self.info_urls.push(format!("charid://{}/{}", m.id.kind, m.id.instance)),
                MENU_LEADER => self.send(team::transfer_team_leadership(own, m.id), zone),
                MENU_KICK => self.send(team::kick_team_member(own, m.id), zone),
                _ => {}
            }
            return true;
        }
        let Some(w) = &self.win else { return false };
        match ev {
            Event::CloseRequested { window } if *window == w.window => {
                self.close(gui);
                self.closed = true;
                true
            }
            Event::Clicked { window, view, .. } if *window == w.window => {
                match view.as_str() {
                    // `N3Msg_LeaveTeam()`
                    "leave" => self.send(team::leave_team(own), zone),
                    // `FUN_10077c8e`
                    "recruit" => match zone.target {
                        None => self.notice("MustSelectChar2Join"),
                        Some(t) if t == own => self.notice("CantInviteSelf"),
                        Some(t) => {
                            let id = Identity { kind: n3::outgoing::DYNEL_CHAR, instance: t };
                            let name = zone.dynels.get(&t).map(|d| d.name.clone()).unwrap_or_default();
                            self.send(team::team_join_request(own, id, false), zone);
                            if let Some(t) = self.texts.as_ref().and_then(|t| t.by_key(100, "JoinTeamRequestSentTo")) {
                                self.lines.push(format!("{t}{name}"));
                            }
                        }
                    },
                    _ => {}
                }
                true
            }
            // `FUN_10079c16`: left click = `TargetMember`, right click = the popup menu
            Event::CanvasPress { window, view, button, x, y, .. } if *window == w.window && view.starts_with("hl") => {
                let Some(row) = view[2..].parse::<usize>().ok().filter(|r| *r < ROWS) else { return true };
                let Some(id) = w.rows[row].id else { return true };
                match button {
                    MouseButton::Left => self.select = Some(id.instance),
                    MouseButton::Right => self.open_menu(gui, row, (*x, *y)),
                    _ => {}
                }
                true
            }
            _ => false,
        }
    }

    fn notice(&mut self, key: &str) {
        let t = self.text(key);
        self.lines.push(t);
    }

    /// The right-click menu of `FUN_10079c16`: "Info on <name>", and for the leader (on another member) "Make <name> leader" and "Kick <name>".
    fn open_menu(&mut self, gui: &mut Gui, row: usize, at: (f32, f32)) {
        let Some(w) = &self.win else { return };
        let Some(m) = self.state.members.get(row) else { return };
        let id = |a: u32| MENU_BASE | a << 4 | row as u32;
        let mut items = vec![MenuItem::entry(id(MENU_INFO), &format!("{}{}", self.text("InfoOn"), m.name))];
        if m.id != self.state.own && self.state.is_leader() {
            items.push(MenuItem::entry(id(MENU_LEADER), &fmt(&self.text("MakeXLeader"), &[&m.name])));
            items.push(MenuItem::entry(id(MENU_KICK), &format!("{}{}", self.text("Kick_"), m.name)));
        }
        let o = gui.view_rect(w.window, &format!("hl{row}")).map_or((0.0, 0.0), |r| (r.l, r.t));
        gui.open_menu(((o.0 + at.0) as i32, (o.1 + at.1) as i32), (self.screen.0 as i32, self.screen.1 as i32), items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::{outgoing::n3_frame, team::*};

    const ME: Identity = Identity { kind: 50000, instance: 7 };
    const OTHER: Identity = Identity { kind: 50000, instance: 9 };

    fn feed(h: &mut HudTeam, p: Vec<u8>) {
        h.on_frame(&n3_frame(0, 7, p), 7);
    }

    fn member(id: Identity, name: &str) -> Vec<u8> {
        team_member(ME, &TeamMember { member: id, team: Identity { kind: TEAM_KIND, instance: 1 }, sub_team: 0, profession: 1, level: 30, name: name.into() })
    }

    #[test]
    fn join_info_leader_and_leave() {
        let mut h = HudTeam::new(&ao_gui::client_dir(), (1024, 768));
        feed(&mut h, member(ME, "Me"));
        feed(&mut h, member(OTHER, "Other"));
        assert!(h.state.in_team() && h.state.is_leader() && h.state.members.len() == 2);
        feed(&mut h, team_member_info(ME, &TeamMemberInfo { member: OTHER, nano: 1, max_nano: 2, max_health: 10, health: 5 }));
        assert_eq!(h.state.members[1].health, (5, 10));
        // leader action 0x23 hands the leadership over
        let a = ao_net::n3::action::character_action(7, &ao_net::n3::action::simple(action::LEADER, OTHER, Identity::default()));
        feed(&mut h, a);
        assert!(!h.state.is_leader());
        // action 0x20 removes a member and prints "<name> left your team."
        let a = ao_net::n3::action::character_action(7, &ao_net::n3::action::simple(action::MEMBER_LEFT, OTHER, Identity::default()));
        feed(&mut h, a);
        assert_eq!(h.state.members.len(), 1);
        // own team record with instance 0 clears everything
        feed(&mut h, team_member(ME, &TeamMember { member: ME, team: Identity::default(), sub_team: 0, profession: 1, level: 30, name: "Me".into() }));
        assert!(!h.state.in_team());
    }

    /// The DockArea1 origin (1836, 893) is off a 1280x800 screen: the window opens inside it (`MoveInsideScreen`).
    #[test]
    fn opens_inside_the_screen() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui").exists() {
            return;
        }
        let mut gui = ao_gui::Gui::new(&dir, None).unwrap();
        let mut h = HudTeam::new(&dir, (1280, 800));
        h.open(&mut gui);
        let w = h.win.as_ref().unwrap().window;
        let (x, y, ow, oh) = gui.window_outer_frame(w).unwrap();
        assert!(x >= 0 && y >= 0 && x + ow as i32 <= 1280 && y + oh as i32 <= 800, "{x},{y} {ow}x{oh}");
        let mut rollup = crate::play::hud_rollup::Rollup::new(&dir, (1280, 800));
        rollup.register_window(&mut gui, "team_view", w).unwrap();
        for (width, height) in [(ow, oh), (ow + 80, oh + 60)] {
            gui.set_window_outer_frame(w, (x, y, width, height));
            let body = gui.view_rect(w, "body").unwrap();
            assert_eq!(body.width() + 1.0, gui.window_size(w).0 as f32);
            assert_eq!(body.height() + 1.0, gui.window_size(w).1 as f32);
            for i in 0..ROWS {
                for part in ["idx", "name", "hp", "nano"] {
                    let row = gui.view_rect(w, &format!("{part}{i}")).unwrap();
                    assert!(row.l >= body.l && row.r <= body.r, "{part}{i}: {row:?} outside {body:?}");
                }
            }
        }
    }

    #[test]
    fn invitation_outside_a_team() {
        let mut h = HudTeam::new(&ao_gui::client_dir(), (1024, 768));
        feed(&mut h, team_invite(ME, &TeamInvite { from: OTHER, flag: 0, name: "Inviter".into(), refusal: None }));
        assert_eq!(h.state.invite, Some((OTHER, "Inviter".into())));
    }
}
