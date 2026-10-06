//! What the original does with the actions of [`super::cmd`] that leave the chat module: slash commands for the game
//! (`N3Msg_TextCommand` / Fanatic), social moves, `/inspect`, `/played`, `/help`, and the chat-server requests of `/invite`, `/kick`,
//! `/leave`, `/cc`, `/lft`, `/ignore <name>`, `/name`. Pure logic: [`perform`] returns the frames and requests; the hub sends them.
//! Evidence: docs/chat/cmd.md (§Zone commands, §Chat-server requests).

use super::cmd::ChatAction;
use super::line::{ChatKind, ChatLine};
use ao_formats::screens::TextDb;
use ao_net::chat::ChatCmd;
use ao_net::frame::Frame;
use ao_net::msg::Identity;
use ao_net::n3::action::social_action;
use ao_net::n3::outgoing::n3_frame;
use ao_net::n3::textcmd::{self, Feedback, Local, TextResult, TextState};

/// Text-db category of `Feedback_*` / `OrgCommandHelp` ... (`FUN_10058b00`, `LDBface::GetTextPtr(0x6e, key)`).
const CAT_FEEDBACK: u32 = 110;
/// `ChatCmdFeedback_*` (and `Error2FewArgs`): category 0x2711.
const CAT_CMD_FEEDBACK: u32 = 10001;
/// `LFTon` / `LFToff` (`GetText(0x64, ..)`).
const CAT_LFT: u32 = 100;

/// Everything `perform` needs from the game and the client (all read from live state in the original).
pub struct ZoneCmdCtx<'a> {
    pub texts: &'a TextDb,
    pub char_id: u32,
    /// `ChatWindowNode+0x1ec` of the window that sent the line.
    pub window: i32,
    /// `InputConfig_t+0xc0` (current target), {0,0} = none.
    pub target: Identity,
    /// `N3Msg_IsTower(target)` (for `/tower terminate`).
    pub target_is_tower: bool,
    /// `N3Msg_GetSkill(GmLevel, 2) != 0`.
    pub gm: bool,
    pub in_team: bool,
    pub team_leader: bool,
    /// `N3Msg_StringToStat` (case-insensitive stat name -> id).
    pub stat_id: &'a dyn Fn(&str) -> Option<i32>,
    /// `N3Msg_GetActionByName` (`/anim <name>`): the `AbstractAnimID_e` or `None` (the engine's animation name table).
    pub anim_by_name: &'a dyn Fn(&str) -> Option<i32>,
    /// Current movement FSM state (`FUN_100704e6`; `ao_net::n3::action::mode`: 4 swim, 8 sit, 0xb sleep, 0xc lounge), 0 = unknown.
    pub move_mode: i32,
    /// A vehicle is equipped (`SimpleChar+0x2c8 != 0 && FUN_1002e347()`).
    pub vehicle_equipped: bool,
    /// `n3Command_t::s_nCommandRefCntr` (the value the next command gets).
    pub social_counter: i32,
    /// `/ignore list` source: the `IgnoreSystem_t` entries.
    pub ignored: &'a [(u32, String)],
    /// Unix time, and the local offset from UTC in minutes (`/played` prints local and GMT).
    pub now_unix: i64,
    pub tz_offset_min: i32,
    /// `N3Msg_GetCurrentHour/Minute` (game clock).
    pub game_time: (u32, u32),
}

/// A chat-server request that needs the character id of a name first (lookup 0x15, the original's `Action_t` classes 2/3/4/5/9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameOp {
    /// `/invite` (class 3).
    Invite,
    /// `/kick` (class 4).
    Kick,
    /// `/leave` (class 5).
    Leave,
    /// `/cc info` (class 2, sub 1).
    CcInfo,
    /// `/ignore <name>` (class 9).
    Ignore,
    /// A tell to a pseudo character, e.g. `/name` -> "name-request" (class 2, sub 5): the text to send.
    Tell(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChatReq {
    Send(ChatCmd),
    ByName { name: String, op: NameOp },
}

/// The result of a lookup for a [`ChatReq::ByName`].
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    Chat(ChatCmd),
    Line(ChatLine),
    /// `IgnoreSystem_t` toggle for this character id (feedback: [`super::cmd::ignore_feedback`]).
    IgnoreToggle(u32),
}

