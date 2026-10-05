//! View tree, element kinds and XML construction (`View::View`, `View::LoadChildViews`,
//! `XMLObject_c::CreateObject`).
//!
//! RE evidence (GUI.dll): `View::View(TiXmlElement*, uint, bool)` 0x1014e21c reads, in this order,
//! `view_flags`, `view_resize_mask`, `name`, `width/height_group(_owner)`, `fade_group`,
//! `layout_borders` (Rect, default 0), `min_size` / `max_size` (Point, default -1,-1),
//! `max_size_limit` (default 16000,16000), `h_alignment` / `v_alignment` (`FUN_10149cae`: left=0,
//! right=1, top=2, bottom=3, anything else = 4/center), then `LoadChildViews` (0x1014de80):
//! `view_layout` = `stacked` | `horizontal` | anything else (including none) = vertical
//! `VLayoutNode`; unknown element names are skipped (`CreateObject` returns null).

use crate::font::FontId;
use crate::geom::{Point, Rect};
use crate::gfx::{GfxId, GfxSet};
use crate::xml::Element;

pub type ViewId = usize;

/// `alignment` enum of GUI.dll (`View::SetHLayoutAlignment`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left = 0,
    Right = 1,
    Top = 2,
    Bottom = 3,
    Center = 4,
}

impl Align {
    /// `FUN_10149cae` (case-insensitive compare).
    pub fn parse(s: &str) -> Align {
        match s.to_ascii_lowercase().as_str() {
            "left" => Align::Left,
            "right" => Align::Right,
            "top" => Align::Top,
            "bottom" => Align::Bottom,
            _ => Align::Center,
        }
    }
}

/// Which `LayoutNode` subclass a view owns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Node {
    /// No layout node (leaf widgets and `view_layout="stacked"`).
    None,
    /// `LayoutNode` base class: children fill the bounds (BorderView, TextInputView).
    Base,
    V,
    H,
}

/// `TVF_*` feature flags of `TextView_c::TextView_c` 0x10164fae.
pub mod tvf {
    pub const ACCEPT_TXT_INPUT: u32 = 0x1;
    pub const ACCEPT_MOUSE_INPUT: u32 = 0x4;
    pub const ALLOW_TEXT_SELECTION: u32 = 0x8;
    pub const PASSWORD: u32 = 0x10;
    pub const MULTILINE: u32 = 0x20;
    pub const WORD_WRAP: u32 = 0x40;
    pub const WORD_SPLIT: u32 = 0x80;
    pub const IGNORE_NEWLINES: u32 = 0x100;
    pub const DISABLE_RC_MENU: u32 = 0x200;
    pub const FILL_BOTTOM_UP: u32 = 0x400;
    pub const ENABLE_SHADOW: u32 = 0x800;
    pub const RENDER_SHADOW: u32 = 0x1000;
    pub const NUMERIC: u32 = 0x2000;
    pub const ALT_ENTER_MODE: u32 = 0x4000;

    pub fn parse(s: &str) -> u32 {
        let mut f = 0;
        for t in s.split(|c: char| c == '|' || c == ',' || c.is_whitespace()).filter(|t| !t.is_empty()) {
            f |= match t.to_ascii_uppercase().as_str() {
                "TVF_ACCEPT_TXT_INPUT" => ACCEPT_TXT_INPUT,
                "TVF_ACCEPT_MOUSE_INPUT" => ACCEPT_MOUSE_INPUT,
                "TVF_ALLOW_TEXT_SELECTION" => ALLOW_TEXT_SELECTION,
                "TVF_PASSWORD" => PASSWORD,
                "TVF_MULTILINE" => MULTILINE,
                "TVF_WORD_WRAP" => WORD_WRAP,
                "TVF_WORD_SPLIT" => WORD_SPLIT,
                "TVF_IGNORE_NEWLINES" => IGNORE_NEWLINES,
                "TVF_DISABLE_RC_MENU" => DISABLE_RC_MENU,
                "TVF_FILL_BOTTOM_UP" => FILL_BOTTOM_UP,
                "TVF_ENABLE_SHADOW" => ENABLE_SHADOW,
                "TVF_RENDER_SHADOW" => RENDER_SHADOW,
                "TVF_NUMERIC" => NUMERIC,
                "TVF_ALT_ENTER_MODE" => ALT_ENTER_MODE,
                _ => 0,
            };
        }
        f
    }
}

