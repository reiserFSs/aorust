//! The social half of `ChatGUIModule_c`: buddy list ("Friends" window model), per-character tell windows, private chat groups and the
//! looking-for-team search. Pure state + decisions; the windows are `social_win.rs`. Evidence: docs/chat/social.md.
//!
//! The original keeps one `FriendListNode_c` (GUI 0x100a6aa7, 0xb0 bytes) per character the player has chatted with or befriended
//! (`FUN_100a5e7a` = find-or-create, owner `FUN_10085b01`, singleton `DAT_102762c8`); the Friends window (`FriendListView_c`,
//! `FUN_100a9c58`) files them into folders by `state`, the chat server's buddy list (`S2C_ADD_BUDDY` / `S2C_REM_BUDDY`) is the
//! source of truth (`ChatGUIModule_c::UpdateBuddyList` 0x10085f17 -> slot `FUN_100a5f74`).

use super::line::ChatMsg;
use ao_net::chat::{ChatCmd, ChatEvent, LftReply, KIND_PRIVATE_GROUP};
use std::collections::BTreeMap;

/// Group key of the private chat group owned by character `owner` (`GetGroupIdentifier(owner, 0xe)`; `(kind << 32) | id` like the hub).
pub fn pg_key(owner: u32) -> u64 {
    (KIND_PRIVATE_GROUP as u64) << 32 | owner as u64
}

/// `ChatPGInviteAction` pref (OptionPanel/Root.xml radio group; default 1 in CharPrefs.xml).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InviteAction {
    /// 0 "AlwaysDeclineInvitat..."
    Decline,
    /// 1 "DisplayInviteDialog"
    Dialog,
    /// 2 "FlashUserIn..." (friends list entry flashes, a line tells about the invitation)
    Flash,
}

impl InviteAction {
    pub fn from_pref(v: i32) -> Self {
        match v {
            0 => Self::Decline,
            1 => Self::Dialog,
            _ => Self::Flash,
        }
    }
}

/// A `ppj::Client_c::Buddy_t` (0x34 bytes): `+0` id, `+4` name, `+0x20` online flag, `+0x24` data bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Buddy {
    pub id: u32,
    pub online: u32,
    pub data: Vec<u8>,
}

impl Buddy {
    /// `FUN_100a5f74`: a data block of exactly one 0 byte marks the temporary entry the client creates when it opens a tell window.
    pub fn temporary(&self) -> bool {
        self.data == [0]
    }
}

/// `FriendListNode_c::+0x2c`: which folder of the Friends window holds the character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
    Online,
    Offline,
    Recent,
}

/// One character of the Friends window (`FriendListNode_c`, GUI 0x100a6aa7: name `+0xc`, id `+0x28`, state `+0x2c`, raw online `+0x30`,
/// unread count `+0x34`, invite pending `+0x39`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub id: u32,
    pub name: String,
    /// 0 online, 1 offline, 2 recent (temporary buddy / tell partner), 3 ignored (red label, menu entry "Invite" disabled).
    pub state: u8,
    /// The server's online flag as sent (`Buddy_t+0x20`).
    pub raw_online: u32,
    /// Messages received while no tell window was open (`+0x34`): red label + flashing icon.
    pub unread: i32,
    /// A private group invitation from this character waits (`+0x39`): flashing icon.
    pub invite: bool,
    /// The tell window is open (`WeakPointer<TellWindow_c>` set).
    pub window_open: bool,
}

impl Node {
    pub fn folder(&self) -> Folder {
        match self.state {
            0 => Folder::Online,
            1 | 3 => Folder::Offline,
            _ => Folder::Recent,
        }
    }
    /// Gfx id of the item icon (`FUN_100a8141`: `0xd8 - (raw_online != 0)`).
    pub fn icon(&self) -> u32 {
        0xd8 - (self.raw_online != 0) as u32
    }
    /// `StringListViewItem_c::FlashIcon(unread > 0 || invite)`.
    pub fn flashing(&self) -> bool {
        self.unread > 0 || self.invite
    }
    /// `SetLabelColor(0xff6666, 0xc04c4c)` for ignored characters (state 3) and for nodes with unread messages.
    pub fn red(&self) -> bool {
        self.state == 3 || self.unread > 0
    }
}