#[derive(Debug, Default, PartialEq)]
pub struct ZoneOut {
    /// ptype-10 frames for the zone connection (seq 0, the flow numbers them).
    pub frames: Vec<Frame>,
    pub chat: Vec<ChatReq>,
    pub lines: Vec<ChatLine>,
    /// `/help`: file name below `text/help/` for the info window.
    pub help_file: Option<String>,
    /// GUI side effects without a wire message (dialogs, local windows).
    pub local: Vec<Local>,
    /// The social command counter was used (`s_nCommandRefCntr++`).
    pub social_sent: bool,
    /// New looking-for-team state (`DAT_10276620`).
    pub lft: Option<bool>,
    /// Commands of `FUN_1003fba6` whose layout is not decoded; nothing was sent.
    pub unsupported: Vec<&'static str>,
}

/// Actions this module performs (the rest belongs to the hub).
pub fn handles(a: &ChatAction) -> bool {
    matches!(
        a,
        ChatAction::ZoneCommand(_)
            | ChatAction::FanaticCommand(_)
            | ChatAction::Tower(_)
            | ChatAction::Social(_)
            | ChatAction::Anim(_)
            | ChatAction::Inspect(_)
            | ChatAction::PlayedTime
            | ChatAction::ShowHelp { .. }
            | ChatAction::Invite(_)
            | ChatAction::Kick(_)
            | ChatAction::Leave(_)
            | ChatAction::Cc(_)
            | ChatAction::CcInfo(_)
            | ChatAction::IgnoreList
            | ChatAction::IgnoreByName(_)
            | ChatAction::Lft { .. }
            | ChatAction::NameRequest(_)
    )
}

/// `%s` substitution of the text db (one argument).
fn fill(t: String, arg: Option<&str>) -> String {
    match arg {
        Some(a) => t.replacen("%s", a, 1),
        None => t,
    }
}

fn db(texts: &TextDb, cat: u32, key: &str) -> String {
    texts.by_key(cat, key).unwrap_or_default()
}

/// `FUN_1009b37f(text, 0x52)`.
fn info_line(text: &str) -> ChatLine {
    ChatLine::new(ChatKind::Other("CCChatCmdFeedbackInfo"), format!("<div><font color=CCChatCmdFeedbackInfo>{text}</font></div>"))
}
/// `FUN_1009b37f(text, 0x51)`.
fn error_line(text: &str) -> ChatLine {
    ChatLine::new(ChatKind::Other("CCChatCmdFeedbackError"), format!("<div><font color=CCChatCmdFeedbackError>{text}</font></div>"))
}
/// `FUN_10058b00(key)`: colour code 0 through the same signal [UNRESOLVED: shown as a system line].
fn feedback_line(texts: &TextDb, f: &Feedback) -> ChatLine {
    ChatLine::new(ChatKind::System, fill(db(texts, CAT_FEEDBACK, f.key), f.arg.as_deref()))
}

/// `Date: %02d. %s %d local` months (GUI 0x101ba9d4.., index = `tm_mon`).
const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// (year, month 1..=12, day) of a day count since 1970-01-01 (proleptic Gregorian).
fn civil(days: i64) -> (i64, usize, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as usize;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The two lines `/played` prints before it asks the server (GUI 0x100b2321, colour 0x52): local time, GMT, game time, local date.
pub fn played_line(now_unix: i64, tz_offset_min: i32, game: (u32, u32)) -> String {
    let hm = |t: i64| ((t.rem_euclid(86_400) / 3600) as u32, (t.rem_euclid(3600) / 60) as u32);
    let local = now_unix + i64::from(tz_offset_min) * 60;
    let ((lh, lm), (gh, gm)) = (hm(local), hm(now_unix));
    let (y, m, d) = civil(local.div_euclid(86_400));
    format!(
        "Time: {lh:02}:{lm:02} local ({gh:02}:{gm:02} GMT), {:02}:{:02} game<br>Date: {d:02}. {} {y} local",
        game.0,
        game.1,
        MONTHS[m - 1]
    )
}

/// UTC offset of the local time zone in minutes (`_localtime64` of the original), 0 where unavailable.
pub fn local_offset_minutes(now_unix: i64) -> i32 {
    #[cfg(unix)]
    {
        #[repr(C)]
        struct Tm {
            sec: i32,
            min: i32,
            hour: i32,
            mday: i32,
            mon: i32,
            year: i32,
            wday: i32,
            yday: i32,
            isdst: i32,
            gmtoff: std::ffi::c_long,
            zone: *const std::ffi::c_char,
        }
        extern "C" {
            fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
        }
        let mut tm = std::mem::MaybeUninit::<Tm>::zeroed();
        // SAFETY: `localtime_r` fills the caller-provided struct (BSD/glibc layout with `tm_gmtoff`) and returns null on failure.
        unsafe {
            if localtime_r(&now_unix, tm.as_mut_ptr()).is_null() {
                return 0;
            }
            (tm.assume_init().gmtoff / 60) as i32
        }
    }
    #[cfg(not(unix))]
    {
        let _ = now_unix;
        0
    }
}

fn text_state<'a>(ctx: &'a ZoneCmdCtx) -> TextState<'a> {
    TextState {
        char_id: ctx.char_id as i32,
        window: ctx.window,
        target: ctx.target,
        gm: ctx.gm,
        in_team: ctx.in_team,
        team_leader: ctx.team_leader,
        stat_id: ctx.stat_id,
    }
}

