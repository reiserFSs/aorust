//! Dockable-view moves (`RollupController` 0x10048d3f, DockWindow 0x1003bdd4).
use super::*;
use crate::play::hud_wincfg::DockState;
use ao_gui::MouseButton;

pub(super) struct DockGroup {
    name: String,
    keys: Vec<String>,
    selected: i64,
    frame: Option<(i32, i32, u32, u32)>,
    pin: bool,
}

pub(super) struct PageDrag {
    key: String,
    start: (f32, f32),
    moved: bool,
}

impl Rollup {
    pub fn contains(&self, window: WindowId) -> bool {
        self.pages.iter().any(|p| p.window == window)
    }

    /// The original TradeView has no persistent identity and starts in RollupArea.
    pub fn register_transient_window(&mut self, gui: &mut Gui, window: WindowId) -> anyhow::Result<()> {
        if self.contains(window) { return Ok(()); }
        self.refresh_groups(gui);
        self.remove_key("");
        self.config.retain(|c| !c.key.is_empty());
        self.register_window(gui, "", window)?;
        self.restore_docks(gui, &[]);
        // RollupController's AddView calls its reveal virtual (0x10048996).
        let (_, top, bottom) = self.area();
        let y = gui.window_pos(window).1;
        let end = y + gui.window_size(window).1 as i32 - 1;
        let delta = if y < top { y - top } else { (end - bottom).max(0) };
        if delta != 0 {
            self.scroll += delta as f32;
            self.dirty = true;
            self.layout(gui);
        }
        Ok(())
    }
    /// Wrap the owner's existing content; all handles and callbacks stay valid.
    pub fn register_window(&mut self, gui: &mut Gui, key: &str, window: WindowId) -> anyhow::Result<()> {
        if self.contains(window) { return Ok(()); }
        let rollup_default = key.is_empty() || self.config.iter().any(|c| c.key == key);
        let title = gui.window_tabs(window).0.into_iter().next().unwrap_or_else(|| key.to_owned());
        let frame = gui.window_outer_frame(window).unwrap_or((0, 0, AREA_W, 100));
        let height = gui.window_size(window).1 as f32;
        let button = |name: &str, art: &str| {
            let (w,h) = gui.gfx_id(art).map_or((15,15), |g| gui.gfx().size(GfxId(g)));
            format!("<CanvasView name=\"{name}\" min_size=\"Point({w},{h})\" max_size=\"Point({w},{h})\" layout_borders=\"Rect(2,2,2,2)\"/>")
        };
        let src = format!("<root><View view_layout=\"vertical\" h_alignment=\"left\"><View name=\"rollup_header\" view_layout=\"stacked\" min_size=\"Point(-1,19)\" max_size=\"Point(16000,19)\"><CanvasView name=\"header_bg\"/><View view_layout=\"horizontal\">{}<TextView name=\"title\" value=\"{}\" layout_borders=\"Rect(8,2,0,2)\"/><HLayoutSpacer/>{}{}</View></View><View name=\"body\" view_layout=\"stacked\" min_size=\"Point(0,{height})\" max_size=\"Point(16000,16000)\"><CanvasView name=\"docked_body_bg\"/></View></View></root>", button("icon",ICON),esc(&title),button("arrow",COLLAPSE),button("close",CLOSE));
        gui.wrap_window_xml(window, &src, "body")?;
        for (name, art) in [("icon",ICON),("arrow",COLLAPSE),("close",CLOSE)] {
            if let Some(id) = gui.gfx_id(art) {
                let (w,h) = gui.gfx().size(GfxId(id));
                gui.set_canvas(window,name,vec![CanvasItem::ImageTint { id:GfxId(id), src:[0.0,0.0,w as f32,h as f32], dst:[0.0,0.0,w as f32,h as f32],color:0x1000000,alpha:0.85 }]);
            }
        }
        gui.set_canvas(window,"header_bg",vec![CanvasItem::Solid { dst:[0.0,0.0,AREA_W as f32,19.0],color:0,alpha:0.85 }]);
        gui.show_collapsing(window, "rollup_header", false);
        // RollupPage_c owns the stretched 0x198 surface (GUI 0x10049389);
        // FriendListView itself deliberately has no background.
        gui.set_visible(window, "docked_body_bg", false);
        gui.relayout_window(window);
        if let Some(g) = self.groups.iter_mut().find(|g| g.keys.iter().any(|k| k == key)) {
            if g.frame.is_none() { g.frame = Some(frame); }
        }
        self.pages.push(Page { key: key.to_owned(), window, expanded: true, title, docked: false, wrapped: true });
        if !self.config.iter().any(|c| c.key == key) {
            self.config.push(PageConfig { key: key.to_owned(), height, expanded: true });
        }
        if !rollup_default && !self.groups.iter().any(|g| g.keys.iter().any(|k| k == key)) {
            let name = self.next_dock_name();
            self.groups.push(DockGroup { name, keys: vec![key.to_owned()], selected: 0, frame:Some(frame), pin: gui.window_pinned(window) });
        }
        self.sync_groups(gui);
        Ok(())
    }

