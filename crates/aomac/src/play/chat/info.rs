//! The InfoView window (`InfoViewModule_c` / `InfoView_c`, GUI.dll): HTML help pages with back/forward history.
//! Evidence: docs/chat/dialogs.md §1. `/help`, `/showfile`, `/tipoftheday` and the `text://` links of the chat windows end here
//! (`InfoViewModule_c::ShowURL` 0x100ef153).

use ao_gui::{Event, Gui, WindowId, WindowSize};
use std::path::PathBuf;

#[path = "character_info.rs"]
mod character;
#[path = "info_fields.rs"]
mod fields;
#[path = "info_template_spells.rs"]
mod template_spells;
#[path = "info_tower_interpolation.rs"]
mod tower_interpolation;
#[path = "quest_info.rs"]
mod quest;
#[path = "item_info.rs"]
mod item;
#[path = "item_effects.rs"]
mod item_effects;
#[path = "item_info_combat.rs"]
mod item_info_combat;
#[path = "item_requirements.rs"]
mod item_requirements;
#[path = "item_info_building.rs"]
mod item_info_building;
#[path = "skill_info.rs"]
mod skill;

/// Arguments of the shared retail item/skill information dispatcher (GUI 100ee05c).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemRequest {
    Reference(ao_net::n3::world::AcgItem),
    Identity { id: ao_net::msg::Identity, shop: bool, container: ao_net::msg::Identity },
    Skill(u32),
}

pub fn item_html(zone: &crate::play::zone::Zone, request: &ItemRequest, texts: &ao_formats::screens::TextDb) -> anyhow::Result<Option<String>> {
    match request {
        ItemRequest::Skill(stat) => skill::html(zone, *stat, texts).map(Some),
        _ => item::html(zone, request, texts),
    }
}

pub fn shop_item_html(zone: &crate::play::zone::Zone, item: ao_net::n3::world::AcgItem, price: i32, texts: &ao_formats::screens::TextDb) -> anyhow::Result<Option<String>> {
    item::html_for_reference(zone, item, Some(price), texts)
}

pub(crate) fn item_template_spells(record: &[u8], list: u32) -> anyhow::Result<Vec<ao_net::n3::spells::Spell>> {
    template_spells::spells(record, list)
}


pub fn character_html(zone: &crate::play::zone::Zone, id: ao_net::msg::Identity, packet: &ao_net::n3::info::InfoPacket, texts: &ao_formats::screens::TextDb) -> anyhow::Result<String> {
    character::html(zone, id, packet, texts)
}

pub fn quest_html(zone: &crate::play::zone::Zone, id: ao_net::msg::Identity, texts: &ao_formats::screens::TextDb) -> anyhow::Result<Option<String>> {
    let store = ao_rdb::RecordStore::open(&ao_gui::client_dir())?;
    quest::html(zone,id,texts,Some(&store))
}

/// `FUN_100eeedb`: `Point(_DAT_101b1840 = 400, _DAT_101b172c = 500)`, used as the client size (**GUESS**: the decompile only shows the
/// constants returned; the window's creation rectangle is `Rect()` and the config archive `InfoViewConfig` is empty on a fresh install).
const CLIENT: (u32, u32) = (400, 500);

/// What showing a URL asks of the hub.
#[derive(Debug, PartialEq, Eq)]
pub enum InfoOut {
    /// `chatcmd://<text>`: `GlobalSignals+0x180(text)` = run as if typed (GUI 0x100a3f43, see `Chat::run_line`).
    Command(String),
    /// "Infoview failed to load and show file: <%s>" on `GlobalSignals+0x17c` with colour code 0xc (`CCRed`).
    Error(String),
    /// `FUN_10031011`: request the server's character information packet.
    Character(ao_net::msg::Identity),
    /// Quest item information (`FUN_1003565d`), resolved synchronously from the zone's quest registry.
    Quest(ao_net::msg::Identity),
    /// Synchronous item/skill page; the hub supplies the zone registry.
    Item { url: String, request: ItemRequest },
    /// Requested external-link extension; retail unknown schemes are no-ops (GUI 100ee05c).
    ExternalUrl(String),
}

