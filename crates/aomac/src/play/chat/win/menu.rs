//! The chat window menus: the icon / right-click menu of `ChatWindow_c` (`FUN_10097289` inserts `FUN_10096f9f` = `FUN_100998bc` + AlwaysBehind / AlwaysOnTop,
//! `GroupChatView_c` adds `FUN_100ab4dc`'s channel entries) and the user-link menu of `ChatView_c::MouseDown` (`FUN_1008f5dd` -> `FUN_1008e135`).
//! Evidence and gaps: docs/chat/gui.md §11.

use super::*;
use ao_gui::MenuItem;

/// What the open menu acts on.
pub(super) struct Ctx {
    /// Frame the menu was opened on (its selected window is the one the entries change).
    frame: usize,
    /// Groups behind the talk-to / subscribe entries (`id & 0xffff` = index).
    groups: Vec<u64>,
    /// `user://NAME` link menu.
    user: Option<String>,
}

// menu ids: `op << 16 | arg`
pub(super) const OP_MODE: u32 = 1;
pub(super) const OP_SLIDER_INACTIVE: u32 = 2;
pub(super) const OP_SLIDER_ACTIVE: u32 = 3;
pub(super) const OP_TIMESTAMPS: u32 = 4;
pub(super) const OP_NO_INPUT: u32 = 5;
pub(super) const OP_HIDE_INPUT: u32 = 6;
pub(super) const OP_FADE: u32 = 7;
pub(super) const OP_BEHIND: u32 = 8;
pub(super) const OP_ON_TOP: u32 = 9;
pub(super) const OP_AUTOSUB: u32 = 10;
pub(super) const OP_TALK_TO: u32 = 11;
pub(super) const OP_SUBSCRIBE: u32 = 12;
pub(super) const OP_IGNORE: u32 = 13;
pub(super) const OP_OPEN_CHAT: u32 = 14;
pub(super) const OP_SEND_TELL: u32 = 15;

fn id(op: u32, arg: u32) -> u32 {
    op << 16 | arg
}

impl ChatWindows {
    /// Text of a `text.mdb` 10001 key (`LDBface::GetText(0x2711, key)`); the key itself when the database lacks it.
    fn label(&self, key: &str) -> String {
        self.text.as_ref().and_then(|d| d.by_key(10001, key)).filter(|s| !s.is_empty()).unwrap_or_else(|| key.to_string())
    }

    /// Groups of the talk-to / subscribe submenus, by name (**GUESS**: the original's order is its group map order).
    pub(super) fn menu_groups(&self) -> Vec<u64> {
        let mut v: Vec<u64> = self.groups.keys().copied().filter(|g| !self.group_name(*g).is_empty()).collect();
        v.sort_by_key(|g| (self.group_name(*g).to_lowercase(), *g));
        v
    }

    /// The window menu of one window (`FUN_100998bc` + `FUN_10096f9f`).
    fn window_menu(&self, d: usize) -> Vec<MenuItem> {
        let c = &self.wins[d].cfg;
        let mut mode = vec![MenuItem::check(id(OP_MODE, 0), &self.label("ChatWindowMenu_Style_Mode_Normal"), c.visual_mode == 0)];
        mode.push(MenuItem::check(id(OP_MODE, 2), &self.label("ChatWindowMenu_Style_Mode_Borderless"), c.visual_mode == 2));
        let mut m = vec![MenuItem::submenu(&self.label("ChatWindowMenu_Style_Mode"), mode)];
        if c.visual_mode != 0 {
            // two `PopupMenuSliderItem_c` (0..1): inactive first (`FUN_10099801`), then active (`FUN_100997be`)
            let sub = vec![MenuItem::slider(id(OP_SLIDER_INACTIVE, 0), c.alpha_inactive), MenuItem::slider(id(OP_SLIDER_ACTIVE, 0), c.alpha_active)];
            m.push(MenuItem::submenu(&self.label("ChatWindowMenu_Style_Transparancy"), sub));
        }
        m.push(MenuItem::separator());
        m.push(MenuItem::check(id(OP_TIMESTAMPS, 0), &self.label("ShowTimestamps"), c.show_timestamps));
        m.push(MenuItem::check(id(OP_NO_INPUT, 0), &self.label("DisableTextInput"), !c.textinput));
        let mut hide = MenuItem::check(id(OP_HIDE_INPUT, 0), &self.label("ChatWindowMenu_Style_HideInputBarWhenInactive"), c.hide_input_when_inactive);
        hide.enabled = c.textinput;
        m.push(hide);
        m.push(MenuItem::check(id(OP_FADE, 0), &self.label("ChatWindowMenu_Style_FadeMessages"), c.fading));
        m.push(MenuItem::separator());
        m.push(MenuItem::check(id(OP_BEHIND, 0), &self.label("ChatWindowMenu_Style_AlwaysBehind"), c.backmost));
        m.push(MenuItem::check(id(OP_ON_TOP, 0), &self.label("ChatWindowMenu_Style_AlwaysOnTop"), c.frontmost));
        m
    }