/// What the hub does after a social event.
#[derive(Clone, Debug, PartialEq)]
pub enum SocialOut {
    /// A plain line of the System window (`GlobalSignals+0x17c(0, text, 0)`).
    System(String),
    /// A request for the chat server.
    Send(ChatCmd),
    /// A message into a private group window (`GroupMessage_t`).
    Msg(ChatMsg),
    GroupAdd { group: u64, name: String },
    GroupRemove { group: u64 },
    /// Private group invitation dialog (`FUN_100a700a`): "PrivateGroupInvitation" + `ChatFriendList_InvitedToPrivateGroupDialogText`.
    InviteDialog { id: u32, name: String },
    /// The buddy list or node flags changed: refresh the Friends window.
    FriendsChanged,
    /// Text for the tell window of `id` (HTML, `FUN_100a6cd5`).
    TellText { id: u32, html: String },
}

/// Everything the logic needs from its environment.
pub struct Env<'a> {
    pub own_id: u32,
    pub connected: bool,
    pub name_of: &'a dyn Fn(u32) -> String,
    /// Text db category 10001 by key (`GetText(0x2711, key)`).
    pub text: &'a dyn Fn(&str) -> String,
    pub invite_pref: InviteAction,
    pub ignored: &'a dyn Fn(u32) -> bool,
}

#[derive(Default)]
pub struct Social {
    pub buddies: Vec<Buddy>,
    pub nodes: BTreeMap<u32, Node>,
    pub lft: Lft,
}

/// `[<a href=user://NAME>NAME</a>]: ` as `FUN_100a6210` builds it (GM senders: red link). The sender link of tell lines.
pub fn tell_prefix(name: &str, gm: bool) -> String {
    let a = format!("<a style=\"text-decoration:none\" href=\"user://{name}\">{name}</a>");
    if gm {
        format!("[<font color=\"#FF0000\">{a}</font>]: ")
    } else {
        format!("[{a}]: ")
    }
}

impl Social {
    fn node_mut(&mut self, id: u32, name: &str, state: u8, raw: u32) -> &mut Node {
        self.nodes.entry(id).or_insert_with(|| Node { id, name: name.to_owned(), state, raw_online: raw, unread: 0, invite: false, window_open: false })
    }

    /// `FUN_100a5f74`: rebuild the nodes from the buddy list. Buddies with data `{0}` are *recent* (state 2); the others are online
    /// (state 0, `online == 1`) or offline (1). Nodes the list no longer holds are dropped unless ignored (state 3).
    pub fn refresh(&mut self, env: &Env) {
        let mut seen = vec![];
        for b in &self.buddies {
            let state = if b.temporary() { 2 } else { (b.online != 1) as u8 };
            seen.push(b.id);
            let name = (env.name_of)(b.id);
            match self.nodes.get_mut(&b.id) {
                Some(n) => {
                    // FUN_100a6c06(name, state, online): an ignored node keeps state 3
                    if !name.is_empty() {
                        n.name = name;
                    }
                    if n.state != 3 {
                        n.state = state;
                    }
                    n.raw_online = b.online;
                }
                None => {
                    self.nodes.insert(b.id, Node { id: b.id, name, state, raw_online: b.online, unread: 0, invite: false, window_open: false });
                }
            }
        }
        self.nodes.retain(|id, n| seen.contains(id) || n.state == 3 || n.window_open || n.invite);
    }

    /// Mark the ignore state of a character (`IgnoreSystem_t`, state 3).
    pub fn set_ignored(&mut self, id: u32, ignored: bool, name: &str) {
        if ignored {
            self.node_mut(id, name, 3, 0).state = 3;
        } else if let Some(n) = self.nodes.get_mut(&id) {
            if n.state == 3 {
                n.state = 1;
            }
        }
    }

    /// `FUN_100a5e7a(name, id)`: find or create the node; a new node (state 2) asks the server for a *temporary* buddy entry (D = 0)
    /// so the character's online state is followed.
    pub fn node_for(&mut self, id: u32, name: &str, env: &Env) -> Vec<SocialOut> {
        if self.nodes.contains_key(&id) {
            return vec![];
        }
        self.nodes.insert(id, Node { id, name: name.to_owned(), state: 2, raw_online: 0, unread: 0, invite: false, window_open: false });
        let mut out = vec![SocialOut::FriendsChanged];
        if env.connected {
            out.push(SocialOut::Send(ChatCmd::BuddyAdd { id, permanent: false }));
        }
        out
    }

