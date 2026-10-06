//! The NPC chat window (`NPCChatWindow_c` / `NPCChatView_c`, GUI.dll): a tabbed window titled with the NPC's name holding the dialogue text
//! (an HTML `TextView_c`) and, below it, the answer list (a second `TextView_c` of bullet links). Evidence: docs/zone/interact.md §2.

use ao_gui::{CanvasItem, Event, Gui, MouseButton, WindowId, WindowSize};
use ao_net::n3::knubot::text as kind;

/// Client rectangle of the window: `Rect(0, 0, _DAT_101b1d18 = 399.0, _DAT_101b1d1c = 299.0)` [GUI 0x10059e8a], inclusive corners.
pub const CLIENT: (u32, u32) = (400, 300);
/// `text_split_proportion` default (`_DAT_101af644` = 0.7f32, `Message::FindFloat` in `FUN_10058ed8`): the text part of the view's height.
const SPLIT: f32 = 0.7;
/// Range of a dragged proportion (`FUN_10058371`: below `_DAT_101b2058` = 0.1 (f32 widened to a double) the value becomes `_DAT_101b2054` = 0.1f, above
/// `_DAT_101af630` = 0.9 it becomes `_DAT_101b2050` = 0.9f).
const SPLIT_MIN: f32 = 0.1;
const SPLIT_MAX: f32 = 0.9;
/// `View::SetBorders(.., _DAT_101a8b98 = 5.0)` of the two text views.
const BORDER: u32 = 5;
/// The splitter `BitmapView_c(Rect, "", 0x120, 0, 4)` = `GFX_GUI_NPCCHAT_SEPERATOR` (32 x 3 px).
const SPLITTER_GFX: &str = "GFX_GUI_NPCCHAT_SEPERATOR";
/// `View::SetMousePointer(this, 10, ..)` over the splitter (`FUN_10058460`): row 10 of the pointer table = `GFX_GUI_POINTER_VER_DRAG` (0x13f), hotspot (6, 16)
/// (docs/gui.md §13.2: the system pointer rows 7..14 start at `4WAY_DRAG` 0x13c, hotspots (16,16) (12,12) (12,12) (6,16) ..).
pub const POINTER_GFX: &str = "GFX_GUI_POINTER_VER_DRAG";
pub const POINTER_HOTSPOT: (f32, f32) = (6.0, 16.0);

/// `NPCChatView_c::SetText` height: `floor(split * (height + 1) - 1)` (`FUN_10058100`), `height` = `Rect::Height` = pixel height - 1 of the view bounds, so
/// `view_h` is the pixel height of the view.
fn text_height(view_h: u32, split: f32) -> u32 {
    (split * view_h as f32 - 1.0).floor().max(0.0) as u32
}

/// `FUN_10058371`: the proportion of a splitter dragged to `y` (view pixels from the view's top): `y / (Height + 1)`, out-of-range values are replaced by the bound.
pub fn split_from(y: f32, view_h: u32) -> f32 {
    (y / view_h as f32).clamp(SPLIT_MIN, SPLIT_MAX)
}

/// The view XML (both text views are `TextView_c(Rect, "", "", FontID 8 = LARGE, ..)`, `FUN_10058ed8`): `NPCChatView_c` is a vertical layout of the text view (fixed height from [`text_height`]), the splitter bitmap and the answer view.
/// The splitter is a `CanvasView` (it needs the press / drag events; the bitmap itself is painted by [`NpcChat::paint_splitter`]).
fn view_xml(h: u32, split: f32) -> String {
    let th = text_height(h, split);
    let tv = |name: &str, extra: &str| {
        format!(
            "<TextView feature_flags=\"TVF_MULTILINE|TVF_WORD_WRAP|TVF_ACCEPT_MOUSE_INPUT|TVF_ALLOW_TEXT_SELECTION\" font=\"LARGE\" h_scrollbar_mode=\"NONE\" layout_borders=\"Rect({BORDER},{BORDER},{BORDER},{})\" name=\"{name}\" v_scrollbar_mode=\"AUTO\"{extra}/>",
            if name == "npc_text" { 0 } else { BORDER }
        )
    };
    format!(
        "<root><View name=\"npcchat\" view_layout=\"vertical\">{}<CanvasView name=\"npc_split\" min_size=\"Point(0,2)\" max_size=\"Point(16000,2)\"/>{}</View></root>",
        tv("npc_text", &format!(" max_size=\"Point(16000,{th})\" min_size=\"Point(0,{th})\"")),
        tv("npc_answers", " max_size=\"Point(16000,16000)\" min_size=\"Point(0,0)\""),
    )
}

