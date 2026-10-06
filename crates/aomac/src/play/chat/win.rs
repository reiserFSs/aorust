//! The chat windows: `ChatGUIModule_c` (GUI.dll 0x1008751b) window layer -- `ChatWindow_c` (a `Window`, vtable class created by
//! `FUN_10097ae3`), one `GroupChatView_c` per tab (`FUN_100abfa3`), its `ChatView_c` (`FUN_10090893`: text area + `InputBar_c`),
//! the per-window group selection and the line formatting of `FUN_1009b4cf`. Evidence and gaps: `docs/chat/gui.md`.

use super::line::{ChatKind, ChatLine, ChatMsg};
use anyhow::Result;
use ao_formats::screens::TextDb;
use ao_gui::{xml, Event, Gui, WindowId, WindowSize};
use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// What a window hands back to the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WinOut {
    /// Enter in a window's input bar. `window_group` = the window's output group (`output_group`, `#%016x#`) if it has one.
    Submit { text: String, window_group: Option<String> },
    /// A link other than `user://` / `chatgroup://` (`itemref://`, `charref://`, `chatcmd:///...`: `ChatGUIModule_c::ShowItemRefLink` 0x10085cb5).
    LinkClicked(String),
    /// `user://NAME` clicked (`ChatView_c` signal +0x130 -> `OpenTellWindow` 0x10085df8).
    OpenTell(String),
    /// "IgnoreUser" of a user link's menu (`FUN_1008dd24`): toggles ignoring the character (`/ignore NAME`).
    IgnoreUser(String),
}

// ------------------------------------------------------------------------------------------------------------ groups

pub const G_SYSTEM: u64 = 0x4000_0001;
pub const G_VICINITY: u64 = 0x4000_0002;
pub const G_TELL: u64 = 0x4000_0003;
pub const G_MYPET: u64 = 0x4100_0000;
pub const G_OTHERPET: u64 = 0x4100_0001;
pub const G_RESEARCH: u64 = 0x4200_001c;

/// Local groups registered by `FUN_10083e53` (0x10083e53): `FUN_10083be4(name, id, 0, kind, 1)` in this order.
pub const LOCAL_GROUPS: &[(u64, &str)] = &[
    (0x4000_0002, "Vicinity"),
    (0x4200_001c, "Research"),
    (0x4000_0001, "System"),
    (0x4000_0003, "Tell Messages"),
    (0x4100_0000, "Your Pets"),
    (0x4100_0001, "Other Pets"),
    (0x4200_0002, "Me hit by nano"),
    (0x4200_0003, "Your pet hit by nano"),
    (0x4200_0004, "Other hit by nano"),
    (0x4200_0005, "You hit other with nano"),
    (0x4200_0006, "Me hit by monster"),
    (0x4200_0007, "Me hit by player"),
    (0x4200_0008, "You hit other"),
    (0x4200_0009, "Your pet hit by other"),
    (0x4200_000a, "Other hit by other"),
    (0x4200_000b, "Me got XP"),
    (0x4200_000c, "Me got SK"),
    (0x4200_0001, "Me hit by environment"),
    (0x4200_0011, "Your pet hit by monster"),
    (0x4200_0012, "Your misses"),
    (0x4200_0013, "Other misses"),
    (0x4200_0014, "You gave health"),
    (0x4200_0015, "Me got health"),
    (0x4200_0016, "Me got nano"),
    (0x4200_0017, "You gave nano"),
    (0x4200_001a, "Team Loot Messages"),
    (0x4200_001b, "Vicinity Loot Messages"),
    (0x4200_0018, "Me Cast Nano"),
];

/// `GetGroupIdentifier` (0x1001b5d5) text form of a group id, as stored in `output_group`: `#%016I64x#`.
pub fn group_ident(id: u64) -> String {
    format!("#{id:016x}#")
}

fn parse_ident(s: &str) -> Option<u64> {
    u64::from_str_radix(s.trim().trim_matches('"').trim_matches('#'), 16).ok()
}