    /// `OpenTellWindow` 0x10085df8 = `FUN_10085b01` + `FUN_100a6568` (= `FUN_100a5e7a` + show): the tell window of `id` opens.
    pub fn open_tell(&mut self, id: u32, name: &str, env: &Env) -> Vec<SocialOut> {
        if id == 0 {
            return vec![];
        }
        let out = self.node_for(id, name, env);
        let n = self.nodes.get_mut(&id).unwrap();
        n.window_open = true;
        n.unread = 0;
        out
    }

    pub fn close_tell(&mut self, id: u32) {
        if let Some(n) = self.nodes.get_mut(&id) {
            n.window_open = false;
        }
    }

    /// `HandlePrivateMessage` 0x1008792e: the sender gets a node (so a temporary buddy), the text goes to its tell window; without an
    /// open window the node's unread count rises (red + flashing in the Friends window). Returns the HTML line of the tell window.
    pub fn tell_in(&mut self, id: u32, name: &str, text: &str, gm: bool, env: &Env) -> Vec<SocialOut> {
        if id == 0 {
            return vec![SocialOut::System(text.to_owned())];
        }
        let mut out = self.node_for(id, name, env);
        let n = self.nodes.get_mut(&id).unwrap();
        if !n.window_open {
            n.unread += 1;
            out.push(SocialOut::FriendsChanged);
        }
        out.push(SocialOut::TellText { id, html: format!("{}{}", tell_prefix(name, gm), text) });
        out
    }

    /// Menu action "Befriend X" (`FUN_100a6940`): permanent buddy entry (D = 1).
    pub fn befriend(&self, id: u32, env: &Env) -> Vec<SocialOut> {
        if env.connected {
            vec![SocialOut::Send(ChatCmd::BuddyAdd { id, permanent: true })]
        } else {
            vec![]
        }
    }

    /// Confirmed "Delete X" (`ReallyWantToRemoveXfromFriends`): `C2S_REM_BUDDY`.
    pub fn remove_friend(&self, id: u32, env: &Env) -> Vec<SocialOut> {
        if env.connected {
            vec![SocialOut::Send(ChatCmd::BuddyRemove(id))]
        } else {
            vec![]
        }
    }

    /// `FUN_100a72c7(true)`: an invitation from `id` arrives (pref `ChatPGInviteAction`).
    pub fn invited(&mut self, id: u32, name: &str, env: &Env) -> Vec<SocialOut> {
        if (env.ignored)(id) {
            return vec![SocialOut::Send(ChatCmd::PrivPart(id))]; // declined silently
        }
        let line = |k: &str| SocialOut::System((env.text)(k).replacen("%s", name, 1));
        match env.invite_pref {
            InviteAction::Decline => vec![line("ChatInviteAutoDeclined"), SocialOut::Send(ChatCmd::PrivPart(id))],
            pref => {
                let mut out = self.node_for(id, name, env);
                self.nodes.get_mut(&id).unwrap().invite = true;
                if pref == InviteAction::Dialog {
                    out.push(SocialOut::InviteDialog { id, name: name.to_owned() });
                }
                out.push(SocialOut::FriendsChanged);
                out.push(line("ChatInvitePending"));
                out
            }
        }
    }

    /// Yes / No of the invitation dialog (`FUN_100a6e0c`): accept = `C2S_PRIVGRP_JOIN` (the group appears with its `PRIVGRP_JOINED`),
    /// decline = `C2S_PRIVGRP_PART`.
    pub fn answer_invite(&mut self, id: u32, accept: bool, env: &Env) -> Vec<SocialOut> {
        if let Some(n) = self.nodes.get_mut(&id) {
            n.invite = false;
        }
        let mut out = vec![SocialOut::FriendsChanged];
        if env.connected {
            out.push(SocialOut::Send(if accept { ChatCmd::PrivJoin(id) } else { ChatCmd::PrivPart(id) }));
        }
        out
    }

    fn group_msg(owner: u32, text: String, name: &str) -> SocialOut {
        SocialOut::Msg(ChatMsg { group: pg_key(owner), group_name: name.to_owned(), text, kind: KIND_PRIVATE_GROUP, ..Default::default() })
    }