/// One visited page (`std::list` node: url at +8, html at +0x24, scroll location at +0x40).
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    url: String,
    text: String,
    scroll: f32,
}

/// The static history list (`DAT_1027660c`, current iterator `DAT_10276618`). It outlives the window: reopening shows the last page.
#[derive(Debug, Default)]
struct History {
    e: Vec<Entry>,
    cur: usize,
}

impl History {
    /// `FUN_100eda84` tail: the same URL as the current page only replaces its text; otherwise the forward part is dropped and the page appended.
    /// `scroll_now` is the current scroll position of the page being left.
    fn visit(&mut self, url: &str, text: String, scroll_now: f32) {
        if self.e.get(self.cur).is_some_and(|c| c.url == url) {
            self.e[self.cur].text = text;
            return;
        }
        if let Some(c) = self.e.get_mut(self.cur) {
            c.scroll = scroll_now;
            self.e.truncate(self.cur + 1);
        }
        self.e.push(Entry { url: url.to_owned(), text, scroll: 0.0 });
        self.cur = self.e.len() - 1;
    }
    /// `FUN_100ed627` (Back button).
    fn back(&mut self, scroll_now: f32) -> bool {
        if self.e.is_empty() || self.cur == 0 {
            return false;
        }
        self.e[self.cur].scroll = scroll_now;
        self.cur -= 1;
        true
    }
    /// `FUN_100ed757` (Forward button).
    fn forward(&mut self, scroll_now: f32) -> bool {
        if self.e.is_empty() || self.cur + 1 >= self.e.len() {
            return false;
        }
        self.e[self.cur].scroll = scroll_now;
        self.cur += 1;
        true
    }
    fn can_back(&self) -> bool {
        self.cur > 0 && !self.e.is_empty()
    }
    fn can_forward(&self) -> bool {
        self.cur + 1 < self.e.len()
    }
}

pub struct InfoView {
    /// `<client>/cd_image/text` (`AnarchyPath_t::GetCDPath` + `text/`).
    text_dir: PathBuf,
    win: Option<WindowId>,
    hist: History,
    /// `InfoView_c+0x1a0`: the URL of the shown page (also kept for `text://` / `charref://`).
    url: String,
}

/// Latin-1 text (the client's text files are 8 bit; `TextDb` reads them the same way).
fn latin1(b: &[u8]) -> String {
    b.iter().map(|&c| c as char).collect()
}

/// `text://%u` of `FUN_100ee05c`: the byte sum (signed `char`s, starting at offset 7) of the whole URL.
fn text_url_key(url: &str) -> String {
    let sum = url.chars().skip(7).fold(0i32, |a, c| a.wrapping_add(c as u32 as u8 as i8 as i32));
    format!("text://{}", sum as u32)
}

/// `?section=N` of the URL (`FUN_100eda84`: `key = value` pairs after `?`, separated by `&`, `atol` of the value).
fn section_param(url: &str) -> Option<i32> {
    let q = &url[url.find('?')? + 1..];
    let mut found = None;
    for pair in q.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k.trim().eq_ignore_ascii_case("section") {
                let v = v.trim();
                let digits: String = v.chars().enumerate().take_while(|(i, c)| c.is_ascii_digit() || (*i == 0 && (*c == '-' || *c == '+'))).map(|(_, c)| c).collect();
                found = Some(digits.parse().unwrap_or(0));
            }
        }
    }
    found
}

