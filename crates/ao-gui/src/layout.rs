//! Preferred sizes and layout nodes, ported from GUI.dll.
//!
//! * `View::UpdatePreferredSize` 0x1014aac8 (clamping by `min_size`, `max_size`, `max_size_limit`)
//! * `LayoutNode::{CalculatePreferredSize,Layout}` 0x10130ded / 0x1012fe4f (stacked/base node)
//! * `HLayoutNode` 0x1012ff61 / 0x101300d7, `VLayoutNode` 0x101304c1 / 0x10130631
//! * `View::SpaceOut` 0x1014a238 (weight distribution)
//!
//! All rects/points are inclusive extents (pixels − 1); every child occupies `extent + 1` pixels of the
//! parent, so the `+ 1.0` in the formulas below is the extent→pixel conversion, not a gap.

use crate::font::FontSystem;
use crate::geom::{Point, Rect};
use crate::gfx::GfxSet;
use crate::text::{self, Colors};
use crate::view::*;

pub struct Env<'a> {
    pub gfx: &'a GfxSet,
    pub fonts: &'a mut FontSystem,
    pub colors: &'a Colors,
}

const BIG: f32 = 16000.0; // _DAT_101c61bc

/// Child participates in the parent's layout (`IsVisible || !(flags & 0x100)`).
fn included(tree: &Tree, c: ViewId) -> bool {
    let v = &tree.views[c];
    v.visible || v.flags & VF_COLLAPSE_WHEN_HIDDEN == 0
}

/// `View::GetPreferredSize(max)` after `UpdatePreferredSize` clamping.
pub fn pref(env: &mut Env, tree: &Tree, id: ViewId, max: bool) -> Point {
    let v = &tree.views[id];
    let mut p = calc(env, tree, id, max);
    if !max {
        p.x = p.x.max(v.min_size.x).min(v.max_limit.x);
        p.y = p.y.max(v.min_size.y).min(v.max_limit.y);
    } else {
        p.x = p.x.max(v.min_size.x).max(v.max_size.x).min(v.max_limit.x);
        p.y = p.y.max(v.min_size.y).max(v.max_size.y).min(v.max_limit.y);
    }
    p
}

fn node_calc(env: &mut Env, tree: &Tree, id: ViewId, max: bool) -> Point {
    let v = &tree.views[id];
    let kids: Vec<ViewId> = v.children.iter().copied().filter(|c| included(tree, *c)).collect();
    // (cmin, cmax, borders) per included child
    let info: Vec<(Point, Point, Rect)> = kids.iter().map(|c| (pref(env, tree, *c, false), pref(env, tree, *c, true), tree.views[*c].borders)).collect();
    match v.node {
        Node::H => {
            let (mut min, mut mx) = (Point::new(-1.0, 0.0), Point::new(-1.0, BIG));
            for (cmin, cmax, b) in &info {
                min.x += cmin.x + b.l + b.r + 1.0;
                mx.x += cmax.x + b.l + b.r + 1.0;
                min.y = min.y.max(cmin.y + b.t + b.b);
                mx.y = mx.y.min(cmax.y + b.t + b.b);
            }
            if mx.y < min.y {
                mx.y = min.y;
            }
            if max {
                mx
            } else {
                min
            }
        }
        Node::V => {
            let (mut min, mut mx) = (Point::new(0.0, -1.0), Point::new(BIG, -1.0));
            for (cmin, cmax, b) in &info {
                min.y += cmin.y + b.t + b.b + 1.0;
                mx.y += cmax.y + b.t + b.b + 1.0;
                min.x = min.x.max(cmin.x + b.l + b.r);
                mx.x = mx.x.min(cmax.x + b.l + b.r);
            }
            if mx.x < min.x {
                mx.x = min.x;
            }
            if max {
                mx
            } else {
                min
            }
        }
        Node::Base => {
            let (mut min, mut mx) = (Point::new(-1.0, -1.0), Point::new(BIG, BIG));
            for (cmin, cmax, b) in &info {
                min.x = min.x.max(cmin.x + b.l + b.r);
                min.y = min.y.max(cmin.y + b.t + b.b);
                mx.x = mx.x.min(cmax.x + b.l + b.r);
                mx.y = mx.y.min(cmax.y + b.t + b.b);
            }
            if mx.x < min.x {
                mx.x = min.x;
            }
            if mx.y < min.y {
                mx.y = min.y;
            }
            if max {
                mx
            } else {
                min
            }
        }
        // `View::CalculatePreferredSize`: no layout node → (-1,-1)
        Node::None => Point::new(-1.0, -1.0),
    }
}