#[derive(Clone, Debug)]
pub struct TextData {
    /// Authored/assigned text (after `#key` localisation); may contain the HTML subset.
    pub text: String,
    pub font: FontId,
    pub tvf: u32,
    /// `TextRenderer_c::SetMinPreferredSize` / `SetMaxPreferredSize` (−1 = unset).
    pub min_pref: Point,
    pub max_pref: Point,
    /// Caret (char index) and selection anchor for editable text.
    pub caret: usize,
    pub anchor: Option<usize>,
    pub scroll_x: f32,
}

#[derive(Clone, Debug)]
pub struct BorderData {
    /// tl, tr, bl, br, left, top, right, bottom, bg (0 = none) — `BorderView_c::Initialize`.
    pub gfx: [Option<GfxId>; 9],
    /// `alpha` attribute (`View::SetLocalAlpha`).
    pub local_alpha: f32,
    /// `color` attribute: `0x01000000..` ColorID or raw 0xRRGGBB.
    pub local_color: u32,
}

#[derive(Clone, Debug)]
pub struct ButtonData {
    pub label: String,
    /// Optional `gfxid_raised/pressed/hover` override (`Button_c::Button_c(TiXmlElement*)`).
    pub gfx_override: Option<[GfxId; 3]>,
    pub pressed: bool,
    pub hover: bool,
}

#[derive(Clone, Debug)]
pub struct TextButtonData {
    pub text: String,
    pub font: FontId,
    pub color: u32,
    pub hover_color: u32,
    pub pressed_color: u32,
    pub pressed: bool,
    pub hover: bool,
    /// `ButtonBase_c::SetToggleButton`: the pressed look persists after release.
    pub toggle: bool,
    pub toggled: bool,
}

#[derive(Clone, Debug)]
pub struct PowerBarData {
    pub bg: Option<GfxId>,
    pub full: Option<GfxId>,
    pub left: Option<GfxId>,
    pub right: Option<GfxId>,
    /// `Direction_e`: 0 down, 1 right, 2 left, 3 up.
    pub dir: u8,
    pub value: f32,
}

#[derive(Clone, Debug)]
pub struct ComboData {
    pub items: Vec<String>,
    pub selected: Option<usize>,
    pub open: bool,
}

#[derive(Clone, Debug)]
pub enum Kind {
    View,
    /// `LayoutSpacer`: fixed min/max extents (`+0x128`/`+0x130`).
    Spacer { min: Point, max: Point },
    Border(BorderData),
    Text(TextData),
    Button(ButtonData),
    TextButton(TextButtonData),
    PowerBar(PowerBarData),
    Bitmap { gfx: Vec<GfxId>, index: usize },
    /// TextInputView_c: outer view whose children are `BorderView(INSET)` → editor `TextView`.
    Input,
    /// ComboBox_c = TextInputView + arrow BitmapView inside the border (`ComboBox_c::Initialize` 0x100021bb).
    Combo(ComboData),
    ScrollView(ScrollData),
    ScrollChild,
    CheckBox { label: String, checked: bool },
    RadioButton { label: String, value: i32 },
    RadioGroup { selected: i32 },
    /// Element the engine has no implementation for (kept so the layout stays intact).
    Unsupported(String),
}

#[derive(Clone, Debug, Default)]
pub struct ScrollData {
    pub v_mode: ScrollMode,
    pub h_mode: ScrollMode,
    pub offset: Point,
}

/// `ScrollBarMode_e` (`ScrollView_c::ScrollView_c` XML: none/auto/auto_reserve/always).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScrollMode {
    #[default]
    None,
    Auto,
    AutoReserve,
    Always,
}