    /// Chat-server events of the social layer. `None` = not a social event.
    pub fn on_event(&mut self, e: &ChatEvent, env: &Env) -> Option<Vec<SocialOut>> {
        let who = |id: u32| (env.name_of)(id);
        Some(match e {
            ChatEvent::BuddyAdd { id, online, data } => {
                match self.buddies.iter_mut().find(|b| b.id == *id) {
                    Some(b) => b.online = *online, // S2C_ADD_BUDDY of a known buddy only updates the online flag (GUI 0x1016dd9a)
                    None => self.buddies.push(Buddy { id: *id, online: *online, data: data.clone() }),
                }
                self.refresh(env);
                vec![SocialOut::FriendsChanged]
            }
            ChatEvent::BuddyRemove(id) => {
                self.buddies.retain(|b| b.id != *id);
                self.nodes.retain(|i, n| i != id || n.state == 3 || n.window_open || n.invite);
                self.refresh(env);
                vec![SocialOut::FriendsChanged]
            }
            ChatEvent::PrivInvited(id) => self.invited(*id, &who(*id), env),
            // 0x33: PrivateGroupAction(kicked) + GroupAction(part) + GroupMessage("LeftPrivateGroup" + group name) [GUI 0x1016ed14]
            ChatEvent::PrivKicked(id) => {
                let name = who(*id);
                vec![Self::group_msg(*id, format!("{}{}", (env.text)("LeftPrivateGroup"), name), &name), SocialOut::GroupRemove { group: pg_key(*id) }]
            }
            ChatEvent::PrivJoined { group, who: w } => {
                let gname = who(*group);
                if *w == env.own_id {
                    // GroupAction(join, flag 1) + "YouJoinedPrivateChat" + group name [GUI 0x1016e9fe..]
                    vec![
                        SocialOut::GroupAdd { group: pg_key(*group), name: gname.clone() },
                        Self::group_msg(*group, format!("{}{}", (env.text)("YouJoinedPrivateChat"), gname), &gname),
                    ]
                } else {
                    vec![Self::group_msg(*group, format!("{}{}", who(*w), (env.text)("JoinedGroup")), &gname)]
                }
            }
            ChatEvent::PrivParted { group, who: w } => {
                vec![Self::group_msg(*group, format!("{}{}", who(*w), (env.text)("LeftGroup")), &who(*group))]
            }
            ChatEvent::PrivDeclined { group, who: w } => {
                vec![Self::group_msg(*group, format!("{}{}", who(*w), (env.text)("DeclinedInvite")), &who(*group))]
            }
            ChatEvent::PrivMessage { group, from, text, data } => vec![SocialOut::Msg(ChatMsg {
                group: pg_key(*group),
                group_name: who(*group),
                from_id: *from,
                from_name: who(*from),
                text: text.clone(),
                kind: data.first().copied().unwrap_or(0),
                ..Default::default()
            })],
            ChatEvent::LftReply(r) => {
                self.lft.on_reply(r);
                vec![]
            }
            _ => return None,
        })
    }
}

// ----------------------------------------------------------------------------------------------------------- LFT

/// `Side` dropdown entries `(item id, text id of category 2005)`; item 7 is "any" (sent as -1). `FUN_100f03fb`.
pub const LFT_SIDES: [(u32, Option<u32>); 4] = [(0, Some(0)), (1, Some(1)), (2, Some(2)), (7, None)];
/// `Location` dropdown `(item id, label)`; the query sends the selected *index* (== id here, items are inserted at their id).
pub const LFT_LOCATIONS: [(u32, &str); 4] = [(0, "this playfield"), (1, "anywhere"), (2, "Rubi-Ka"), (3, "Shadowlands")];
/// Defaults of the dropdowns (`SelectByID`): side 7 "any", location 2 "Rubi-Ka", profession 0x10 "any".
pub const LFT_DEFAULT: (u32, u32, u32) = (7, 2, 0x10);
/// `EventTimer_c::Start(3000000 us)`: the Search button is dead for this long after a query.
pub const LFT_SEARCH_COOLDOWN: f32 = 3.0;

/// Profession item ids: 1..15 without 13 ("Monster"; text id of category 2004), then 0x10 "any" (`FUN_100f03fb`).
pub fn lft_professions() -> impl Iterator<Item = u32> {
    (1..16).filter(|p| *p != 13)
}

