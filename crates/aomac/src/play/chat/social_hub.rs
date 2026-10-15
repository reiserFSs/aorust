//! Hub glue of the social layer (`ChatGUIModule_c::HandlePPJActions` branches for buddy list / private groups / LFT, the tell windows and
//! the Friends / Team Search windows): routes [`super::social`] decisions into the chat server, the chat windows and
//! [`super::social_win`]. Evidence: docs/chat/social.md.

use super::line::{ChatKind, ChatLine};
use super::social::{self, SocialOut};
use super::social_win::{Req, Texts};
use super::Chat;
use crate::play::zone::Zone;
use ao_formats::screens::TextDb;
use ao_gui::{Event, Gui};
use ao_net::chat::{ChatCmd, ChatEvent};

/// Lines kept per tell partner while its window is closed (the window keeps 100 as well).
const TELL_LOG: usize = 100;

impl Chat {
    /// Runs `f` with the social environment (names from the chat server's tables, the text db, the ignore list).
    fn with_social<R>(&mut self, texts: &TextDb, f: impl FnOnce(&mut social::Social, &social::Env) -> R) -> R {
        let names = |id: u32| self.net.name_of(id).unwrap_or_default().to_owned();
        let text = |k: &str| texts.by_key(10001, k).unwrap_or_default();
        let ign = |id: u32| self.ignored.contains(&id);
        let env = social::Env { own_id: self.own_id, connected: self.net.logged_in(), name_of: &names, text: &text, invite_pref: self.invite_action, ignored: &ign };
        f(&mut self.social, &env)
    }

    /// A buddy / private group / LFT event of the chat server.
    pub(super) fn social_event(&mut self, gui: &mut Gui, e: ChatEvent, texts: &TextDb) {
        let busy = self.social.lft.busy;
        if let Some(outs) = self.with_social(texts, |s, env| s.on_event(&e, env)) {
            self.apply_social(gui, outs, texts);
        }
        if busy != self.social.lft.busy {
            self.swin.lft_busy_changed(gui, self.social.lft.busy);
        }
        if matches!(e, ChatEvent::LftReply(_)) {
            let lft = self.social.lft.clone();
            self.swin.lft_rows(gui, &lft, &Texts(texts));
        }
    }

    fn apply_social(&mut self, gui: &mut Gui, outs: Vec<SocialOut>, texts: &TextDb) {
        let mut refresh = false;
        for o in outs {
            match o {
                SocialOut::System(t) => self.line(gui, ChatLine::new(ChatKind::System, t)),
                SocialOut::Send(c) => self.net.send(c),
                SocialOut::Msg(m) => {
                    if m.group >> 32 != 0 && !m.group_name.is_empty() && !self.net.groups.contains_key(&m.group) {
                        // a message of a group the windows may not know yet (its PRIVGRP_JOINED came first in the original)
                        self.net.set_group(m.group, Some(&m.group_name));
                    }
                    self.msg(gui, m)
                }
                SocialOut::GroupAdd { group, name } => {
                    self.net.set_group(group, Some(&name));
                    self.group(group, name, 0);
                    // the invite dialog's choice: every chat window shows the group iff its box was ticked (`FUN_1009d129` / `FUN_1009ce7b`)
                    if let (Some(sel), Some(w)) = (self.pg_windows.remove(&(group as u32)), self.win.as_mut()) {
                        for (wn, _) in w.window_list() {
                            w.subscribe_group(&wn, group, sel.contains(&wn));
                        }
                    }
                }
                SocialOut::GroupRemove { group } => {
                    self.net.set_group(group, None);
                    self.ungroup(group);
                }
                SocialOut::InviteDialog { id, name } => {
                    let cws = self.window_list();
                    self.swin.invite_dialog(gui, id, &name, &Texts(texts), &cws)
                }
                SocialOut::FriendsChanged => refresh = true,
                SocialOut::TellText { id, html } => self.tell_line(gui, id, html),
            }
        }
        if refresh {
            self.refresh_friends(gui, texts);
        }
    }

    fn refresh_friends(&mut self, gui: &mut Gui, texts: &TextDb) {
        let names = self.window_names();
        self.swin.refresh_friends(gui, &self.social, &Texts(texts), &names);
    }

    /// Names of the chat windows (the "Chat Windows" folder of the Friends window).
    fn window_names(&self) -> Vec<String> {
        self.window_list().into_iter().map(|w| w.1).collect()
    }