    fn next_dock_name(&self) -> String {
        (0..).map(|i| format!("DockArea{i}")).find(|n| !self.groups.iter().any(|g| g.name == *n)).unwrap()
    }

    fn group_of(&self, window: WindowId) -> Option<usize> {
        let key = &self.pages.iter().find(|p| p.window == window)?.key;
        self.groups.iter().position(|g| g.keys.contains(key))
    }

    fn remove_key(&mut self, key: &str) {
        for g in &mut self.groups {
            if let Some(at) = g.keys.iter().position(|k| k == key) {
                g.keys.remove(at);
                if (at as i64) < g.selected { g.selected -= 1; }
                g.selected = g.selected.min(g.keys.len().saturating_sub(1) as i64);
            }
        }
    }

    fn free_page(&mut self, gui: &mut Gui, key: &str, x: i32, y: i32) {
        self.remove_key(key);
        let Some(at) = self.pages.iter().position(|p| p.key == key) else { return };
        self.pages[at].docked = false;
        gui.set_window_visible(self.pages[at].window, true);
        let p = &self.pages[at];
        gui.show_collapsing(p.window, "rollup_header", false);
        gui.show_collapsing(p.window, "body", true);
        gui.set_visible(p.window, "docked_body_bg", false);
        gui.set_window_dock_frame(p.window, Some(&p.title));
        self.resize_page(gui, p);
        gui.set_window_pos(p.window, (x, y));
        let frame = gui.window_outer_frame(p.window).unwrap();
        let name = self.next_dock_name();
        self.groups.push(DockGroup { name, keys: vec![key.to_owned()], selected: 0, frame:Some(frame), pin: false });
        self.dirty = true;
        self.layout(gui);
        self.sync_groups(gui);
    }

    fn dock_page(&mut self, gui: &mut Gui, key: &str, y: i32) {
        self.remove_key(key);
        let Some(at) = self.pages.iter().position(|p| p.key == key) else { return };
        let mut p = self.pages.remove(at);
        p.docked = true;
        gui.set_window_dock_frame(p.window, None);
        gui.show_collapsing(p.window, "rollup_header", true);
        gui.show_collapsing(p.window, "body", p.expanded);
        gui.set_visible(p.window, "docked_body_bg", true);
        self.resize_page(gui, &p);
        let at = self.pages.iter().position(|p| p.docked && gui.window_pos(p.window).1 + gui.window_size(p.window).1 as i32 / 2 > y).unwrap_or(self.pages.len());
        self.pages.insert(at, p);
        let keys: Vec<_> = self.pages.iter().filter(|p| p.docked).map(|p| p.key.clone()).collect();
        self.config.sort_by_key(|c| keys.iter().position(|k| *k == c.key).unwrap_or(usize::MAX));
        self.dirty = true;
        self.layout(gui);
        self.sync_groups(gui);
    }

