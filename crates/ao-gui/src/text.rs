//! Text colours (`TextColors.xml`, `GUIColors.xml`) and the HTML-subset text layout used by
//! `TextView_c` (GUI.dll `HTMLParser_c` + `TextRenderer_c::_ReWrap` 0x10161ba4 / `_RenderLine` 0x10161112).
//!
//! RE notes
//! * A line is rendered glyph by glyph at the pen position; the pen advances by the glyph advance
//!   (`_RenderString` 0x1015ffdd).  `TVF_PASSWORD` (0x10) replaces every glyph with `*` (0x2a).
//! * Line alignment 1 = right, 4 = centre (`_RenderLine`).
//! * `\n` is a space when `TVF_IGNORE_NEWLINES` is set, a line break when `TVF_MULTILINE` is set.
//! * UNRESOLVED: the exact line pitch of `_AddLineDesc` (we use the font height) and the HTML tags
//!   other than `br`, `center`, `font color`, `a`, `p`, `div`, `b`, `i`, `u`, `img` (ignored).

use crate::font::{FontId, FontSystem};
use crate::geom::Point;
use crate::view::{parse_int, tvf, Align};
use crate::xml;
use std::collections::HashMap;
use std::path::Path;

/// `FontSystem_t::SetColorsIntoMap` is the source of the numeric `ColorCode_e` table; named colours
/// come from `TextColors.xml`.
#[derive(Default, Clone)]
pub struct Colors {
    named: HashMap<String, String>,
}

impl Colors {
    pub fn load(gui_dir: &Path) -> anyhow::Result<Colors> {
        let src = std::fs::read_to_string(gui_dir.join("TextColors.xml"))?;
        let root = xml::parse(&src)?;
        let mut c = Colors::default();
        for e in &root.children {
            if e.name == "HTMLColor" {
                if let (Some(n), Some(v)) = (e.attr("name"), e.attr("color")) {
                    c.named.insert(n.to_ascii_lowercase(), v.to_string());
                }
            }
        }
        Ok(c)
    }

    /// `#rrggbb`, `0xRRGGBB` or a (possibly nested) colour name.
    pub fn parse(&self, s: &str) -> Option<u32> {
        let mut s = s.trim().trim_matches('"').trim_matches('\'').to_string();
        for _ in 0..8 {
            if let Some(h) = s.strip_prefix('#') {
                return u32::from_str_radix(h, 16).ok();
            }
            if let Some(v) = parse_int(&s) {
                return Some(v as u32 & 0xffffff);
            }
            s = self.named.get(&s.to_ascii_lowercase())?.clone();
        }
        None
    }
}

#[derive(Clone, Debug)]
pub struct TextRun {
    pub text: String,
    /// `None` = the renderer's default colour.
    pub color: Option<u32>,
    pub link: bool,
}

#[derive(Clone, Debug)]
pub struct TextLine {
    pub y: i32,
    pub width: i32,
    pub align: Align,
    pub runs: Vec<TextRun>,
}

#[derive(Clone, Debug, Default)]
pub struct TextLayout {
    pub lines: Vec<TextLine>,
    pub max_width: i32,
    pub height: i32,
}

#[derive(Clone)]
enum Tok {
    Text(String),
    Br,
    Open(String, Vec<(String, String)>),
    Close(String),
}

fn decode_entities(s: &str) -> String {
    s.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&")
}

