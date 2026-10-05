//! The GUI engine: windows, drawing, input.  See `docs/gui.md` for the RE evidence of every rule.

use anyhow::{anyhow, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::draw::*;
use crate::font::{FontId, FontSystem};
use crate::geom::{Point, Rect};
use crate::gfx::{Atlas, GfxId, GfxSet};
use crate::input::*;
use crate::layout::{self, Env};
use crate::text::{self, Colors};
use crate::view::*;
use crate::xml;

/// `GUIColors.xml` / `GUIConfig_c::GUIConfig_c` 0x1012f342 defaults: Default, Selected, Hover, Text,
/// TextSelected, TextHover.
const DEFAULT_PALETTE: [u32; 6] = [0x80e9f3, 0xffffcc, 0xa5ffdb, 0x99ccaa, 0x99ccaa, 0x99ccaa];
/// `GUIConfig_c` layer alpha: layer 2 (used by buttons) = 0.85.
const BUTTON_ALPHA: f32 = 0.85;

/// How a window is sized when opened.
#[derive(Clone, Copy, Debug)]
pub enum WindowSize {
    /// Preferred (minimum) size of the root view (`View::GetPreferredSize`).
    Preferred,
    /// Pixel size (width, height).
    Fixed(u32, u32),
}

struct Window {
    root: ViewId,
    pos: (i32, i32),
    visible: bool,
    default_button: Option<ViewId>,
}

/// First id handed out by `Gui::add_image` (above every skin id).
pub const EXTRA_BASE: u32 = 0x10000;

pub struct ExtraImage {
    pub rgba: Vec<u8>,
    pub w: u32,
    pub h: u32,
    pub smooth: bool,
}

struct Popup {
    window: WindowId,
    combo: ViewId,
    hover: Option<usize>,
}

pub struct Gui {
    gfx: GfxSet,
    atlas: Atlas,
    fonts: FontSystem,
    colors: Colors,
    palette: [u32; 6],
    views_dir: PathBuf,
    tree: Tree,
    windows: Vec<Option<Window>>,
    glyphs: GlyphAtlas,
    glyph_map: HashMap<(FontId, char), Option<[u16; 4]>>,
    localize: Box<dyn Fn(&str) -> Option<String>>,
    mouse: Point,
    hover: Option<ViewId>,
    pressed: Option<ViewId>,
    focus: Option<ViewId>,
    events: Vec<Event>,
    time: f32,
    caret_epoch: f32,
    popup: Option<Popup>,
    scroll_drag: Option<(ViewId, f32)>,
    /// Roots of `add_view` instances.
    items: std::collections::HashSet<ViewId>,
    extras: Vec<ExtraImage>,
    /// Elements/attributes the engine could not honour while building views.
    pub warnings: Vec<String>,
}

fn px(r: Rect) -> [f32; 4] {
    [r.l, r.t, r.r + 1.0, r.b + 1.0]
}

fn mul(a: [u8; 3], b: [u8; 3]) -> [u8; 3] {
    [(a[0] as u32 * b[0] as u32 / 255) as u8, (a[1] as u32 * b[1] as u32 / 255) as u8, (a[2] as u32 * b[2] as u32 / 255) as u8]
}

fn rgb(c: u32) -> [u8; 3] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

impl Gui {
    /// `client_dir` = the client root (contains `cd_image/`). `localize` resolves `#Key` attribute values
    /// (see `XMLObject_c::GetAttrString`); `None` keeps keys verbatim.
    pub fn new(client_dir: &Path, localize: Option<Box<dyn Fn(&str) -> Option<String>>>) -> Result<Gui> {
        let cd = client_dir.join("cd_image");
        let gui_dir = cd.join("gui/Default");
        let gfx = GfxSet::load(&gui_dir).context("load Graphics.uvgi")?;
        let atlas = Atlas::pack(&gfx, 2048)?;
        let fonts = FontSystem::new(&cd, &gfx, None)?;
        let colors = Colors::load(&gui_dir)?;
        let mut palette = DEFAULT_PALETTE;
        if let Ok(src) = std::fs::read_to_string(gui_dir.join("GUIColors.xml")) {
            if let Ok(root) = xml::parse(&src) {
                for (i, name) in ["Default", "Selected", "Hover", "Text", "TextSelected", "TextHover"].iter().enumerate() {
                    if let Some(c) = root.children.iter().find(|c| c.attr("name") == Some(*name)).and_then(|c| c.attr("color")).and_then(parse_int) {
                        palette[i] = c as u32 & 0xffffff;
                    }
                }
            }
        }
        Ok(Gui {
            gfx,
            atlas,
            fonts,
            colors,
            palette,
            views_dir: gui_dir.join("Views"),
            tree: Tree::default(),
            windows: Vec::new(),
            glyphs: GlyphAtlas::new(),
            glyph_map: HashMap::new(),
            localize: localize.unwrap_or_else(|| Box::new(|_| None)),
            mouse: Point::default(),
            hover: None,
            pressed: None,
            focus: None,
            events: Vec::new(),
            time: 0.0,
            caret_epoch: 0.0,
            popup: None,
            scroll_drag: None,
            items: Default::default(),
            extras: Vec::new(),
            warnings: Vec::new(),
        })
    }

    pub fn gfx(&self) -> &GfxSet {
        &self.gfx
    }
    /// Skin atlas to upload once (`Atlas::pages`, `Atlas::entry`).
    pub fn atlas(&self) -> &Atlas {
        &self.atlas
    }
    /// Glyph coverage atlas; re-upload when `version` changed.
    pub fn glyph_atlas(&self) -> &GlyphAtlas {
        &self.glyphs
    }
    pub fn fonts(&mut self) -> &mut FontSystem {
        &mut self.fonts
    }

    // ------------------------------------------------------------------ windows

    /// Loads `Views/<name>.xml` (first `<View>` under `<root>`) as a window at `pos`.
    pub fn open_window(&mut self, view_name: &str, pos: (i32, i32), size: WindowSize) -> Result<WindowId> {
        let path = self.views_dir.join(format!("{view_name}.xml"));
        let src = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let root_el = xml::parse(&src).with_context(|| format!("parse {}", path.display()))?;
        let view_el = root_el.children.first().ok_or_else(|| anyhow!("{view_name}: empty <root>"))?;
        let mut ctx = BuildCtx { gfx: &self.gfx, localize: &*self.localize, warnings: Vec::new() };
        let root = build(&mut self.tree, &mut ctx, view_el).ok_or_else(|| anyhow!("{view_name}: cannot build root"))?;
        self.warnings.extend(ctx.warnings.into_iter().map(|w| format!("{view_name}: {w}")));
        self.windows.push(Some(Window { root, pos, visible: true, default_button: None }));
        let id = self.windows.len() - 1;
        self.resize_window(id, size);
        Ok(id)
    }

    pub fn close_window(&mut self, w: WindowId) {
        if let Some(slot) = self.windows.get_mut(w) {
            *slot = None;
        }
        if self.focus.is_some_and(|f| self.window_of(f).is_none()) {
            self.focus = None;
        }
    }

    pub fn set_window_pos(&mut self, w: WindowId, pos: (i32, i32)) {
        if let Some(Some(win)) = self.windows.get_mut(w) {
            win.pos = pos;
        }
    }
    pub fn set_window_visible(&mut self, w: WindowId, v: bool) {
        if let Some(Some(win)) = self.windows.get_mut(w) {
            win.visible = v;
        }
    }
    /// Size of the window content in pixels.
    pub fn window_size(&self, w: WindowId) -> (u32, u32) {
        self.windows[w].as_ref().map_or((0, 0), |win| {
            let f = self.tree.views[win.root].frame;
            ((f.width() + 1.0) as u32, (f.height() + 1.0) as u32)
        })
    }

    pub fn resize_window(&mut self, w: WindowId, size: WindowSize) {
        let Some(Some(win)) = self.windows.get(w) else { return };
        let root = win.root;
        let (cw, ch) = match size {
            WindowSize::Fixed(a, b) => (a as f32, b as f32),
            WindowSize::Preferred => {
                let mut env = Env { gfx: &self.gfx, fonts: &mut self.fonts, colors: &self.colors };
                let p = layout::pref(&mut env, &self.tree, root, false);
                (p.x + 1.0, p.y + 1.0)
            }
        };
        self.relayout(root, Rect::new(0.0, 0.0, cw - 1.0, ch - 1.0));
    }

    fn relayout(&mut self, root: ViewId, frame: Rect) {
        let mut env = Env { gfx: &self.gfx, fonts: &mut self.fonts, colors: &self.colors };
        layout::set_frame(&mut env, &mut self.tree, root, frame);
    }

    /// Recomputes the layout of a window keeping its size (call after changing text, visibility, ...).
    pub fn relayout_window(&mut self, w: WindowId) {
        if let Some(Some(win)) = self.windows.get(w) {
            let (root, f) = (win.root, self.tree.views[win.root].frame);
            self.relayout(root, f);
        }
    }

    /// `Window::SetDefaultButton`: Enter activates this button.
    pub fn set_default_button(&mut self, w: WindowId, name: &str) {
        if let Some(v) = self.find(w, name) {
            if let Some(Some(win)) = self.windows.get_mut(w) {
                win.default_button = Some(v);
            }
        }
    }

    /// Instantiates `Views/<xml>.xml` as a child of the named view (the client's `CharSelectItem_c`
    /// does this for `characters_view`) and relayouts the window.
    pub fn add_view(&mut self, w: WindowId, parent: &str, xml_name: &str) -> Result<ViewHandle> {
        let p = self.find(w, parent).ok_or_else(|| anyhow!("no view {parent:?}"))?;
        let path = self.views_dir.join(format!("{xml_name}.xml"));
        let src = std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let root_el = xml::parse(&src)?;
        let view_el = root_el.children.first().ok_or_else(|| anyhow!("{xml_name}: empty <root>"))?;
        let mut ctx = BuildCtx { gfx: &self.gfx, localize: &*self.localize, warnings: Vec::new() };
        let id = build(&mut self.tree, &mut ctx, view_el).ok_or_else(|| anyhow!("{xml_name}: cannot build"))?;
        self.warnings.extend(ctx.warnings.into_iter().map(|w| format!("{xml_name}: {w}")));
        self.tree.append_child(p, id);
        self.items.insert(id);
        self.relayout_window(w);
        Ok(id)
    }

    /// Removes all children of the named view (items added with `add_view` are dropped).
    pub fn remove_children(&mut self, w: WindowId, parent: &str) {
        if let Some(p) = self.find(w, parent) {
            let kids = std::mem::take(&mut self.tree.views[p].children);
            for k in kids {
                self.tree.views[k].parent = None;
                self.items.remove(&k);
            }
            self.hover = None;
            self.pressed = None;
            self.relayout_window(w);
        }
    }

    /// Swaps the layout node of a view (`View::SetLayoutNode`): `vertical` or `horizontal`.
    pub fn set_layout_vertical(&mut self, w: WindowId, name: &str, vertical: bool) {
        if let Some(v) = self.find(w, name) {
            self.tree.views[v].node = if vertical { Node::V } else { Node::H };
            self.relayout_window(w);
        }
    }

    fn find_in(&self, h: ViewHandle, name: &str) -> Option<ViewId> {
        self.tree.find(h, name)
    }
    pub fn set_text_in(&mut self, h: ViewHandle, name: &str, text: &str) {
        if let Some(v) = self.find_in(h, name) {
            self.set_text_view(v, text);
        }
        if let Some(w) = self.window_of(h) {
            self.relayout_window(w);
        }
    }
    pub fn text_in(&self, h: ViewHandle, name: &str) -> String {
        match self.find_in(h, name).and_then(|v| self.editor_of(v)).map(|e| &self.tree.views[e].kind) {
            Some(Kind::Text(t)) => t.text.clone(),
            _ => String::new(),
        }
    }
    pub fn set_visible_in(&mut self, h: ViewHandle, name: &str, visible: bool) {
        if let Some(v) = self.find_in(h, name) {
            self.tree.views[v].visible = visible;
        }
        if let Some(w) = self.window_of(h) {
            self.relayout_window(w);
        }
    }
    /// Removes a named view from its parent (`View::RemoveChild`).
    pub fn remove_view_in(&mut self, h: ViewHandle, name: &str) {
        if let Some(v) = self.find_in(h, name) {
            if let Some(p) = self.tree.views[v].parent.take() {
                self.tree.views[p].children.retain(|c| *c != v);
            }
        }
        if let Some(w) = self.window_of(h) {
            self.relayout_window(w);
        }
    }
    /// Sets the colour (0xRRGGBB) of a `TextView` (`View::SetColor`).
    pub fn set_color_in(&mut self, h: ViewHandle, name: &str, color: u32) {
        if let Some(v) = self.find_in(h, name) {
            self.tree.views[v].color = color;
        }
    }
    /// Makes a `TextButton`/`Button` a toggle button and sets its state (`ButtonBase_c::SetToggleButton`/`SetValue`).
    pub fn set_toggle_in(&mut self, h: ViewHandle, name: &str, toggle: bool, on: bool) {
        if let Some(v) = self.find_in(h, name) {
            if let Kind::TextButton(b) = &mut self.tree.views[v].kind {
                b.toggle = toggle;
                b.toggled = on;
            }
        }
    }
    /// Frame (relative to the window root) of a view inside an instance.
    pub fn frame_in(&self, h: ViewHandle, name: &str) -> Option<Rect> {
        let v = self.find_in(h, name)?;
        let o = self.origin(v);
        let f = self.tree.views[v].frame;
        Some(Rect::new(o.0, o.1, o.0 + f.width(), o.1 + f.height()))
    }
    pub fn set_enabled_in(&mut self, h: ViewHandle, name: &str, enabled: bool) {
        if let Some(v) = self.find_in(h, name) {
            self.tree.views[v].enabled = enabled;
        }
    }

    fn set_text_view(&mut self, v: ViewId, text: &str) {
        match self.tree.views[v].kind.clone() {
            Kind::Button(mut b) => {
                b.label = text.to_string();
                self.tree.views[v].kind = Kind::Button(b);
            }
            Kind::TextButton(mut b) => {
                b.text = text.to_string();
                self.tree.views[v].kind = Kind::TextButton(b);
            }
            _ => {
                if let Some(e) = self.editor_of(v) {
                    if let Kind::Text(t) = &mut self.tree.views[e].kind {
                        t.text = text.to_string();
                        t.caret = text.chars().count();
                        t.anchor = None;
                        t.scroll_x = 0.0;
                    }
                }
            }
        }
    }

    // ------------------------------------------------------------------ lookup / setters

    fn find(&self, w: WindowId, name: &str) -> Option<ViewId> {
        let win = self.windows.get(w)?.as_ref()?;
        self.tree.find(win.root, name)
    }

    fn window_of(&self, mut v: ViewId) -> Option<WindowId> {
        while let Some(p) = self.tree.views[v].parent {
            v = p;
        }
        self.windows.iter().position(|w| w.as_ref().is_some_and(|w| w.root == v))
    }

    fn editor_of(&self, v: ViewId) -> Option<ViewId> {
        match &self.tree.views[v].kind {
            Kind::Text(_) => Some(v),
            Kind::Input | Kind::Combo(_) => self.tree.find(v, "_editor"),
            _ => None,
        }
    }

    /// Named view lookup for the app (`Window::FindView`).
    pub fn has_view(&self, w: WindowId, name: &str) -> bool {
        self.find(w, name).is_some()
    }

    /// Text of a `TextView`, `TextInputView` or `ComboBox` (the edited text).
    pub fn text(&self, w: WindowId, name: &str) -> String {
        match self.find(w, name).and_then(|v| self.editor_of(v)).map(|e| &self.tree.views[e].kind) {
            Some(Kind::Text(t)) => t.text.clone(),
            _ => String::new(),
        }
    }

    /// `TextView_c::SetText` / `TextInputView_c::SetText`; also sets button labels.
    pub fn set_text(&mut self, w: WindowId, name: &str, text: &str) {
        let Some(v) = self.find(w, name) else { return };
        self.set_text_view(v, text);
        self.relayout_window(w);
    }

    pub fn set_visible(&mut self, w: WindowId, name: &str, visible: bool) {
        if let Some(v) = self.find(w, name) {
            self.tree.views[v].visible = visible;
            self.relayout_window(w);
        }
    }
    pub fn set_enabled(&mut self, w: WindowId, name: &str, enabled: bool) {
        if let Some(v) = self.find(w, name) {
            self.tree.views[v].enabled = enabled;
            if let Some(e) = self.editor_of(v) {
                self.tree.views[e].enabled = enabled;
            }
        }
    }
    /// `TextView_c::SetFeatureFlags` on the editor of a `TextInputView` (e.g. `tvf::PASSWORD`).
    pub fn set_feature_flags(&mut self, w: WindowId, name: &str, flags: u32) {
        if let Some(e) = self.find(w, name).and_then(|v| self.editor_of(v)) {
            if let Kind::Text(t) = &mut self.tree.views[e].kind {
                t.tvf |= flags;
            }
        }
    }
    /// `PowerbarView_c::SetValue` (0..1).
    pub fn set_progress(&mut self, w: WindowId, name: &str, value: f32) {
        if let Some(v) = self.find(w, name) {
            if let Kind::PowerBar(p) = &mut self.tree.views[v].kind {
                p.value = value.clamp(0.0, 1.0);
            }
        }
    }
    pub fn focus(&mut self, w: WindowId, name: &str) {
        if let Some(e) = self.find(w, name).and_then(|v| self.editor_of(v)) {
            self.focus = Some(e);
            self.caret_epoch = self.time;
        }
    }
    pub fn focused_view(&self) -> Option<String> {
        self.focus.map(|f| self.outer_name(f))
    }
    /// Replaces the ComboBox entries.
    pub fn combo_set_items(&mut self, w: WindowId, name: &str, items: Vec<String>) {
        if let Some(v) = self.find(w, name) {
            if let Kind::Combo(c) = &mut self.tree.views[v].kind {
                c.items = items;
                c.selected = None;
            }
        }
    }
    pub fn combo_selected(&self, w: WindowId, name: &str) -> Option<usize> {
        match self.find(w, name).map(|v| &self.tree.views[v].kind) {
            Some(Kind::Combo(c)) => c.selected,
            _ => None,
        }
    }
    /// Frame (window pixels, inclusive extents) of a named view; for tests/inspection.
    pub fn view_rect(&self, w: WindowId, name: &str) -> Option<Rect> {
        let v = self.find(w, name)?;
        let o = self.origin(v);
        let f = self.tree.views[v].frame;
        let win = self.windows[w].as_ref()?;
        Some(Rect::new(o.0 + win.pos.0 as f32, o.1 + win.pos.1 as f32, o.0 + win.pos.0 as f32 + f.width(), o.1 + win.pos.1 as f32 + f.height()))
    }
    /// Frame relative to the window root.
    pub fn view_frame(&self, w: WindowId, name: &str) -> Option<Rect> {
        let v = self.find(w, name)?;
        let o = self.origin(v);
        let f = self.tree.views[v].frame;
        Some(Rect::new(o.0, o.1, o.0 + f.width(), o.1 + f.height()))
    }
    /// Names of all named views of a window (inspection).
    pub fn view_names(&self, w: WindowId) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(Some(win)) = self.windows.get(w) {
            fn rec(t: &Tree, v: ViewId, out: &mut Vec<String>) {
                if !t.views[v].name.is_empty() {
                    out.push(t.views[v].name.clone());
                }
                for c in &t.views[v].children {
                    rec(t, *c, out);
                }
            }
            rec(&self.tree, win.root, &mut out);
        }
        out
    }

    /// Top-left of `v` in window-root coordinates (applies scroll offsets of `ScrollChild` parents).
    fn origin(&self, v: ViewId) -> (f32, f32) {
        let (mut x, mut y) = (0.0, 0.0);
        let mut cur = v;
        loop {
            let view = &self.tree.views[cur];
            let Some(p) = view.parent else { break };
            x += view.frame.l;
            y += view.frame.t;
            if let Kind::ScrollChild = self.tree.views[p].kind {
                if let Some(sv) = self.tree.views[p].parent {
                    if let Kind::ScrollView(sd) = &self.tree.views[sv].kind {
                        x -= sd.offset.x;
                        y -= sd.offset.y;
                    }
                }
            }
            cur = p;
        }
        // root frame position is the window position; its own frame.l/t is ignored
        (x, y)
    }

    fn item_of(&self, mut v: ViewId) -> Option<ViewHandle> {
        loop {
            if self.items.contains(&v) {
                return Some(v);
            }
            v = self.tree.views[v].parent?;
        }
    }

    fn outer_name(&self, v: ViewId) -> String {
        // editors are named `_editor`; report the owning TextInputView/ComboBox name
        let view = &self.tree.views[v];
        if view.name == "_editor" {
            let mut cur = v;
            while let Some(p) = self.tree.views[cur].parent {
                if matches!(self.tree.views[p].kind, Kind::Input | Kind::Combo(_)) {
                    return self.tree.views[p].name.clone();
                }
                cur = p;
            }
        }
        view.name.clone()
    }

    // ------------------------------------------------------------------ drawing

    fn map_color(&self, c: u32) -> [u8; 3] {
        if c & 0xff00_0000 != 0 {
            let i = ((c >> 24) as usize).wrapping_sub(1);
            rgb(self.palette.get(i).copied().unwrap_or(self.palette[0]))
        } else {
            rgb(c)
        }
    }

    /// Builds the draw list for all visible windows (later windows on top).
    pub fn frame(&mut self, dt: f32) -> DrawList {
        self.time += dt;
        let mut out = DrawList::default();
        let wins: Vec<(ViewId, (i32, i32))> = self.windows.iter().flatten().filter(|w| w.visible).map(|w| (w.root, w.pos)).collect();
        for (root, pos) in wins {
            self.draw_view(root, pos.0 as f32, pos.1 as f32, [255; 3], 1.0, true, &mut out.cmds);
        }
        self.draw_popup(&mut out.cmds);
        out
    }

    fn push_gfx(&self, out: &mut Vec<DrawCmd>, id: GfxId, dst: Rect, tint: [u8; 3], alpha: f32) {
        let (w, h) = self.gfx.size(id);
        if w == 0 || dst.r < dst.l || dst.b < dst.t {
            return;
        }
        out.push(DrawCmd::Gfx { id, src: [0.0, 0.0, w as f32, h as f32], dst: px(dst), tint, alpha });
    }

    /// `BorderView_c::CreateGfx` 0x10125d3b geometry.
    fn draw_border(&self, out: &mut Vec<DrawCmd>, gfx: &[Option<GfxId>; 9], r: Rect, tint: [u8; 3], alpha: f32) {
        let sz = |g: Option<GfxId>| g.map_or((0.0f32, 0.0f32), |g| {
            let (w, h) = self.gfx.size(g);
            (w as f32, h as f32)
        });
        let [tl, tr, bl, br, left, top, right, bottom, bg] = *gfx;
        let (stl, str_, sbl, sbr) = (sz(tl), sz(tr), sz(bl), sz(br));
        // corner dst rects (inclusive); a missing corner is the degenerate Rect(l,t,l-1,t-1)
        let rtl = Rect::new(r.l, r.t, r.l + stl.0 - 1.0, r.t + stl.1 - 1.0);
        let rtr = Rect::new(r.r - str_.0 + 1.0, r.t, r.r, r.t + str_.1 - 1.0);
        let rbl = Rect::new(r.l, r.b - sbl.1 + 1.0, r.l + sbl.0 - 1.0, r.b);
        let rbr = Rect::new(r.r - sbr.0 + 1.0, r.b - sbr.1 + 1.0, r.r, r.b);
        let (wl, wt, wr, wb) = (sz(left).0, sz(top).1, sz(right).0, sz(bottom).1);
        if let Some(g) = bg {
            self.push_gfx(out, g, r.resize(wl, wt, -wr, -wb), tint, alpha);
        }
        for (g, rect) in [(tl, rtl), (tr, rtr), (bl, rbl), (br, rbr)] {
            if let Some(g) = g {
                self.push_gfx(out, g, rect, tint, alpha);
            }
        }
        if let Some(g) = left {
            self.push_gfx(out, g, Rect::new(rtl.l, rtl.b + 1.0, rtl.l + wl - 1.0, rbl.t - 1.0), tint, alpha);
        }
        if let Some(g) = top {
            self.push_gfx(out, g, Rect::new(rtl.r + 1.0, rtl.t, rtr.l - 1.0, rtl.t + wt - 1.0), tint, alpha);
        }
        if let Some(g) = right {
            self.push_gfx(out, g, Rect::new(rtr.r - wr + 1.0, rtr.b + 1.0, rtr.r, rbr.t - 1.0), tint, alpha);
        }
        if let Some(g) = bottom {
            self.push_gfx(out, g, Rect::new(rbl.r + 1.0, rbl.b - wb + 1.0, rbr.l - 1.0, rbl.b), tint, alpha);
        }
    }

    fn glyph_src(&mut self, font: FontId, ch: char) -> Option<[u16; 4]> {
        if let Some(c) = self.glyph_map.get(&(font, ch)) {
            return *c;
        }
        let g = self.fonts.font(font).glyph(ch).clone();
        let src = if g.width > 0 && g.height > 0 && g.bits.iter().any(|b| *b != 0) {
            let cov: Vec<u8> = g.bits.iter().map(|b| if *b > 1 { *b } else if *b == 1 { 255 } else { 0 }).collect();
            self.glyphs.alloc(g.width as u32, g.height as u32, &cov).map(|(x, y)| [x as u16, y as u16, g.width as u16, g.height as u16])
        } else {
            None
        };
        self.glyph_map.insert((font, ch), src);
        src
    }

    /// Draws `s` with its top-left at (x, y); returns the pen advance. `password` draws `*`.
    fn draw_string(&mut self, out: &mut Vec<DrawCmd>, font: FontId, s: &str, x: i32, y: i32, tint: [u8; 3], alpha: f32, password: bool) -> i32 {
        let mut pen = x;
        for ch in s.chars() {
            let ch = if password { '*' } else { ch };
            if let Some(src) = self.glyph_src(font, ch) {
                out.push(DrawCmd::Glyph { src, dst: [pen, y], tint, alpha });
            }
            pen += self.fonts.font(font).advance(ch);
        }
        pen - x
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_view(&mut self, id: ViewId, ox: f32, oy: f32, parent_tint: [u8; 3], parent_alpha: f32, is_root: bool, out: &mut Vec<DrawCmd>) {
        let v = self.tree.views[id].clone();
        if !v.visible {
            return;
        }
        // origin of this view's top-left in screen px
        let (x0, y0) = if is_root { (ox, oy) } else { (ox + v.frame.l, oy + v.frame.t) };
        let rect = Rect::new(x0, y0, x0 + v.frame.width(), y0 + v.frame.height());
        let tint = mul(parent_tint, self.map_color(v.color));
        let alpha = parent_alpha * v.alpha;
        match &v.kind {
            Kind::Border(b) => {
                let t = mul(parent_tint, self.map_color(b.local_color));
                self.draw_border(out, &b.gfx, rect, t, parent_alpha * v.alpha * b.local_alpha);
            }
            Kind::Button(b) => self.draw_button(out, &v, b, rect, tint, alpha),
            Kind::TextButton(b) => {
                let col = if b.pressed || b.toggled { b.pressed_color } else if b.hover { b.hover_color } else { b.color };
                let t = mul(tint, self.map_color(col));
                self.draw_string(out, b.font, &b.text.clone(), rect.l as i32, rect.t as i32, t, alpha, false);
            }
            Kind::PowerBar(p) => self.draw_powerbar(out, p, rect, alpha),
            Kind::Text(t) => self.draw_text_view(out, id, t, rect, tint, alpha),
            Kind::Bitmap { gfx, index } => {
                if let Some(g) = gfx.get(*index) {
                    let (w, h) = self.gfx.size(*g);
                    // BitmapView draws the bitmap at its natural size, centred (UNRESOLVED: BitmapView_c::_Layout not read)
                    let l = rect.l + ((rect.width() + 1.0 - w as f32) * 0.5).floor();
                    let t = rect.t + ((rect.height() + 1.0 - h as f32) * 0.5).floor();
                    self.push_gfx(out, *g, Rect::new(l, t, l + w as f32 - 1.0, t + h as f32 - 1.0), tint, alpha);
                }
            }
            Kind::CheckBox { label, checked } => {
                let g = if *checked { "GFX_GUI_CHECKBOX_CHECKED" } else { "GFX_GUI_CHECKBOX_UNCHECKED" };
                if let Some(id) = self.gfx.id(g) {
                    self.push_gfx(out, id, Rect::new(rect.l, rect.t, rect.l + 10.0, rect.t + 10.0), tint, alpha);
                }
                self.draw_string(out, FontId::Normal, &label.clone(), rect.l as i32 + 15, rect.t as i32, tint, alpha, false);
            }
            Kind::RadioButton { label, .. } => {
                if let Some(id) = self.gfx.id("GFX_GUI_RADIOBUTTON_UNCHECKED") {
                    self.push_gfx(out, id, Rect::new(rect.l, rect.t, rect.l + 10.0, rect.t + 10.0), tint, alpha);
                }
                self.draw_string(out, FontId::Normal, &label.clone(), rect.l as i32 + 15, rect.t as i32, tint, alpha, false);
            }
            _ => {}
        }
        // children
        match &v.kind {
            Kind::ScrollView(_) => {
                out.push(DrawCmd::Clip(Some([rect.l as i32, rect.t as i32, rect.r as i32 + 1, rect.b as i32 + 1])));
                for c in &v.children {
                    self.draw_view(*c, x0, y0, tint, alpha, false, out);
                }
                out.push(DrawCmd::Clip(None));
                self.draw_scrollbar(out, id, rect, tint, alpha);
            }
            Kind::ScrollChild => {
                let off = v.parent.and_then(|p| match &self.tree.views[p].kind {
                    Kind::ScrollView(sd) => Some(sd.offset),
                    _ => None,
                });
                let off = off.unwrap_or_default();
                for c in &v.children {
                    self.draw_view(*c, x0 - off.x, y0 - off.y, tint, alpha, false, out);
                }
            }
            Kind::Button(_) | Kind::TextButton(_) | Kind::Text(_) | Kind::PowerBar(_) | Kind::Bitmap { .. } => {}
            _ => {
                for c in &v.children {
                    self.draw_view(*c, x0, y0, tint, alpha, false, out);
                }
            }
        }
    }

    fn draw_button(&mut self, out: &mut Vec<DrawCmd>, v: &View, b: &ButtonData, r: Rect, tint: [u8; 3], alpha: f32) {
        let disabled = if v.enabled { [255; 3] } else { [0x90; 3] }; // Button_c::StateChanged: 0xffffff / 0x909090
        let tint = mul(tint, disabled);
        let g = |ids: [u32; 9]| -> [Option<GfxId>; 9] {
            let mut o = [None; 9];
            for (s, i) in o.iter_mut().zip(ids) {
                *s = (i != 0).then_some(GfxId(i));
            }
            o
        };
        // Button_c::Initialize 0x10128994 (raised), StateChanged 0x10128338 (pressed / highlight)
        let raised = g([0x24, 0x26, 0x1f, 0x21, 0x22, 0x25, 0x23, 0x20, 0x1e]);
        let pressed = g([0x35, 0x37, 0x30, 0x32, 0x33, 0x36, 0x34, 0x31, 0x2f]);
        let hover = g([0x2c, 0x2e, 0x27, 0x29, 0x2a, 0x2d, 0x2b, 0x28, 0]);
        let (set, col) = if b.pressed { (pressed, 0x2000000) } else { (raised, 0x1000000) };
        let t = mul(tint, self.map_color(col));
        self.draw_border(out, &set, r, t, alpha * BUTTON_ALPHA);
        if b.hover && v.enabled {
            let t = mul(tint, self.map_color(0x3000000));
            self.draw_border(out, &hover, r, t, alpha * BUTTON_ALPHA);
        }
        if !b.label.is_empty() {
            // label: TextView (font NORMAL) centred in the bounds (Rect::TranslateCenter 0x1000aae4)
            let s = text::string_size(&mut self.fonts, &self.colors, FontId::Normal, &b.label);
            let (lw, lh) = (s.x + 1.0, s.y + 1.0);
            let lx = r.l + ((r.width() + 1.0 - lw) * 0.5).floor();
            let ly = r.t + ((r.height() + 1.0 - lh) * 0.5).floor();
            let lt = mul(tint, self.map_color(col));
            self.draw_string(out, FontId::Normal, &b.label, lx as i32, ly as i32, lt, alpha, false);
        }
    }

    fn draw_powerbar(&mut self, out: &mut Vec<DrawCmd>, p: &PowerBarData, r: Rect, alpha: f32) {
        // PowerbarView_c::Initialize 0x1013ec3f / Recalculate 0x1013e789
        if let Some(bg) = p.bg {
            self.push_gfx(out, bg, r, [255; 3], alpha);
        }
        let Some(full) = p.full else { return };
        let (fw, fh) = self.gfx.size(full);
        let (bw, bh) = p.bg.map_or((fw, fh), |g| self.gfx.size(g));
        let (fwe, fhe) = (fw as f32 - 1.0, fh as f32 - 1.0);
        let ox = (((bw as f32 - 1.0) - fwe) * 0.5).floor();
        let oy = (((bh as f32 - 1.0) - fhe) * 0.5).floor();
        let (mut src, mut dst) = (Rect::new(0.0, 0.0, fwe, fhe), Rect::new(0.0, 0.0, fwe, fhe));
        let v = p.value;
        match p.dir {
            0 => {
                dst.b = (dst.height() * v + dst.t).floor();
                src.b = (src.height() * v + src.t).floor();
            }
            1 => {
                dst.r = (dst.width() * v + dst.l).floor();
                src.r = (src.width() * v + src.l).floor();
            }
            2 => {
                dst.l = ((1.0 - v) * dst.width() + dst.l).floor();
                src.l = ((1.0 - v) * src.width() + src.l).floor();
            }
            _ => {
                dst.t = ((1.0 - v) * dst.height() + dst.t).floor();
                src.t = ((1.0 - v) * src.height() + src.t).floor();
            }
        }
        let dst = dst.translate(r.l + ox, r.t + oy);
        out.push(DrawCmd::Gfx { id: full, src: [src.l, src.t, src.width() + 1.0, src.height() + 1.0], dst: px(dst), tint: [255; 3], alpha });
    }

    fn draw_text_view(&mut self, out: &mut Vec<DrawCmd>, id: ViewId, t: &TextData, r: Rect, tint: [u8; 3], alpha: f32) {
        let editable = t.tvf & tvf::ACCEPT_TXT_INPUT != 0;
        let focused = self.focus == Some(id);
        let wrap = if t.tvf & tvf::WORD_WRAP != 0 { Some(r.width() as i32 + 1) } else { None };
        let layout = text::layout_text(&mut self.fonts, &self.colors, t.font, &t.text, t.tvf, wrap);
        let clip = [r.l as i32, r.t as i32, r.r as i32 + 1, r.b as i32 + 1];
        if editable {
            out.push(DrawCmd::Clip(Some(clip)));
        }
        let pw = t.tvf & tvf::PASSWORD != 0;
        let chars: Vec<char> = t.text.chars().collect();
        let font_h = self.fonts.font(t.font).height;
        // selection background (TextRenderer_c::_RenderString: Clear(rect, 0xc0c0c0))
        let adv = |g: &mut Gui, c: char| g.fonts.font(t.font).advance(if pw { '*' } else { c });
        let origin_x = r.l as i32 - t.scroll_x as i32;
        if editable && focused {
            if let Some(a) = t.anchor.filter(|a| *a != t.caret) {
                let (s, e) = (a.min(t.caret), a.max(t.caret));
                let sx: i32 = chars[..s].iter().map(|c| adv(self, *c)).sum();
                let ex: i32 = chars[..e].iter().map(|c| adv(self, *c)).sum();
                out.push(DrawCmd::Solid { dst: [(origin_x + sx) as f32, r.t, (origin_x + ex) as f32, r.t + font_h as f32], color: [0xc0; 3], alpha });
            }
        }
        for line in &layout.lines {
            let lx = match line.align {
                Align::Right => r.l as i32 + 1 + (r.width() as i32 - line.width),
                Align::Center => r.l as i32 + (r.width() as i32 + 1 - line.width) / 2,
                _ => origin_x,
            };
            let mut pen = lx;
            let y = r.t as i32 + line.y;
            for run in &line.runs {
                let c = run.color.map_or(tint, |c| mul(tint, rgb(c)));
                let c = if run.link { mul(tint, rgb(0x2299ff)) } else { c };
                pen += self.draw_string(out, t.font, &run.text, pen, y, c, alpha, pw);
            }
        }
        if editable && focused {
            // caret: 1px line, blink 0.5s (UNRESOLVED: TextRenderer_c::SlotCursorTimer period/colour)
            let on = ((self.time - self.caret_epoch) * 2.0) as i64 % 2 == 0;
            if on {
                let cx: i32 = chars[..t.caret.min(chars.len())].iter().map(|c| adv(self, *c)).sum();
                out.push(DrawCmd::Solid { dst: [(origin_x + cx) as f32, r.t, (origin_x + cx + 1) as f32, r.t + font_h as f32], color: tint, alpha });
            }
        }
        if editable {
            out.push(DrawCmd::Clip(None));
        }
    }

    fn draw_scrollbar(&mut self, out: &mut Vec<DrawCmd>, sv: ViewId, r: Rect, tint: [u8; 3], alpha: f32) {
        let Some((track, thumb_t, thumb_h)) = self.scroll_geometry(sv) else { return };
        let x = r.r - layout::SCROLLBAR_W;
        let gid = |n: &str| self.gfx.id(n);
        let up = gid("GFX_GUI_SCROLLBAR_GRAY_UP_NORMAL");
        let down = gid("GFX_GUI_SCROLLBAR_GRAY_DOWN_NORMAL");
        let empty = gid("GFX_GUI_SCROLLBAR_GRAY_EMPTY");
        let full = gid("GFX_GUI_SCROLLBAR_GRAY_FULL");
        let w = layout::SCROLLBAR_W;
        if let Some(g) = empty {
            self.push_gfx(out, g, Rect::new(x, r.t + 11.0, x + w, r.b - 11.0), tint, alpha);
        }
        if let Some(g) = up {
            self.push_gfx(out, g, Rect::new(x, r.t, x + w, r.t + 10.0), tint, alpha);
        }
        if let Some(g) = down {
            self.push_gfx(out, g, Rect::new(x, r.b - 10.0, x + w, r.b), tint, alpha);
        }
        if let Some(g) = full {
            let t0 = r.t + 11.0 + thumb_t;
            self.push_gfx(out, g, Rect::new(x, t0, x + w, t0 + thumb_h - 1.0), tint, alpha);
        }
        let _ = track;
    }

    /// (track length px, thumb top offset, thumb height) of the vertical bar, if shown.
    fn scroll_geometry(&self, sv: ViewId) -> Option<(f32, f32, f32)> {
        let view = &self.tree.views[sv];
        let Kind::ScrollView(sd) = &view.kind else { return None };
        let child = *view.children.first()?;
        let inner = *self.tree.views[child].children.first()?;
        let content = self.tree.views[inner].frame.height() + 1.0;
        let vis = view.frame.height() + 1.0;
        let bar_shown = matches!(sd.v_mode, ScrollMode::Always) || (matches!(sd.v_mode, ScrollMode::Auto | ScrollMode::AutoReserve) && content > vis);
        if !bar_shown {
            return None;
        }
        let track = vis - 22.0;
        let prop = (vis / content).min(1.0);
        let thumb_h = (track * prop).max(8.0).min(track);
        let max_off = (content - vis).max(0.0);
        let t = if max_off > 0.0 { sd.offset.y / max_off * (track - thumb_h) } else { 0.0 };
        Some((track, t, thumb_h))
    }

    fn draw_popup(&mut self, out: &mut Vec<DrawCmd>) {
        let Some(p) = &self.popup else { return };
        let (w, combo, hover) = (p.window, p.combo, p.hover);
        let Some(win) = self.windows.get(w).and_then(|w| w.as_ref()) else { return };
        let Kind::Combo(c) = self.tree.views[combo].kind.clone() else { return };
        let o = self.origin(combo);
        let f = self.tree.views[combo].frame;
        let (l, t) = (o.0 + win.pos.0 as f32, o.1 + win.pos.1 as f32 + f.height() + 1.0);
        let ih = self.fonts.font(FontId::Normal).height as f32 + 2.0;
        let rect = Rect::new(l, t, l + f.width(), t + ih * c.items.len() as f32 + 1.0);
        // UNRESOLVED: PopupMenu_c skin; drawn with the button raised border set.
        let raised: [Option<GfxId>; 9] = [0x24, 0x26, 0x1f, 0x21, 0x22, 0x25, 0x23, 0x20, 0x1e].map(|i| Some(GfxId(i)));
        let col = self.map_color(0x1000000);
        self.draw_border(out, &raised, rect, col, 0.95);
        for (i, s) in c.items.iter().enumerate() {
            let y = t + 2.0 + ih * i as f32;
            if hover == Some(i) {
                out.push(DrawCmd::Solid { dst: [l + 2.0, y, rect.r - 1.0, y + ih], color: rgb(self.palette[2]), alpha: 0.5 });
            }
            self.draw_string(out, FontId::Normal, s, l as i32 + 6, y as i32 + 1, [255; 3], 1.0, false);
        }
    }

    /// True when `(x, y)` is over something interactive or any non-root view of a visible window
    /// (a full-screen transparent window such as CharacterSelectionWindow does not count by itself).
    pub fn wants_mouse(&self, x: f32, y: f32) -> bool {
        if self.popup.is_some() || self.hit(x, y).is_some() {
            return true;
        }
        for (_, root, pos) in self.windows_top_down() {
            if self.covers(root, x - pos.0 as f32, y - pos.1 as f32, true, 0.0, 0.0) {
                return true;
            }
        }
        false
    }

    fn covers(&self, id: ViewId, x: f32, y: f32, is_root: bool, ox: f32, oy: f32) -> bool {
        let v = &self.tree.views[id];
        if !v.visible {
            return false;
        }
        let (l, t) = if is_root { (0.0, 0.0) } else { (ox + v.frame.l, oy + v.frame.t) };
        let r = Rect::new(l, t, l + v.frame.width(), t + v.frame.height());
        if !is_root && r.contains(Point::new(x, y)) && matches!(v.kind, Kind::Border(_) | Kind::Button(_) | Kind::TextButton(_) | Kind::Text(_) | Kind::PowerBar(_) | Kind::Input | Kind::Combo(_)) {
            return true;
        }
        v.children.iter().any(|c| self.covers(*c, x, y, false, l, t))
    }

    /// Appends `text` (top-left at `x`,`y`, window pixels) in `font`/`color` (0xRRGGBB) to `list`.
    /// Returns the advance width in pixels.
    pub fn text_cmds(&mut self, font: FontId, text: &str, x: i32, y: i32, color: u32, alpha: f32, list: &mut DrawList) -> i32 {
        self.draw_string(&mut list.cmds, font, text, x, y, rgb(color), alpha, false)
    }
    /// Registers a runtime RGBA image (e.g. a loading screen); draw it with `DrawCmd::Gfx { id, .. }`.
    /// `smooth` = bilinear sampling (`RenderWindow_t::UseFilter(true)`), else nearest.
    pub fn add_image(&mut self, _name: &str, rgba: Vec<u8>, w: u32, h: u32, smooth: bool) -> GfxId {
        let id = GfxId(EXTRA_BASE + self.extras.len() as u32);
        self.extras.push(ExtraImage { rgba, w, h, smooth });
        id
    }
    pub fn extra_images(&self) -> &[ExtraImage] {
        &self.extras
    }
    /// Pixel width of `text` in `font`.
    pub fn text_width(&mut self, font: FontId, text: &str) -> i32 {
        self.fonts.font(font).text_width(text)
    }
    /// Line height of `font` in pixels.
    pub fn font_height(&mut self, font: FontId) -> i32 {
        self.fonts.font(font).height
    }

    // ------------------------------------------------------------------ input

    /// Feeds one input event; returns the UI events it produced.
    pub fn input(&mut self, ev: InputEvent) -> Vec<Event> {
        match ev {
            InputEvent::MouseMove { x, y } => {
                self.mouse = Point::new(x, y);
                self.update_hover();
                if let Some(p) = &mut self.popup {
                    let (w, combo) = (p.window, p.combo);
                    let hv = self.popup_item_at(w, combo, x, y);
                    if let Some(p) = &mut self.popup {
                        p.hover = hv;
                    }
                }
                self.drag_scroll(y);
                self.drag_select(x);
            }
            InputEvent::MouseDown { x, y, button } if button == MouseButton::Left => {
                self.mouse = Point::new(x, y);
                self.update_hover();
                self.mouse_down(x, y);
            }
            InputEvent::MouseUp { x, y, button } if button == MouseButton::Left => {
                self.mouse = Point::new(x, y);
                self.mouse_up();
            }
            InputEvent::Wheel { x, y, dy } => self.wheel(x, y, dy),
            InputEvent::Key { key, pressed: true, mods } => self.key_down(key, mods),
            InputEvent::Text(s) => self.text_input(&s),
            InputEvent::Paste(s) => self.text_input(&s),
            _ => {}
        }
        std::mem::take(&mut self.events)
    }

    fn windows_top_down(&self) -> Vec<(WindowId, ViewId, (i32, i32))> {
        self.windows.iter().enumerate().rev().filter_map(|(i, w)| w.as_ref().filter(|w| w.visible).map(|w| (i, w.root, w.pos))).collect()
    }

    /// Topmost interactive view under the mouse, with its window.
    fn hit(&self, x: f32, y: f32) -> Option<(WindowId, ViewId)> {
        for (wid, root, pos) in self.windows_top_down() {
            let (lx, ly) = (x - pos.0 as f32, y - pos.1 as f32);
            if let Some(v) = self.hit_view(root, lx, ly, true, 0.0, 0.0, None) {
                return Some((wid, v));
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn hit_view(&self, id: ViewId, x: f32, y: f32, is_root: bool, ox: f32, oy: f32, clip: Option<Rect>) -> Option<ViewId> {
        let v = &self.tree.views[id];
        if !v.visible {
            return None;
        }
        let (l, t) = if is_root { (0.0, 0.0) } else { (ox + v.frame.l, oy + v.frame.t) };
        let r = Rect::new(l, t, l + v.frame.width(), t + v.frame.height());
        let clip_r = clip.map_or(r, |c| c.intersect(&r));
        let (mut cox, mut coy) = (l, t);
        let mut cclip = clip;
        match &v.kind {
            Kind::ScrollView(_) => cclip = Some(clip_r),
            Kind::ScrollChild => {
                if let Some(p) = v.parent {
                    if let Kind::ScrollView(sd) = &self.tree.views[p].kind {
                        cox -= sd.offset.x;
                        coy -= sd.offset.y;
                    }
                }
            }
            _ => {}
        }
        for c in v.children.iter().rev() {
            if let Some(h) = self.hit_view(*c, x, y, false, cox, coy, cclip) {
                return Some(h);
            }
        }
        let inside = clip_r.contains(Point::new(x, y)) && !clip_r.is_empty();
        let interactive = match &v.kind {
            Kind::Button(_) | Kind::TextButton(_) | Kind::ScrollView(_) | Kind::CheckBox { .. } | Kind::RadioButton { .. } => true,
            Kind::Text(t) => t.tvf & (tvf::ACCEPT_TXT_INPUT | tvf::ACCEPT_MOUSE_INPUT) != 0,
            _ => false,
        };
        (inside && interactive).then_some(id)
    }

    fn update_hover(&mut self) {
        let h = self.hit(self.mouse.x, self.mouse.y).map(|h| h.1);
        if h != self.hover {
            for (id, state) in [(self.hover, false), (h, true)] {
                if let Some(id) = id {
                    match &mut self.tree.views[id].kind {
                        Kind::Button(b) => b.hover = state,
                        Kind::TextButton(b) => b.hover = state,
                        _ => {}
                    }
                }
            }
            self.hover = h;
        }
        // a pressed button is only "pressed" while the pointer is over it
        if let Some(p) = self.pressed {
            let over = self.hover == Some(p);
            match &mut self.tree.views[p].kind {
                Kind::Button(b) => b.pressed = over,
                Kind::TextButton(b) => b.pressed = over,
                _ => {}
            }
        }
    }

    fn popup_item_at(&self, w: WindowId, combo: ViewId, x: f32, y: f32) -> Option<usize> {
        let win = self.windows.get(w)?.as_ref()?;
        let Kind::Combo(c) = &self.tree.views[combo].kind else { return None };
        let o = self.origin(combo);
        let f = self.tree.views[combo].frame;
        let (l, t) = (o.0 + win.pos.0 as f32, o.1 + win.pos.1 as f32 + f.height() + 1.0);
        let ih = self.fonts_height_normal() + 2.0;
        if x < l || x > l + f.width() || y < t + 2.0 {
            return None;
        }
        let i = ((y - t - 2.0) / ih) as usize;
        (i < c.items.len()).then_some(i)
    }

    fn fonts_height_normal(&self) -> f32 {
        // fonts() needs &mut; Normal's height is fixed once created
        13.0
    }

    fn mouse_down(&mut self, x: f32, y: f32) {
        // an open combo popup captures the click
        if let Some(p) = self.popup.take() {
            if let Some(i) = self.popup_item_at(p.window, p.combo, x, y) {
                let text = match &mut self.tree.views[p.combo].kind {
                    Kind::Combo(c) => {
                        c.selected = Some(i);
                        c.items[i].clone()
                    }
                    _ => String::new(),
                };
                if let Some(e) = self.tree.find(p.combo, "_editor") {
                    if let Kind::Text(t) = &mut self.tree.views[e].kind {
                        t.text = text.clone();
                        t.caret = text.chars().count();
                        t.anchor = None;
                    }
                }
                let name = self.tree.views[p.combo].name.clone();
                self.events.push(Event::ComboChanged { window: p.window, view: name, index: i, text });
            }
            self.set_combo_arrow(p.combo, false);
            return;
        }
        let Some((_, v)) = self.hit(x, y) else {
            self.focus = None;
            return;
        };
        match self.tree.views[v].kind.clone() {
            Kind::Button(_) | Kind::TextButton(_) => {
                self.pressed = Some(v);
                match &mut self.tree.views[v].kind {
                    Kind::Button(b) => b.pressed = true,
                    Kind::TextButton(b) => b.pressed = true,
                    _ => {}
                }
                self.focus = None;
            }
            Kind::Text(t) if t.tvf & tvf::ACCEPT_TXT_INPUT != 0 => {
                // click inside a ComboBox editor opens the popup (ComboBox_c::MouseDown 0x10001ec0)
                let combo = self.combo_of(v);
                self.focus = Some(v);
                self.caret_epoch = self.time;
                let idx = self.char_at(v, x);
                if let Kind::Text(t) = &mut self.tree.views[v].kind {
                    t.caret = idx;
                    t.anchor = Some(idx);
                }
                self.pressed = Some(v);
                if let Some(c) = combo {
                    self.open_combo(c);
                }
            }
            Kind::Bitmap { .. } => {}
            Kind::ScrollView(_) => self.scrollbar_press(v, y),
            _ => {}
        }
    }

    fn combo_of(&self, v: ViewId) -> Option<ViewId> {
        let mut cur = v;
        while let Some(p) = self.tree.views[cur].parent {
            if matches!(self.tree.views[p].kind, Kind::Combo(_)) {
                return Some(p);
            }
            cur = p;
        }
        None
    }

    fn set_combo_arrow(&mut self, combo: ViewId, open: bool) {
        let mut stack = vec![combo];
        while let Some(c) = stack.pop() {
            if let Kind::Bitmap { index, .. } = &mut self.tree.views[c].kind {
                *index = open as usize;
            }
            stack.extend(self.tree.views[c].children.clone());
        }
        if let Kind::Combo(cd) = &mut self.tree.views[combo].kind {
            cd.open = open;
        }
    }

    fn open_combo(&mut self, combo: ViewId) {
        let Some(w) = self.window_of(combo) else { return };
        let n = match &self.tree.views[combo].kind {
            Kind::Combo(c) => c.items.len(),
            _ => 0,
        };
        if n == 0 {
            return;
        }
        self.set_combo_arrow(combo, true);
        self.popup = Some(Popup { window: w, combo, hover: None });
    }

    fn char_at(&mut self, editor: ViewId, x: f32) -> usize {
        let Kind::Text(t) = self.tree.views[editor].kind.clone() else { return 0 };
        let o = self.origin(editor);
        let win_x = self.window_of(editor).and_then(|w| self.windows[w].as_ref()).map_or(0, |w| w.pos.0) as f32;
        let rel = x - win_x - o.0 + t.scroll_x;
        let pw = t.tvf & tvf::PASSWORD != 0;
        let mut acc = 0.0;
        for (i, c) in t.text.chars().enumerate() {
            let a = self.fonts.font(t.font).advance(if pw { '*' } else { c }) as f32;
            if rel < acc + a * 0.5 {
                return i;
            }
            acc += a;
        }
        t.text.chars().count()
    }

    fn drag_select(&mut self, x: f32) {
        let Some(p) = self.pressed else { return };
        if !matches!(self.tree.views[p].kind, Kind::Text(_)) {
            return;
        }
        let idx = self.char_at(p, x);
        if let Kind::Text(t) = &mut self.tree.views[p].kind {
            t.caret = idx;
        }
        self.ensure_caret_visible(p);
    }

    fn mouse_up(&mut self) {
        if let Some(p) = self.pressed.take() {
            let over = self.hit(self.mouse.x, self.mouse.y).map(|h| h.1) == Some(p);
            match &mut self.tree.views[p].kind {
                Kind::Button(b) => b.pressed = false,
                Kind::TextButton(b) => {
                    b.pressed = false;
                    if over && b.toggle {
                        b.toggled = true;
                    }
                }
                _ => {}
            }
            if over && matches!(self.tree.views[p].kind, Kind::Button(_) | Kind::TextButton(_)) && self.tree.views[p].enabled {
                if let Some(w) = self.window_of(p) {
                    let view = self.tree.views[p].name.clone();
                    let item = self.item_of(p);
                    self.events.push(Event::Clicked { window: w, view, item });
                }
            }
        }
        self.scroll_drag = None;
    }

    fn wheel(&mut self, x: f32, y: f32, dy: f32) {
        // nearest ScrollView ancestor of the hit view (or the view itself)
        let mut cur = self.hit(x, y).map(|h| h.1);
        while let Some(c) = cur {
            if let Kind::ScrollView(_) = self.tree.views[c].kind {
                self.scroll_by(c, -dy * 3.0 * 13.0);
                return;
            }
            cur = self.tree.views[c].parent;
        }
    }

    fn scroll_by(&mut self, sv: ViewId, dy: f32) {
        let Some(child) = self.tree.views[sv].children.first().copied() else { return };
        let Some(inner) = self.tree.views[child].children.first().copied() else { return };
        let content = self.tree.views[inner].frame.height() + 1.0;
        let vis = self.tree.views[sv].frame.height() + 1.0;
        let max_off = (content - vis).max(0.0);
        if let Kind::ScrollView(sd) = &mut self.tree.views[sv].kind {
            sd.offset.y = (sd.offset.y + dy).clamp(0.0, max_off);
        }
    }

    fn scrollbar_press(&mut self, sv: ViewId, y: f32) {
        let Some((_, th, thh)) = self.scroll_geometry(sv) else { return };
        let o = self.origin(sv);
        let win_y = self.window_of(sv).and_then(|w| self.windows[w].as_ref()).map_or(0, |w| w.pos.1) as f32;
        let top = o.1 + win_y;
        let rel = y - top;
        let vis = self.tree.views[sv].frame.height() + 1.0;
        if rel < 11.0 {
            self.scroll_by(sv, -13.0);
        } else if rel > vis - 11.0 {
            self.scroll_by(sv, 13.0);
        } else if rel < 11.0 + th {
            self.scroll_by(sv, -vis);
        } else if rel > 11.0 + th + thh {
            self.scroll_by(sv, vis);
        } else {
            self.scroll_drag = Some((sv, rel - th));
        }
    }

    fn drag_scroll(&mut self, y: f32) {
        let Some((sv, grab)) = self.scroll_drag else { return };
        let Some((track, _, thh)) = self.scroll_geometry(sv) else { return };
        let o = self.origin(sv);
        let win_y = self.window_of(sv).and_then(|w| self.windows[w].as_ref()).map_or(0, |w| w.pos.1) as f32;
        let t = (y - (o.1 + win_y) - grab).clamp(0.0, track - thh);
        let child = self.tree.views[sv].children[0];
        let inner = self.tree.views[child].children[0];
        let content = self.tree.views[inner].frame.height() + 1.0;
        let vis = self.tree.views[sv].frame.height() + 1.0;
        let max_off = (content - vis).max(0.0);
        if let Kind::ScrollView(sd) = &mut self.tree.views[sv].kind {
            sd.offset.y = if track - thh > 0.0 { t / (track - thh) * max_off } else { 0.0 };
        }
    }

    // ------------------------------------------------------------------ keyboard

    fn focusables(&self) -> Vec<ViewId> {
        let mut out = Vec::new();
        for (_, root, _) in self.windows_top_down().into_iter().rev() {
            fn rec(g: &Gui, v: ViewId, out: &mut Vec<ViewId>) {
                let view = &g.tree.views[v];
                if !view.visible {
                    return;
                }
                if let Kind::Text(t) = &view.kind {
                    if t.tvf & tvf::ACCEPT_TXT_INPUT != 0 && view.enabled {
                        out.push(v);
                    }
                }
                for c in &view.children {
                    rec(g, *c, out);
                }
            }
            rec(self, root, &mut out);
        }
        out
    }

    fn key_down(&mut self, key: Key, mods: Modifiers) {
        if self.popup.is_some() {
            if key == Key::Escape {
                if let Some(p) = self.popup.take() {
                    self.set_combo_arrow(p.combo, false);
                }
            }
            return;
        }
        self.caret_epoch = self.time;
        match key {
            Key::Tab => {
                let f = self.focusables();
                if f.is_empty() {
                    return;
                }
                let cur = self.focus.and_then(|c| f.iter().position(|x| *x == c));
                let n = f.len();
                let next = match cur {
                    Some(i) => {
                        if mods.shift {
                            (i + n - 1) % n
                        } else {
                            (i + 1) % n
                        }
                    }
                    None => 0,
                };
                self.focus = Some(f[next]);
                if let Kind::Text(t) = &mut self.tree.views[f[next]].kind {
                    // TextRenderer_c::SlotTabPressed selects the whole field when it gains focus by Tab
                    t.anchor = Some(0);
                    t.caret = t.text.chars().count();
                }
            }
            Key::Enter => {
                if let Some(f) = self.focus {
                    if let Some(w) = self.window_of(f) {
                        let view = self.outer_name(f);
                        self.events.push(Event::EnterPressed { window: w, view });
                        if let Some(Some(win)) = self.windows.get(w) {
                            if let Some(b) = win.default_button {
                                let name = self.tree.views[b].name.clone();
                                if self.tree.views[b].enabled && self.tree.views[b].visible {
                                    self.events.push(Event::Clicked { window: w, view: name, item: None });
                                }
                            }
                        }
                    }
                } else if let Some((w, win)) = self.windows.iter().enumerate().rev().find_map(|(i, w)| w.as_ref().filter(|w| w.visible).map(|w| (i, w))) {
                    if let Some(b) = win.default_button {
                        let name = self.tree.views[b].name.clone();
                        self.events.push(Event::Clicked { window: w, view: name, item: None });
                    }
                }
            }
            Key::Escape => {
                if let Some(w) = self.windows.iter().rposition(|w| w.as_ref().is_some_and(|w| w.visible)) {
                    self.events.push(Event::Escape { window: w });
                }
            }
            _ => self.edit_key(key, mods),
        }
    }

    fn edit_key(&mut self, key: Key, mods: Modifiers) {
        let Some(f) = self.focus else { return };
        let Kind::Text(mut t) = self.tree.views[f].kind.clone() else { return };
        let n = t.text.chars().count();
        let mut changed = false;
        let move_to = |t: &mut TextData, pos: usize, mods: Modifiers| {
            if mods.shift {
                t.anchor.get_or_insert(t.caret);
            } else {
                t.anchor = None;
            }
            t.caret = pos;
        };
        match key {
            Key::Left => {
                let sel = t.anchor.filter(|a| *a != t.caret && !mods.shift);
                let pos = sel.map_or(t.caret.saturating_sub(1), |a| a.min(t.caret));
                move_to(&mut t, pos, mods);
            }
            Key::Right => {
                let sel = t.anchor.filter(|a| *a != t.caret && !mods.shift);
                let pos = sel.map_or((t.caret + 1).min(n), |a| a.max(t.caret));
                move_to(&mut t, pos, mods);
            }
            Key::Home => move_to(&mut t, 0, mods),
            Key::End => move_to(&mut t, n, mods),
            Key::Backspace => {
                if !delete_selection(&mut t) && t.caret > 0 {
                    let c = t.caret - 1;
                    remove_char(&mut t, c);
                    t.caret -= 1;
                }
                changed = true;
            }
            Key::Delete => {
                if !delete_selection(&mut t) && t.caret < n {
                    let c = t.caret;
                    remove_char(&mut t, c);
                }
                changed = true;
            }
            Key::Letter(c) if mods.ctrl => match c.to_ascii_lowercase() {
                'a' => {
                    t.anchor = Some(0);
                    t.caret = n;
                }
                'c' | 'x' => {
                    if let Some(a) = t.anchor.filter(|a| *a != t.caret) {
                        let (s, e) = (a.min(t.caret), a.max(t.caret));
                        if t.tvf & tvf::PASSWORD == 0 {
                            let sel: String = t.text.chars().skip(s).take(e - s).collect();
                            self.events.push(Event::Copy(sel));
                        }
                        if c.eq_ignore_ascii_case(&'x') && t.tvf & tvf::PASSWORD == 0 {
                            delete_selection(&mut t);
                            changed = true;
                        }
                    }
                }
                'v' => self.events.push(Event::PasteRequested),
                _ => {}
            },
            _ => {}
        }
        self.tree.views[f].kind = Kind::Text(t);
        self.ensure_caret_visible(f);
        if changed {
            self.emit_text_changed(f);
        }
    }

    fn text_input(&mut self, s: &str) {
        let Some(f) = self.focus else { return };
        let Kind::Text(mut t) = self.tree.views[f].kind.clone() else { return };
        let mut any = false;
        for ch in s.chars() {
            if ch.is_control() {
                continue;
            }
            if t.tvf & tvf::NUMERIC != 0 && !ch.is_ascii_digit() {
                continue;
            }
            delete_selection(&mut t);
            let byte = t.text.char_indices().nth(t.caret).map_or(t.text.len(), |(b, _)| b);
            t.text.insert(byte, ch);
            t.caret += 1;
            any = true;
        }
        if any {
            self.caret_epoch = self.time;
            self.tree.views[f].kind = Kind::Text(t);
            self.ensure_caret_visible(f);
            self.emit_text_changed(f);
        }
    }

    fn emit_text_changed(&mut self, f: ViewId) {
        if let (Some(w), Kind::Text(t)) = (self.window_of(f), &self.tree.views[f].kind) {
            let text = t.text.clone();
            let view = self.outer_name(f);
            self.events.push(Event::TextChanged { window: w, view, text });
        }
        if let Some(c) = self.combo_of(f) {
            if let Kind::Combo(cd) = &mut self.tree.views[c].kind {
                cd.selected = None;
            }
        }
    }

    /// Horizontal scroll of the single-line editor so that the caret stays visible.
    fn ensure_caret_visible(&mut self, f: ViewId) {
        let Kind::Text(t) = self.tree.views[f].kind.clone() else { return };
        let pw = t.tvf & tvf::PASSWORD != 0;
        let chars: Vec<char> = t.text.chars().collect();
        let cx: f32 = chars[..t.caret.min(chars.len())].iter().map(|c| self.fonts.font(t.font).advance(if pw { '*' } else { *c }) as f32).sum();
        let total: f32 = chars.iter().map(|c| self.fonts.font(t.font).advance(if pw { '*' } else { *c }) as f32).sum();
        let w = self.tree.views[f].frame.width() + 1.0;
        let mut sx = t.scroll_x;
        if cx < sx {
            sx = cx;
        } else if cx > sx + w - 2.0 {
            sx = cx - (w - 2.0);
        }
        if total + 1.0 <= w {
            sx = 0.0;
        }
        if let Kind::Text(t) = &mut self.tree.views[f].kind {
            t.scroll_x = sx.max(0.0);
        }
    }
}

fn byte_of(t: &TextData, ci: usize) -> usize {
    t.text.char_indices().nth(ci).map_or(t.text.len(), |(b, _)| b)
}

fn remove_char(t: &mut TextData, ci: usize) {
    let b = byte_of(t, ci);
    if b < t.text.len() {
        t.text.remove(b);
    }
}

fn delete_selection(t: &mut TextData) -> bool {
    if let Some(a) = t.anchor.take().filter(|a| *a != t.caret) {
        let (s, e) = (a.min(t.caret), a.max(t.caret));
        let (bs, be) = (byte_of(t, s), byte_of(t, e));
        t.text.replace_range(bs..be, "");
        t.caret = s;
        return true;
    }
    t.anchor = None;
    false
}
