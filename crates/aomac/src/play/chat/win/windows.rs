//! `ChatWindows`: the GUI windows (`ChatWindow_c` frames with their tabs), lines, activation fades, tab docking and persistence (docs/chat/gui.md §2, §7, §10).

use super::*;

/// Client-size floor of a framed chat window (`WndBorder::SetSizeLimits`): the original sets none (`ChatWindow_c` never calls it; only `DoSetFrame`'s
/// button-row width applies). **GUESS**: without a floor the input bar / text area layout collapses.
pub(super) const MIN_CLIENT: (u32, u32) = (50, 60);
/// Offset of a tab torn out of its window (`FUN_10097d0b`: `SetFrame(Bounds + Point)` then `MoveInsideScreen(true, true, true)`; the `Point` is not
/// readable in the decompile): **GUESS** 20 px down-right.
const TEAR_OFFSET: f32 = 20.0;

impl ChatWindows {
    /// Opens the windows from (first hit): `<prefs dir>/Chat/Windows/*/Config.xml`, the client's `prefs/NewChar/Chat/Windows/*`,
    /// else the code defaults of `FUN_10094c58`.
    pub fn new(gui: &mut Gui, screen: (u32, u32)) -> Result<Self> {
        let client = ao_gui::client_dir();
        let prefs = super::super::super::prefs::dir();
        let mut cfgs = prefs.as_deref().map(read_windows).unwrap_or_default();
        if cfgs.is_empty() {
            cfgs = read_windows(&client.join("prefs/NewChar"));
            cfgs.iter_mut().for_each(|c| c.template = true);
        }
        if cfgs.is_empty() {
            cfgs = code_defaults();
            cfgs.iter_mut().for_each(|c| c.template = true);
        }
        let mut s = ChatWindows {
            wins: vec![],
            frames: vec![],
            screen,
            groups: LOCAL_GROUPS.iter().map(|(i, n)| (*i, n.to_string())).collect(),
            text: TextDb::load(&client).ok(),
            prefs,
            last_active: String::new(),
            next_n: 0,
            reserved: Reserved::default(),
            dirty: false,
            menu: None,
            pw: WinPrefs::default(),
        };
        s.last_active = cfgs.iter().find(|c| c.startup).or(cfgs.first()).map(|c| c.window_name.clone()).unwrap_or_default();
        for c in cfgs.into_iter().filter(|c| c.open) {
            s.add_doc(c);
        }
        // Windows in the normal (tabbed) mode with the very same saved `WindowFrame` share one `ChatWindow`, ordered by `tab_index`
        // (**GUESS**: the original keeps the tab group in a field (`+0x98`) that `FUN_1009a77b` does not write, so the grouping rule is ours).
        let mut groups: Vec<Vec<usize>> = vec![];
        for d in 0..s.wins.len() {
            let c = &s.wins[d].cfg;
            if c.visual_mode == 0 && c.frame.is_some() {
                if let Some(g) = groups.iter_mut().find(|g| s.wins[g[0]].cfg.visual_mode == 0 && s.wins[g[0]].cfg.frame == c.frame) {
                    g.push(d);
                    continue;
                }
            }
            groups.push(vec![d]);
        }
        for mut g in groups {
            g.sort_by_key(|&d| s.wins[d].cfg.tab_index);
            let outer = place(s.wins[g[0]].cfg.frame, s.wins[g[0]].cfg.template, s.screen, s.reserved);
            s.add_frame(gui, g, outer)?;
        }
        Ok(s)
    }

    fn add_doc(&mut self, cfg: Cfg) -> usize {
        let n = self.next_n;
        self.next_n += 1;
        let alpha = alpha_of(&cfg, false);
        self.wins.push(Win { cfg, id: usize::MAX, frame: usize::MAX, n, lines: VecDeque::new(), alpha, target: alpha, rate: 0.0, active: false, hint: String::new() });
        self.wins.len() - 1
    }

    /// Registers a frame holding `docs` and opens its GUI window.
    fn add_frame(&mut self, gui: &mut Gui, docs: Vec<usize>, outer: (i32, i32, u32, u32)) -> Result<usize> {
        let fi = self.frames.len();
        for &d in &docs {
            self.wins[d].frame = fi;
        }
        self.frames.push(Frame { id: usize::MAX, docs: docs.clone(), sel: 0, placed: outer });
        let id = self.make_frame(gui, &docs, outer, 0)?;
        self.frames[fi].id = id;
        Ok(fi)
    }

