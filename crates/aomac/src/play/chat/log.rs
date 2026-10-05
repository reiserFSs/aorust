//! Combat / feedback lines of the chat window: what the original turns game events into besides player chat.
//! Evidence and every address: docs/chat/log.md (Gamecode.dll `FUN_10012bd5` = the combat-log formatter, `FUN_10012b05` = the
//! GlobalSignals+0x17c text signal, GUI.dll `FUN_10083898` = its chat-window slot, ldb.dll = `LDBformat` / `RemoteFormat`).
//!
//! Every line leaves the game code as `(class, text, ColorCode)` ([`LogLine`]): `class` is the chat-window message class the
//! line is routed by (the table of `FUN_10083e53` [GUI], [`class`]), `ColorCode` selects the `TextColors.xml` colour
//! ([`color_name`], `ChatGUIModule_c::ColorCodeToHTMLColor` [GUI 0x10087860]); code 0 means "no `<font>`" and is `ChatKind::Other("")`.
//! The window wraps a line as `<div><font color=NAME>text</font></div>` (`FUN_1009b37f` [GUI], [`window_html`]).
#![allow(dead_code)]

use super::line::{ChatKind, ChatLine};
use ao_formats::screens::elf_hash;
use ao_net::msg::Identity;
use ao_net::n3::{dynel::Dynel, misc::Misc, world::World, Message, N3};

// ------------------------------------------------------------------------------------------------ classes / colours

/// Chat-window message classes (`FUN_10083e53` [GUI 0x10083e53] registers `name -> id` pairs; shown in the chat-config window).
pub mod class {
    pub const SYSTEM: u32 = 0x4000_0001;
    pub const VICINITY: u32 = 0x4000_0002;
    pub const TELL: u32 = 0x4000_0003;
    pub const YOUR_PETS: u32 = 0x4100_0000;
    pub const OTHER_PETS: u32 = 0x4100_0001;
    pub const ME_HIT_BY_ENVIRONMENT: u32 = 0x4200_0001;
    pub const ME_HIT_BY_NANO: u32 = 0x4200_0002;
    pub const YOUR_PET_HIT_BY_NANO: u32 = 0x4200_0003;
    pub const OTHER_HIT_BY_NANO: u32 = 0x4200_0004;
    pub const YOU_HIT_OTHER_WITH_NANO: u32 = 0x4200_0005;
    pub const ME_HIT_BY_MONSTER: u32 = 0x4200_0006;
    pub const ME_HIT_BY_PLAYER: u32 = 0x4200_0007;
    pub const YOU_HIT_OTHER: u32 = 0x4200_0008;
    pub const YOUR_PET_HIT_BY_OTHER: u32 = 0x4200_0009;
    pub const OTHER_HIT_BY_OTHER: u32 = 0x4200_000a;
    pub const ME_GOT_XP: u32 = 0x4200_000b;
    pub const ME_GOT_SK: u32 = 0x4200_000c;
    pub const YOUR_PET_HIT_BY_MONSTER: u32 = 0x4200_0011;
    pub const YOUR_MISSES: u32 = 0x4200_0012;
    pub const OTHER_MISSES: u32 = 0x4200_0013;
    pub const YOU_GAVE_HEALTH: u32 = 0x4200_0014;
    pub const ME_GOT_HEALTH: u32 = 0x4200_0015;
    pub const ME_GOT_NANO: u32 = 0x4200_0016;
    pub const YOU_GAVE_NANO: u32 = 0x4200_0017;
    pub const ME_CAST_NANO: u32 = 0x4200_0018;
    pub const TEAM_LOOT: u32 = 0x4200_001a;
    pub const VICINITY_LOOT: u32 = 0x4200_001b;
    pub const RESEARCH: u32 = 0x4200_001c;
}

/// `ColorCode_e` -> `TextColors.xml` name: the `{index, name}` table at [GUI 0x10268d38] (index 0 = `CCNoneColor`).
pub fn color_name(code: u32) -> &'static str {
    match code {
        0 => "",
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
        _ => "",
    }
}

/// `FUN_1009b37f` [GUI 0x1009b37f]: `<div>` + (`<font color=NAME>` when the code is non-zero) + text + (`</font>`) + `</div>`.
/// The original runs the text through `ChatGUIModule_c::ExpandChatTextArgs` [GUI 0x1008619b] first (the window's job).
pub fn window_html(color: &str, text: &str) -> String {
    if color.is_empty() {
        format!("<div>{text}</div>")
    } else {
        format!("<div><font color={color}>{text}</font></div>")
    }
}

// ------------------------------------------------------------------------------------------------ LDBformat / RemoteFormat

/// An argument of [`ldb_format`] (`LDBformat::Feed(int|unsigned|float|char const*)`, ldb.dll 0x10004c8b/da7/ec3/fd1).
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    N(i32),
    F(f32),
    S(String),
}

impl From<i32> for Arg {
    fn from(v: i32) -> Self {
        Arg::N(v)
    }
}
impl From<&str> for Arg {
    fn from(v: &str) -> Self {
        Arg::S(v.to_string())
    }
}
impl From<String> for Arg {
    fn from(v: String) -> Self {
        Arg::S(v)
    }
}

/// `LDBformat::MakeFlagBit` [ldb 0x100040c8]: letters A..Z are bits 0..25, digits 0..5 bits 26..31 (table at 0x1000b1f8); `Feed(int)` raises
/// bit 26 for 0 and bit 27 for 1 (`LDBformat::Feed` 0x10004c8b).
fn flag_bit(c: u8) -> u32 {
    let u = c.to_ascii_uppercase();
    let i = if u < b'A' { u as i32 - 0x16 } else { u as i32 - 0x41 };
    if (0..32).contains(&i) {
        1 << i
    } else {
        0
    }
}

/// `LDBformat::Init` [ldb 0x100053f2] + `CreateToken` [0x1000535a]: `(argument index, token text)`; index 0 = literal.
fn ldb_tokens(t: &[u8]) -> Vec<(usize, String)> {
    let at = |i: usize| t.get(i).copied().unwrap_or(0);
    let mut toks = Vec::new();
    let create = |toks: &mut Vec<(usize, String)>, a: usize, mut b: usize, idx: usize| {
        if a <= b {
            if at(b) == 0 {
                b = b.wrapping_sub(1);
            }
            let s = if b >= a { &t[a..=b.min(t.len() - 1)] } else { &[][..] };
            toks.push((idx, s.iter().map(|&c| c as char).collect()));
        }
    };
    let (mut n, mut lit, mut p) = (1usize, 0usize, 0usize);
    while at(p) != 0 {
        if at(p) == b'%' {
            if p != 0 {
                create(&mut toks, lit, p - 1, 0);
            }
            let mut q = p + 1;
            if at(q) == b'%' {
                create(&mut toks, q, q, 0);
            } else {
                while at(q) != 0 && b"-+ 0#123456789.hlL".contains(&at(q)) {
                    q += 1;
                }
                create(&mut toks, p, q, n);
                n += 1;
            }
            lit = q + 1;
            p = q + 1;
        } else if at(p) == b'#' {
            create(&mut toks, lit, p.wrapping_sub(1), 0);
            lit = p + 1;
            let c = at(p + 1);
            if c == b'#' {
                p += 1; // the second '#' is scanned again as a '#'
                continue;
            }
            if c == 0 {
                lit = p;
                p += 1;
                break;
            }
            let idx = (c as i32 - 0x30).max(0) as usize;
            let c2 = at(p + 2);
            if c2 == 0 {
                lit = p;
                p += 2;
                break;
            }
            if c2 == b'{' {
                let mut q = p + 2;
                while at(q) != b'}' && at(q) != 0 {
                    q += 1;
                }
                create(&mut toks, p + 2, q, idx);
                lit = q + 1;
                p = q + 1;
            } else if c2 == b':' {
                lit = p + 3;
                p += 3;
            } else {
                lit = p;
                p += 2;
            }
        } else {
            p += 1;
        }
    }
    create(&mut toks, lit, p, 0);
    toks
}

/// `LDBformat::FeedInternal` [ldb 0x100046ab]: `{FLAGS:text|FLAGS:text|...}`, the option with the highest popcount of
/// `option_flags & fed_flags` wins (later options win ties); a text after a space starts at that space.
fn pick_variant(tok: &str, flags: u32) -> String {
    let b = tok.as_bytes();
    if b.first() != Some(&b'{') {
        return tok.to_string();
    }
    let mut opts: Vec<(u32, usize, usize)> = Vec::new();
    let mut p = 0usize;
    loop {
        p += 1;
        if p >= b.len() {
            break;
        }
        let mut f = 0u32;
        while p < b.len() && b[p] != b':' && b[p] != b' ' {
            f |= flag_bit(b[p]);
            p += 1;
        }
        if p >= b.len() {
            return tok.to_string();
        }
        if b[p] != b' ' {
            p += 1;
        }
        let start = p;
        while p < b.len() && b[p] != b'|' && b[p] != b'}' {
            p += 1;
        }
        if p >= b.len() {
            return tok.to_string();
        }
        opts.push((f, start, p));
    }
    let (mut best, mut bi) = (-1i32, 0usize);
    for (i, &(f, _, _)) in opts.iter().enumerate() {
        let s = (f & flags).count_ones() as i32;
        if best <= s {
            best = s;
            bi = i;
        }
    }
    opts.get(bi).map_or_else(String::new, |&(_, s, e)| tok[s..e].to_string())
}