    fn window_list(&self) -> Vec<(String, String)> {
        self.win.as_ref().map(|w| w.window_list()).unwrap_or_default()
    }

    /// A line of the tell partner's window (kept while it is closed).
    fn tell_line(&mut self, gui: &mut Gui, id: u32, html: String) {
        let log = self.tell_log.entry(id).or_default();
        log.push(html.clone());
        if log.len() > TELL_LOG {
            log.remove(0);
        }
        self.swin.tell_text(gui, id, &html);
    }

    /// `HandlePrivateMessage`: an incoming tell goes to the sender's tell window logic (node, unread count, temporary buddy).
    pub(super) fn social_tell_in(&mut self, gui: &mut Gui, id: u32, name: &str, text: &str, gm: bool, texts: &TextDb) {
        if id == 0 {
            return;
        }
        let outs = self.with_social(texts, |s, env| s.tell_in(id, name, text, gm, env));
        self.apply_social(gui, outs, texts);
        if id != self.own_id && (gm || !self.ignored.contains(&id)) {
            // GUI 1008792e: no sender/text payload, never a sent-tell echo.
            self.tip_events.push(("OnReceiveTellMessage", String::new()));
        }
    }

    /// `OpenTellWindow(name, id)` 0x10085df8.
    pub(super) fn open_tell_window(&mut self, gui: &mut Gui, id: u32, name: &str, texts: &TextDb) {
        let outs = self.with_social(texts, |s, env| s.open_tell(id, name, env));
        self.apply_social(gui, outs, texts);
        let fresh = !self.swin.tell_is_open(id);
        self.swin.open_tell(gui, id, name);
        if fresh {
            for h in self.tell_log.get(&id).cloned().unwrap_or_default() {
                self.swin.tell_text(gui, id, &h);
            }
        }
        self.refresh_friends(gui, texts);
    }

    /// By name (links, `/tell name`): the id is looked up first when unknown.
    pub(super) fn open_tell_named(&mut self, gui: &mut Gui, name: &str, texts: &TextDb) {
        // unknown name: `Out::OpenTell` follows with the lookup answer
        if let Some(id) = self.net.lookup_open(name) {
            self.open_tell_window(gui, id, name, texts);
        }
    }

    pub(super) fn social_open_tell_out(&mut self, gui: &mut Gui, id: u32, name: &str, texts: &TextDb) {
        if id == u32::MAX {
            self.line(gui, ChatLine::new(ChatKind::Error, format!("Unknown user {name}"))); // [GUESS] text, as the tell path
        } else {
            self.open_tell_window(gui, id, name, texts);
        }
    }

    /// Per frame: LFT search cooldown, closed tell windows.
    pub(super) fn social_update(&mut self, gui: &mut Gui, dt: f32) {
        self.social.lft.tick(dt);
        for id in self.swin.closed_tells(&self.social) {
            self.social.close_tell(id);
        }
        self.swin.update(gui, dt, &self.social);
    }

    /// The HUD's `friends_window` / `lft_window` dvalues (`SlotFriendWindowActivated`; the Team Search entry of the right menu).
    pub fn sync_windows(&mut self, gui: &mut Gui, friends: bool, lft: bool, esc_lft: bool, blocked: bool, texts: &TextDb) {
        if self.win.is_none() {
            return;
        }
        let names = self.window_names();
        self.swin.set_friends(gui, friends, &self.social, &Texts(texts), &names);
        if lft && self.swin.lft_window().is_none() {
            self.swin.open_lft(gui, &self.social.lft.clone(), &Texts(texts), blocked, esc_lft);
        } else if !lft && self.swin.lft_window().is_some() {
            self.swin.close_lft(gui, &Texts(texts));
        }
        if let Some(win) = self.swin.lft_window() { gui.set_enabled(win, "Invite", !blocked); }
    }

