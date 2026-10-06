//! Combat-log lines and floating damage numbers exactly as the original client builds them (docs/zone/combat-log.md).
//!
//! * [`LdbFormat`] is `LDBformat` of `ldb.dll` (`Init` [LDB 0x100053f2], `Feed` [LDB 0x10004c8b/0x10004da7/0x10004fd1],
//!   `Dump` [LDB 0x10004885]): printf-style tokens fed in order, an unfed token stays in the text literally.
//! * [`render`] is `FUN_10012bd5` [GC 0x10012bd5], the client's single "feedback message" formatter (type `0x1a..=0x47`).
//! * [`FloatingNumber`] constants are the damage-number draw parameters of the HUD path (`RenderTextModule_t::DamageTextMessage`
//!   [GUI 0x1004ae2f]) and of the world path (`_GfxControlFont_t`, effect `0x2f5a` of `Setupf/gfxtweak.bin`).


use super::stat_names;
use ao_formats::screens::TextDb;

/// LDB text category of the `Feedback_*` strings (`FUN_1003807c` [GC 0x1003807c] = `LDBface::GetText(0x6e, key)`).
pub const CAT_FEEDBACK: u32 = 110;
/// LDB category of the stat names handed to `%s` by the special-attack texts (`LDBface::GetText(0x7d3, stat)`).
pub const CAT_STAT_NAMES: u32 = 2003;

/// Source of the client's text strings. `TextDb` (`cd_image/text/text.mdb`) is the real one.
pub trait Texts {
    /// `GetText(110, key)`: string-keyed (`LDBface::ElfHash`) feedback text.
    fn feedback(&self, key: &str) -> Option<String>;
    /// `GetText(2003, id)`: localized stat name.
    fn stat_name(&self, stat: u32) -> Option<String>;
}

impl Texts for TextDb {
    fn feedback(&self, key: &str) -> Option<String> {
        self.by_key(CAT_FEEDBACK, key)
    }
    fn stat_name(&self, stat: u32) -> Option<String> {
        self.by_id(CAT_STAT_NAMES, stat)
    }
}

// ---------------------------------------------------------------------------------------------------------------
// LDBformat
// ---------------------------------------------------------------------------------------------------------------

/// One value handed to [`LdbFormat::feed`] (`Feed(int)`, `Feed(unsigned)`, `Feed(const char*)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arg<'a> {
    Int(i32),
    UInt(u32),
    Str(&'a str),
}

#[derive(Debug)]
enum Token {
    Literal(String),
    /// `%…` conversion with its 1-based argument number (`Init` counts them).
    Spec { index: u32, text: String },
}

/// `LDBformat` (`ldb.dll`). `Init` splits the format at every `%` conversion (the flag/width characters
/// `"-+ 0#123456789.hlL"` belong to it, `%%` is a literal `%`); `Feed` replaces the conversion whose number equals the
/// number of values fed so far. `#N{…}` plural tokens are not needed by any combat text and are kept literally.
#[derive(Debug)]
pub struct LdbFormat {
    tokens: Vec<Token>,
    fed: u32,
}

impl LdbFormat {
    pub fn new(fmt: &str) -> Self {
        let b = fmt.as_bytes();
        let (mut tokens, mut lit_start, mut index, mut i) = (Vec::new(), 0usize, 1u32, 0usize);
        while i < b.len() {
            if b[i] != b'%' {
                i += 1;
                continue;
            }
            if i > lit_start {
                tokens.push(Token::Literal(fmt[lit_start..i].to_string()));
            }
            if b.get(i + 1) == Some(&b'%') {
                tokens.push(Token::Literal("%".into()));
                i += 2;
            } else {
                let mut j = i + 1;
                while j < b.len() && b"-+ 0#123456789.hlL".contains(&b[j]) {
                    j += 1;
                }
                let end = (j + 1).min(b.len()); // the conversion letter
                tokens.push(Token::Spec { index, text: fmt[i..end].to_string() });
                index += 1;
                i = end;
            }
            lit_start = i;
        }
        if lit_start < b.len() {
            tokens.push(Token::Literal(fmt[lit_start..].to_string()));
        }
        Self { tokens, fed: 1 }
    }