/// `CalculatePreferredSize(bool)` virtual of each class.
fn calc(env: &mut Env, tree: &Tree, id: ViewId, max: bool) -> Point {
    let v = &tree.views[id];
    match &v.kind {
        Kind::Spacer { min, max: mx } => {
            if max {
                *mx
            } else {
                *min
            }
        }
        Kind::Text(t) => text_pref(env, tree, id, t, max),
        Kind::Button(b) => button_pref(env, &b.label),
        Kind::TextButton(b) => text::string_size(env.fonts, env.colors, b.font, &b.text),
        Kind::PowerBar(p) => {
            let g = |id: Option<crate::gfx::GfxId>| id.map(|g| env.gfx.size(g));
            let main = g(p.bg).or_else(|| g(p.full)).unwrap_or((0, 0));
            let mut s = Point::new(main.0 as f32 - 1.0, main.1 as f32 - 1.0);
            for cap in [p.left, p.right].into_iter().flatten() {
                let (w, h) = env.gfx.size(cap);
                if p.dir == 1 || p.dir == 2 {
                    s.x += w as f32;
                } else {
                    s.y += h as f32;
                }
            }
            s
        }
        Kind::Bitmap { gfx, .. } => {
            let mut s = Point::new(-1.0, -1.0);
            for g in gfx {
                let (w, h) = env.gfx.size(*g);
                s.x = s.x.max(w as f32 - 1.0);
                s.y = s.y.max(h as f32 - 1.0);
            }
            s
        }
        Kind::ScrollView(sd) => {
            let mut p = match v.children.first() {
                Some(c) => pref(env, tree, *c, max),
                None => Point::new(-1.0, -1.0),
            };
            if sd.v_mode == ScrollMode::AutoReserve || sd.v_mode == ScrollMode::Always {
                p.x += SCROLLBAR_W + 1.0;
            }
            if sd.h_mode == ScrollMode::AutoReserve || sd.h_mode == ScrollMode::Always {
                p.y += SCROLLBAR_W + 1.0;
            }
            p
        }
        Kind::ScrollChild => {
            let Some(c) = v.children.first() else { return Point::new(0.0, 0.0) };
            if max {
                return pref(env, tree, *c, true);
            }
            let mut p = pref(env, tree, *c, false);
            if let Some(par) = v.parent {
                if let Kind::ScrollView(sd) = &tree.views[par].kind {
                    if sd.h_mode != ScrollMode::None {
                        p.x = 0.0;
                    }
                    if sd.v_mode != ScrollMode::None {
                        p.y = 0.0;
                    }
                }
            }
            p
        }
        Kind::CheckBox { label, .. } | Kind::RadioButton { label, .. } => {
            // UNRESOLVED: CheckBox_c/RadioButton_c::CalculatePreferredSize not yet read; 11px marker + gap + label.
            let s = text::string_size(env.fonts, env.colors, crate::font::FontId::Normal, label);
            Point::new(11.0 + 4.0 + s.x + 1.0, s.y.max(10.0))
        }
        Kind::View | Kind::Border(_) | Kind::Input | Kind::Combo(_) | Kind::RadioGroup { .. } | Kind::Unsupported(_) => node_calc(env, tree, id, max),
    }
}

