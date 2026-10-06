//! Slash commands that reach the game: `N3Msg_TextCommand(window, text, target)` (Interfaces 0x10008a72 -> Gamecode
//! `n3EngineClientAnarchy_t::N3Msg_TextCommand` 0x100176db -> `FUN_1003fba6`) and the `Fanatic::ClientInterface_c::Command`
//! (Fanatic.dll 0x10001066) path. Evidence and the full command table: docs/chat/cmd.md §Zone commands.
//!
//! `FUN_1003fba6` receives the input line WITHOUT its slash and with `ChatGUIModule_c::ExpandChatTextArgs` applied. It reads the first
//! word with `istringstream >> std::string` and compares it with `std::string::compare` (CASE-SENSITIVE, unlike the GUI command map that
//! got us here), then builds one of
//! * `CharacterActionIIR_t` (`5E477770`, [`super::action::character_action`]) with `param = window` (`items`/`Inspect`: 0),
//! * `OrgClientIIR_c` (`7F4B3108`) — `/org ...`,
//! * `RaidCmdIIR_c` — `/raid ...`,
//!
//! and hands it to `n3Dynel_t::SendIIRToObservers` (= the zone connection, ptype 10, docs/zone/outgoing.md). The dynel is the client's
//! control dynel, so every header identity is `{0xC350, char_id}` and the "to be passed on" byte is 0 (`ClearToBePassedOn`).

use super::action::character_action;
use super::outgoing::{message_key, DYNEL_CHAR};
use super::misc::{FollowTarget, Misc, Vec3};
use super::world::CharacterAction;
use crate::msg::Identity;
use crate::wire::Writer;

/// Action ids of `CharacterActionIIR_t` built by `FUN_1003fba6` (docs/zone/actions.md §2).
pub mod action {
    pub const BANK_OPEN: i32 = 0x24;
    pub const BANK_INFO: i32 = 0x5c;
    pub const DAMAGE_MULT: i32 = 0x67;
    pub const TEAM_LOOT_SET: i32 = 0x7f;
    pub const PLAYED: i32 = 0x85;
    pub const BORN: i32 = 0x86;
    pub const TEAM_LOOT: i32 = 0x91;
    pub const CLONE: i32 = 0x96;
    pub const VERSION: i32 = 0x9d;
    pub const RESET_SKILL: i32 = 0x9a;
    pub const CLEAR_UNIQUE: i32 = 0xae;
    pub const ITEMS_LIST: i32 = 0xf4;
    pub const ITEMS_DELETE: i32 = 0xf5;
    pub const INSPECT: i32 = 0x105;
}

/// `OrgClientIIR_c` command codes (`this+0x18`), as pushed by the `/org` switch (GUI-independent, Gamecode 0x10040c7e).
pub mod org {
    pub const CREATE: u8 = 0x01;
    pub const RANKS: u8 = 0x02;
    pub const CONTRACT: u8 = 0x03;
    pub const INFO: u8 = 0x05;
    pub const STARTVOTE: u8 = 0x07;
    pub const VOTE_INFO: u8 = 0x08;
    pub const VOTE: u8 = 0x09;
    pub const PROMOTE: u8 = 0x0a;
    pub const DEMOTE: u8 = 0x0b;
    pub const KICK_TARGET: u8 = 0x0c;
    pub const KICK_NAME: u8 = 0x0d;
    pub const INVITE: u8 = 0x0e;
    /// `N3Msg_OrgDisbandConfirmed` (Gamecode 0x1001a611: code 6, id = the engine target `+0x5c`, empty text, flag 0).
    pub const DISBAND_CONFIRMED: u8 = 0x06;
    /// `N3Msg_OrgLeaveConfirmed` (Gamecode 0x1001a7c1: code 0x10, id {0,0}).
    pub const LEAVE_CONFIRMED: u8 = 0x10;
    pub const TAX: u8 = 0x11;
    pub const BANK: u8 = 0x12;
    pub const BANK_ADD: u8 = 0x13;
    pub const BANK_REMOVE: u8 = 0x14;
    pub const DEBT: u8 = 0x16;
    pub const HISTORY: u8 = 0x17;
    pub const OBJECTIVE: u8 = 0x18;
    pub const DESCRIPTION: u8 = 0x19;
    pub const NAME: u8 = 0x1a;
    pub const GOVERNING_FORM: u8 = 0x1b;
    pub const STOPVOTE: u8 = 0x1c;
    pub const CITY: u8 = 0x1f;
    /// Codes whose body carries the text (`OrgClientIIR_c` write, Gamecode 0x10125fef).
    pub fn has_text(code: u8) -> bool {
        matches!(code, 1 | 7 | 9 | 0xd | 0x11 | 0x13 | 0x14 | 0x17 | 0x18 | 0x19 | 0x1a | 0x1b | 0x1c)
    }
}

/// Identity kind of raid objects in `RaidCmdIIR_c` (`/raid lootaccess`, Gamecode 0x100405ed).
pub const RAID_KIND: i32 = 0xC76A;

fn header(w: &mut Writer, class: &str, char_id: i32) {
    w.u32(message_key(class));
    Identity { kind: DYNEL_CHAR, instance: char_id }.write(w);
    w.u8(0);
}

/// `OrgClientIIR_c` (ctor Gamecode 0x10126164, write 0x10125fef): `u8 code, Identity id, i32 window` then
/// `i16 len + text` for [`org::has_text`] codes, `u8 flag` (0 from the client) for code 10.
pub fn org_client(char_id: i32, window: i32, code: u8, id: Identity, text: &str) -> Vec<u8> {
    let mut w = Writer::default();
    header(&mut w, "OrgClientIIR_c", char_id);
    w.u8(code);
    id.write(&mut w);
    w.i32(window);
    if org::has_text(code) {
        w.str_i16(text);
    } else if code == org::PROMOTE {
        w.u8(0);
    }
    w.0
}

/// `RaidCmdIIR_c` (ctor 0x100a3531, write 0x100a34ea): `i32 cmd, Identity a`, and for cmd 5 also `Identity b, Identity c`.
pub fn raid_cmd(char_id: i32, cmd: i32, a: Identity, b: Identity, c: Identity) -> Vec<u8> {
    let mut w = Writer::default();
    header(&mut w, "RaidCmdIIR_c", char_id);
    w.i32(cmd);
    a.write(&mut w);
    if cmd == 5 {
        b.write(&mut w);
        c.write(&mut w);
    }
    w.0
}

/// `FanaticIIR_t` (Fanatic.dll ctor 0x10001187, write 0x10001112; key `MapToKey("FanaticIIR_t")`): `i32 window, Identity target,
/// i32 len, bytes`. Sent by `Fanatic::ClientInterface_c::Command(window, target, text)` [Fanatic 0x10001066] for the script/GM commands
/// registered in GUI 0x100b39f6 (handler GUI 0x100b30cf: `text` = the line without its slash, NOT expanded; `target` = `InputConfig_t+0xc0`).
pub fn fanatic(char_id: i32, window: i32, target: Identity, text: &str) -> Vec<u8> {
    let mut w = Writer::default();
    header(&mut w, "FanaticIIR_t", char_id);
    w.i32(window);
    target.write(&mut w);
    w.i32(text.len() as i32);
    w.bytes(text.as_bytes());
    w.0
}