/// State of the `NPCChatView_c` text composer: the type of the last appended text (`+0x188`, `-1` = nothing yet) and whether the text view has content.
#[derive(Debug, Clone, PartialEq)]
pub struct Composer {
    pub last: i32,
    pub html: String,
}

impl Default for Composer {
    fn default() -> Self {
        Composer { last: -1, html: String::new() }
    }
}

impl Composer {
    /// `FUN_100586fd` [GUI 0x100586fd]: the HTML one `KnubotAppendText` adds. `npc` is the name of the NPC (`view+0x14c`), `own` the name of
    /// the client character, `show_questions` the pref `ShowNPCQuestions` (absent from every prefs xml = 0). Returns what was appended.
    pub fn add(&mut self, text: &str, mut ty: i32, npc: &str, own: &str, show_questions: bool) -> String {
        let has = !self.html.is_empty();
        let mut out = String::new();
        let mut skip_text = false;
        match ty {
            kind::SPEECH => {
                out.push_str("<font color=CCNPCChatText>");
                // the speaker label appears whenever the previous text was not NPC speech
                if self.last != kind::SPEECH {
                    if has {
                        out.push_str("<br>");
                    }
                    out.push_str(npc);
                    out.push_str(": ");
                }
            }
            kind::OOC => {
                out.push_str("<font color=CCNPCOOCText>");
                if self.last != kind::OOC && has {
                    out.push_str("<br>");
                }
            }
            kind::QUESTION => {
                out.push_str("<br><font color=CCNPCChatQuestion>");
                if show_questions {
                    out.push_str(own);
                    out.push_str(": ");
                } else {
                    skip_text = true;
                }
            }
            kind::SYSTEM => out.push_str("<br><font color=CCNPCChatSystem>"),
            kind::EMOTE => {
                out.push_str("<br><font color=CCNPCChatEmote>");
                out.push_str(npc);
                out.push(' ');
            }
            kind::DESCRIPTION => out.push_str("<br><font color=CCNPCChatDescription>"),
            kind::PLAIN => {
                if has {
                    out.push_str("<br>");
                }
                out.push_str("<font color=CCNPCChatText>");
                out.push_str(npc);
                out.push_str(": ");
                ty = kind::SPEECH;
            }
            _ => {}
        }
        if !skip_text {
            // a literal backslash + `n` is a line break
            let mut it = text.chars().peekable();
            while let Some(c) = it.next() {
                if c == '\\' && it.peek() == Some(&'n') {
                    it.next();
                    out.push_str("<br>");
                } else {
                    out.push(c);
                }
            }
        }
        out.push_str("</font>");
        self.html.push_str(&out);
        self.last = ty;
        out
    }
}

/// `FUN_10058dc4`: one bullet line per answer, `href` = the answer index.
pub fn answers_html(answers: &[String]) -> String {
    answers
        .iter()
        .enumerate()
        .map(|(i, a)| format!("<div indent=wrapped><img src=tdb://id:GFX_GUI_NPCCHAT_BULLET> <a href={i} style=text-decoration:none><font color=CCNPCChatQuestion>{}</font></a></div>", escape(a)))
        .collect()
}

/// `String::Escape` of the answer text: the characters the HTML parser would take for markup.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// What a click in the window asks the game to do.
#[derive(Debug, PartialEq, Eq)]
pub enum ChatOut {
    /// Answer link `index` of the list (`FUN_10058d1a`): `N3Msg_SendNPCChatAnswer`.
    Answer(i32),
    /// The window was closed by the player (`FUN_10059d7f` destructor sends `N3Msg_NPCChatCloseWindow` unless the server closed it).
    Closed,
    /// Button bar: `FUN_100584f1` -> `N3Msg_NPCChatRequestDescription`.
    Description,
    /// Button bar: `FUN_100582eb` -> `InfoViewModule_c::ShowURL("charid://%u/%u", npc)`.
    Info,
    /// Button bar: `FUN_1005852c` -> `N3Msg_NPCChatStartTrade`, or the end of the running trade (`FUN_10058408`).
    Trade,
    /// Button bar: `LAB_1005834a` -> `N3Msg_UseItem(npc, false)`.
    Use,
}