fn tokenize(src: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut i = 0;
    let mut text = String::new();
    while i < b.len() {
        if b[i] == b'<' {
            if let Some(end) = src[i..].find('>') {
                let inner = &src[i + 1..i + end];
                let tag_like = inner.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '/');
                if tag_like {
                    if !text.is_empty() {
                        out.push(Tok::Text(decode_entities(&std::mem::take(&mut text))));
                    }
                    let inner = inner.trim();
                    if let Some(c) = inner.strip_prefix('/') {
                        out.push(Tok::Close(c.trim().to_ascii_lowercase()));
                    } else {
                        let selfclose = inner.ends_with('/');
                        let inner = inner.trim_end_matches('/').trim();
                        let (name, rest) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
                        let name = name.to_ascii_lowercase();
                        let attrs = parse_tag_attrs(rest);
                        if name == "br" {
                            out.push(Tok::Br);
                        } else {
                            out.push(Tok::Open(name.clone(), attrs));
                            if selfclose {
                                out.push(Tok::Close(name));
                            }
                        }
                    }
                    i += end + 1;
                    continue;
                }
            }
        }
        let ch = src[i..].chars().next().unwrap();
        text.push(ch);
        i += ch.len_utf8();
    }
    if !text.is_empty() {
        out.push(Tok::Text(decode_entities(&text)));
    }
    out
}

fn parse_tag_attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let key = rest[..eq].trim().to_ascii_lowercase();
        let after = rest[eq + 1..].trim_start();
        let (val, tail) = if let Some(r) = after.strip_prefix('"') {
            r.split_once('"').unwrap_or((r, ""))
        } else if let Some(r) = after.strip_prefix('\'') {
            r.split_once('\'').unwrap_or((r, ""))
        } else {
            after.split_once(char::is_whitespace).unwrap_or((after, ""))
        };
        out.push((key, val.to_string()));
        rest = tail.trim_start();
    }
    out
}

struct Word {
    text: String,
    color: Option<u32>,
    link: bool,
    /// width in px of the word including its trailing space
    w: i32,
    hard_break_after: bool,
    align: Align,
}