/// The text between the end of the `n`-th (0-based) `<section>` tag and the next `</section>`; empty when it is not closed
/// (`FUN_100eda84`: `HTMLParser_c` tag tokens, `section` / `/section`).
fn extract_section(html: &str, n: i32) -> String {
    let mut left = n;
    let mut start = None;
    let mut i = 0;
    while let Some(p) = html[i..].find('<') {
        let at = i + p;
        let Some(len) = html[at..].find('>') else { break };
        let name = html[at + 1..at + len].split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        match start {
            None if name == "section" => {
                let hit = left == 0;
                left -= 1;
                if hit {
                    start = Some(at + len + 1);
                }
            }
            Some(s) if name == "/section" => return html[s..at].to_owned(),
            _ => {}
        }
        i = at + len + 1;
    }
    String::new()
}

/// Character identities followed by optional embedded HTML (`charref://`).
fn character_url(s: &str) -> Option<(ao_net::msg::Identity, &str)> {
    let mut parts = s.splitn(3, '/');
    let kind = parts.next()?.parse().ok()?;
    let instance = parts.next()?.parse().ok()?;
    Some((ao_net::msg::Identity { kind, instance }, parts.next().unwrap_or("")))
}

fn item_request(url: &str) -> Option<ItemRequest> {
    let identity = |s: &str, shop| {
        let mut p = s.split('/');
        let id = ao_net::msg::Identity { kind: p.next()?.parse().ok()?, instance: p.next()?.parse().ok()? };
        let container = if let Some(kind) = p.next() {
            if shop { return None; }
            let container = ao_net::msg::Identity { kind: kind.parse().ok()?, instance: p.next()?.parse().ok()? };
            if p.next().is_some() { return None; }
            container
        } else { ao_net::msg::Identity::default() };
        Some(ItemRequest::Identity { id, shop, container })
    };
    if let Some(s) = url.strip_prefix("itemref://") {
        let mut p = s.split('/');
        let item = ao_net::n3::world::AcgItem { low_id: p.next()?.parse().ok()?, high_id: p.next()?.parse().ok()?, level: p.next()?.parse().ok()? };
        if p.next().is_some() { return None; }
        Some(ItemRequest::Reference(item))
    } else if let Some(s) = url.strip_prefix("shopitemid://") {
        identity(s, true)
    } else if let Some(s) = url.strip_prefix("itemid://") {
        identity(s, false)
    } else {
        url.strip_prefix("skillid://")?.parse().ok().map(ItemRequest::Skill)
    }
}

impl InfoView {
    pub fn new(client: &std::path::Path) -> Self {
        Self { text_dir: client.join("cd_image/text"), win: None, hist: History::default(), url: String::new() }
    }

    pub fn window(&self) -> Option<WindowId> {
        self.win
    }
    /// `file://<CDPath>text\TipOfTheDay.html?section=<n>` (`FUN_100b6f0d`).
    pub fn tip_url(&self, n: i32) -> String {
        format!("file://{}?section={n}", self.text_dir.join("TipOfTheDay.html").display())
    }

    /// `info_window` DValue true (`/open InfoView`): the window with the last visited page, if any.
    pub fn open(&mut self, gui: &mut Gui, screen: (u32, u32)) {
        if self.win.is_none() {
            self.ensure_window(gui, screen);
            self.show_current(gui);
        }
    }

    /// `FUN_100ee05c` file:// branch: the file as given, then `<CDPath>text/help/<name>` (the `<scripts>/text/` step of the original is not ported:
    /// the port has no scripts directory).
    fn load_file(&self, name: &str) -> Option<String> {
        std::fs::read(name).or_else(|_| std::fs::read(self.text_dir.join("help").join(name))).ok().map(|b| latin1(&b))
    }