/// State of the LFT window and of the own "looking for team" flag (`DAT_10276620`).
#[derive(Clone, Debug, Default)]
pub struct Lft {
    /// Own flag.
    pub on: bool,
    pub description: String,
    pub results: Vec<LftReply>,
    /// `LFTWindow_c+0x7c`: a query is out and its end marker has not arrived.
    pub busy: bool,
    /// Seconds left of the search cooldown (`+0x80` EventTimer).
    pub cooldown: f32,
}

impl Lft {
    /// `FUN_100ef912` (Search): `None` while busy or cooling down. The result list is cleared; `side_id` / `profession_id` are the
    /// dropdown item ids, `location` the selected index. `connected` = `ChatGUIModule_c+0x30` (the chat client exists).
    pub fn search(&mut self, side_id: u32, profession_id: u32, location: u32, connected: bool) -> Option<ChatCmd> {
        if self.busy || self.cooldown > 0.0 {
            return None;
        }
        self.results.clear();
        if !connected {
            return None;
        }
        self.busy = true;
        self.cooldown = LFT_SEARCH_COOLDOWN;
        Some(ChatCmd::LftQuery {
            side: if side_id == 7 { u32::MAX } else { side_id },
            professions: if profession_id == 0x10 { u32::MAX } else { 1u32 << (profession_id & 31) },
            location,
        })
    }

    pub fn tick(&mut self, dt: f32) {
        self.cooldown = (self.cooldown - dt).max(0.0);
    }

    /// `HandleLFTMessage` 0x10087069 + the slot `FUN_100efe4b`: status 0 raises the candidate signal with the packet's fields, status 2 raises it with id 0;
    /// the slot clears `busy` (`+0x7c`) for id 0 and adds a row otherwise. **Live (Ithaca 2026-10-06)**: an empty search is answered by ONE status-0 packet with
    /// every field zero (id 0, empty strings), i.e. status 0 / id 0 is the end marker too (docs/chat/live.md).
    pub fn on_reply(&mut self, r: &LftReply) {
        match r.status {
            0 | 2 if r.id == 0 || r.status == 2 => self.busy = false,
            0 => self.results.push(r.clone()),
            _ => {}
        }
    }

    /// `FUN_100f01a3(on, desc)`: the checkbox / description changed. `LftOff` or `LftOn(desc)`; the window config keeps `TeamDesc`.
    pub fn set(&mut self, on: bool, desc: &str) -> ChatCmd {
        self.on = on;
        if on {
            self.description = desc.to_owned();
            ChatCmd::LftOn(desc.to_owned())
        } else {
            ChatCmd::LftOff
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env<'a>(names: &'a dyn Fn(u32) -> String, text: &'a dyn Fn(&str) -> String, ignored: &'a dyn Fn(u32) -> bool, pref: InviteAction) -> Env<'a> {
        Env { own_id: 1, connected: true, name_of: names, text, invite_pref: pref, ignored }
    }
    fn names(id: u32) -> String {
        match id {
            1 => "Me".into(),
            5 => "Bob".into(),
            6 => "Eve".into(),
            _ => String::new(),
        }
    }
    /// Strings of text.mdb category 10001 (checked against the real file by `texts_match_the_real_db`).
    fn text(k: &str) -> String {
        match k {
            "ChatInvitePending" => "You were invited to a private chat group by %s.",
            "ChatInviteAutoDeclined" => "Chat group invitation from %s was auto declined.",
            "YouJoinedPrivateChat" => "You joined private group: ",
            "JoinedGroup" => " joined the group.",
            "LeftGroup" => " left the group.",
            "LeftPrivateGroup" => "You left private group: ",
            "DeclinedInvite" => " declined the invitation.",
            _ => "",
        }
        .into()
    }
    fn no(_: u32) -> bool {
        false
    }

    #[test]
    fn buddy_list_files_into_folders() {
        let mut s = Social::default();
        let e = env(&names, &text, &no, InviteAction::Dialog);
        s.on_event(&ChatEvent::BuddyAdd { id: 5, online: 1, data: vec![1] }, &e);
        s.on_event(&ChatEvent::BuddyAdd { id: 6, online: 0, data: vec![1] }, &e);
        assert_eq!((s.nodes[&5].folder(), s.nodes[&5].icon()), (Folder::Online, 0xd7));
        assert_eq!((s.nodes[&6].folder(), s.nodes[&6].icon()), (Folder::Offline, 0xd8));
        // a temporary entry (D = {0}) is a recent contact whatever its online flag says
        s.on_event(&ChatEvent::BuddyAdd { id: 7, online: 1, data: vec![0] }, &e);
        assert_eq!(s.nodes[&7].folder(), Folder::Recent);
        // a second S2C_ADD_BUDDY only updates the online flag
        s.on_event(&ChatEvent::BuddyAdd { id: 6, online: 1, data: vec![1] }, &e);
        assert_eq!(s.nodes[&6].folder(), Folder::Online);
        s.on_event(&ChatEvent::BuddyRemove(5), &e);
        assert!(!s.nodes.contains_key(&5) && s.buddies.len() == 2);
    }

