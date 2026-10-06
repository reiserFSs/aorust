//! What the original does with a line typed into the chat input bar. Pure logic: [`parse`] turns the line + the
//! current chat state ([`CmdCtx`]) into the [`ChatAction`]s the hub has to perform. Evidence/addresses: docs/chat/cmd.md.
//!
//! Flow (GUI.dll): `FUN_1009a2f8` (window "send") -> command dispatcher `FUN_100a39e5` (only lines starting with `/`):
//! word = text up to the first whitespace (index >= 1), looked up case-insensitively first in the *window* command map
//! (`/ch /g /group /o /t /v /say /w /whisper /s /shout /me /script /`, registered by the ChatWindowNode constructor
//! `FUN_1009dc28`) and then in the *global* map (`FUN_100badf1` + `FUN_100b24e4/2e87/39f6`); a miss falls back to a script
//! file, else `ChatCmdFeedback_CommandNotFound`. Lines without `/` go to the window's output group (`FUN_1009a26c`).
//! There is NO prefix matching of command words (only group names are prefix-matched).

use super::line::{ChatKind, ChatLine};

// --------------------------------------------------------------------------------------------------------------
// state in / actions out
// --------------------------------------------------------------------------------------------------------------

/// Type byte of a chat group (`group + 0x4c`, low byte): 3 = organisation, 0x82 = team (`FUN_1009c6ad` / `FUN_1009c8d7`);
/// the vicinity group is id `0x40000002` (`ChatGUIModule_c::GetGroupIdentifier(0x40000002, "")`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    Vicinity,
    Org,
    Team,
    /// Public/private chat-server group (OOC, Newbie Help, ...): `Group{..}` on the chat server.
    Other,
}

/// One chat group known to `ChatGUIModule_c` (the map `FUN_10083e10()+0x20`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInfo {
    /// `group + 0x28`; what `/ch` and `/g` match (case-insensitive prefix).
    pub name: String,
    pub kind: GroupKind,
    /// `group + 0x50` bit 1 (`FUN_1009989e` refuses these).
    pub read_only: bool,
    /// `group + 0x54`: joined/available (else `FUN_10085538` prints "Error: Chat group %s is currently not available.").
    pub active: bool,
    /// `FUN_1009985f(window, group)`: the active window shows this group (wildcard windows invert the test).
    pub subscribed: bool,
}

/// The game target (`InputConfig_t+0xc0/0xc4`) used by `%t` and `/ignore`. `kind` 50000 = character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub kind: u32,
    pub id: u32,
    pub name: String,
}

pub struct CmdCtx<'a> {
    /// `InputConfig_t+0xd8` name (`%m`).
    pub own_name: &'a str,
    /// `InputConfig_t+0xd4` (`/ignore` refuses ourselves).
    pub own_id: u32,
    /// Every group of `ChatGUIModule_c` in map order.
    pub groups: &'a [GroupInfo],
    /// The active window's output group = current input target (`window + 0xb4`), matched by exact `GroupInfo::name`.
    pub output_group: Option<&'a str>,
    /// `ChatGUIModule_c::GetAFKMessage` while AFK (`IsAFK` = `+0x2a4 != 0`).
    pub afk: Option<&'a str>,
    /// Head of the reply list (`ChatGUIModule_c+0x288`), the sender of the last tell.
    pub last_tell_from: Option<&'a str>,
    /// Current game target (`%t`, `/ignore`), if any.
    pub target: Option<Target>,
    /// Name of the fighting target (`N3Msg_GetAttackingID`, `%f`).
    pub fight_target: Option<&'a str>,
    /// `stat:gmlevel` (the GUI condition is `stat:gmlevel & 0x0001`; false answers `CommandNotAuthorized`).
    pub gm_level: u32,
    /// Pref `ChatWarnWhenSpeakingToUnsubGroups`.
    pub warn_unsub: bool,
    /// Chat server session is up (`/cc` needs it).
    pub chat_connected: bool,
    /// Current LFT flag (`DAT_10276620`).
    pub lft_on: bool,
    /// `scripts/<name>` exists (`FUN_100a3699` fallback). `None` = no script directory.
    pub script_exists: Option<&'a dyn Fn(&str) -> bool>,
    /// Text db lookup by full key, e.g. `"ChatCmdFeedback_AFK_AFKOn"` (category 10001). Missing -> empty string.
    pub text: &'a dyn Fn(&str) -> String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatAction {
    /// Local line (`FUN_1009b37f`), already wrapped like the original.
    Feedback(ChatLine),
    /// Private message by name (resolved by lookup 0x15). Empty `text` = just open the tell window.
    Tell { to: String, text: String },
    /// Zone server ptype 5 text (modes 0/1/2 of `FUN_10085538` on the vicinity group).
    Vicinity(String),
    Whisper(String),
    Shout(String),
    /// `/me` (mode 3, also the AFK announcements).
    Emote(String),
    /// Chat-server group message to the group called `group`.
    Group { group: String, text: String },
    /// `/ch`: the window's output group becomes `GroupInfo::name` (`FUN_1009a06f`).
    SetInputTarget(String),
    /// `ChatGUIModule_c::SetAFK`; `None` = AFK off.
    SetAfk(Option<String>),
    /// The "custom AFK message" dialog of a bare `/afk` (`FUN_10082a6f`): `default` = DefaultAFKReply, `body` = AFKDialogBody.
    AfkPrompt { default: String, body: String },
    /// `/invite`, `/kick`, `/leave <nick>` (private chat group; queued action types 3, 4, 5).
    Invite(String),
    Kick(String),
    Leave(String),
    /// `/ignore list` (prints "Ignored characters:" + `%d\t%s` lines from `IgnoreSystem_t`).
    IgnoreList,
    /// `/ignore <nick>` (queued action type 9; the hub resolves the name then toggles).
    IgnoreByName(String),
    /// `/ignore [id]`: toggle `IgnoreSystem_t` for a character id; the feedback is `ignore_feedback`.
    IgnoreToggle { id: u32, name: Option<String> },
    /// `/cc <args...>` for the chat server (`addbuddy`, `rembuddy`, ...).
    Cc(Vec<String>),
    /// `/cc info <name>` (queued action, not sent as cc).
    CcInfo(String),
    /// `/name <new name>` ("name-request" action).
    NameRequest(String),
    /// `/lft [text]`: `on` = new LFT state.
    Lft { on: bool, text: String },
    /// `/help [topic]`, `/showfile <file>`, `/tipoftheday`: `InfoViewModule_c::ShowURL(url)` (`file://<name>[?section=N]`).
    ShowUrl(String),
    /// `/tipoftheday [prev]` (`FUN_100b6f0d`): next/previous tip, the hub keeps `CurrentTipOfTheDay` and checks the level (stat 0x36 > 3).
    TipOfTheDay { prev: bool },
    /// `/messagebox <text>` (`FUN_100b7a6f`): a `DialogBox_c` with one OK button; the text is `%`-expanded.
    MessageBox(String),
    /// `/camp`: AFCM 0x134 = `FlowControlModule_t::StartQuitToLoginMessage` (GUI 0x10027c74).
    Camp,
    /// `/quit`: AFCM 0x133 = `StartQuitToSystemMessage` (GUI 0x10029a0d).
    Quit,
    /// `/start <url>` (`FUN_100b9a84` -> `FUN_100b94e0`): `ShellExecute("open", url)` for `http://` / `https://` only; the argument is the first token.
    Start(String),
    /// `/open` `/close` `/toggle <window>` (`FUN_100b77b6`): `name` without its quotes.
    Window { name: String, op: WindowOp },
    /// `/script <name>` or a `scripts/<name>` file.
    RunScript(String),
    /// `/<emote>` or `/emote <name>`: `N3Msg_DoSocialAction(id)`; id = index in [`EMOTES`] + 1.
    Social(u32),
    /// `/anim <name>` (`N3Msg_GetActionByName` then DoSocialAction; resolved by the hub).
    Anim(String),
    /// `/inspect <id>`: `N3Msg_Inspect(character 50000, id)`.
    Inspect(u32),
    /// `/played` first prints "Time: ... local (... GMT), ... game<br>Date: ..." (`FUN_100b2321`); the hub does that, then
    /// the `ZoneCommand("played")` follows.
    PlayedTime,
    /// `N3Msg_TextCommand(window, text, target)`: `text` is the line without the leading `/`, `%` args expanded.
    ZoneCommand(String),
    /// `Fanatic::ClientInterface_c::Command` (FanaticIIR_t): the line without its slash, NOT `%`-expanded (GUI 0x100b30cf).
    FanaticCommand(String),
    /// `/tower <sub> ..` (GUI 0x100b314f): `create` -> Fanatic, `terminate` -> Fanatic when the target is a tower, everything else `N3Msg_TextCommand`.
    /// Carries the line without its slash (`%`-expanded only when it goes to `N3Msg_TextCommand`: it does not, see docs).
    Tower(String),
    /// A GUI-local command that is not chat (`/camp`, `/quit`, `/open`, `/option` ...): the unmodified line.
    ClientCommand(String),
    /// `/option` `/setoption` `/dvalue` `/chardist` `/viewdist` `/char&viewdist`: the tokens (command word first), run by the flow on the
    /// DValue store (`play/dvalue.rs`, docs/chat/dvalue.md).
    DValue(Vec<String>),
}