fn push_payload(out: &mut ZoneOut, ctx: &ZoneCmdCtx, p: Vec<u8>) {
    out.frames.push(n3_frame(0, ctx.char_id, p));
}

/// `FUN_1003fba6` result -> frames, lines, local effects.
fn text_command(out: &mut ZoneOut, ctx: &ZoneCmdCtx, line: &str) {
    match textcmd::text_command(line, &text_state(ctx)) {
        TextResult::Handled(o) => {
            for p in o.payloads {
                push_payload(out, ctx, p);
            }
            out.lines.extend(o.feedback.iter().map(|f| feedback_line(ctx.texts, f)));
            out.local.extend(o.local);
        }
        TextResult::Ignored => {}
        TextResult::Unsupported(what) => out.unsupported.push(what),
    }
}

/// `N3Msg_DoSocialAction` (Gamecode 0x100269d3).
fn social(out: &mut ZoneOut, ctx: &ZoneCmdCtx, anim: i32) {
    let sleep_or_lounge = anim == 0x44 || anim == 0x45;
    let key = if ctx.move_mode == 4 {
        Some("Feedback_CantDoSocialActionsWhileSwimming")
    } else if ctx.vehicle_equipped && sleep_or_lounge {
        Some("Feedback_YouCanNotDoThisWithAVehicleEquipped")
    } else if matches!(ctx.move_mode, 8 | 0xb | 0xc) || !sleep_or_lounge {
        None
    } else {
        Some("Feedback_MustSitToLoungeOrSleep")
    };
    match key {
        Some(k) => out.lines.push(feedback_line(ctx.texts, &Feedback { key: k, arg: None })),
        None => {
            push_payload(out, ctx, social_action(ctx.char_id as i32, ctx.social_counter, anim));
            out.social_sent = true;
        }
    }
}