/// `N3Msg_Inspect(target)` (Gamecode 0x1001dc58): `CharacterActionIIR_t` action 0x105, param 0, identity_a = the inspected object.
pub fn inspect(char_id: i32, target: Identity) -> Vec<u8> {
    character_action(char_id, &CharacterAction { action: action::INSPECT, param: 0, identity_a: target, identity_b: Identity::default(), text: String::new() })
}

// ---------------------------------------------------------------------------------------------------------------
// FUN_1003fba6
// ---------------------------------------------------------------------------------------------------------------

/// What `FUN_1003fba6` needs to know about the game (all read from engine state in the original).
pub struct TextState<'a> {
    pub char_id: i32,
    /// `ChatWindowNode+0x1ec` (the `window` argument of `N3Msg_TextCommand`).
    pub window: i32,
    /// `n3EngineClientAnarchy_t+0x5c` = the current target ({0,0} = none). [INFERENCE: same object as `InputConfig_t+0xc0`.]
    pub target: Identity,
    /// `N3Msg_GetSkill(GmLevel 0xd7, 2) != 0`.
    pub gm: bool,
    /// `FUN_100657d1(teammanager)`.
    pub in_team: bool,
    /// own control identity == team leader identity (`FUN_10001ddc(own+0x14, team+0x14)`).
    pub team_leader: bool,
    /// `FUN_1002edf3`: stat name -> `Stat_e` (`_stricmp`), `None` if unknown (the original then uses 0x499602d2).
    pub stat_id: &'a dyn Fn(&str) -> Option<i32>,
    pub pet: PetState<'a>,
}

/// One entry of the own pet list (`dynel+0x1d8` -> `+0x1c`, filled by `AddPetIIR_c` / `RemovePetIIR_c`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pet {
    pub id: Identity,
    /// Name of the pet's dynel (`vtbl[0xe8]+0x34`); `None` when the dynel is unknown (`FUN_10058e36` null: never matches a name).
    pub name: Option<String>,
}

/// Engine state read by `/pet`, `/tower` and `/follow` (all `Default` = nothing known).
#[derive(Debug, Clone, Default)]
pub struct PetState<'a> {
    /// The own pet list (service towers are in the same list: `Feedback_YouHaveNoServiceTower` tests its size too).
    pub pets: &'a [Pet],
    /// `/follow`: name of the target dynel (`FUN_10058e36(target)` non-null); `None` = no such dynel, the command does nothing.
    pub target_name: Option<String>,
    /// `Features` (stat 0xE0) of the own / target dynel (`FUN_10044b6e`).
    pub own_features: u32,
    pub target_features: u32,
    /// District fight-mode level (`FUN_1003e228`; default 2 without district data) of the own / target dynel.
    pub own_fight_level: i32,
    pub target_fight_level: i32,
    /// Own vehicle exists and its `vtbl[0x90]` is true (`FUN_100574e2`; [UNRESOLVED] meaning of the slot).
    pub can_move: bool,
    /// Current movement FSM state (`FUN_100704e6`), 0 = unknown.
    pub move_mode: i32,
}

/// `FUN_1002edf3` result for an unknown name (`Stat_e` "invalid").
pub const STAT_INVALID: i32 = 0x499602d2;

/// A feedback line `FUN_10058b00(key, window)` prints: `LDBface::GetText(110, key)` (text.mdb category 110), with `%s` = `arg`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feedback {
    pub key: &'static str,
    pub arg: Option<String>,
}

/// GUI-side effects (dialogs) that have no wire message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Local {
    /// `/bank close`: closes the bank window (`FUN_10046bdd`).
    BankClose,
    /// `/raid listlocal`: `FUN_10065f00` (prints the local raid list).
    RaidListLocal,
    /// `/org leave` (confirmation dialog, `FUN_1000446b` signal) and `/org disband`.
    OrgLeaveDialog,
    OrgDisbandDialog,
    /// `/pet script <name>` / `/tower script <name>`: `FUN_10052230("scripts/<name>", pets, tower)` queues a pet script on the
    /// engine ([`PetScript`]); no `PetCommandIIR_c` is sent by the command itself.
    PetScript { path: String, pets: Vec<Identity>, tower: bool },
}

#[derive(Debug, Default, PartialEq)]
pub struct TextOut {
    /// N3 payloads (ptype 10), in send order.
    pub payloads: Vec<Vec<u8>>,
    pub feedback: Vec<Feedback>,
    pub local: Vec<Local>,
}

#[derive(Debug, PartialEq)]
pub enum TextResult {
    Handled(TextOut),
    /// First word not in `FUN_1003fba6` (or it does nothing for these words/arguments): the original sends and prints nothing.
    Ignored,
    /// A command of `FUN_1003fba6` whose layout is not decoded (docs/chat/cmd.md gaps): the local GM debug commands.
    Unsupported(&'static str),
}

/// `istringstream >> std::string` cursor.
struct Words<'a> {
    s: &'a str,
    pos: usize,
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

impl<'a> Words<'a> {
    fn new(s: &'a str) -> Self {
        Self { s, pos: 0 }
    }
    fn skip_ws(&mut self) {
        self.pos += self.s[self.pos..].len() - self.s[self.pos..].trim_start_matches(is_space).len();
    }
    /// `ss >> word` (empty at end of input).
    fn word(&mut self) -> &'a str {
        self.skip_ws();
        let rest = &self.s[self.pos..];
        let n = rest.find(is_space).unwrap_or(rest.len());
        self.pos += n;
        &rest[..n]
    }
    /// `ss >> ws; tellg` then the remainder of the line.
    fn rest(&mut self) -> &'a str {
        self.skip_ws();
        &self.s[self.pos..]
    }
}

/// `isdigit(word[0])` then `sscanf(word, "%u")`.
fn scan_u(word: &str) -> Option<u32> {
    if !word.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        return None;
    }
    let d: String = word.chars().take_while(char::is_ascii_digit).collect();
    Some(d.parse::<u64>().map_or(u32::MAX, |v| v.min(u32::MAX as u64) as u32))
}

/// `atoi`: optional blanks, sign, digits.
fn atoi(word: &str) -> i32 {
    let t = word.trim_start_matches(is_space);
    let (neg, t) = match t.as_bytes().first() {
        Some(b'-') => (true, &t[1..]),
        Some(b'+') => (false, &t[1..]),
        _ => (false, t),
    };
    let d: String = t.chars().take_while(char::is_ascii_digit).collect();
    let v = if d.is_empty() { 0 } else { d.parse::<i64>().unwrap_or(i64::MAX).min(i32::MAX as i64) as i32 };
    if neg { -v } else { v }
}

/// `strtoul(word, 0, 10)`.
fn strtoul(word: &str) -> u32 {
    let t = word.trim_start_matches(is_space);
    let d: String = t.chars().take_while(char::is_ascii_digit).collect();
    d.parse::<u64>().map_or(if d.is_empty() { 0 } else { u32::MAX }, |v| v.min(u32::MAX as u64) as u32)
}