/// Which buttons of the bar are enabled (`FUN_10058ed8`: `View::Enable(button, ..)`): description = `b20` of `KnubotOpenChatWindow`, trade = `b21`, use = bit 21 of
/// the NPC's stat 0 (`N3Msg_GetSkill(npc, 0, 2)`); the info button is always on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BarFlags {
    pub description: bool,
    pub trade: bool,
    pub use_npc: bool,
}

/// Stat 0, bit 21: enables the fourth button (`FUN_10058ed8`: `(GetSkill(npc, 0, 2) >> 21) & 1`).
pub const STAT0_USE_BIT: i32 = 1 << 21;

/// LDB category 10000 texts of the NPC windows: the tooltips of the four buttons (`FUN_10059704`: keys `RequestNPCdescription`, `RequestNPCinfo`, `GiveItems`, `Shop`).
/// (The tab titles `Tab_Tools` / `Trade` of the bar and trade windows are never drawn: a style-2 window has no frame.)
#[derive(Clone, Debug, Default)]
pub struct ChatTexts {
    pub tips: [String; 4],
}

impl ChatTexts {
    pub fn load(db: &ao_formats::screens::TextDb) -> Self {
        let t = |k: &str| db.by_key(10000, k).unwrap_or_default();
        ChatTexts { tips: [t("RequestNPCdescription"), t("RequestNPCinfo"), t("GiveItems"), t("Shop")] }
    }
}

/// The four buttons of `ButtonBar_c` (`FUN_10059704`): name, `Button_c::SetGfx(0/1/2, ..)` art (state 2 = the hover art = the raised id), `SetBorders`.
const BAR: [(&str, &str, &str, &str); 4] = [
    ("desc", "GFX_GUI_BUTTON_DESC_NORMAL", "GFX_GUI_BUTTON_DESC_PRESSED", "Rect(5,5,0,5)"),
    ("info", "GFX_GUI_BUTTON_INFO_NORMAL", "GFX_GUI_BUTTON_INFO_PRESSED", "Rect(5,5,0,5)"),
    ("trade", "GFX_GUI_BUTTON_GIVE_NORMAL", "GFX_GUI_BUTTON_GIVE_PRESSED", "Rect(5,5,0,5)"),
    ("use", "GFX_GUI_BUTTON_SHOP_NORMAL", "GFX_GUI_BUTTON_SHOP_PRESSED", "Rect(5,5,5,5)"),
];

/// The bar's view XML: an `HLayoutNode` of the four icon buttons on the black 0.85 alpha background of a style-2 window (`WndBorder::SetBorderGfx` 0x1015a82d).
pub fn style2_xml(name: &str, layout: &str, children: &str) -> String {
    format!(
        "<root><BorderView name=\"{name}\" bg_gfx=\"GFX_GUI_WINDOW_BACKGROUND\" alpha=\"0.85\" tl_gfx=\"none\" tr_gfx=\"none\" bl_gfx=\"none\" br_gfx=\"none\" left_gfx=\"none\" top_gfx=\"none\" right_gfx=\"none\" bottom_gfx=\"none\" view_layout=\"{layout}\">{children}</BorderView></root>"
    )
}

fn bar_xml() -> String {
    let btns: String = BAR
        .iter()
        .map(|(n, up, down, b)| format!("<Button gfxid_raised=\"{up}\" gfxid_pressed=\"{down}\" gfxid_hover=\"{up}\" layout_borders=\"{b}\" name=\"{n}\"/>"))
        .collect();
    style2_xml("buttonbar", "horizontal", &btns)
}

/// Dock slot 5 of `Window::LayoutDockedWindows` [GUI 0x101550ed]: left aligned below the window (`x = L, y = B + 1`); slot 2: right of it (`x = R, y = T`), the
/// rectangles being inclusive. `outer` = `(x, y, w, h)` of the chat window's frame.
pub fn dock_below(outer: (i32, i32, u32, u32)) -> (i32, i32) {
    (outer.0, outer.1 + outer.3 as i32)
}