    #[test]
    fn tell_window_requests_a_temporary_buddy_once() {
        let mut s = Social::default();
        let e = env(&names, &text, &no, InviteAction::Dialog);
        let o = s.open_tell(5, "Bob", &e);
        assert!(o.contains(&SocialOut::Send(ChatCmd::BuddyAdd { id: 5, permanent: false })));
        assert!(s.open_tell(5, "Bob", &e).iter().all(|o| !matches!(o, SocialOut::Send(_))));
        assert!(s.nodes[&5].window_open);
        assert_eq!(s.befriend(5, &e), [SocialOut::Send(ChatCmd::BuddyAdd { id: 5, permanent: true })]);
        assert_eq!(s.remove_friend(5, &e), [SocialOut::Send(ChatCmd::BuddyRemove(5))]);
    }

    #[test]
    fn incoming_tell_marks_unread_until_the_window_opens() {
        let mut s = Social::default();
        let e = env(&names, &text, &no, InviteAction::Dialog);
        let o = s.tell_in(5, "Bob", "hi", false, &e);
        assert!(o.contains(&SocialOut::TellText { id: 5, html: "[<a style=\"text-decoration:none\" href=\"user://Bob\">Bob</a>]: hi".into() }));
        let n = &s.nodes[&5];
        assert_eq!((n.unread, n.flashing(), n.red(), n.folder()), (1, true, true, Folder::Recent));
        s.open_tell(5, "Bob", &e);
        assert_eq!(s.nodes[&5].unread, 0);
        s.tell_in(5, "Bob", "again", true, &e);
        assert_eq!(s.nodes[&5].unread, 0);
        assert!(tell_prefix("X", true).starts_with("[<font color=\"#FF0000\"><a "));
    }

    #[test]
    fn invitation_follows_the_pref() {
        let mut s = Social::default();
        let o = s.on_event(&ChatEvent::PrivInvited(5), &env(&names, &text, &no, InviteAction::Decline)).unwrap();
        assert_eq!(o, [SocialOut::System("Chat group invitation from Bob was auto declined.".into()), SocialOut::Send(ChatCmd::PrivPart(5))]);
        let o = s.on_event(&ChatEvent::PrivInvited(5), &env(&names, &text, &no, InviteAction::Dialog)).unwrap();
        assert!(o.contains(&SocialOut::InviteDialog { id: 5, name: "Bob".into() }));
        assert!(o.contains(&SocialOut::System("You were invited to a private chat group by Bob.".into())));
        assert!(s.nodes[&5].invite && s.nodes[&5].flashing());
        let o = s.on_event(&ChatEvent::PrivInvited(6), &env(&names, &text, &no, InviteAction::Flash)).unwrap();
        assert!(!o.iter().any(|o| matches!(o, SocialOut::InviteDialog { .. })));
        // ignored inviters are declined without a word
        let ign = |i: u32| i == 6;
        let o = s.on_event(&ChatEvent::PrivInvited(6), &env(&names, &text, &ign, InviteAction::Dialog)).unwrap();
        assert_eq!(o, [SocialOut::Send(ChatCmd::PrivPart(6))]);
        let e = env(&names, &text, &no, InviteAction::Dialog);
        assert!(s.answer_invite(5, true, &e).contains(&SocialOut::Send(ChatCmd::PrivJoin(5))));
        assert!(!s.nodes[&5].invite);
        assert!(s.answer_invite(5, false, &e).contains(&SocialOut::Send(ChatCmd::PrivPart(5))));
    }