    /// Opens the right-click / icon menu of frame `window` at `at`; a `user://NAME` link gives the user menu instead.
    pub(super) fn open_menu(&mut self, gui: &mut Gui, window: WindowId, at: (i32, i32), link: Option<&str>) {
        let Some(fi) = self.frame_of(window) else { return };
        let d = self.sel_doc(fi);
        let groups = self.menu_groups();
        let user = link.and_then(|l| l.strip_prefix("user://")).map(str::to_string);
        let items = if user.is_some() {
            // `FUN_1008e135`: IgnoreUser, OpenChat, SendTell (each shown when its `ChatView` flag bit is set; **GUESS**: all three)
            vec![
                MenuItem::entry(id(OP_IGNORE, 0), &self.label("IgnoreUser")),
                MenuItem::entry(id(OP_OPEN_CHAT, 0), &self.label("OpenChat")),
                MenuItem::entry(id(OP_SEND_TELL, 0), &self.label("SendTell")),
            ]
        } else {
            let c = &self.wins[d].cfg;
            let talk: Vec<MenuItem> = groups.iter().enumerate().map(|(i, g)| MenuItem::check(id(OP_TALK_TO, i as u32), &self.group_name(*g), c.output_group == *g)).collect();
            let subscribe: Vec<MenuItem> = groups.iter().enumerate().map(|(i, g)| MenuItem::check(id(OP_SUBSCRIBE, i as u32), &self.group_name(*g), c.shows(*g))).collect();
            // order: window submenu (`FUN_10097289` inserts it first), separator, then `FUN_100ab4dc`'s entries (**GUESS**: the relative order of the two inserters)
            vec![
                MenuItem::submenu(&self.label("ChatWindowMenu_Visual"), self.window_menu(d)),
                MenuItem::separator(),
                MenuItem::submenu(&self.label("ChatWindowMenu_TalkToChannel"), talk),
                MenuItem::submenu(&self.label("ChatWindowMenu_ChannelSubscribeMenu"), subscribe),
                MenuItem::check(id(OP_AUTOSUB, 0), &self.label("ChatWindowMenu_AutoSubscribeChannels"), c.autosubscribe),
            ]
        };
        self.menu = Some(Ctx { frame: fi, groups, user });
        gui.open_menu(at, (self.screen.0 as i32, self.screen.1 as i32), items);
    }

    /// Test hook: chooses the entry `op`/`arg` of the menu of frame `fi` as if clicked.
    #[cfg(test)]
    pub(super) fn test_pick(&mut self, gui: &mut Gui, fi: usize, op: u32, arg: u32) -> Vec<WinOut> {
        self.menu = Some(Ctx { frame: fi, groups: self.menu_groups(), user: None });
        self.menu_picked(gui, id(op, arg))
    }

