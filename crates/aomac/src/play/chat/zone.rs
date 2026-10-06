//! Zone-server chat: N3 text messages -> [`ChatLine`]s, and the outgoing vicinity / shout / whisper frames.
//! Evidence (addresses, ids, open questions): docs/chat/zone.md.

use super::line::{ChatKind, ChatLine};
use ao_formats::screens::TextDb;
use ao_net::frame::Frame;
use ao_net::msg::Identity;
use ao_net::n3::chat::{self as wire, N3Chat, RemoteFormat, RfArg};
use ao_net::n3::outgoing::{text_frame, text_payload, TextKind};

/// `LDBformat` / `RemoteFormat` (ldb.dll): the text-db template engine behind `FormatFeedbackIIR_t`, `ChatTextIIR_t` and the chat server's
/// local-format system message. Addresses in the comments are `[ldb]` (ldb.dll image base 0x10000000).
pub mod ldb {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Tok {
        /// Argument number this token consumes; 0 = literal.
        arg: i32,
        text: Vec<char>,
    }

    /// `LDBformat` [ldb 0x100054da]: a template split into tokens (`Init` 0x100053f2), fed one argument at a time.
    #[derive(Debug, Clone)]
    pub struct Format {
        toks: Vec<Tok>,
        /// Next argument number (`this+0x10`, starts at 1).
        next: i32,
    }

    /// `LDBface::GetText(cat, id)`; a missing entry reads `no LDBintern (cat:id)` like the mapped-file path [ldb 0x10001622].
    pub fn text(db: &TextDb, cat: u32, id: u32) -> String {
        db.by_id(cat, id).unwrap_or_else(|| format!("no LDBintern ({}:{})", cat as i32, id as i32))
    }

    /// Bytes of a wire string: UTF-8 when valid, else Latin-1 (the text db is Latin-1).
    pub fn lossy(b: &[u8]) -> String {
        match std::str::from_utf8(b) {
            Ok(s) => s.to_owned(),
            Err(_) => b.iter().map(|&c| c as char).collect(),
        }
    }