    /// The tab title (`FUN_100ab980`): the window's `name` plus, with `ChatShowOGrpInTitleBar`, ` <font color=green>[<output group>]</font>`.
    pub(super) fn title(&self, d: usize) -> String {
        let c = &self.wins[d].cfg;
        match self.group_name(c.output_group) {
            g if self.pw.title_group && c.output_group != 0 && !g.is_empty() => format!("{} <font color=green>[{g}]</font>", c.name),
            _ => c.name.clone(),
        }
    }

    /// The titles of the tabs of frame `fi`.
    pub(super) fn titles(&self, fi: usize) -> Vec<String> {
        self.frames[fi].docs.iter().map(|&d| self.title(d)).collect()
    }

    /// The view XML of one window's tab: `GroupChatView_c` around the text area and the input bar (`ChatView_c::FUN_1008d728`, input mode 1: input bar at its
    /// preferred height = one text line plus its border, the text frame ends 5 px above it). The look depends on `visual_mode` (`FUN_100aab08`):
    /// * 0: the group view has no art and a 3 px client border; the `ChatView` borders (`FUN_1008daa0(true)`) are the `GFX_GUI_INSET_*` set with a 5 px client margin;
    /// * 1: the group view is a `GFX_GUI_WINDOW3_BORDER_*` + `GFX_GUI_WINDOW_BACKGROUND` frame at local alpha 0.4 (`_DAT_101b9e78`), 3 px border, inset `ChatView`;
    /// * 2 (shipped default): no group border (client 0) and `FUN_1008daa0(false)`: both `ChatView` borders are *only* `GFX_GUI_WINDOW_BACKGROUND` (0x1bf) with
    ///   no client margin, so the text area and the input bar are black panels (the window alpha 0.8 / 0.3 of `FUN_10096b23` fades them with the text).
    ///
    /// `none` names no art (the XML parser keeps the INSET default for an *empty* attribute).
    pub(super) fn doc_xml(&self, d: usize, line_h: u32) -> String {
        let w = &self.wins[d];
        let n = w.n;
        let mode = w.cfg.visual_mode;
        let (art, margin) = if mode == 2 {
            (r#"tl_gfx="none" tr_gfx="none" bl_gfx="none" br_gfx="none" left_gfx="none" top_gfx="none" right_gfx="none" bottom_gfx="none" bg_gfx="GFX_GUI_WINDOW_BACKGROUND""#, 0)
        } else {
            ("", 5)
        };
        let ih = line_h + 2 * margin as u32;
        let input = if w.cfg.textinput {
            format!(
                r#"<BorderView name="input_border_{n}" {art} layout_borders="Rect(0,5,0,0)" min_size="Point(-1,{ih})" max_size="Point(16000,{ih})">
                     <TextView name="input_{n}" max_size="Point(16000,-1)" layout_borders="Rect({margin},{margin},{margin},{margin})" font="CHAT" feature_flags="{INPUT_FLAGS}"/>
                   </BorderView>"#
            )
        } else {
            String::new()
        };
        let m = format!("Rect({margin},{margin},{margin},{margin})");
        let body = format!(
            r#"<BorderView name="text_border_{n}" {art} max_size="Point(16000,16000)">
                   <ScrollView name="scroll_{n}" v_scrollbar_mode="always" layout_borders="{m}" max_size="Point(16000,16000)">
                     <ScrollViewChild view_layout="vertical" max_size="Point(16000,16000)">
                       <TextView name="text_{n}" max_size="Point(16000,-1)" font="CHAT" feature_flags="{TEXT_FLAGS}"/>
                     </ScrollViewChild>
                   </ScrollView>
                 </BorderView>
                 {input}"#
        );
        match mode {
            2 => format!(r#"<View name="chat_{n}" view_layout="vertical" max_size="Point(16000,16000)">{body}</View>"#),
            1 => format!(
                r#"<View name="chat_{n}" view_layout="vertical" max_size="Point(16000,16000)">
                     <BorderView tl_gfx="GFX_GUI_WINDOW3_BORDER_TL" tr_gfx="GFX_GUI_WINDOW3_BORDER_TR" bl_gfx="GFX_GUI_WINDOW3_BORDER_BL" br_gfx="GFX_GUI_WINDOW3_BORDER_BR" left_gfx="GFX_GUI_WINDOW3_BORDER_LEFT" top_gfx="GFX_GUI_WINDOW3_BORDER_TOP" right_gfx="GFX_GUI_WINDOW3_BORDER_RIGHT" bottom_gfx="GFX_GUI_WINDOW3_BORDER_BOTTOM" bg_gfx="GFX_GUI_WINDOW_BACKGROUND" alpha="0.4" view_layout="vertical" max_size="Point(16000,16000)">
                       <View view_layout="vertical" layout_borders="Rect(3,3,3,3)" max_size="Point(16000,16000)">{body}</View>
                     </BorderView>
                   </View>"#
            ),
            _ => format!(r#"<View name="chat_{n}" view_layout="vertical" layout_borders="Rect(3,3,3,3)" max_size="Point(16000,16000)">{body}</View>"#),
        }
    }

    /// Opens the GUI window of a frame: visual mode 0 = style-0 window with a tab strip (one tab per window, `Window::InsertTab`), else the borderless
    /// style-3 window. `outer` = (x, y, w, h) of the whole window; returns the window id (also stored in every window of `docs`).
    fn make_frame(&mut self, gui: &mut Gui, docs: &[usize], outer: (i32, i32, u32, u32), sel: usize) -> Result<WindowId> {
        let sel = sel.min(docs.len() - 1);
        let line_h = gui.font_height(ao_gui::FontId::Chat) as u32;
        let views: String = docs.iter().map(|&d| self.doc_xml(d, line_h)).collect();
        let src = format!(r#"<root><View name="chat_frame" view_layout="vertical">{views}</View></root>"#);
        let (x, y, w, h) = outer;
        let id = if self.wins[docs[0]].cfg.visual_mode == 0 {
            let names: Vec<String> = docs.iter().map(|&d| self.title(d)).collect();
            let id = gui.open_tabbed_window_xml("ChatWindow", &names[sel], &src, (x, y), WindowSize::Fixed(w.saturating_sub(10).max(1), h.saturating_sub(31).max(1)))?;
            gui.set_window_fade(id, false); // chat windows run their own `FadeTo` rule (docs/chat/gui.md §3), not the generic hover fade
            gui.set_window_tabs(id, &names, sel);
            gui.set_window_frame(id, true, true);
            gui.set_window_size_limits(id, MIN_CLIENT, (0, 0));
            id
        } else {
            gui.open_window_xml("ChatWindow", &src, (x, y), WindowSize::Fixed(w, h))?
        };
        gui.set_window_context(id, true);
        gui.set_text_shadow_offset(1, 1); // `ChatTextShadowOffset` pref default 1 (CharPrefs.xml), `FUN_1008d673`
        for (i, &d) in docs.iter().enumerate() {
            self.wins[d].id = id;
            gui.show_collapsing(id, &self.wins[d].chat(), i == sel);
            self.refresh(gui, d);
        }
        let d = docs[sel];
        let a = alpha_of(&self.wins[d].cfg, self.wins[d].active);
        (self.wins[d].alpha, self.wins[d].target) = (a, a);
        let s = &self.wins[d];
        gui.set_window_alpha(id, a);
        // `is_backmost` / `is_frontmost` -> window flags 0x200 / 0x100 (`FUN_10097ae3`); default: normal stacking. **GUESS**: frontmost wins if both are set.
        gui.set_window_layer(id, if s.cfg.frontmost { 1 } else if s.cfg.backmost { -1 } else { 0 });
        Ok(id)
    }

    /// Closes and re-opens the GUI window of frame `fi` from its current state (visual mode, tabs, input bars changed).
    pub(super) fn rebuild_frame(&mut self, gui: &mut Gui, fi: usize) {
        let (old, docs, sel, placed) = {
            let f = &self.frames[fi];
            (f.id, f.docs.clone(), f.sel.min(f.docs.len().saturating_sub(1)), f.placed)
        };
        gui.close_window(old);
        match self.make_frame(gui, &docs, placed, sel) {
            Ok(id) => {
                self.frames[fi].id = id;
                self.frames[fi].sel = sel;
            }
            Err(e) => eprintln!("chat: window rebuild failed: {e:#}"),
        }
    }

    fn refresh(&mut self, gui: &mut Gui, d: usize) {
        let w = &self.wins[d];
        let all = w.lines.iter().map(String::as_str).collect::<Vec<_>>().join("<br>");
        gui.set_text(w.id, &format!("text_{}", w.n), &all);
        gui.scroll_to_bottom(w.id, &w.scroll());
    }

    pub(super) fn frame_of(&self, window: WindowId) -> Option<usize> {
        self.frames.iter().position(|f| f.id == window)
    }

    /// The window whose tab is selected in frame `fi`.
    pub(super) fn sel_doc(&self, fi: usize) -> usize {
        let f = &self.frames[fi];
        f.docs[f.sel.min(f.docs.len() - 1)]
    }

    pub fn resize(&mut self, gui: &mut Gui, screen: (u32, u32)) {
        if screen == self.screen {
            return;
        }
        self.screen = screen;
        self.replace_all(gui);
    }

    fn replace_all(&mut self, gui: &mut Gui) {
        for f in &mut self.frames {
            let c = &self.wins[f.docs[0]].cfg;
            f.placed = place(c.frame, c.template, self.screen, self.reserved);
            gui.set_window_outer_frame(f.id, f.placed);
        }
    }

    /// The screen area the HUD's wings/bars cover; only the *template* default windows (first run) are kept clear of it (see [`place`]).
    /// Windows with a saved frame are positioned as the original does and may overlap the HUD (they draw above it).
    pub fn set_reserved(&mut self, gui: &mut Gui, reserved: Reserved) {
        if reserved != self.reserved {
            self.reserved = reserved;
            self.replace_all(gui);
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

    pub(super) fn group_name(&self, id: u64) -> String {
        self.groups.get(&id).cloned().unwrap_or_default()
    }

    fn fill(&mut self, gui: &mut Gui, i: usize, html: &str) {
        let w = &mut self.wins[i];
        w.lines.push_back(html.to_string());
        while w.lines.len() > MAX_LINES {
            w.lines.pop_front();
        }
        self.refresh(gui, i);
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
    /// `group_hint` = a group name (`Other` combat lines name their group).
    pub fn push(&mut self, gui: &mut Gui, line: &ChatLine, group_hint: Option<&str>) {
        let by_name = |n: &str| self.groups.iter().find(|(_, g)| g.eq_ignore_ascii_case(n)).map(|(i, _)| *i);
        let (group, color) = match &line.kind {
            ChatKind::Error | ChatKind::System => (G_SYSTEM, line.kind.color_name().to_string()),
            ChatKind::TellOut => (G_TELL, line.kind.color_name().to_string()),
            ChatKind::Other(c) => (group_hint.and_then(by_name).unwrap_or(G_SYSTEM), c.to_string()),
        };
        self.deliver(gui, group, |stamp| format!("<div indent=wrapped><font color={color}>{stamp}{}</font></div>", line.text));
    }

    /// The event comes from one of the chat windows (the app must not handle it again).
    pub fn owns(&self, ev: &Event) -> bool {
        let w = match ev {
            Event::EnterPressed { window, .. }
            | Event::Escape { window }
            | Event::LinkClicked { window, .. }
            | Event::TextChanged { window, .. }
            | Event::FrameIcon { window, .. }
            | Event::TabSelected { window, .. }
            | Event::TabDropped { window, .. }
            | Event::WindowFrame { window }
            | Event::ContextMenu { window, .. } => *window,
            Event::MenuPicked { .. } | Event::MenuSlider { .. } => return self.menu.is_some(),
            _ => return false,
        };
        self.frames.iter().any(|x| x.id == w)
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
                if self.frames.iter().any(|f| f.id == *window) {
                    gui.clear_focus();
                }
            }
            Event::LinkClicked { window, href, .. } => {
                if let Some(rest) = href.strip_prefix("user://") {
                    out.push(WinOut::OpenTell(rest.to_string()));
                } else if let Some(rest) = href.strip_prefix("chatgroup://") {
                    // `ChatView_c` signal +0x134: the window's output group becomes the clicked group (docs/chat/gui.md §6, guess)
                    if let (Some(g), Some(fi)) = (parse_ident(rest), self.frame_of(*window)) {
                        let d = self.sel_doc(fi);
                        self.wins[d].cfg.output_group = g;
                        self.dirty = true;
                    }
                } else {
                    out.push(WinOut::LinkClicked(href.clone()));
                }
            }
            Event::TabSelected { window, index } => {
                if let Some(fi) = self.frame_of(*window) {
                    self.select_tab(gui, fi, *index);
                }
            }
            Event::WindowFrame { window } => {
                if let Some(fi) = self.frame_of(*window) {
                    if let Some(r) = gui.window_outer_frame(*window) {
                        self.frames[fi].placed = r;
                        self.sync_frame_cfg(fi);
                    }
                }
            }
            Event::TabDropped { window, tab, target, .. } => self.tab_dropped(gui, *window, *tab, *target),
            Event::FrameIcon { window, x, y } => self.open_menu(gui, *window, (*x, *y), None),
            Event::ContextMenu { window, x, y, link } => self.open_menu(gui, *window, (*x, *y), link.as_deref()),
            Event::MenuPicked { id } => out.extend(self.menu_picked(gui, *id)),
            Event::MenuSlider { id, value } => self.menu_slider(gui, *id, *value),
            _ => {}
        }
        out
    }

    /// Copies the frame's placement into the config of every window in it (`WindowFrame` of `Window::SaveWndConfig`, the same value for every tab).
    pub(super) fn sync_frame_cfg(&mut self, fi: usize) {
        let (x, y, w, h) = self.frames[fi].placed;
        for (i, &d) in self.frames[fi].docs.clone().iter().enumerate() {
            let c = &mut self.wins[d].cfg;
            c.frame = Some([x as f32, y as f32, (x + w as i32 - 1) as f32, (y + h as i32 - 1) as f32]);
            c.template = false;
            c.tab_index = i as i32;
        }
        self.dirty = true;
    }

    fn reindex(&mut self) {
        for (fi, f) in self.frames.iter().enumerate() {
            for &d in &f.docs {
                self.wins[d].frame = fi;
            }
        }
    }

    /// A tab was pressed (`TabView` selection, `Window::SetTabSelection`): its view shows, the others collapse; the window alpha follows the selected tab
    /// (`FUN_10096b23`).
    pub(super) fn select_tab(&mut self, gui: &mut Gui, fi: usize, index: usize) {
        let f = &mut self.frames[fi];
        f.sel = index.min(f.docs.len().saturating_sub(1));
        let (id, docs, sel) = (f.id, f.docs.clone(), f.sel);
        for (i, &d) in docs.iter().enumerate() {
            gui.show_collapsing(id, &self.wins[d].chat(), i == sel);
        }
        gui.set_window_alpha(id, self.wins[docs[sel]].alpha);
        gui.clear_focus();
        self.dirty = true;
    }

    /// A tab was dropped (`FUN_10097881` accept on another `ChatWindow`'s `TabView`, `FUN_10097d0b` tear-out): reorder, dock into the target window, or
    /// (only with 2+ tabs) open a new window at the old frame + [`TEAR_OFFSET`].
    fn tab_dropped(&mut self, gui: &mut Gui, window: WindowId, tab: usize, target: Option<(WindowId, usize)>) {
        let Some(src) = self.frame_of(window) else { return };
        let Some(&d) = self.frames[src].docs.get(tab) else { return };
        match target {
            Some((tw, idx)) => {
                let Some(dst) = self.frame_of(tw) else { return };
                if dst == src {
                    let f = &mut self.frames[src];
                    f.docs.remove(tab);
                    let at = if idx > tab { idx - 1 } else { idx }.min(f.docs.len());
                    f.docs.insert(at, d);
                    f.sel = at;
                    self.sync_frame_cfg(src);
                    self.rebuild_frame(gui, src);
                    return;
                }
                self.frames[src].docs.remove(tab);
                let f = &mut self.frames[dst];
                let at = idx.min(f.docs.len());
                f.docs.insert(at, d);
                f.sel = at;
                self.wins[d].cfg.visual_mode = 0;
                if self.frames[src].docs.is_empty() {
                    gui.close_window(self.frames[src].id);
                    self.frames.remove(src);
                    self.reindex();
                    let dst = if dst > src { dst - 1 } else { dst };
                    self.sync_frame_cfg(dst);
                    self.rebuild_frame(gui, dst);
                } else {
                    self.reindex();
                    let f = &mut self.frames[src];
                    f.sel = f.sel.min(f.docs.len() - 1);
                    self.sync_frame_cfg(src);
                    self.sync_frame_cfg(dst);
                    self.rebuild_frame(gui, src);
                    self.rebuild_frame(gui, dst);
                }
            }
            None => {
                if self.frames[src].docs.len() < 2 {
                    return; // `FUN_10097d0b`: a window with a single tab cannot be torn out (`DragObject_c::Cancel`)
                }
                let f = &mut self.frames[src];
                f.docs.remove(tab);
                f.sel = f.sel.min(f.docs.len() - 1);
                let (x, y, w, h) = f.placed;
                let r = [x as f32 + TEAR_OFFSET, y as f32 + TEAR_OFFSET, (x + w as i32 - 1) as f32 + TEAR_OFFSET, (y + h as i32 - 1) as f32 + TEAR_OFFSET];
                let outer = place(Some(r), false, self.screen, Reserved::default());
                self.sync_frame_cfg(src);
                self.rebuild_frame(gui, src);
                match self.add_frame(gui, vec![d], outer) {
                    Ok(nf) => self.sync_frame_cfg(nf),
                    Err(e) => eprintln!("chat: tear-out failed: {e:#}"),
                }
            }
        }
    }

    /// Applies the chat DValues ([`WinPrefs`]): a font change re-creates the CHAT font and rebuilds every frame (input bar height = line height), a
    /// title/prompt change shows on the next [`update`](Self::update).
    pub fn set_prefs(&mut self, gui: &mut Gui, p: &WinPrefs) {
        if *p == self.pw {
            return;
        }
        let font_changed = p.font != self.pw.font;
        self.pw = p.clone();
        if font_changed && gui.set_chat_font(&p.font.0, &p.font.1, p.font.2) {
            for fi in 0..self.frames.len() {
                self.rebuild_frame(gui, fi);
            }
            for w in &mut self.wins {
                w.hint.clear();
            }
        }
    }

    /// What follows from the output groups and `ChatShowOGrpIn*`: the tab titles and the input prompt (`FUN_100ab980`, `FUN_10090f0b`).
    fn sync_decor(&mut self, gui: &mut Gui) {
        for fi in 0..self.frames.len() {
            let id = self.frames[fi].id;
            if self.wins[self.frames[fi].docs[0]].cfg.visual_mode == 0 {
                let titles = self.titles(fi);
                let (cur, sel) = gui.window_tabs(id);
                if cur != titles {
                    gui.set_window_tabs(id, &titles, sel);
                }
            }
            for k in 0..self.frames[fi].docs.len() {
                let d = self.frames[fi].docs[k];
                let w = &self.wins[d];
                if !w.cfg.textinput {
                    continue;
                }
                let g = self.group_name(w.cfg.output_group);
                let hint = if self.pw.input_group && w.cfg.output_group != 0 { g } else { String::new() };
                if hint != w.hint {
                    gui.set_text_hint(id, &w.input(), &hint);
                    self.wins[d].hint = hint;
                }
            }
        }
    }

    /// Activation fades (`FUN_10096b23`): the window whose input bar has the keyboard focus is *active* (alpha
    /// `window_transparency_active`, fade 0.2 s), all others fade to `window_transparency_inactive` over 1 s. Pending changes are written once the
    /// pointer is idle (the original writes at shutdown, `FUN_10094a28`; the port has no shutdown hook).
    pub fn update(&mut self, gui: &mut Gui, dt: f32) {
        self.sync_decor(gui);
        let focused = gui.focused_view();
        let hide = |w: &Win, active: bool| w.cfg.hide_input_when_inactive && !active;
        for i in 0..self.wins.len() {
            let selected = self.frames.get(self.wins[i].frame).is_some_and(|f| self.sel_doc(self.wins[i].frame) == i && f.id == self.wins[i].id);
            let w = &mut self.wins[i];
            let active = focused.as_deref() == Some(w.input().as_str());
            if active != w.active {
                w.active = active;
                w.target = alpha_of(&w.cfg, active);
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
                if selected {
                    gui.set_window_alpha(w.id, w.alpha);
                }
            }
        }
        if self.dirty && !gui.interacting() {
            self.dirty = false;
            if let Err(e) = self.save() {
                eprintln!("chat: saving the window configs failed: {e}");
            }
        }
    }

    /// Enter in the game: focus the input bar of the last active window (`ChatLastActiveWindow`, else the first window with one).
    pub fn focus_input(&mut self, gui: &mut Gui) {
        let pick = self.wins.iter().position(|w| w.cfg.window_name == self.last_active && w.cfg.textinput).or_else(|| self.wins.iter().position(|w| w.cfg.textinput));
        if let Some(i) = pick {
            let fi = self.wins[i].frame;
            if let Some(pos) = self.frames[fi].docs.iter().position(|&d| d == i) {
                if self.frames[fi].sel != pos {
                    self.select_tab(gui, fi, pos);
                    let names = self.titles(fi);
                    gui.set_window_tabs(self.frames[fi].id, &names, pos);
                }
            }
            let w = &self.wins[i];
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

    /// `#%016x#` of the output group of the window that has (or last had) the focus.
    #[cfg(test)]
    pub fn active_output_group(&self) -> Option<String> {
        let w = self.active_win()?;
        (w.cfg.output_group != 0).then(|| group_ident(w.cfg.output_group))
    }

    /// `/ch <group>` (`FUN_1009a06f`): the active window's output group becomes `id`.
    pub fn set_output_group(&mut self, id: u64) {
        let name = self.active_win().map(|w| w.cfg.window_name.clone());
        if let Some(w) = self.wins.iter_mut().find(|w| Some(&w.cfg.window_name) == name.as_ref()) {
            w.cfg.output_group = id;
        }
    }

    /// Shows (`on`) or hides group `group` in the window called `window_name` (the window's channel-subscribe menu; the private-group invite dialog's
    /// "assign to chat windows" checkboxes). False if there is no such window.
    pub fn subscribe_group(&mut self, window_name: &str, group: u64, on: bool) -> bool {
        let Some(w) = self.wins.iter_mut().find(|w| w.cfg.window_name == window_name) else { return false };
        if w.cfg.shows(group) != on {
            if let Some(p) = w.cfg.set.iter().position(|g| *g == group) {
                w.cfg.set.remove(p);
            } else {
                w.cfg.set.push(group);
            }
            self.dirty = true;
        }
        true
    }

    /// `(window_name, name)` of every chat window: the Friends window's "Chat Windows" folder lists the names, the private-group invite dialog
    /// offers a check box per window and subscribes by `window_name` ([`subscribe_group`](Self::subscribe_group)).
    pub fn window_list(&self) -> Vec<(String, String)> {
        self.wins.iter().map(|w| (w.cfg.window_name.clone(), w.cfg.name.clone())).collect()
    }

    /// Writes every window's `Config.xml` under `<prefs dir>/Chat/Windows/<window_name>/` (`FUN_10094a28`, at shutdown).
    pub fn save(&self) -> std::io::Result<()> {
        let Some(dir) = &self.prefs else { return Ok(()) };
        for f in &self.frames {
            let (x, y, ww, hh) = f.placed;
            for (i, &d) in f.docs.iter().enumerate() {
                let w = &self.wins[d];
                let d = dir.join("Chat/Windows").join(&w.cfg.window_name);
                std::fs::create_dir_all(&d)?;
                let mut cfg = w.cfg.clone();
                cfg.frame = Some([x as f32, y as f32, (x + ww as i32 - 1) as f32, (y + hh as i32 - 1) as f32]);
                cfg.tab_index = i as i32;
                std::fs::write(d.join("Config.xml"), cfg.to_xml(&|g| self.group_name(g)))?;
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn window_ids(&self) -> Vec<WindowId> {
        self.frames.iter().map(|f| f.id).collect()
    }
}