    /// `Feed`: `%s` fed a number prints `int_value<N>` / `uint_value<N>` (`ldb.dll` string constants); `%u`/`%d` fed a
    /// string is a stack garbage `snprintf` in the client and is rendered as the string itself here.
    pub fn feed(mut self, a: Arg) -> Self {
        let fed = self.fed;
        for t in &mut self.tokens {
            if let Token::Spec { index, text } = t {
                if *index == fed && text.starts_with('%') {
                    let conv = text.chars().last().unwrap_or('s');
                    *text = match (a, conv) {
                        (Arg::Str(s), _) => s.to_string(),
                        (Arg::Int(v), 's') => format!("int_value<{v}>"),
                        (Arg::UInt(v), 's') => format!("uint_value<{v}>"),
                        (Arg::Int(v), 'u') => (v as u32).to_string(),
                        (Arg::Int(v), _) => v.to_string(),
                        (Arg::UInt(v), 'd' | 'i') => (v as i32).to_string(),
                        (Arg::UInt(v), _) => v.to_string(),
                    };
                }
            }
        }
        self.fed += 1;
        self
    }

    /// `Dump`: concatenation of all tokens with leading spaces dropped and runs of spaces collapsed to one.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        let mut prev_space = true;
        for t in &self.tokens {
            let s = match t {
                Token::Literal(s) => s.as_str(),
                Token::Spec { text, .. } => text.as_str(),
            };
            for c in s.chars() {
                if c == ' ' {
                    if prev_space {
                        continue;
                    }
                    prev_space = true;
                } else {
                    prev_space = false;
                }
                out.push(c);
            }
        }
        out
    }
}

/// `fStatToString` [GC 0x100367b2]: the English enum name, `Missing stat: N` when the id is not in the table.
pub fn stat_to_string(stat: u32) -> String {
    stat_names::stat_name(stat).map_or_else(|| format!("Missing stat: {}", stat as i32), str::to_string)
}

/// `FUN_10036adf` [GC 0x10036adf]: name of a damage type id (map built at `FUN_100324d2` [GC 0x10033a76..]):
/// 0x5a projectile, 0x5b melee, 0x5c energy, 0x5d chemical, 0x5e radiation, 0x5f cold, 0x60 poison, 0x61 fire, 0xa8 nano,
/// 0x1b "unknown"; anything else `Missing damagetype: N`.
pub fn damage_type_name(t: i32) -> String {
    match t {
        0x5a => "projectile",
        0x5b => "melee",
        0x5c => "energy",
        0x5d => "chemical",
        0x5e => "radiation",
        0x5f => "cold",
        0x60 => "poison",
        0x61 => "fire",
        0xa8 => "nano",
        0x1b => "unknown",
        _ => return format!("Missing damagetype: {t}"),
    }
    .to_string()
}

/// `FUN_1009afde` [GC 0x1009afde]: damage type of a hit = weapon stat `0x1b4` when it is `0x5a..=0x61` or `0xa8`, else `0x5a`.
pub fn weapon_damage_type(stat_1b4: i32) -> i32 {
    match stat_1b4 {
        0x5a..=0x61 | 0xa8 => stat_1b4,
        _ => 0x5a,
    }
}

// ---------------------------------------------------------------------------------------------------------------
// colours and floating numbers
// ---------------------------------------------------------------------------------------------------------------

/// `FontSystem_t::SetColorsIntoMap` [GUI 0x1012e316]: `ColorCode_e` -> `0xRRGGBB` (what a `0x10 <code>` escape inside a
/// GUI text string selects, `FontSystem_t::GetColor` [GUI 0x1012e2d1]; unknown codes give `0xff00ff`).
pub const GUI_COLORS: &[(u32, u32)] = &[
    (0x00, 0x00ff00), (0x01, 0x30d2ff), (0x02, 0x30d2ff), (0x03, 0xffffc9), (0x04, 0x00ffff), (0x05, 0xffff45),
    (0x06, 0x00a651), (0x07, 0x63e689), (0x08, 0x0ff20b), (0x09, 0xff0099), (0x0a, 0x2299ff), (0x0b, 0x000000),
    (0x0c, 0xff0000), (0x0d, 0x00f000), (0x0e, 0x0000ff), (0x0f, 0xffffff), (0x10, 0xffff00), (0x11, 0xccaa44),
    (0x12, 0xdddd44), (0x13, 0x00dd44), (0x14, 0x66aa66), (0x15, 0xffffff), (0x16, 0x9ad5d9), (0x17, 0xff0000),
    (0x18, 0xff0000), (0x19, 0xd9d9d2), (0x1a, 0x999926), (0x1b, 0xff7718), (0x1c, 0x8cb6ff), (0x1d, 0xffff00),
    (0x1e, 0xffe3a1), (0x1f, 0x00ee00), (0x20, 0xeeeeee), (0x21, 0xcccccc), (0x22, 0xff63ff), (0x23, 0xffffff),
    (0x32, 0xff8cfc), (0x33, 0xffffff), (0x34, 0xff61a6), (0x35, 0x6699ff), (0x36, 0x66ff99), (0x37, 0x00f000),
    (0x50, 0xeeeeee),
];