pub fn dock_right(outer: (i32, i32, u32, u32)) -> (i32, i32) {
    (outer.0 + outer.2 as i32 - 1, outer.1)
}

pub struct NpcChat {
    pub win: WindowId,
    pub npc: ao_net::msg::Identity,
    pub name: String,
    pub text: Composer,
    /// `view+0x16c`: the answers of the last `KnubotAnswerList` (emptied by a click).
    pub answers: Vec<String>,
    /// `text_split_proportion` (`view+0x168`).
    pub split: f32,
    /// The splitter drag (`view+0x17c` / `+0x180`): the text height at the press and the pointer travel since.
    drag: Option<(f32, f32)>,
    /// The button bar window (`ButtonBar_c` in its own style-2 window, docked below this one).
    pub bar: WindowId,
    pub flags: BarFlags,
    /// Pref `ShowNPCQuestions` (IndependentPrefs, default 1).
    pub show_questions: bool,
    /// Outer frame the docked windows were last placed for.
    docked: Option<(i32, i32, u32, u32)>,
}

impl NpcChat {
    /// `FUN_10059e8a`: the window is centred (`Window::MoveToCenter`) and shown; `FUN_10058577`: the button bar window is created and docked below it.
    pub fn open(gui: &mut Gui, screen: (u32, u32), npc: ao_net::msg::Identity, name: &str, flags: BarFlags, texts: &ChatTexts) -> anyhow::Result<Self> {
        let (w, h) = CLIENT;
        let pos = ((screen.0 as i32 - w as i32) / 2, (screen.1 as i32 - h as i32) / 2);
        let win = gui.open_tabbed_window_xml("NPCChatWindow", name, &view_xml(h, SPLIT), pos, WindowSize::Fixed(w, h))?;
        let bar = gui.open_window_xml("NPCChatButtonBar", &bar_xml(), pos, WindowSize::Preferred)?;
        for (i, (n, ..)) in BAR.iter().enumerate() {
            gui.set_tooltip(bar, n, &texts.tips[i], "");
        }
        let mut c = NpcChat { win, npc, name: name.to_owned(), text: Composer::default(), answers: vec![], split: SPLIT, drag: None, bar, flags, show_questions: true, docked: None };
        c.set_flags(gui, flags);
        c.paint_splitter(gui);
        c.sync_docks(gui, None);
        Ok(c)
    }

    /// `View::Enable` of the bar's buttons.
    pub fn set_flags(&mut self, gui: &mut Gui, f: BarFlags) {
        self.flags = f;
        for (n, on) in [("desc", f.description), ("info", true), ("trade", f.trade), ("use", f.use_npc)] {
            gui.set_enabled(self.bar, n, on);
        }
    }

    /// Height of the view in pixels (`Rect::Height() + 1` of `NPCChatView_c`'s bounds).
    fn view_h(&self, gui: &Gui) -> u32 {
        gui.view_rect(self.win, "npcchat").map_or(CLIENT.1, |r| (r.height() + 1.0) as u32)
    }

    /// The splitter bitmap: `GFX_GUI_NPCCHAT_SEPERATOR` over the view's width ([INFERENCE]: `BitmapView_c` drawing was not read, the 16000 px max width suggests it
    /// is stretched).
    fn paint_splitter(&self, gui: &mut Gui) {
        let (w, _) = gui.canvas_size(self.win, "npc_split");
        if let Some(id) = gui.gfx().id(SPLITTER_GFX) {
            let (iw, ih) = gui.gfx().size(id);
            gui.set_canvas(self.win, "npc_split", vec![CanvasItem::Image { id, src: [0.0, 0.0, iw as f32, ih as f32], dst: [0.0, 0.0, w as f32, ih as f32], alpha: 1.0 }]);
        }
    }

    /// `FUN_1005824d`: min / max preferred size of the text part from the proportion.
    fn apply_split(&mut self, gui: &mut Gui) {
        let th = text_height(self.view_h(gui), self.split) as f32;
        gui.set_text_pref_size(self.win, "npc_text", (0.0, th), (16000.0, th));
        self.paint_splitter(gui);
    }

    /// Current height of the text part.
    pub fn text_part(&self, gui: &Gui) -> u32 {
        text_height(self.view_h(gui), self.split)
    }