/// `/open` `/close` `/toggle` (`FUN_100b77b6`): the sense the shared handler gets (0 open, 1 close, 2 toggle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowOp {
    Open,
    Close,
    Toggle,
}

/// Names of `/open` `/close` `/toggle` and the DValue each one drives (table at GUI 0x1026c4a0, 24 `{name, dvalue}` pairs).
pub const WINDOW_NAMES: &[(&str, &str)] = &[
    ("ChatConfig", "chat_group_window"),
    ("Controls", "specialaction_window"),
    ("Faction", "faction_window"),
    ("Friends", "friends_window"),
    ("InfoView", "info_window"),
    ("Inventory", "inventory_window"),
    ("Knowledge", "knowledge_window"),
    ("LeftMenu", "cc_left_menu"),
    ("Missions", "mission_window"),
    ("Nano", "nano_window"),
    ("NCU", "ncu_window"),
    ("Perks", "perk_window"),
    ("Pet", "pet_window"),
    ("PlanetMap", "planetmap_window"),
    ("PlayfieldMap", "map_window"),
    ("RightMenu", "cc_right_menu"),
    ("Settings", "optionpanel_window"),
    ("Shortcutbar", "shortcutbar_window"),
    ("Stats", "stat_window"),
    ("Team", "team_view"),
    ("Tradeskill", "tradeskill_window"),
    ("Wear", "wear_window"),
    ("Raid", "raid_window"),
    ("ItemStore", "itemshop_window"),
];

/// DValue of a `/open` window name (case-insensitive, `_stricmp` list walk).
pub fn window_dvalue(name: &str) -> Option<&'static str> {
    WINDOW_NAMES.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, d)| *d)
}

// --------------------------------------------------------------------------------------------------------------
// tables
// --------------------------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Ch,
    Group,
    Org,
    Team,
    /// mode: 0 say, 1 whisper, 2 shout, 3 /me
    Vicinity(u8),
    Script,
    Tell,
    Reply,
    Afk,
    Invite,
    Kick,
    Leave,
    Ignore,
    Name,
    Cc,
    Lft,
    Help,
    Emote,
    Anim,
    Inspect,
    Played,
    /// `FUN_100b2278`: forward the line to the game.
    Forward,
    /// `FUN_100b30cf`: `Fanatic::ClientInterface_c::Command(window, target, line without slash)`.
    Fanatic,
    /// `FUN_100b314f`.
    Tower,
    /// `/<emote name>` (argc 1, `FUN_100b2aba`).
    SocialMove,
    Client,
    /// `/showfile`
    ShowFile,
    /// The distributed-value commands.
    DValue,
    Tip,
    MessageBox,
    /// `/text`: local info line.
    Text,
    /// `/funcom`: local info line.
    Funcom,
    Camp,
    Quit,
    Start,
    Window(WindowOp),
}

struct Cmd {
    name: &'static str,
    /// Max tokens (`String::Tokenize` limit; the last token takes the rest of the line); -1 = unlimited.
    argc: i32,
    gm: bool,
    kind: Kind,
}

const fn c(name: &'static str, argc: i32, kind: Kind) -> Cmd {
    Cmd { name, argc, gm: false, kind }
}
const fn gm(name: &'static str, argc: i32, kind: Kind) -> Cmd {
    Cmd { name, argc, gm: true, kind }
}

/// `FUN_1009dc28` (ChatWindowNode_c ctor) 0x1009e2c5..0x1009e721.
const WINDOW_CMDS: &[Cmd] = &[
    c("/ch", 2, Kind::Ch),
    c("/group", 3, Kind::Group),
    c("/g", 3, Kind::Group),
    c("/o", 2, Kind::Org),
    c("/t", 2, Kind::Team),
    c("/", 2, Kind::Vicinity(0)),
    c("/v", 2, Kind::Vicinity(0)),
    c("/say", 2, Kind::Vicinity(0)),
    c("/w", 2, Kind::Vicinity(1)),
    c("/whisper", 2, Kind::Vicinity(1)),
    c("/s", 2, Kind::Vicinity(2)),
    c("/shout", 2, Kind::Vicinity(2)),
    c("/me", 2, Kind::Vicinity(3)),
    c("/script", 2, Kind::Script),
];

/// Global map: `FUN_100badf1` (0x100badf1..0x100bbc30) plus the three table-driven registrars.
const GLOBAL_CMDS: &[Cmd] = &[
    // FUN_100badf1
    c("/fxscript", 3, Kind::Client),
    c("/help", 2, Kind::Help),
    c("/selectself", 1, Kind::Client),
    c("/funcom", 1, Kind::Funcom),
    c("/bug", 2, Kind::Client),
    c("/showfile", 2, Kind::ShowFile),
    c("/tipoftheday", 2, Kind::Tip),
    c("/option", 3, Kind::DValue),
    c("/setoption", 3, Kind::DValue),
    c("/dvalue", 3, Kind::DValue),
    c("/open", 2, Kind::Window(WindowOp::Open)),
    c("/toggle", 2, Kind::Window(WindowOp::Toggle)),
    c("/close", 2, Kind::Window(WindowOp::Close)),
    c("/messagebox", 2, Kind::MessageBox),
    c("/assist", 2, Kind::Client),
    c("/text", 2, Kind::Text),
    c("/start", 2, Kind::Start),
    c("/camp", 1, Kind::Camp),
    c("/quit", 1, Kind::Quit),
    c("/chardist", 2, Kind::DValue),
    c("/viewdist", 2, Kind::DValue),
    c("/char&viewdist", 3, Kind::DValue),
    c("/cc", -1, Kind::Cc),
    c("/afk", 2, Kind::Afk),
    c("/tell", 3, Kind::Tell),
    c("/reply", 2, Kind::Reply),
    c("/r", 2, Kind::Reply),
    c("/petition", 2, Kind::Client),
    c("/name", 2, Kind::Name),
    c("/invite", 2, Kind::Invite),
    c("/kick", 2, Kind::Kick),
    c("/leave", 2, Kind::Leave),
    c("/ignore", 2, Kind::Ignore),
    c("/voice", 2, Kind::Client),
    c("/macro", 3, Kind::Client),
    c("/petduel", 2, Kind::Client),
    c("/duel", 2, Kind::Client),
    c("/lft", 2, Kind::Lft),
    c("/filter", 3, Kind::Client),
    c("/waypoint", 4, Kind::Client),
    // FUN_100b24e4: forwarded with FUN_100b2278 (argc -1)
    c("/played", 1, Kind::Played),
    c("/version", -1, Kind::Forward),
    c("/bank", -1, Kind::Forward),
    c("/team", -1, Kind::Forward),
    c("/org", -1, Kind::Forward),
    c("/born", -1, Kind::Forward),
    c("/pet", -1, Kind::Forward),
    c("/follow", -1, Kind::Forward),
    c("/items", -1, Kind::Forward),
    c("/raid", -1, Kind::Forward),
    c("/chr", -1, Kind::Forward),
    c("/getlocal", -1, Kind::Forward),
    c("/setlocal", -1, Kind::Forward),
    c("/getlocalfull", -1, Kind::Forward),
    c("/monsterdata", -1, Kind::Forward),
    c("/clearunique", -1, Kind::Forward),
    c("/tplocal", -1, Kind::Forward),
    gm("/clone", -1, Kind::Forward),
    gm("/criterialocal", -1, Kind::Forward),
    gm("/damagemult", -1, Kind::Forward),
    gm("/joycamacc", -1, Kind::Forward),
    gm("/spelllocal", -1, Kind::Forward),
    gm("/resetskill", -1, Kind::Forward),
    c("/rp", 1, Kind::Client),
    c("/inspect", 2, Kind::Inspect),
    c("/reclaim", 1, Kind::Client),
    // FUN_100b2e87
    c("/anim", 2, Kind::Anim),
    c("/emote", 2, Kind::Emote),
    // FUN_100b39f6: handler GUI 0x100b30cf = Fanatic::ClientInterface_c::Command (`Kind::Fanatic`); /tower 0x100b314f; /command 0x100b379b,
    // /gfx 0x100b3258, /terminate 0x100b345e, /reloadgfxtweak 0x100b3317, /togglegroundlightingfix 0x100b30ad are GUI-local (not decoded).
    c("/command", 4, Kind::Client),
    c("/gfx", 3, Kind::Client),
    c("/tower", 2, Kind::Tower),
    c("/terminate", 2, Kind::Client),
    c("/getfull", 2, Kind::Fanatic),
    c("/anon", 2, Kind::Fanatic),
    c("/stuck", 2, Kind::Fanatic),
    c("/list", 2, Kind::Fanatic),
    c("/shop", 2, Kind::Fanatic),
    c("/teleport", 2, Kind::Fanatic),
    c("/tp", 2, Kind::Fanatic),
    c("/monster", 2, Kind::Fanatic),
    c("/npc", 2, Kind::Fanatic),
    c("/spawn", 2, Kind::Fanatic),
    c("/reload", 2, Kind::Fanatic),
    c("/weather", 2, Kind::Fanatic),
    c("/perks", 2, Kind::Fanatic),
    c("/perk", 2, Kind::Fanatic),
    c("/gethash", 2, Kind::Fanatic),
    c("/item", 2, Kind::Fanatic),
    c("/dumphash", 2, Kind::Fanatic),
    c("/framerate", 2, Kind::Fanatic),
    c("/lazyreload", 2, Kind::Fanatic),
    c("/spawnacgentrance", 2, Kind::Fanatic),
    c("/spawnquest", 2, Kind::Fanatic),
    c("/syncdisplay", 2, Kind::Fanatic),
    c("/teleportdynel", 2, Kind::Fanatic),
    c("/reloadgfxtweak", 0, Kind::Client),
    c("/togglegroundlightingfix", 0, Kind::Client),
];

