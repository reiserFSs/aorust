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
        gmtoff: i64,
        zone: *const u8,
    }
    extern "C" {
        fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let mut tm = Tm { sec: 0, min: 0, hour: 0, mday: 0, mon: 0, year: 0, wday: 0, yday: 0, isdst: 0, gmtoff: 0, zone: std::ptr::null() };
    // SAFETY: localtime_r fills the caller-provided struct (layout of macOS `struct tm`).
    unsafe { localtime_r(&now, &mut tm) };
    format!("({:02}:{:02}) ", tm.hour, tm.min)
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
        o += "        <Bool name=\"is_backmost\" value=\"false\" />\n        <Bool name=\"is_frontmost\" value=\"false\" />\n    </Archive>\n    <Archive code=\"0\" name=\"chat_view_config\" />\n";
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
        let _ = writeln!(o, "    <Int32 name=\"tab_index\" value=\"0\" />");
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

/// Reference screen of the shipped window frames (the largest right edge in `prefs/NewChar` is 2303): **UNRESOLVED/guess** --
/// `Window::LoadWndConfig` is not decompiled, we scale frames from this screen to ours and clamp them inside it.
const REF_SCREEN: (f32, f32) = (2304.0, 1440.0);

fn place(frame: Option<[f32; 4]>, screen: (u32, u32)) -> (i32, i32, u32, u32) {
    let (sw, sh) = (screen.0 as f32, screen.1 as f32);
    match frame {
        Some([l, t, r, b]) => {
            let (kx, ky) = ((sw / REF_SCREEN.0).min(1.0), (sh / REF_SCREEN.1).min(1.0));
            let w = (((r - l + 1.0) * kx).round().max(120.0)).min(sw);
            let h = (((b - t + 1.0) * ky).round().max(60.0)).min(sh);
            let x = (l * kx).round().clamp(0.0, sw - w);
            let y = (t * ky).round().clamp(0.0, sh - h);
            (x as i32, y as i32, w as u32, h as u32)
        }
        // `Window::MoveToCenter`, size `_DAT_101b1840` x `_DAT_101a959c` = 400 x 200 (`FUN_10097ae3`)
        None => {
            let (w, h) = (400u32.min(screen.0), 200u32.min(screen.1));
            (((screen.0 - w) / 2) as i32, ((screen.1 - h) / 2) as i32, w, h)
        }
    }
}

// ----------------------------------------------------------------------------------------------------------- windows

const MAX_LINES: usize = 100; // `FUN_10088e92` drops the oldest line when the window holds more than 100
const FADE_IN: f32 = 0.2; // `FUN_10096b23`: FadeTo(active alpha, 200000 us)
const FADE_OUT: f32 = 1.0; // FadeTo(inactive alpha, 1000000 us)

struct Win {
    cfg: Cfg,
    id: WindowId,
    n: usize,
    lines: VecDeque<String>,
    alpha: f32,
    target: f32,
    /// Alpha change per second of the running fade.
    rate: f32,
    active: bool,
}

impl Win {
    fn input(&self) -> String {
        format!("input_{}", self.n)
    }
    fn scroll(&self) -> String {
        format!("scroll_{}", self.n)
    }
}

pub struct ChatWindows {
    wins: Vec<Win>,
    screen: (u32, u32),
    groups: HashMap<u64, String>,
    text: Option<TextDb>,
    prefs: Option<PathBuf>,
    /// `ChatLastActiveWindow` (`window_name`).
    last_active: String,
    next_n: usize,
}

/// `ChatView_c` text view flags 0xe6c (`FUN_100925ff`): ENABLE_SHADOW | FILL_BOTTOM_UP | DISABLE_RC_MENU | WORD_WRAP | MULTILINE |
/// ALLOW_TEXT_SELECTION | ACCEPT_MOUSE_INPUT.
const TEXT_FLAGS: &str = "TVF_ENABLE_SHADOW|TVF_FILL_BOTTOM_UP|TVF_DISABLE_RC_MENU|TVF_WORD_WRAP|TVF_MULTILINE|TVF_ALLOW_TEXT_SELECTION|TVF_ACCEPT_MOUSE_INPUT";
/// `InputBar_c` (`FUN_100919ab`) editor flags 0x800f (the part ao-gui names): ACCEPT_TXT_INPUT | ACCEPT_MOUSE_INPUT | ALLOW_TEXT_SELECTION | ENABLE_SHADOW.
const INPUT_FLAGS: &str = "TVF_ACCEPT_TXT_INPUT|TVF_ACCEPT_MOUSE_INPUT|TVF_ALLOW_TEXT_SELECTION|TVF_ENABLE_SHADOW";

impl ChatWindows {
    /// Opens the windows from (first hit): `<prefs dir>/Chat/Windows/*/Config.xml`, the client's `prefs/NewChar/Chat/Windows/*`,
    /// else the code defaults of `FUN_10094c58`.
    pub fn new(gui: &mut Gui, screen: (u32, u32)) -> Result<Self> {
        let client = ao_gui::client_dir();
        let prefs = super::super::prefs::dir();
        let mut cfgs = prefs.as_deref().map(read_windows).unwrap_or_default();
        if cfgs.is_empty() {
            cfgs = read_windows(&client.join("prefs/NewChar"));
        }
        if cfgs.is_empty() {
            cfgs = code_defaults();
        }
        let mut s = ChatWindows {
            wins: vec![],
            screen,
            groups: LOCAL_GROUPS.iter().map(|(i, n)| (*i, n.to_string())).collect(),
            text: TextDb::load(&client).ok(),
            prefs,
            last_active: String::new(),
            next_n: 0,
        };
        s.last_active = cfgs.iter().find(|c| c.startup).or(cfgs.first()).map(|c| c.window_name.clone()).unwrap_or_default();
        for c in cfgs.into_iter().filter(|c| c.open) {
            s.open(gui, c)?;
        }
        Ok(s)
    }

    fn open(&mut self, gui: &mut Gui, cfg: Cfg) -> Result<usize> {
        let n = self.next_n;
        self.next_n += 1;
        let (x, y, w, h) = place(cfg.frame, self.screen);
        let line_h = gui.font_height(ao_gui::FontId::Chat) as u32;
        // `ChatView_c::FUN_1008d728`, input mode 1: input bar at its preferred height (one text line in a 5 px border), the text
        // frame ends 5 px above it; the GroupChatView border is 3 px (`_DAT_101a96d4`).
        let input = if cfg.textinput {
            format!(
                r#"<BorderView name="input_border_{n}" layout_borders="Rect(0,5,0,0)" min_size="Point(-1,{ih})" max_size="Point(16000,{ih})">
                     <TextView name="input_{n}" max_size="Point(16000,-1)" layout_borders="Rect(5,5,5,5)" font="CHAT" feature_flags="{INPUT_FLAGS}"/>
                   </BorderView>"#,
                ih = line_h + 10
            )
        } else {
            String::new()
        };
        let src = format!(
            r#"<root><View name="chat_{n}" view_layout="vertical" layout_borders="Rect(3,3,3,3)">
                 <BorderView name="text_border_{n}" max_size="Point(16000,16000)">
                   <ScrollView name="scroll_{n}" v_scrollbar_mode="always" layout_borders="Rect(5,5,5,5)" max_size="Point(16000,16000)">
                     <ScrollViewChild view_layout="vertical" max_size="Point(16000,16000)">
                       <TextView name="text_{n}" max_size="Point(16000,-1)" font="CHAT" feature_flags="{TEXT_FLAGS}"/>
                     </ScrollViewChild>
                   </ScrollView>
                 </BorderView>
                 {input}
               </View></root>"#
        );
        let id = gui.open_window_xml("ChatWindow", &src, (x, y), WindowSize::Fixed(w, h))?;
        gui.set_text_shadow_offset(1, 1); // `ChatTextShadowOffset` pref default 1 (CharPrefs.xml), `FUN_1008d673`
        let alpha = cfg.alpha_inactive;
        gui.set_window_alpha(id, alpha);
        self.wins.push(Win { cfg, id, n, lines: VecDeque::new(), alpha, target: alpha, rate: 0.0, active: false });
        Ok(self.wins.len() - 1)
    }

    pub fn resize(&mut self, gui: &mut Gui, screen: (u32, u32)) {
        if screen == self.screen {
            return;
        }
        self.screen = screen;
        for w in &self.wins {
            let (x, y, ww, hh) = place(w.cfg.frame, screen);
            gui.set_window_pos(w.id, (x, y));
            gui.resize_window(w.id, WindowSize::Fixed(ww, hh));
        }
    }

    /// `ChatGUIModule_c::AddGroup` (0x10085f91): a group the chat server announced (windows that do not exclude it show it).
    pub fn add_group(&mut self, id: u64, name: &str) {
        self.groups.insert(id, name.to_string());
    }

    pub fn remove_group(&mut self, id: u64) {
        if id >> 32 != 0 {
            self.groups.remove(&id);
        }
    }

    fn group_name(&self, id: u64) -> String {
        self.groups.get(&id).cloned().unwrap_or_default()
    }

    fn fill(&mut self, gui: &mut Gui, i: usize, html: &str) {
        let w = &mut self.wins[i];
        w.lines.push_back(html.to_string());
        while w.lines.len() > MAX_LINES {
            w.lines.pop_front();
        }
        let all = w.lines.iter().map(String::as_str).collect::<Vec<_>>().join("<br>");
        gui.set_text(w.id, &format!("text_{}", w.n), &all);
        gui.scroll_to_bottom(w.id, &w.scroll());
    }

    fn deliver(&mut self, gui: &mut Gui, group: u64, make: impl Fn(&str) -> String) {
        for i in 0..self.wins.len() {
            if self.wins[i].cfg.shows(group) {
                let stamp = if self.wins[i].cfg.show_timestamps { timestamp() } else { String::new() };
                let html = make(&stamp);
                self.fill(gui, i, &html);
            }
        }
    }

    /// A message of the chat server / zone chat (`FUN_10084f9e` -> `FUN_1009b4cf`).
    pub fn push_msg(&mut self, gui: &mut Gui, m: &ChatMsg) {
        let mut m = m.clone();
        if m.tell && m.group == 0 {
            m.group = G_TELL;
        }
        let name = if m.group_name.is_empty() { self.group_name(m.group) } else { m.group_name.clone() };
        if m.group >> 32 != 0 && !name.is_empty() {
            self.groups.entry(m.group).or_insert_with(|| name.clone());
        }
        let color = group_color(m.group, m.kind);
        let db = self.text.as_ref();
        let whispers = db.and_then(|d| d.by_key(10001, "Whispers")).unwrap_or_else(|| " whispers: ".into());
        let shouts = db.and_then(|d| d.by_key(10001, "Shouts")).unwrap_or_else(|| " shouts: ".into());
        self.deliver(gui, m.group, |stamp| format_line(&m, &name, color, stamp, &whispers, &shouts));
    }

    /// A line the client itself generates (errors, command feedback, system, combat feedback).
    /// `group_hint` = a group name (`Other` combat lines name their group, `ChatKind::Group` its channel).
    pub fn push(&mut self, gui: &mut Gui, line: &ChatLine, group_hint: Option<&str>) {
        let by_name = |n: &str| self.groups.iter().find(|(_, g)| g.eq_ignore_ascii_case(n)).map(|(i, _)| *i);
        let (group, color) = match &line.kind {
            ChatKind::Error | ChatKind::System | ChatKind::CmdFeedback => (G_SYSTEM, line.kind.color_name().to_string()),
            ChatKind::TellOut | ChatKind::TellIn => (G_TELL, line.kind.color_name().to_string()),
            ChatKind::Vicinity | ChatKind::Shout | ChatKind::Whisper | ChatKind::Emote => (G_VICINITY, line.kind.color_name().to_string()),
            ChatKind::Group(n) => {
                let id = group_hint.and_then(by_name).or_else(|| by_name(n)).unwrap_or(0);
                (id, group_color(id, 0).to_string())
            }
            ChatKind::Other(c) => (group_hint.and_then(by_name).unwrap_or(G_SYSTEM), c.to_string()),
        };
        self.deliver(gui, group, |stamp| format!("<div indent=wrapped><font color={color}>{stamp}{}</font></div>", line.text));
    }

    /// The event comes from one of the chat windows (the app must not handle it again).
    pub fn owns(&self, ev: &Event) -> bool {
        let w = match ev {
            Event::EnterPressed { window, .. } | Event::Escape { window } | Event::LinkClicked { window, .. } | Event::TextChanged { window, .. } => *window,
            _ => return false,
        };
        self.wins.iter().any(|x| x.id == w)
    }

    fn index_of_input(&self, view: &str) -> Option<usize> {
        self.wins.iter().position(|w| w.input() == view)
    }

    pub fn event(&mut self, gui: &mut Gui, ev: &Event) -> Vec<WinOut> {
        let mut out = vec![];
        match ev {
            Event::EnterPressed { view, .. } => {
                if let Some(i) = self.index_of_input(view) {
                    let w = &self.wins[i];
                    let text = gui.text(w.id, view).trim_end().to_string();
                    let group = (w.cfg.output_group != 0).then(|| group_ident(w.cfg.output_group));
                    let (id, deactivate) = (w.id, w.cfg.deactivate_on_send);
                    gui.set_text(id, view, "");
                    if !text.is_empty() {
                        out.push(WinOut::Submit { text, window_group: group });
                    }
                    if deactivate || self.wins[i].cfg.output_group == 0 {
                        gui.clear_focus();
                    }
                }
            }
            Event::Escape { window } => {
                if self.wins.iter().any(|w| w.id == *window) {
                    gui.clear_focus();
                }
            }
            Event::LinkClicked { window, href, .. } => {
                if let Some(rest) = href.strip_prefix("user://") {
                    out.push(WinOut::OpenTell(rest.to_string()));
                } else if let Some(rest) = href.strip_prefix("chatgroup://") {
                    // `ChatView_c` signal +0x134: the window's output group becomes the clicked group (docs/chat/gui.md §6, guess)
                    if let (Some(g), Some(w)) = (parse_ident(rest), self.wins.iter_mut().find(|w| w.id == *window)) {
                        w.cfg.output_group = g;
                    }
                } else {
                    out.push(WinOut::LinkClicked(href.clone()));
                }
            }
            _ => {}
        }
        out
    }

    /// Activation fades (`FUN_10096b23`): the window whose input bar has the keyboard focus is *active* (alpha
    /// `window_transparency_active`, fade 0.2 s), all others fade to `window_transparency_inactive` over 1 s.
    pub fn update(&mut self, gui: &mut Gui, dt: f32) {
        let focused = gui.focused_view();
        let hide = |w: &Win, active: bool| w.cfg.hide_input_when_inactive && !active;
        for w in &mut self.wins {
            let active = focused.as_deref() == Some(w.input().as_str());
            if active != w.active {
                w.active = active;
                w.target = if active { w.cfg.alpha_active } else { w.cfg.alpha_inactive };
                w.rate = (w.target - w.alpha).abs() / if active { FADE_IN } else { FADE_OUT };
                if active {
                    self.last_active = w.cfg.window_name.clone();
                }
                if w.cfg.textinput {
                    gui.set_visible(w.id, &format!("input_border_{}", w.n), !hide(w, active));
                }
            }
            if w.alpha != w.target {
                let step = w.rate * dt;
                w.alpha = if (w.target - w.alpha).abs() <= step { w.target } else { w.alpha + step * (w.target - w.alpha).signum() };
                gui.set_window_alpha(w.id, w.alpha);
            }
        }
    }

    /// Enter in the game: focus the input bar of the last active window (`ChatLastActiveWindow`, else the first window with one).
    pub fn focus_input(&mut self, gui: &mut Gui) {
        let pick = self.wins.iter().find(|w| w.cfg.window_name == self.last_active && w.cfg.textinput).or_else(|| self.wins.iter().find(|w| w.cfg.textinput));
        if let Some(w) = pick {
            gui.focus(w.id, &w.input());
        }
    }

    fn active_win(&self) -> Option<&Win> {
        self.wins.iter().find(|w| w.active).or_else(|| self.wins.iter().find(|w| w.cfg.window_name == self.last_active))
    }

    /// Focus the input bar like `focus_input` and put `text` into it (`StartChatCmdMessage` opens it with "/").
    pub fn focus_input_text(&mut self, gui: &mut Gui, text: &str) {
        self.focus_input(gui);
        if let Some(w) = self.active_win() {
            let (id, v) = (w.id, w.input());
            gui.set_text(id, &v, text);
        }
    }

    /// `#%016x#` of the output group of the window that has (or last had) the focus: where a plain line without `/` goes.
    pub fn active_output_group(&self) -> Option<String> {
        let w = self.active_win()?;
        (w.cfg.output_group != 0).then(|| group_ident(w.cfg.output_group))
    }

    /// Id form of [`active_output_group`](Self::active_output_group).
    pub fn active_output_id(&self) -> Option<u64> {
        self.active_win().map(|w| w.cfg.output_group).filter(|g| *g != 0)
    }

    /// `/ch <group>` (`FUN_1009a06f`): the active window's output group becomes `id`.
    pub fn set_output_group(&mut self, id: u64) {
        let name = self.active_win().map(|w| w.cfg.window_name.clone());
        if let Some(w) = self.wins.iter_mut().find(|w| Some(&w.cfg.window_name) == name.as_ref()) {
            w.cfg.output_group = id;
        }
    }

    /// Writes every window's `Config.xml` under `<prefs dir>/Chat/Windows/<window_name>/` (`FUN_10094a28`, at shutdown).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(dir) = &self.prefs else { return Ok(()) };
        for w in &self.wins {
            let d = dir.join("Chat/Windows").join(&w.cfg.window_name);
            std::fs::create_dir_all(&d)?;
            std::fs::write(d.join("Config.xml"), w.cfg.to_xml(&|g| self.group_name(g)))?;
        }
        Ok(())
    }

    #[cfg(test)]
    fn window_ids(&self) -> Vec<WindowId> {
        self.wins.iter().map(|w| w.id).collect()
    }
}

#[cfg(test)]
mod tests;