    #[test]
    fn private_group_lifecycle_messages() {
        let mut s = Social::default();
        let e = env(&names, &text, &no, InviteAction::Dialog);
        let o = s.on_event(&ChatEvent::PrivJoined { group: 5, who: 1 }, &e).unwrap();
        assert_eq!(o[0], SocialOut::GroupAdd { group: 0xE_0000_0005, name: "Bob".into() });
        let SocialOut::Msg(m) = &o[1] else { panic!() };
        assert_eq!((m.group, m.text.as_str()), (0xE_0000_0005, "You joined private group: Bob"));
        let o = s.on_event(&ChatEvent::PrivJoined { group: 5, who: 6 }, &e).unwrap();
        let SocialOut::Msg(m) = &o[0] else { panic!() };
        assert_eq!(m.text, "Eve joined the group.");
        let SocialOut::Msg(m) = &s.on_event(&ChatEvent::PrivParted { group: 5, who: 6 }, &e).unwrap()[0] else { panic!() };
        assert_eq!(m.text, "Eve left the group.");
        let SocialOut::Msg(m) = &s.on_event(&ChatEvent::PrivDeclined { group: 1, who: 6 }, &e).unwrap()[0] else { panic!() };
        assert_eq!((m.text.as_str(), m.group), ("Eve declined the invitation.", 0xE_0000_0001));
        let o = s.on_event(&ChatEvent::PrivKicked(5), &e).unwrap();
        assert!(matches!(&o[0], SocialOut::Msg(m) if m.text == "You left private group: Bob"));
        assert_eq!(o[1], SocialOut::GroupRemove { group: 0xE_0000_0005 });
        let o = s.on_event(&ChatEvent::PrivMessage { group: 5, from: 6, text: "yo".into(), data: vec![] }, &e).unwrap();
        let SocialOut::Msg(m) = &o[0] else { panic!() };
        assert_eq!((m.group, m.from_name.as_str(), m.text.as_str()), (0xE_0000_0005, "Eve", "yo"));
        assert!(s.on_event(&ChatEvent::LoggedIn, &e).is_none());
    }

    #[test]
    fn lft_search_and_results() {
        let mut l = Lft::default();
        // defaults: side any (-1), profession any (-1), Rubi-Ka (2)
        assert_eq!(l.search(7, 0x10, 2, true), Some(ChatCmd::LftQuery { side: u32::MAX, professions: u32::MAX, location: 2 }));
        assert_eq!(l.search(7, 0x10, 2, true), None); // busy
        l.on_reply(&LftReply { status: 0, id: 3, name: "A".into(), level: 10, playfield: 4001, side: 1, profession: 6, description: "d".into() });
        l.on_reply(&LftReply { status: 1, id: 9, name: "ignored".into(), level: 0, playfield: 0, side: 0, profession: 0, description: String::new() });
        assert_eq!(l.results.len(), 1);
        l.on_reply(&LftReply { status: 2, id: 0, name: String::new(), level: 0, playfield: 0, side: 0, profession: 0, description: String::new() });
        assert!(!l.busy);
        assert_eq!(l.search(2, 3, 0, true), None); // 3 s cooldown still running
        l.tick(3.1);
        assert_eq!(l.search(2, 3, 0, true), Some(ChatCmd::LftQuery { side: 2, professions: 8, location: 0 }));
        assert!(l.results.is_empty());
        assert_eq!(l.set(true, "raid"), ChatCmd::LftOn("raid".into()));
        assert_eq!(l.set(false, "ignored"), ChatCmd::LftOff);
        assert_eq!(lft_professions().count(), 14);
    }