/// Social move names, id = index + 1 (table at GUI 0x101badc4 + 12*id, 70 entries, `FUN_100b29a1`); each is also a `/<name>` command.
pub const EMOTES: [&str; 70] = [
    "prostrate", "angry", "apachi", "applause", "itch", "backflip", "ballet", "blowkiss", "bow", "bulge", "chicken", "cross",
    "crossarm", "adjust", "curt", "disco", "drink", "eat", "fblock", "fishsize", "flamenco", "flip", "giggle", "gloat", "greet",
    "italian", "kneel", "laugh-b", "laugh-s", "legshake", "lookout", "moon", "nod", "nono", "pointba", "pointfor", "pointlef",
    "pointrig", "pointup", "pray", "puke", "pulp", "read", "rocky", "salute", "scared", "scratch", "shake", "shrug", "slap",
    "speech", "spit", "strong1", "strong2", "strong3", "strong4", "surprised", "surrender", "swroyal", "thinker", "thumbs", "wave",
    "ymca", "kiss", "kisslow", "kisshigh", "hug", "sleep", "lounge", "facepalm",
];

/// `/help <topic>` (map filled by `FUN_100a4977`); "" = `helpcommands.html`. Keys marked `*` have their key string shared with
/// another literal in the binary and are inferred from the client's own index page chatcommands.html.
const HELP_TOPICS: &[(&str, &str)] = &[
    ("org", "OrganisationCommands.html"), // *
    ("petition", "Petition Commands.html"),
    ("pet", "Pet Commands.html"), // *
    ("emote", "Social Moves Commands.html"),
    ("macro", "Macrocommands.html"),
    ("script", "Script Commands.html"),
    ("chat", "chatcommands.html"), // *
    ("team", "Team Commands.html"), // *
    ("misc", "MiscellaneousCommands.html"), // *
    ("list", "listcommand.html"), // *
    ("camera", "CameraHelp.html"),
    ("actionbar", "actionbarhelp.html"),
    ("towers", "TowerHelp.html"),
    ("voices", "Voicecommands.html"),
    ("perks", "Perks.html"),
    ("perk", "Perks.html"), // *
    ("factions", "FactionsHelp.html"),
    ("shadowlevel", "ShadowLevel.html"),
    ("raid", "RaidCommands.html"), // *
];

fn find_cmd(word: &str) -> Option<&'static Cmd> {
    let w = |t: &&'static Cmd| t.name.eq_ignore_ascii_case(word);
    WINDOW_CMDS.iter().find(w).or_else(|| GLOBAL_CMDS.iter().find(w)).or_else(|| {
        let n = word.strip_prefix('/')?;
        EMOTES.iter().any(|e| e.eq_ignore_ascii_case(n)).then_some(&SOCIAL_CMD)
    })
}
static SOCIAL_CMD: Cmd = c("/<emote>", 1, Kind::SocialMove);

/// Id of a social move by (case-insensitive) name (`FUN_100b29be`).
pub fn emote_id(name: &str) -> Option<u32> {
    EMOTES.iter().position(|e| e.eq_ignore_ascii_case(name)).map(|i| i as u32 + 1)
}

// --------------------------------------------------------------------------------------------------------------
// helpers
// --------------------------------------------------------------------------------------------------------------

/// `String::Tokenize(line, max)` (Utils.dll 0x1000e2a6). Whitespace separated; a token that starts with `"` ends at the next `"`
/// followed by whitespace/end and KEEPS its quotes; once `max - 1` tokens exist the rest of the line (including any extra
/// leading blanks) becomes the last token; a blank after the `max-1`th token with nothing behind it yields an empty last token.
pub fn tokenize(s: &str, max: i32) -> Vec<String> {
    let len = s.len();
    let chars: Vec<(usize, char)> = s.char_indices().collect();
    let (mut out, mut start, mut in_q) = (Vec::<String>::new(), 0usize, false);
    if len == 0 {
        return out;
    }
    let mut idx = 0;
    while idx <= chars.len() {
        let i = chars.get(idx).map_or(len, |t| t.0);
        let ch = chars.get(idx).map(|t| t.1);
        let step = ch.map_or(1, char::len_utf8);
        idx += 1;
        let ws = ch.is_some_and(char::is_whitespace);
        if !in_q && i == start && i != len && ws {
            start += step;
            continue;
        }
        let mut plain = true;
        if i < len && ch == Some('"') {
            if !in_q {
                if i == start {
                    in_q = true;
                    plain = false;
                }
            } else if i == len - 1 || s[i + step..].chars().next().is_some_and(char::is_whitespace) {
                in_q = false;
                out.push(s[start..i + step].to_string());
                start = i + step;
                plain = false;
            }
        }
        if !plain {
            continue;
        }
        if i == len || (!in_q && ws) {
            if i != start {
                out.push(s[start..i].to_string());
            }
            start = i + step;
        }
        if out.len() as i32 == max - 1 {
            if i < len {
                out.push(s[start.min(len)..].to_string());
            }
            break;
        }
    }
    out
}

/// `ChatGUIModule_c::ExpandChatTextArgs` (GUI 0x1008619b): `%%` -> `%`, `%f` fighting target, `%m` own name, `%t` target;
/// any other `%x` stays as typed.
pub fn expand(s: &str, ctx: &CmdCtx) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(ch) = it.next() {
        if ch != '%' {
            out.push(ch);
            continue;
        }
        match it.peek().copied() {
            None => out.push('%'),
            Some('%') => {
                it.next();
                out.push('%');
            }
            Some(k @ ('f' | 'm' | 't')) => {
                it.next();
                match k {
                    'f' => out.push_str(ctx.fight_target.unwrap_or("&lt;no fighting target&gt;")),
                    'm' => out.push_str(ctx.own_name),
                    _ => out.push_str(ctx.target.as_ref().map_or("&lt;no target&gt;", |t| t.name.as_str())),
                }
            }
            Some(_) => out.push('%'),
        }
    }
    out
}