pub const SCROLLBAR_W: f32 = 10.0; // GFX_GUI_SCROLLBAR_GRAY_* are 11 px wide → extent 10 (`ScrollBar_c::GetDefaultWidth`)

/// `TextRenderer_c::CalculatePreferredSize` 0x101623ea.
fn text_pref(env: &mut Env, tree: &Tree, id: ViewId, t: &TextData, max: bool) -> Point {
    let (mn, mx) = (t.min_pref, t.max_pref);
    let pm = if max { mx } else { mn };
    if pm.x > 0.0 && pm.y >= 0.0 {
        return pm;
    }
    let wrap = if t.tvf & tvf::MULTILINE != 0 {
        // wrap width: `ftol(minPref.x) + 1` when set, else the current frame (UNRESOLVED: sqrt heuristic of the client)
        if mn.x >= 0.0 {
            Some(mn.x as i32 + 1)
        } else {
            let f = tree.views[id].frame;
            if f.width() >= 0.0 {
                Some(f.width() as i32 + 1)
            } else {
                None
            }
        }
    } else {
        None
    };
    let l = text::layout_text(env.fonts, env.colors, t.font, &t.text_for_measure(), t.tvf, wrap);
    let mut p = if t.tvf & tvf::MULTILINE == 0 {
        Point::new(l.max_width as f32 - 1.0, env.fonts.font(t.font).height as f32 - 1.0)
    } else {
        Point::new(l.max_width as f32 - 1.0, l.height as f32 - 1.0)
    };
    if t.tvf & tvf::ACCEPT_TXT_INPUT != 0 {
        p.x += 4.0; // _DAT_101b36b0
    }
    if t.tvf & (tvf::ENABLE_SHADOW | tvf::RENDER_SHADOW) != 0 {
        p.x += 1.0;
        p.y += 1.0;
    }
    if pm.x != -1.0 {
        p.x = pm.x;
    }
    if pm.y != -1.0 {
        p.y = pm.y;
    }
    p
}

impl TextData {
    /// Text used for measuring (identical to what is drawn).
    pub fn text_for_measure(&self) -> String {
        self.text.clone()
    }
}

/// `Button_c::CalculatePreferredSize` 0x10127ee4 for border mode 0 (the XML `Button`):
/// border sizes are replaced by Rect(8,1,8,4) and 10 is added to the width.
pub fn button_pref(env: &mut Env, label: &str) -> Point {
    let mut p = Point::new(0.0, 0.0);
    if !label.is_empty() {
        let s = text::string_size(env.fonts, env.colors, crate::font::FontId::Normal, label);
        p = s;
        p.x += 1.0;
    }
    let b = Rect::new(8.0, 1.0, 8.0, 4.0);
    p.x += b.r + b.l;
    p.x -= 1.0;
    p.x += 10.0; // _DAT_101ae2d8
    // fVar1 = 2.0 + b.b + b.t + 2.0 + p.y  (_DAT_101a8b90 = 2.0)
    p.y += 2.0 + b.b + b.t + 2.0;
    p
}

// ------------------------------------------------------------------------------------ layout