/// Perform one action of [`handles`]; other actions give an empty [`ZoneOut`].
pub fn perform(a: &ChatAction, ctx: &ZoneCmdCtx) -> ZoneOut {
    let mut out = ZoneOut::default();
    match a {
        ChatAction::ZoneCommand(line) => text_command(&mut out, ctx, line),
        ChatAction::FanaticCommand(line) => {
            push_payload(&mut out, ctx, textcmd::fanatic(ctx.char_id as i32, ctx.window, ctx.target, line));
        }
        ChatAction::Tower(line) => {
            // GUI 0x100b314f: `create` always, `terminate` on a tower target -> Fanatic; the rest -> N3Msg_TextCommand (line NOT expanded)
            let sub = line.split_whitespace().nth(1).unwrap_or("");
            if sub.eq_ignore_ascii_case("create") || (ctx.target_is_tower && sub.eq_ignore_ascii_case("terminate")) {
                push_payload(&mut out, ctx, textcmd::fanatic(ctx.char_id as i32, ctx.window, ctx.target, line));
            } else {
                text_command(&mut out, ctx, line);
            }
        }
        ChatAction::Social(anim) => social(&mut out, ctx, *anim as i32),
        ChatAction::Anim(name) => match (ctx.anim_by_name)(name) {
            Some(anim) => social(&mut out, ctx, anim),
            None => out.lines.push(error_line(&format!("Error: No animation named '{name}'."))),
        },
        ChatAction::Inspect(id) => {
            push_payload(&mut out, ctx, textcmd::inspect(ctx.char_id as i32, Identity { kind: 50000, instance: *id as i32 }));
        }
        ChatAction::PlayedTime => out.lines.push(info_line(&played_line(ctx.now_unix, ctx.tz_offset_min, ctx.game_time))),
        ChatAction::ShowHelp { file } => out.help_file = Some(file.clone()),
        ChatAction::Invite(n) => out.chat.push(ChatReq::ByName { name: n.clone(), op: NameOp::Invite }),
        ChatAction::Kick(n) => out.chat.push(ChatReq::ByName { name: n.clone(), op: NameOp::Kick }),
        ChatAction::Leave(n) => out.chat.push(ChatReq::ByName { name: n.clone(), op: NameOp::Leave }),
        ChatAction::CcInfo(n) => out.chat.push(ChatReq::ByName { name: n.clone(), op: NameOp::CcInfo }),
        ChatAction::IgnoreByName(n) => out.chat.push(ChatReq::ByName { name: n.clone(), op: NameOp::Ignore }),
        ChatAction::Cc(args) => out.chat.push(ChatReq::Send(ChatCmd::Cc { args: args.clone(), window: ctx.window as u32 })),
        ChatAction::NameRequest(text) => {
            // `/name <new name>`: a tell to the pseudo character "name-request" (GUI 0x100ba5b7 -> 0x1008947b type 5)
            out.chat.push(ChatReq::ByName { name: "name-request".into(), op: NameOp::Tell(text.clone()) });
        }
        ChatAction::IgnoreList => {
            out.lines.push(info_line("Ignored characters:"));
            out.lines.extend(ctx.ignored.iter().map(|(id, name)| info_line(&format!("{id}\t{name}"))));
        }
        ChatAction::Lft { on, text } => {
            out.chat.push(ChatReq::Send(if *on { ChatCmd::LftOn(text.clone()) } else { ChatCmd::LftOff }));
            out.lft = Some(*on);
            out.lines.push(info_line(&db(ctx.texts, CAT_LFT, if *on { "LFTon" } else { "LFToff" })));
        }
        _ => {}
    }
    out
}