    /// `FUN_10058186` (mouse down on the splitter, `FUN_10058100` = the text height): the drag starts; `FUN_10058460` (mouse move while dragging): the pointer's
    /// travel moves the text part, `FUN_10058371` stores the clamped proportion and re-lays the view out. `dy` = pointer movement since the last step.
    pub fn drag_start(&mut self, gui: &Gui) {
        self.drag = Some((self.text_part(gui) as f32, 0.0));
    }

    pub fn drag_by(&mut self, gui: &mut Gui, dy: f32) {
        let Some((th, acc)) = self.drag.as_mut() else { return };
        *acc += dy;
        let y = *th + *acc;
        self.split = split_from(y, self.view_h(gui));
        self.apply_split(gui);
    }

    /// `FUN_10058230` (mouse up): the drag ends.
    pub fn drag_end(&mut self) {
        self.drag = None;
    }

    #[cfg(test)]
    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// `FUN_10058460`: the mouse pointer is [`POINTER_GFX`] over the splitter's frame (and while dragging).
    pub fn pointer_over(&self, gui: &Gui, x: f32, y: f32) -> bool {
        self.drag.is_some() || gui.view_rect(self.win, "npc_split").is_some_and(|r| x >= r.l && x <= r.r + 1.0 && y >= r.t && y <= r.b + 1.0)
    }

    /// Places the docked windows (`Window::LayoutDockedWindows`): the button bar below, the trade window (if any) to the right; only when the chat window moved.
    pub fn sync_docks(&mut self, gui: &mut Gui, trade: Option<WindowId>) {
        let Some(f) = gui.window_outer_frame(self.win) else { return };
        if self.docked == Some(f) && trade.is_none() {
            return;
        }
        self.docked = Some(f);
        let (bx, by) = dock_below(f);
        gui.set_window_pos(self.bar, (bx, by));
        if let Some(t) = trade {
            let (tx, ty) = dock_right(f);
            gui.set_window_pos(t, (tx, ty));
        }
    }

    /// `KnubotAppendText` -> `FUN_100586fd`.
    pub fn append(&mut self, gui: &mut Gui, text: &str, ty: i32, own: &str) {
        self.text.add(text, ty, &self.name, own, self.show_questions);
        gui.set_text(self.win, "npc_text", &self.text.html);
        gui.scroll_to_bottom(self.win, "npc_text");
    }

    /// `KnubotAnswerList` -> `FUN_10058dc4`: the answer view is cleared and refilled.
    pub fn set_answers(&mut self, gui: &mut Gui, answers: Vec<String>) {
        gui.set_text(self.win, "npc_answers", &answers_html(&answers));
        self.answers = answers;
    }

    /// `FUN_10058a4c`: the trade text goes into the answer view (`<font color=CCNPCChatTrade>text</font>`).
    pub fn set_trade_text(&mut self, gui: &mut Gui, text: &str) {
        gui.set_text(self.win, "npc_answers", &format!("<font color=CCNPCChatTrade>{text}</font>"));
    }

    /// `LAB_10058362` (the trade ended: `KnubotRejectedItems`) and `FUN_10058408` (the trade button / decline): the last text type becomes 3 (system).
    pub fn trade_ended(&mut self) {
        self.text.last = kind::SYSTEM;
    }

    /// `FUN_10058d1a`: a link of the answer view. The chosen answer is echoed as a question line (only with `ShowNPCQuestions`), the
    /// answer is sent, the answer view is cleared.
    pub fn link(&mut self, gui: &mut Gui, href: &str, own: &str) -> Option<ChatOut> {
        let idx = href.trim().parse::<u32>().ok().filter(|&i| (i as usize) < self.answers.len())?;
        let a = self.answers[idx as usize].clone();
        self.append(gui, &a, kind::QUESTION, own);
        gui.set_text(self.win, "npc_answers", "");
        self.answers.clear();
        Some(ChatOut::Answer(idx as i32))
    }

    /// A click on bar button `i` (0 description, 1 info, 2 trade, 3 use) when it is enabled.
    pub fn press(&self, i: usize) -> Option<ChatOut> {
        let f = self.flags;
        match i {
            0 if f.description => Some(ChatOut::Description),
            1 => Some(ChatOut::Info),
            2 if f.trade => Some(ChatOut::Trade),
            3 if f.use_npc => Some(ChatOut::Use),
            _ => None,
        }
    }