/// `FUN_10085320` (0x10085320): `TextColors.xml` name of a group's text. `kind` = vicinity message kind.
pub fn group_color(id: u64, kind: u8) -> &'static str {
    let (hi, lo) = (id >> 32, id & 0xffff_ffff);
    if hi == 0 && lo < 0x4200_001d {
        match lo {
            G_SYSTEM => return "ct_system",
            G_VICINITY => {
                return match kind {
                    1 => "ctch_whisper",
                    2 => "ctch_shout",
                    3 => "ctch_emote",
                    _ => "ctch_vicinity",
                }
            }
            G_TELL => return "ctch_tell",
            G_MYPET => return "ctch_mypet",
            G_OTHERPET => return "ctch_otherpet",
            G_RESEARCH => return "ctch_research",
            _ => {}
        }
    }
    match hi as u8 {
        10 => "ctch_tower",
        1 => "ctch_admin",
        3 => "ctch_clan",
        4 => "ctch_misc",
        5 => "ctch_gm",
        8 => "ctch_news",
        0x0e => "ctch_pgroup",
        0x82 => "ctch_team",
        0x86 => "ctch_seekingteam",
        0x87 => "ctch_newbie",
        0x8f => "ctch_raid",
        _ => "white",
    }
}

// ------------------------------------------------------------------------------------------------------- line format

/// Local time `(%H:%M) ` (`strftime` in `FUN_1009b4cf`).
fn timestamp() -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let (h, m, _) = super::zonecmd::local_time(now).unwrap_or_default();
    format!("({h:02}:{m:02}) ")
}

fn user_link(name: &str, gm: bool) -> String {
    if gm {
        format!("<font color=\"#FF0000\"><a style=\"text-decoration:none\" href=\"user://{name}\">{name} (GM)</a></font>")
    } else {
        format!("<a style=\"text-decoration:none\" href=\"user://{name}\">{name}</a>")
    }
}

/// `FUN_1009b4cf`: the HTML of one chat line. `stamp` = the window's `show_timestamps` text (`(%H:%M) ` or empty).
/// `whispers`/`shouts` = text-db 10001 `Whispers` / `Shouts` (the infix after the sender in the vicinity channel).
fn format_line(m: &ChatMsg, group_name: &str, color: &str, stamp: &str, whispers: &str, shouts: &str) -> String {
    let (hi, lo) = (m.group >> 32, m.group & 0xffff_ffff);
    let sender = (!m.from_name.is_empty()).then(|| user_link(&m.from_name, m.flags & 1 != 0));
    let glink = || format!("<a style=\"text-decoration:none\" href=\"chatgroup://{}\">{group_name}</a>", group_ident(m.group));
    let mut s = format!("<div indent=wrapped><font color={color}>{stamp}");
    let chat_style = |s: &mut String| {
        let _ = write!(s, "[{}] ", glink());
        if let Some(l) = &sender {
            let _ = write!(s, "{l}: ");
        }
    };
    if hi != 0 || lo > 0x4200_001c {
        chat_style(&mut s);
    } else if (0x4200_0000..=0x4200_0019).contains(&lo) || lo == G_SYSTEM || lo == G_RESEARCH {
        // combat/system/research: no prefix
    } else if lo == G_VICINITY {
        if let Some(l) = &sender {
            match m.kind {
                1 => s += &format!("{l}{whispers}"),
                2 => s += &format!("{l}{shouts}"),
                3 => s += &format!("{l} "),
                _ => s += &format!("{l}: "),
            }
        }
    } else if lo == G_TELL {
        if let Some(l) = &sender {
            let _ = write!(s, "[{l}]: ");
        }
    } else if (0x4100_0000..0x4100_0002).contains(&lo) {
        if let Some(l) = &sender {
            let _ = write!(s, "{l}: ");
        }
    } else {
        chat_style(&mut s);
    }
    s += &m.text;
    s += "</font></div>";
    s
}

// ------------------------------------------------------------------------------------------------------ window config

/// One `Chat/Windows/WindowN/Config.xml` (`FUN_1009a77b` writer, `FUN_1009dc28` reader).
#[derive(Debug, Clone, PartialEq)]
pub struct Cfg {
    pub name: String,
    pub window_name: String,
    pub startup: bool,
    pub open: bool,
    pub autosubscribe: bool,
    pub fading: bool,
    pub logged: bool,
    pub clickthrough: bool,
    pub textinput: bool,
    pub deactivate_on_send: bool,
    pub hide_input_when_inactive: bool,
    pub show_timestamps: bool,
    pub alpha_active: f32,
    pub alpha_inactive: f32,
    /// 0 = none.
    pub output_group: u64,
    /// 0 normal (style 0 window), 1/2 border (style 3, transparent).
    pub visual_mode: i32,
    /// `selected_group_ids`: the groups the window *excludes* when `autosubscribe`, else the groups it shows.
    pub set: Vec<u64>,
    /// `chat_window_config/WindowFrame` (l, t, r, b), inclusive.
    pub frame: Option<[f32; 4]>,
    /// Loaded from the client template / code defaults, not from the character's own saved config (not serialized).
    pub template: bool,
    /// `tab_index`: position of this window among the tabs of its `ChatWindow` (`FUN_100974b5` inserts before the first tab with a greater index).
    pub tab_index: i32,
    /// `chat_window_config/is_frontmost` / `is_backmost` -> window flags 0x100 / 0x200 (`FUN_10097ae3`).
    pub frontmost: bool,
    pub backmost: bool,
}