/// `GameData` org sub-command table (Gamecode 0x100332aa.., map 0x102e2710, `_stricmp`-style lookup [INFERENCE: case-insensitive]).
pub fn org_code(word: &str) -> u8 {
    const T: [(&str, u8); 23] = [
        ("help", 1), ("create", 2), ("ranks", 3), ("governingform", 4), ("info", 5), ("promote", 6), ("demote", 7), ("name", 8),
        ("history", 9), ("description", 10), ("objective", 11), ("leave", 12), ("invite", 13), ("disband", 14), ("kick", 15),
        ("contract", 16), ("tax", 17), ("bank", 18), ("startvote", 19), ("vote", 20), ("stopvote", 21), ("debt", 22), ("city", 23),
    ];
    T.iter().find(|(n, _)| n.eq_ignore_ascii_case(word)).map_or(0, |t| t.1)
}

/// Raid sub-commands (map 0x102e2720).
fn raid_code(word: &str) -> i32 {
    const T: [(&str, i32); 6] = [("create", 1), ("list", 2), ("listlocal", 3), ("move", 4), ("lootaccess", 5), ("locks", 6)];
    T.iter().find(|(n, _)| n.eq_ignore_ascii_case(word)).map_or(0, |t| t.1)
}

/// Bank sub-commands (`bankcmd`, list 0x102e2630): open 1, close 2, info 3.
fn bank_code(word: &str) -> i32 {
    ["open", "close", "info"].iter().position(|n| n.eq_ignore_ascii_case(word)).map_or(0, |i| i as i32 + 1)
}

/// Team sub-commands (`teamcmd`, list 0x102e2660): team 1, loot 2, all 3, leader 4, alpha 5.
pub fn team_code(word: &str) -> i32 {
    ["team", "loot", "all", "leader", "alpha"].iter().position(|n| n.eq_ignore_ascii_case(word)).map_or(0, |i| i as i32 + 1)
}

fn id(kind: i32, instance: i32) -> Identity {
    Identity { kind, instance }
}

fn fb(key: &'static str) -> Feedback {
    Feedback { key, arg: None }
}

/// `PetCommandIIR_c` (key `6B333303`; ctor Gamecode 0x10076260, write slot 8 = 0x100760b7, read 0x100761a3): after the N3 header
/// `i32 window, i32 code, i32 arg, list<Identity> pets, i32 tower, i32 len, bytes text`. The list is `(n + 1) * 0x3f1` then `n`
/// identities (0x1003a527; empty = all pets); `tower` is the ctor's last argument (0 = `/pet`, 1 = `/tower`); `text` is the entry's
/// `+8` string (`FUN_10053ccd` keeps at most 255 bytes). The client's reader accepts `code` 1..=0x10 and `len` <= 250.
pub fn pet_command(char_id: i32, window: i32, code: i32, arg: i32, pets: &[Identity], tower: bool, text: &str) -> Vec<u8> {
    let mut w = Writer::default();
    header(&mut w, "PetCommandIIR_c", char_id);
    w.i32(window);
    w.i32(code);
    w.i32(arg);
    w.i32((pets.len() as i32 + 1) * 0x3f1);
    pets.iter().for_each(|p| p.write(&mut w));
    w.i32(i32::from(tower));
    let t = &text.as_bytes()[..text.len().min(255)];
    w.i32(t.len() as i32);
    w.bytes(t);
    w.0
}

/// A running pet script: the object built by `FUN_10055681` and advanced by `FUN_1005555f` (called every frame by the `NpcHolder_t` update
/// `FUN_100522c9` with the frame time). Lines are read as `<command>` lines of `/pet` (`FUN_10053d69` with no pet list: pet names in the
/// line are ignored, the pets are the ones addressed by `/pet <names> script <file>`), `delay <seconds>` and `#` comments.
#[derive(Debug, Clone, PartialEq)]
pub struct PetScript {
    /// `(delay before the command, entry)`, in file order (the deque at `this+8`).
    steps: std::collections::VecDeque<(f32, PetEntry)>,
    /// Seconds waited for the front step (`this+0xc`).
    elapsed: f32,
    pets: Vec<Identity>,
    tower: bool,
}

impl PetScript {
    /// `FUN_10055681`. `getline` reads at most 511 bytes per line (`0x200` buffer; a longer line sets failbit and ends the read); blank
    /// space before a line is skipped (`>> ws`); a line whose first byte is `#` is a comment; `delay ` (`_strnicmp`, 6 bytes) adds
    /// `atof(rest)` to the pending delay; any other line is parsed, an invalid one is skipped, a valid one is queued with the pending
    /// delay, which is then reset to 0. A trailing delay without a command is dropped.
    pub fn parse(src: &str, pets: Vec<Identity>, tower: bool) -> Self {
        let (mut steps, mut pending) = (std::collections::VecDeque::new(), 0f32);
        for line in src.lines() {
            let line = line.trim_start_matches(|c: char| c.is_ascii_whitespace());
            if line.len() >= 0x200 {
                break;
            }
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.as_bytes().get(..6).is_some_and(|h| h.eq_ignore_ascii_case(b"delay ")) {
                pending += atof(&line[6..]);
            } else if let Some(e) = pet_entry(&mut pet_words(line.as_bytes())) {
                steps.push_back((pending, e));
                pending = 0.0;
            }
        }
        Self { steps, elapsed: 0.0, pets, tower }
    }

    /// `FUN_1005555f(dt)`: `None` = the script is finished (queue empty; the owner deletes it). While `elapsed <= delay` of the front
    /// step the time accumulates; otherwise the step is sent (`PetCommandIIR_c`, window 0, the script's pets) and popped, `elapsed` = 0.
    pub fn step(&mut self, dt: f32, char_id: i32) -> Option<Option<Vec<u8>>> {
        let (delay, _) = self.steps.front()?;
        if self.elapsed <= *delay {
            self.elapsed += dt;
            return Some(None);
        }
        let (_, e) = self.steps.pop_front()?;
        self.elapsed = 0.0;
        Some(Some(pet_command(char_id, 0, e.code, e.arg, &self.pets, self.tower, &e.text)))
    }
}

/// C `atof`: the longest numeric prefix after leading blanks (0 when there is none).
fn atof(s: &str) -> f32 {
    let s = s.trim_start();
    (1..=s.len()).rev().filter(|&n| s.is_char_boundary(n)).find_map(|n| s[..n].parse::<f32>().ok().filter(|_| !s[..n].ends_with(['e', 'E']))).unwrap_or(0.0)
}

/// `FollowTargetIIR_c` as `/follow` builds it (ctor Gamecode 0x100734f8 called at 0x1004206b, write 0x10073030): header = own dynel,
/// flag 1 (`EnablePassOn` at the end of the ctor), target = the current target, speed 2.5 (`[0x1015d87c]`), mode 0, position 0,0,0,
/// no waypoints. The speed is non-zero so the writer always takes the long form.
pub fn follow_target(char_id: i32, target: Identity) -> Vec<u8> {
    let f = FollowTarget { form: 2, mode: 0, target, speed: FOLLOW_SPEED, pos: Vec3::default(), path: Vec::new() };
    Misc::FollowTarget(f).encode(id(DYNEL_CHAR, char_id), 1)
}

/// `/follow` speed argument of the ctor (`[0x1015d87c]` = 0x40200000).
pub const FOLLOW_SPEED: f32 = 2.5;

