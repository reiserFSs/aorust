//! The NPC chat window (`NPCChatWindow_c` / `NPCChatView_c`, GUI.dll): a tabbed window titled with the NPC's name holding the dialogue text
//! (an HTML `TextView_c`) and, below it, the answer list (a second `TextView_c` of bullet links). Evidence: docs/zone/interact.md §2.

use ao_gui::{Event, Gui, WindowId, WindowSize};
use ao_net::n3::knubot::text as kind;

/// Client rectangle of the window: `Rect(0, 0, _DAT_101b1d18 = 399.0, _DAT_101b1d1c = 299.0)` [GUI 0x10059e8a], inclusive corners.
pub const CLIENT: (u32, u32) = (400, 300);
/// `text_split_proportion` default (`_DAT_101af644` = 0.7f32, `Message::FindFloat` in `FUN_10058ed8`): the text part of the view's height.
const SPLIT: f32 = 0.7;
/// `View::SetBorders(.., _DAT_101a8b98 = 5.0)` of the two text views.
const BORDER: u32 = 5;

/// `NPCChatView_c::SetText` height: `floor(split * (height + 1) - 1)` (`FUN_10058100`).
fn text_height(view_h: u32) -> u32 {
    (SPLIT * (view_h as f32 + 1.0) - 1.0).floor().max(0.0) as u32
}

/// The view XML: `NPCChatView_c` is a vertical layout of the text view (fixed height from [`text_height`]), the splitter bitmap and the answer view.
fn view_xml(h: u32) -> String {
    let th = text_height(h);
    let tv = |name: &str, extra: &str| {
        format!(
            "<TextView feature_flags=\"TVF_MULTILINE|TVF_WORD_WRAP|TVF_ACCEPT_MOUSE_INPUT|TVF_ALLOW_TEXT_SELECTION\" font=\"NORMAL\" h_scrollbar_mode=\"NONE\" layout_borders=\"Rect({BORDER},{BORDER},{BORDER},{})\" name=\"{name}\" v_scrollbar_mode=\"AUTO\"{extra}/>",
            if name == "npc_text" { 0 } else { BORDER }
        )
    };
    format!(
        "<root><View name=\"npcchat\" view_layout=\"vertical\">{}{}</View></root>",
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
}

pub struct NpcChat {
    pub win: WindowId,
    pub npc: ao_net::msg::Identity,
    pub name: String,
    pub text: Composer,
    /// `view+0x16c`: the answers of the last `KnubotAnswerList` (emptied by a click).
    pub answers: Vec<String>,
    /// Enable flags of the button bar's description / trade buttons (`KnubotOpenChatWindow` body).
    pub b20: bool,
    pub b21: bool,
}

impl NpcChat {
    /// `FUN_10059e8a`: the window is centred (`Window::MoveToCenter`) and shown.
    pub fn open(gui: &mut Gui, screen: (u32, u32), npc: ao_net::msg::Identity, name: &str, b20: bool, b21: bool) -> anyhow::Result<Self> {
        let (w, h) = CLIENT;
        let pos = ((screen.0 as i32 - w as i32) / 2, (screen.1 as i32 - h as i32) / 2);
        let win = gui.open_tabbed_window_xml("NPCChatWindow", name, &view_xml(h), pos, WindowSize::Fixed(w, h))?;
        Ok(NpcChat { win, npc, name: name.to_owned(), text: Composer::default(), answers: vec![], b20, b21 })
    }

    /// `KnubotAppendText` -> `FUN_100586fd`.
    pub fn append(&mut self, gui: &mut Gui, text: &str, ty: i32, own: &str) {
        self.text.add(text, ty, &self.name, own, false);
        gui.set_text(self.win, "npc_text", &self.text.html);
        gui.scroll_to_bottom(self.win, "npc_text");
    }

    /// `KnubotAnswerList` -> `FUN_10058dc4`: the answer view is cleared and refilled.
    pub fn set_answers(&mut self, gui: &mut Gui, answers: Vec<String>) {
        gui.set_text(self.win, "npc_answers", &answers_html(&answers));
        self.answers = answers;
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

    pub fn event(&mut self, gui: &mut Gui, ev: &Event, own: &str) -> Option<Option<ChatOut>> {
        match ev {
            Event::LinkClicked { window, view, href } if *window == self.win && view == "npc_answers" => Some(self.link(gui, href, own)),
            Event::CloseRequested { window } if *window == self.win => Some(Some(ChatOut::Closed)),
            _ => None,
        }
    }

    pub fn close(self, gui: &mut Gui) {
        gui.close_window(self.win);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_height_is_the_floor_of_the_split() {
        assert_eq!(text_height(300), 209);
        assert_eq!(text_height(0), 0);
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
        let mut w = NpcChat::open(&mut gui, (1280, 800), npc, "Guard", true, false).unwrap();
        w.append(&mut gui, "Welcome\\nstranger", kind::SPEECH, "Me");
        w.set_answers(&mut gui, vec!["Where am I?".into(), "Bye".into()]);
        assert!(gui.text(w.win, "npc_text").contains("Welcome"));
        assert_eq!(w.link(&mut gui, "1", "Me"), Some(ChatOut::Answer(1)));
        assert!(w.answers.is_empty());
        assert_eq!(w.link(&mut gui, "1", "Me"), None, "the list is gone after an answer");
        gui.frame(0.0);
    }
}