    /// `InfoViewModule_c::ShowURL`: opens the window (`info_window` DValue) and shows `url`. `toggle` is the flag the ShowURL signal passes
    /// (link clicks pass false): the same URL again closes the window.
    pub fn show_url(&mut self, gui: &mut Gui, screen: (u32, u32), url: &str, toggle: bool) -> Vec<InfoOut> {
        let was_open = self.win.is_some();
        // `FUN_100384f3` generates item pages, not character pages.
        let lower = url.to_ascii_lowercase();
        if lower.starts_with("http://") || lower.starts_with("https://") {
            return vec![InfoOut::ExternalUrl(url.to_owned())];
        }
        let (key, body, out) = if let Some(request) = item_request(&lower) {
            if let ItemRequest::Identity { id, shop: false, container } = &request {
                if id.kind == ao_net::n3::quest::QUEST_KIND && *container == ao_net::msg::Identity::default() {
                    (url.to_owned(), Some(String::new()), vec![InfoOut::Quest(*id)])
                } else {
                    (url.to_owned(), Some(String::new()), vec![InfoOut::Item { url: url.to_owned(), request }])
                }
            } else {
                (url.to_owned(), Some(String::new()), vec![InfoOut::Item { url: url.to_owned(), request }])
            }
        } else if lower.starts_with("charid://") {
            let Some((id, tail)) = character_url(&url[9..]) else { return vec![] };
            if !tail.is_empty() { return vec![]; }
            (url.to_owned(), Some("<center><font color=CCRed>Please wait<br>Transferring information</font></center>".into()), vec![InfoOut::Character(id)])
        } else if lower.starts_with("charref://") {
            let Some((_, body)) = character_url(&url[10..]) else { return vec![] };
            if body.is_empty() { return vec![]; }
            let key = &url[..url.len() - body.len() - 1];
            (key.to_owned(), Some(body.to_owned()), vec![])
        } else if lower.starts_with("text://") {
            (text_url_key(url), Some(url[7..].to_owned()), vec![])
        } else if lower.starts_with("chatcmd://") {
            return vec![InfoOut::Command(url[10..].to_owned())];
        } else if lower.starts_with("file://") {
            let q = url.find('?');
            let name = &url[7..q.unwrap_or(url.len())];
            match self.load_file(name) {
                Some(t) => (url.to_owned(), Some(t), vec![]),
                None => return vec![InfoOut::Error(format!("Infoview failed to load and show file: <{name}>"))],
            }
        } else {
            return vec![];
        };
        if toggle && self.url == key && was_open {
            self.close(gui);
            return vec![];
        }
        self.url = key.clone();
        self.ensure_window(gui, screen);
        let Some(w) = self.win else { return out };
        // `FUN_100eda84`: `?section=N` selects one <section> of the file
        let text = match (section_param(&key), body) {
            (Some(n), Some(t)) => extract_section(&t, n),
            (_, t) => t.unwrap_or_default(),
        };
        let scroll = gui.scroll_offset(w, "BrowserView");
        self.hist.visit(&key, text, scroll);
        self.show_current(gui);
        out
    }

    /// `InfoView_c` response slot: a response for an older target must not replace the current page.
    pub fn character_page(&mut self, gui: &mut Gui, id: ao_net::msg::Identity, html: String) {
        self.identity_page(gui,"charid://",id,html);
    }

    pub fn quest_page(&mut self, gui: &mut Gui, id: ao_net::msg::Identity, html: String) {
        self.identity_page(gui,"itemid://",id,html);
    }

    pub fn item_page(&mut self, gui: &mut Gui, url: &str, html: String) {
        if self.win.is_none() || self.url != url { return; }
        let scroll = self.win.map(|w| gui.scroll_offset(w, "BrowserView")).unwrap_or_default();
        self.hist.visit(url, html, scroll);
        self.show_current(gui);
    }

    fn identity_page(&mut self, gui: &mut Gui, scheme: &str, id: ao_net::msg::Identity, html: String) {
        if self.win.is_none() || !self.url.get(..scheme.len()).is_some_and(|s|s.eq_ignore_ascii_case(scheme)) || !character_url(&self.url[scheme.len()..]).is_some_and(|(target,_)| target == id) { return; }
        let key = self.url.clone();
        let scroll = self.win.map(|w| gui.scroll_offset(w, "BrowserView")).unwrap_or_default();
        self.hist.visit(&key, html, scroll);
        self.show_current(gui);
    }