/// `ColorCode_e` -> `0xRRGGBB`.
pub fn gui_color(code: u32) -> u32 {
    GUI_COLORS.iter().find(|e| e.0 == code).map_or(0xff00ff, |e| e.1)
}

/// `DAT_10156b08` [GC 0x10156b08] (`0x53` entries, read by `FUN_10011108` [GC 0x10011108]): category -> colour of the world
/// floating number. Entries `0x1e..` are zero in the file (no writer exists: only the one read at 0x10011133), `0..=8`
/// differ from [`GUI_COLORS`] (a second, older palette); the effect ORs `0xff000000`, a category > 0x52 gives `0xffff0000`.
pub const WORLD_COLORS: [u32; 0x1e] = [
    0x000000, 0x15e0a0, 0x30d2ff, 0xffffc9, 0x00ffff, 0xffff45, 0x00a651, 0x63e689, 0x0ff20b, 0xff0099, 0x2299ff, 0x000000,
    0xff0000, 0x00f000, 0x0000ff, 0xffffff, 0xffff00, 0xccaa44, 0xdddd44, 0x00dd44, 0x66aa66, 0xffffff, 0x9ad5d9, 0xff0000,
    0xff0000, 0xd9d9d2, 0x999926, 0xff7718, 0x8cb6ff, 0xffff00,
];

/// `FUN_10011108` colour: `ARGB`.
pub fn world_color(category: u32) -> u32 {
    let rgb = if category > 0x52 {
        return 0xffff_0000;
    } else {
        WORLD_COLORS.get(category as usize).copied().unwrap_or(0)
    };
    rgb | 0xff00_0000
}

/// Where a floating number is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Space {
    /// Screen-space text of the HUD (`FUN_100044c2` -> `AFCM::Send(0x19, 0x3b, value, category)` -> `DamageTextMessage`):
    /// only for the client's own character.
    Hud,
    /// Billboard effect above the dynel (`FUN_10011108`: `CreateEffect2(0x2f5a, dynel)` + `SetColor` + `SetText`).
    World,
}

/// Font of a floating number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatingFont {
    /// GUI `FontID_e` (`FontSystem_t`); 2 for the HUD number.
    Gui { font_id: u32 },
    /// Bitmap font billboard (`_GfxControlFont_t`, effect record template param 9 = material id handed to
    /// `_EffectHandler_t::MMGetMaterial`; glyph advance table `DAT_102c460a`).
    Effect { effect_id: u32, material: u32 },
}

/// Draw parameters of one floating number.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FloatingNumber {
    /// `0xAARRGGBB` ([`Space::World`]) or `0x00RRGGBB` ([`Space::Hud`], alpha is the fade).
    pub color: u32,
    /// Hud: pixels per second (70 px over the life); World: effect units per second (`0.4 * elapsed`).
    pub rise_speed: f32,
    /// Seconds until the number is removed.
    pub life: f32,
    pub font: FloatingFont,
    pub space: Space,
}

/// HUD path (own character): `RenderText_t` [GUI 0x1004ac94] created by `DamageTextMessage` [GUI 0x1004ae2f]. Text `"%d"`,
/// `FontID_e 2`, centred on `(50 + r, DAT_102761c0 - 20)` with `r = round(rand()/32767*40 - 20)` (the first `rand()` is discarded),
/// the colour escape is the category's `ColorCode`. Life 2.3 s (`DAT_101b1198`); [`FloatingNumber::rise_speed`] =
/// `70 px (DAT_101b1190) / 2.3 s`: `y = y0 - 70 * elapsed/life` (`FUN_1004aa4f` [GUI 0x1004aa4f], flag bit 0); alpha
/// `= 255 * remaining/life` is applied when flag bit 1 is set, but `DamageTextMessage` passes flags = 1, so **no fade**.
pub const HUD_NUMBER: FloatingNumber = FloatingNumber {
    color: 0,
    rise_speed: 70.0 / 2.3,
    life: 2.3,
    font: FloatingFont::Gui { font_id: 2 },
    space: Space::Hud,
};
/// HUD number start position: x = 50 + [-20, 20] jitter, y = `DAT_102761c0` - 20 ([UNRESOLVED] what `DAT_102761c0` is).
pub const HUD_X: i32 = 50;
pub const HUD_Y_FROM_REF: i32 = -20;
pub const HUD_X_JITTER: i32 = 20;