/// `LDBformat`: substitute `%s` / `%d` left to right.
fn fmt(tpl: &str, args: &[&str]) -> String {
    let (mut out, mut a) = (String::new(), args.iter());
    let mut it = tpl.chars().peekable();
    while let Some(ch) = it.next() {
        if ch == '%' && matches!(it.peek(), Some('s' | 'd')) {
            it.next();
            out.push_str(a.next().copied().unwrap_or(""));
        } else {
            out.push(ch);
        }
    }
    out
}

/// `FUN_1009b37f`: the colour (ColorCode_e 0x51 `CCChatCmdFeedbackError` / 0x52 `CCChatCmdFeedbackInfo`, GUI 0x10268d38 table)
/// wraps the `%`-expanded text as `<div><font color=NAME>text</font></div>`.
pub fn feedback_line(error: bool, text: &str, ctx: &CmdCtx) -> ChatLine {
    let name = if error { "CCChatCmdFeedbackError" } else { "CCChatCmdFeedbackInfo" };
    ChatLine::new(ChatKind::Other(name), format!("<div><font color={name}>{}</font></div>", expand(text, ctx)))
}

fn err(ctx: &CmdCtx, text: &str) -> ChatAction {
    ChatAction::Feedback(feedback_line(true, text, ctx))
}
fn info(ctx: &CmdCtx, text: &str) -> ChatAction {
    ChatAction::Feedback(feedback_line(false, text, ctx))
}
fn key(ctx: &CmdCtx, k: &str) -> String {
    (ctx.text)(&format!("ChatCmdFeedback_{k}"))
}
fn usage(ctx: &CmdCtx, tok0: &str, rest: &str) -> Vec<ChatAction> {
    vec![err(ctx, &format!("Usage: {tok0}{rest}"))]
}

/// What Shift+R (`TextInputModule_t::StartChatReplyMessage`, GUI 0x10021fd0 -> 0x1009494e) puts into the opened input line.
pub fn reply_prefill(ctx: &CmdCtx) -> String {
    ctx.last_tell_from.map_or(String::new(), |n| format!("/tell {n} "))
}

fn strip_quotes(s: &str) -> &str {
    s.trim_end_matches('"').trim_start_matches('"')
}

/// `FUN_10083814`: groups whose name starts with `name` (case-insensitive); an exact (same length) match wins alone.
fn find_groups<'a>(ctx: &CmdCtx<'a>, name: &str) -> Vec<&'a GroupInfo> {
    let want = name.to_ascii_lowercase();
    let mut v = Vec::new();
    for g in ctx.groups {
        if g.name.to_ascii_lowercase().starts_with(&want) {
            if g.name.len() == name.len() {
                return vec![g];
            }
            v.push(g);
        }
    }
    v
}

/// `FUN_1009a9f3`: the candidate list of `AmbiguousGroupName` (red = read-only, white = selectable here, silver = other).
fn ambiguous(ctx: &CmdCtx, cands: &[&GroupInfo], tok0: &str, tok1: &str) -> ChatAction {
    let mut list = String::new();
    for (i, g) in cands.iter().enumerate() {
        list.push_str(if g.read_only {
            "<font color=red>"
        } else if g.subscribed {
            "<font color=white>"
        } else {
            "<font color=silver>"
        });
        if i != 0 {
            list.push_str(", ");
        }
        list.push_str(&g.name);
    }
    err(ctx, &fmt(&key(ctx, "AmbiguousGroupName"), &[&list, tok0, tok1]))
}

fn no_group(ctx: &CmdCtx, name: &str) -> ChatAction {
    err(ctx, &format!("No chat-group named '{name}'."))
}

/// `FUN_10085538`: `Group::Send(text, mode)` -> the transport action; inactive groups only print the error.
fn send(ctx: &CmdCtx, g: &GroupInfo, text: String, mode: u8) -> ChatAction {
    if !g.active {
        return err(ctx, &fmt("Error: Chat group %s is currently not available.", &[&g.name]));
    }
    match g.kind {
        GroupKind::Vicinity => match mode {
            1 => ChatAction::Whisper(text),
            2 => ChatAction::Shout(text),
            3 => ChatAction::Emote(text),
            _ => ChatAction::Vicinity(text),
        },
        _ => ChatAction::Group { group: g.name.clone(), text },
    }
}

/// `send` + the `ChatWarnWhenSpeakingToUnsubGroups` warning.
fn send_warn(ctx: &CmdCtx, g: &GroupInfo, text: String, mode: u8) -> Vec<ChatAction> {
    let mut v = vec![send(ctx, g, text, mode)];
    if ctx.warn_unsub && !g.subscribed {
        v.push(err(ctx, &key(ctx, "TalkToUnsubscribedChannel")));
    }
    v
}

fn vicinity<'a>(ctx: &CmdCtx<'a>) -> Option<&'a GroupInfo> {
    ctx.groups.iter().find(|g| g.kind == GroupKind::Vicinity)
}

// --------------------------------------------------------------------------------------------------------------
// parse
// --------------------------------------------------------------------------------------------------------------

/// A line typed into the chat input.
pub fn parse(input: &str, ctx: &CmdCtx) -> Vec<ChatAction> {
    if input.is_empty() {
        return vec![];
    }
    if !input.starts_with('/') {
        return plain(input, ctx);
    }
    // FUN_100a3436: the word ends at the first whitespace at index >= 1
    let end = input.char_indices().skip(1).find(|(_, c)| c.is_whitespace()).map_or(input.len(), |t| t.0);
    let word = &input[..end];
    let Some(cmd) = find_cmd(word) else {
        if let Some(f) = ctx.script_exists {
            if f(&word[1..]) {
                return vec![ChatAction::RunScript(word[1..].to_string())];
            }
        }
        return vec![err(ctx, &fmt(&key(ctx, "CommandNotFound"), &[&word[1..]]))];
    };
    if cmd.gm && ctx.gm_level & 1 == 0 {
        return vec![err(ctx, &key(ctx, "CommandNotAuthorized"))];
    }
    let toks = tokenize(input, cmd.argc);
    if toks.is_empty() {
        return vec![];
    }
    // "/<cmd> help" shows the help page of <cmd> if there is one (FUN_100a39e5 0x100a3cdc..)
    let second = if cmd.argc < 3 { tokenize(input, -1) } else { toks.clone() };
    if second.get(1).is_some_and(|t| t.eq_ignore_ascii_case("help")) {
        if let Some((_, f)) = HELP_TOPICS.iter().find(|(k, _)| k.eq_ignore_ascii_case(&word[1..])) {
            return vec![ChatAction::ShowUrl(format!("file://{f}"))];
        }
    }
    run(cmd, input, &toks, ctx)
}

/// Text without a slash (`FUN_1009a2f8` tail): to the window's output group when it is writable and shown in this window.
fn plain(input: &str, ctx: &CmdCtx) -> Vec<ChatAction> {
    let Some(g) = ctx.output_group.and_then(|n| ctx.groups.iter().find(|g| g.name == n)) else { return vec![] };
    if g.read_only || !g.subscribed {
        return vec![];
    }
    vec![send(ctx, g, expand(input, ctx), 0)]
}