/// The chat server answered the lookup of a [`ChatReq::ByName`] (`id` != `u32::MAX`); `own_id` is `InputConfig_t+0xd4`.
pub fn resolve(op: NameOp, id: u32, own_id: u32, texts: &TextDb) -> Vec<Resolved> {
    let cmd_text = |k: &str| db(texts, CAT_CMD_FEEDBACK, &format!("ChatCmdFeedback_{k}"));
    match op {
        NameOp::Invite if id == own_id => vec![Resolved::Line(info_line(&cmd_text("Ignore_CantInviteYourself")))],
        NameOp::Invite => vec![Resolved::Chat(ChatCmd::PrivInvite(id))],
        NameOp::Kick => vec![Resolved::Chat(ChatCmd::PrivKick(id))],
        NameOp::Leave => vec![Resolved::Chat(ChatCmd::PrivPart(id))],
        NameOp::CcInfo => vec![Resolved::Chat(ChatCmd::Forward {
            to: id,
            // [GUESS] key/value roles from the names ("destination" = "chatserver", "commane" = "ccinfo"); map order = key order
            entries: vec![("commane".into(), "ccinfo".into()), ("destination".into(), "chatserver".into())],
        })],
        NameOp::Ignore if id == own_id => vec![Resolved::Line(info_line(&cmd_text("Ignore_CantIgnoreYourself")))],
        NameOp::Ignore => vec![Resolved::IgnoreToggle(id)],
        NameOp::Tell(text) => {
            vec![Resolved::Chat(ChatCmd::Tell { to: id, text }), Resolved::Line(info_line(&cmd_text("Name_SendingNameChangeRequest")))]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::action::parse_character_action;
    use ao_net::n3::outgoing::{message_key, DYNEL_CHAR};

    fn db() -> Option<TextDb> {
        let d = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
        TextDb::load(&d).ok()
    }
    fn no_stat(_: &str) -> Option<i32> {
        None
    }
    fn anim(n: &str) -> Option<i32> {
        (n == "wave").then_some(62)
    }
    fn ctx<'a>(t: &'a TextDb) -> ZoneCmdCtx<'a> {
        ZoneCmdCtx {
            texts: t,
            char_id: 0x1234,
            window: 3,
            target: Identity { kind: DYNEL_CHAR, instance: 99 },
            target_is_tower: false,
            gm: false,
            in_team: false,
            team_leader: false,
            stat_id: &no_stat,
            anim_by_name: &anim,
            move_mode: 0,
            vehicle_equipped: false,
            social_counter: 41,
            ignored: &[],
            now_unix: 0,
            tz_offset_min: 0,
            game_time: (0, 0),
        }
    }

    #[test]
    fn dates_and_played_line() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(19_723), (2024, 1, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        // 2024-01-01 23:30:00 UTC viewed from UTC+2 is already the 2nd
        assert_eq!(played_line(19_723 * 86_400 + 23 * 3600 + 1800, 120, (13, 5)), "Time: 01:30 local (23:30 GMT), 13:05 game<br>Date: 02. Jan 2024 local");
        assert_eq!(played_line(0, -90, (0, 0)), "Time: 22:30 local (00:00 GMT), 00:00 game<br>Date: 31. Dec 1969 local");
    }

    #[test]
    fn game_commands_become_frames() {
        let Some(t) = db() else { return };
        let mut c = ctx(&t);
        let o = perform(&ChatAction::ZoneCommand("version".into()), &c);
        assert_eq!(o.frames.len(), 1);
        let f = &o.frames[0];
        assert_eq!((f.ptype, f.sender, f.receiver, f.seq), (10, 0x1234, 2, 0));
        let (_, a) = parse_character_action(&f.payload).unwrap();
        assert_eq!((a.action, a.param), (0x9d, 3));
        // feedback comes from text.mdb category 110
        let o = perform(&ChatAction::ZoneCommand("team loot".into()), &c);
        assert_eq!(o.lines[0].text, "You are not a member of a team!");
        c.in_team = true;
        assert_eq!(perform(&ChatAction::ZoneCommand("team".into()), &c).lines.len(), 3);
        let o = perform(&ChatAction::ZoneCommand("org nosuch".into()), &c);
        assert_eq!(o.lines[0].text, "Invalid /org command. Type /org help  in chat to view available commands.");
        let o = perform(&ChatAction::ZoneCommand("org create caf\u{e9}".into()), &c);
        assert_eq!(o.lines[0].text, "You cannot use letters ( \u{e9} ) in this command.");
        assert!(perform(&ChatAction::ZoneCommand("follow".into()), &c).unsupported == ["follow"]);
        assert!(perform(&ChatAction::ZoneCommand("nonsense".into()), &c) == ZoneOut::default());
    }

    #[test]
    fn fanatic_inspect_tower() {
        let Some(t) = db() else { return };
        let mut c = ctx(&t);
        let o = perform(&ChatAction::FanaticCommand("stuck now".into()), &c);
        let p = &o.frames[0].payload;
        assert_eq!(&p[..4], &message_key("FanaticIIR_t").to_be_bytes());
        assert_eq!(&p[p.len() - 9..], b"stuck now");
        let o = perform(&ChatAction::Inspect(77), &c);
        let (_, a) = parse_character_action(&o.frames[0].payload).unwrap();
        assert_eq!((a.action, a.identity_a), (0x105, Identity { kind: 50000, instance: 77 }));
        assert_eq!(&perform(&ChatAction::Tower("tower create 3".into()), &c).frames[0].payload[..4], &message_key("FanaticIIR_t").to_be_bytes());
        assert!(perform(&ChatAction::Tower("tower terminate".into()), &c).frames.is_empty());
        c.target_is_tower = true;
        assert_eq!(perform(&ChatAction::Tower("tower terminate".into()), &c).frames.len(), 1);
    }

    #[test]
    fn social_rules() {
        let Some(t) = db() else { return };
        let mut c = ctx(&t);
        let o = perform(&ChatAction::Social(62), &c);
        assert!(o.social_sent);
        let p = &o.frames[0].payload;
        assert_eq!(&p[..4], &ao_net::n3::action::SOCIAL_ACTION_CMD.to_be_bytes());
        assert_eq!(&p[p.len() - 8..], &[0, 0, 0, 41, 0, 0, 0, 62]);
        // lounge/sleep need a sitting character
        let o = perform(&ChatAction::Social(0x45), &c);
        assert_eq!(o.lines[0].text, "You must sit in order to use /lounge or /sleep.");
        assert!(o.frames.is_empty());
        c.move_mode = 8;
        assert_eq!(perform(&ChatAction::Social(0x45), &c).frames.len(), 1);
        c.vehicle_equipped = true;
        assert_eq!(perform(&ChatAction::Social(0x45), &c).lines[0].text, "You can not do this with a vehicle equipped.");
        c.move_mode = 4;
        assert_eq!(perform(&ChatAction::Social(1), &c).lines[0].text, "You can't perform social animations while swimming!");
        c.move_mode = 0;
        assert_eq!(perform(&ChatAction::Anim("wave".into()), &c).frames.len(), 1);
        assert!(perform(&ChatAction::Anim("xx".into()), &c).lines[0].text.contains("No animation named 'xx'"));
    }

    #[test]
    fn chat_server_requests() {
        let Some(t) = db() else { return };
        let c = ctx(&t);
        assert_eq!(
            perform(&ChatAction::Invite("Bob".into()), &c).chat,
            [ChatReq::ByName { name: "Bob".into(), op: NameOp::Invite }]
        );
        assert_eq!(
            perform(&ChatAction::Cc(vec!["addbuddy".into(), "Bob".into()]), &c).chat,
            [ChatReq::Send(ChatCmd::Cc { args: vec!["addbuddy".into(), "Bob".into()], window: 3 })]
        );
        let o = perform(&ChatAction::Lft { on: true, text: "raid".into() }, &c);
        assert_eq!((o.chat, o.lft), (vec![ChatReq::Send(ChatCmd::LftOn("raid".into()))], Some(true)));
        assert!(o.lines[0].text.contains("Looking for team: ON"));
        let o = perform(&ChatAction::Lft { on: false, text: String::new() }, &c);
        assert_eq!(o.chat, [ChatReq::Send(ChatCmd::LftOff)]);
        let mut c2 = ctx(&t);
        let ig = [(5u32, "Bob".to_string())];
        c2.ignored = &ig;
        let o = perform(&ChatAction::IgnoreList, &c2);
        assert!(o.lines[0].text.contains("Ignored characters:") && o.lines[1].text.contains("5\tBob"));
        let o = perform(&ChatAction::NameRequest("Newname".into()), &c);
        assert_eq!(o.chat, [ChatReq::ByName { name: "name-request".into(), op: NameOp::Tell("Newname".into()) }]);
        assert_eq!(perform(&ChatAction::ShowHelp { file: "chatcommands.html".into() }, &c).help_file.as_deref(), Some("chatcommands.html"));
        assert!(!handles(&ChatAction::Vicinity("x".into())));
    }

    #[test]
    fn lookups_resolve() {
        let Some(t) = db() else { return };
        assert_eq!(resolve(NameOp::Invite, 9, 1, &t), [Resolved::Chat(ChatCmd::PrivInvite(9))]);
        assert_eq!(resolve(NameOp::Kick, 9, 1, &t), [Resolved::Chat(ChatCmd::PrivKick(9))]);
        assert_eq!(resolve(NameOp::Leave, 9, 1, &t), [Resolved::Chat(ChatCmd::PrivPart(9))]);
        let Resolved::Line(l) = &resolve(NameOp::Invite, 1, 1, &t)[0] else { panic!() };
        assert!(l.text.contains("Can't invite yourself."), "{}", l.text);
        assert_eq!(resolve(NameOp::Ignore, 9, 1, &t), [Resolved::IgnoreToggle(9)]);
        let Resolved::Line(l) = &resolve(NameOp::Ignore, 1, 1, &t)[0] else { panic!() };
        assert!(l.text.contains("Can't ignore yourself."));
        let r = resolve(NameOp::CcInfo, 9, 1, &t);
        let Resolved::Chat(ChatCmd::Forward { to: 9, entries }) = &r[0] else { panic!() };
        assert_eq!(entries.len(), 2);
        let r = resolve(NameOp::Tell("Zed".into()), 9, 1, &t);
        assert_eq!(r[0], Resolved::Chat(ChatCmd::Tell { to: 9, text: "Zed".into() }));
        let Resolved::Line(l) = &r[1] else { panic!() };
        assert!(l.text.contains("Sending name change request."));
    }
}