    pub fn dock_event(&mut self, gui: &mut Gui, ev: &Event) -> bool {
        match *ev {
            Event::TabSelected { window, index } => {
                let Some(g) = self.group_of(window) else { return false };
                self.groups[g].selected = self.groups[g].keys.iter().enumerate()
                    .filter(|(_,k)| self.pages.iter().any(|p| p.key == **k))
                    .nth(index).map_or(0,|(i,_)| i as i64);
                self.sync_groups(gui);
                self.dirty = true;
                true
            }
            Event::WindowFrame { window } | Event::FramePin { window, .. } => {
                let Some(g) = self.group_of(window) else { return false };
                if let Some(f) = gui.window_outer_frame(window) { self.groups[g].frame = Some(f); }
                self.groups[g].pin = gui.window_pinned(window);
                self.sync_groups(gui);
                self.dirty = true;
                false // inventory and list owners still reflow their content
            }
            Event::TabDropped { window, tab, x, y, target } => {
                let Some(g) = self.group_of(window) else { return false };
                let Some(key) = self.groups[g].keys.iter()
                    .filter(|k| self.pages.iter().any(|p| p.key == **k)).nth(tab).cloned() else { return true };
                let (left, top, bottom) = self.area();
                if x >= left && y >= top && y < bottom {
                    self.dock_page(gui, &key, y);
                } else if let Some((target, index)) = target {
                    if let Some(t) = self.group_of(target) {
                        let name = self.groups[t].name.clone();
                        let old = self.groups[t].keys.iter().position(|k| *k == key);
                        let index = index.saturating_sub(usize::from(old.is_some_and(|old| old < index)));
                        self.remove_key(&key);
                        let t = self.groups.iter().position(|g| g.name == name).unwrap();
                        let at = index.min(self.groups[t].keys.len());
                        self.groups[t].keys.insert(at, key);
                        self.groups[t].selected = at as i64;
                        self.dirty = true;
                        self.sync_groups(gui);
                    } else { self.free_page(gui, &key, x, y); }
                } else if self.groups[g].keys.len() > 1 {
                    self.free_page(gui, &key, x, y);
                }
                true
            }
            _ => false,
        }
    }

