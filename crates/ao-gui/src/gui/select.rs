//! Mouse selection and copy in read-only multi-line `TextView`s (`TVF_ALLOW_TEXT_SELECTION`, the chat text, docs/chat/gui.md §12).
//!
//! RE: `TextRenderer_c::MouseDown` 0x101637ef (left, flag 0x4: a hyperlink under the pointer raises the link signal, otherwise `SetCursorPosition` +
//! `BeginSelection` 0x1016108a and the mouse is captured), `MouseMove` 0x101639c6 (`SetCursorPosition` + `ExpandSelection`; outside the view the 20 ms
//! `SlotScrollTimer` scrolls), `CopyActiveSelectionToClipboard` 0x10160f84 -> `CopyToClipboard` 0x10160d15 (`HTMLParser_c::ExtractText` of the range, `\n` -> CRLF,
//! then `ClearSelection`), the highlight is `Clear(rect, 0xc0c0c0)` in `_RenderString`. Only one selection exists at a time (`BeginSelection` clears every other
//! renderer). The original has no double-click word selection in `MouseDown` (**not ported**: nothing to port).

use super::*;

/// The active selection: char indices into the view's flat text (see [`Gui::flat_text`]).
pub(super) struct Sel {
    pub view: ViewId,
    pub anchor: usize,
    pub caret: usize,
    pub dragging: bool,
}

/// Seconds between auto-scroll steps while dragging outside the view (`SlotScrollTimer`: `EventTimer_c::Start(20000)` = 20 ms).
const SCROLL_STEP: f32 = 0.02;

/// Plain text of a layout: the runs of each line, `\n` after lines that end at a real break.
pub(super) fn flat_lines(layout: &text::TextLayout) -> Vec<(usize, String, bool)> {
    let mut start = 0;
    layout
        .lines
        .iter()
        .map(|l| {
            let s: String = l.runs.iter().map(|r| r.text.as_str()).collect();
            let n = s.chars().count();
            let out = (start, s, l.hard_break);
            start += n + l.hard_break as usize;
            out
        })
        .collect()
}

impl Gui {
    /// Layout of a read-only text view as drawn: (layout, content left / top in screen pixels, fill-bottom offset, width).
    fn sel_layout(&mut self, v: ViewId) -> Option<(text::TextLayout, f32, f32, i32, i32)> {
        let Kind::Text(t) = self.tree.views[v].kind.clone() else { return None };
        let o = self.origin(v);
        let (wx, wy) = self.window_of(v).and_then(|w| self.windows[w].as_ref()).map_or((0, 0), |w| w.pos);
        let width = self.tree.views[v].frame.width() as i32 + 1;
        let wrap = if t.tvf & tvf::WORD_WRAP != 0 { Some(width) } else { None };
        let layout = text::layout_text(&mut self.fonts, &self.colors, t.font, &t.text, t.tvf, wrap);
        let dy = self.fill_bottom_dy(v, t.tvf, layout.height);
        Some((layout, o.0 + wx as f32, o.1 + wy as f32, dy, width))
    }

    fn line_pen(align: Align, width: i32, line_w: i32) -> i32 {
        match align {
            Align::Right => width - line_w,
            Align::Center => (width - line_w) / 2,
            _ => 0,
        }
    }

    /// Flat char index under screen (x, y) (nearest character boundary; above / below the text = start / end).
    fn sel_index_at(&mut self, v: ViewId, x: f32, y: f32) -> usize {
        let Some((layout, ox, oy, dy, width)) = self.sel_layout(v) else { return 0 };
        let Kind::Text(t) = self.tree.views[v].kind.clone() else { return 0 };
        let flat = flat_lines(&layout);
        let line_h = self.fonts.font(t.font).height;
        let ry = (y - oy) as i32 - dy;
        let Some(li) = layout.lines.iter().position(|l| ry < l.y + line_h).or(layout.lines.len().checked_sub(1)) else { return 0 };
        if ry < 0 {
            return 0;
        }
        let line = &layout.lines[li];
        let rx = x - ox - (Self::line_pen(line.align, width, line.width) + line.indent) as f32;
        let (start, s, hard) = &flat[li];
        let n = s.chars().count();
        let mut acc = 0.0;
        for (i, c) in s.chars().enumerate() {
            let a = self.fonts.font(t.font).advance(c) as f32;
            if rx < acc + a * 0.5 {
                return start + i;
            }
            acc += a;
        }
        // past the end of the line: the line end (before the break)
        let _ = hard;
        start + n
    }

    /// The selected text (`HTMLParser_c::ExtractText` of the range; `\n` for real line breaks), if any.
    pub fn selected_text(&mut self) -> Option<String> {
        let sel = self.ix.sel.as_ref()?;
        let (s, e) = (sel.anchor.min(sel.caret), sel.anchor.max(sel.caret));
        if s == e {
            return None;
        }
        let v = sel.view;
        let (layout, ..) = self.sel_layout(v)?;
        let all: String = flat_lines(&layout).into_iter().map(|(_, s, h)| if h { s + "\n" } else { s }).collect();
        Some(all.chars().skip(s).take(e - s).collect())
    }