/// `FUN_1003fba6(window, line, target)`; `line` has no slash and is already `ExpandChatTextArgs`'d.
pub fn text_command(line: &str, st: &TextState) -> TextResult {
    let mut ss = Words::new(line);
    let first = ss.word();
    let act = |action: i32, param: i32, a: Identity, b: Identity| {
        character_action(st.char_id, &CharacterAction { action, param, identity_a: a, identity_b: b, text: String::new() })
    };
    let zero = Identity::default();
    let sent = |p: Vec<Vec<u8>>| TextResult::Handled(TextOut { payloads: p, ..Default::default() });
    match first {
        "version" => sent(vec![act(action::VERSION, st.window, zero, zero)]),
        "items" => {
            let w = ss.word();
            if w.is_empty() || w == "list" {
                sent(vec![act(action::ITEMS_LIST, 0, zero, zero)])
            } else if w == "delete" {
                // sscanf into the window argument's slot: a non-numeric word leaves `n = window`
                let n = scan_u(ss.word()).map_or(st.window, |v| v as i32);
                sent(vec![act(action::ITEMS_DELETE, 0, zero, id(0, n))])
            } else {
                TextResult::Ignored
            }
        }
        "resetskill" | "clearunique" => {
            if !st.gm {
                return TextResult::Ignored;
            }
            let w = ss.word();
            let n = scan_u(w).map_or_else(|| (st.stat_id)(w).unwrap_or(STAT_INVALID), |v| v as i32);
            let a = if first == "resetskill" { action::RESET_SKILL } else { action::CLEAR_UNIQUE };
            sent(vec![act(a, st.window, zero, id(0, n))])
        }
        "bank" => match bank_code(ss.word()) {
            1 if st.gm => sent(vec![act(action::BANK_OPEN, st.window, zero, zero)]),
            2 => TextResult::Handled(TextOut { local: vec![Local::BankClose], ..Default::default() }),
            3 => sent(vec![act(action::BANK_INFO, st.window, zero, zero)]),
            _ => TextResult::Ignored,
        },
        "clone" if st.gm => sent(vec![act(action::CLONE, st.window, zero, zero)]),
        "played" => sent(vec![act(action::PLAYED, st.window, zero, zero)]),
        "born" => sent(vec![act(action::BORN, st.window, zero, zero)]),
        "damagemult" if st.gm => {
            let w = ss.word();
            let b = if w == "list" { zero } else { id(0, scan_u(w).map_or(100, |v| v as i32)) };
            sent(vec![act(action::DAMAGE_MULT, st.window, zero, b)])
        }
        "team" => team(&mut ss, st),
        "raid" => raid(&mut ss, st),
        "org" => org_cmd(&mut ss, st),
        "pet" => pet_cmd(false, line, st),
        "tower" => pet_cmd(true, line, st),
        "follow" => follow(st),
        "getlocal" | "setlocal" | "getlocalfull" | "criterialocal" | "spelllocal" | "monsterdata" | "joycamacc" | "tplocal" if st.gm => {
            TextResult::Unsupported("gm debug command")
        }
        _ => TextResult::Ignored,
    }
}

/// Gamecode 0x100406df..0x100408d4.
fn team(ss: &mut Words, st: &TextState) -> TextResult {
    let zero = Identity::default();
    if !st.in_team {
        return TextResult::Handled(TextOut { feedback: vec![fb("Feedback_YouAreNotMemberOfTeam")], ..Default::default() });
    }
    let w1 = ss.word();
    if w1.is_empty() {
        let feedback = vec![fb("Feedback_AvailableTeamCommands"), fb("Feedback_TeamLoot"), fb("Feedback_TeamLootAll")];
        return TextResult::Handled(TextOut { feedback, ..Default::default() });
    }
    if team_code(w1) != 2 {
        return TextResult::Ignored;
    }
    let w2 = ss.word();
    let (action, a) = if w2.is_empty() {
        (action::TEAM_LOOT, zero)
    } else if st.team_leader {
        (action::TEAM_LOOT_SET, id(0, team_code(w2)))
    } else {
        return TextResult::Handled(TextOut { feedback: vec![fb("Feedback_OnlyTeamLeaderCanChangeLootOrder")], ..Default::default() });
    };
    let p = character_action(st.char_id, &CharacterAction { action, param: st.window, identity_a: a, identity_b: zero, text: String::new() });
    TextResult::Handled(TextOut { payloads: vec![p], ..Default::default() })
}

/// Gamecode 0x100404bb..0x100406d4.
fn raid(ss: &mut Words, st: &TextState) -> TextResult {
    let zero = Identity::default();
    let cmd = raid_code(ss.word());
    let (a, b, c) = match cmd {
        0 => return TextResult::Ignored,
        3 => return TextResult::Handled(TextOut { local: vec![Local::RaidListLocal], ..Default::default() }),
        4 => {
            let slot = strtoul(ss.word()) as i32;
            let group = atoi(ss.word());
            if group as u32 > 6 {
                return TextResult::Ignored;
            }
            (id(slot, group), zero, zero)
        }
        5 => {
            let (x, y) = (atoi(ss.word()), atoi(ss.word()));
            let z = strtoul(ss.word()) as i32;
            (id(RAID_KIND, x), id(y, 0), id(DYNEL_CHAR, z))
        }
        _ => (zero, zero, zero),
    };
    TextResult::Handled(TextOut { payloads: vec![raid_cmd(st.char_id, cmd, a, b, c)], ..Default::default() })
}

/// Gamecode 0x100408e3..0x10041491.
fn org_cmd(ss: &mut Words, st: &TextState) -> TextResult {
    let zero = Identity::default();
    let code = org_code(ss.word());
    let rest = ss.rest();
    // every byte of the rest must be printable ASCII; offenders are listed (once each) in CannotUseLettersX
    let mut bad = String::new();
    for ch in rest.chars() {
        if !(' '..='~').contains(&ch) && !bad.contains(ch) {
            bad.push(ch);
        }
    }
    if !bad.is_empty() {
        return TextResult::Handled(TextOut { feedback: vec![Feedback { key: "CannotUseLettersX", arg: Some(bad) }], ..Default::default() });
    }
    let help = || TextResult::Handled(TextOut { feedback: vec![fb("OrgCommandHelp")], ..Default::default() });
    let t = st.target;
    // (wire code, identity, text)
    let (c, who, text): (u8, Identity, &str) = match code {
        2 => (org::CREATE, t, rest),
        3 => (org::RANKS, zero, ""),
        4 => (org::GOVERNING_FORM, t, rest),
        5 => (org::INFO, t, ""),
        6 => (org::PROMOTE, t, ""),
        7 => (org::DEMOTE, t, ""),
        8 => (org::NAME, t, rest),
        9 => (org::HISTORY, t, rest),
        10 => (org::DESCRIPTION, t, rest),
        11 => (org::OBJECTIVE, t, rest),
        12 => return TextResult::Handled(TextOut { local: vec![Local::OrgLeaveDialog], ..Default::default() }),
        13 => (org::INVITE, t, ""),
        14 => return TextResult::Handled(TextOut { local: vec![Local::OrgDisbandDialog], ..Default::default() }),
        15 if rest.is_empty() => (org::KICK_TARGET, t, ""),
        15 => (org::KICK_NAME, t, rest),
        16 => (org::CONTRACT, zero, ""),
        17 => (org::TAX, t, rest),
        18 => {
            let mut s = Words::new(rest);
            let (a, b) = (s.word(), s.word());
            if a.is_empty() {
                (org::BANK, t, "")
            } else if a == "add" && !b.is_empty() {
                (org::BANK_ADD, t, b)
            } else if a == "remove" && !b.is_empty() {
                (org::BANK_REMOVE, t, b)
            } else {
                return help();
            }
        }
        19 if rest.is_empty() => return TextResult::Handled(TextOut { feedback: vec![fb("OrgVoteHelp")], ..Default::default() }),
        19 => (org::STARTVOTE, t, rest),
        20 if rest.is_empty() => return TextResult::Handled(TextOut { feedback: vec![fb("OrgVoteHelpInfo")], ..Default::default() }),
        20 if rest == "info" => (org::VOTE_INFO, t, rest),
        20 => (org::VOTE, t, rest),
        21 => (org::STOPVOTE, t, rest),
        22 => (org::DEBT, zero, ""),
        23 => (org::CITY, zero, ""),
        _ => return help(),
    };
    TextResult::Handled(TextOut { payloads: vec![org_client(st.char_id, st.window, c, who, text)], ..Default::default() })
}