fn run(cmd: &Cmd, line: &str, t: &[String], ctx: &CmdCtx) -> Vec<ChatAction> {
    let n = t.len();
    let x = |i: usize| expand(&t[i], ctx);
    match cmd.kind {
        Kind::Ch => {
            if n != 2 {
                return usage(ctx, &t[0], " &lt;group name&gt;");
            }
            let name = strip_quotes(&t[1]);
            let cands = find_groups(ctx, name);
            if cands.is_empty() {
                return vec![no_group(ctx, name)];
            }
            let mut sel = None;
            if cands.len() == 1 {
                sel = Some(cands[0]);
            } else {
                for g in &cands {
                    if g.read_only || !g.subscribed {
                        continue;
                    }
                    if sel.is_some() {
                        return vec![ambiguous(ctx, &cands, &t[0], &t[1])];
                    }
                    sel = Some(*g);
                }
            }
            let Some(g) = sel else { return vec![err(ctx, &key(ctx, "Ch_NoMatchingGroupAreSelectable"))] };
            if !g.subscribed {
                vec![err(ctx, &key(ctx, "Ch_CanOnlySelectSubscribedGroups"))]
            } else if g.read_only {
                vec![err(ctx, &fmt(&key(ctx, "GroupIsReadOnly"), &[&g.name]))]
            } else {
                vec![ChatAction::SetInputTarget(g.name.clone())]
            }
        }
        Kind::Group => {
            if n < 3 {
                return usage(ctx, &t[0], " &lt;group name&gt; message");
            }
            let name = strip_quotes(&t[1]);
            let cands = find_groups(ctx, name);
            if cands.is_empty() {
                return vec![no_group(ctx, name)];
            }
            let g = if cands.len() == 1 {
                if cands[0].read_only {
                    return vec![err(ctx, &fmt(&key(ctx, "GroupIsReadOnly"), &[&cands[0].name]))];
                }
                cands[0]
            } else {
                let mut sel = None;
                for g in &cands {
                    if g.read_only {
                        continue;
                    }
                    if sel.is_some() {
                        return vec![ambiguous(ctx, &cands, &t[0], &t[1])];
                    }
                    sel = Some(*g);
                }
                let Some(g) = sel else { return vec![err(ctx, &key(ctx, "AllMatchingGroupsAreReadOnly"))] };
                g
            };
            send_warn(ctx, g, x(2), 0)
        }
        Kind::Org | Kind::Team => {
            if n < 2 {
                return usage(ctx, &t[0], " &lt;message&gt;");
            }
            let want = if cmd.kind == Kind::Org { GroupKind::Org } else { GroupKind::Team };
            match ctx.groups.iter().find(|g| g.active && g.kind == want) {
                Some(g) => send_warn(ctx, g, x(1), 0),
                None => vec![err(ctx, &key(ctx, if want == GroupKind::Org { "O_YouAreNotMemberOfAnOrganizaion" } else { "T_YouAreNotMemberOfATeam" }))],
            }
        }
        Kind::Vicinity(mode) => {
            if n < 2 {
                return usage(ctx, &t[0], " &lt;message&gt;");
            }
            match vicinity(ctx) {
                Some(g) => send_warn(ctx, g, x(1), mode),
                None => vec![],
            }
        }
        Kind::Script => {
            if n < 2 {
                vec![err(ctx, &(ctx.text)("Error2FewArgs"))]
            } else {
                vec![ChatAction::RunScript(t[1].clone())]
            }
        }
        Kind::Tell => {
            if n < 2 {
                return vec![];
            }
            vec![ChatAction::Tell { to: x(1), text: if n < 3 { String::new() } else { x(2) } }]
        }
        Kind::Reply => match ctx.last_tell_from {
            None => vec![info(ctx, "You have not received any tell messages yet.")],
            Some(to) => vec![ChatAction::Tell { to: to.to_string(), text: if n < 2 { String::new() } else { x(1) } }],
        },
        Kind::Afk => afk(t, ctx),
        Kind::Invite | Kind::Kick | Kind::Leave => {
            if n < 2 {
                return usage(ctx, &t[0], " &lt;nick&gt;");
            }
            vec![match cmd.kind {
                Kind::Invite => ChatAction::Invite(x(1)),
                Kind::Kick => ChatAction::Kick(x(1)),
                _ => ChatAction::Leave(x(1)),
            }]
        }
        Kind::Name => {
            if n < 2 {
                return usage(ctx, &t[0], " &lt;new name&gt;");
            }
            vec![ChatAction::NameRequest(x(1))]
        }
        Kind::Ignore => ignore(t, ctx),
        Kind::Cc => {
            if n == 3 && t[1].eq_ignore_ascii_case("info") {
                return vec![ChatAction::CcInfo(x(2))];
            }
            if n < 2 {
                return vec![err(ctx, "Invalid syntax")];
            }
            if !ctx.chat_connected {
                return vec![err(ctx, "Error: Not connected to chat-server.")];
            }
            vec![ChatAction::Cc((1..n).map(x).collect())]
        }
        Kind::Lft => {
            // FUN_100b69b0: text -> LFT on with that text; bare /lft toggles
            if n >= 2 {
                vec![ChatAction::Lft { on: true, text: t[1].clone() }]
            } else {
                vec![ChatAction::Lft { on: !ctx.lft_on, text: String::new() }]
            }
        }
        Kind::Help => {
            if n < 2 {
                return vec![ChatAction::ShowUrl("file://helpcommands.html".into())];
            }
            match HELP_TOPICS.iter().find(|(k, _)| k.eq_ignore_ascii_case(&t[1])) {
                Some((_, f)) => vec![ChatAction::ShowUrl(format!("file://{f}"))],
                None => vec![err(ctx, &format!("Error: no help topic named '{}'.", t[1]))],
            }
        }
        Kind::Emote => {
            if n < 2 {
                return usage(ctx, &t[0], " &lt;emote-name&gt;");
            }
            match emote_id(&t[1]) {
                Some(id) => vec![ChatAction::Social(id)],
                None => vec![err(ctx, &format!("Error: No emote named '{}'.", t[1]))],
            }
        }
        Kind::Anim => {
            if n < 2 {
                return usage(ctx, &t[0], " &lt;animation-name&gt;");
            }
            vec![ChatAction::Anim(t[1].clone())]
        }
        Kind::SocialMove => emote_id(&t[0][1..]).map(ChatAction::Social).into_iter().collect(),
        Kind::Inspect => {
            if n < 2 {
                return vec![];
            }
            vec![ChatAction::Inspect(strtoul(&t[1]))]
        }
        Kind::Played => vec![ChatAction::PlayedTime, ChatAction::ZoneCommand("played".into())],
        Kind::Forward => vec![ChatAction::ZoneCommand(expand(&line[1..], ctx))],
        Kind::Fanatic => vec![ChatAction::FanaticCommand(line[1..].to_string())],
        Kind::Tower => {
            if n < 2 {
                vec![]
            } else {
                vec![ChatAction::Tower(line[1..].to_string())]
            }
        }
        Kind::Client => vec![ChatAction::ClientCommand(line.to_string())],
        Kind::DValue => vec![ChatAction::DValue(t.to_vec())],
        // FUN_100b6e66
        Kind::ShowFile => {
            if n < 2 {
                return usage(ctx, &t[0], " filename");
            }
            vec![ChatAction::ShowUrl(format!("file://{}", t[1]))]
        }
        // FUN_100b6f0d: the level gate (stat 0x36 > 3) and the tip counter are the hub's
        Kind::Tip => vec![ChatAction::TipOfTheDay { prev: t.get(1).is_some_and(|a| a.trim().eq_ignore_ascii_case("prev")) }],
        // FUN_100b7a6f
        Kind::MessageBox => {
            if n < 2 {
                return usage(ctx, &t[0], " &lt;message&gt;");
            }
            vec![ChatAction::MessageBox(x(1))]
        }
        // FUN_100b5913 (colour 0x52)
        Kind::Text => {
            if n < 2 {
                return vec![];
            }
            vec![info(ctx, &t[1])]
        }
        // FUN_100b59d3 (colour 0x52)
        Kind::Funcom => vec![info(ctx, "Funcom made this excellent product :)\nThank you for playing Anarchy Online.")],
        Kind::Camp => vec![ChatAction::Camp],
        Kind::Quit => vec![ChatAction::Quit],
        // FUN_100b9a84: usage " <URL>"; FUN_100b94e0 starts only http:// and https:// (case-insensitive) with the first token
        Kind::Start => {
            if n < 2 {
                return usage(ctx, &t[0], " &lt;URL&gt;");
            }
            let url = t[1].split_whitespace().next().unwrap_or("");
            let l = url.to_ascii_lowercase();
            if l.starts_with("http://") || l.starts_with("https://") {
                vec![ChatAction::Start(url.to_string())]
            } else {
                vec![]
            }
        }
        // FUN_100b77b6: quotes around the name are stripped
        Kind::Window(op) => {
            if n < 2 {
                return usage(ctx, &t[0], " window");
            }
            vec![ChatAction::Window { name: t[1].trim_matches('"').to_string(), op }]
        }
    }
}

fn strtoul(s: &str) -> u32 {
    let d: String = s.chars().take_while(char::is_ascii_digit).collect();
    d.parse().unwrap_or(0)
}