impl Cfg {
    /// Defaults of `FUN_1009dc28` for keys absent from the file (0.8/0.3 = `_DAT_101b8a54` / `_DAT_101ae0ec`).
    fn blank(name: &str, window_name: &str) -> Cfg {
        Cfg {
            name: name.into(),
            window_name: window_name.into(),
            startup: false,
            open: true,
            autosubscribe: false,
            fading: false,
            logged: false,
            clickthrough: false,
            textinput: true,
            deactivate_on_send: true,
            hide_input_when_inactive: false,
            show_timestamps: false,
            alpha_active: 0.8,
            alpha_inactive: 0.3,
            output_group: 0,
            visual_mode: 2,
            set: vec![],
            frame: None,
            template: false,
            tab_index: 0,
            frontmost: false,
            backmost: false,
        }
    }

    pub fn parse(src: &str) -> Option<Cfg> {
        let root = xml::parse(src).ok()?;
        let find = |n: &str| root.children.iter().find(|c| c.attr("name") == Some(n));
        let val = |n: &str| find(n).and_then(|c| c.attr("value"));
        let s = |n: &str| val(n).map(|v| v.trim_matches('"').to_string());
        let b = |n: &str, d: bool| val(n).map_or(d, |v| v == "true");
        let f = |n: &str, d: f32| val(n).and_then(|v| v.parse().ok()).unwrap_or(d);
        let mut c = Cfg::blank(&s("name")?, &s("window_name").unwrap_or_default());
        c.startup = b("is_startup_window", false);
        c.open = b("is_window_open", true);
        c.autosubscribe = b("is_autosubscribe_window", false);
        c.fading = b("is_message_fading_enabled", false);
        c.logged = b("is_logged", false);
        c.clickthrough = b("is_clickthrough", false);
        c.textinput = b("is_textinput_enabled", true);
        c.deactivate_on_send = b("deactivate_on_send", true);
        c.hide_input_when_inactive = b("hide_input_when_inactive", false);
        c.show_timestamps = b("show_timestamps", false);
        c.alpha_active = f("window_transparency_active", 0.8);
        c.alpha_inactive = f("window_transparency_inactive", 0.3);
        c.visual_mode = val("visual_mode").and_then(|v| v.parse().ok()).unwrap_or(2);
        c.output_group = s("output_group").and_then(|g| parse_ident(&g)).unwrap_or(0);
        if let Some(a) = find("selected_group_ids") {
            c.set = a.children.iter().filter_map(|e| e.attr("value")?.parse().ok()).collect();
        }
        if let Some(w) = find("chat_window_config").and_then(|a| a.children.iter().find(|e| e.attr("name") == Some("WindowFrame"))) {
            let v = w.attr("value")?.trim().strip_prefix("Rect(")?.strip_suffix(')')?;
            let n: Vec<f32> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if n.len() == 4 {
                c.frame = Some([n[0], n[1], n[2], n[3]]);
            }
        }
        let flag = |k: &str| find("chat_window_config").is_some_and(|a| a.children.iter().any(|e| e.attr("name") == Some(k) && e.attr("value") == Some("true")));
        c.frontmost = flag("is_frontmost");
        c.backmost = flag("is_backmost");
        c.tab_index = val("tab_index").and_then(|v| v.parse().ok()).unwrap_or(0);
        Some(c)
    }