/// `FUN_10053d69` map 1 (`DAT_102e30a8`, command is the last word): follow 1, behind 2, survive 3, wait 4, guard 6, attack 7,
/// terminate 10, free 11, heal 12, report 14. `std::map<string,int>`: case-sensitive.
fn pet_code1(w: &[u8]) -> Option<i32> {
    let n = ["follow", "behind", "survive", "wait", "", "guard", "attack", "", "", "terminate", "free", "heal", "", "report"];
    n.iter().position(|&c| !c.is_empty() && c.as_bytes() == w).map(|i| i as i32 + 1)
}

/// `FUN_10053d69` map 2 (`DAT_102e30ac`, the command precedes its argument word): cycle 5, social 9, rename 15, chat 16, script 17.
fn pet_code2(w: &[u8]) -> Option<i32> {
    [(&b"cycle"[..], 5), (b"social", 9), (b"rename", 15), (b"chat", 16), (b"script", 17)].iter().find(|(n, _)| *n == w).map(|&(_, c)| c)
}

/// The word splitter of `FUN_10053d69` (0x10054245..0x10054362): skips blanks (only `' '`); a word starting with `"` runs to the
/// next `"` (or the end), any other ends at the next blank or `"` (the delimiter is consumed).
fn pet_words(s: &[u8]) -> Vec<Vec<u8>> {
    let find = |c: u8, from: usize| s.get(from..).and_then(|t| t.iter().position(|&x| x == c)).map(|i| i + from);
    let (mut v, mut pos) = (Vec::new(), 0);
    while pos < s.len() {
        while pos < s.len() && s[pos] == b' ' {
            pos += 1;
        }
        if pos >= s.len() {
            break;
        }
        let (a, b) = (find(b' ', pos), find(b'"', pos + 1));
        let end = if s[pos] == b'"' { b } else { a.into_iter().chain(b).min() };
        let from = pos + usize::from(s[pos] == b'"');
        v.push(s[from..end.unwrap_or(s.len())].to_vec());
        match end {
            Some(e) => pos = e + 1,
            None => break,
        }
    }
    v
}

/// `String::StripSpecialChars(s, false)` [Utils 0x1000d764]: drops `0x10` and the char after it, and everything between `0x11` and `0x12`.
fn strip_special(s: &str) -> String {
    let (mut out, mut skip, mut it) = (String::new(), false, s.chars());
    while let Some(c) = it.next() {
        match c {
            '\x10' => {
                it.next();
            }
            '\x11' => skip = true,
            '\x12' => skip = false,
            _ if !skip => out.push(c),
            _ => {}
        }
    }
    out
}

/// `String::CompareNoCase(a, b, -1) == 0` [Utils 0x1000de83]: `towupper` of every code point.
fn eq_nocase(a: &str, b: &str) -> bool {
    let up = |s: &str| s.chars().map(|c| c.to_uppercase().next().unwrap_or(c)).collect::<Vec<_>>();
    up(a) == up(b)
}

/// The command entry `{code, arg, text}` of `FUN_10053d69` (12 bytes, `FUN_10053d17`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PetEntry {
    pub code: i32,
    pub arg: i32,
    pub text: String,
}

/// Command part of `FUN_10053d69`: pops the command (and its argument word) off `words`, leaving the pet names. `None` = invalid (NULL).
fn pet_entry(words: &mut Vec<Vec<u8>>) -> Option<PetEntry> {
    let last = words.last().cloned()?;
    let (mut code, mut arg, mut text) = (pet_code1(&last).unwrap_or(0), 0, String::new());
    if code == 0 && words.len() >= 2 {
        words.pop();
        if let Some(c) = pet_code2(words.last().unwrap()) {
            code = c;
            let a = String::from_utf8_lossy(&last).into_owned();
            match c {
                5 => match last.first().map(u8::to_ascii_lowercase) {
                    Some(b'a') => arg = 1,
                    Some(b'w') => arg = 0,
                    _ => code = 0,
                },
                9 => {
                    arg = super::action::emote_by_name(&a).map_or(0, |e| e.id);
                    if arg == 0 {
                        code = 0;
                    }
                }
                _ => text = a,
            }
        }
    }
    words.pop();
    (code != 0).then_some(PetEntry { code, arg, text })
}

/// `/pet` and `/tower` (Gamecode 0x10041ca8 / 0x10041df4): `text` = the line after `"pet "` / `"tower "` (byte offsets 4 / 6),
/// `FUN_10053d69` parses it into `{code, arg, text}` plus the list of the addressed pets.
fn pet_cmd(tower: bool, line: &str, st: &TextState) -> TextResult {
    let invalid = || TextResult::Handled(TextOut { feedback: vec![fb(if tower { "Feedback_InvalidTowerCommand" } else { "Feedback_InvalidPetcommand" })], ..Default::default() });
    let p = &st.pet;
    if p.pets.is_empty() {
        return TextResult::Handled(TextOut { feedback: vec![fb(if tower { "Feedback_YouHaveNoServiceTower" } else { "Feedback_YouHaveNoPet" })], ..Default::default() });
    }
    // strlen > 4 (also for "tower": a bare "tower" then reads past the terminator in the original [UNDEFINED]: treated as empty)
    if line.len() <= 4 {
        return invalid();
    }
    let mut words = pet_words(line.as_bytes().get(if tower { 6 } else { 4 }..).unwrap_or_default());
    let Some(PetEntry { code, arg, text }) = pet_entry(&mut words) else { return invalid() };
    let names: Vec<String> = words.iter().map(|w| String::from_utf8_lossy(w).into_owned()).collect();
    let mut ids = Vec::new();
    match names.first() {
        // `rename` without a pet name: `FUN_10058816(own)+0x5c` [INFERENCE: the current target]
        None if code == 0xf => ids.push(st.target),
        None => {}
        Some(n) if n.eq_ignore_ascii_case("all") => {}
        Some(_) => {
            for n in &names {
                let n = strip_special(n);
                ids.extend(p.pets.iter().filter(|x| x.name.as_deref().is_some_and(|m| eq_nocase(&n, &strip_special(m)))).map(|x| x.id));
            }
            if ids.is_empty() {
                return invalid();
            }
        }
    }
    if code == 0x11 {
        return TextResult::Handled(TextOut { local: vec![Local::PetScript { path: format!("scripts/{text}"), pets: ids, tower }], ..Default::default() });
    }
    TextResult::Handled(TextOut { payloads: vec![pet_command(st.char_id, st.window, code, arg, &ids, tower, &text)], ..Default::default() })
}