#[derive(Clone, Debug)]
pub struct View {
    pub name: String,
    pub kind: Kind,
    pub parent: Option<ViewId>,
    pub children: Vec<ViewId>,
    /// Frame in the parent's coordinate system (inclusive extents).
    pub frame: Rect,
    pub borders: Rect,
    pub min_size: Point,
    pub max_size: Point,
    pub max_limit: Point,
    pub weight: f32,
    pub h_align: Align,
    pub v_align: Align,
    pub node: Node,
    pub stacked: bool,
    pub flags: u32,
    pub visible: bool,
    pub enabled: bool,
    /// `View::SetColor` (inherited by children, multiplicative) 0xRRGGBB or ColorID.
    pub color: u32,
    /// `View::SetAlpha`.
    pub alpha: f32,
    pub tab_order: i32,
}

impl View {
    pub fn new(kind: Kind) -> View {
        View {
            name: String::new(),
            kind,
            parent: None,
            children: Vec::new(),
            frame: Rect::default(),
            borders: Rect::default(),
            min_size: Point::new(-1.0, -1.0),
            max_size: Point::new(-1.0, -1.0),
            max_limit: Point::new(16000.0, 16000.0),
            weight: 1.0,
            h_align: Align::Center,
            v_align: Align::Center,
            node: Node::None,
            stacked: false,
            flags: 0,
            visible: true,
            enabled: true,
            color: 0xffffff,
            alpha: 1.0,
            tab_order: -1,
        }
    }
}

/// `view_flags` bit: view is excluded from layout while hidden (`HLayoutNode::Layout` 0x101300d7).
pub const VF_COLLAPSE_WHEN_HIDDEN: u32 = 0x100;

#[derive(Default)]
pub struct Tree {
    pub views: Vec<View>,
}

impl Tree {
    pub fn add(&mut self, v: View) -> ViewId {
        self.views.push(v);
        self.views.len() - 1
    }
    pub fn append_child(&mut self, parent: ViewId, child: ViewId) {
        self.views[child].parent = Some(parent);
        self.views[parent].children.push(child);
    }
    pub fn find(&self, root: ViewId, name: &str) -> Option<ViewId> {
        if self.views[root].name == name && !name.is_empty() {
            return Some(root);
        }
        self.views[root].children.iter().find_map(|c| self.find(*c, name))
    }
    /// Every view named `name` under `root` (an item holds `summary_view` and `detailed_view`, both with a `level` etc.).
    pub fn find_all(&self, root: ViewId, name: &str) -> Vec<ViewId> {
        let mut out = Vec::new();
        let mut stack = vec![root];
        while let Some(v) = stack.pop() {
            if self.views[v].name == name && !name.is_empty() {
                out.push(v);
            }
            stack.extend(self.views[v].children.iter().rev());
        }
        out
    }
}

// ------------------------------------------------------------------ attribute parsing (XMLObject_c)

/// `Variant::LoadFromString` for ints: decimal or `0x` hex (`ExpressionParser_c` in `GetAttrInt`).
pub fn parse_int(s: &str) -> Option<i64> {
    let t = s.trim();
    let (neg, t) = t.strip_prefix('-').map_or((false, t), |r| (true, r));
    let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) { i64::from_str_radix(h, 16).ok()? } else { t.parse::<i64>().ok()? };
    Some(if neg { -v } else { v })
}

fn nums(s: &str, prefix: &str) -> Option<Vec<f32>> {
    let body = s.trim().strip_prefix(prefix)?.strip_prefix('(')?.strip_suffix(')')?;
    body.split(',').map(|p| p.trim().parse::<f32>().ok()).collect()
}

/// `XMLObject_c::GetAttrPoint`: `Point(x,y)`; any other typed value gives (0,0) (`Variant::AsPoint`).
pub fn attr_point(e: &Element, key: &str, default: Point) -> Point {
    match e.attr(key) {
        None => default,
        Some(s) => match nums(s, "Point") {
            Some(v) if v.len() == 2 => Point::new(v[0], v[1]),
            _ => Point::new(0.0, 0.0),
        },
    }
}