/// Lays `text` out. `wrap` = available width in pixels (only used with `TVF_WORD_WRAP`).
pub fn layout_text(fonts: &mut FontSystem, colors: &Colors, font: FontId, text: &str, flags: u32, wrap: Option<i32>) -> TextLayout {
    let password = flags & tvf::PASSWORD != 0;
    let fnt = fonts.font(font);
    let line_h = fnt.height;
    let mut words: Vec<Word> = Vec::new();
    let mut color_stack: Vec<Option<u32>> = vec![None];
    let mut align_stack = vec![Align::Left];
    let mut link = 0usize;
    let measure = |fnt: &mut crate::font::Font, s: &str| -> i32 {
        if password {
            s.chars().map(|_| fnt.advance('*')).sum()
        } else {
            fnt.text_width(s)
        }
    };
    let push_text = |words: &mut Vec<Word>, fnt: &mut crate::font::Font, s: &str, color: Option<u32>, link: bool, align: Align| {
        // split into words keeping spaces attached to the preceding word
        let mut cur = String::new();
        let mut it = s.chars().peekable();
        while let Some(c) = it.next() {
            cur.push(c);
            if c == ' ' && it.peek() != Some(&' ') {
                let w = measure(fnt, &cur);
                words.push(Word { text: std::mem::take(&mut cur), color, link, w, hard_break_after: false, align });
            }
        }
        if !cur.is_empty() {
            let w = measure(fnt, &cur);
            words.push(Word { text: cur, color, link, w, hard_break_after: false, align });
        }
    };
    for tok in tokenize(text) {
        match tok {
            Tok::Text(t) => {
                let mut first = true;
                for part in t.split('\n') {
                    if !first {
                        if flags & tvf::MULTILINE != 0 && flags & tvf::IGNORE_NEWLINES == 0 {
                            if let Some(w) = words.last_mut() {
                                w.hard_break_after = true;
                            } else {
                                words.push(Word { text: String::new(), color: None, link: false, w: 0, hard_break_after: true, align: *align_stack.last().unwrap() });
                            }
                        } else {
                            push_text(&mut words, fonts.font(font), " ", *color_stack.last().unwrap(), link > 0, *align_stack.last().unwrap());
                        }
                    }
                    first = false;
                    let part = part.trim_end_matches('\r');
                    push_text(&mut words, fonts.font(font), part, *color_stack.last().unwrap(), link > 0, *align_stack.last().unwrap());
                }
            }
            Tok::Br => {
                if let Some(w) = words.last_mut() {
                    w.hard_break_after = true;
                } else {
                    words.push(Word { text: String::new(), color: None, link: false, w: 0, hard_break_after: true, align: *align_stack.last().unwrap() });
                }
            }
            Tok::Open(name, attrs) => match name.as_str() {
                "font" => {
                    let c = attrs.iter().find(|(k, _)| k == "color").and_then(|(_, v)| colors.parse(v));
                    color_stack.push(c.or(*color_stack.last().unwrap()));
                }
                "center" => align_stack.push(Align::Center),
                "a" => link += 1,
                "p" | "div" | "b" | "i" | "u" | "span" => {
                    color_stack.push(*color_stack.last().unwrap());
                    align_stack.push(*align_stack.last().unwrap());
                }
                _ => {}
            },
            Tok::Close(name) => match name.as_str() {
                "font" => {
                    if color_stack.len() > 1 {
                        color_stack.pop();
                    }
                }
                "center" => {
                    if align_stack.len() > 1 {
                        align_stack.pop();
                    }
                }
                "a" => link = link.saturating_sub(1),
                "p" | "div" | "b" | "i" | "u" | "span" => {
                    if color_stack.len() > 1 {
                        color_stack.pop();
                    }
                    if align_stack.len() > 1 {
                        align_stack.pop();
                    }
                }
                _ => {}
            },
        }
    }
    // wrap
    let limit = if flags & tvf::WORD_WRAP != 0 { wrap } else { None };
    let mut layout = TextLayout::default();
    let mut cur_runs: Vec<TextRun> = Vec::new();
    let mut cur_w = 0;
    let mut cur_align = Align::Left;
    let mut y = 0;
    let flush = |layout: &mut TextLayout, runs: &mut Vec<TextRun>, w: &mut i32, align: Align, y: &mut i32| {
        // trailing space of the last word does not count towards the line width (`GetStringSize`-style trim)
        layout.max_width = layout.max_width.max(*w);
        layout.lines.push(TextLine { y: *y, width: *w, align, runs: std::mem::take(runs) });
        *y += line_h;
        *w = 0;
    };
    let nwords = words.len();
    for (n, wd) in words.into_iter().enumerate() {
        if let Some(l) = limit {
            let trimmed = wd.w - if wd.text.ends_with(' ') { fonts.font(font).advance(' ') } else { 0 };
            if cur_w > 0 && cur_w + trimmed > l {
                flush(&mut layout, &mut cur_runs, &mut cur_w, cur_align, &mut y);
            }
        }
        cur_align = wd.align;
        cur_w += wd.w;
        if !wd.text.is_empty() {
            match cur_runs.last_mut() {
                Some(r) if r.color == wd.color && r.link == wd.link => r.text.push_str(&wd.text),
                _ => cur_runs.push(TextRun { text: wd.text, color: wd.color, link: wd.link }),
            }
        }
        if wd.hard_break_after && n < nwords {
            flush(&mut layout, &mut cur_runs, &mut cur_w, cur_align, &mut y);
        }
    }
    if !cur_runs.is_empty() || layout.lines.is_empty() {
        flush(&mut layout, &mut cur_runs, &mut cur_w, cur_align, &mut y);
    }
    layout.height = y;
    // a trailing break leaves an empty last line only when text is non-empty; empty text keeps one line
    layout
}

/// `TextView_c::GetStringSize` (0x101657cc): content size of a single string in `font`
/// (inclusive extents, i.e. pixels − 1).
pub fn string_size(fonts: &mut FontSystem, colors: &Colors, font: FontId, text: &str) -> Point {
    let l = layout_text(fonts, colors, font, text, 0, None);
    Point::new(l.max_width as f32 - 1.0, l.height as f32 - 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tokenizes_font_and_center() {
        let t = tokenize("<center><font color=red>Hi &amp; bye</font>x<br>y</center>");
        assert!(matches!(t[0], Tok::Open(ref n, _) if n == "center"));
        assert!(matches!(t[2], Tok::Text(ref s) if s == "Hi & bye"));
        assert!(t.iter().any(|t| matches!(t, Tok::Br)));
    }
}