/// World path (everyone else): effect `0x2f5a` (`gfxtweak.bin` record type 0x7de = `_GfxControlFont_t`, ctor `FUN_100df42d`
/// [GC 0x100df42d], params `FUN_100decc3` [GC 0x100decc3]): life = param 0x1a = 1.3 s, char size (`+0x2c`) = param 0x1c = 0.01,
/// rise = param 0x1d = 0.4 * elapsed (`FUN_100deb04` [GC 0x100deb04] translates the billboard by `(0, 0.4 * t, 0)`),
/// material id = param 9 = 40, constant colour (`SetColor` = start = stop colour).
pub const WORLD_NUMBER: FloatingNumber = FloatingNumber {
    color: 0,
    rise_speed: 0.4,
    life: 1.3,
    font: FloatingFont::Effect { effect_id: 0x2f5a, material: 40 },
    space: Space::World,
};

/// The draw parameters of a number in `category` for `space`.
pub fn floating_number(space: Space, category: u32) -> FloatingNumber {
    match space {
        Space::Hud => FloatingNumber { color: gui_color(category), ..HUD_NUMBER },
        Space::World => FloatingNumber { color: world_color(category), ..WORLD_NUMBER },
    }
}

// ---------------------------------------------------------------------------------------------------------------
// FUN_10012bd5: the feedback formatter
// ---------------------------------------------------------------------------------------------------------------

/// Feedback message types of `FUN_10012bd5` that this slice's messages produce (the type is the first argument).
pub mod ty {
    /// Own char hit by a nano/other (stat `Health` delta < 0 on the client char); `value` = damage.
    pub const SELF_ATTACKED: u32 = 0x1a;
    /// Another char lost health (`Health` delta, not a pet of the local player).
    pub const OTHER_ATTACKED: u32 = 0x1b;
    /// As [`OTHER_ATTACKED`] for a char flagged as the local player's pet.
    pub const PET_ATTACKED: u32 = 0x1c;
    /// Client char hit by a monster / player (`FUN_1009b170`).
    pub const HIT_YOU: u32 = 0x1d;
    /// Client char hit by another *player* (PvP).
    pub const PLAYER_HIT_YOU: u32 = 0x1e;
    /// Client char hit something.
    pub const YOU_HIT: u32 = 0x1f;
    pub const OTHER_HIT_OTHER: u32 = 0x20;
    pub const OTHER_HIT_OTHER_PVP: u32 = 0x21;
    /// Client char healed (`Health` delta > 0).
    pub const HEALED: u32 = 0x22;
    /// XP gained/lost (stat 0x34 delta).
    pub const XP: u32 = 0x24;
    /// Special attack hit the client char.
    pub const SPECIAL_HIT_YOU: u32 = 0x30;
    pub const SPECIAL_OTHER: u32 = 0x31;
    /// Client char's special attack hit.
    pub const SPECIAL_YOU_HIT: u32 = 0x32;
    pub const SPECIAL_OTHER_PVP: u32 = 0x33;
    /// An attack missed.
    pub const MISS: u32 = 0x3b;
    /// Shadowknowledge gained/lost (stat 0x23d delta).
    pub const SHADOW_KNOWLEDGE: u32 = 0x3e;
    /// Alien XP gained (stat 0x28).
    pub const ALIEN_XP: u32 = 0x45;
}

/// A character as the formatter sees it (`SimpleChar_t`: `+0x140` = client char, `(+0xe8)->vt+0x34` = name).
#[derive(Clone, Copy, Debug)]
pub struct Who<'a> {
    /// Identity instance (the formatter compares the two chars by pointer).
    pub id: i32,
    pub name: &'a str,
    pub is_client: bool,
}

/// Arguments of `FUN_10012a1e(type, A, value, B, extra, stat, hit, ident)` (the call frame of `FUN_10012bd5`).
#[derive(Clone, Copy, Debug)]
pub struct Feedback<'a> {
    pub ty: u32,
    /// `[ebp+0xc]`: the subject (victim / target); also the owner of the floating number.
    pub a: Who<'a>,
    /// `[ebp+0x10]`: damage / amount.
    pub value: i32,
    /// `[ebp+0x14]`: the second char (attacker), when known.
    pub b: Option<Who<'a>>,
    /// `[ebp+0x18]`: extra string (special attack name, missed stat name).
    pub extra: Option<&'a str>,
    /// `[ebp+0x1c]`: damage type / stat; 0 -> 0x1b.
    pub stat: i32,
    /// `[ebp+0x20]`: hit flags: 4 = critical, 2 = glancing (appended as ` Critical hit!` / ` Glancing hit.`).
    pub hit: i32,
    /// `[ebp+0x24]` resolved to a char (name only).
    pub ident: Option<Who<'a>>,
}