/// `/follow` (Gamecode 0x10041f02..0x10042187): follow the current target.
fn follow(st: &TextState) -> TextResult {
    let p = &st.pet;
    let say = |f: Feedback| TextResult::Handled(TextOut { feedback: vec![f], ..Default::default() });
    let Some(name) = &p.target_name else { return TextResult::Handled(TextOut::default()) };
    let blocked = |level: i32, features: u32| level > 1 || features & 1 != 0 || features & 0x400_0000 != 0;
    if blocked(p.own_fight_level, p.own_features) || blocked(p.target_fight_level, p.target_features) {
        return say(fb("Feedback_CantFollow"));
    }
    if !p.can_move || matches!(p.move_mode, 1 | 8 | 9 | 0xb | 0xc) {
        return say(fb("Feedback_YouCantMove"));
    }
    TextResult::Handled(TextOut {
        payloads: vec![follow_target(st.char_id, st.target)],
        feedback: vec![Feedback { key: "FollowingX", arg: Some(name.clone()) }],
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------------------------------------------
// tests (byte layouts follow the read/write functions cited above)
// ---------------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::action::parse_character_action;

    fn state<'a>(stat: &'a dyn Fn(&str) -> Option<i32>) -> TextState<'a> {
        TextState { char_id: 0x1234, window: 7, target: id(DYNEL_CHAR, 99), gm: false, in_team: true, team_leader: true, stat_id: stat, pet: PetState::default() }
    }
    fn no_stat(_: &str) -> Option<i32> {
        None
    }
    fn one(r: TextResult) -> Vec<u8> {
        match r {
            TextResult::Handled(o) if o.payloads.len() == 1 => o.payloads.into_iter().next().unwrap(),
            other => panic!("{other:?}"),
        }
    }
    fn body(p: &[u8]) -> &[u8] {
        &p[13..]
    }

    #[test]
    fn version_is_a_character_action_with_the_window_as_param() {
        let st = state(&no_stat);
        let p = one(text_command("version", &st));
        let (h, a) = parse_character_action(&p).unwrap();
        assert_eq!(h.target, id(DYNEL_CHAR, 0x1234));
        assert_eq!(h.flag, 0, "ClearToBePassedOn");
        assert_eq!((a.action, a.param, a.identity_a, a.identity_b, a.text.as_str()), (0x9d, 7, Identity::default(), Identity::default(), ""));
    }

    #[test]
    fn items_and_case_sensitivity() {
        let st = state(&no_stat);
        let (_, a) = parse_character_action(&one(text_command("items", &st))).unwrap();
        assert_eq!((a.action, a.param), (0xf4, 0));
        let (_, a) = parse_character_action(&one(text_command("items list", &st))).unwrap();
        assert_eq!(a.action, 0xf4);
        let (_, a) = parse_character_action(&one(text_command("items delete 12", &st))).unwrap();
        assert_eq!((a.action, a.identity_b), (0xf5, id(0, 12)));
        // a non-numeric slot keeps the window id (sscanf leaves the argument slot untouched)
        let (_, a) = parse_character_action(&one(text_command("items delete x", &st))).unwrap();
        assert_eq!(a.identity_b, id(0, 7));
        assert_eq!(text_command("items bogus", &st), TextResult::Ignored);
        assert_eq!(text_command("VERSION", &st), TextResult::Ignored, "std::string::compare is case-sensitive");
        assert_eq!(text_command("", &st), TextResult::Ignored);
    }

    #[test]
    fn gm_gated_commands() {
        let stat = |n: &str| (n == "Strength").then_some(16);
        let mut st = state(&stat);
        assert_eq!(text_command("clone", &st), TextResult::Ignored);
        st.gm = true;
        let (_, a) = parse_character_action(&one(text_command("clone", &st))).unwrap();
        assert_eq!(a.action, 0x96);
        let (_, a) = parse_character_action(&one(text_command("resetskill 5", &st))).unwrap();
        assert_eq!((a.action, a.identity_b), (0x9a, id(0, 5)));
        let (_, a) = parse_character_action(&one(text_command("resetskill Strength", &st))).unwrap();
        assert_eq!(a.identity_b, id(0, 16));
        let (_, a) = parse_character_action(&one(text_command("clearunique nosuchstat", &st))).unwrap();
        assert_eq!((a.action, a.identity_b), (0xae, id(0, STAT_INVALID)));
        let (_, a) = parse_character_action(&one(text_command("damagemult", &st))).unwrap();
        assert_eq!((a.action, a.identity_b), (0x67, id(0, 100)));
        let (_, a) = parse_character_action(&one(text_command("damagemult 250", &st))).unwrap();
        assert_eq!(a.identity_b, id(0, 250));
        let (_, a) = parse_character_action(&one(text_command("damagemult list", &st))).unwrap();
        assert_eq!(a.identity_b, Identity::default());
    }

    #[test]
    fn team_loot() {
        let mut st = state(&no_stat);
        let (_, a) = parse_character_action(&one(text_command("team loot", &st))).unwrap();
        assert_eq!((a.action, a.param), (0x91, 7));
        let (_, a) = parse_character_action(&one(text_command("team loot leader", &st))).unwrap();
        assert_eq!((a.action, a.identity_a), (0x7f, id(0, 4)));
        st.team_leader = false;
        let TextResult::Handled(o) = text_command("team loot all", &st) else { panic!() };
        assert_eq!(o.feedback, vec![fb("Feedback_OnlyTeamLeaderCanChangeLootOrder")]);
        let TextResult::Handled(o) = text_command("team", &st) else { panic!() };
        assert_eq!(o.feedback.len(), 3);
        st.in_team = false;
        let TextResult::Handled(o) = text_command("team loot", &st) else { panic!() };
        assert_eq!(o.feedback, vec![fb("Feedback_YouAreNotMemberOfTeam")]);
    }

    #[test]
    fn raid_commands() {
        let st = state(&no_stat);
        let p = one(text_command("raid move 3 2", &st));
        let mut want = message_key("RaidCmdIIR_c").to_be_bytes().to_vec();
        want.extend([0, 0, 0xC3, 0x50, 0, 0, 0x12, 0x34, 0]); // header identity + passed-on
        want.extend([0, 0, 0, 4, 0, 0, 0, 3, 0, 0, 0, 2]); // cmd 4, id {3, 2}
        assert_eq!(p, want);
        let p = one(text_command("raid lootaccess 5 6 77", &st));
        let b = body(&p);
        assert_eq!(b.len(), 4 + 8 * 3);
        assert_eq!(&b[..12], &[0, 0, 0, 5, 0, 0, 0xC7, 0x6A, 0, 0, 0, 5]);
        assert_eq!(&b[12..20], &[0, 0, 0, 6, 0, 0, 0, 0]);
        assert_eq!(&b[20..], &[0, 0, 0xC3, 0x50, 0, 0, 0, 77]);
        assert_eq!(body(&one(text_command("raid create", &st))).len(), 4 + 8, "only cmd 5 carries b and c");
        assert_eq!(text_command("raid move 1 9", &st), TextResult::Ignored);
        assert_eq!(text_command("raid nonsense", &st), TextResult::Ignored);
        let TextResult::Handled(o) = text_command("raid listlocal", &st) else { panic!() };
        assert_eq!(o.local, vec![Local::RaidListLocal]);
    }

    #[test]
    fn org_wire_layout() {
        let st = state(&no_stat);
        let p = one(text_command("org create My Org", &st));
        let mut want = message_key("OrgClientIIR_c").to_be_bytes().to_vec();
        want.extend([0, 0, 0xC3, 0x50, 0, 0, 0x12, 0x34, 0]);
        want.push(1); // create
        want.extend([0, 0, 0xC3, 0x50, 0, 0, 0, 99]); // current target
        want.extend([0, 0, 0, 7]); // window
        want.extend([0, 6]);
        want.extend(b"My Org");
        assert_eq!(p, want);
        // promote carries a flag byte instead of a text
        let b = one(text_command("org promote", &st));
        assert_eq!(body(&b), &[0x0a, 0, 0, 0xC3, 0x50, 0, 0, 0, 99, 0, 0, 0, 7, 0]);
        // no text for invite / ranks (ranks: zero identity)
        assert_eq!(body(&one(text_command("org invite", &st))).len(), 1 + 8 + 4);
        assert_eq!(&body(&one(text_command("org ranks", &st)))[..9], &[2, 0, 0, 0, 0, 0, 0, 0, 0]);
        // kick: no argument kicks the target, a name is sent as text with another code
        assert_eq!(body(&one(text_command("org kick", &st)))[0], 0x0c);
        let p = one(text_command("org kick Bob", &st));
        assert_eq!((body(&p)[0], &body(&p)[13..]), (0x0d, &[0, 3, b'B', b'o', b'b'][..]));
        // bank sub-commands
        assert_eq!(body(&one(text_command("org bank", &st)))[0], 0x12);
        let p = one(text_command("org bank add 100", &st));
        assert_eq!((body(&p)[0], &body(&p)[13..]), (0x13, &[0, 3, b'1', b'0', b'0'][..]));
        assert_eq!(body(&one(text_command("org bank remove 5", &st)))[0], 0x14);
        // vote
        assert_eq!(body(&one(text_command("org vote info", &st))).len(), 1 + 8 + 4, "code 8 has no text");
        assert_eq!(body(&one(text_command("org vote 3", &st)))[0], 9);
        assert_eq!(body(&one(text_command("org startvote Q 10 a;b", &st)))[0], 7);
        assert_eq!(body(&one(text_command("org STOPVOTE 3", &st)))[0], 0x1c, "org sub-commands are matched case-insensitively");
    }

    #[test]
    fn org_feedback() {
        let st = state(&no_stat);
        let fbk = |l: &str| match text_command(l, &st) {
            TextResult::Handled(o) => (o.feedback, o.payloads.len(), o.local),
            r => panic!("{r:?}"),
        };
        assert_eq!(fbk("org").0, vec![fb("OrgCommandHelp")]);
        assert_eq!(fbk("org frobnicate").0, vec![fb("OrgCommandHelp")]);
        assert_eq!(fbk("org help").0, vec![fb("OrgCommandHelp")]);
        assert_eq!(fbk("org vote").0, vec![fb("OrgVoteHelpInfo")]);
        assert_eq!(fbk("org startvote").0, vec![fb("OrgVoteHelp")]);
        assert_eq!(fbk("org bank oops").0, vec![fb("OrgCommandHelp")]);
        assert_eq!(fbk("org create h\u{e9}llo caf\u{e9}").0, vec![Feedback { key: "CannotUseLettersX", arg: Some("\u{e9}".into()) }]);
        assert_eq!(fbk("org leave").2, vec![Local::OrgLeaveDialog]);
        assert_eq!(fbk("org disband").2, vec![Local::OrgDisbandDialog]);
        assert_eq!(fbk("org create x").1, 1);
    }

    #[test]
    fn fanatic_and_inspect_layouts() {
        let p = fanatic(0x1234, 7, id(DYNEL_CHAR, 99), "stuck");
        let mut want = message_key("FanaticIIR_t").to_be_bytes().to_vec();
        want.extend([0, 0, 0xC3, 0x50, 0, 0, 0x12, 0x34, 0]);
        want.extend([0, 0, 0, 7, 0, 0, 0xC3, 0x50, 0, 0, 0, 99, 0, 0, 0, 5]);
        want.extend(b"stuck");
        assert_eq!(p, want);
        let (_, a) = parse_character_action(&inspect(0x1234, id(50000, 5))).unwrap();
        assert_eq!((a.action, a.param, a.identity_a, a.identity_b), (0x105, 0, id(50000, 5), Identity::default()));
    }

    #[test]
    fn stream_helpers() {
        let mut s = Words::new("  org   create  two words ");
        assert_eq!((s.word(), s.word(), s.rest()), ("org", "create", "two words "));
        assert_eq!(s.word(), "two", "rest() does not consume");
        assert_eq!((atoi("-12x"), atoi("x"), strtoul("42 "), strtoul("x"), scan_u("7z"), scan_u("-1")), (-12, 0, 42, 0, Some(7), None));
        assert_eq!(team_code("LOOT"), 2);
        assert_eq!(org_code("Kick"), 15);
    }
}