/// `FUN_100b7cb8`. Announcements go out as `/me` (mode 3) in the vicinity group.
fn afk(t: &[String], ctx: &CmdCtx) -> Vec<ChatAction> {
    let n = t.len();
    let announce = |k: &str| vicinity(ctx).map(|g| send(ctx, g, key(ctx, k), 3));
    if n == 2 && t[1].eq_ignore_ascii_case("status") {
        return vec![match ctx.afk {
            Some(m) => info(ctx, &fmt(&key(ctx, "AFK_Status_AFKIsCurrentlyOn"), &[m])),
            None => info(ctx, &key(ctx, "AFK_Status_AFKIsCurrentlyOff")),
        }];
    }
    if ctx.afk.is_some() && n < 2 {
        let mut v = vec![ChatAction::SetAfk(None), info(ctx, &key(ctx, "AFK_AFKOff"))];
        v.extend(announce("AFK_IsBack"));
        return v;
    }
    let mut v = Vec::new();
    let msg = if n < 2 {
        let default = key(ctx, "DefaultAFKReply");
        v.push(ChatAction::AfkPrompt { default: default.clone(), body: key(ctx, "AFKDialogBody") });
        default
    } else {
        expand(&t[1], ctx)
    };
    match ctx.afk {
        None => {
            v.push(info(ctx, &key(ctx, "AFK_AFKOn")));
            v.extend(announce("AFK_IsAFK"));
        }
        Some(old) if old != msg => v.push(info(ctx, &fmt(&key(ctx, "ChangedAFKMessageTo"), &[old, &msg]))),
        Some(_) => {}
    }
    v.push(ChatAction::SetAfk(Some(msg)));
    v
}

/// `FUN_100ba9fc`.
fn ignore(t: &[String], ctx: &CmdCtx) -> Vec<ChatAction> {
    let n = t.len();
    let numeric = n == 2 && t[1].chars().next().is_some_and(|c| c.is_ascii_digit());
    let (kind, id, name) = if n < 2 {
        match &ctx.target {
            Some(tg) => (tg.kind, tg.id, Some(tg.name.clone())),
            None => (0, 0, None),
        }
    } else if numeric {
        (50000, strtoul(&t[1]), None)
    } else if t[1] == "list" {
        return vec![ChatAction::IgnoreList];
    } else {
        return vec![ChatAction::IgnoreByName(expand(&t[1], ctx))];
    };
    if kind == 0 && id == 0 {
        vec![info(ctx, &key(ctx, "Ignore_NoTargetOrNick"))]
    } else if kind != 50000 {
        vec![info(ctx, &key(ctx, "Ignore_CanOnlyIgnoreCharacters"))]
    } else if id == ctx.own_id {
        vec![info(ctx, &key(ctx, "Ignore_CantIgnoreYourself"))]
    } else {
        vec![ChatAction::IgnoreToggle { id, name }]
    }
}