    pub(super) fn drag_input(&mut self, gui: &mut Gui, ev: &InputEvent) {
        match *ev {
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => {
                let (left, top, bottom) = self.area();
                if x < left as f32 || x >= self.screen.0 as f32 || y < top as f32 || y >= (bottom + 1) as f32 {
                    self.drag = None;
                    return;
                }
                let p = self.pages.iter().rev().find(|p| p.docked && gui.view_rect(p.window, "rollup_header").is_some_and(|r| r.contains(ao_gui::Point::new(x,y))) && !["icon", "arrow", "close"].iter().any(|n| gui.view_rect(p.window, n).is_some_and(|r| r.contains(ao_gui::Point::new(x,y)))));
                self.drag = p.map(|p| PageDrag { key: p.key.clone(), start: (x,y), moved: false });
            }
            InputEvent::MouseMove { x, y } => {
                let Some(d) = &mut self.drag else { return };
                if (x-d.start.0).abs() + (y-d.start.1).abs() > 4.0 { d.moved = true; }
            }
            InputEvent::MouseUp { x, y, button: MouseButton::Left } => {
                let Some(d) = self.drag.take().filter(|d| d.moved) else { return };
                let (left, top, bottom) = self.area();
                if x >= left as f32 && y >= top as f32 && y < bottom as f32 { self.dock_page(gui, &d.key, y as i32); }
                else {
                    let target = gui.tab_drop_target(x,y);
                    self.free_page(gui, &d.key, x as i32, y as i32);
                    if target.is_some() {
                        if let Some(p) = self.pages.iter().find(|p| p.key == d.key) {
                            self.dock_event(gui,&Event::TabDropped { window:p.window,tab:0,x:x as i32,y:y as i32,target });
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn resize_groups(&mut self, gui: &mut Gui, screen: (u32,u32)) {
        for g in &mut self.groups {
            let Some((mut x,mut y,w,h)) = g.frame else { continue };
            if x + w as i32 == self.screen.0 as i32 { x = screen.0 as i32 - w as i32; }
            if y + h as i32 == self.screen.1 as i32 { y = screen.1 as i32 - h as i32; }
            let (x,y) = crate::play::hud_wincfg::inside_screen((x,y,w,h),screen);
            if g.frame != Some((x,y,w,h)) { self.dirty = true; }
            g.frame = Some((x,y,w,h));
        }
        self.sync_groups(gui);
    }

    pub fn refresh_groups(&mut self, gui: &mut Gui) {
        let old = self.pages.len();
        self.pages.retain(|p| gui.window_outer_frame(p.window).is_some());
        for g in &mut self.groups {
            let selected = g.keys.get(g.selected.max(0) as usize);
            let p = self.pages.iter().find(|p| Some(&p.key) == selected)
                .or_else(|| g.keys.iter().find_map(|k| self.pages.iter().find(|p| p.key == *k)));
            if let Some(p) = p {
                let pin = gui.window_pinned(p.window);
                if pin != g.pin {
                    g.pin = pin;
                    self.dirty = true;
                }
                if let Some(frame) = gui.window_outer_frame(p.window) {
                    if Some(frame) != g.frame {
                        g.frame = Some(frame);
                        self.dirty = true;
                    }
                }
            }
        }
        if self.pages.len() != old {
            self.sync_groups(gui);
        }
    }

    pub(super) fn sync_groups(&self, gui: &mut Gui) {
        for g in &self.groups {
            let open: Vec<_> = g.keys.iter().filter_map(|k| self.pages.iter().find(|p| p.key == *k)).collect();
            let titles: Vec<_> = open.iter().map(|p| p.title.clone()).collect();
            let selected_key = g.keys.get(g.selected.max(0) as usize);
            let selected = open.iter().position(|p| Some(&p.key) == selected_key).unwrap_or(0);
            for (i, p) in open.iter().enumerate() {
                gui.set_window_dock_frame(p.window, Some(&p.title));
                gui.set_window_tabs(p.window, &titles, selected);
                if let Some(frame) = g.frame { gui.set_window_outer_frame(p.window, frame); }
                gui.set_window_pinned(p.window, g.pin);
                gui.set_window_visible(p.window, i == selected);
            }
        }
    }

    pub fn dock_states(&self, gui: &Gui) -> Vec<DockState> {
        let configs: Vec<_> = self.config.iter().filter(|c| !c.key.is_empty() && !self.groups.iter().any(|g| g.keys.contains(&c.key))).collect();
        let node = |c: &PageConfig| ao_gui::xml::parse(&format!("<Archive code=\"0\"><Float name=\"page_height\" value=\"{}\"/><Bool name=\"is_page_expanded\" value=\"{}\"/></Archive>", c.height,c.expanded)).unwrap();
        let mut states = vec![DockState { name: "RollupArea".into(), identities: configs.iter().map(|c| c.key.clone()).collect(), selected: 0, frame: None, pin: false, nodes: configs.iter().map(|c| node(c)).collect(), scroll: Some(self.scroll) }];
        for g in &self.groups {
            let frame = g.keys.get(g.selected.max(0) as usize).and_then(|k| self.pages.iter().find(|p| p.key == *k)).and_then(|p| gui.window_outer_frame(p.window)).or(g.frame);
            let pin = g.keys.get(g.selected.max(0) as usize).and_then(|k| self.pages.iter().find(|p| p.key == *k)).map_or(g.pin,|p| gui.window_pinned(p.window));
            let identities: Vec<_> = g.keys.iter().filter(|k| !k.is_empty()).cloned().collect();
            let selected = if g.selected < 0 { g.selected } else {
                g.keys.get(g.selected as usize).and_then(|k| identities.iter().position(|i| i == k)).map_or(-1, |i| i as i64)
            };
            states.push(DockState { name:g.name.clone(), identities, selected, frame, pin, nodes:vec![], scroll:None });
        }
        states
    }

    pub fn restore_docks(&mut self, gui: &mut Gui, states: &[DockState]) {
        for s in states {
            if s.name == "RollupArea" {
                self.scroll = s.scroll.unwrap_or(0.0);
                for (i,key) in s.identities.iter().enumerate() {
                    if let Some(c) = self.config.iter_mut().find(|c| c.key == *key) {
                        if let Some(n) = s.nodes.get(i) {
                            for a in &n.children {
                                if a.attr("name") == Some("page_height") { c.height = a.attr("value").and_then(|v| v.parse().ok()).unwrap_or(c.height); }
                                if a.attr("name") == Some("is_page_expanded") { c.expanded = a.attr("value") == Some("true"); }
                            }
                        }
                    }
                }
                self.config.sort_by_key(|c| s.identities.iter().position(|k| *k == c.key).unwrap_or(usize::MAX));
            } else {
                let frame = s.frame.map(|frame| {
                    let (x,y) = crate::play::hud_wincfg::inside_screen(frame,self.screen);
                    (x,y,frame.2,frame.3)
                }).or_else(|| self.groups.iter().find(|g| g.name == s.name).and_then(|g| g.frame));
                self.groups.retain(|g| g.name != s.name && !g.keys.iter().any(|k| s.identities.contains(k)));
                self.groups.push(DockGroup { name:s.name.clone(), keys:s.identities.clone(), selected:s.selected, frame, pin:s.pin });
            }
        }
        for i in 0..self.pages.len() {
            self.pages[i].docked = !self.groups.iter().any(|g| g.keys.contains(&self.pages[i].key));
            let p = &mut self.pages[i];
            if p.docked {
                gui.set_window_dock_frame(p.window, None);
                gui.show_collapsing(p.window, "rollup_header", true);
                p.expanded = self.config.iter().find(|c| c.key == p.key).is_none_or(|c| c.expanded);
                gui.show_collapsing(p.window, "body", p.expanded);
                gui.set_visible(p.window, "docked_body_bg", true);
                self.resize_page(gui, &self.pages[i]);
            } else {
                gui.show_collapsing(p.window, "rollup_header", false);
                gui.show_collapsing(p.window, "body", true);
                gui.set_visible(p.window, "docked_body_bg", false);
            }
        }
        let rank = |k:&str| self.config.iter().position(|c| c.key == k).unwrap_or(usize::MAX);
        self.pages.sort_by_key(|p| rank(&p.key));
        self.layout(gui);
        self.sync_groups(gui);
        if !states.is_empty() { self.dirty = false; }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn friends_wrapper_paints_native_body_only_while_docked() {
        let dir = ao_gui::client_dir();
        let Ok(mut gui) = Gui::new(&dir, None) else { return };
        let mut rollup = Rollup::new(&dir, (1280, 800));
        let window = gui.open_tabbed_window_xml("Friends", "Friends", "<root><StringListView name=\"list\"/></root>", (100, 100), WindowSize::Fixed(169, 215)).unwrap();
        rollup.register_window(&mut gui, "friends_window", window).unwrap();
        for _ in 0..2 {
            rollup.dock_page(&mut gui, "friends_window", AREA_TOP);
            let bg = gui.view_rect(window, "docked_body_bg").unwrap();
            let draw = gui.frame(0.0);
            assert!(draw.cmds.iter().any(|cmd| matches!(cmd, ao_gui::DrawCmd::Gfx { id: GfxId(0x198), dst, alpha, .. }
                if *dst == [bg.l, bg.t, bg.r + 1.0, bg.b + 1.0] && *alpha == 1.0)));
            assert!(draw.cmds.iter().any(|cmd| matches!(cmd, ao_gui::DrawCmd::Solid { dst, color: [64,64,64], .. }
                if *dst == [bg.l, bg.b + 1.0 - BODY_BOTTOM as f32, bg.r + 1.0, bg.b + 1.0])));
            rollup.free_page(&mut gui, "friends_window", 100, 100);
            let free = gui.view_rect(window, "docked_body_bg").unwrap();
            assert!(!gui.frame(0.0).cmds.iter().any(|cmd| matches!(cmd, ao_gui::DrawCmd::Gfx { id: GfxId(0x198), dst, .. }
                if *dst == [free.l, free.t, free.r + 1.0, free.b + 1.0])));
        }
    }

    #[test]
    fn wrapped_dock_toggle_keeps_column_width_and_owner_rows() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut rollup = Rollup::new(&dir, (1280, 800));
        let xml = "<root><View view_layout=\"vertical\"><CanvasView name=\"rows\" min_size=\"Point(0,120)\" max_size=\"Point(16000,120)\"/></View></root>";
        let window = gui.open_tabbed_window_xml("team_test", "Team", xml, (100, 100), WindowSize::Fixed(185, 140)).unwrap();
        rollup.register_window(&mut gui, "team_test", window).unwrap();
        rollup.dock_page(&mut gui, "team_test", AREA_TOP);
        for expanded in [false, true, false, true] {
            let index = rollup.pages.iter().position(|p| p.window == window).unwrap();
            rollup.set_expanded(&mut gui, index, expanded, (15, 15));
            assert_eq!(gui.window_size(window).0, AREA_W);
            if expanded {
                let body = gui.view_rect(window, "body").unwrap();
                let rows = gui.view_rect(window, "rows").unwrap();
                assert!(rows.l >= body.l && rows.r <= body.r, "{rows:?} outside {body:?}");
            }
        }
        rollup.free_page(&mut gui, "team_test", 100, 100);
        assert_eq!(gui.window_size(window).0, AREA_W, "undock must not request the elastic maximum width");
    }

    #[test]
    fn docked_viewport_culls_and_free_page_removes_clip() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut rollup = Rollup::new(&dir, (1280, 400));
        let xml = "<root><CanvasView name=\"content\" min_size=\"Point(185,200)\" max_size=\"Point(185,200)\"/></root>";
        let w = rollup.open_page(&mut gui, "clip_test", "Clip", xml, 200.0).unwrap();
        let (left, top, bottom) = rollup.area();
        let clips: Vec<_> = gui.frame(0.0).cmds.into_iter().filter_map(|c| if let ao_gui::DrawCmd::Clip(c) = c { Some(c) } else { None }).collect();
        assert!(clips.contains(&Some([left, top, 1280, bottom + 1])));
        assert!(!gui.wants_mouse(left as f32 + 50.0, bottom as f32 + 10.0));
        rollup.free_page(&mut gui, "clip_test", left, bottom + 10);
        assert!(gui.window_visible(w));
        assert!(gui.wants_mouse(left as f32 + 50.0, bottom as f32 + 50.0));
        let clips: Vec<_> = gui.frame(0.0).cmds.into_iter().filter_map(|c| if let ao_gui::DrawCmd::Clip(c) = c { Some(c) } else { None }).collect();
        assert!(!clips.contains(&Some([left, top, 1280, bottom + 1])));
        rollup.dock_page(&mut gui, "clip_test", top);
        let other = rollup.open_page(&mut gui, "clip_other", "Other", xml, 200.0).unwrap();
        rollup.scroll = 0.0;
        rollup.layout(&mut gui);
        assert!(!gui.window_visible(other), "second page is below the area but still on screen");
        rollup.scroll = 10000.0;
        rollup.layout(&mut gui);
        assert!(!gui.window_visible(w), "first page is entirely above the area");
        rollup.free_page(&mut gui, "clip_test", 100, 100);
        assert!(gui.window_visible(w), "freeing a culled page restores visibility");
    }

    #[test]
    fn user_config_can_omit_a_registered_wrapped_page() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir, None).unwrap();
        let mut rollup = Rollup::new(&dir, (1280, 800));
        let window = gui.open_tabbed_window_xml("omitted", "Omitted", "<root><View/></root>", (100, 100), WindowSize::Fixed(185, 127)).unwrap();
        rollup.register_window(&mut gui, "omitted_view", window).unwrap();
        let src = r#"<Archive><Archive name="dock_config"><Array name="dock_node_configs"><Archive><Float name="page_height" value="80"/><Bool name="is_page_expanded" value="true"/></Archive></Array><Array name="docked_view_identities"><String value="other_view"/></Array></Archive></Archive>"#;
        rollup.load_user(&mut gui, src);
        assert!(!rollup.config.iter().any(|c| c.key == "omitted_view"));
        gui.resize_window(window, WindowSize::Fixed(260, 210));
        let height = gui.window_size(window).1;
        rollup.resize_page(&mut gui, &rollup.pages[0]);
        assert_eq!(gui.window_size(window).1, height);
        gui.show_collapsing(window, "rollup_header", true);
        let docked_height = gui.window_size(window).1;
        rollup.resize_page(&mut gui, &rollup.pages[0]);
        assert_eq!(gui.window_size(window).1, docked_height, "missing page config must not count the visible header twice");
        rollup.free_page(&mut gui, "omitted_view", 320, 240);
        assert_eq!(gui.window_size(window).1, height);
        let (x, y, ..) = gui.window_outer_frame(window).unwrap();
        assert_eq!((x, y), (320, 240), "free dock placement uses the outer frame, not its inset client origin");
        gui.resize_window(window, WindowSize::Fixed(300, 250));
        assert_eq!(gui.window_size(window), (300, 250), "fallback sizing retains the elastic free-window body");
    }

    use super::*;

    #[test]
    fn dock_tabs_undock_restore_and_keep_owner_handles() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir,None).unwrap();
        let mut rollup = Rollup::new(&dir,(1280,800));
        let src = "<root><View min_size=\"Point(192,80)\" max_size=\"Point(192,80)\"><CanvasView name=\"owner_content\"/></View></root>";
        let a = rollup.open_page(&mut gui,"a","A",src,80.0).unwrap();
        let b = rollup.open_page(&mut gui,"b","B",src,80.0).unwrap();
        rollup.free_page(&mut gui,"a",100,100);
        rollup.free_page(&mut gui,"b",400,100);
        assert!(rollup.dock_event(&mut gui,&Event::TabDropped { window:a,tab:0,x:400,y:100,target:Some((b,0)) }));
        assert!(gui.window_visible(a));
        assert!(!gui.window_visible(b));
        assert_eq!(gui.window_tabs(a).0,["A","B"]);
        assert!(gui.view_rect(a,"owner_content").is_some());
        assert!(rollup.dock_event(&mut gui,&Event::TabSelected { window:a,index:1 }));
        assert!(gui.window_visible(b));
        assert!(!gui.window_visible(a));
        let saved = rollup.dock_states(&gui);
        rollup.restore_docks(&mut gui,&saved);
        assert!(gui.window_visible(b));
        assert_eq!(gui.window_tabs(b).1,1);
        assert!(rollup.dock_event(&mut gui,&Event::TabDropped { window:b,tab:0,x:300,y:300,target:None }));
        assert!(gui.window_visible(a) && gui.window_visible(b));
        assert_eq!(gui.window_tabs(a).0,["A"]);
        rollup.dock_page(&mut gui,"a",AREA_TOP);
        assert!(!gui.window_tabbed(a));
        assert!(gui.view_rect(a,"owner_content").is_some());
        assert!(rollup.event(&mut gui,&Event::CanvasClick { window:a,view:"owner_content".into(),x:1.0,y:1.0 }).is_none());
        let states = rollup.dock_states(&gui);
        assert!(states.iter().find(|s| s.name=="RollupArea").unwrap().identities.iter().any(|k| k=="a"));
        assert!(states.iter().any(|s| s.name!="RollupArea" && s.identities.is_empty()));
    }

    #[test]
    fn screen_resize_keeps_free_dock_bottom_right_anchor() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir,None).unwrap();
        let mut rollup = Rollup::new(&dir,(1280,800));
        let w = gui.open_tabbed_window_xml("team","Team","<root><View min_size=\"Point(185,127)\"/></root>",(1085,642),WindowSize::Fixed(185,127)).unwrap();
        rollup.register_window(&mut gui,"team_view",w).unwrap();
        rollup.set_screen(&mut gui,(1600,1000));
        assert_eq!(gui.window_outer_frame(w),Some((1405,842,195,158)));
        rollup.set_screen(&mut gui,(800,600));
        assert_eq!(gui.window_outer_frame(w),Some((605,442,195,158)));
        let saved = rollup.dock_states(&gui);
        assert_eq!(saved.iter().find(|s| s.identities.iter().any(|k| k=="team_view")).unwrap().frame,Some((605,442,195,158)));
    }

    #[test]
    fn single_dock_title_drag_and_border_resize_dispatch() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir,None).unwrap();
        let mut rollup = Rollup::new(&dir,(1280,800));
        let w = gui.open_tabbed_window_xml("team","Team","<root><View/></root>",(100,100),WindowSize::Fixed(185,127)).unwrap();
        rollup.register_window(&mut gui,"team_view",w).unwrap();
        for input in [
            InputEvent::MouseDown { x:130.0,y:110.0,button:MouseButton::Left },
            InputEvent::MouseMove { x:150.0,y:130.0 },
            InputEvent::MouseUp { x:150.0,y:130.0,button:MouseButton::Left },
        ] {
            for e in gui.input(input) { rollup.dock_event(&mut gui,&e); }
        }
        assert_eq!(gui.window_outer_frame(w),Some((120,120,195,158)));
        for input in [
            InputEvent::MouseDown { x:314.0,y:277.0,button:MouseButton::Left },
            InputEvent::MouseMove { x:344.0,y:297.0 },
            InputEvent::MouseUp { x:344.0,y:297.0,button:MouseButton::Left },
        ] {
            for e in gui.input(input) { rollup.dock_event(&mut gui,&e); }
        }
        assert_eq!(gui.window_outer_frame(w),Some((120,120,225,178)));
    }

    #[test]
    fn template_dock_names_and_negative_selection_survive_lazy_open() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir,None).unwrap();
        let mut rollup = Rollup::new(&dir,(1280,800));
        let state = |name:&str,key:&str,selected| DockState { name:name.into(),identities:vec![key.into()],selected,frame:None,pin:false,nodes:vec![],scroll:None };
        rollup.restore_docks(&mut gui,&[state("DockArea0","inventory_window",-1),state("DockArea1","team_view",0)]);
        assert_eq!(rollup.next_dock_name(),"DockArea2");
        for key in ["team_view","inventory_window"] {
            let w = gui.open_tabbed_window_xml(key,key,"<root><View/></root>",(100,100),WindowSize::Fixed(185,127)).unwrap();
            rollup.register_window(&mut gui,key,w).unwrap();
        }
        let states = rollup.dock_states(&gui);
        let inventory = states.iter().find(|s| s.name=="DockArea0").unwrap();
        assert_eq!(inventory.identities,["inventory_window"]);
        assert_eq!(inventory.selected,-1);
        assert_eq!(inventory.frame,Some((100,100,195,158)));
        assert_eq!(states.iter().find(|s| s.name=="DockArea1").unwrap().identities,["team_view"]);
    }

    #[test]
    fn transient_trade_docks_without_archiving_an_identity() {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/gui/Default/Graphics.uvgi").exists() { return; }
        let mut gui = Gui::new(&dir,None).unwrap();
        let mut rollup = Rollup::new(&dir,(1280,800));
        rollup.open_page(&mut gui,"prefix","Prefix","<root><View/></root>",500.0).unwrap();
        let xml = "<root><View name=\"owner_content\" min_size=\"Point(185,127)\"/></root>";
        let w = gui.open_tabbed_window_xml("trade","Trade",xml,(100,100),WindowSize::Fixed(185,127)).unwrap();
        rollup.register_transient_window(&mut gui,w).unwrap();
        assert!(rollup.pages.iter().any(|p| p.window == w && p.docked && p.key.is_empty()));
        let (_, top, bottom) = rollup.area();
        let y = gui.window_pos(w).1;
        assert!(rollup.scroll > 0.0 && y >= top && y + gui.window_size(w).1 as i32 - 1 <= bottom);
        assert!(gui.view_rect(w,"owner_content").is_some());
        assert!(matches!(rollup.event(&mut gui,&Event::CanvasClick { window:w,view:"close".into(),x:1.0,y:1.0 }), Some(super::super::RollupEvent::Closed(key)) if key.is_empty()));
        let a = gui.open_tabbed_window_xml("a","A",xml,(300,100),WindowSize::Fixed(185,127)).unwrap();
        rollup.register_window(&mut gui,"a",a).unwrap();
        rollup.free_page(&mut gui,"",200,100);
        gui.resize_window(w, WindowSize::Fixed(260,240));
        let body = gui.view_rect(w,"body").unwrap();
        assert_eq!((body.width() + 1.0, body.height() + 1.0),(260.0,240.0),
            "restoring the page height must not remove the free body resize range");
        rollup.dock_event(&mut gui,&Event::TabDropped { window:w,tab:0,x:300,y:100,target:Some((a,0)) });
        let states = rollup.dock_states(&gui);
        assert!(states.iter().all(|s| s.identities.iter().all(|key| !key.is_empty())));
        let group = states.iter().find(|s| s.identities == ["a"]).unwrap();
        assert_eq!(group.selected,-1, "selected transient tab is not a persisted view");
        let area = states.iter().find(|s| s.name == "RollupArea").unwrap();
        assert_eq!(area.nodes.len(),area.identities.len());
        gui.close_window(w);
        let next = gui.open_tabbed_window_xml("trade","Trade",xml,(100,100),WindowSize::Fixed(185,127)).unwrap();
        rollup.register_transient_window(&mut gui,next).unwrap();
        assert!(rollup.pages.iter().any(|p| p.window == next && p.docked));
        assert!(rollup.groups.iter().all(|g| g.keys.iter().all(|key| !key.is_empty())));
    }
}