#[cfg(test)]
mod pet_tests {
    use super::*;

    fn pets() -> Vec<Pet> {
        vec![Pet { id: id(DYNEL_CHAR, 500), name: Some("Rex".into()) }, Pet { id: id(DYNEL_CHAR, 501), name: Some("\u{11}ff0000\u{12}Fido Jr".into()) }]
    }
    fn st<'a>(pets: &'a [Pet], stat: &'a dyn Fn(&str) -> Option<i32>) -> TextState<'a> {
        TextState {
            char_id: 0x1234,
            window: 7,
            target: id(DYNEL_CHAR, 99),
            gm: false,
            in_team: false,
            team_leader: false,
            stat_id: stat,
            pet: PetState { pets, target_name: Some("Bob".into()), can_move: true, ..Default::default() },
        }
    }
    fn none(_: &str) -> Option<i32> {
        None
    }
    fn payload(r: TextResult) -> Vec<u8> {
        match r {
            TextResult::Handled(o) if o.payloads.len() == 1 => o.payloads.into_iter().next().unwrap(),
            other => panic!("{other:?}"),
        }
    }
    fn fbk(r: TextResult) -> Vec<&'static str> {
        match r {
            TextResult::Handled(o) => o.feedback.iter().map(|f| f.key).collect(),
            other => panic!("{other:?}"),
        }
    }
    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn keys_match_the_registry() {
        assert_eq!(message_key("PetCommandIIR_c"), 0x6B33_3303);
        assert_eq!(message_key("FollowTargetIIR_c"), 0x260F_3671);
    }

    #[test]
    fn pet_attack_for_all_pets() {
        let (p, s) = (pets(), none);
        let b = payload(text_command("pet attack", &st(&p, &s)));
        // key, {0xC350, 0x1234}, passed-on 0 | window 7, code 7, arg 0, list (0+1)*0x3f1, tower 0, len 0
        assert_eq!(hex(&b), "6b333303 0000c350 00001234 00 00000007 00000007 00000000 000003f1 00000000 00000000".replace(' ', ""));
    }

    #[test]
    fn pet_rename_by_name_strips_markup_and_ignores_case() {
        let (p, s) = (pets(), none);
        let b = payload(text_command("pet rex rename Max", &st(&p, &s)));
        assert_eq!(
            hex(&b[13..]),
            "00000007 0000000f 00000000 000007e2 0000c350 000001f4 00000000 00000003 4d6178".replace(' ', "")
        );
        // quoted name with a blank, matched against the markup-stripped dynel name
        let b = payload(text_command("pet \"fido jr\" heal", &st(&p, &s)));
        assert_eq!(hex(&b[13..]), "00000007 0000000c 00000000 000007e2 0000c350 000001f5 00000000 00000000".replace(' ', ""));
        // an unknown name drops the command
        assert_eq!(fbk(text_command("pet Nobody attack", &st(&p, &s))), ["Feedback_InvalidPetcommand"]);
        // `all` addresses every pet (empty list)
        let b = payload(text_command("pet ALL guard", &st(&p, &s)));
        assert_eq!(hex(&b[13..13 + 16]), "00000007 00000006 00000000 000003f1".replace(' ', ""));
        // rename without a pet name sends the current target [INFERENCE]
        let b = payload(text_command("pet rename Zed", &st(&p, &s)));
        assert_eq!(hex(&b[17..]), "0000000f 00000000 000007e2 0000c350 00000063 00000000 00000003 5a6564".replace(' ', ""));
    }

    #[test]
    fn pet_arguments() {
        let (p, s) = (pets(), none);
        let arg = |l: &str| i32::from_be_bytes(payload(text_command(l, &st(&p, &s)))[21..25].try_into().unwrap());
        assert_eq!(arg("pet cycle a"), 1);
        assert_eq!(arg("pet cycle Wait"), 0);
        assert_eq!(fbk(text_command("pet cycle x", &st(&p, &s))), ["Feedback_InvalidPetcommand"]);
        let bow = crate::n3::action::emote_by_name("bow").unwrap().id;
        assert_eq!(arg("pet social BOW"), bow);
        assert_eq!(fbk(text_command("pet social nosuchemote", &st(&p, &s))), ["Feedback_InvalidPetcommand"]);
    }

    #[test]
    fn tower_sets_the_tower_flag() {
        let (p, s) = (pets(), none);
        let b = payload(text_command("tower terminate", &st(&p, &s)));
        assert_eq!(hex(&b[13..]), "00000007 0000000a 00000000 000003f1 00000001 00000000".replace(' ', ""));
        assert_eq!(fbk(text_command("tower", &st(&p, &s))), ["Feedback_InvalidTowerCommand"]);
    }

    #[test]
    fn pet_gates_and_scripts() {
        let s = none;
        assert_eq!(fbk(text_command("pet attack", &st(&[], &s))), ["Feedback_YouHaveNoPet"]);
        assert_eq!(fbk(text_command("tower attack", &st(&[], &s))), ["Feedback_YouHaveNoServiceTower"]);
        let p = pets();
        for l in ["pet", "pet ", "pet x", "pet \"", "pets attack"] {
            let r = text_command(l, &st(&p, &s));
            if l == "pets attack" {
                assert_eq!(r, TextResult::Ignored);
            } else {
                assert_eq!(fbk(r), ["Feedback_InvalidPetcommand"], "{l}");
            }
        }
        match text_command("pet Rex script fight.txt", &st(&p, &s)) {
            TextResult::Handled(o) => {
                assert!(o.payloads.is_empty());
                assert_eq!(o.local, [Local::PetScript { path: "scripts/fight.txt".into(), pets: vec![id(DYNEL_CHAR, 500)], tower: false }]);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn pet_script_runs_commands_with_delays_one_per_frame() {
        let src = "# a comment\n  attack\r\nDELAY 0.5\ndelay 0.25\nRex cycle aggressive\nbogus line\ndelay 9\n";
        let mut s = PetScript::parse(src, vec![id(DYNEL_CHAR, 500)], false);
        assert_eq!(s.steps.iter().map(|(d, e)| (*d, e.code, e.arg)).collect::<Vec<_>>(), [(0.0, 7, 0), (0.75, 5, 1)]);
        let mut sent = |dt| s.step(dt, 0x1234).map(|o| o.map(|b| (b[13..17].to_vec(), b[17..21].to_vec())));
        // front delay 0: the first frame accumulates (0 <= 0), the second sends attack (window 0, code 7)
        assert_eq!(sent(0.1), Some(None));
        assert_eq!(sent(0.1), Some(Some((vec![0, 0, 0, 0], vec![0, 0, 0, 7]))));
        // cycle waits 0.75 s: 0.5 + 0.25 accumulate, the frame after sends it, then the queue is empty (the trailing delay is dropped)
        assert_eq!(sent(0.5), Some(None));
        assert_eq!(sent(0.25), Some(None));
        assert_eq!(sent(0.01), Some(None));
        assert_eq!(sent(0.01), Some(Some((vec![0, 0, 0, 0], vec![0, 0, 0, 5]))));
        assert_eq!(sent(0.1), None);
        assert_eq!((atof("  1.5abc"), atof("x"), atof("2e")), (1.5, 0.0, 2.0));
    }

    #[test]
    fn word_splitter() {
        let w = |s: &str| pet_words(s.as_bytes()).into_iter().map(|x| String::from_utf8(x).unwrap()).collect::<Vec<_>>();
        assert_eq!(w("  a  b "), ["a", "b"]);
        assert_eq!(w("\"Rex Jr\" attack"), ["Rex Jr", "attack"]);
        assert_eq!(w("ab\"cd ef"), ["ab", "cd", "ef"]);
        assert_eq!(w("\"open end"), ["open end"]);
        assert_eq!(w("\"\" x"), ["", "x"]);
        assert!(w("   ").is_empty());
        assert_eq!(strip_special("\u{10}x\u{11}hidden\u{12}Rex"), "Rex");
    }

    #[test]
    fn follow_sends_the_long_form_and_gates() {
        let (p, s) = (pets(), none);
        let b = payload(text_command("follow", &st(&p, &s)));
        // key, header {0xC350, 0x1234}, passed-on 1 | form 2, mode 0, target {0xC350, 99}, speed 2.5, pos 0,0,0, count 0
        assert_eq!(hex(&b), "260f3671 0000c350 00001234 01 02 00 0000c350 00000063 40200000 00000000 00000000 00000000 00".replace(' ', ""));
        let mut o = match text_command("follow", &st(&p, &s)) {
            TextResult::Handled(o) => o,
            other => panic!("{other:?}"),
        };
        assert_eq!(o.feedback.remove(0), Feedback { key: "FollowingX", arg: Some("Bob".into()) });
        let go = |f: &dyn Fn(&mut PetState)| {
            let mut t = st(&p, &s);
            f(&mut t.pet);
            fbk(text_command("follow", &t))
        };
        assert_eq!(go(&|s| s.own_fight_level = 2), ["Feedback_CantFollow"]);
        assert_eq!(go(&|s| s.target_features = 1), ["Feedback_CantFollow"]);
        assert_eq!(go(&|s| s.own_features = 0x400_0000), ["Feedback_CantFollow"]);
        assert_eq!(go(&|s| s.can_move = false), ["Feedback_YouCantMove"]);
        for m in [1, 8, 9, 0xb, 0xc] {
            assert_eq!(go(&|s| s.move_mode = m), ["Feedback_YouCantMove"]);
        }
        assert_eq!(go(&|s| s.move_mode = 4), ["FollowingX"]);
        assert_eq!(text_command("follow", &{
            let mut t = st(&p, &s);
            t.pet.target_name = None;
            t
        }), TextResult::Handled(TextOut::default()));
    }
}