    /// Dvalues of social windows closed by the user since the last call (the HUD button pops out).
    pub fn take_closed_windows(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.swin.closed)
    }

    /// Live harness hook (`buddyadd=<name>`, `buddyrm=<name>`, `lftsearch=<side>:<profession>:<location>` with the dropdown item ids,
    /// 7 / 0x10 = any): the same `Req`s the Friends / Team Search windows emit. `false` = the name's id is not known yet (a 0x15 lookup was sent).
    #[cfg(test)]
    pub fn live_social(&mut self, gui: &mut Gui, step: &str, arg: &str, zone: &Zone, texts: &TextDb) -> bool {
        let req = match step {
            "buddyadd" | "buddyrm" => {
                let Some(id) = self.net.lookup_open(arg) else { return false };
                if step == "buddyadd" { Req::Befriend(id) } else { Req::RemoveFriend(id) }
            }
            _ => {
                let n: Vec<u32> = arg.split(':').map(|x| x.parse().expect("side:profession:location")).collect();
                Req::LftSearch { side: n[0], profession: n[1], location: n[2] }
            }
        };
        self.social_req(gui, req, zone, texts);
        true
    }

    #[cfg(test)]
    pub(super) fn social_ui_event_for_test(&mut self, gui: &mut Gui, ev: &Event, texts: &TextDb) -> bool {
        let zone = Zone::default();
        self.social_ui_event(gui, ev, &zone, texts)
    }

    /// Social window events; `true` if one of its windows (or the popup menu of the Friends window) handled it.
    pub(super) fn social_ui_event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone, texts: &TextDb) -> bool {
        let names = self.window_names();
        let lft = self.social.lft.clone();
        let (hit, reqs) = self.swin.event(gui, ev, &self.social, &lft, &Texts(texts), &names);
        for r in reqs {
            self.social_req(gui, r, zone, texts);
        }
        hit
    }

    /// `/ignore` target/id and resolved-name paths share the native toggle, feedback and Friends refresh (GUI 100ba9fc).
    pub(super) fn toggle_ignored(&mut self, gui: &mut Gui, id: u32, name: &str, texts: &TextDb) {
        let on = !self.ignored.remove(&id);
        if on { self.ignored.insert(id); }
        self.social.set_ignored(id, on, name);
        self.refresh_friends(gui, texts);
        let key = if on { "ChatCmdFeedback_Ignore_IgnoringCharacter" } else { "ChatCmdFeedback_Ignore_UnignoringCharacter" };
        let text = texts.by_key(10001, key).unwrap_or_default().replacen("%s", name, 1);
        self.line(gui, super::zonecmd::info_line(&text));
    }

    fn social_req(&mut self, gui: &mut Gui, r: Req, zone: &Zone, texts: &TextDb) {
        match r {
            Req::OpenTell { id: 0, name } => self.open_tell_named(gui, &name, texts),
            Req::OpenTell { id, name } => self.open_tell_window(gui, id, &name, texts),
            Req::Tell { id, name, text } => {
                self.net.tell(&name, &text);
                // outgoing tell echo in the partner's window: `ChatTellMsgToField` template "To [%s]: "
                let head = texts.by_key(10001, "ChatTellMsgToField").unwrap_or_else(|| "To [%s]: ".into()).replacen("%s", &name, 1);
                self.tell_line(gui, id, format!("<font color=ct_otell>{head}{text}</font>"));
            }
            Req::Befriend(id) => {
                let o = self.with_social(texts, |s, env| s.befriend(id, env));
                self.apply_social(gui, o, texts);
            }
            Req::RemoveFriend(id) => {
                let o = self.with_social(texts, |s, env| s.remove_friend(id, env));
                self.apply_social(gui, o, texts);
            }
            Req::Invite(id) => self.net.send(ChatCmd::PrivInvite(id)),
            Req::Ignore { id, name, on } => {
                if on {
                    self.ignored.insert(id);
                } else {
                    self.ignored.remove(&id);
                }
                self.social.set_ignored(id, on, &name);
                self.refresh_friends(gui, texts);
            }
            Req::Answer { id, accept, windows } => {
                if accept {
                    self.pg_windows.insert(id, windows);
                }
                let o = self.with_social(texts, |s, env| s.answer_invite(id, accept, env));
                self.apply_social(gui, o, texts);
            }
            Req::LftSearch { side, profession, location } => {
                let connected = self.net.logged_in();
                if let Some(c) = self.social.lft.search(side, profession, location, connected) {
                    self.net.send(c);
                }
                let lft = self.social.lft.clone();
                self.swin.lft_rows(gui, &lft, &Texts(texts));
                self.swin.lft_busy_changed(gui, lft.busy);
            }
            Req::LftSet { on, description } => {
                let c = self.social.lft.set(on, &description);
                self.net.send(c);
            }
            Req::LftInvite { id, name } => {
                if zone.target_team.in_team() && !zone.target_team.is_leader() { return; }
                if zone.dynels.get(&(id as i32)).is_some_and(|d| d.npc) { return; }
                // GUI 100eff95 sends TeamJoinRequest directly, never the unrelated `/team loot` text parser.
                let payload = ao_net::n3::team::team_join_request(zone.char_id as i32, ao_net::msg::Identity { kind: 50000, instance: id as i32 }, false);
                self.outbox.push(ao_net::n3::outgoing::n3_frame(0, zone.char_id, payload));
                let head = texts.by_key(100, "JoinTeamRequestSentTo").unwrap_or_default();
                self.line(gui, ChatLine::new(ChatKind::System, format!("{head}{name}")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::chat::net::Out;
    use ao_gui::{InputEvent, MouseButton};

    #[test]
    fn ignore_target_and_name_paths_refresh_friends_and_feedback() {
        let Some((mut gui, db, mut c)) = rig() else { return };
        c.win = None;
        let zone = Zone::new(1);
        c.perform(&mut gui, super::super::cmd::ChatAction::IgnoreToggle { id: 5, name: Some("Bob".into()) }, &zone, &db);
        assert!(c.ignored.contains(&5));
        assert_eq!(c.social.nodes[&5].state, 3);
        let expected = db.by_key(10001, "ChatCmdFeedback_Ignore_IgnoringCharacter").unwrap().replacen("%s", "Bob", 1);
        assert!(c.pending_log_lines().any(|l| l.text.contains(&expected)));
        c.name_op(&mut gui, super::super::zonecmd::NameOp::Ignore, 5, "Bob", &db);
        assert!(!c.ignored.contains(&5));
        assert_ne!(c.social.nodes[&5].state, 3);
        let expected = db.by_key(10001, "ChatCmdFeedback_Ignore_UnignoringCharacter").unwrap().replacen("%s", "Bob", 1);
        assert!(c.pending_log_lines().any(|l| l.text.contains(&expected)));
    }

    #[test]
    fn lft_invite_sends_join_request_and_requires_team_leader() {
        let Some((mut gui, db, mut c)) = rig() else { return };
        let mut zone = Zone::new(1);
        let invite = || Req::LftInvite { id: 5, name: "Bob".into() };
        c.social_req(&mut gui, invite(), &zone, &db);
        let frames = c.take_outbox();
        assert_eq!(frames.len(), 1);
        let who = ao_net::msg::Identity { kind: 50000, instance: 5 };
        assert_eq!(frames[0].payload, ao_net::n3::team::team_join_request(1, who, false));
        zone.target_team.team = Some(ao_net::msg::Identity { kind: 0xdea9, instance: 9 });
        c.social_req(&mut gui, invite(), &zone, &db);
        assert!(c.take_outbox().is_empty());
        zone.target_team.leader = Some(zone.target_team.own);
        c.social_req(&mut gui, invite(), &zone, &db);
        assert_eq!(c.take_outbox().len(), 1);
    }

    fn rig() -> Option<(Gui, TextDb, Chat)> {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() {
            return None;
        }
        crate::play::prefs::set_test_dir(crate::play::prefs::test_dir());
        let (l, db) = (TextDb::load(&client).ok()?, TextDb::load(&client).ok()?);
        let mut gui = Gui::new(&client, Some(Box::new(move |s: &str| Some(l.label(s)).filter(|r| r != s)))).ok()?;
        let mut c = Chat::new(false);
        c.open(&mut gui, (1280, 828), None).ok()?;
        c.own_id = 1;
        Some((gui, db, c))
    }

    /// What `Chat::update` does with the events of the network layer.
    fn feed(c: &mut Chat, gui: &mut Gui, texts: &TextDb, evs: Vec<ChatEvent>) {
        for e in evs {
            for o in c.net.inject(e) {
                match o {
                    Out::Social(e) => c.social_event(gui, e, texts),
                    Out::Msg(m) if m.tell => c.social_tell_in(gui, m.from_id, &m.from_name, &m.text, false, texts),
                    Out::OpenTell { id, name } => c.social_open_tell_out(gui, id, &name, texts),
                    _ => {}
                }
            }
        }
    }

    fn click(gui: &mut Gui, win: usize, view: &str) -> Vec<Event> {
        let r = gui.view_rect(win, view).unwrap();
        let (x, y) = ((r.l + r.r) / 2.0, (r.t + r.b) / 2.0);
        gui.input(InputEvent::MouseMove { x, y });
        gui.input(InputEvent::MouseDown { x, y, button: MouseButton::Left });
        gui.input(InputEvent::MouseUp { x, y, button: MouseButton::Left })
    }

    /// Replayed events: a character invites us, the dialog's Yes joins (the ticked windows apply), the group becomes an output
    /// group (`say` -> 0x39), its messages arrive in the group; a tell opens a node + window.
    #[test]
    fn private_group_flow_through_the_hub() {
        let Some((mut gui, db, mut c)) = rig() else { return };
        feed(&mut c, &mut gui, &db, vec![ChatEvent::UserName { id: 5, name: "Bob".into() }, ChatEvent::UserName { id: 1, name: "Me".into() }, ChatEvent::PrivInvited(5)]);
        assert!(c.social.nodes[&5].invite);
        let dw = c.swin.dialog_windows()[0];
        // Yes with the default window ticked
        let mut answered = false;
        for e in click(&mut gui, dw, "btn0") {
            assert!(c.social_ui_event_for_test(&mut gui, &e, &db));
            answered = true;
        }
        assert!(answered && !c.social.nodes[&5].invite);
        assert!(c.pg_windows.contains_key(&5));
        feed(&mut c, &mut gui, &db, vec![ChatEvent::PrivJoined { group: 5, who: 1 }]);
        assert_eq!(c.net.groups.get(&0xE_0000_0005).map(String::as_str), Some("Bob"));
        assert!(c.pg_windows.is_empty());
        assert_eq!(c.net.group_by_name("bob"), Some(0xE_0000_0005));
        feed(&mut c, &mut gui, &db, vec![ChatEvent::PrivKicked(5)]);
        assert!(!c.net.groups.contains_key(&0xE_0000_0005));
    }

    #[test]
    fn tell_opens_a_node_and_window() {
        let Some((mut gui, db, mut c)) = rig() else { return };
        feed(&mut c, &mut gui, &db, vec![ChatEvent::UserName { id: 5, name: "Bob".into() }, ChatEvent::Tell { from: 5, text: "psst".into(), data: vec![0] }]);
        assert_eq!(c.social.nodes[&5].unread, 1);
        assert!(!c.swin.tell_is_open(5));
        assert_eq!(c.tell_log[&5].len(), 1);
        assert_eq!(c.take_tip_events(), [("OnReceiveTellMessage", String::new())]);
        c.social_tell_in(&mut gui, 1, "Me", "self echo", false, &db);
        assert!(c.take_tip_events().is_empty());
        c.open_tell_window(&mut gui, 5, "Bob", &db);
        assert!(c.swin.tell_is_open(5) && c.social.nodes[&5].unread == 0 && c.social.nodes[&5].window_open);
        // the earlier line is replayed into the window
        let mut values = crate::play::dvalue::DValues::new(&ao_gui::client_dir());
        values.set_i64("IsTipEnabled", 1);
        let mut tips = crate::play::hud_tips::Tips::new(&ao_gui::client_dir()).unwrap();
        feed(&mut c, &mut gui, &db, vec![ChatEvent::Tell { from: 5, text: "again".into(), data: vec![0] }]);
        for (event, data) in c.take_tip_events() {
            tips.trigger(&mut gui, &mut values, (1280, 828), event, &data);
        }
        assert!(tips.window().is_some(), "native incoming tell reaches the authored tip registry");
        let zone = crate::play::zone::Zone::new(1);
        c.show_url(&mut gui, &zone, &db, "itemid://1000020/49573");
        let events = c.take_tip_events();
        let store = ao_rdb::RecordStore::open(&ao_gui::client_dir()).unwrap();
        let record = store.get(ao_formats::dynel_visual::ITEM_TEMPLATE_TYPE, 49573).unwrap().unwrap();
        let item = ao_formats::dynel_visual::parse_item_template(&record).unwrap();
        assert_eq!(events, [("OnLookatItem", item.name.unwrap_or_default())]);
        for (event, data) in events {
            tips.trigger(&mut gui, &mut values, (1280, 828), event, &data);
        }
        let w = c.swin.tell_window_for_test(5).unwrap();
        assert!(gui.text(w, "text").contains("psst"));
    }
}