pub fn attr_rect(e: &Element, key: &str, default: Rect) -> Rect {
    match e.attr(key) {
        None => default,
        Some(s) => match nums(s, "Rect") {
            Some(v) if v.len() == 4 => Rect::new(v[0], v[1], v[2], v[3]),
            _ => Rect::default(),
        },
    }
}

pub fn attr_f32(e: &Element, key: &str, default: f32) -> f32 {
    e.attr(key).and_then(|s| s.trim().parse().ok()).unwrap_or(default)
}

pub fn attr_u32(e: &Element, key: &str, default: u32) -> u32 {
    e.attr(key).and_then(parse_int).map_or(default, |v| v as u32)
}

pub fn attr_bool(e: &Element, key: &str, default: bool) -> bool {
    match e.attr(key) {
        None => default,
        Some(s) => match s.to_ascii_lowercase().as_str() {
            "true" => true,
            "false" => false,
            o => o.parse::<i64>().map(|v| v != 0).unwrap_or(false),
        },
    }
}

/// Everything construction needs from the host.
pub struct BuildCtx<'a> {
    pub gfx: &'a GfxSet,
    /// `#key` → text (`XMLObject_c::GetAttrString` 0x1001443f calls `LdbGetText`).
    pub localize: &'a dyn Fn(&str) -> Option<String>,
    pub warnings: Vec<String>,
}

impl BuildCtx<'_> {
    /// `XMLObject_c::GetAttrString`: values starting with `#` are looked up in the text database;
    /// unresolved keys stay verbatim.
    pub fn string(&self, e: &Element, key: &str) -> String {
        let Some(s) = e.attr(key) else { return String::new() };
        self.text(s)
    }
    pub fn text(&self, s: &str) -> String {
        if s.starts_with('#') {
            (self.localize)(s).unwrap_or_else(|| s.to_string())
        } else {
            s.to_string()
        }
    }
    fn gfx_attr(&self, e: &Element, key: &str) -> Option<GfxId> {
        e.attr(key).filter(|s| !s.is_empty()).and_then(|n| self.gfx.id(n))
    }
}

/// `View::View(TiXmlElement*, default_flags, load_children)` common attributes.
fn apply_view_attrs(v: &mut View, e: &Element, default_flags: u32) {
    v.flags = attr_u32(e, "view_flags", default_flags);
    v.name = e.attr("name").unwrap_or("").to_string();
    v.borders = attr_rect(e, "layout_borders", Rect::default());
    v.min_size = attr_point(e, "min_size", Point::new(-1.0, -1.0));
    v.max_size = attr_point(e, "max_size", Point::new(-1.0, -1.0));
    v.max_limit = attr_point(e, "max_size_limit", Point::new(16000.0, 16000.0));
    v.h_align = Align::parse(e.attr("h_alignment").unwrap_or(""));
    v.v_align = Align::parse(e.attr("v_alignment").unwrap_or(""));
    v.tab_order = e.attr("tab_order").and_then(parse_int).map_or(-1, |v| v as i32);
}

fn default_gfx(ctx: &BuildCtx, ids: [u32; 9]) -> [Option<GfxId>; 9] {
    let mut out = [None; 9];
    for (o, id) in out.iter_mut().zip(ids) {
        if id != 0 && ctx.gfx.image(GfxId(id)).is_some() {
            *o = Some(GfxId(id));
        }
    }
    out
}

/// `BorderView_c::BorderView_c(Rect,String,uint,uint)` default ids 0xf9,0xfb,0xf4,0xf6,0xf7,0xfa,0xf8,0xf5 (GFX_GUI_INSET_*).
pub fn inset_border(ctx: &BuildCtx) -> BorderData {
    BorderData { gfx: default_gfx(ctx, [0xf9, 0xfb, 0xf4, 0xf6, 0xf7, 0xfa, 0xf8, 0xf5, 0]), local_alpha: 1.0, local_color: 0xffffff }
}