    fn ensure_window(&mut self, gui: &mut Gui, screen: (u32, u32)) {
        if self.win.is_some() {
            return;
        }
        // GUESS: placement (the window is created with `Rect()` and the saved `InfoViewConfig` position; centred here)
        let pos = ((screen.0 as i32 - CLIENT.0 as i32) / 2, (screen.1 as i32 - CLIENT.1 as i32) / 2);
        match gui.open_tabbed_window("InfoView", "Info", pos, WindowSize::Fixed(CLIENT.0, CLIENT.1)) {
            Ok(w) => {
                self.win = Some(w);
                // ctor tail of `FUN_100ee972`: Forward starts disabled, Back only when there is older history
                gui.set_enabled(w, "ForwardButton", false);
                gui.set_enabled(w, "BackButton", false);
                if !self.hist.e.is_empty() {
                    // reopening shows the last visited page (`DAT_10276618 = last`)
                    self.hist.cur = self.hist.e.len() - 1;
                }
            }
            Err(e) => eprintln!("chat: InfoView window: {e:#}"),
        }
    }

    /// Text of the current history entry + its saved scroll position + the button states.
    fn show_current(&mut self, gui: &mut Gui) {
        let Some(w) = self.win else { return };
        let Some(e) = self.hist.e.get(self.hist.cur) else { return };
        gui.set_text(w, "BrowserView", &e.text);
        gui.set_scroll_offset(w, "BrowserView", e.scroll);
        gui.set_enabled(w, "BackButton", self.hist.can_back());
        gui.set_enabled(w, "ForwardButton", self.hist.can_forward());
        self.url = e.url.clone();
    }

    pub fn close(&mut self, gui: &mut Gui) {
        if let Some(w) = self.win.take() {
            gui.close_window(w);
        }
        // `info_window` false: the window object (and its URL member) is gone, the history list stays
        self.url.clear();
    }