    /// Same schema the original writes (`FUN_1009a77b`), group names from `names`.
    pub fn to_xml(&self, names: &dyn Fn(u64) -> String) -> String {
        let mut o = String::from("<Archive code=\"0\">\n    <Array name=\"selected_group_ids\">\n");
        for g in &self.set {
            let _ = writeln!(o, "        <Int64 value=\"{g}\" />");
        }
        o += "    </Array>\n    <Array name=\"selected_group_names\">\n";
        for g in &self.set {
            let _ = writeln!(o, "        <String value='&quot;{}&quot;' />", names(*g));
        }
        o += "    </Array>\n    <Archive code=\"0\" name=\"log_window_config\" />\n    <Archive code=\"0\" name=\"chat_window_config\">\n";
        if let Some(r) = self.frame {
            let _ = writeln!(o, "        <Rect name=\"WindowFrame\" value=\"Rect({:.6},{:.6},{:.6},{:.6})\" />", r[0], r[1], r[2], r[3]);
        }
        let _ = writeln!(o, "        <Bool name=\"is_backmost\" value=\"{}\" />\n        <Bool name=\"is_frontmost\" value=\"{}\" />", self.backmost, self.frontmost);
        o += "    </Archive>\n    <Archive code=\"0\" name=\"chat_view_config\" />\n";
        let _ = writeln!(o, "    <Int32 name=\"visual_mode\" value=\"{}\" />", self.visual_mode);
        let og = if self.output_group == 0 { String::new() } else { group_ident(self.output_group) };
        let _ = writeln!(o, "    <String name=\"output_group\" value='&quot;{og}&quot;' />");
        let _ = writeln!(o, "    <Float name=\"window_transparency_inactive\" value=\"{:.6}\" />", self.alpha_inactive);
        let _ = writeln!(o, "    <Float name=\"window_transparency_active\" value=\"{:.6}\" />", self.alpha_active);
        for (k, v) in [
            ("show_timestamps", self.show_timestamps),
            ("hide_input_when_inactive", self.hide_input_when_inactive),
            ("deactivate_on_send", self.deactivate_on_send),
            ("is_textinput_enabled", self.textinput),
            ("is_clickthrough", self.clickthrough),
            ("is_logged", self.logged),
            ("is_message_fading_enabled", self.fading),
            ("is_autosubscribe_window", self.autosubscribe),
            ("is_window_open", self.open),
        ] {
            let _ = writeln!(o, "    <Bool name=\"{k}\" value=\"{v}\" />");
        }
        let _ = writeln!(o, "    <Int32 name=\"tab_index\" value=\"{}\" />", self.tab_index);
        let _ = writeln!(o, "    <String name=\"window_name\" value='&quot;{}&quot;' />", self.window_name);
        let _ = writeln!(o, "    <Bool name=\"is_default_window\" value=\"false\" />");
        let _ = writeln!(o, "    <Bool name=\"is_startup_window\" value=\"{}\" />", self.startup);
        let _ = writeln!(o, "    <String name=\"name\" value='&quot;{}&quot;' />\n</Archive>", self.name);
        o
    }

    /// Does the window show group `id`? (`autosubscribe` windows hold the *excluded* groups -- docs/chat/gui.md §4.)
    pub fn shows(&self, id: u64) -> bool {
        self.set.contains(&id) != self.autosubscribe
    }
}

/// `FUN_10094c58` (0x10094c58) when no window config exists: "Default Window" (startup, autosubscribe, excludes Other Pets and the
/// combat groups) and "Combat" (shows the combat groups).
fn code_defaults() -> Vec<Cfg> {
    const COMBAT_ALL: &[u32] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 27, 24];
    const COMBAT_WIN: &[u32] = &[1, 2, 3, 5, 6, 7, 8, 9, 13, 14, 15, 16, 17, 18, 20, 21, 22, 23, 24];
    let ids = |l: &[u32]| l.iter().map(|n| 0x4200_0000u64 | *n as u64).collect::<Vec<_>>();
    let mut d = Cfg::blank("Default Window", "Window1");
    d.startup = true;
    d.autosubscribe = true;
    d.output_group = G_VICINITY;
    d.set = [vec![G_OTHERPET], ids(COMBAT_ALL)].concat();
    let mut c = Cfg::blank("Combat", "Window2");
    c.textinput = false;
    c.output_group = 0;
    c.set = ids(COMBAT_WIN);
    vec![d, c]
}

fn read_windows(dir: &Path) -> Vec<Cfg> {
    let mut v: Vec<(String, Cfg)> = std::fs::read_dir(dir.join("Chat/Windows"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let c = Cfg::parse(&std::fs::read_to_string(e.path().join("Config.xml")).ok()?)?;
            Some((e.file_name().to_string_lossy().into_owned(), c))
        })
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v.into_iter().map(|x| x.1).collect()
}