/// C `snprintf` for one `%` token with the conversions the client's texts use (d i u x X c s f).
fn cfmt(spec: &str, a: &Arg) -> String {
    let b = spec.as_bytes();
    let (mut left, mut zero, mut plus, mut space) = (false, false, false, false);
    let mut i = 1;
    while i < b.len() {
        match b[i] {
            b'-' => left = true,
            b'0' => zero = true,
            b'+' => plus = true,
            b' ' => space = true,
            b'#' => {}
            _ => break,
        }
        i += 1;
    }
    let mut width = 0usize;
    while i < b.len() && b[i].is_ascii_digit() {
        width = width * 10 + (b[i] - b'0') as usize;
        i += 1;
    }
    let mut prec: Option<usize> = None;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let mut v = 0;
        while i < b.len() && b[i].is_ascii_digit() {
            v = v * 10 + (b[i] - b'0') as usize;
            i += 1;
        }
        prec = Some(v);
    }
    while i < b.len() && matches!(b[i], b'h' | b'l' | b'L') {
        i += 1;
    }
    let conv = b.get(i).copied().unwrap_or(b's');
    let int = |a: &Arg| match a {
        Arg::N(v) => *v,
        Arg::F(f) => *f as i32,
        Arg::S(_) => 0,
    };
    let mut body = match (conv, a) {
        (b's', Arg::S(s)) => prec.map_or(s.clone(), |p| s.chars().take(p).collect()),
        (b's', Arg::N(v)) => format!("int_value<{v}>"),
        (b's', Arg::F(f)) => format!("{f}"),
        (b'u', _) => (int(a) as u32).to_string(),
        (b'x', _) => format!("{:x}", int(a) as u32),
        (b'X', _) => format!("{:X}", int(a) as u32),
        (b'c', _) => ((int(a) as u8) as char).to_string(),
        (b'f', Arg::F(f)) => format!("{:.*}", prec.unwrap_or(6), f),
        (b'f', _) => format!("{:.*}", prec.unwrap_or(6), int(a) as f32),
        (_, _) => {
            let v = int(a);
            if plus && v >= 0 {
                format!("+{v}")
            } else if space && v >= 0 {
                format!(" {v}")
            } else {
                v.to_string()
            }
        }
    };
    if body.chars().count() < width {
        let pad = width - body.chars().count();
        if left {
            body.push_str(&" ".repeat(pad));
        } else if zero && conv != b's' {
            let neg = body.starts_with('-');
            let digits = if neg { &body[1..] } else { &body[..] };
            body = format!("{}{}{}", if neg { "-" } else { "" }, "0".repeat(pad), digits);
        } else {
            body = format!("{}{}", " ".repeat(pad), body);
        }
    }
    body
}

/// `LDBformat(template)` + `Feed(args...)` + `Dump()` (ldb.dll 0x100054da / 0x10004c8b.. / 0x10004885): `%` conversions take the next
/// argument, `#N{..}` picks a variant by the value of argument N; `Dump` drops leading spaces and collapses runs of spaces.
pub fn ldb_format(template: &str, args: &[Arg]) -> String {
    let latin: Vec<u8> = template.chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }).collect();
    let mut toks = ldb_tokens(&latin);
    for (k, a) in args.iter().enumerate() {
        let n = k + 1;
        for (idx, text) in toks.iter_mut() {
            if *idx != n {
                continue;
            }
            if text.starts_with('%') {
                *text = cfmt(text, a);
            } else {
                let flags = match a {
                    Arg::N(0) => 1 << 26,
                    Arg::N(1) => 1 << 27,
                    _ => 0,
                };
                *text = pick_variant(text, flags);
            }
        }
    }
    let mut out = String::new();
    let mut skip = true;
    for (_, t) in &toks {
        for c in t.chars() {
            if skip {
                if c != ' ' {
                    skip = false;
                    out.push(c);
                }
            } else {
                if c == ' ' {
                    skip = true;
                }
                out.push(c);
            }
        }
    }
    out
}

fn b85(s: &[u8]) -> Option<u32> {
    if s.len() < 5 {
        return None;
    }
    let mut v = 0u32;
    for &c in &s[..5] {
        if !(0x21..=0x75).contains(&c) {
            return None;
        }
        v = v.wrapping_mul(85).wrapping_add(c as u32 - 0x21);
    }
    Some(v)
}

/// Length prefix of a `F`/`s` span (`FUN_10004013` / `FUN_1000403f` [ldb]): a UTF-8 coded number `len + 1`; returns `(prefix bytes, len)`.
fn span_len(s: &[u8]) -> Option<(usize, usize)> {
    let c = *s.first()?;
    if c == 0x92 {
        return Some((1, 0x26));
    }
    let lead = !(c & 0x80 != 0 && c & 0xc0 != 0xc0);
    let nb = if !lead { 1 } else { (((0xe500_0000u32 >> ((c as u32 >> 3) & 0x1e)) & 3) + 1) as usize };
    let cont = |i: usize| s.get(i).map(|b| b & 0xc0 == 0x80);
    let cp: u32 = if !lead || c & 0x80 == 0 || cont(1) != Some(true) {
        c as u32
    } else if c & 0x20 == 0 {
        ((c as u32 & 0x1f) << 6) | (s[1] as u32 & 0x3f)
    } else if cont(2) != Some(true) {
        c as u32
    } else if c & 0x10 != 0 {
        match s.get(3) {
            Some(&d) => (((((c as u32 & 7) << 6) | (s[1] as u32 & 0x3f)) << 6 | (s[2] as u32 & 0x3f)) << 6) | (d as u32 & 0x3f),
            None => return None,
        }
    } else {
        (((c as u32 & 0xf) << 6 | (s[1] as u32 & 0x3f)) << 6) | (s[2] as u32 & 0x3f)
    };
    if cp == 0 {
        None
    } else {
        Some((nb, cp as usize - 1))
    }
}

/// `RemoteFormat::ParseString` [ldb 0x1000593d] + `RemoteFormat::Init` [0x10005565]: plain text with embedded `~&AAAAABBBBB<args>~` spans.
/// `A`,`B` are base-85 `(category, id)` of the template ([`TextCtx`]); arguments: `i`/`u`/`f` + 5 base-85 chars, `R` + two such words
/// (a nested text id), `s`/`F` + length-prefixed nested string. Used by `FormatFeedbackIIR_t` [GC 0x10039341] and by the chat-server
/// vicinity text (`HandleVicinityMessage` [GUI 0x10086728]).
pub fn remote_parse(text: &dyn Fn(u32, u32) -> Option<String>, s: &[u8]) -> String {
    let mut out = Vec::<u8>::new();
    let mut p = 0usize;
    while p < s.len() && s[p] != 0 {
        if s[p] == b'~' && s.get(p + 1) == Some(&b'&') {
            if let Some((res, used)) = remote_span(text, &s[p..]) {
                out.extend(res.chars().map(|c| if (c as u32) < 256 { c as u32 as u8 } else { b'?' }));
                p += used;
                continue;
            }
        }
        out.push(s[p]);
        p += 1;
    }
    out.iter().map(|&c| c as char).collect()
}

fn remote_span(text: &dyn Fn(u32, u32) -> Option<String>, s: &[u8]) -> Option<(String, usize)> {
    if s.len() <= 11 {
        return None;
    }
    let (a, b) = (b85(&s[2..])?, b85(s.get(7..)?)?);
    let tpl = text(a, b).unwrap_or_default();
    let mut args: Vec<Arg> = Vec::new();
    let mut i = 12usize;
    let mut used = 0usize;
    while i < s.len() {
        match s[i] {
            b'F' | b's' => {
                let Some((nb, len1)) = s.get(i + 1..).and_then(span_len) else { break };
                let start = i + 1 + nb;
                let Some(sub) = s.get(start..start + len1) else { break };
                let parsed = remote_parse(text, sub);
                let raw: String = sub.iter().map(|&c| c as char).collect();
                args.push(Arg::S(if s[i] == b's' && parsed.is_empty() { raw } else { parsed }));
                i = start + len1;
            }
            b'R' => {
                let (Some(x), Some(y)) = (s.get(i + 1..).and_then(b85), s.get(i + 6..).and_then(b85)) else { break };
                args.push(Arg::S(text(x, y).unwrap_or_default()));
                i += 11;
            }
            c @ (b'f' | b'i' | b'u') => {
                let Some(v) = s.get(i + 1..).and_then(b85) else { break };
                args.push(match c {
                    b'f' => Arg::F(f32::from_bits(v)),
                    _ => Arg::N(v as i32),
                });
                i += 6;
            }
            b'~' => {
                used = i + 1;
                break;
            }
            _ => break,
        }
    }
    if used == 0 {
        return None;
    }
    Some((ldb_format(&tpl, &args), used))
}

// ------------------------------------------------------------------------------------------------ context

/// `ChatFilterEnabled` / `ChatFilterRules` (`/chatfilter`, `FUN_100b8d4e` [GUI]); `FUN_10084f9e` [GUI 0x10084f9e] drops a line when any rule
/// matches its text with the client's `RegExp` (Utils.dll, not ported: `is_match` decides; the default is a case-sensitive substring test,
/// `[GUESS]` for the regular-expression dialect).
pub struct ChatFilter {
    pub enabled: bool,
    pub rules: Vec<String>,
    pub is_match: fn(&str, &str) -> bool,
}

impl Default for ChatFilter {
    fn default() -> Self {
        Self { enabled: false, rules: Vec::new(), is_match: |rule, text| text.contains(rule) }
    }
}

impl ChatFilter {
    pub fn drops(&self, text: &str) -> bool {
        self.enabled && self.rules.iter().any(|r| (self.is_match)(r, text))
    }
}