    pub(super) fn menu_picked(&mut self, gui: &mut Gui, mid: u32) -> Vec<WinOut> {
        let Some(ctx) = self.menu.take() else { return vec![] };
        let (op, arg) = (mid >> 16, mid & 0xffff);
        let fi = ctx.frame;
        if fi >= self.frames.len() {
            return vec![];
        }
        let d = self.sel_doc(fi);
        self.dirty = true;
        match op {
            OP_MODE => {
                let mode = arg as i32;
                if self.wins[d].cfg.visual_mode != mode {
                    // A borderless document cannot share a tab strip; reuse the existing tear-out path before changing its style.
                    if mode != 0 && self.frames[fi].docs.len() > 1 {
                        self.tab_dropped(gui, self.frames[fi].id, self.frames[fi].sel, None);
                    }
                    let fi = self.wins[d].frame;
                    self.wins[d].cfg.visual_mode = mode;
                    // `FUN_10096ec5` -> `Window::SetStyle`: the frame keeps its outer rectangle, the client shrinks / grows by the border
                    self.rebuild_frame(gui, fi);
                    self.sync_frame_cfg(fi);
                }
            }
            OP_TIMESTAMPS => self.wins[d].cfg.show_timestamps ^= true,
            OP_NO_INPUT => {
                self.wins[d].cfg.textinput ^= true;
                self.rebuild_frame(gui, fi);
            }
            OP_HIDE_INPUT => {
                let w = &mut self.wins[d];
                w.cfg.hide_input_when_inactive ^= true;
                if w.cfg.textinput {
                    gui.set_visible(w.id, &format!("input_border_{}", w.n), !(w.cfg.hide_input_when_inactive && !w.active));
                }
            }
            OP_FADE => self.wins[d].cfg.fading ^= true,
            OP_BEHIND | OP_ON_TOP => {
                let c = &mut self.wins[d].cfg;
                if op == OP_BEHIND {
                    c.backmost ^= true;
                } else {
                    c.frontmost ^= true;
                }
                // **GUESS**: frontmost wins if both are set
                let layer = if c.frontmost { 1 } else if c.backmost { -1 } else { 0 };
                gui.set_window_layer(self.frames[fi].id, layer);
            }
            OP_AUTOSUB => {
                // flipping the sign convention of `selected_group_ids` keeps what the window shows now (docs/chat/gui.md §4)
                let known = self.menu_groups();
                let c = &mut self.wins[d].cfg;
                let shown: Vec<bool> = known.iter().map(|g| c.shows(*g)).collect();
                c.autosubscribe = !c.autosubscribe;
                c.set = known.iter().zip(shown).filter(|(_, s)| *s != c.autosubscribe).map(|(g, _)| *g).collect();
            }
            OP_TALK_TO => {
                if let Some(g) = ctx.groups.get(arg as usize) {
                    self.wins[d].cfg.output_group = *g;
                }
            }
            OP_SUBSCRIBE => {
                if let Some(g) = ctx.groups.get(arg as usize) {
                    let (name, on) = (self.wins[d].cfg.window_name.clone(), !self.wins[d].cfg.shows(*g));
                    self.subscribe_group(&name, *g, on);
                }
            }
            OP_IGNORE | OP_OPEN_CHAT | OP_SEND_TELL => {
                self.dirty = false;
                if let Some(u) = ctx.user {
                    return vec![if op == OP_IGNORE { WinOut::IgnoreUser(u) } else { WinOut::OpenTell(u) }];
                }
            }
            _ => {}
        }
        vec![]
    }

    /// A transparency slider moved (`FUN_10099801` / `FUN_100997be`): the value is the window's `window_transparency_*`; it applies at once.
    pub(super) fn menu_slider(&mut self, gui: &mut Gui, mid: u32, value: f32) {
        let Some(ctx) = &self.menu else { return };
        let fi = ctx.frame;
        if fi >= self.frames.len() {
            return;
        }
        let d = self.sel_doc(fi);
        let w = &mut self.wins[d];
        match mid >> 16 {
            OP_SLIDER_INACTIVE => w.cfg.alpha_inactive = value,
            OP_SLIDER_ACTIVE => w.cfg.alpha_active = value,
            _ => return,
        }
        w.alpha = alpha_of(&w.cfg, w.active);
        w.target = w.alpha;
        gui.set_window_alpha(w.id, w.alpha);
        self.dirty = true;
    }
}