/// Outer rectangle (x, y, w, h) of a window.
///
/// Every supplied frame, irrespective of source: `Window::LoadWndConfig` 0x10154d6e =
/// `WndBorder::SetClientFrame(WindowFrame)` (absolute top-left coordinates, inclusive Rect),
/// followed by `Window::MoveInsideScreen(false, true, true)` 0x10154abc: translate, never resize.
/// There is no resolution scaling or HUD avoidance.
/// No frame: `Window::MoveToCenter`, 400 x 200 (`_DAT_101b1840` / `_DAT_101a959c`, `FUN_10097ae3`).
fn place(frame: Option<[f32; 4]>, screen: (u32, u32)) -> (i32, i32, u32, u32) {
    let (sw, sh) = (screen.0 as f32, screen.1 as f32);
    match frame {
        Some([l, t, r, b]) => {
            let (w, h) = (r - l + 1.0, b - t + 1.0);
            let dx = if l < 0.0 { -l } else if r > sw - 1.0 { (sw - 1.0 - r).max(-l) } else { 0.0 };
            let dy = if t < 0.0 { -t } else if b > sh - 1.0 { (sh - 1.0 - b).max(-t) } else { 0.0 };
            ((l + dx) as i32, (t + dy) as i32, w as u32, h as u32)
        }
        None => {
            let (w, h) = (400u32.min(screen.0), 200u32.min(screen.1));
            (((screen.0 - w) / 2) as i32, ((screen.1 - h) / 2) as i32, w, h)
        }
    }
}

/// The window alpha a chat window rests at (`FUN_10096b23`, `FUN_10096ec5`): mode 0 (framed "Normal") calls `Window::FadeTo(1.0)` and never fades; modes 1/2
/// fade to `window_transparency_active` while the window is active, else to `window_transparency_inactive`.
fn alpha_of(c: &Cfg, active: bool) -> f32 {
    match (c.visual_mode, active) {
        (0, _) => 1.0,
        (_, true) => c.alpha_active,
        (_, false) => c.alpha_inactive,
    }
}

// ----------------------------------------------------------------------------------------------------------- windows

const MAX_LINES: usize = 100; // `FUN_10088e92` drops the oldest line when the window holds more than 100
const FADE_IN: f32 = 0.2; // `FUN_10096b23`: FadeTo(active alpha, 200000 us)
const FADE_OUT: f32 = 1.0; // FadeTo(inactive alpha, 1000000 us)

struct Win {
    cfg: Cfg,
    /// The GUI window of the frame that holds this window's tab.
    id: WindowId,
    /// Index into `ChatWindows::frames`.
    frame: usize,
    n: usize,
    lines: VecDeque<String>,
    alpha: f32,
    target: f32,
    /// Alpha change per second of the running fade.
    rate: f32,
    active: bool,
    /// Document flag `+0x160` (`FUN_100989fa`): a line arrived while the tab was not visible and it was not selected since; `FUN_100ab980` then wraps the
    /// tab name in `<font color=red>`.
    unread: bool,
    /// Message-fade parameters currently applied to the GUI view (`FUN_1009349d`: delay, time in seconds); `None` = fading off. Reset when the GUI window is rebuilt.
    fade: Option<(f32, f32)>,
    /// Prompt text currently set on the input bar (the output group's name while the editor is empty, `InputBar_c`), to avoid re-setting it every frame.
    hint: String,
}

impl Win {
    fn input(&self) -> String {
        format!("input_{}", self.n)
    }
    fn scroll(&self) -> String {
        format!("scroll_{}", self.n)
    }
    fn chat(&self) -> String {
        format!("chat_{}", self.n)
    }
}

/// One `ChatWindow_c`: a GUI window with the tabs of its windows (a visual-mode-2 window has exactly one tab and no frame).
struct Frame {
    id: WindowId,
    /// Indices into `ChatWindows::wins` in tab order (`tab_index`).
    docs: Vec<usize>,
    /// Selected tab (index into `docs`).
    sel: usize,
    /// Last outer frame (x, y, w, h): what `Window::SaveWndConfig` writes as `WindowFrame`.
    placed: (i32, i32, u32, u32),
}