// --------------------------------------------------------------------------------------------------------------
// tests
// --------------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn g(name: &str, kind: GroupKind) -> GroupInfo {
        GroupInfo { name: name.into(), kind, read_only: false, active: true, subscribed: true }
    }

    fn groups() -> Vec<GroupInfo> {
        vec![
            g("Vicinity", GroupKind::Vicinity),
            g("OOC", GroupKind::Other),
            g("Newbie Help", GroupKind::Other),
            g("Neu Org News", GroupKind::Other),
            g("Clan", GroupKind::Org),
            g("Team", GroupKind::Team),
            GroupInfo { read_only: true, ..g("Announcements", GroupKind::Other) },
        ]
    }

    /// Key -> text as in text.mdb category 10001 (subset).
    fn txt(k: &str) -> String {
        match k.strip_prefix("ChatCmdFeedback_").unwrap_or(k) {
            "AFK_IsBack" => "is back",
            "AFK_IsAFK" => "is afk",
            "AFK_AFKOff" => "AFK off.",
            "AFK_AFKOn" => "AFK on. All tell messages will be replied with afk.",
            "DefaultAFKReply" => "The user is currently AFK.",
            "AFKDialogBody" => "dialog",
            "ChangedAFKMessageTo" => "Changed AFK message from '%s' to '%s'",
            "AFK_Status_AFKIsCurrentlyOn" => "AFK is currently on. Current AFK message is '%s'.",
            "AFK_Status_AFKIsCurrentlyOff" => "AFK is currently off.",
            "CommandNotFound" => "No chat command or script named \"%s\" available.",
            "CommandNotAuthorized" => "not authorized",
            "GroupIsReadOnly" => "Group '%s' is read-only.",
            "AmbiguousGroupName" => "ambiguous:%s|%s %s",
            "Ch_CanOnlySelectSubscribedGroups" => "only subscribed",
            "Ch_NoMatchingGroupAreSelectable" => "none selectable",
            "AllMatchingGroupsAreReadOnly" => "all read-only",
            "TalkToUnsubscribedChannel" => "warning unsub",
            "O_YouAreNotMemberOfAnOrganizaion" => "no org",
            "T_YouAreNotMemberOfATeam" => "no team",
            "Ignore_IgnoringCharacter" => "Ignoring character \"%s\".",
            "Ignore_UnignoringCharacter" => "Unignoring character \"%s\".",
            "Ignore_CantIgnoreYourself" => "Can't ignore yourself.",
            "Ignore_NoTargetOrNick" => "No target selected and no nick specified.",
            "Ignore_CanOnlyIgnoreCharacters" => "Can only ignore characters.",
            "Error2FewArgs" => "Error: to few arguments",
            _ => "",
        }
        .to_string()
    }

    fn ctx<'a>(gs: &'a [GroupInfo], text: &'a dyn Fn(&str) -> String) -> CmdCtx<'a> {
        CmdCtx {
            own_name: "Me",
            own_id: 7,
            groups: gs,
            output_group: Some("OOC"),
            afk: None,
            last_tell_from: Some("Bob"),
            target: None,
            fight_target: None,
            gm_level: 0,
            warn_unsub: true,
            chat_connected: true,
            lft_on: false,
            script_exists: None,
            text,
        }
    }

    fn fb(a: &ChatAction) -> &str {
        match a {
            ChatAction::Feedback(l) => &l.text,
            _ => panic!("not feedback: {a:?}"),
        }
    }
    fn tell(to: &str, text: &str) -> ChatAction {
        ChatAction::Tell { to: to.into(), text: text.into() }
    }

    #[test]
    fn tokenizer() {
        assert_eq!(tokenize("/tell Bob  hi there ", 3), ["/tell", "Bob", " hi there "]);
        assert_eq!(tokenize("/tell Bob ", 3), ["/tell", "Bob", ""]);
        assert_eq!(tokenize("/tell Bob", 3), ["/tell", "Bob"]);
        assert_eq!(tokenize("/camp now please", 1), ["/camp now please"]);
        assert_eq!(tokenize("/cc  addbuddy   bob", -1), ["/cc", "addbuddy", "bob"]);
        // quotes are kept; a quoted token may contain blanks
        assert_eq!(tokenize("/g \"Newbie Help\" hello world", 3), ["/g", "\"Newbie Help\"", "hello world"]);
        // a quote inside a word is an ordinary character
        assert_eq!(tokenize("/afk it\"s fine", 2), ["/afk", "it\"s fine"]);
        assert!(tokenize("", 3).is_empty());
    }

    #[test]
    fn expands() {
        let gs = groups();
        let mut c = ctx(&gs, &txt);
        assert_eq!(expand("100%% sure %m %x %t %f %", &c), "100% sure Me %x &lt;no target&gt; &lt;no fighting target&gt; %");
        c.target = Some(Target { kind: 50000, id: 9, name: "Bob".into() });
        c.fight_target = Some("Mob");
        assert_eq!(expand("%t vs %f", &c), "Bob vs Mob");
    }

    #[test]
    fn help_page_examples() {
        let gs = groups();
        let c = ctx(&gs, &txt);
        let one = |s: &str| parse(s, &c);
        // chatcommands.html / "Chatting With Others.html"
        assert_eq!(one("/tell Bob hello there"), [tell("Bob", "hello there")]);
        assert_eq!(one("/TELL Bob hi"), [tell("Bob", "hi")]);
        assert_eq!(one("/tell Bob"), [tell("Bob", "")]);
        assert_eq!(one("/tell"), []);
        assert_eq!(one("/reply thanks"), [tell("Bob", "thanks")]);
        assert_eq!(one("/r thanks"), [tell("Bob", "thanks")]);
        assert_eq!(one("/R"), [tell("Bob", "")]);
        assert_eq!(one("/say hi"), [ChatAction::Vicinity("hi".into())]);
        assert_eq!(one("/ hi"), [ChatAction::Vicinity("hi".into())]);
        assert_eq!(one("/shout hey"), [ChatAction::Shout("hey".into())]);
        assert_eq!(one("/s hey"), [ChatAction::Shout("hey".into())]);
        assert_eq!(one("/whisper psst"), [ChatAction::Whisper("psst".into())]);
        assert_eq!(one("/W psst"), [ChatAction::Whisper("psst".into())]);
        assert_eq!(one("/me smiles"), [ChatAction::Emote("smiles".into())]);
        assert_eq!(one("/group OOC hello"), [ChatAction::Group { group: "OOC".into(), text: "hello".into() }]);
        assert_eq!(one("/g ooc hello"), [ChatAction::Group { group: "OOC".into(), text: "hello".into() }]);
        assert_eq!(one("/g \"Newbie Help\" hi"), [ChatAction::Group { group: "Newbie Help".into(), text: "hi".into() }]);
        assert_eq!(one("/ch team"), [ChatAction::SetInputTarget("Team".into())]);
        assert_eq!(one("/ch \"newbie help\""), [ChatAction::SetInputTarget("Newbie Help".into())]);
        assert_eq!(one("/invite Bob"), [ChatAction::Invite("Bob".into())]);
        assert_eq!(one("/kick Bob"), [ChatAction::Kick("Bob".into())]);
        assert_eq!(one("/leave Bob"), [ChatAction::Leave("Bob".into())]);
        assert_eq!(one("/ignore Bob"), [ChatAction::IgnoreByName("Bob".into())]);
        assert_eq!(one("/ignore list"), [ChatAction::IgnoreList]);
        assert_eq!(one("/ignore 1234"), [ChatAction::IgnoreToggle { id: 1234, name: None }]);
        assert_eq!(one("/cc addbuddy Bob"), [ChatAction::Cc(vec!["addbuddy".into(), "Bob".into()])]);
        assert_eq!(one("/cc rembuddy ?"), [ChatAction::Cc(vec!["rembuddy".into(), "?".into()])]);
        assert_eq!(one("/o hello"), [ChatAction::Group { group: "Clan".into(), text: "hello".into() }]);
        assert_eq!(one("/t hello"), [ChatAction::Group { group: "Team".into(), text: "hello".into() }]);
        assert_eq!(one("/name Foo"), [ChatAction::NameRequest("Foo".into())]);
        assert_eq!(one("/help chat"), [ChatAction::ShowUrl("file://chatcommands.html".into())]);
        assert_eq!(one("/help"), [ChatAction::ShowUrl("file://helpcommands.html".into())]);
        assert_eq!(one("/macro help"), [ChatAction::ShowUrl("file://Macrocommands.html".into())]);
    }

    #[test]
    fn plain_text_goes_to_output_group() {
        let gs = groups();
        let mut c = ctx(&gs, &txt);
        assert_eq!(parse("hello %m", &c), [ChatAction::Group { group: "OOC".into(), text: "hello Me".into() }]);
        c.output_group = Some("Vicinity");
        assert_eq!(parse("hello", &c), [ChatAction::Vicinity("hello".into())]);
        c.output_group = Some("Announcements"); // read-only: silently dropped
        assert_eq!(parse("hello", &c), []);
        c.output_group = None;
        assert_eq!(parse("hello", &c), []);
    }

    #[test]
    fn option_commands_become_dvalue_actions() {
        let gs = groups();
        let c = ctx(&gs, &txt);
        let toks = |l: &str| match parse(l, &c).as_slice() {
            [ChatAction::DValue(t)] => t.clone(),
            a => panic!("{l}: {a:?}"),
        };
        assert_eq!(toks("/option NumHotbars 3"), ["/option", "NumHotbars", "3"]);
        assert_eq!(toks("/setoption ChatFontName \"Courier New\""), ["/setoption", "ChatFontName", "\"Courier New\""]);
        assert_eq!(toks("/DVALUE Foo 1 + 2"), ["/DVALUE", "Foo", "1 + 2"], "the third token is the rest of the line");
        assert_eq!(toks("/viewdist 50"), ["/viewdist", "50"]);
        assert_eq!(toks("/char&viewdist 40 60"), ["/char&viewdist", "40", "60"]);
        assert_eq!(toks("/chardist"), ["/chardist"]);
    }

    #[test]
    fn usage_and_errors() {
        let gs = groups();
        let c = ctx(&gs, &txt);
        assert_eq!(fb(&parse("/g OOC", &c)[0]), "<div><font color=CCChatCmdFeedbackError>Usage: /g &lt;group name&gt; message</font></div>");
        assert_eq!(fb(&parse("/o", &c)[0]), "<div><font color=CCChatCmdFeedbackError>Usage: /o &lt;message&gt;</font></div>");
        assert_eq!(fb(&parse("/shout", &c)[0]), "<div><font color=CCChatCmdFeedbackError>Usage: /shout &lt;message&gt;</font></div>");
        assert_eq!(fb(&parse("/invite", &c)[0]), "<div><font color=CCChatCmdFeedbackError>Usage: /invite &lt;nick&gt;</font></div>");
        assert_eq!(fb(&parse("/ch", &c)[0]), "<div><font color=CCChatCmdFeedbackError>Usage: /ch &lt;group name&gt;</font></div>");
        // no prefix matching of command words; /channel (in the help page) is not a command in the binary
        assert_eq!(fb(&parse("/chan x", &c)[0]), "<div><font color=CCChatCmdFeedbackError>No chat command or script named \"chan\" available.</font></div>");
        assert_eq!(fb(&parse("/channel x", &c)[0]), "<div><font color=CCChatCmdFeedbackError>No chat command or script named \"channel\" available.</font></div>");
        assert_eq!(fb(&parse("/g zzz hi", &c)[0]), "<div><font color=CCChatCmdFeedbackError>No chat-group named 'zzz'.</font></div>");
        assert_eq!(fb(&parse("/g Announcements hi", &c)[0]), "<div><font color=CCChatCmdFeedbackError>Group 'Announcements' is read-only.</font></div>");
        assert_eq!(fb(&parse("/ch Announcements", &c)[0]), "<div><font color=CCChatCmdFeedbackError>Group 'Announcements' is read-only.</font></div>");
        // /ch n matches Newbie Help + Neu Org News: ambiguous list, white = selectable
        let a = parse("/ch ne", &c);
        assert_eq!(fb(&a[0]), "<div><font color=CCChatCmdFeedbackError>ambiguous:<font color=white>Newbie Help<font color=white>, Neu Org News|/ch ne</font></div>");
        // gm-only forwarded command
        assert_eq!(fb(&parse("/clone x", &c)[0]), "<div><font color=CCChatCmdFeedbackError>not authorized</font></div>");
    }

    #[test]
    fn group_prefix_and_exact() {
        let gs = vec![g("OOC", GroupKind::Other), g("OOC Trade", GroupKind::Other)];
        let c = ctx(&gs, &txt);
        // exact (same length) match wins over longer prefix matches
        assert_eq!(parse("/g ooc x", &c), [ChatAction::Group { group: "OOC".into(), text: "x".into() }]);
        assert_eq!(parse("/g ooc t x", &c)[0], ChatAction::Group { group: "OOC".into(), text: "t x".into() }); // 3rd token = rest
        assert_eq!(parse("/g \"ooc t\" x", &c), [ChatAction::Group { group: "OOC Trade".into(), text: "x".into() }]);
    }

    #[test]
    fn unsubscribed_warning() {
        let mut gs = groups();
        gs[1].subscribed = false;
        let c = ctx(&gs, &txt);
        let a = parse("/g ooc hi", &c);
        assert_eq!(a[0], ChatAction::Group { group: "OOC".into(), text: "hi".into() });
        assert_eq!(fb(&a[1]), "<div><font color=CCChatCmdFeedbackError>warning unsub</font></div>");
        // /ch only selects groups shown in this window
        assert_eq!(fb(&parse("/ch ooc", &c)[0]), "<div><font color=CCChatCmdFeedbackError>only subscribed</font></div>");
    }

    #[test]
    fn afk_flow() {
        let gs = groups();
        let mut c = ctx(&gs, &txt);
        let on = parse("/afk lunch %m", &c);
        assert_eq!(on.len(), 3);
        assert!(fb(&on[0]).contains("AFK on."));
        assert_eq!(on[1], ChatAction::Emote("is afk".into()));
        assert_eq!(on[2], ChatAction::SetAfk(Some("lunch Me".into())));
        let bare = parse("/afk", &c);
        assert!(matches!(bare[0], ChatAction::AfkPrompt { .. }));
        assert_eq!(bare.last(), Some(&ChatAction::SetAfk(Some("The user is currently AFK.".into()))));
        c.afk = Some("lunch");
        let off = parse("/afk", &c);
        assert_eq!(off[0], ChatAction::SetAfk(None));
        assert!(fb(&off[1]).contains("AFK off."));
        assert_eq!(off[2], ChatAction::Emote("is back".into()));
        assert!(fb(&parse("/afk status", &c)[0]).contains("Current AFK message is 'lunch'."));
        let changed = parse("/afk dinner", &c);
        assert!(fb(&changed[0]).contains("from 'lunch' to 'dinner'"));
    }

    #[test]
    fn ignore_target_rules() {
        let gs = groups();
        let mut c = ctx(&gs, &txt);
        assert!(fb(&parse("/ignore", &c)[0]).contains("No target selected"));
        c.target = Some(Target { kind: 50000, id: 7, name: "Me".into() });
        assert!(fb(&parse("/ignore", &c)[0]).contains("Can't ignore yourself"));
        c.target = Some(Target { kind: 50000, id: 9, name: "Bob".into() });
        assert_eq!(parse("/ignore", &c), [ChatAction::IgnoreToggle { id: 9, name: Some("Bob".into()) }]);
        c.target = Some(Target { kind: 51000, id: 9, name: "Mob".into() });
        assert!(fb(&parse("/ignore", &c)[0]).contains("Can only ignore"));
    }

    #[test]
    fn forwarded_and_local() {
        let gs = groups();
        let mut c = ctx(&gs, &txt);
        assert_eq!(parse("/version", &c), [ChatAction::ZoneCommand("version".into())]);
        assert_eq!(parse("/team invite %m", &c), [ChatAction::ZoneCommand("team invite Me".into())]);
        assert_eq!(parse("/played", &c), [ChatAction::PlayedTime, ChatAction::ZoneCommand("played".into())]);
        assert_eq!(parse("/camp", &c), [ChatAction::Camp]);
        assert_eq!(parse("/quit", &c), [ChatAction::Quit]);
        assert_eq!(parse("/wave", &c), [ChatAction::Social(emote_id("wave").unwrap())]);
        assert_eq!(parse("/emote Hug", &c), [ChatAction::Social(emote_id("hug").unwrap())]);
        assert_eq!(emote_id("wave"), Some(62));
        assert_eq!(emote_id("facepalm"), Some(70));
        assert_eq!(parse("/inspect 1234", &c), [ChatAction::Inspect(1234)]);
        assert_eq!(parse("/getlocal 1", &c), [ChatAction::ZoneCommand("getlocal 1".into())]);
        assert_eq!(parse("/stuck Now %m", &c), [ChatAction::FanaticCommand("stuck Now %m".into())]);
        assert_eq!(parse("/tower create 3", &c), [ChatAction::Tower("tower create 3".into())]);
        assert_eq!(parse("/tower", &c), []);
        c.gm_level = 1;
        assert_eq!(parse("/clone", &c), [ChatAction::ZoneCommand("clone".into())]);
        c.chat_connected = false;
        assert!(fb(&parse("/cc addbuddy bob", &c)[0]).contains("Not connected to chat-server."));
        c.last_tell_from = None;
        assert!(fb(&parse("/r hi", &c)[0]).contains("You have not received any tell messages yet."));
        assert_eq!(reply_prefill(&c), "");
        c.last_tell_from = Some("Bob");
        assert_eq!(reply_prefill(&c), "/tell Bob ");
    }

    #[test]
    fn gui_local_dialog_commands() {
        let gs = groups();
        let mut c = ctx(&gs, &txt);
        // /showfile, /tipoftheday, /messagebox (FUN_100b6e66 / 0x100b6f0d / 0x100b7a6f)
        assert_eq!(parse("/showfile my file.html", &c), [ChatAction::ShowUrl("file://my file.html".into())]);
        assert_eq!(fb(&parse("/showfile", &c)[0]), "<div><font color=CCChatCmdFeedbackError>Usage: /showfile filename</font></div>");
        assert_eq!(parse("/tipoftheday", &c), [ChatAction::TipOfTheDay { prev: false }]);
        assert_eq!(parse("/TipOfTheDay PREV", &c), [ChatAction::TipOfTheDay { prev: true }]);
        c.own_name = "Me";
        assert_eq!(parse("/messagebox hi %m", &c), [ChatAction::MessageBox("hi Me".into())]);
        assert!(fb(&parse("/messagebox", &c)[0]).contains("Usage: /messagebox &lt;message&gt;"));
        // /text and /funcom are info lines (0x52)
        assert!(fb(&parse("/text hello %m", &c)[0]).starts_with("<div><font color=CCChatCmdFeedbackInfo>hello Me"));
        assert!(fb(&parse("/funcom", &c)[0]).contains("Funcom made this excellent product"));
        assert_eq!(parse("/text", &c), []);
        // /open /close /toggle: quotes stripped, table lookup is the hub's
        assert_eq!(parse("/open \"Inventory\"", &c), [ChatAction::Window { name: "Inventory".into(), op: WindowOp::Open }]);
        assert_eq!(parse("/close infoview", &c), [ChatAction::Window { name: "infoview".into(), op: WindowOp::Close }]);
        assert_eq!(parse("/toggle Nano", &c), [ChatAction::Window { name: "Nano".into(), op: WindowOp::Toggle }]);
        assert!(fb(&parse("/open", &c)[0]).contains("Usage: /open window"));
        assert_eq!(window_dvalue("infoview"), Some("info_window"));
        assert_eq!(window_dvalue("ItemStore"), Some("itemshop_window"));
        assert_eq!(window_dvalue("nope"), None);
        assert_eq!(WINDOW_NAMES.len(), 24);
        // /start only starts http(s) URLs
        assert_eq!(parse("/start https://example.org/x y", &c), [ChatAction::Start("https://example.org/x".into())]);
        assert_eq!(parse("/start ftp://example.org", &c), []);
        assert!(fb(&parse("/start", &c)[0]).contains("Usage: /start &lt;URL&gt;"));
    }

    /// The real text db has every key we use (category 10001).
    #[test]
    fn text_db_keys() {
        let Some(dir) = std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Games/ProjectRubiKa/client")) else { return };
        let Ok(db) = ao_formats::screens::TextDb::load(&dir) else { return };
        for k in [
            "AFK_IsBack", "AFK_AFKOff", "AFK_IsAFK", "AFK_AFKOn", "DefaultAFKReply", "AFKDialogBody", "ChangedAFKMessageTo",
            "AFK_Status_AFKIsCurrentlyOn", "AFK_Status_AFKIsCurrentlyOff", "CommandNotFound", "CommandNotAuthorized",
            "GroupIsReadOnly", "AmbiguousGroupName", "Ch_CanOnlySelectSubscribedGroups", "Ch_NoMatchingGroupAreSelectable",
            "AllMatchingGroupsAreReadOnly", "TalkToUnsubscribedChannel", "O_YouAreNotMemberOfAnOrganizaion",
            "T_YouAreNotMemberOfATeam", "Ignore_IgnoringCharacter", "Ignore_UnignoringCharacter", "Ignore_CantIgnoreYourself",
            "Ignore_NoTargetOrNick", "Ignore_CanOnlyIgnoreCharacters",
        ] {
            assert!(db.by_key(10001, &format!("ChatCmdFeedback_{k}")).is_some(), "missing ChatCmdFeedback_{k}");
        }
        assert!(db.by_key(10001, "Error2FewArgs").is_some());
        let text = |k: &str| db.by_key(10001, k).unwrap_or_default();
        let gs = groups();
        let c = ctx(&gs, &text);
        assert_eq!(fb(&parse("/xyz", &c)[0]), "<div><font color=CCChatCmdFeedbackError>No chat command or script named \"xyz\" available.</font></div>");
    }
}