impl<'a> Feedback<'a> {
    pub fn new(ty: u32, a: Who<'a>, value: i32) -> Self {
        Self { ty, a, value, b: None, extra: None, stat: 0, hit: 0, ident: None }
    }
}

/// Result of [`render`].
#[derive(Clone, Debug, PartialEq)]
pub struct Rendered {
    /// `[ebp+0x10]` after the switch: first argument of the chat signal (`FUN_10012b05(style, text, category)`), values
    /// `0x42000001..=0x42000018`; [UNRESOLVED] what the chat window does with it (`GlobalSignals+0x17c` receiver in GUI.dll not traced).
    pub style: u32,
    /// `[ebp-0x10]`: chat category = `ColorCode_e` of the line and of the floating number.
    pub category: u32,
    /// Final line (empty = nothing is printed).
    pub text: String,
    /// `"%d"` number shown at `a` (`sprintf(.., "%d", value)`), `None` when the formatter zeroes the value.
    pub number: Option<i32>,
}

const STYLE: u32 = 0x4200_0000;

fn fmt1(texts: &dyn Texts, key: &str, args: &[Arg]) -> String {
    let Some(f) = texts.feedback(key) else { return String::new() };
    args.iter().fold(LdbFormat::new(&f), |l, a| l.feed(*a)).dump()
}