/// What the formatter needs to know about the world (the original reads these from live `SimpleChar_t` dynels).
pub struct LogCtx<'a> {
    /// The client's character (`n3Dynel_t::IsClientChar`, dynel byte +0x140).
    pub own: Identity,
    /// Dynel name (`vtable +0x34` of the dynel's `+0xe8` object); `None` = the dynel is not in the client's world (the original then drops the event).
    pub name: &'a dyn Fn(Identity) -> Option<String>,
    /// `LDBface::GetText(category, id)`; keys are `ElfHash`ed ids (`TextDb::by_id`).
    pub text: &'a dyn Fn(u32, u32) -> Option<String>,
    /// `SimpleChar_t` byte +0x21c != 0 (a non-player character).
    pub is_npc: &'a dyn Fn(Identity) -> bool,
    /// `+0x21c != 0 && stat flag 0x8000000 && FUN_100523c3` (a pet owned by the client's character).
    pub is_own_pet: &'a dyn Fn(Identity) -> bool,
    /// Nano record name by nano instance id (`FUN_10082998` object, vtable +0x34).
    pub nano_name: &'a dyn Fn(i32) -> Option<String>,
    /// Current stat of a dynel (`GetStat(stat, ..)`), used for old values and the damage-type stats 0x153 / 0x1b4.
    pub stat: &'a dyn Fn(Identity, i32) -> Option<i32>,
    pub filter: &'a ChatFilter,
}

impl LogCtx<'_> {
    fn is_own(&self, id: Identity) -> bool {
        id == self.own
    }
    fn nm(&self, id: Identity) -> String {
        (self.name)(id).unwrap_or_default()
    }
    fn known(&self, id: Identity) -> bool {
        (self.name)(id).is_some()
    }
    /// `FUN_1003807c` + `String`: a `Feedback_*` key of text category 110.
    fn fb(&self, key: &str) -> String {
        (self.text)(110, elf_hash(key)).unwrap_or_default()
    }
    fn key(&self, cat: u32, key: &str) -> String {
        (self.text)(cat, elf_hash(key)).unwrap_or_default()
    }
}

/// `FUN_10036adf` [GC 0x10036adf]: damage-type name by stat id (table built in `FUN_100324d2` [GC 0x10033a85]).
pub fn damage_type_name(d: i32) -> String {
    match d {
        90 => "projectile",
        91 => "melee",
        92 => "energy",
        93 => "chemical",
        94 => "radiation",
        95 => "cold",
        168 => "nano",
        97 => "fire",
        96 => "poison",
        27 => "unknown",
        474 => "fall",
        489 => "backstab",
        _ => return format!("Missing damagetype: {d}"),
    }
    .to_string()
}

// ------------------------------------------------------------------------------------------------ events

/// Game events that produce log lines. Built by [`from_n3`] (or by the hub for local actions such as [`LogEvent::CastNano`]).
#[derive(Debug, Clone, PartialEq)]
pub enum LogEvent {
    /// `AttackInfoIIR_t` 46002F16: `attacker` = frame header dynel, `victim` = `other`. `mode` = `unk_30` (3 normal, 4 critical, 2 glancing).
    Hit { attacker: Identity, victim: Identity, damage: i32, mode: i32 },
    /// `MissedAttackInfoIIR_t` 5C654B28: `attacker` = header = `source`; `stat` 0 = none else `Stat_e` of the special attack.
    Miss { attacker: Identity, victim: Identity, stat: i32 },
    /// `SpecialAttackInfoIIR_t` 754F1115.
    SpecialHit { attacker: Identity, victim: Identity, damage: i32, special: i32 },
    /// `AbsorbIIR_t` 264E5F61.
    Absorb { who: Identity, amount: i32, dtype: i32 },
    /// `HealthDamageIIR_t` 3710256C: `delta` < 0 = damage; `attacker.kind == 0` = none.
    HealthDamage { who: Identity, delta: i32, dtype: i32, attacker: Identity, nano: i32 },
    /// `ReflectAttackIIR_t` 1C3A4F77.
    Reflect { who: Identity, amount: i32, attacker: Identity },
    /// `ShieldAttackIIR_t` 25192476.
    DamageShield { who: Identity, amount: i32, attacker: Identity },
    /// `StatIIR_t` 2B333D6E, one pair: new value of `stat` on `who` (old value from [`LogCtx::stat`]).
    Stat { who: Identity, stat: i32, new: i32 },
    /// `NewLevelIIR_t` 7F405A16 (eight `i32`: level, ip, xp, ..., title level `+0x2c`, ..., xp gain `+0x34`).
    NewLevel { who: Identity, f: [i32; 8] },
    /// `ShadowLevelIIR_t` 3C1E2803 (same eight `i32`, SK gain `+0x30`, title level `+0x34`).
    ShadowLevel { who: Identity, f: [i32; 8] },
    /// `CharacterActionIIR_t` 5E477770.
    Action { who: Identity, action: i32, param: i32, a: Identity, b: Identity },
    /// `FeedbackIIR_t` 50544D19: text `(cat, id)` shown in `group`.
    Feedback { group: i32, cat: u32, id: u32 },
    /// `FormatFeedbackIIR_t` 206B4B73.
    FormatFeedback { who: Identity, group: i32, text: Vec<u8>, mode: i32 },
    /// `N3Msg_CastNanoSpell` [GC 0x1001b54b], local: the client starts casting `nano_name`.
    CastNano { who: Identity, nano_name: String },
}

/// One line with the chat-window class it is routed by.
#[derive(Debug, Clone, PartialEq)]
pub struct LogLine {
    /// `class::*` (`FUN_10083898` falls back to [`class::SYSTEM`] when the class has no window; legacy ids < 0x40000000 select a window by number).
    pub class: u32,
    pub line: ChatLine,
}