/// `View::SpaceOut` 0x1014a238; returns the left-over space.
#[allow(clippy::too_many_arguments)]
pub fn space_out(n: usize, avail: f32, sum_min: f32, total_weight: f32, min: &[f32], max: &[f32], weights: Option<&[f32]>, out: &mut [f32]) -> f32 {
    let mut avail = avail;
    let mut extra = avail - sum_min;
    let mut total_weight = if weights.is_none() { n as f32 } else { total_weight };
    let mut frozen = vec![false; n];
    let mut start = 0usize;
    loop {
        let mut i = start;
        let mut froze = false;
        while i < n {
            if !frozen[i] {
                let (w, share) = match weights {
                    None => (1.0, 1.0 / total_weight),
                    Some(ws) => {
                        let w = ws[i];
                        (w, if w < 1e-5 { 0.0 } else { w / total_weight })
                    }
                };
                let o = share * extra + min[i];
                out[i] = o;
                if max[i] <= o {
                    extra -= max[i] - min[i];
                    total_weight -= w;
                    out[i] = max[i];
                    frozen[i] = true;
                    if i == start {
                        start = i + 1;
                    }
                    froze = true;
                    break;
                }
            }
            i += 1;
        }
        if !froze {
            for o in out.iter().take(n) {
                avail -= *o;
            }
            if avail > 0.0 {
                let mut cap = 0.0;
                for i in 0..n {
                    if max[i] <= out[i] {
                        frozen[i] = true;
                    } else {
                        frozen[i] = false;
                        cap += max[i] - out[i];
                    }
                }
                if cap > 0.0 {
                    for i in 0..n {
                        if !frozen[i] {
                            let f = ((max[i] - out[i]) / cap) * avail;
                            out[i] += f;
                            avail -= f;
                        }
                    }
                }
            }
            return avail;
        }
    }
}

/// Sets `id`'s frame (inclusive extents, parent coordinates) and re-lays-out its subtree
/// (`View::SetFrame` 0x1014cce5 → `_ReLayout`).
pub fn set_frame(env: &mut Env, tree: &mut Tree, id: ViewId, frame: Rect) {
    tree.views[id].frame = frame;
    layout(env, tree, id);
}

pub fn layout(env: &mut Env, tree: &mut Tree, id: ViewId) {
    let f = tree.views[id].frame;
    let b = Rect::new(0.0, 0.0, f.width(), f.height()); // View::GetBounds
    let kind_scroll = matches!(tree.views[id].kind, Kind::ScrollView(_) | Kind::ScrollChild);
    if kind_scroll {
        layout_scroll(env, tree, id);
        return;
    }
    if tree.views[id].stacked {
        // children follow the parent's size (resize mask 0xf, `View::LoadChildViews`)
        let kids: Vec<ViewId> = tree.views[id].children.clone();
        for c in kids {
            set_frame(env, tree, c, b);
        }
        return;
    }
    match tree.views[id].node {
        Node::None => {}
        Node::Base => {
            let kids: Vec<ViewId> = tree.views[id].children.iter().copied().filter(|c| included(tree, *c)).collect();
            for c in kids {
                let r = b.shrink(&tree.views[c].borders);
                set_frame(env, tree, c, r);
            }
        }
        Node::H | Node::V => layout_hv(env, tree, id, b),
    }
}

