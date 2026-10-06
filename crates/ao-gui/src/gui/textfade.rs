//! Chat message fading (`is_message_fading_enabled`, prefs `ChatTextFadeDelay` / `ChatTextFadeTime`; docs/chat/gui.md §5.1).
//!
//! RE (GUI.dll): the chat text area (`FUN_100925ff`, the object around the scrolling `TextView` at `+0x128`) keeps, besides the normal text, a queue of
//! *fade lines* (`+0x12c`, count `+0x130`, the delay / time in microseconds at `+0x158` / `+0x160`).
//! * `FUN_1009349d(delay, time)` (called by `FUN_1008f432` with `ChatTextFadeDelay` / `ChatTextFadeTime` x 1e6 `_DAT_101ae2f8`, and with `0, 0` by
//!   `FUN_1008fb36(false)` when the window option is off): both zero = fading off (frame timer stopped, every fade line deleted, the text view shown again and
//!   scrolled to the bottom); otherwise the text view (scrollbar included) is **hidden**, and while the queue is empty and the view holds text, `FUN_10092f69`
//!   spawns one fade line from that whole text.
//! * `FUN_100935ba` (add text): the text view always receives the line; with fading on `FUN_10092f69(html)` also creates a `TextRenderer_c` child (view flag
//!   `0x80` = ignores the parent alpha: `View::_CallRender` 0x1014d2e3 resets the inherited alpha to 1.0, so the window transparency does not touch the
//!   lines), flags `0xa60`, sized to the bounds width, bottom-aligned on the bounds bottom; every older line moves up by `height + 1 - shadow_y`
//!   (`ChatTextShadowOffset`, 1 shipped: exactly one line height); front lines whose bottom is above the bounds top are deleted.
//! * Per frame (`FUN_10092241`): front lines older than `delay + time` are deleted; every line gets `View::SetAlpha((born + delay - now + time) / time)`
//!   while that is < 1, i.e. opaque for `delay`, then a linear fade over `time`.
use super::*;
use std::collections::VecDeque;

/// `FUN_10092f69`: `HTMLParser_c::SetFeatureFlags(0xa60)` = ENABLE_SHADOW | DISABLE_RC_MENU | WORD_WRAP | MULTILINE (the shadow is only rendered with
/// the `ChatView` shadow flag, off in the shipped client).
const FLAGS: u32 = tvf::ENABLE_SHADOW | tvf::DISABLE_RC_MENU | tvf::WORD_WRAP | tvf::MULTILINE;

struct FadeLine {
    html: String,
    /// `get_system_time()` at creation (the GUI clock, seconds).
    born: f32,
    /// Text height (px).
    h: i32,
    /// Distance (px) of the line's bottom from the bounds bottom; grows by `h + 1 - shadow_y` with every newer line.
    off: f32,
}

/// Fade state of one chat text area (the `ScrollView` hosting the text); the view is hidden while this exists.
pub(super) struct TextFade {
    delay: f32,
    time: f32,
    lines: VecDeque<FadeLine>,
}

impl TextFade {
    /// `View::SetAlpha` value of a line at time `now` (`FUN_10092241`); 1.0 until the delay has passed.
    fn alpha(&self, l: &FadeLine, now: f32) -> f32 {
        let a = (l.born + self.delay - now + self.time) / self.time;
        if a < 1.0 {
            a
        } else {
            1.0
        }
    }
}

impl Gui {
    /// `FUN_1009349d`: `Some((delay, time))` seconds (not both 0) turns fading on for the named `ScrollView`, `None` off. `existing` is the text the view holds
    /// (HTML); it becomes the first fade line when fading starts with an empty queue.
    pub fn set_text_fade(&mut self, w: WindowId, name: &str, cfg: Option<(f32, f32)>, existing: &str) {
        let Some(v) = self.find(w, name) else { return };
        match cfg.filter(|c| *c != (0.0, 0.0)) {
            None => {
                if self.text_fades.remove(&v).is_some() {
                    self.tree.views[v].visible = true;
                    self.relayout_window(w);
                }
            }
            Some((delay, time)) => {
                let f = self.text_fades.entry(v).or_insert_with(|| TextFade { delay, time, lines: VecDeque::new() });
                (f.delay, f.time) = (delay, time);
                if self.tree.views[v].visible {
                    self.tree.views[v].visible = false;
                    self.relayout_window(w);
                }
                if !existing.is_empty() && self.text_fades[&v].lines.is_empty() {
                    self.add_fade_line(w, name, existing);
                }
            }
        }
    }