/// The chat DValues the windows follow live (`DistributedValue_c`, defaults of `LoginPrefs.xml` / `MainPrefs.xml`): the hub copies them in every frame
/// ([`ChatWindows::set_prefs`]); a change re-titles the tabs, shows/hides the input prompt and swaps the CHAT font.
#[derive(Debug, Clone, PartialEq)]
pub struct WinPrefs {
    /// `ChatShowOGrpInTitleBar` (default true; read by `FUN_100ab980`): the tab title gets ` [<output group>]`.
    pub title_group: bool,
    /// `ChatShowOGrpInInputBar` (default true; read by `FUN_10090f0b`): the prompt overlay of an empty input bar.
    pub input_group: bool,
    /// `ChatFontName` / `ChatFontStyle` / `ChatFontSize` (tenths of a point; defaults Verdana / Regular / 140).
    pub font: (String, String, i32),
    /// `ChatTextFadeDelay` / `ChatTextFadeTime` seconds (`FUN_1008f432`; defaults 8.0 / 0.3 of `MainPrefs.xml`): used by windows with `is_message_fading_enabled`.
    pub fade: (f32, f32),
}

impl Default for WinPrefs {
    fn default() -> Self {
        WinPrefs { title_group: true, input_group: true, font: ("Verdana".into(), "Regular".into(), 140), fade: (8.0, 0.3) }
    }
}

impl WinPrefs {
    /// The values from the registry; a missing or mistyped variable keeps its default.
    pub fn from_dvalues(d: &crate::play::dvalue::DValues) -> Self {
        use crate::play::dvalue::Variant;
        let base = WinPrefs::default();
        let flag = |n: &str, def: bool| d.get_i64(n).map_or(def, |v| v != 0);
        let text = |n: &str, def: String| match d.get(n) {
            Some(Variant::Str(s)) if !s.is_empty() => s.clone(),
            _ => def,
        };
        let size = match d.get("ChatFontSize") {
            Some(Variant::Int(v)) => *v as i32,
            Some(Variant::Float(v)) => *v as i32,
            _ => base.font.2,
        };
        // `FUN_1008f432`: `AsDouble * 1e6` (`_DAT_101ae2f8`) truncated to whole microseconds
        let secs = |n: &str, def: f32| d.get_f32(n).map_or(def, |v| ((v as f64 * 1e6).trunc() / 1e6) as f32);
        WinPrefs {
            fade: (secs("ChatTextFadeDelay", base.fade.0), secs("ChatTextFadeTime", base.fade.1)),
            title_group: flag("ChatShowOGrpInTitleBar", base.title_group),
            input_group: flag("ChatShowOGrpInInputBar", base.input_group),
            font: (text("ChatFontName", base.font.0), text("ChatFontStyle", base.font.1), size),
        }
    }
}

impl ChatWindows {
    /// Closes every chat window frame.
    pub fn close_all(&mut self, gui: &mut Gui) {
        for f in self.frames.drain(..) {
            gui.close_window(f.id);
        }
    }
}

pub struct ChatWindows {
    wins: Vec<Win>,
    frames: Vec<Frame>,
    screen: (u32, u32),
    groups: HashMap<u64, String>,
    text: Option<TextDb>,
    prefs: Option<PathBuf>,
    /// `ChatLastActiveWindow` (`window_name`).
    last_active: String,
    next_n: usize,
    /// A frame / tab / setting changed and has not been written yet ([`ChatWindows::update`] saves once the pointer is idle).
    dirty: bool,
    /// What the open popup menu acts on.
    menu: Option<menu::Ctx>,
    /// Chat DValues in effect ([`WinPrefs`]).
    pw: WinPrefs,
}

/// `ChatView_c` text view flags 0xe6c (`FUN_100925ff`): ENABLE_SHADOW | FILL_BOTTOM_UP | DISABLE_RC_MENU | WORD_WRAP | MULTILINE |
/// ALLOW_TEXT_SELECTION | ACCEPT_MOUSE_INPUT.
const TEXT_FLAGS: &str = "TVF_ENABLE_SHADOW|TVF_FILL_BOTTOM_UP|TVF_DISABLE_RC_MENU|TVF_WORD_WRAP|TVF_MULTILINE|TVF_ALLOW_TEXT_SELECTION|TVF_ACCEPT_MOUSE_INPUT";
/// `InputBar_c` (`FUN_100919ab`) editor flags 0x800f (the part ao-gui names): ACCEPT_TXT_INPUT | ACCEPT_MOUSE_INPUT | ALLOW_TEXT_SELECTION | ENABLE_SHADOW.
const INPUT_FLAGS: &str = "TVF_ACCEPT_TXT_INPUT|TVF_ACCEPT_MOUSE_INPUT|TVF_ALLOW_TEXT_SELECTION|TVF_ENABLE_SHADOW";

mod menu;
mod windows;

#[cfg(test)]
mod tests;