fn layout_hv(env: &mut Env, tree: &mut Tree, id: ViewId, b: Rect) {
    let horizontal = tree.views[id].node == Node::H;
    let (h_align, v_align) = (tree.views[id].h_align, tree.views[id].v_align);
    let kids: Vec<ViewId> = tree.views[id].children.iter().copied().filter(|c| included(tree, *c)).collect();
    let n = kids.len();
    if n == 0 {
        return;
    }
    let (mut min, mut max, mut weights, mut cross_max) = (vec![0.0; n], vec![0.0; n], vec![0.0; n], vec![0.0; n]);
    let (mut sum_min, mut sum_w) = (0.0f32, 0.0f32);
    for (i, c) in kids.iter().enumerate() {
        let cmin = pref(env, tree, *c, false);
        let cmax = pref(env, tree, *c, true);
        let br = tree.views[*c].borders;
        weights[i] = tree.views[*c].weight;
        if horizontal {
            cross_max[i] = br.t + cmax.y + br.b;
            max[i] = br.l + cmax.x + br.r + 1.0;
            min[i] = cmin.x + br.l + br.r + 1.0;
        } else {
            cross_max[i] = br.l + cmax.x + br.r;
            max[i] = br.t + cmax.y + br.b + 1.0;
            min[i] = cmin.y + br.t + br.b + 1.0;
        }
        sum_min += min[i];
        sum_w += weights[i];
    }
    let avail = if horizontal { b.width() } else { b.height() } + 1.0;
    let mut out = vec![0.0; n];
    let left = space_out(n, avail, sum_min, sum_w, &min, &max, Some(&weights), &mut out);
    let gap = left / n as f32;
    let mut pos = gap * 0.5 + if horizontal { b.l } else { b.t };
    for (i, c) in kids.iter().enumerate() {
        let len = out[i] - 1.0;
        let mut rect = if horizontal { Rect::new(0.0, 0.0, len, b.b) } else { Rect::new(0.0, 0.0, b.r, len) };
        let cross = if horizontal { rect.height() } else { rect.width() };
        if cross_max[i] < cross {
            if horizontal {
                rect.b = cross_max[i];
            } else {
                rect.r = cross_max[i];
            }
        }
        let cross_pos = if horizontal {
            match v_align {
                Align::Top => b.t,
                Align::Bottom => b.b - rect.height(),
                _ => (b.height() - rect.height()) * 0.5 + b.t,
            }
        } else {
            match h_align {
                Align::Left => b.l,
                Align::Right => b.r - rect.width(),
                _ => (b.width() - rect.width()) * 0.5 + b.l,
            }
        };
        let rect = if horizontal { rect.translate(pos, cross_pos) } else { rect.translate(cross_pos, pos) };
        pos += gap + out[i];
        let rect = rect.shrink(&tree.views[*c].borders).floor();
        set_frame(env, tree, *c, rect);
    }
}

/// `ScrollView_c` / `ScrollViewChild_c::LayoutChilds` 0x10143c17.
fn layout_scroll(env: &mut Env, tree: &mut Tree, id: ViewId) {
    let f = tree.views[id].frame;
    let b = Rect::new(0.0, 0.0, f.width(), f.height());
    match tree.views[id].kind.clone() {
        Kind::ScrollView(sd) => {
            let Some(client) = tree.views[id].children.first().copied() else { return };
            // client = ScrollViewChild; decide scrollbar visibility from its content height
            let mut r = b;
            let reserve_v = matches!(sd.v_mode, ScrollMode::AutoReserve | ScrollMode::Always);
            if reserve_v {
                r.r -= SCROLLBAR_W + 1.0;
            }
            set_frame(env, tree, client, r);
            if matches!(sd.v_mode, ScrollMode::Auto) {
                let content = content_height(env, tree, client);
                if content > b.height() {
                    r.r -= SCROLLBAR_W + 1.0;
                    set_frame(env, tree, client, r);
                }
            }
        }
        Kind::ScrollChild => {
            let Some(inner) = tree.views[id].children.first().copied() else { return };
            let size = b.size();
            let (cmin, cmax) = (pref(env, tree, inner, false), pref(env, tree, inner, true));
            let mut s = size;
            if cmin.x < s.x {
                s.x = if cmax.x <= s.x { cmax.x } else { s.x };
            } else {
                s.x = cmin.x;
            }
            if cmin.y < s.y {
                s.y = if cmax.y <= s.y { cmax.y } else { s.y };
            } else {
                s.y = cmin.y;
            }
            set_frame(env, tree, inner, Rect::new(0.0, 0.0, s.x, s.y));
        }
        _ => {}
    }
}

/// Height (extent) of the scrolled content: the client's clamped size.
pub fn content_height(env: &mut Env, tree: &mut Tree, scroll_child: ViewId) -> f32 {
    let Some(inner) = tree.views[scroll_child].children.first().copied() else { return -1.0 };
    let cmin = pref(env, tree, inner, false);
    let cmax = pref(env, tree, inner, true);
    let f = tree.views[scroll_child].frame;
    let h = f.height();
    if cmin.y < h {
        if cmax.y <= h {
            cmax.y
        } else {
            h
        }
    } else {
        cmin.y
    }
}