    /// Live capture (Ithaca 2026-10-06, docs/captures/chat_social_ithaca.rec, two sessions): sent frames re-encode byte for byte,
    /// received ones decode and drive the buddy folders / the LFT state like the real server's answers did.
    #[test]
    fn live_social_capture_replay() {
        let name = |id: u32| match id {
            0x6584 => "Testy",
            0x7dfe => "Beinrangel",
            0x7bf4 => "Battle",
            _ => "",
        };
        let names = |id: u32| name(id).to_owned();
        let e = env(&names, &text, &no, InviteAction::Dialog);
        let (mut s, mut evs, mut sent) = (Social::default(), vec![], vec![]);
        for l in include_str!("../../../../../docs/captures/chat_social_ithaca.rec").lines().filter(|l| !l.starts_with('#')) {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next(), p.next().unwrap(), p.next().unwrap());
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            assert_eq!(b.len(), 4 + u16::from_be_bytes([b[2], b[3]]) as usize);
            if dir == "<" {
                evs.push(ao_net::chat::decode(u16::from_be_bytes([b[0], b[1]]), &b[4..]).unwrap());
            } else {
                sent.push(hex.to_owned());
            }
        }
        // C2S layouts as the client sends them (permanent add `D = {1}`, remove, invite / kick / leave, LFT on / off / query)
        let enc = |c: ChatCmd| ao_net::chat::encode(&c).unwrap().iter().map(|x| format!("{x:02x}")).collect::<String>();
        for (c, n) in [
            (ChatCmd::BuddyAdd { id: 0x6584, permanent: true }, 1),
            (ChatCmd::BuddyRemove(0x6584), 1),
            (ChatCmd::LftOn("aomac test".into()), 1),
            (ChatCmd::LftOff, 1),
            (ChatCmd::LftQuery { side: u32::MAX, professions: u32::MAX, location: 2 }, 2),
            (ChatCmd::LftQuery { side: 1, professions: 1 << 6, location: 1 }, 1),
            (ChatCmd::PrivInvite(0x6584), 1),
            (ChatCmd::PrivKick(0x6584), 1),
            (ChatCmd::PrivPart(0x6584), 1),
        ] {
            let h = enc(c.clone());
            assert_eq!(sent.iter().filter(|x| **x == h).count(), n, "{c:?}");
        }
        // the server echoes S2C_ADD_BUDDY with an EMPTY data block (we sent `{1}`) and the real online flag; no reply for ourselves
        let adds: Vec<_> = evs.iter().filter_map(|e| if let ChatEvent::BuddyAdd { id, online, data } = e { Some((*id, *online, data.clone())) } else { None }).collect();
        assert_eq!(adds, [(0x6584, 0, vec![]), (0x7dfe, 1, vec![]), (0x7bf4, 1, vec![])]);
        let rems: Vec<_> = evs.iter().filter_map(|e| if let ChatEvent::BuddyRemove(i) = e { Some(*i) } else { None }).collect();
        assert_eq!(rems, [0x6584, 0x82e8, 0x7dfe, 0x7bf4]);
        for e2 in evs.iter().filter(|e| matches!(e, ChatEvent::BuddyAdd { .. })).take(3) {
            s.on_event(e2, &e);
        }
        assert_eq!((s.nodes[&0x6584].folder(), s.nodes[&0x7dfe].folder(), s.nodes[&0x7bf4].folder()), (Folder::Offline, Folder::Online, Folder::Online));
        assert_eq!((s.nodes[&0x6584].icon(), s.nodes[&0x7dfe].icon()), (0xd8, 0xd7));
        // an empty search answer = one status-0 packet with id 0 and all fields empty: no row, the search ends
        let lft: Vec<_> = evs.iter().filter_map(|e| if let ChatEvent::LftReply(r) = e { Some(r.clone()) } else { None }).collect();
        assert_eq!(lft.len(), 3);
        let mut l = Lft::default();
        for r in &lft {
            assert_eq!(*r, LftReply { status: 0, id: 0, name: String::new(), level: 0, playfield: 0, side: 0, profession: 0, description: String::new() });
            assert!(l.search(7, 0x10, 2, true).is_some());
            l.on_reply(r);
            assert!(!l.busy && l.results.is_empty());
            l.tick(3.1);
        }
    }

    /// The strings above are the real text.mdb ones; the dropdown / column labels come from categories 2003..2005 and 100.
    #[test]
    fn texts_match_the_real_db() {
        let Some(h) = std::env::var_os("HOME") else { return };
        let Ok(db) = ao_formats::screens::TextDb::load(&std::path::Path::new(&h).join("Games/ProjectRubiKa/client")) else { return };
        for k in ["ChatInvitePending", "ChatInviteAutoDeclined", "YouJoinedPrivateChat", "JoinedGroup", "LeftGroup", "LeftPrivateGroup", "DeclinedInvite"] {
            assert_eq!(db.by_key(10001, k).as_deref(), Some(text(k).as_str()), "{k}");
        }
        assert_eq!([0, 1, 2].map(|i| db.by_id(2005, i).unwrap()), ["neutral", "clan", "omni"]);
        assert_eq!(db.by_id(2004, 6).as_deref(), Some("Adventurer"));
        assert_eq!([0x36, 0x21, 0x3c].map(|i| db.by_id(2003, i).unwrap()), ["Level", "Side", "Profession"]);
        assert_eq!(db.by_key(100, "Team Search").as_deref(), Some("Team Search"));
    }
}