fn be32(b: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn ident(b: &[u8], o: usize) -> Option<Identity> {
    Some(Identity { kind: be32(b, o)?, instance: be32(b, o + 4)? })
}

const NEW_LEVEL: u32 = 0x7F40_5A16;
const SHADOW_LEVEL: u32 = 0x3C1E_2803;
const ABSORB: u32 = 0x264E_5F61;
const HEALTH_DAMAGE: u32 = 0x3710_256C;
const REFLECT_ATTACK: u32 = 0x1C3A_4F77;
const SHIELD_ATTACK: u32 = 0x2519_2476;
const FEEDBACK: u32 = 0x5054_4D19;
const FORMAT_FEEDBACK: u32 = 0x206B_4B73;

/// Map a decoded zone N3 message to a log event. Messages `ao_net` has no decoder for are read from the raw body (`N3::Unknown`) with
/// the layouts of their `ReadSubClass` functions (docs/chat/log.md §wire).
pub fn from_n3(m: &Message) -> Option<LogEvent> {
    let who = m.header.target;
    match &m.body {
        N3::Misc(Misc::AttackInfo(a)) => Some(LogEvent::Hit { attacker: who, victim: a.other, damage: a.damage, mode: a.unk_30 }),
        N3::Misc(Misc::MissedAttackInfo(a)) => Some(LogEvent::Miss { attacker: a.source, victim: a.target, stat: a.stat }),
        N3::Misc(Misc::SpecialAttackInfo(a)) => Some(LogEvent::SpecialHit { attacker: who, victim: a.target, damage: a.damage, special: a.special }),
        N3::World(World::CharacterAction(c)) => {
            Some(LogEvent::Action { who, action: c.action, param: c.param, a: c.identity_a, b: c.identity_b })
        }
        N3::Unknown(b) => match m.header.msg_type {
            ABSORB => Some(LogEvent::Absorb { who, amount: be32(b, 0)?, dtype: be32(b, 4)? }),
            HEALTH_DAMAGE => Some(LogEvent::HealthDamage {
                who,
                delta: be32(b, 4)?,
                dtype: be32(b, 8)?,
                attacker: ident(b, 16)?,
                nano: be32(b, 24)?,
            }),
            REFLECT_ATTACK => Some(LogEvent::Reflect { who, amount: be32(b, 0)?, attacker: ident(b, 4)? }),
            SHIELD_ATTACK => Some(LogEvent::DamageShield { who, amount: be32(b, 0)?, attacker: ident(b, 4)? }),
            NEW_LEVEL | SHADOW_LEVEL => {
                let mut f = [0i32; 8];
                for (i, v) in f.iter_mut().enumerate() {
                    *v = be32(b, 4 * i)?;
                }
                Some(if m.header.msg_type == NEW_LEVEL { LogEvent::NewLevel { who, f } } else { LogEvent::ShadowLevel { who, f } })
            }
            FEEDBACK => Some(LogEvent::Feedback { group: be32(b, 0)?, cat: be32(b, 4)? as u32, id: be32(b, 8)? as u32 }),
            FORMAT_FEEDBACK => {
                let group = be32(b, 0)?;
                let len = u16::from_be_bytes(b.get(4..6)?.try_into().ok()?) as usize;
                let text = if len == 0 || len >= 0x8000 { Vec::new() } else { b.get(6..6 + len)?.to_vec() };
                let mode = be32(b, 6 + if len >= 0x8000 { 0 } else { len })?;
                Some(LogEvent::FormatFeedback { who, group, text, mode })
            }
            _ => None,
        },
        _ => None,
    }
}

/// Every event of a message: [`from_n3`], and for `StatIIR_t` one [`LogEvent::Stat`] per `(stat, new value)` pair (the client iterates
/// a `std::map`, so stat-id order and the last duplicate wins).
pub fn events(m: &Message) -> Vec<LogEvent> {
    match &m.body {
        N3::Dynel(Dynel::Stat(s)) => {
            let mut map = std::collections::BTreeMap::new();
            for &(stat, new) in &s.stats {
                map.insert(stat, new);
            }
            map.into_iter().map(|(stat, new)| LogEvent::Stat { who: m.header.target, stat, new }).collect()
        }
        _ => from_n3(m).into_iter().collect(),
    }
}

// ------------------------------------------------------------------------------------------------ FUN_10012bd5

/// The arguments of `FUN_10012bd5(category, dynel, amount, other, text, damage type, mode, nano)` [GC 0x10012bd5].
struct Call {
    cat: u32,
    /// `[0xc]` (`iVar2`): the dynel the message is about (the victim for hits).
    d: Identity,
    /// `[0x10]`: amount; 0 = no line.
    amount: i32,
    /// `[0x14]`: the other party, if any.
    o: Option<Identity>,
    /// `[0x18]`: a name string (special attack / nano).
    s18: Option<String>,
    /// `[0x1c]`: damage type stat (0 -> 0x1b).
    dtype: i32,
    /// `[0x20]`: 4 = critical, 2 = glancing.
    mode: i32,
    /// `[0x24]`: nano instance id (0 = none).
    nano: i32,
}

impl Call {
    fn new(cat: u32, d: Identity, amount: i32) -> Self {
        Self { cat, d, amount, o: None, s18: None, dtype: 0, mode: 0, nano: 0 }
    }
}

struct Out {
    class: u32,
    color: u32,
    text: String,
}

fn n(v: i32) -> Arg {
    Arg::N(v)
}
fn s(v: impl Into<String>) -> Arg {
    Arg::S(v.into())
}

fn combat_text(c: &Call, ctx: &LogCtx) -> Option<Out> {
    use class as k;
    if c.amount == 0 {
        return None;
    }
    let amt = c.amount;
    let dtype = if c.dtype == 0 { 0x1b } else { c.dtype };
    let dtn = damage_type_name(dtype);
    let nano = if c.nano != 0 { Some((ctx.nano_name)(c.nano).unwrap_or_default()) } else { None };
    let d_name = || ctx.nm(c.d);
    // "[0x14] == 0 || [0x14] == dynel": no distinct other party
    let other = c.o.filter(|&o| o != c.d);
    let f = |key: &str, a: &[Arg]| ldb_format(&ctx.fb(key), a);
    let (class, color, text): (u32, u32, String) = match c.cat {
        0x1a => {
            let t = if dtype == 0x1da {
                f("Feedback_FallDamage", &[n(amt)])
            } else if let Some(nn) = &nano {
                f("Feedback_AttackedByForPointsOfDamage", &[s(nn.as_str()), n(amt), s(dtn)])
            } else if let Some(o) = other {
                f("Feedback_YouWereAttackedByNanobotsFrom", &[s(ctx.nm(o)), n(amt), s(dtn)])
            } else {
                f("Feedback_AttackedByNanobotsForPointsOfDamage", &[n(amt), s(dtn)])
            };
            (k::ME_HIT_BY_NANO, 0x15, t)
        }
        0x1b | 0x1c => {
            let pet = c.cat == 0x1c;
            let t = if dtype == 0x1da {
                f("Feedback_TookPointsOfFallDamage", &[s(d_name()), n(amt)])
            } else if let Some(nn) = &nano {
                match c.o {
                    None => f("Feedback_WasAttackedByForPointsOfDamage", &[s(d_name()), s(nn.as_str()), n(amt), s(dtn)]),
                    Some(o) => f("Feedback_WasAttackedByForPointsOfDamageFrom", &[s(d_name()), s(nn.as_str()), s(ctx.nm(o)), n(amt), s(dtn)]),
                }
            } else if let Some(o) = other {
                f("Feedback_WasAttackedByNanobotsFrom", &[s(d_name()), s(ctx.nm(o)), n(amt), s(dtn)])
            } else {
                f("Feedback_WasAttackedByNanobots", &[s(d_name()), n(amt), s(dtn)])
            };
            (if pet { k::YOUR_PET_HIT_BY_NANO } else { k::OTHER_HIT_BY_NANO }, if pet { 0x1b } else { 0x16 }, t)
        }
        0x1d => {
            let t = match other {
                None => f("Feedback_WereHitForPointsOfDamage", &[n(amt)]),
                Some(o) => f("Feedback_HitYouForPointsOfDamage", &[s(ctx.nm(o)), n(amt), s(dtn)]),
            };
            (k::ME_HIT_BY_MONSTER, 0x17, t)
        }
        0x1e => {
            let t = match c.o {
                None => f("Feedback_PlayerHitYou", &[n(amt)]),
                Some(o) => f("Feedback_PlayerHitYouForPointsOfDamage", &[s(ctx.nm(o)), n(amt), s(dtn)]),
            };
            (k::ME_HIT_BY_PLAYER, 0x18, t)
        }
        0x1f => (k::YOU_HIT_OTHER, 0x19, f("Feedback_HitWithSpecial", &[s(d_name()), n(amt), s(dtn)])),
        0x20 | 0x21 => {
            let t = match c.o {
                None => f("Feedback_SomethingHitOther", &[s(d_name()), n(amt), s(dtn)]),
                Some(o) => f("Feedback_OtherHitOther", &[s(ctx.nm(o)), s(d_name()), n(amt), s(dtn)]),
            };
            if c.cat == 0x21 {
                (k::YOUR_PET_HIT_BY_OTHER, 0x1b, t)
            } else {
                (k::OTHER_HIT_BY_OTHER, 0x1a, t)
            }
        }
        0x22 => (k::ME_GOT_HEALTH, 0x1c, f("Feedback_HealedForPoints", &[n(amt)])),
        0x24 => {
            let t = if amt < 0 { f("Feedback_LostXP", &[n(amt.wrapping_abs())]) } else { f("Feedback_ReceivedXP", &[n(amt)]) };
            (k::ME_GOT_XP, 0x1d, t)
        }
        0x25 | 0x27 => {
            let (some, you) = if c.cat == 0x25 {
                ("Feedback_SomeonesDamageShieldHitYou", "Feedback_YouWereHitByDamageShield")
            } else {
                ("Feedback_SomeonesReflectShieldHitYou", "Feedback_YouWereHitByReflectShield")
            };
            match c.o {
                None => (k::ME_HIT_BY_MONSTER, 0x17, f(some, &[n(amt)])),
                Some(o) => {
                    let cls = if (ctx.is_npc)(o) { k::ME_HIT_BY_MONSTER } else { k::ME_HIT_BY_PLAYER };
                    (cls, 0x17, f(you, &[n(amt), s(ctx.nm(o))]))
                }
            }
        }
        0x26 | 0x28 => {
            let (something, other_k, yours) = if c.cat == 0x26 {
                ("Feedback_SomethingHitOtherWithDamageShield", "Feedback_OtherDamageShieldHitOther", "Feedback_YourDamageShieldHitOther")
            } else {
                ("Feedback_SomethingHitOtherWithReflectShield", "Feedback_OtherReflectShieldHitOther", "Feedback_YourReflectShieldHitOther")
            };
            match c.o {
                None => (k::OTHER_HIT_BY_OTHER, 0x1a, f(something, &[s(d_name()), n(amt)])),
                Some(o) if !ctx.is_own(o) => (k::OTHER_HIT_BY_OTHER, 0x1a, f(other_k, &[s(ctx.nm(o)), s(d_name()), n(amt)])),
                Some(_) => (k::YOU_HIT_OTHER, 0x1a, f(yours, &[s(d_name()), n(amt)])),
            }
        }
        0x2c => (k::ME_HIT_BY_ENVIRONMENT, 0x1a, f("Feedback_MeHitByToxic", &[n(amt)])),
        0x30 => {
            let (Some(o), Some(sp)) = (c.o, &c.s18) else { return None };
            (k::ME_HIT_BY_PLAYER, 0x17, f("Feedback_HitYouForPointsOfDamage", &[s(ctx.nm(o)), n(amt), s(sp.as_str())]))
        }
        0x31 | 0x33 => {
            let Some(sp) = &c.s18 else { return None };
            let t = match c.o {
                None => f("Feedback_SomethingHitOther", &[s(d_name()), n(amt), s(sp.as_str())]),
                Some(o) => f("Feedback_MonsterHitWithSpecial", &[s(ctx.nm(o)), s(d_name()), n(amt), s(sp.as_str())]),
            };
            if c.cat == 0x33 {
                (k::YOUR_PET_HIT_BY_OTHER, 0x1b, t)
            } else {
                (k::OTHER_HIT_BY_OTHER, 0x1a, t)
            }
        }
        0x32 => {
            let Some(sp) = &c.s18 else { return None };
            (k::YOU_HIT_OTHER, 0x19, f("Feedback_HitWithSpecial", &[s(d_name()), n(amt), s(sp.as_str())]))
        }
        0x34 | 0x35 => {
            let t_you = |cls: u32| (cls, 0x17, f("Feedback_YouAbsorbedDamage", &[n(amt), s(dtn.as_str())]));
            if c.cat == 0x34 {
                match c.o {
                    None => t_you(k::ME_HIT_BY_MONSTER),
                    Some(o) => t_you(if (ctx.is_npc)(o) { k::ME_HIT_BY_MONSTER } else { k::ME_HIT_BY_PLAYER }),
                }
            } else {
                match c.o {
                    None => (k::OTHER_HIT_BY_OTHER, 0x1a, f("Feedback_SomeoneAbsorbedDamage", &[n(amt), s(dtn.as_str())])),
                    Some(o) => {
                        let cls = if ctx.is_own(o) { k::YOU_HIT_OTHER } else { k::OTHER_HIT_BY_OTHER };
                        (cls, 0x1a, f("Feedback_OtherAbsorbedDamage", &[s(ctx.nm(o)), n(amt), s(dtn.as_str())]))
                    }
                }
            }
        }
        0x3b => {
            let src = c.o;
            let src_own = src.is_some_and(|o| ctx.is_own(o));
            if !src_own {
                if !ctx.is_own(c.d) {
                    return None;
                }
                let name = src.map(|o| ctx.nm(o)).unwrap_or_default();
                let t = match &c.s18 {
                    None => f("Feedback_OtherTriedToHitMissed", &[s(name)]),
                    Some(sp) => ldb_format(&ctx.key(110, "OtherTriesToAttackWithSpecialButMisses"), &[s(name), s(sp.as_str())]),
                };
                (k::OTHER_MISSES, 0, t)
            } else {
                let t = match &c.s18 {
                    None => f("Feedback_YouTriedToHitMissed", &[s(d_name())]),
                    Some(sp) => ldb_format(&ctx.key(110, "TryToAttackWithSpecialButMiss"), &[s(d_name()), s(sp.as_str())]),
                };
                (k::YOUR_MISSES, 0, t)
            }
        }
        0x3e => {
            let t = if amt < 0 { f("Feedback_LostSK", &[n(amt.wrapping_abs())]) } else { f("Feedback_GainedSK", &[n(amt)]) };
            (k::ME_GOT_SK, 0x1d, t)
        }
        0x3f => (k::YOUR_PET_HIT_BY_MONSTER, 0x1a, f("Feedback_PetHitByToxic", &[s(d_name()), n(amt)])),
        0x40 => {
            let t = match &nano {
                None => f("Feedback_YouHitOtherByNanobots", &[s(d_name()), n(amt), s(dtn)]),
                Some(nn) => f("Feedback_YouHitOtherWith", &[s(d_name()), s(nn.as_str()), n(amt), s(dtn)]),
            };
            (k::YOU_HIT_OTHER_WITH_NANO, 0x16, t)
        }
        0x41 => (k::YOU_GAVE_HEALTH, 0, f("Feedback_YouHealed", &[s(d_name()), n(amt)])),
        0x42 => (k::ME_GOT_HEALTH, 0, f("Feedback_GotHealedByPlayer", &[s(d_name()), n(amt)])),
        0x43 => (k::YOU_GAVE_NANO, 0, f("Feedback_PlayerIncreasedNano", &[s(d_name()), n(amt)])),
        0x44 => (k::ME_GOT_NANO, 0, f("Feedback_GotNanoIncreaseFrom", &[s(d_name()), n(amt)])),
        0x45 => {
            if amt < 1 {
                return None;
            }
            (k::ME_GOT_XP, 0x1d, f("Feedback_GainedAlienXP", &[n(amt)]))
        }
        0x47 => {
            let Some(nn) = &c.s18 else { return None };
            (k::ME_CAST_NANO, 0x23, f("Feedback_ExecutingNanoProgram", &[s(nn.as_str())]))
        }
        _ => return None,
    };
    if text.is_empty() {
        return None;
    }
    // tail of the function [GC 0x10014b92]: text + " " + "Critical hit!" / "Glancing hit."
    let mut text = text;
    if c.mode == 4 {
        text = format!("{text} {}", ctx.fb("Feedback_CriticalHit"));
    }
    if c.mode == 2 {
        text = format!("{text} {}", ctx.fb("Feedback_GlancingHit"));
    }
    Some(Out { class, color, text })
}

fn line(class: u32, color: u32, text: String) -> LogLine {
    LogLine { class, line: ChatLine::new(ChatKind::Other(color_name(color)), text) }
}

fn pet_involved(ctx: &LogCtx, a: Identity, b: Identity) -> bool {
    (ctx.is_own_pet)(a) || (ctx.is_own_pet)(b)
}

/// `FUN_1009a709` [GC 0x1009a709]: a valid damage-type stat.
fn valid_dtype(v: i32) -> bool {
    matches!(v, 0x5a..=0x61 | 0xa8)
}

fn combat(c: &Call, ctx: &LogCtx, out: &mut Vec<LogLine>) {
    if let Some(o) = combat_text(c, ctx) {
        out.push(line(o.class, o.color, o.text));
    }
}

/// A line emitted directly through `FUN_10012b05(group, text, color)`.
fn direct(group: i32, color: u32, text: String, out: &mut Vec<LogLine>) {
    if !text.is_empty() {
        out.push(line(group as u32, color, text));
    }
}

// ------------------------------------------------------------------------------------------------ events -> lines

/// Lines for one event, with their message class, after the `/chatfilter` rules.
pub fn classify(ev: &LogEvent, ctx: &LogCtx) -> Vec<LogLine> {
    let mut out = Vec::new();
    match ev {
        LogEvent::Hit { attacker, victim, damage, mode } => {
            // AttackInfoIIR apply [GC 0x1009ed0d] -> FUN_1006a8f3 -> FUN_1009b170 [GC 0x1009b170]
            if !ctx.known(*victim) || !ctx.known(*attacker) {
                return out;
            }
            let over = (ctx.stat)(*attacker, 0x153).filter(|&v| v != 0 && valid_dtype(v));
            let weapon = (ctx.stat)(*attacker, 0x1b4).filter(|&v| v > 0x59 && (v < 0x62 || v == 0xa8));
            let dtype = over.or(weapon).unwrap_or(0x5a);
            let cat = if ctx.is_own(*victim) {
                // `[0x21c] == 0 && FUN_10058a05() == 0`: FUN_10058a05's input is unresolved ([GUESS] false)
                if !(ctx.is_npc)(*attacker) {
                    0x1e
                } else {
                    0x1d
                }
            } else if ctx.is_own(*attacker) {
                0x1f
            } else if pet_involved(ctx, *attacker, *victim) {
                0x21
            } else {
                0x20
            };
            combat(&Call { o: Some(*attacker), dtype, mode: *mode, ..Call::new(cat, *victim, *damage) }, ctx, &mut out);
        }
        LogEvent::Miss { attacker, victim, stat } => {
            // FUN_1006ae50 [GC 0x1006ae50]; both must be SimpleChars
            if !ctx.known(*victim) || !ctx.known(*attacker) {
                return out;
            }
            let s18 = (*stat != 0).then(|| ao_formats::stats::name(*stat as u32).map_or_else(String::new, str::to_string));
            combat(&Call { o: Some(*attacker), s18, ..Call::new(0x3b, *victim, 1) }, ctx, &mut out);
        }
        LogEvent::SpecialHit { attacker, victim, damage, special } => {
            // FUN_1006a9c5 [GC 0x1006a9c5]
            if !ctx.known(*victim) {
                return out;
            }
            let cat = if !ctx.is_own(*attacker) {
                if ctx.is_own(*victim) {
                    0x30
                } else if pet_involved(ctx, *attacker, *victim) {
                    0x33
                } else {
                    0x31
                }
            } else {
                0x32
            };
            let name = (ctx.text)(2003, *special as u32).unwrap_or_default();
            combat(&Call { o: Some(*attacker), s18: Some(name), ..Call::new(cat, *victim, *damage) }, ctx, &mut out);
        }
        LogEvent::Absorb { who, amount, dtype } => {
            // [GC 0x1009ea2e]
            if ctx.known(*who) {
                let cat = if ctx.is_own(*who) { 0x34 } else { 0x35 };
                combat(&Call { dtype: *dtype, ..Call::new(cat, *who, *amount) }, ctx, &mut out);
            }
        }
        LogEvent::HealthDamage { who, delta, dtype, attacker, nano } => {
            // [GC 0x100a00c8]
            if who.kind != 50000 || !ctx.known(*who) {
                return out;
            }
            let att = (attacker.kind == 50000 && ctx.known(*attacker)).then_some(*attacker);
            let amount = delta.wrapping_abs();
            if *delta < 0 {
                let cat = if ctx.is_own(*who) {
                    0x1a
                } else if (ctx.is_own_pet)(*who) {
                    0x1c
                } else if att.is_some_and(|a| ctx.is_own(a)) {
                    0x40
                } else {
                    0x1b
                };
                combat(&Call { o: att, dtype: *dtype, nano: *nano, ..Call::new(cat, *who, amount) }, ctx, &mut out);
            } else if ctx.is_own(*who) {
                combat(&Call { nano: *nano, ..Call::new(0x22, *who, amount) }, ctx, &mut out);
            }
        }
        LogEvent::Reflect { who, amount, attacker } | LogEvent::DamageShield { who, amount, attacker } => {
            // [GC 0x100a0c6b] / [GC 0x100a0df6]
            if ctx.known(*who) {
                let reflect = matches!(ev, LogEvent::Reflect { .. });
                let att = (attacker.kind == 50000 && ctx.known(*attacker)).then_some(*attacker);
                let (cat, o) = match (ctx.is_own(*who), reflect) {
                    (true, true) => (0x27, None),
                    (true, false) => (0x25, None),
                    (false, true) => (0x28, att),
                    (false, false) => (0x26, att),
                };
                combat(&Call { o, ..Call::new(cat, *who, *amount) }, ctx, &mut out);
            }
        }
        LogEvent::Stat { who, stat, new } => {
            // StatIIR apply [GC 0x100a1aaf]
            if !ctx.known(*who) {
                return out;
            }
            let diff = new.wrapping_sub((ctx.stat)(*who, *stat).unwrap_or(0));
            let own = ctx.is_own(*who);
            match *stat {
                0x1b => {
                    if diff < 0 {
                        let cat = if own {
                            0x1a
                        } else if (ctx.is_own_pet)(*who) {
                            0x1c
                        } else {
                            0x1b
                        };
                        combat(&Call::new(cat, *who, diff.wrapping_abs()), ctx, &mut out);
                    } else if diff > 0 && own {
                        combat(&Call::new(0x22, *who, diff), ctx, &mut out);
                    }
                }
                0x34 if own => combat(&Call::new(0x24, *who, diff), ctx, &mut out),
                0x28 if own => combat(&Call::new(0x45, *who, diff), ctx, &mut out), // [GUESS] the AlienXP branch with stat 0xa9 / 0xb2 adds a second term
                0x23d if own => combat(&Call::new(0x3e, *who, diff), ctx, &mut out),
                0x2aa..=0x2ac if own => {
                    let t = ldb_format(&ctx.fb("Feedback_GotPVPScore"), &[n(diff), s((ctx.text)(2002, *stat as u32).unwrap_or_default())]);
                    out.push(line(class::ME_GOT_XP, 0x1d, t));
                }
                _ => {}
            }
        }
        LogEvent::NewLevel { who, f } => {
            // [GC 0x10075a0c]
            if ctx.known(*who) && ctx.is_own(*who) {
                combat(&Call::new(0x24, *who, f[7]), ctx, &mut out);
                direct(0, 0, ldb_format(&ctx.fb("Feedback_NewLevel"), &[n(f[0])]), &mut out);
                title_line(ctx, *who, f[5], &mut out);
            }
        }
        LogEvent::ShadowLevel { who, f } => {
            // [GC 0x1007759e]
            if ctx.known(*who) && ctx.is_own(*who) {
                combat(&Call::new(0x3e, *who, f[6]), ctx, &mut out);
                direct(0, 0, ldb_format(&ctx.key(101, "Format_WelcomeToSKLevel"), &[n(f[0])]), &mut out);
                title_line(ctx, *who, f[7], &mut out);
            }
        }
        LogEvent::Action { who, action, param, a, b } => action_lines(ctx, *who, *action, *param, *a, *b, &mut out),
        LogEvent::Feedback { group, cat, id } => {
            // FeedbackIIR apply [GC 0x10072e81]
            direct(*group, 0, (ctx.text)(*cat, *id).unwrap_or_default(), &mut out);
        }
        LogEvent::FormatFeedback { who, group, text, mode } => {
            // FormatFeedbackIIR apply [GC 0x10039341] -> FUN_1005aaa9 [GC 0x1005aaa9]: modes 1 and 2 are floating texts, else a chat line in yellow (16)
            if ctx.known(*who) && *mode != 1 && *mode != 2 {
                direct(*group, 16, remote_parse(ctx.text, text), &mut out);
            }
        }
        LogEvent::CastNano { who, nano_name } => {
            if ctx.is_own(*who) {
                combat(&Call { s18: Some(nano_name.clone()), ..Call::new(0x47, *who, 1) }, ctx, &mut out);
            }
        }
    }
    out.retain(|l| !ctx.filter.drops(&l.line.text));
    out
}

/// "Congratulations! You have now reached the level of %s!": `GetTitleStr(profession, title level)` = text category 1060, id `profession*100 + level`.
fn title_line(ctx: &LogCtx, who: Identity, title_level: i32, out: &mut Vec<LogLine>) {
    if title_level > 0 {
        let prof = (ctx.stat)(who, 0x3c).unwrap_or(0);
        let title = (ctx.text)(1060, (prof * 100 + title_level) as u32).unwrap_or_default();
        direct(0, 0, ldb_format(&ctx.fb("Feedback_CongratulationsReachedLevel"), &[s(title)]), out);
    }
}

/// `CharacterActionIIR_t` apply `FUN_1005d0d8` [GC 0x1005d0d8]: only the actions that print text (action id = index into the byte table
/// at [GC 0x1005efde]). `param` doubles as the destination class of the direct lines.
fn action_lines(ctx: &LogCtx, who: Identity, action: i32, param: i32, a: Identity, b: Identity, out: &mut Vec<LogLine>) {
    if !ctx.known(who) {
        return;
    }
    let own = ctx.is_own(who);
    // identity_a as a dynel (kind 50000 and in the world)
    let a_dyn = (a.kind == 50000 && ctx.known(a)).then_some(a);
    let mut tail: Option<String> = None; // (E-0x4c): emitted at the end for the client's own dynel only, class = param
    match action {
        0x01 => tail = Some(ctx.key(1000, "KilledMissionTarget")),
        0x2b => {
            tail = Some(ctx.key(1000, "RecievedTeamBonus"));
            if own {
                combat(&Call::new(0x24, who, b.instance), ctx, out);
            }
        }
        0x4f => tail = Some(ctx.key(1000, "YouAreInsured")),
        0x77 => {
            if let Some(d) = a_dyn {
                direct(param, 0, ldb_format(&ctx.fb("Feedback_TeamMemberLinkdead"), &[s(ctx.nm(d))]), out);
            }
        }
        0x81 | 0x82 => {
            if let Some(d) = a_dyn {
                let cat = if action == 0x81 { 0x42 } else { 0x41 };
                combat(&Call { o: Some(who), ..Call::new(cat, d, b.instance) }, ctx, out);
            }
        }
        0x8a | 0x8b => {
            if let Some(d) = a_dyn {
                if action == 0x8a || b.instance > 0 {
                    let cat = if action == 0x8b { 0x43 } else { 0x44 };
                    combat(&Call { o: Some(who), ..Call::new(cat, d, b.instance) }, ctx, out);
                }
            }
        }
        0x8c => {
            if a_dyn.is_some() && b.instance > 0 {
                tail = Some(ldb_format(&ctx.fb("Feedback_IncreasedNanoPool"), &[n(b.instance)]));
            }
        }
        0x9b => {
            if let Some(d) = a_dyn {
                direct(param, 12, ldb_format(&ctx.fb("Feedback_YouHaveBeenDetected"), &[s(ctx.nm(d))]), out);
            }
        }
        0x9c => {
            if let Some(d) = a_dyn {
                tail = Some(ldb_format(&ctx.fb("Feedback_YouWereDrained"), &[n(b.instance.wrapping_abs()), s(ctx.nm(d))]));
            }
        }
        0x9d => {
            direct(param, 0, ldb_format(&ctx.key(200, "VersionsInfo"), &[n(10), n(b.instance)]), out);
            if b.instance != 10 {
                direct(param, 0, ctx.key(200, "ClientServerVersMM"), out);
            }
        }
        // [GUESS] the id fed to GetText(2002, id) is identity_b.instance (the stat of the skill); the same register pattern as Stuck*
        0xa4 => direct(param, 30, ldb_format(&ctx.fb("Feedback_SkillAvailable"), &[s((ctx.text)(2002, b.instance as u32).unwrap_or_default())]), out),
        0xc4 => direct(param, 0, ldb_format(&ctx.fb("Feedback_StuckResolved"), &[n(b.instance)]), out),
        0xc5 => direct(param, 0, ldb_format(&ctx.fb("Feedback_StuckAvailable"), &[n(b.instance)]), out),
        // toxic damage: a.instance > 0 && b.instance > 0
        0xd1 if b.instance > 0 && a.instance > 0 => {
            if own {
                combat(&Call::new(0x2c, who, b.instance), ctx, out);
            } else if (ctx.is_own_pet)(who) {
                combat(&Call::new(0x3f, who, b.instance), ctx, out);
            }
        }
        _ => {}
    }
    if let Some(t) = tail {
        // [GC 0x1005edd6]: `header == client control dynel && text non-empty`
        if own {
            direct(param, 0, ldb_format(&t, &[]), out);
        }
    }
}

/// Lines for one event as plain `ChatLine`s (colour in `ChatKind::Other(name)`, "" = no `<font>`), after the chat filter.
pub fn lines_for(ev: &LogEvent, ctx: &LogCtx) -> Vec<ChatLine> {
    classify(ev, ctx).into_iter().map(|l| l.line).collect()
}

// ------------------------------------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;
    use ao_formats::screens::TextDb;
    use std::collections::HashMap;

    fn client() -> Option<TextDb> {
        let dir = std::env::var_os("AO_CLIENT_DIR").map(std::path::PathBuf::from).or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::Path::new(&h).join("Games/ProjectRubiKa/client"))
        })?;
        TextDb::load(&dir).ok()
    }

    fn id(i: i32) -> Identity {
        Identity { kind: 50000, instance: i }
    }

    /// Run `f` with a world: dynel names, the real text.mdb when installed, else the given templates.
    fn with_ctx<R>(own: i32, npcs: &[i32], f: impl FnOnce(&LogCtx) -> R) -> Option<R> {
        let db = client()?;
        let names: HashMap<i32, &str> =
            [(1, "Testy"), (2, "Snake"), (3, "Bob"), (4, "Alice"), (5, "Rex"), (own, "Testy")].into_iter().collect();
        let npcs = npcs.to_vec();
        let filter = ChatFilter::default();
        let ctx = LogCtx {
            own: id(own),
            name: &|i| (i.kind == 50000).then(|| names.get(&i.instance).map(|s| s.to_string())).flatten(),
            text: &|c, i| db.by_id(c, i),
            is_npc: &|i| npcs.contains(&i.instance),
            is_own_pet: &|i| i.instance == 5,
            nano_name: &|i| Some(format!("Nano#{i}")),
            stat: &|_, _| None,
            filter: &filter,
        };
        Some(f(&ctx))
    }

    fn texts(v: Vec<LogLine>) -> Vec<(u32, &'static str, String)> {
        v.into_iter()
            .map(|l| {
                let ChatKind::Other(c) = l.line.kind else { panic!() };
                (l.class, c, l.line.text)
            })
            .collect()
    }

    #[test]
    fn ldb_templates() {
        // %s/%u substitution, plural variants, Dump's space collapsing
        assert_eq!(ldb_format("%s hit %s for %u points of %s damage.", &[s("Bob"), s("Snake"), n(17), s("melee")]), "Bob hit Snake for 17 points of melee damage.");
        assert_eq!(ldb_format("%u #1{1: credit was| credits were} deducted.", &[n(1)]), "1 credit was deducted.");
        assert_eq!(ldb_format("%u #1{1: credit was| credits were} deducted.", &[n(2)]), "2 credits were deducted.");
        assert_eq!(ldb_format("Removing %d #1{ 1:buddy | buddies }.", &[n(1)]), "Removing 1 buddies .");
        assert_eq!(ldb_format("  a  %s   b", &[s("x")]), "a x b");
        assert_eq!(ldb_format("100%% %02d|%-3d|", &[n(7), n(5)]), "100% 07|5 |");
        assert_eq!(ldb_format("%u", &[n(-1)]), "4294967295");
        assert_eq!(ldb_format("%s", &[n(3)]), "int_value<3>");
    }

    #[test]
    fn remote_format_spans() {
        // ~& + base85(cat=110) + base85(id) + 'i' base85(42) + '~'
        fn b85(v: u32) -> String {
            let mut d = [0u8; 5];
            let mut v = v;
            for i in (0..5).rev() {
                d[i] = (v % 85) as u8 + 0x21;
                v /= 85;
            }
            String::from_utf8(d.to_vec()).unwrap()
        }
        let tpl = |c: u32, i: u32| (c == 110 && i == 7).then(|| "You got %d things and %s.".to_string());
        let s = format!("pre ~&{}{}i{}s\u{3}ab~ post", b85(110), b85(7), b85(42));
        assert_eq!(remote_parse(&tpl, s.as_bytes()), "pre You got 42 things and ab. post");
        assert_eq!(remote_parse(&tpl, b"no span ~ here"), "no span ~ here");
    }

    #[test]
    fn combat_lines_from_the_originals_templates() {
        let Some(()) = with_ctx(1, &[2], |ctx| {
            // Testy (own) hit by player Bob for 17, critical
            let l = texts(classify(&LogEvent::Hit { attacker: id(3), victim: id(1), damage: 17, mode: 4 }, ctx));
            assert_eq!(l, vec![(class::ME_HIT_BY_PLAYER, "CCPlayerHitMeColor", "Player Bob hit you for 17 points of projectile damage. Critical hit!".into())]);
            // Testy hit by monster Snake for 5
            let l = texts(classify(&LogEvent::Hit { attacker: id(2), victim: id(1), damage: 5, mode: 3 }, ctx));
            assert_eq!(l, vec![(class::ME_HIT_BY_MONSTER, "CCMonsterHitMeColor", "Snake hit you for 5 points of projectile damage.".into())]);
            // Testy hits Snake, glancing
            let l = texts(classify(&LogEvent::Hit { attacker: id(1), victim: id(2), damage: 9, mode: 2 }, ctx));
            assert_eq!(l, vec![(class::YOU_HIT_OTHER, "CCMeHitOtherColor", "You hit Snake for 9 points of projectile damage. Glancing hit.".into())]);
            // Bob hits Alice; Bob hits Rex (Rex is our pet)
            let l = texts(classify(&LogEvent::Hit { attacker: id(3), victim: id(4), damage: 3, mode: 3 }, ctx));
            assert_eq!(l[0].0, class::OTHER_HIT_BY_OTHER);
            assert_eq!(l[0].2, "Bob hit Alice for 3 points of projectile damage.");
            let l = texts(classify(&LogEvent::Hit { attacker: id(3), victim: id(5), damage: 3, mode: 3 }, ctx));
            assert_eq!((l[0].0, l[0].1), (class::YOUR_PET_HIT_BY_OTHER, "CCOtherHitOtherMyPetColor"));
            // zero damage prints nothing
            assert!(classify(&LogEvent::Hit { attacker: id(3), victim: id(4), damage: 0, mode: 3 }, ctx).is_empty());
            // misses
            let l = texts(classify(&LogEvent::Miss { attacker: id(1), victim: id(2), stat: 0 }, ctx));
            assert_eq!(l, vec![(class::YOUR_MISSES, "", "You tried to hit Snake, but missed!".into())]);
            let l = texts(classify(&LogEvent::Miss { attacker: id(2), victim: id(1), stat: 142 }, ctx));
            assert_eq!(l, vec![(class::OTHER_MISSES, "", "Snake tries to attack you with Brawl, but misses!".into())]);
            assert!(classify(&LogEvent::Miss { attacker: id(3), victim: id(4), stat: 0 }, ctx).is_empty(), "not about us");
            // special attack (stat 142 = Brawl in text category 2003)
            let l = texts(classify(&LogEvent::SpecialHit { attacker: id(1), victim: id(2), damage: 5, special: 142 }, ctx));
            assert_eq!(l, vec![(class::YOU_HIT_OTHER, "CCMeHitOtherColor", "You hit Snake for 5 points of Brawling damage.".into())]);
            let l = texts(classify(&LogEvent::SpecialHit { attacker: id(2), victim: id(1), damage: 5, special: 142 }, ctx));
            assert_eq!(l[0].2, "Snake hit you for 5 points of Brawling damage.");
            // xp / sk / heal
            let xp = |amount| LogEvent::Stat { who: id(1), stat: 0x34, new: amount };
            assert_eq!(texts(classify(&xp(100), ctx)), vec![(class::ME_GOT_XP, "CCMeGotXpColor", "You received 100 xp.".into())]);
            assert_eq!(texts(classify(&xp(-30), ctx))[0].2, "You lost 30 xp.");
            let l = texts(classify(&LogEvent::Stat { who: id(1), stat: 0x23d, new: 4 }, ctx));
            assert_eq!(l[0].2, "You gained 4 points of Shadowknowledge.");
            let l = texts(classify(&LogEvent::HealthDamage { who: id(1), delta: 12, dtype: 0, attacker: Identity::default(), nano: 0 }, ctx));
            assert_eq!(l, vec![(class::ME_GOT_HEALTH, "CCMeHealedColor", "You were healed for 12 points.".into())]);
            // nano damage with a nano program and an attacker
            let l = texts(classify(&LogEvent::HealthDamage { who: id(1), delta: -40, dtype: 95, attacker: id(2), nano: 163449 }, ctx));
            assert_eq!(l[0].2, "You were attacked with Nano#163449 for 40 points of cold damage.");
            let l = texts(classify(&LogEvent::HealthDamage { who: id(4), delta: -40, dtype: 95, attacker: id(1), nano: 163449 }, ctx));
            assert_eq!((l[0].0, l[0].2.as_str()), (class::YOU_HIT_OTHER_WITH_NANO, "You hit Alice with Nano#163449 for 40 points of cold damage."));
            // absorb, shields
            let l = texts(classify(&LogEvent::Absorb { who: id(1), amount: 8, dtype: 91 }, ctx));
            assert_eq!(l[0].2, "You absorbed 8 points of melee damage.");
            let l = texts(classify(&LogEvent::DamageShield { who: id(4), amount: 8, attacker: id(1) }, ctx));
            assert_eq!(l[0].2, "Your damage shield hit Alice for 8 points of damage.");
            // local cast
            let l = texts(classify(&LogEvent::CastNano { who: id(1), nano_name: "Heal I".into() }, ctx));
            assert_eq!(l, vec![(class::ME_CAST_NANO, "CCMeCastNano", "Executing Nano Program: Heal I.".into())]);
            // character actions
            let act = |action, param, a, b| LogEvent::Action { who: id(1), action, param, a, b };
            let l = texts(classify(&act(0x9c, class::SYSTEM as i32, id(2), id(25)), ctx));
            assert_eq!(l[0].2, "You were drained for 25 points of nano energy by Snake.");
            let l = texts(classify(&act(0x9b, 0x4000_0001, id(2), Identity::default()), ctx));
            assert_eq!((l[0].1, l[0].2.as_str()), ("CCRed", "You have been detected by Snake!"));
            let l = texts(classify(&act(0x01, 0x4000_0001, Identity::default(), Identity::default()), ctx));
            assert_eq!(l[0].2, "You killed the mission target!..");
            // level up
            let nl = LogEvent::NewLevel { who: id(1), f: [12, 0, 0, 0, 0, 0, 0, 0] };
            assert_eq!(texts(classify(&nl, ctx))[0].2, "New Level: 12!");
        }) else {
            eprintln!("skipped: no client text.mdb");
            return;
        };
    }

    #[test]
    fn chat_filter_drops_matching_lines() {
        let Some(()) = with_ctx(1, &[2], |ctx| {
            let f = ChatFilter { enabled: true, rules: vec!["Snake".into()], ..Default::default() };
            let ctx2 = LogCtx { own: ctx.own, name: ctx.name, text: ctx.text, is_npc: ctx.is_npc, is_own_pet: ctx.is_own_pet, nano_name: ctx.nano_name, stat: ctx.stat, filter: &f };
            let ev = LogEvent::Hit { attacker: id(2), victim: id(1), damage: 5, mode: 3 };
            assert_eq!(classify(&ev, ctx).len(), 1);
            assert!(classify(&ev, &ctx2).is_empty());
        }) else {
            return;
        };
    }

    /// The live capture: every AttackInfo / MissedAttackInfo / SpecialAttackInfo maps to an event and, for a known world, to a line.
    #[test]
    fn captured_combat_messages() {
        use ao_net::frame::Frame;
        use ao_net::n3;
        let Some(db) = client() else { return };
        let mut msgs = Vec::new();
        for l in include_str!("../../../../../docs/captures/zone_ithaca.rec").lines() {
            let mut p = l.split(' ');
            let (_, dir, hex) = (p.next().unwrap(), p.next().unwrap(), p.next().unwrap());
            if dir != "<" {
                continue;
            }
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            if let Ok(Some((f, _))) = Frame::decode_with(&b, false) {
                if f.ptype == 0xA {
                    msgs.push(n3::decode(&f).unwrap());
                }
            }
        }
        let evs: Vec<LogEvent> = msgs.iter().filter_map(from_n3).collect();
        let hits = evs.iter().filter(|e| matches!(e, LogEvent::Hit { .. })).count();
        let miss = evs.iter().filter(|e| matches!(e, LogEvent::Miss { .. })).count();
        let spec = evs.iter().filter(|e| matches!(e, LogEvent::SpecialHit { .. })).count();
        assert_eq!((hits, miss, spec), (134, 11, 3));
        // first AttackInfo of the capture: header 0xfa8d7 hits 0xf4a4c for 17 (docs/zone/misc.md §4)
        assert_eq!(evs.iter().find(|e| matches!(e, LogEvent::Hit { .. })), Some(&LogEvent::Hit { attacker: id(0xfa8d7), victim: id(0xf4a4c), damage: 17, mode: 3 }));
        // every one of them formats against a world where all combatants are known NPCs
        let filter = ChatFilter::default();
        let ctx = LogCtx {
            own: id(0x6584),
            name: &|i| Some(format!("npc{:x}", i.instance)),
            text: &|c, i| db.by_id(c, i),
            is_npc: &|_| true,
            is_own_pet: &|_| false,
            nano_name: &|_| None,
            stat: &|_, _| None,
            filter: &filter,
        };
        let first = classify(evs.iter().find(|e| matches!(e, LogEvent::Hit { .. })).unwrap(), &ctx);
        assert_eq!(first[0].line.text, "npcfa8d7 hit npcf4a4c for 17 points of projectile damage.");
        assert_eq!(first[0].class, class::OTHER_HIT_BY_OTHER);
        let special = evs.iter().find(|e| matches!(e, LogEvent::SpecialHit { .. })).unwrap();
        assert_eq!(classify(special, &ctx)[0].line.text, "npc827a hit npcfa8ea for 5 points of Brawling damage.");
        for e in evs.iter().filter(|e| matches!(e, LogEvent::Hit { damage, .. } if *damage != 0)) {
            assert_eq!(classify(e, &ctx).len(), 1, "{e:?}");
        }
        // the captured CharacterActions (0xA7, 0x63, 0x62, 0xAD) print nothing; StatIIRs expand to per-stat events
        let acts = evs.iter().filter(|e| matches!(e, LogEvent::Action { .. })).count();
        assert_eq!(acts, msgs.iter().filter(|m| matches!(m.body, N3::World(World::CharacterAction(_)))).count());
        for e in evs.iter().filter(|e| matches!(e, LogEvent::Action { .. })) {
            assert!(classify(e, &ctx).is_empty(), "{e:?}");
        }
        assert!(msgs.iter().flat_map(events).any(|e| matches!(e, LogEvent::Stat { stat: 0x1b, .. })));
        // crit / glancing suffix: the capture has unk_30 = 4 in a few messages
        let crits = evs.iter().filter(|e| matches!(e, LogEvent::Hit { mode: 4, .. })).count();
        assert!(crits >= 1);
        // Misses between two bystanders print nothing (neither is the client's character)
        for e in evs.iter().filter(|e| matches!(e, LogEvent::Miss { .. })) {
            assert!(classify(e, &ctx).is_empty());
        }
    }

    fn raw(msg_type: u32, who: Identity, body: &[i32]) -> Message {
        let mut b = Vec::new();
        for v in body {
            b.extend_from_slice(&v.to_be_bytes());
        }
        Message { header: ao_net::n3::N3Header { msg_type, target: who, flag: 0 }, sender: 1, body: N3::Unknown(b) }
    }

    /// The raw layouts of the `ReadSubClass` functions (docs/chat/log.md, wire table).
    #[test]
    fn raw_wire_layouts() {
        let who = id(1);
        let m = raw(ABSORB, who, &[8, 91]);
        assert_eq!(from_n3(&m), Some(LogEvent::Absorb { who, amount: 8, dtype: 91 }));
        let m = raw(HEALTH_DAMAGE, who, &[100, -40, 95, 7, 50000, 2, 163449]);
        assert_eq!(from_n3(&m), Some(LogEvent::HealthDamage { who, delta: -40, dtype: 95, attacker: id(2), nano: 163449 }));
        let m = raw(REFLECT_ATTACK, who, &[12, 50000, 2, 3]);
        assert_eq!(from_n3(&m), Some(LogEvent::Reflect { who, amount: 12, attacker: id(2) }));
        let m = raw(SHIELD_ATTACK, who, &[12, 50000, 2, 3]);
        assert_eq!(from_n3(&m), Some(LogEvent::DamageShield { who, amount: 12, attacker: id(2) }));
        let m = raw(NEW_LEVEL, who, &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(from_n3(&m), Some(LogEvent::NewLevel { who, f: [1, 2, 3, 4, 5, 6, 7, 8] }));
        let m = raw(FEEDBACK, who, &[0x4200_0001, 110, 0x1234]);
        assert_eq!(from_n3(&m), Some(LogEvent::Feedback { group: 0x4200_0001, cat: 110, id: 0x1234 }));
        // FormatFeedback: i32 group, u16 len + bytes, i32 mode
        let mut b = 5i32.to_be_bytes().to_vec();
        b.extend_from_slice(&3u16.to_be_bytes());
        b.extend_from_slice(b"abc");
        b.extend_from_slice(&0i32.to_be_bytes());
        let m = Message { header: ao_net::n3::N3Header { msg_type: FORMAT_FEEDBACK, target: who, flag: 0 }, sender: 1, body: N3::Unknown(b) };
        assert_eq!(from_n3(&m), Some(LogEvent::FormatFeedback { who, group: 5, text: b"abc".to_vec(), mode: 0 }));
        assert_eq!(from_n3(&raw(ABSORB, who, &[8])), None, "truncated body");
    }

    /// Every `Feedback_*` key the formatter uses exists in text category 110 and has as many conversions as arguments fed.
    #[test]
    fn feedback_keys_exist() {
        let Some(db) = client() else { return };
        for k in [
            "Feedback_FallDamage", "Feedback_AttackedByForPointsOfDamage", "Feedback_YouWereAttackedByNanobotsFrom",
            "Feedback_AttackedByNanobotsForPointsOfDamage", "Feedback_TookPointsOfFallDamage", "Feedback_WasAttackedByForPointsOfDamageFrom",
            "Feedback_WasAttackedByForPointsOfDamage", "Feedback_WasAttackedByNanobotsFrom", "Feedback_WasAttackedByNanobots",
            "Feedback_YouHitOtherWith", "Feedback_YouHitOtherByNanobots", "Feedback_HitYouForPointsOfDamage",
            "Feedback_WereHitForPointsOfDamage", "Feedback_PlayerHitYouForPointsOfDamage", "Feedback_PlayerHitYou",
            "Feedback_HitWithSpecial", "Feedback_OtherHitOther", "Feedback_SomethingHitOther", "Feedback_HealedForPoints",
            "Feedback_LostXP", "Feedback_ReceivedXP", "Feedback_GainedAlienXP", "Feedback_LostSK", "Feedback_GainedSK",
            "Feedback_MonsterHitWithSpecial", "Feedback_YouWereHitByDamageShield", "Feedback_SomeonesDamageShieldHitYou",
            "Feedback_YourDamageShieldHitOther", "Feedback_OtherDamageShieldHitOther", "Feedback_SomethingHitOtherWithDamageShield",
            "Feedback_YouWereHitByReflectShield", "Feedback_SomeonesReflectShieldHitYou", "Feedback_YourReflectShieldHitOther",
            "Feedback_OtherReflectShieldHitOther", "Feedback_SomethingHitOtherWithReflectShield", "Feedback_YouAbsorbedDamage",
            "Feedback_OtherAbsorbedDamage", "Feedback_SomeoneAbsorbedDamage", "Feedback_MeHitByToxic", "Feedback_PetHitByToxic",
            "TryToAttackWithSpecialButMiss", "Feedback_YouTriedToHitMissed", "OtherTriesToAttackWithSpecialButMisses",
            "Feedback_OtherTriedToHitMissed", "Feedback_YouHealed", "Feedback_GotHealedByPlayer", "Feedback_PlayerIncreasedNano",
            "Feedback_GotNanoIncreaseFrom", "Feedback_ExecutingNanoProgram", "Feedback_CriticalHit", "Feedback_GlancingHit",
            "Feedback_NewLevel", "Feedback_CongratulationsReachedLevel", "Feedback_GotPVPScore",
        ] {
            assert!(db.by_id(110, elf_hash(k)).is_some(), "{k}");
        }
    }
}