/// Port of `FUN_10012bd5` for the types in [`ty`]. `None` = type not handled by this slice. The branch structure follows the
/// decompile of every `case` (see docs/zone/combat-log.md for the per-case tables).
pub fn render(texts: &dyn Texts, f: &Feedback) -> Option<Rendered> {
    let stat = if f.stat == 0 { 0x1b } else { f.stat };
    let v = f.value as u32;
    let abs = f.value.unsigned_abs();
    let dmg = damage_type_name(stat);
    let name_a = f.a.name;
    let s = Arg::Str;
    let mut number = (f.value != 0).then_some(f.value);
    let (style, category, mut text);
    match f.ty {
        ty::SELF_ATTACKED => {
            style = STYLE + 2;
            category = 0x15;
            text = if stat == 0x1da {
                fmt1(texts, "Feedback_FallDamage", &[Arg::UInt(v)])
            } else if let Some(i) = f.ident {
                fmt1(texts, "Feedback_YouWereAttackedByNanobotsFrom", &[s(i.name), Arg::UInt(v), s(&dmg)])
            } else if f.b.is_none_or(|b| b.id == f.a.id) {
                fmt1(texts, "Feedback_AttackedByNanobotsForPointsOfDamage", &[Arg::UInt(v), s(&dmg)])
            } else {
                fmt1(texts, "Feedback_AttackedByForPointsOfDamage", &[s(f.b.map_or("", |b| b.name)), Arg::UInt(v), s(&dmg)])
            };
        }
        ty::OTHER_ATTACKED | ty::PET_ATTACKED => {
            let pet = f.ty == ty::PET_ATTACKED;
            style = STYLE + 3 + u32::from(!pet);
            category = if pet { 0x1b } else { 0x16 };
            text = if stat == 0x1da {
                fmt1(texts, "Feedback_TookPointsOfFallDamage", &[s(name_a), Arg::UInt(v)])
            } else {
                match (f.b, f.ident) {
                    (None, None) => fmt1(texts, "Feedback_WasAttackedByNanobots", &[s(name_a), Arg::UInt(v), s(&dmg)]),
                    (Some(b), None) => {
                        fmt1(texts, "Feedback_WasAttackedByForPointsOfDamage", &[s(name_a), s(b.name), Arg::UInt(v), s(&dmg)])
                    }
                    (None, Some(i)) => {
                        fmt1(texts, "Feedback_WasAttackedByNanobotsFrom", &[s(name_a), s(i.name), Arg::UInt(v), s(&dmg)])
                    }
                    (Some(b), Some(i)) => fmt1(
                        texts,
                        "Feedback_WasAttackedByForPointsOfDamageFrom",
                        &[s(name_a), s(i.name), s(b.name), Arg::UInt(v), s(&dmg)],
                    ),
                }
            };
        }
        ty::HIT_YOU => {
            style = STYLE + 6;
            category = 0x17;
            text = match f.b {
                Some(b) if b.id != f.a.id => {
                    fmt1(texts, "Feedback_HitYouForPointsOfDamage", &[s(b.name), Arg::UInt(v), s(&dmg)])
                }
                _ => fmt1(texts, "Feedback_WereHitForPointsOfDamage", &[Arg::UInt(v)]),
            };
        }
        ty::PLAYER_HIT_YOU => {
            style = STYLE + 7;
            category = 0x18;
            text = match f.b {
                None => fmt1(texts, "Feedback_PlayerHitYou", &[Arg::UInt(v)]),
                Some(b) => fmt1(texts, "Feedback_PlayerHitYouForPointsOfDamage", &[s(b.name), Arg::UInt(v), s(&dmg)]),
            };
        }
        ty::YOU_HIT => {
            style = STYLE + 8;
            category = 0x19;
            text = fmt1(texts, "Feedback_HitWithSpecial", &[s(name_a), Arg::UInt(v), s(&dmg)]);
        }
        ty::OTHER_HIT_OTHER | ty::OTHER_HIT_OTHER_PVP => {
            let pvp = f.ty == ty::OTHER_HIT_OTHER_PVP;
            style = STYLE + 9 + u32::from(!pvp);
            category = 0x1a + u32::from(pvp);
            text = match f.b {
                // third `%s` of "Something hit %s for %u points of %s damage." is never fed: it stays literally
                None => fmt1(texts, "Feedback_SomethingHitOther", &[s(name_a), Arg::UInt(v)]),
                Some(b) => fmt1(texts, "Feedback_OtherHitOther", &[s(b.name), s(name_a), Arg::UInt(v), s(&dmg)]),
            };
        }
        ty::HEALED => {
            style = STYLE + 0x15;
            category = 0x1c;
            text = fmt1(texts, "Feedback_HealedForPoints", &[Arg::UInt(v)]);
        }
        ty::XP | ty::SHADOW_KNOWLEDGE => {
            let xp = f.ty == ty::XP;
            style = STYLE + if xp { 0xb } else { 0xc };
            category = 0x1d;
            let key = match (xp, f.value < 0) {
                (true, true) => "Feedback_LostXP",
                (true, false) => "Feedback_ReceivedXP",
                (false, true) => "Feedback_LostSK",
                (false, false) => "Feedback_GainedSK",
            };
            text = fmt1(texts, key, &[if xp { Arg::UInt(abs) } else { Arg::Int(abs as i32) }]);
        }
        ty::ALIEN_XP => {
            if f.value < 1 {
                return None;
            }
            style = STYLE + 0xb;
            category = 0x1d;
            text = fmt1(texts, "Feedback_GainedAlienXP", &[Arg::Int(f.value)]);
        }
        ty::SPECIAL_HIT_YOU => {
            style = STYLE + 7;
            category = 0x17;
            text = match (f.extra, f.b) {
                (Some(x), Some(b)) => fmt1(texts, "Feedback_HitYouForPointsOfDamage", &[s(b.name), Arg::UInt(v), s(x)]),
                _ => String::new(),
            };
        }
        ty::SPECIAL_OTHER | ty::SPECIAL_OTHER_PVP => {
            let pvp = f.ty == ty::SPECIAL_OTHER_PVP;
            style = STYLE + 9 + u32::from(!pvp);
            category = 0x1a + u32::from(pvp);
            text = match (f.extra, f.b) {
                (Some(x), None) => fmt1(texts, "Feedback_SomethingHitOther", &[s(name_a), Arg::UInt(v), s(x)]),
                (Some(x), Some(b)) => fmt1(texts, "Feedback_MonsterHitWithSpecial", &[s(b.name), s(name_a), Arg::UInt(v), s(x)]),
                (None, _) => String::new(),
            };
        }
        ty::SPECIAL_YOU_HIT => {
            style = STYLE + 8;
            category = 0x19;
            text = match f.extra {
                Some(x) => fmt1(texts, "Feedback_HitWithSpecial", &[s(name_a), Arg::UInt(v), s(x)]),
                None => String::new(),
            };
        }
        ty::MISS => {
            number = None;
            category = 0;
            let b_client = f.b.is_some_and(|b| b.is_client);
            if !b_client {
                if !f.a.is_client {
                    return Some(Rendered { style: 0, category: 0, text: String::new(), number: None });
                }
                // the client was attacked: B (the one that missed) is named
                style = STYLE + 0x13;
                let nb = f.b.map_or("", |b| b.name);
                text = match f.extra {
                    None => fmt1(texts, "Feedback_OtherTriedToHitMissed", &[s(nb)]),
                    Some(x) => fmt1(texts, "OtherTriesToAttackWithSpecialButMisses", &[s(nb), s(x)]),
                };
            } else {
                style = STYLE + 0x12;
                text = match f.extra {
                    None => fmt1(texts, "Feedback_YouTriedToHitMissed", &[s(name_a)]),
                    Some(x) => fmt1(texts, "TryToAttackWithSpecialButMiss", &[s(name_a), s(x)]),
                };
            }
        }
        _ => return None,
    }
    // [GC 0x10014bb6..0x10014cb5]: hit flag 4 / 2 appends the critical / glancing text to a non-empty line
    if !text.is_empty() {
        let extra = match f.hit {
            4 => texts.feedback("Feedback_CriticalHit"),
            2 => texts.feedback("Feedback_GlancingHit"),
            _ => None,
        };
        if let Some(x) = extra {
            text = format!("{text} {x}");
        }
    }
    Some(Rendered { style, category, text, number })
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::collections::HashMap;

    /// The real English strings (`text.mdb` category 110, copied from the client) for the keys the tests exercise.
    pub struct Fixed(pub HashMap<&'static str, &'static str>);

    impl Fixed {
        pub fn new() -> Self {
            Self(
                [
                    ("Feedback_WereHitForPointsOfDamage", "You were hit for %u points of damage."),
                    ("Feedback_HitYouForPointsOfDamage", "%s hit you for %u points of %s damage."),
                    ("Feedback_PlayerHitYou", "A player hit you for %u points of damage."),
                    ("Feedback_PlayerHitYouForPointsOfDamage", "Player %s hit you for %u points of %s damage."),
                    ("Feedback_HitWithSpecial", "You hit %s for %u points of %s damage."),
                    ("Feedback_OtherHitOther", "%s hit %s for %u points of %s damage."),
                    ("Feedback_SomethingHitOther", "Something hit %s for %u points of %s damage."),
                    ("Feedback_MonsterHitWithSpecial", "%s hit %s for %u points of %s damage."),
                    ("Feedback_HealedForPoints", "You were healed for %u points."),
                    ("Feedback_YouTriedToHitMissed", "You tried to hit %s, but missed!"),
                    ("Feedback_OtherTriedToHitMissed", "%s tried to hit you, but missed!"),
                    ("TryToAttackWithSpecialButMiss", "You try to attack %s with %s, but you miss!"),
                    ("Feedback_CriticalHit", "Critical hit!"),
                    ("Feedback_GlancingHit", "Glancing hit."),
                    ("Feedback_WasAttackedByNanobots", "%s was attacked with nanobots for %u points of %s damage."),
                    ("Feedback_AttackedByNanobotsForPointsOfDamage", "You were attacked with nanobots for %u points of %s damage."),
                    ("Feedback_ReceivedXP", "You received %u xp."),
                    ("Feedback_LostXP", "You lost %u xp."),
                    ("Feedback_Attacking", "Attacking %s..."),
                    ("Feedback_AttackedBy", "Attacked by %s!"),
                    ("Feedback_UseAggDefSlider", "Use the Def-Agg slider in the Stats view to change between defensive and aggressive."),
                    ("Feedback_DeathByWeaponDamage", "You died of weapon damage!"),
                    ("Feedback_YouWereHitForPointsOfDamage", "You were hit for %u points of damage."),
                ]
                .into_iter()
                .collect(),
            )
        }
    }

    impl Texts for Fixed {
        fn feedback(&self, key: &str) -> Option<String> {
            self.0.get(key).map(|s| s.to_string())
        }
        fn stat_name(&self, stat: u32) -> Option<String> {
            (stat == 142).then(|| "Brawling".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::Fixed;
    use super::*;

    fn who(name: &str, c: bool) -> Who<'_> {
        Who { id: name.len() as i32 * 100 + i32::from(c), name, is_client: c }
    }

    #[test]
    fn ldb_format_semantics() {
        // fed in order, spaces collapsed, an unfed conversion stays literally (Dump [LDB 0x10004885])
        let l = LdbFormat::new("Something hit %s for %u points of %s damage.").feed(Arg::Str("Bob")).feed(Arg::UInt(17));
        assert_eq!(l.dump(), "Something hit Bob for 17 points of %s damage.");
        let l = LdbFormat::new("  a  %s  b").feed(Arg::Str("x"));
        assert_eq!(l.dump(), "a x b");
        // `%s` fed a number, `%%` literal
        assert_eq!(LdbFormat::new("%s 100%%").feed(Arg::UInt(5)).dump(), "uint_value<5> 100%");
        // a replaced value that itself starts with '%' is not replaced again
        assert_eq!(LdbFormat::new("%s %s").feed(Arg::Str("%u")).feed(Arg::Str("b")).dump(), "%u b");
    }

    #[test]
    fn hit_you_and_crit() {
        let t = Fixed::new();
        let mut f = Feedback::new(ty::HIT_YOU, who("Testy", true), 17);
        f.b = Some(who("Cross-Wired Junkbot", false));
        f.stat = 0x5a;
        let r = render(&t, &f).unwrap();
        assert_eq!(r.text, "Cross-Wired Junkbot hit you for 17 points of projectile damage.");
        assert_eq!((r.style, r.category, r.number), (0x4200_0006, 0x17, Some(17)));
        f.hit = 4;
        assert_eq!(render(&t, &f).unwrap().text, "Cross-Wired Junkbot hit you for 17 points of projectile damage. Critical hit!");
        f.b = None;
        assert_eq!(render(&t, &f).unwrap().text, "You were hit for 17 points of damage. Critical hit!");
    }

    #[test]
    fn miss_texts_have_no_number() {
        let t = Fixed::new();
        let mut f = Feedback::new(ty::MISS, who("Testy", true), 1);
        f.b = Some(who("Junkbot", false));
        let r = render(&t, &f).unwrap();
        assert_eq!((r.text.as_str(), r.number, r.category, r.style), ("Junkbot tried to hit you, but missed!", None, 0, 0x4200_0013));
        let mut f = Feedback::new(ty::MISS, who("Junkbot", false), 1);
        f.b = Some(who("Testy", true));
        assert_eq!(render(&t, &f).unwrap().text, "You tried to hit Junkbot, but missed!");
        f.extra = Some("Brawl");
        assert_eq!(render(&t, &f).unwrap().text, "You try to attack Junkbot with Brawl, but you miss!");
        // two bystanders: nothing printed
        let mut f = Feedback::new(ty::MISS, who("A", false), 1);
        f.b = Some(who("B", false));
        assert_eq!(render(&t, &f).unwrap().text, "");
    }

    #[test]
    fn other_hit_other_leaves_the_unfed_conversion() {
        let t = Fixed::new();
        let f = Feedback::new(ty::OTHER_HIT_OTHER, who("Bob", false), 9);
        assert_eq!(render(&t, &f).unwrap().text, "Something hit Bob for 9 points of %s damage.");
        let mut f = Feedback::new(ty::OTHER_HIT_OTHER, who("Bob", false), 9);
        f.b = Some(who("Rat", false));
        f.stat = 0x5b;
        let r = render(&t, &f).unwrap();
        assert_eq!((r.text.as_str(), r.category, r.style), ("Rat hit Bob for 9 points of melee damage.", 0x1a, 0x4200_000a));
    }

    #[test]
    fn special_attack_texts() {
        let t = Fixed::new();
        let mut f = Feedback::new(ty::SPECIAL_YOU_HIT, who("Junkbot", false), 20);
        f.b = Some(who("Testy", true));
        f.extra = Some("Brawl");
        assert_eq!(render(&t, &f).unwrap().text, "You hit Junkbot for 20 points of Brawl damage.");
        f.extra = None;
        let r = render(&t, &f).unwrap();
        assert_eq!((r.text.as_str(), r.number), ("", Some(20)), "no extra: no line but the number is still shown");
    }

    #[test]
    fn tables() {
        assert_eq!(stat_to_string(142), "Brawl");
        assert_eq!(stat_to_string(0x1b), "Health");
        assert_eq!(stat_to_string(0x7fff_0000), "Missing stat: 2147418112");
        assert_eq!(damage_type_name(weapon_damage_type(0)), "projectile");
        assert_eq!(damage_type_name(weapon_damage_type(0xa8)), "nano");
        assert_eq!(gui_color(0x17), 0xff0000);
        assert_eq!(gui_color(0x99), 0xff00ff);
        assert_eq!(world_color(0x19), 0xffd9d9d2);
        assert_eq!(world_color(0x53), 0xffff0000);
        assert_eq!(floating_number(Space::World, 0x15).life, 1.3);
        assert_eq!(floating_number(Space::Hud, 0x15).color, 0xffffff);
    }
}