    impl Format {
        /// `LDBformat::Init` [ldb 0x100053f2]. `%…` printf specs and `#N{…}` / `#N:` markers become argument tokens;
        /// `%%` is a literal `%`, `##` a literal `#`.
        pub fn new(template: &str) -> Self {
            let s: Vec<char> = template.chars().collect();
            let at = |i: isize| -> char { usize::try_from(i).ok().and_then(|i| s.get(i)).copied().unwrap_or('\0') };
            let mut toks = Vec::new();
            // CreateToken [ldb 0x1000535a]: s[a..=b] (b backs up one when it points at the terminator); nothing when a > b.
            let mut make = |a: isize, b: isize, arg: i32| {
                if a <= b {
                    let b = if at(b) == '\0' { b - 1 } else { b };
                    let (lo, hi) = (a.max(0) as usize, ((b + 1).max(0) as usize).min(s.len()));
                    toks.push(Tok { arg, text: s.get(lo..hi.max(lo)).unwrap_or(&[]).to_vec() });
                }
            };
            let (mut start, mut p) = (0isize, 0isize);
            let mut counter = 1i32;
            while at(p) != '\0' {
                let mut next = p + 1; // pcVar5
                match at(p) {
                    '%' => {
                        if p != 0 {
                            make(start, p - 1, 0);
                        }
                        let mut q = p + 1;
                        if at(q) == '%' {
                            make(q, q, 0);
                            start = q + 1;
                            next = start;
                        } else {
                            while at(q) != '\0' && "-+ 0#123456789.hlL".contains(at(q)) {
                                q += 1;
                            }
                            make(p, q, counter);
                            start = q + 1;
                            next = start;
                            counter += 1;
                        }
                    }
                    '#' => {
                        make(start, p - 1, 0);
                        start = p + 1;
                        next = start;
                        if at(start) != '#' {
                            let digit = at(start) as i32 - '0' as i32;
                            start = p;
                            if at(p + 1) == '\0' {
                                p += 1;
                                break;
                            }
                            next = p + 2;
                            match at(next) {
                                '\0' => {
                                    p = next;
                                    break;
                                }
                                '{' => {
                                    let mut q = next;
                                    while at(q) != '}' && at(q) != '\0' {
                                        q += 1;
                                    }
                                    make(next, q, digit);
                                    start = q + 1;
                                    next = start;
                                }
                                ':' => {
                                    start = next + 1;
                                    next = start;
                                    counter = digit;
                                }
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                p = next;
            }
            make(start, p, 0);
            Format { toks, next: 1 }
        }

        /// `Feed(int)` [ldb 0x10004c8b].
        pub fn int(&mut self, v: i32) -> &mut Self {
            let flags = match v {
                0 => 0x400_0000,
                1 => 0x800_0000,
                _ => 0,
            };
            self.feed(flags, |spec| cfmt_int(spec, v as i64, false, "int_value"))
        }

        /// `Feed(uint)` [ldb 0x10004da7]; flags and the `%s` fallback text are assumed to mirror `Feed(int)` (not decompiled separately).
        pub fn uint(&mut self, v: u32) -> &mut Self {
            let flags = match v {
                0 => 0x400_0000,
                1 => 0x800_0000,
                _ => 0,
            };
            self.feed(flags, |spec| cfmt_int(spec, v as i64, true, "uint_value"))
        }

        /// `Feed(float)` [ldb 0x10004ec3].
        pub fn float(&mut self, v: f32) -> &mut Self {
            self.feed(0, |spec| cfmt_float(spec, v as f64))
        }

        /// `Feed(char const*)` / `Feed(std::string const&)` [ldb 0x10004fd1 / 0x100050b2].
        pub fn string(&mut self, v: &str) -> &mut Self {
            self.feed(0, |spec| cfmt_str(spec, v))
        }

        fn feed(&mut self, flags: u32, printf: impl Fn(&str) -> String) -> &mut Self {
            let n = self.next;
            for t in self.toks.iter_mut().filter(|t| t.arg == n) {
                if t.text.first() == Some(&'%') {
                    let spec: String = t.text.iter().collect();
                    t.text = printf(&spec).chars().collect();
                } else {
                    choose(t, flags);
                }
            }
            self.next += 1;
            self
        }

        /// `LDBformat::Dump` [ldb 0x10004885]: the tokens joined, leading blanks dropped and runs of blanks collapsed.
        pub fn dump(&self) -> String {
            let mut out = String::new();
            let mut blank = true;
            for &c in self.toks.iter().flat_map(|t| &t.text) {
                if blank {
                    if c == ' ' {
                        continue;
                    }
                    blank = false;
                } else if c == ' ' {
                    blank = true;
                }
                out.push(c);
            }
            out
        }
    }

    /// `LDBformat::MakeFlagBit` [ldb 0x100040c8]: `A`..`Z` -> bit 0..25, `0`..`5` -> bit 26..31 (`'6'` reads table entry 32, taken as 0).
    fn flag_bit(c: char) -> u32 {
        let u = c.to_ascii_uppercase() as i32;
        let idx = if u < 0x41 { u - 0x16 } else { u - 0x41 };
        if (0..32).contains(&idx) {
            1 << idx
        } else {
            0
        }
    }

    /// `FeedInternal` [ldb 0x100046ab]: a `{flags:text|flags:text}` token is replaced by the alternative whose flags share
    /// the most bits with the argument's flags (the later one on ties). A malformed block stays as it is.
    fn choose(t: &mut Tok, feed: u32) {
        if t.text.first() != Some(&'{') {
            return;
        }
        let s = &t.text;
        let at = |i: usize| s.get(i).copied().unwrap_or('\0');
        let mut alts: Vec<(usize, usize, u32)> = Vec::new();
        let mut p = 0usize;
        loop {
            p += 1;
            if at(p) == '\0' {
                break;
            }
            let mut flags = 0u32;
            while !matches!(at(p), ':' | ' ' | '\0') {
                flags |= flag_bit(at(p));
                p += 1;
            }
            if at(p) == '\0' {
                return;
            }
            if at(p) != ' ' {
                p += 1;
            }
            let first = p;
            if at(p) == '\0' {
                return;
            }
            while !matches!(at(p), '|' | '}' | '\0') {
                p += 1;
            }
            if at(p) == '\0' {
                return;
            }
            alts.push((first, p - 1, flags));
        }
        let mut best = (-1i32, 0usize);
        for (i, a) in alts.iter().enumerate() {
            let score = (a.2 & feed).count_ones() as i32;
            if best.0 <= score {
                best = (score, i);
            }
        }
        if let Some(&(a, b, _)) = alts.get(best.1) {
            t.text = if b >= a { s[a..=b].to_vec() } else { Vec::new() };
        }
    }

    /// The part of a printf spec between `%` and the conversion letter.
    struct Spec {
        left: bool,
        plus: bool,
        space: bool,
        zero: bool,
        width: usize,
        prec: Option<usize>,
        conv: char,
    }

    fn spec(s: &str) -> Spec {
        let c: Vec<char> = s.chars().collect();
        let mut sp = Spec { left: false, plus: false, space: false, zero: false, width: 0, prec: None, conv: 's' };
        let mut i = 1;
        while let Some(&f) = c.get(i) {
            match f {
                '-' => sp.left = true,
                '+' => sp.plus = true,
                ' ' => sp.space = true,
                '0' => sp.zero = true,
                '#' => {}
                _ => break,
            }
            i += 1;
        }
        while let Some(d) = c.get(i).and_then(|c| c.to_digit(10)) {
            sp.width = sp.width * 10 + d as usize;
            i += 1;
        }
        if c.get(i) == Some(&'.') {
            i += 1;
            let mut p = 0;
            while let Some(d) = c.get(i).and_then(|c| c.to_digit(10)) {
                p = p * 10 + d as usize;
                i += 1;
            }
            sp.prec = Some(p);
        }
        while matches!(c.get(i), Some('h' | 'l' | 'L')) {
            i += 1;
        }
        sp.conv = c.get(i).copied().unwrap_or('s');
        sp
    }

    fn pad(sp: &Spec, body: String, numeric: bool) -> String {
        let len = body.chars().count();
        if len >= sp.width {
            return body;
        }
        let fill = sp.width - len;
        if sp.left {
            body + &" ".repeat(fill)
        } else if sp.zero && numeric {
            let (sign, rest) = match body.chars().next() {
                Some(c @ ('-' | '+' | ' ')) => (c.to_string(), body[1..].to_owned()),
                _ => (String::new(), body),
            };
            format!("{sign}{}{rest}", "0".repeat(fill))
        } else {
            " ".repeat(fill) + &body
        }
    }

    /// `snprintf(token, v)` for an integer argument; `%s` prints `<name>_value<v>` (`int_value<%d>`).
    fn cfmt_int(s: &str, v: i64, unsigned: bool, name: &str) -> String {
        let sp = spec(s);
        let sign = |n: String, neg: bool| if neg { format!("-{n}") } else if sp.plus { format!("+{n}") } else if sp.space { format!(" {n}") } else { n };
        let digits = |n: String| match sp.prec {
            Some(p) if n.len() < p => "0".repeat(p - n.len()) + &n,
            _ => n,
        };
        let body = match sp.conv {
            's' => return format!("{name}<{v}>"),
            'd' | 'i' => {
                let v = if unsigned { v as u32 as i32 as i64 } else { v };
                sign(digits(v.unsigned_abs().to_string()), v < 0)
            }
            'u' => digits((v as u32).to_string()),
            'x' => digits(format!("{:x}", v as u32)),
            'X' => digits(format!("{:X}", v as u32)),
            'o' => digits(format!("{:o}", v as u32)),
            'c' => char::from_u32(v as u32 & 0xFF).map(String::from).unwrap_or_default(),
            _ => String::new(),
        };
        pad(&sp, body, sp.prec.is_none())
    }

    fn cfmt_float(s: &str, v: f64) -> String {
        let sp = spec(s);
        let body = match sp.conv {
            's' => return format!("float_value<{v:.6}>"),
            'f' | 'F' => format!("{:.*}", sp.prec.unwrap_or(6), v),
            'e' | 'E' | 'g' | 'G' => format!("{:e}", v),
            _ => String::new(),
        };
        let body = if v >= 0.0 && sp.plus { format!("+{body}") } else { body };
        pad(&sp, body, true)
    }

    fn cfmt_str(s: &str, v: &str) -> String {
        let sp = spec(s);
        if sp.conv != 's' {
            return String::new();
        }
        let v: String = match sp.prec {
            Some(p) => v.chars().take(p).collect(),
            None => v.to_owned(),
        };
        pad(&sp, v, false)
    }

    /// `RemoteFormat::Init` [ldb 0x10005565]: the expanded text of a parsed blob.
    pub fn expand(db: &TextDb, rf: &RemoteFormat) -> String {
        let mut f = Format::new(&text(db, rf.cat, rf.id));
        for a in &rf.args {
            match a {
                RfArg::Int(v) => f.int(*v),
                RfArg::Uint(v) => f.uint(*v),
                RfArg::Float(v) => f.float(*v),
                RfArg::Entry(c, i) => f.string(&text(db, *c, *i)),
                // 's': the nested text when the string is itself a blob, else the string
                RfArg::Str(b) => match wire::parse_remote(b) {
                    Some(n) => f.string(&expand(db, &n)),
                    None => f.string(&lossy(b)),
                },
                // 'F': always the nested expansion (empty when it is not a blob)
                RfArg::Nested(b) => f.string(&wire::parse_remote(b).map(|n| expand(db, &n)).unwrap_or_default()),
            };
        }
        f.dump()
    }

    /// `RemoteFormat::ParseString` [ldb 0x1000593d]: copy `s`, replacing every complete `~&…~` blob by its expansion.
    pub fn parse_string(db: &TextDb, s: &[u8]) -> String {
        let mut out = String::new();
        let mut plain = Vec::new();
        let mut i = 0;
        while i < s.len() {
            if s[i] == b'~' && s[i + 1..].first() == Some(&b'&') {
                if let Some(rf) = wire::parse_remote(&s[i..]) {
                    out += &lossy(&std::mem::take(&mut plain));
                    out += &expand(db, &rf);
                    i += rf.consumed;
                    continue;
                }
            }
            plain.push(s[i]);
            i += 1;
        }
        out + &lossy(&plain)
    }
}

/// What the lines need from the running session.
pub struct ZoneChatCtx<'a> {
    pub texts: &'a TextDb,
    /// Name of the character/NPC dynel with this instance id (`N3Msg_GetName`).
    pub name_of: &'a dyn Fn(u32) -> Option<String>,
    /// Header dynel of the message is a `SimpleChar_t` with byte `+0x80 == 0` (required by `FormatFeedbackIIR_t`'s apply code,
    /// docs/chat/zone.md); the caller knows the header, [`N3Chat`] does not.
    pub header_is_char: bool,
    /// Current target (`InputConfig_t` `+0xc0`), `None` = nothing selected.
    pub target: Option<u32>,
    /// Dynel we are fighting (`N3Msg_GetAttackingID`).
    pub fighting: Option<u32>,
    /// Dynel under the mouse (`InputConfig_t` `+0xd8`).
    pub mouse: Option<u32>,
}

/// A line plus the id its `GlobalSignals_c` `+0x17c` signal carries: `< 0x40000000` selects the chat window whose `+0x1ec` equals it,
/// anything else a chat-server group (slot GUI 0x10083898).
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneText {
    pub channel: u32,
    pub line: ChatLine,
}

/// `ChatGUIModule_c::ColorCodeToHTMLColor` [GUI 0x10087860] table (`PTR_s_CCNoneColor_10268d3c`): colour code -> `TextColors.xml` name.
/// Unknown codes read `white`.
pub fn color_code_name(code: u8) -> &'static str {
    match code {
        0 => "CCNoneColor",
        1 => "CCMenubarColor",
        2 => "CCWhisperColor",
        3 => "CCShoutColor",
        4 => "CCTellColor",
        5 => "CCVicinityColor",
        6 => "CCCommColor",
        7 => "CCTeamColor",
        8 => "CCClanColor",
        9 => "CCEmoteColor",
        10 => "CCLinkColor",
        11 => "CCToolTipColor",
        12 => "CCRed",
        13 => "CCGreen",
        14 => "CCBlue",
        15 => "CCWhite",
        16 => "CCYellow",
        17 => "CCCashColor",
        18 => "CCInfoHeadline",
        19 => "CCInfoHeader",
        20 => "CCInfoText",
        21 => "CCMeHitByNanoColor",
        22 => "CCOtherHitByNanoColor",
        23 => "CCMonsterHitMeColor",
        24 => "CCPlayerHitMeColor",
        25 => "CCMeHitOtherColor",
        26 => "CCOtherHitOtherColor",
        27 => "CCOtherHitOtherMyPetColor",
        28 => "CCMeHealedColor",
        29 => "CCMeGotXpColor",
        30 => "CCSkillColor",
        31 => "CCShowFullNameColor",
        32 => "CCCCHeaderColor",
        33 => "CCCCTextColor",
        34 => "CCTowerColor",
        35 => "CCMeCastNano",
        50 => "CCAdminColor",
        51 => "CCMiscColor",
        52 => "CCGMColor",
        53 => "CCSeekingTeamColor",
        54 => "CCNewbieColor",
        55 => "CCNewsColor",
        80 => "CCInfoTextBolb",
        81 => "CCChatCmdFeedbackError",
        82 => "CCChatCmdFeedbackInfo",
        _ => "white",
    }
}

/// Colour code `FUN_1005aaa9` [GC] passes for a `FormatFeedbackIIR_t` chat line (`CCYellow`).
pub const FORMAT_FEEDBACK_COLOR: u8 = 16;

/// `ChatGUIModule_c::ExpandChatTextArgs` [GUI 0x1008619b]: `%%` -> `%`, `%t` target name, `%f` fighting target, `%m` mouse target.
/// Anything else after a `%` stays as typed. Applied by every chat window to each line it shows and by the input commands to the text they send.
pub fn expand_chat_text_args(s: &str, ctx: &ZoneChatCtx) -> String {
    let name = |id: u32| (ctx.name_of)(id).unwrap_or_default();
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match it.peek().copied() {
            Some('%') => out.push('%'),
            Some('f') => match ctx.fighting {
                Some(id) => out += &name(id),
                None => out += "&lt;no fighting target&gt;",
            },
            Some('m') => out += &ctx.mouse.map(name).unwrap_or_default(),
            Some('t') => match ctx.target {
                Some(id) => out += &name(id),
                None => out += "&lt;no target&gt;",
            },
            _ => {
                out.push('%');
                continue;
            }
        }
        it.next();
    }
    out
}

/// Lines (with their window/group id) a received N3 chat message produces. On-screen texts (`ChatTextIIR_t` with the screen byte,
/// `FormatFeedbackIIR_t` modes 1/2) are not chat lines and yield nothing; see [`screen_text`].
pub fn routed(msg: &N3Chat, ctx: &ZoneChatCtx) -> Vec<ZoneText> {
    let (channel, color, text) = match msg {
        N3Chat::Text(t) if !t.screen => (t.channel, t.color, ldb::parse_string(ctx.texts, &t.text)),
        N3Chat::Feedback(f) => (f.channel, 0, ldb::text(ctx.texts, f.category, f.id)),
        N3Chat::Format(f) if ctx.header_is_char && f.mode != 1 && f.mode != 2 => {
            (f.channel, FORMAT_FEEDBACK_COLOR, ldb::parse_string(ctx.texts, &f.format))
        }
        _ => return Vec::new(),
    };
    vec![ZoneText { channel, line: ChatLine::new(ChatKind::Other(color_code_name(color)), text) }]
}

#[cfg(test)]
/// [`routed`] without the routing id.
pub fn lines(msg: &N3Chat, ctx: &ZoneChatCtx) -> Vec<ChatLine> {
    routed(msg, ctx).into_iter().map(|z| z.line).collect()
}

#[cfg(test)]
/// Text of a message the original shows on screen rather than in a chat window (AFCM `0x19` -> `RenderTextModule_t`, or the
/// `GlobalSignals_c` `+0x1d8` signal for `FormatFeedbackIIR_t` mode 1). Layout/duration of that overlay are not part of the chat work.
pub fn screen_text(msg: &N3Chat, ctx: &ZoneChatCtx) -> Option<String> {
    match msg {
        N3Chat::Text(t) if t.screen => Some(ldb::lossy(&t.text)),
        N3Chat::Format(f) if ctx.header_is_char && (f.mode == 1 || f.mode == 2) => Some(ldb::parse_string(ctx.texts, &f.format)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// Outgoing
// ---------------------------------------------------------------------------------------------------------------

/// The speech type byte of the chat buffer (`msg+4` in `FUN_100891a4` [GUI]), chosen by the slash command (`FUN_1009caf6` [GUI]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speech {
    /// `/say`, `/v`, plain text in the vicinity window (kind 0, AFCM `0x16d`).
    Say = 0,
    /// `/whisper`, `/w` (kind 1, AFCM `0x170`).
    Whisper = 1,
    /// `/shout`, `/s` (kind 2, AFCM `0x129`).
    Shout = 2,
    /// `/me` (kind 3, sent as a vicinity message).
    Emote = 3,
}

impl Speech {
    #[cfg(test)]
    /// Slash command -> speech type (`String::CompareNoCase`, as `FUN_1009caf6` tests them).
    pub fn from_command(cmd: &str) -> Option<Speech> {
        Some(match cmd.to_ascii_lowercase().as_str() {
            "/say" | "/v" => Speech::Say,
            "/whisper" | "/w" => Speech::Whisper,
            "/shout" | "/s" => Speech::Shout,
            "/me" => Speech::Emote,
            _ => return None,
        })
    }
}

/// Text bytes as the client holds them: Latin-1 when every char fits (the client's native 8-bit text) else UTF-8. [GUESS: encoding]
pub fn text_bytes(s: &str) -> Vec<u8> {
    if s.chars().all(|c| (c as u32) < 0x100) {
        s.chars().map(|c| c as u8).collect()
    } else {
        s.as_bytes().to_vec()
    }
}

/// The ptype-5 frame for `text` (already passed through [`expand_chat_text_args`]). `target` is the character/NPC currently
/// selected (`InputConfig_t` `+0xc0/+0xc4`, `{0,0}` when none): the GUI always sends it as the AFCM identity, for all three message types.
/// `None` when the text is longer than 1024 bytes (the client does not send it).
pub fn frame(seq: u16, char_id: u32, speech: Speech, text: &str, target: Identity) -> Option<Frame> {
    let kind = match speech {
        Speech::Say | Speech::Emote => TextKind::Vicinity,
        Speech::Whisper => TextKind::Whisper,
        Speech::Shout => TextKind::Shout,
    };
    let buf = wire::chat_buffer(&text_bytes(text), speech as u8)?;
    Some(text_frame(seq, char_id, text_payload(kind, target, &buf)))
}

#[cfg(test)]
pub fn vicinity_frame(seq: u16, char_id: u32, text: &str, target: Identity) -> Option<Frame> {
    frame(seq, char_id, Speech::Say, text, target)
}

#[cfg(test)]
pub fn shout_frame(seq: u16, char_id: u32, text: &str, target: Identity) -> Option<Frame> {
    frame(seq, char_id, Speech::Shout, text, target)
}

#[cfg(test)]
pub fn whisper_frame(seq: u16, char_id: u32, text: &str, target: Identity) -> Option<Frame> {
    frame(seq, char_id, Speech::Whisper, text, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ao_net::n3::chat::{encode_remote, ChatText, Feedback, FormatFeedback};

    fn client() -> Option<TextDb> {
        let d = std::path::PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
        TextDb::load(&d).ok()
    }

    fn fmt(t: &str, f: impl FnOnce(&mut ldb::Format)) -> String {
        let mut x = ldb::Format::new(t);
        f(&mut x);
        x.dump()
    }

    #[test]
    fn ldb_printf_and_plural() {
        assert_eq!(fmt("able in %02d:%02d:%02d", |f| { f.int(1).int(2).int(3); }), "able in 01:02:03");
        assert_eq!(fmt("%s has been added", |f| { f.string("Bob"); }), "Bob has been added");
        assert_eq!(fmt("100%% of %s", |f| { f.string("x"); }), "100% of x");
        assert_eq!(fmt("a##b", |_| {}), "a#b");
        // flags directly after '{', ':' ends them, a leading blank in the other alternative is required
        let t = "%d #1{1: item| items}";
        assert_eq!(fmt(t, |f| { f.int(1); }), "1 item");
        assert_eq!(fmt(t, |f| { f.int(2); }), "2 items");
        assert_eq!(fmt(t, |f| { f.int(0); }), "0 items");
        // "{ 1:buddy | buddies }" (text-db 20000/18838393): the blank after '{' ends the (empty) flags, so the last alternative always wins
        assert_eq!(fmt("Removing %d #1{ 1:buddy | buddies }.", |f| { f.int(1); }), "Removing 1 buddies .");
        // unfed tokens stay; runs of blanks collapse; leading blanks go
        assert_eq!(fmt("  a  %s  b", |_| {}), "a %s b");
    }

    #[test]
    fn remote_blob_expands_with_the_real_text_db() {
        let Some(db) = client() else { return };
        let blob = encode_remote(20000, 18838393, &[RfArg::Int(3)]);
        assert_eq!(ldb::parse_string(&db, &blob), "Removing 3 buddies .");
        // text around the blob, a nested string blob and a stray '~'
        let inner = encode_remote(20000, 32005140, &[RfArg::Str(b"Bob".to_vec())]);
        let outer = encode_remote(20000, 74098291, &[RfArg::Str(inner)]);
        let mut s = b"~ x ".to_vec();
        s.extend(&outer);
        s.extend(b" y");
        assert_eq!(ldb::parse_string(&db, &s), "~ x Currect motd: \"Bob has been added to your buddy-list.\". y");
        // an entry reference and a missing entry
        let b = encode_remote(20000, 32005140, &[RfArg::Entry(20000, 258810596)]);
        assert_eq!(ldb::parse_string(&db, &b), "Motd has been cleared. has been added to your buddy-list.");
        assert_eq!(ldb::parse_string(&db, &encode_remote(7, 9, &[])), "no LDBintern (7:9)");
    }

    fn ctx<'a>(db: &'a TextDb, names: &'a dyn Fn(u32) -> Option<String>) -> ZoneChatCtx<'a> {
        ZoneChatCtx { texts: db, name_of: names, header_is_char: true, target: Some(5), fighting: None, mouse: None }
    }

    #[test]
    fn lines_from_messages() {
        let Some(db) = client() else { return };
        let names = |id: u32| (id == 5).then(|| "Bob".to_owned());
        let c = ctx(&db, &names);
        let text = N3Chat::Text(ChatText { text: b"Hello %t".to_vec(), color: 5, screen: false, channel: 0x4000_0002 });
        let z = routed(&text, &c);
        assert_eq!(z.len(), 1);
        assert_eq!((z[0].channel, z[0].line.kind.color_name()), (0x4000_0002, "CCVicinityColor"));
        assert_eq!(z[0].line.text, "Hello %t"); // %t is expanded by the window sink, not here
        assert_eq!(expand_chat_text_args(&z[0].line.text, &c), "Hello Bob");

        let fb = N3Chat::Feedback(Feedback { channel: 3, category: 20000, id: 258810596 });
        assert_eq!(lines(&fb, &c), vec![ChatLine::new(ChatKind::Other("CCNoneColor"), "Motd has been cleared.")]);

        let blob = encode_remote(20000, 54583877, &[RfArg::Str(b"Carl".to_vec())]);
        let ff = N3Chat::Format(FormatFeedback { channel: 3, format: blob.clone(), mode: 0 });
        assert_eq!(lines(&ff, &c), vec![ChatLine::new(ChatKind::Other("CCYellow"), "Could not send message to offline player: Carl")]);
        // on-screen variants and non-character headers produce no chat line
        let screen = N3Chat::Format(FormatFeedback { channel: 3, format: blob, mode: 2 });
        assert!(lines(&screen, &c).is_empty());
        assert_eq!(screen_text(&screen, &c).as_deref(), Some("Could not send message to offline player: Carl"));
        let c2 = ZoneChatCtx { header_is_char: false, ..ctx(&db, &names) };
        assert!(lines(&ff, &c2).is_empty());
        let st = N3Chat::Text(ChatText { text: b"Boom".to_vec(), color: 0, screen: true, channel: 0 });
        assert!(lines(&st, &c).is_empty());
        assert_eq!(screen_text(&st, &c).as_deref(), Some("Boom"));
    }

    #[test]
    fn chat_text_args() {
        let db = TextDb::parse(b"MMDB\0\0\0\0".to_vec()).unwrap();
        let names = |id: u32| Some(format!("N{id}"));
        let c = ZoneChatCtx { fighting: Some(7), mouse: Some(9), target: None, ..ctx(&db, &names) };
        assert_eq!(expand_chat_text_args("100%% %t %f %m %x %", &c), "100% &lt;no target&gt; N7 N9 %x %");
        let none = ZoneChatCtx { fighting: None, mouse: None, ..ctx(&db, &names) };
        assert_eq!(expand_chat_text_args("%f|%m|%t", &none), "&lt;no fighting target&gt;||N5");
    }

    #[test]
    fn colour_table() {
        assert_eq!(color_code_name(5), "CCVicinityColor");
        assert_eq!(color_code_name(36), "white");
        assert_eq!(color_code_name(82), "CCChatCmdFeedbackInfo");
    }

    #[test]
    fn commands_map_to_speech() {
        assert_eq!(Speech::from_command("/W"), Some(Speech::Whisper));
        assert_eq!(Speech::from_command("/shout"), Some(Speech::Shout));
        assert_eq!(Speech::from_command("/me"), Some(Speech::Emote));
        assert_eq!(Speech::from_command("/v"), Some(Speech::Say));
        assert_eq!(Speech::from_command("/tell"), None);
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn outgoing_frames() {
        let target = Identity { kind: 0xC350, instance: 0x1234 };
        let f = vicinity_frame(9, 0x6584, "hi", target).unwrap();
        assert_eq!((f.ptype, f.seq, f.sender, f.receiver), (5, 9, 0x6584, 2));
        // type 3, target, len 5 = 2 + "hi" + kind, then [u16 len]["hi"][kind]
        assert_eq!(hex(&f.payload), "00000003".to_owned() + "0000c350" + "00001234" + "00000005" + "0002" + "6869" + "00");
        let s = shout_frame(1, 1, "x", Identity::default()).unwrap();
        assert_eq!(hex(&s.payload), "00000004".to_owned() + "0000000000000000" + "00000004" + "0001" + "78" + "02");
        let w = whisper_frame(1, 1, "é", target).unwrap();
        assert_eq!(&hex(&w.payload)[..8], "00000002");
        assert_eq!(hex(&w.payload[16..]), "0001e901"); // Latin-1, kind 1
        let e = frame(1, 1, Speech::Emote, "waves", target).unwrap();
        assert_eq!((&hex(&e.payload)[..8], *e.payload.last().unwrap()), ("00000003", 3));
        assert!(vicinity_frame(1, 1, &"a".repeat(1025), target).is_none());
        assert!(vicinity_frame(1, 1, &"a".repeat(1024), target).is_some());
        assert!(f.encode().is_ok());
    }
}