    /// `FUN_10092f69`: a new line at the bottom of the fading area (no-op while fading is off).
    pub fn add_fade_line(&mut self, w: WindowId, name: &str, html: &str) {
        let Some(v) = self.find(w, name) else { return };
        if !self.text_fades.contains_key(&v) {
            return;
        }
        let fr = self.tree.views[v].frame;
        let lay = text::layout_text(&mut self.fonts, &self.colors, FontId::Chat, html, FLAGS, Some(fr.width() as i32 + 1));
        let shift = lay.height as f32 + 1.0 - self.text_shadow.1 as f32;
        let (height, now) = (fr.height() + 1.0, self.time);
        let f = self.text_fades.get_mut(&v).expect("checked above");
        f.lines.iter_mut().for_each(|l| l.off += shift);
        f.lines.push_back(FadeLine { html: html.to_string(), born: now, h: lay.height, off: 0.0 });
        // lines scrolled out above the bounds top are deleted from the front
        while f.lines.front().is_some_and(|l| height - l.off < 0.0) {
            f.lines.pop_front();
        }
    }

    /// Number of live fade lines of the named view (0 when fading is off).
    pub fn fade_line_count(&self, w: WindowId, name: &str) -> usize {
        self.find(w, name).and_then(|v| self.text_fades.get(&v)).map_or(0, |f| f.lines.len())
    }

    /// `FUN_10092241`, deletion half: front lines older than `delay + time` go.
    pub(super) fn tick_text_fades(&mut self) {
        let now = self.time;
        for f in self.text_fades.values_mut() {
            let life = f.delay + f.time;
            while f.lines.front().is_some_and(|l| now > l.born + life) {
                f.lines.pop_front();
            }
        }
    }

    /// Draws the fade lines in place of the hidden text area; each line at its own alpha only (view flag 0x80: the parent / window alpha is ignored).
    pub(super) fn draw_fade_lines(&mut self, id: ViewId, ox: f32, oy: f32, parent_tint: [u8; 3], out: &mut Vec<DrawCmd>) {
        let v = &self.tree.views[id];
        let (fr, color) = (v.frame, v.color);
        let rect = Rect::new(ox + fr.l, oy + fr.t, ox + fr.l + fr.width(), oy + fr.t + fr.height());
        let tint = mul(parent_tint, self.map_color(color));
        let now = self.time;
        let Some(f) = self.text_fades.get(&id) else { return };
        let lines: Vec<(String, i32, f32, f32)> = f.lines.iter().map(|l| (l.html.clone(), l.h, l.off, f.alpha(l, now))).collect();
        let outer = push_clip(out, [rect.l as i32, rect.t as i32, rect.r as i32 + 1, rect.b as i32 + 1]);
        for (html, h, off, alpha) in lines {
            let top = rect.b + 1.0 - off - h as f32;
            let t = TextData { text: html, font: FontId::Chat, tvf: FLAGS, min_pref: Point::new(-1.0, -1.0), max_pref: Point::new(-1.0, -1.0), caret: 0, anchor: None, scroll_x: 0.0, hint: String::new() };
            self.draw_text_view(out, id, &t, Rect::new(rect.l, top, rect.r, top + h as f32 - 1.0), tint, alpha);
        }
        out.push(DrawCmd::Clip(outer));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(born: f32) -> FadeLine {
        FadeLine { html: String::new(), born, h: 14, off: 0.0 }
    }

    #[test]
    fn alpha_is_one_for_the_delay_then_falls_linearly_over_the_time() {
        let f = TextFade { delay: 8.0, time: 0.3, lines: VecDeque::new() };
        let l = line(10.0);
        assert_eq!(f.alpha(&l, 10.0), 1.0);
        assert_eq!(f.alpha(&l, 18.0), 1.0); // age == delay: (0.3) / 0.3
        assert!((f.alpha(&l, 18.15) - 0.5).abs() < 1e-4);
        assert!(f.alpha(&l, 18.3).abs() < 1e-4);
    }

    #[test]
    fn zero_time_never_divides_into_a_fade() {
        let f = TextFade { delay: 2.0, time: 0.0, lines: VecDeque::new() };
        assert_eq!(f.alpha(&line(0.0), 1.0), 1.0); // +inf, not < 1
    }
}