    /// GUI event; `true` when it belonged to the window.
    pub fn event(&mut self, gui: &mut Gui, screen: (u32, u32), ev: &Event) -> (bool, Vec<InfoOut>) {
        let Some(w) = self.win else { return (false, vec![]) };
        match ev {
            Event::Clicked { window, view, .. } if *window == w => {
                let scroll = gui.scroll_offset(w, "BrowserView");
                let moved = match view.as_str() {
                    "BackButton" => self.hist.back(scroll),
                    "ForwardButton" => self.hist.forward(scroll),
                    _ => false,
                };
                if moved {
                    self.show_current(gui);
                }
                (true, vec![])
            }
            Event::LinkClicked { window, href, .. } if *window == w => {
                // links of a Tip-of-the-day page are not filtered by `FUN_100ed87d` either way; all of them go to `FUN_100ee961` (flag 0)
                (true, self.show_url(gui, screen, href, false))
            }
            Event::CloseRequested { window } if *window == w => {
                self.close(gui);
                (true, vec![])
            }
            _ => (false, vec![]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn external_urls_do_not_visit_or_open_an_info_page() {
        let Some((mut gui, mut iv)) = rig() else { return };
        for url in ["http://example.com/a", "HTTPS://example.com/b"] {
            assert_eq!(iv.show_url(&mut gui, (1280, 800), url, true), [InfoOut::ExternalUrl(url.into())]);
        }
        assert!(iv.hist.e.is_empty());
        assert!(iv.url.is_empty());
        assert!(iv.window().is_none());
    }

    #[test]
    fn item_urls_preserve_template_quality_and_container_identity() {
        let item = ao_net::n3::world::AcgItem { low_id: 1, high_id: 2, level: 200 };
        assert_eq!(item_request("itemref://1/2/200"), Some(ItemRequest::Reference(item)));
        assert_eq!(item_request("skillid://152"), Some(ItemRequest::Skill(152)));
        let id = ao_net::msg::Identity { kind: 104, instance: 64 };
        let container = ao_net::msg::Identity { kind: 50000, instance: 42 };
        assert_eq!(item_request("itemid://104/64/50000/42"), Some(ItemRequest::Identity { id, shop: false, container }));
        assert_eq!(item_request("shopitemid://104/64"), Some(ItemRequest::Identity { id, shop: true, container: Default::default() }));
        for bad in ["itemref://1/2", "itemref://1/2/3/4", "skillid://-1", "itemid://104/64/1", "shopitemid://104/64/1/2", "itemid://x/1"] {
            assert!(item_request(bad).is_none(), "{bad}");
        }
    }


    #[test]
    fn character_identity_keeps_embedded_html_intact() {
        let id = ao_net::msg::Identity { kind: 50000, instance: 42 };
        assert_eq!(character_url("50000/42"), Some((id, "")));
        assert_eq!(character_url("50000/42/<a href=\"chatcmd:///inspect 42\">Info</a>"), Some((id, "<a href=\"chatcmd:///inspect 42\">Info</a>")));
        for bad in ["50000", "x/42", "50000/x", "50000/2147483648"] {
            assert!(character_url(bad).is_none());
        }
    }

    #[test]
    fn section_extraction_matches_the_tip_file_layout() {
        let html = "<section>one</section>\n\n<section></font>two</section>\n<section>three";
        assert_eq!(extract_section(html, 0), "one");
        assert_eq!(extract_section(html, 1), "</font>two");
        // not closed / absent: empty (the original leaves its result string empty)
        assert_eq!(extract_section(html, 2), "");
        assert_eq!(extract_section(html, 7), "");
        assert_eq!(section_param("file://x/TipOfTheDay.html?section=12"), Some(12));
        assert_eq!(section_param("file://x.html"), None);
        assert_eq!(section_param("file://x.html?a=1&Section = 3"), Some(3));
    }

    #[test]
    fn text_url_key_is_the_signed_byte_sum_from_offset_7() {
        assert_eq!(text_url_key("text://ab"), format!("text://{}", b'a' as u32 + b'b' as u32));
        assert_eq!(text_url_key("text://"), "text://0");
        // signed `char`: a Latin-1 byte >= 0x80 counts negative
        assert_eq!(text_url_key("text://\u{e4}"), format!("text://{}", (-28i32) as u32));
    }

    #[test]
    fn history_follows_fun_100eda84() {
        let mut h = History::default();
        h.visit("a", "A".into(), 0.0);
        assert!(!h.can_back() && !h.can_forward());
        h.visit("b", "B".into(), 11.0);
        assert!(h.can_back() && !h.can_forward());
        assert_eq!(h.e[0].scroll, 11.0);
        assert!(h.back(5.0));
        assert_eq!((h.e[h.cur].url.as_str(), h.e[h.cur].scroll), ("a", 11.0));
        assert!(h.can_forward() && !h.can_back());
        // same URL: text replaced, nothing appended
        h.visit("a", "A2".into(), 0.0);
        assert_eq!((h.e.len(), h.e[0].text.as_str()), (2, "A2"));
        // a new page drops the forward part
        h.visit("c", "C".into(), 0.0);
        assert_eq!(h.e.iter().map(|e| e.url.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert!(!h.forward(0.0) && h.back(0.0) && !h.back(0.0));
    }

    fn rig() -> Option<(Gui, InfoView)> {
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() {
            eprintln!("skipping: no client");
            return None;
        }
        Some((Gui::new(&client, None).unwrap(), InfoView::new(&client)))
    }

    #[test]
    fn item_pages_share_history_toggle_and_reject_stale_urls() {
        let Some((mut gui, mut iv)) = rig() else { return };
        let url = "itemref://1/2/200";
        let request = ItemRequest::Reference(ao_net::n3::world::AcgItem { low_id: 1, high_id: 2, level: 200 });
        assert_eq!(iv.show_url(&mut gui, (1280, 800), url, false), [InfoOut::Item { url: url.into(), request }]);
        let w = iv.window().unwrap();
        iv.item_page(&mut gui, url, "First item".into());
        assert!(gui.text(w, "BrowserView").contains("First item"));
        iv.show_url(&mut gui, (1280, 800), "skillid://152", false);
        iv.item_page(&mut gui, url, "stale".into());
        assert!(!gui.text(w, "BrowserView").contains("stale"));
        iv.item_page(&mut gui, "skillid://152", "Skill".into());
        assert_eq!(iv.hist.e.len(), 2);
        assert!(iv.show_url(&mut gui, (1280, 800), "skillid://152", true).is_empty());
        assert!(iv.window().is_none());
    }

    #[test]
    fn character_requests_render_replies_and_reject_stale_targets() {
        let Some((mut gui, mut iv)) = rig() else { return };
        let id = ao_net::msg::Identity { kind: 50000, instance: 42 };
        assert_eq!(iv.show_url(&mut gui, (1280,800), "charid://50000/42", true), [InfoOut::Character(id)]);
        let w = iv.window().expect("character Info window");
        assert!(gui.text(w,"BrowserView").contains("Transferring information"));
        iv.character_page(&mut gui, id, "<font color=CCInfoHeadline>NPC</font><br>".into());
        assert!(gui.text(w,"BrowserView").contains("NPC"));
        assert_eq!(iv.show_url(&mut gui,(1280,800),"CHARID://050000/042",false),[InfoOut::Character(id)]);
        iv.character_page(&mut gui,id,"normalized identity".into());
        assert!(gui.text(w,"BrowserView").contains("normalized identity"));
        assert_eq!(iv.url,"CHARID://050000/042");
        iv.show_url(&mut gui,(1280,800),"charid://50000/43",true);
        iv.character_page(&mut gui,id,"stale".into());
        assert!(!gui.text(w,"BrowserView").contains("stale"));
        iv.show_url(&mut gui,(1280,800),"charref://50000/42/<font color=CCInfoHeadline>saved</font>",false);
        assert!(gui.text(w,"BrowserView").contains("saved"));
        assert_eq!(iv.url,"charref://50000/42");
        let quest = ao_net::msg::Identity {kind:ao_net::n3::quest::QUEST_KIND,instance:77};
        assert_eq!(iv.show_url(&mut gui,(1280,800),"itemid://56003/77",false),[InfoOut::Quest(quest)]);
        iv.quest_page(&mut gui,quest,"<font color=CCInfoHeadline>Mission</font>".into());
        assert!(gui.text(w,"BrowserView").contains("Mission"));
        iv.character_page(&mut gui,id,"wrong page".into());
        assert!(!gui.text(w,"BrowserView").contains("wrong page"));
    }

    #[test]
    fn help_pages_open_navigate_and_toggle() {
        let Some((mut gui, mut iv)) = rig() else { return };
        let scr = (1280, 800);
        assert!(iv.show_url(&mut gui, scr, "file://helpcommands.html", true).is_empty());
        let w = iv.window().expect("window");
        assert!(gui.text(w, "BrowserView").contains("Help Commands"));
        assert!(!gui.is_enabled(w, "BackButton") && !gui.is_enabled(w, "ForwardButton"));
        // a link on the page: Pet Commands (a file name with a blank)
        assert!(iv.show_url(&mut gui, scr, "file://Pet Commands.html", false).is_empty());
        assert!(gui.is_enabled(w, "BackButton") && !gui.is_enabled(w, "ForwardButton"));
        let (hit, _) = iv.event(&mut gui, scr, &Event::Clicked { window: w, view: "BackButton".into(), item: None });
        assert!(hit && gui.text(w, "BrowserView").contains("Help Commands") && gui.is_enabled(w, "ForwardButton"));
        // the same URL through ShowURL closes the window; the history survives for the next one
        assert!(iv.show_url(&mut gui, scr, "file://helpcommands.html", true).is_empty());
        assert!(iv.window().is_none());
        iv.show_url(&mut gui, scr, "file://chatcommands.html", true);
        let w = iv.window().unwrap();
        assert!(gui.is_enabled(w, "BackButton"));
    }

    #[test]
    fn missing_file_reports_and_chatcmd_runs() {
        let Some((mut gui, mut iv)) = rig() else { return };
        let out = iv.show_url(&mut gui, (1280, 800), "file://nope.html", true);
        assert_eq!(out, [InfoOut::Error("Infoview failed to load and show file: <nope.html>".into())]);
        assert!(iv.window().is_none());
        let out = iv.show_url(&mut gui, (1280, 800), "chatcmd:///close infoview", false);
        assert_eq!(out, [InfoOut::Command("/close infoview".into())]);
    }

    #[test]
    fn tip_of_the_day_sections_come_from_the_real_file() {
        let Some((mut gui, mut iv)) = rig() else { return };
        let p = ao_gui::client_dir().join("cd_image/text/TipOfTheDay.html");
        iv.show_url(&mut gui, (1280, 800), &format!("file://{}?section=3", p.display()), true);
        let w = iv.window().unwrap();
        let t = gui.text(w, "BrowserView");
        assert!(t.contains("Tip of the Day") && !t.contains("</section>"), "{t}");
    }

    /// `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac chat_info_shot -- --nocapture` writes `info-help.png`, `info-tip.png`, `dialog-yesno.png`, `dialog-afk.png`.
    #[test]
    fn chat_info_shot() {
        use super::super::dialog::{Dialogs, Kind, Spec};
        let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(std::path::PathBuf::from) else { return };
        let client = ao_gui::client_dir();
        if !client.join("cd_image/gui").exists() {
            return eprintln!("skipping: no client");
        }
        let labels = ao_formats::screens::TextDb::load(&client).unwrap();
        let size = (1280u32, 828u32);
        let gui = Gui::new(&client, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).unwrap();
        struct Fe(Gui);
        impl ao_render::Frontend for Fe {
            fn gui(&self) -> &Gui {
                &self.0
            }
            fn input(&mut self, ev: ao_gui::InputEvent, _: &mut ao_render::Host) {
                self.0.input(ev);
            }
            fn frame(&mut self, dt: f32, _: (u32, u32), _: &mut ao_render::Host) -> ao_gui::DrawList {
                self.0.frame(dt)
            }
        }
        let mut fe = Fe(gui);
        let snap = |fe: &mut Fe, name: &str| {
            let mut o = ao_render::Offscreen::new(fe, size).unwrap();
            std::fs::create_dir_all(&out).unwrap();
            let list = o.frame(fe, 0.016);
            o.png(fe, &list, &out.join(name)).unwrap();
            eprintln!("wrote {name}");
        };
        let mut iv = InfoView::new(&client);
        iv.show_url(&mut fe.0, size, "file://helpcommands.html", true);
        snap(&mut fe, "info-help.png");
        let p = client.join("cd_image/text/TipOfTheDay.html");
        iv.show_url(&mut fe.0, size, &format!("file://{}?section=1", p.display()), true);
        snap(&mut fe, "info-tip.png");
        iv.close(&mut fe.0);
        let mut d = Dialogs::default();
        let spec = |kind, body: &str, buttons: &[&str], input: Option<&str>| Spec { kind, body: body.into(), buttons: buttons.iter().map(|s| s.to_string()).collect(), input: input.map(Into::into) };
        d.go(&mut fe.0, size, &spec(Kind::OrgLeave, "Do you really want to leave your organization?", &["Yes", "No"], None));
        snap(&mut fe, "dialog-yesno.png");
        d.go(&mut fe.0, size, &spec(Kind::Afk, "Enter your AFK message (30 seconds):", &["Ok"], Some("The user is currently AFK.")));
        snap(&mut fe, "dialog-afk.png");
    }
}