/// ColorID or raw colour (`GUIConfig_c::GetColorID` 0x1012f3e2 for the names, else `GetAttrInt`).
pub fn color_attr(e: &Element, key: &str, default: u32) -> u32 {
    match e.attr(key) {
        None => default,
        Some(s) => match s.to_ascii_uppercase().as_str() {
            "DEFAULT" => 0x1000000,
            "SELECTED" => 0x2000000,
            "HOVER" => 0x3000000,
            "TEXT" => 0x4000000,
            "TEXT_SELECTED" => 0x5000000,
            "TEXT_HOVER" => 0x6000000,
            _ => parse_int(s).map_or(default, |v| v as u32),
        },
    }
}

fn text_data(ctx: &BuildCtx, e: &Element) -> TextData {
    let tvf_v = tvf::parse(e.attr("feature_flags").unwrap_or(""));
    let text = ctx.string(e, "value");
    TextData {
        text,
        font: FontId::from_name(e.attr("font").unwrap_or("")),
        tvf: tvf_v,
        min_pref: attr_point(e, "min_size", Point::new(-1.0, -1.0)),
        max_pref: attr_point(e, "max_size", Point::new(-1.0, -1.0)),
        caret: 0,
        anchor: None,
        scroll_x: 0.0,
    }
}

fn new_text_view(ctx: &BuildCtx, e: &Element) -> View {
    let mut v = View::new(Kind::Text(text_data(ctx, e)));
    apply_view_attrs(&mut v, e, 0);
    v.color = attr_u32(e, "color", 0xffffff);
    v
}