    pub fn clear_selection(&mut self) {
        self.ix.sel = None;
    }

    /// Start of a selection (left press on a read-only text view that allows it).
    pub(super) fn sel_begin(&mut self, v: ViewId, x: f32, y: f32) {
        let i = self.sel_index_at(v, x, y);
        self.ix.sel = Some(Sel { view: v, anchor: i, caret: i, dragging: true });
        self.ix.sel_timer = 0.0;
    }

    pub(super) fn sel_drag(&mut self, x: f32, y: f32) {
        let Some(s) = &self.ix.sel else { return };
        if !s.dragging {
            return;
        }
        let v = s.view;
        let i = self.sel_index_at(v, x, y);
        if let Some(s) = &mut self.ix.sel {
            s.caret = i;
        }
    }

    pub(super) fn sel_end(&mut self) {
        if let Some(s) = &mut self.ix.sel {
            s.dragging = false;
        }
    }

    /// While a drag is held above / below the enclosing `ScrollView` the view scrolls a line every 20 ms (**GUESS**: the step size).
    pub(super) fn sel_autoscroll(&mut self, dt: f32) {
        let Some(s) = &self.ix.sel else { return };
        if !s.dragging {
            return;
        }
        let v = s.view;
        let Some(sv) = self.tree.views[v].parent.and_then(|c| self.tree.views[c].parent).filter(|p| matches!(self.tree.views[*p].kind, Kind::ScrollView(_))) else { return };
        let o = self.origin(sv);
        let wy = self.window_of(sv).and_then(|w| self.windows[w].as_ref()).map_or(0, |w| w.pos.1) as f32;
        let (top, bottom) = (o.1 + wy, o.1 + wy + self.tree.views[sv].frame.height());
        let m = self.mouse;
        let dir = if m.y < top {
            -1.0
        } else if m.y > bottom {
            1.0
        } else {
            return;
        };
        self.ix.sel_timer += dt;
        while self.ix.sel_timer >= SCROLL_STEP {
            self.ix.sel_timer -= SCROLL_STEP;
            let lh = self.fonts.font(FontId::Chat).height as f32;
            self.scroll_by(sv, dir * lh);
        }
        self.sel_drag(m.x, m.y);
    }

    /// Ctrl+C with a selection: [`Event::Copy`] with the text, then the selection is cleared (`CopyToClipboard` ends with `ClearSelection`).
    pub(super) fn sel_copy(&mut self) -> bool {
        match self.selected_text() {
            Some(t) => {
                self.events.push(Event::Copy(t));
                self.ix.sel = None;
                true
            }
            None => false,
        }
    }

    /// Highlight rectangles `(x0, y0, x1, y1)` (screen) of the selection of view `v` for a layout drawn with `origin_x` / top `r_t`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn sel_rects(&mut self, v: ViewId, layout: &text::TextLayout, r_l: f32, r_t: f32, dy: i32, width: i32, font: FontId) -> Vec<[f32; 4]> {
        let Some(s) = &self.ix.sel else { return vec![] };
        if s.view != v || s.anchor == s.caret {
            return vec![];
        }
        let (a, b) = (s.anchor.min(s.caret), s.anchor.max(s.caret));
        let fh = self.fonts.font(font).height as f32;
        let mut out = vec![];
        for (li, (start, text, hard)) in flat_lines(layout).into_iter().enumerate() {
            let n = text.chars().count();
            // the break char itself is selectable: it highlights the rest of the line only up to the text end
            let (s0, s1) = (a.max(start), b.min(start + n + hard as usize));
            if s0 >= s1 {
                continue;
            }
            let chars: Vec<char> = text.chars().collect();
            let mut adv = |k: usize| -> f32 { chars[..k.min(n)].iter().map(|c| self.fonts.font(font).advance(*c)).sum::<i32>() as f32 };
            let pen = r_l + (Self::line_pen(layout.lines[li].align, width, layout.lines[li].width) + layout.lines[li].indent) as f32;
            let (x0, x1) = (pen + adv(s0 - start), pen + adv((s1 - start).min(n)));
            if x1 > x0 {
                let y = r_t + (layout.lines[li].y + dy) as f32;
                out.push([x0, y, x1, y + fh]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::{TextLayout, TextLine, TextRun};
    use crate::view::Align;

    fn line(s: &str, hard: bool) -> TextLine {
        TextLine { y: 0, width: 0, indent: 0, align: Align::Left, runs: vec![TextRun { text: s.into(), color: None, link: false, href: String::new(), img: None }], hard_break: hard }
    }

    #[test]
    fn flat_text_joins_soft_wraps_and_breaks_hard() {
        let l = TextLayout { lines: vec![line("hello ", false), line("world", true), line("next", false)], max_width: 0, height: 0 };
        let f = flat_lines(&l);
        assert_eq!(f.iter().map(|x| x.0).collect::<Vec<_>>(), vec![0, 6, 12]);
        let all: String = f.into_iter().map(|(_, s, h)| if h { s + "\n" } else { s }).collect();
        assert_eq!(all, "hello world\nnext");
    }
}