    pub fn event(&mut self, gui: &mut Gui, ev: &Event, own: &str) -> Option<Option<ChatOut>> {
        match ev {
            Event::LinkClicked { window, view, href } if *window == self.win && view == "npc_answers" => Some(self.link(gui, href, own)),
            Event::CloseRequested { window } if *window == self.win => Some(Some(ChatOut::Closed)),
            Event::Clicked { window, view, .. } if *window == self.bar => Some(BAR.iter().position(|b| b.0 == view).and_then(|i| self.press(i))),
            Event::CanvasPress { window, view, button: MouseButton::Left, .. } if *window == self.win && view == "npc_split" => {
                self.drag_start(gui);
                Some(None)
            }
            Event::CanvasDrag { window, view, dy, .. } if *window == self.win && view == "npc_split" => {
                self.drag_by(gui, *dy);
                Some(None)
            }
            Event::CanvasRelease { window, view } if *window == self.win && view == "npc_split" => {
                self.drag_end();
                Some(None)
            }
            _ => None,
        }
    }

    pub fn close(self, gui: &mut Gui) {
        gui.close_window(self.bar);
        gui.close_window(self.win);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_gui::InputEvent;

    #[test]
    fn text_height_is_the_floor_of_the_split() {
        assert_eq!(text_height(300, SPLIT), 209);
        assert_eq!(text_height(0, SPLIT), 0);
        assert_eq!(text_height(300, 0.5), 149);
    }

    /// `FUN_10058371`: `y / (Height + 1)`, clamped to [0.1, 0.9].
    #[test]
    fn splitter_proportion_is_clamped() {
        assert_eq!(split_from(150.0, 300), 0.5);
        assert_eq!(split_from(-40.0, 300), 0.1);
        assert_eq!(split_from(5.0, 300), 0.1);
        assert_eq!(split_from(299.0, 300), 0.9);
        assert_eq!(split_from(270.0, 300), 0.9);
        assert!((split_from(210.0, 300) - 0.7).abs() < 1e-6);
    }

    /// `Window::LayoutDockedWindows` slot 5 (below, left aligned) and slot 2 (right, top aligned) with inclusive corners.
    #[test]
    fn dock_slots_follow_the_chat_window() {
        assert_eq!(dock_below((100, 50, 410, 320)), (100, 370));
        assert_eq!(dock_right((100, 50, 410, 320)), (509, 50));
    }

    #[test]
    fn composer_follows_fun_100586fd() {
        let mut c = Composer::default();
        // first speech: speaker label, no <br> on an empty view
        assert_eq!(c.add("Hello\\nthere", kind::SPEECH, "Guard", "Me", false), "<font color=CCNPCChatText>Guard: Hello<br>there</font>");
        // same speaker again: no label, no break
        assert_eq!(c.add("More", kind::SPEECH, "Guard", "Me", false), "<font color=CCNPCChatText>More</font>");
        // question echo only with the pref
        assert_eq!(c.add("Yes", kind::QUESTION, "Guard", "Me", false), "<br><font color=CCNPCChatQuestion></font>");
        assert_eq!(c.add("Yes", kind::QUESTION, "Guard", "Me", true), "<br><font color=CCNPCChatQuestion>Me: Yes</font>");
        // speech after a question: the label comes back with a break
        assert_eq!(c.add("Ok", kind::SPEECH, "Guard", "Me", false), "<font color=CCNPCChatText><br>Guard: Ok</font>");
        assert_eq!(c.add("waves", kind::EMOTE, "Guard", "Me", false), "<br><font color=CCNPCChatEmote>Guard waves</font>");
        assert_eq!(c.last, kind::EMOTE);
        assert_eq!(c.add("x", kind::SYSTEM, "Guard", "Me", false), "<br><font color=CCNPCChatSystem>x</font>");
        // type 6 is speech with the label whatever came before, and counts as speech afterwards
        assert_eq!(c.add("y", kind::PLAIN, "Guard", "Me", false), "<br><font color=CCNPCChatText>Guard: y</font>");
        assert_eq!(c.last, kind::SPEECH);
    }

    #[test]
    fn answers_are_bullet_links_by_index() {
        let h = answers_html(&["A <b>".into(), "B".into()]);
        assert!(h.contains("<a href=0 style=text-decoration:none><font color=CCNPCChatQuestion>A &lt;b&gt;</font></a>"));
        assert!(h.contains("<a href=1 "));
    }

    fn rig() -> Option<Gui> {
        let client = ao_gui::client_dir();
        client.join("cd_image/gui").exists().then(|| Gui::new(&client, None).unwrap())
    }

    #[test]
    fn window_shows_text_and_a_click_on_an_answer_is_sent() {
        let Some(mut gui) = rig() else { return };
        let npc = ao_net::msg::Identity { kind: 0xC350, instance: 9 };
        let mut w = NpcChat::open(&mut gui, (1280, 800), npc, "Guard", BarFlags::default(), &ChatTexts::default()).unwrap();
        w.append(&mut gui, "Welcome\\nstranger", kind::SPEECH, "Me");
        w.set_answers(&mut gui, vec!["Where am I?".into(), "Bye".into()]);
        assert!(gui.text(w.win, "npc_text").contains("Welcome"));
        assert_eq!(w.link(&mut gui, "1", "Me"), Some(ChatOut::Answer(1)));
        assert!(w.answers.is_empty());
        assert_eq!(w.link(&mut gui, "1", "Me"), None, "the list is gone after an answer");
        gui.frame(0.0);
    }

    fn open_chat(gui: &mut Gui, flags: BarFlags) -> NpcChat {
        let npc = ao_net::msg::Identity { kind: 0xC350, instance: 9 };
        let texts = ChatTexts { tips: ["Description".into(), "Info".into(), "Give items".into(), "Shop".into()] };
        NpcChat::open(gui, (1280, 800), npc, "Guard", flags, &texts).unwrap()
    }

    #[test]
    fn bar_buttons_use_the_client_art_and_enable_flags() {
        let Some(mut gui) = rig() else { return };
        // the numeric ids of `FUN_10059704` are these gfx names
        for (id, name) in [(0x3f, "DESC_NORMAL"), (0x40, "DESC_PRESSED"), (0x44, "INFO_NORMAL"), (0x45, "INFO_PRESSED"), (0x42, "GIVE_NORMAL"), (0x43, "GIVE_PRESSED"), (0x49, "SHOP_NORMAL"), (0x4a, "SHOP_PRESSED")] {
            assert_eq!(gui.gfx().id(&format!("GFX_GUI_BUTTON_{name}")), Some(ao_gui::GfxId(id)), "{name}");
        }
        assert_eq!(gui.gfx().id(SPLITTER_GFX), Some(ao_gui::GfxId(0x120)));
        assert_eq!(gui.gfx().id(POINTER_GFX), Some(ao_gui::GfxId(0x13f)));
        let mut w = open_chat(&mut gui, BarFlags { description: true, trade: false, use_npc: false });
        assert!(gui.is_enabled(w.bar, "desc") && gui.is_enabled(w.bar, "info"));
        assert!(!gui.is_enabled(w.bar, "trade") && !gui.is_enabled(w.bar, "use"));
        assert_eq!((w.press(0), w.press(1), w.press(2), w.press(3)), (Some(ChatOut::Description), Some(ChatOut::Info), None, None));
        w.set_flags(&mut gui, BarFlags { description: false, trade: true, use_npc: true });
        assert_eq!((w.press(0), w.press(2), w.press(3)), (None, Some(ChatOut::Trade), Some(ChatOut::Use)));
        // the bar sits below the chat window, 4 buttons of 37 x 31 plus the 5 px borders
        let chat = gui.window_outer_frame(w.win).unwrap();
        let bar = gui.window_outer_frame(w.bar).unwrap();
        assert_eq!((bar.0, bar.1), dock_below(chat));
        assert!(bar.2 >= 4 * 37 && bar.3 >= 31, "{bar:?}");
        gui.frame(0.0);
    }

    #[test]
    fn a_click_on_a_bar_button_is_reported() {
        let Some(mut gui) = rig() else { return };
        let mut w = open_chat(&mut gui, BarFlags { description: true, trade: true, use_npc: true });
        gui.frame(0.0);
        for (name, want) in [("desc", ChatOut::Description), ("info", ChatOut::Info), ("trade", ChatOut::Trade), ("use", ChatOut::Use)] {
            let r = gui.view_rect(w.bar, name).unwrap();
            let (x, y) = (r.l + r.width() / 2.0, r.t + r.height() / 2.0);
            gui.input(InputEvent::MouseMove { x, y });
            gui.input(InputEvent::MouseDown { x, y, button: MouseButton::Left });
            let evs = gui.input(InputEvent::MouseUp { x, y, button: MouseButton::Left });
            let outs: Vec<_> = evs.iter().filter_map(|e| w.event(&mut gui, e, "Me")).collect();
            assert_eq!(outs, vec![Some(want)], "{name}");
        }
        // a disabled button reports nothing
        w.set_flags(&mut gui, BarFlags::default());
        let r = gui.view_rect(w.bar, "trade").unwrap();
        let (x, y) = (r.l + 3.0, r.t + 3.0);
        gui.input(InputEvent::MouseDown { x, y, button: MouseButton::Left });
        let evs = gui.input(InputEvent::MouseUp { x, y, button: MouseButton::Left });
        assert!(evs.iter().all(|e| !matches!(e, Event::Clicked { .. })), "{evs:?}");
    }

    #[test]
    fn dragging_the_splitter_resizes_the_text_part_within_the_clamp() {
        let Some(mut gui) = rig() else { return };
        let mut w = open_chat(&mut gui, BarFlags::default());
        gui.frame(0.0);
        let th0 = w.text_part(&gui);
        assert_eq!(th0, 209);
        let h0 = gui.view_rect(w.win, "npc_text").unwrap().height();
        let sp = gui.view_rect(w.win, "npc_split").unwrap();
        let (x, y) = (sp.l + 50.0, sp.t + 1.0);
        // real pointer events: press on the splitter, drag 40 px up
        gui.input(InputEvent::MouseMove { x, y });
        let mut evs = gui.input(InputEvent::MouseDown { x, y, button: MouseButton::Left });
        assert!(w.pointer_over(&gui, x, y));
        evs.extend(gui.input(InputEvent::MouseMove { x, y: y - 40.0 }));
        for e in &evs {
            w.event(&mut gui, e, "Me");
        }
        assert!(w.dragging());
        // y = 209 - 40 = 169 px -> split 169 / 300, the text part floor(split * 300 - 1) = 168 (the original's `- 1` shifts the splitter by a pixel)
        assert_eq!(w.text_part(&gui), 168);
        assert!((w.split - 169.0 / 300.0).abs() < 1e-5, "{}", w.split);
        gui.frame(0.0);
        let h1 = gui.view_rect(w.win, "npc_text").unwrap().height();
        assert!(h1 < h0 - 30.0, "text view shrank: {h0} -> {h1}");
        let evs = gui.input(InputEvent::MouseUp { x, y: y - 40.0, button: MouseButton::Left });
        for e in &evs {
            w.event(&mut gui, e, "Me");
        }
        assert!(!w.dragging());
        // far beyond the top: clamped to 0.1
        w.drag_start(&gui);
        w.drag_by(&mut gui, -500.0);
        assert_eq!(w.split, 0.1);
        w.drag_by(&mut gui, 1000.0);
        assert_eq!(w.split, 0.9);
        w.drag_end();
    }

    #[test]
    fn question_echo_follows_the_pref() {
        let Some(mut gui) = rig() else { return };
        let mut w = open_chat(&mut gui, BarFlags::default());
        w.set_answers(&mut gui, vec!["Yes".into()]);
        assert_eq!(w.link(&mut gui, "0", "Me"), Some(ChatOut::Answer(0)));
        assert!(w.text.html.contains("Me: Yes"), "default pref 1 echoes the question: {}", w.text.html);
        w.show_questions = false;
        w.set_answers(&mut gui, vec!["No".into()]);
        w.link(&mut gui, "0", "Me");
        assert!(!w.text.html.contains("Me: No"));
    }

    #[test]
    fn trade_text_goes_into_the_answer_view() {
        let Some(mut gui) = rig() else { return };
        let mut w = open_chat(&mut gui, BarFlags::default());
        w.set_trade_text(&mut gui, "Buy my wares");
        assert_eq!(gui.text(w.win, "npc_answers"), "<font color=CCNPCChatTrade>Buy my wares</font>");
        w.trade_ended();
        assert_eq!(w.text.last, kind::SYSTEM);
    }
}