/// Builds the view for `e` and its subtree; returns `None` for unknown element names handled as skipped.
pub fn build(tree: &mut Tree, ctx: &mut BuildCtx, e: &Element) -> Option<ViewId> {
    let id = match e.name.as_str() {
        "View" => {
            let mut v = View::new(Kind::View);
            apply_view_attrs(&mut v, e, 0);
            let id = tree.add(v);
            load_children(tree, ctx, id, e);
            id
        }
        "HLayoutSpacer" | "VLayoutSpacer" => {
            // LayoutSpacer(TiXmlElement*) + (H|V)LayoutSpacer(TiXmlElement*), FUN_10130c6c:
            // min_size/max_size are scalars (sscanf %f), min default 0, max default 16000; they are stored
            // through LayoutSpacer::SetMinSize/SetMaxSize, which subtract (1,1).
            let sc = |k: &str, d: f32| e.attr(k).and_then(|s| s.trim().parse::<f32>().ok()).unwrap_or(d);
            let (mn, mx) = (sc("min_size", 0.0), sc("max_size", 16000.0));
            let (min, max) = if e.name == "HLayoutSpacer" {
                (Point::new(mn - 1.0, -1.0), Point::new(mx - 1.0, 16000.0 - 1.0))
            } else {
                (Point::new(-1.0, mn - 1.0), Point::new(16000.0 - 1.0, mx - 1.0))
            };
            let mut v = View::new(Kind::Spacer { min, max });
            apply_view_attrs(&mut v, e, 4);
            tree.add(v)
        }
        "BorderView" => {
            let mut v = View::new(Kind::Border(BorderData {
                gfx: {
                    // defaults of BorderView_c::BorderView_c(TiXmlElement*) 0x10126d6c (INSET set)
                    let mut g = default_gfx(ctx, [0xf9, 0xfb, 0xf4, 0xf6, 0xf7, 0xfa, 0xf8, 0xf5, 0]);
                    for (slot, key) in ["tl_gfx", "tr_gfx", "bl_gfx", "br_gfx", "left_gfx", "top_gfx", "right_gfx", "bottom_gfx", "bg_gfx"].iter().enumerate() {
                        // `if (len != 0) id = GetID(name)`; an empty attribute keeps the default
                        if let Some(n) = e.attr(key).filter(|s| !s.is_empty()) {
                            g[slot] = ctx.gfx.id(n);
                        }
                    }
                    g
                },
                local_alpha: attr_f32(e, "alpha", 1.0),
                local_color: color_attr(e, "color", 0xffffff),
            }));
            apply_view_attrs(&mut v, e, 4);
            v.node = Node::Base;
            let id = tree.add(v);
            if !e.children.is_empty() {
                // client View(el, 4, true) with the same element; name cleared; borders (0,0,0,0)
                let mut c = View::new(Kind::View);
                apply_view_attrs(&mut c, e, 4);
                c.name = String::new();
                c.borders = Rect::default();
                let cid = tree.add(c);
                load_children(tree, ctx, cid, e);
                tree.append_child(id, cid);
            }
            id
        }
        "TextView" => tree.add(new_text_view(ctx, e)),
        "Button" => {
            let mut v = View::new(Kind::Button(ButtonData {
                label: ctx.string(e, "label"),
                gfx_override: None,
                pressed: false,
                hover: false,
            }));
            apply_view_attrs(&mut v, e, 0);
            tree.add(v)
        }
        "TextButton" => {
            let mut v = View::new(Kind::TextButton(TextButtonData {
                text: ctx.string(e, "text"),
                font: FontId::from_name(e.attr("font").unwrap_or("")),
                color: attr_u32(e, "color", 0xffffff),
                hover_color: attr_u32(e, "hover_color", 0xffffff),
                pressed_color: attr_u32(e, "pressed_color", 0xffffff),
                pressed: false,
                hover: false,
                toggle: false,
                toggled: false,
            }));
            apply_view_attrs(&mut v, e, 0);
            tree.add(v)
        }
        "PowerBar" => {
            let mut v = View::new(Kind::PowerBar(PowerBarData {
                bg: ctx.gfx_attr(e, "bg_gfx"),
                full: ctx.gfx_attr(e, "full_gfx"),
                left: ctx.gfx_attr(e, "left_gfx"),
                right: ctx.gfx_attr(e, "right_gfx"),
                dir: match e.attr("direction").map(str::to_ascii_lowercase).as_deref() {
                    Some("left") => 2,
                    Some("up") => 3,
                    Some("down") => 0,
                    _ => 1,
                },
                value: 1.0,
            }));
            apply_view_attrs(&mut v, e, 0);
            v.node = Node::Base; // PowerbarView_c::Initialize sets an HLayoutNode (no children)
            tree.add(v)
        }
        "BitmapView" => {
            let mut v = View::new(Kind::Bitmap { gfx: ctx.gfx_attr(e, "bitmap_id").into_iter().collect(), index: 0 });
            apply_view_attrs(&mut v, e, 4);
            tree.add(v)
        }
        "TextInputView" | "ComboBox" => build_input(tree, ctx, e, e.name == "ComboBox"),
        "ScrollView" => {
            let mode = |k: &str| match e.attr(k).map(str::to_ascii_lowercase).as_deref() {
                Some("auto") => ScrollMode::Auto,
                Some("auto_reserve") => ScrollMode::AutoReserve,
                Some("always") => ScrollMode::Always,
                _ => ScrollMode::None,
            };
            let mut v = View::new(Kind::ScrollView(ScrollData { v_mode: mode("v_scrollbar_mode"), h_mode: mode("h_scrollbar_mode"), offset: Point::default() }));
            apply_view_attrs(&mut v, e, 0);
            v.node = Node::Base;
            let id = tree.add(v);
            for c in &e.children {
                if let Some(cid) = build(tree, ctx, c) {
                    tree.append_child(id, cid);
                }
            }
            id
        }
        "ViewSelector" => {
            // `ViewSelector_c`: its children share the parent's frame (stacked); the application shows one at a time
            // (`Gui::select_child`, the original's `SetValue(index)` = vtable +0xf0).
            let mut v = View::new(Kind::View);
            apply_view_attrs(&mut v, e, 0);
            let id = tree.add(v);
            load_children(tree, ctx, id, e);
            tree.views[id].stacked = true;
            tree.views[id].node = Node::None;
            id
        }
        "ScrollViewChild" => {
            let mut v = View::new(Kind::ScrollChild);
            apply_view_attrs(&mut v, e, 0);
            let id = tree.add(v);
            load_children(tree, ctx, id, e);
            id
        }
        "CheckBox" => {
            let mut v = View::new(Kind::CheckBox { label: ctx.string(e, "label"), checked: false });
            apply_view_attrs(&mut v, e, 0);
            tree.add(v)
        }
        "RadioButton" => {
            let mut v = View::new(Kind::RadioButton { label: ctx.string(e, "label"), value: attr_u32(e, "value", 0) as i32 });
            apply_view_attrs(&mut v, e, 0);
            tree.add(v)
        }
        "RadioButtonGroup" => {
            let mut v = View::new(Kind::RadioGroup { selected: 0 });
            apply_view_attrs(&mut v, e, 0);
            let id = tree.add(v);
            load_children(tree, ctx, id, e);
            id
        }
        other => {
            // Elements registered by Gamecode.dll (OptionCheckBox, ItemSlotView, CCMenu, ...) or by
            // widgets this engine does not implement yet.
            ctx.warnings.push(format!("unsupported element <{other}> (name={:?})", e.attr("name").unwrap_or("")));
            let mut v = View::new(Kind::Unsupported(other.to_string()));
            apply_view_attrs(&mut v, e, 0);
            let id = tree.add(v);
            load_children(tree, ctx, id, e);
            id
        }
    };
    Some(id)
}

/// `View::LoadChildViews` 0x1014de80.
pub fn load_children(tree: &mut Tree, ctx: &mut BuildCtx, id: ViewId, e: &Element) {
    let layout = e.attr("view_layout");
    match layout.map(str::to_ascii_lowercase).as_deref() {
        Some("stacked") => {
            tree.views[id].stacked = true;
            tree.views[id].node = Node::None;
        }
        Some("horizontal") => tree.views[id].node = Node::H,
        _ => tree.views[id].node = Node::V,
    }
    for c in &e.children {
        if let Some(cid) = build(tree, ctx, c) {
            tree.append_child(id, cid);
        }
    }
}

/// `TextInputView_c::TextInputView_c(TiXmlElement*)` 0x10149077 / `Initialize` 0x101488ec (label
/// position none): `View(base layout) → BorderView(INSET gfx) → client = editor TextView` with client
/// borders (3,1,3,1); editor flags 0xd; the editor is built from the same element.
fn build_input(tree: &mut Tree, ctx: &mut BuildCtx, e: &Element, combo: bool) -> ViewId {
    let mut outer = View::new(if combo { Kind::Combo(ComboData { items: vec![], selected: None, open: false }) } else { Kind::Input });
    apply_view_attrs(&mut outer, e, 0);
    outer.node = Node::Base;
    let outer_id = tree.add(outer);

    let mut border = View::new(Kind::Border(inset_border(ctx)));
    border.flags = 4;
    border.node = Node::Base;
    let border_id = tree.add(border);

    let mut editor = new_text_view(ctx, e);
    if let Kind::Text(t) = &mut editor.kind {
        t.tvf = 0xd; // HTMLParser_c::SetFeatureFlags(…, 0xd)
    }
    editor.name = "_editor".to_string();
    editor.borders = Rect::new(3.0, 1.0, 3.0, 1.0); // BorderView_c::SetClient(…, 3, 1, 3, 1)
    let editor_id = tree.add(editor);
    tree.append_child(border_id, editor_id);

    if combo {
        // ComboBox_c::Initialize: BitmapView(GFX_GUI_COMBOBOX_CLOSED/OPEN = 0xb3/0xb4), borders (0,0,5,0)
        // added to the BorderView whose layout node becomes an HLayoutNode.
        let mut bmp = View::new(Kind::Bitmap { gfx: [0xb3, 0xb4].iter().filter_map(|i| ctx.gfx.image(GfxId(*i)).map(|_| GfxId(*i))).collect(), index: 0 });
        bmp.flags = 4;
        bmp.borders = Rect::new(0.0, 0.0, 5.0, 0.0);
        let bmp_id = tree.add(bmp);
        tree.views[border_id].node = Node::H;
        tree.append_child(border_id, bmp_id);
    }
    tree.append_child(outer_id, border_id);
    outer_id
}
