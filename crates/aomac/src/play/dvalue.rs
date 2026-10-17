//! The client's distributed-value store (`DistributedValue_c`, Utils.dll) and its two prefs files per account / character
//! (docs/chat/dvalue.md). Variables are typed (`Variant`), carry optional min / max (a set clamps to them), belong to a
//! category (0 Variables, 1 Main, 2 Login, 3 Char; `ControlCenterModule_c::LoadMainConfig` GUI 0x1006c371) and are loaded in
//! two passes: the defaults of `cd_image/gui/Default/{Variables,MainPrefs,LoginPrefs,CharPrefs}.xml` (`LoadConfig(path, cat, true)` =
//! `AddVariable`), then the user's `Prefs.xml` files (`LoadConfig(path, cat, false)` = `SetDValue` of variables that exist).
//! `SaveConfig(path, cat)` writes every persistent variable of one category back as `<Root><Value name= value=/>…`.

mod cmd;
mod indep;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ao_gui::xml::{self, Element};

pub use cmd::Out;
pub use indep::IndepPrefs;
pub use indep::Kind;

/// `DValueCategory_e` (valid range 0..5, `AddVariable` ignores others).
pub const CAT_VARIABLES: u8 = 0;
pub const CAT_MAIN: u8 = 1;
pub const CAT_LOGIN: u8 = 2;
pub const CAT_CHAR: u8 = 3;

/// `Variant` (Utils.dll) as far as prefs files carry it: type codes 4 int32 / 5 int64 -> [`Variant::Int`], 6 bool, 7 float,
/// 9 string, 0xc message (an `<Archive>`, kept as its XML text); geometry values (`Rect(..)`, `Point(..)`, `IRect`, `IPoint`,
/// `Identity`, `Vector3`, `Ptr`) are kept as the text they were written with.
#[derive(Clone, Debug, PartialEq)]
pub enum Variant {
    Void,
    Int(i64),
    Bool(bool),
    Float(f32),
    Str(String),
    Raw(String),
    Archive(String),
}

impl Variant {
    /// `Variant::LoadFromString` (Utils 0x10012cb9): `true`/`false` (any case) -> bool; an optional `-` and digits with at most one
    /// `.` -> float / int (`sscanf %I64d`); `"..."` -> string (the last character is dropped whatever it is; shorter than 3 characters
    /// -> empty); `Rect(`, `Point(`, `Identity(`, `Vector3(`, `IRect(`, `IPoint(`, `Ptr(` forms; `void`; anything else fails.
    pub fn from_text(s: &str) -> Option<Variant> {
        if s.eq_ignore_ascii_case("false") {
            return Some(Variant::Bool(false));
        }
        if s.eq_ignore_ascii_case("true") {
            return Some(Variant::Bool(true));
        }
        let body = s.strip_prefix('-').unwrap_or(s);
        let mut dots = 0;
        if body.chars().all(|c| {
            dots += (c == '.') as u32;
            c.is_ascii_digit() || (c == '.' && dots == 1)
        }) {
            return Some(if dots == 1 { Variant::Float(s.parse().unwrap_or(0.0)) } else { Variant::Int(s.parse().unwrap_or(0)) });
        }
        if let Some(rest) = s.strip_prefix('"') {
            // `len - 2` characters starting after the quote
            let n = rest.chars().count();
            return Some(Variant::Str(if n < 2 { String::new() } else { rest.chars().take(n - 1).collect() }));
        }
        if s.eq_ignore_ascii_case("void") {
            return Some(Variant::Void);
        }
        ["Rect(", "Point(", "Identity(", "Vector3(", "IRect(", "IPoint(", "Ptr("].iter().any(|p| s.starts_with(p)).then(|| Variant::Raw(s.to_string()))
    }

    /// `Variant::SaveToString` (0x10012ff6): strings are quoted, floats are `%f`.
    pub fn to_text(&self) -> String {
        match self {
            Variant::Void => "void".into(),
            Variant::Int(v) => v.to_string(),
            Variant::Bool(b) => b.to_string(),
            Variant::Float(f) => format!("{:.6}", f64::from(*f)),
            Variant::Str(s) => format!("\"{s}\""),
            Variant::Raw(s) => s.clone(),
            Variant::Archive(_) => "[Archive]".into(),
        }
    }

    /// `Variant::AsString` (0x100131fd): `void`, `%d`, `true`/`false`, `%f`, the raw string, `[Archive]`; other types give "".
    pub fn as_string(&self) -> String {
        match self {
            Variant::Str(s) => s.clone(),
            Variant::Raw(_) => String::new(),
            v => v.to_text(),
        }
    }

    /// `Variant::AsInt64`-like numeric view: bool 0/1, floats truncated, strings through `atol`.
    pub fn as_i64(&self) -> i64 {
        match self {
            Variant::Int(v) => *v,
            Variant::Bool(b) => i64::from(*b),
            Variant::Float(f) => *f as i64,
            Variant::Str(s) => {
                let t = s.trim_start();
                let n = t.char_indices().take_while(|(i, c)| c.is_ascii_digit() || (*i == 0 && (*c == '-' || *c == '+'))).count();
                t[..n].parse().unwrap_or(0)
            }
            _ => 0,
        }
    }

    fn num(&self) -> Option<f64> {
        match self {
            Variant::Int(v) => Some(*v as f64),
            Variant::Bool(b) => Some(f64::from(u8::from(*b))),
            Variant::Float(f) => Some(f64::from(*f)),
            _ => None,
        }
    }

    /// `Variant::operator<` / `>` for the types a min / max can have: numbers (promoted) and strings; anything else is unordered.
    pub fn order(&self, o: &Variant) -> Option<std::cmp::Ordering> {
        match (self, o) {
            (Variant::Str(a), Variant::Str(b)) => Some(a.cmp(b)),
            _ => self.num()?.partial_cmp(&o.num()?),
        }
    }
}

/// `DistributedValue_c::ValueNode_c`: value +0x50, persistent byte +0x60 (`keep_default`), has-min / has-max +0x61 / +0x62,
/// min +0x68, max +0x78, category +0x88.
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub value: Variant,
    pub min: Option<Variant>,
    pub max: Option<Variant>,
    pub category: u8,
    pub persist: bool,
}

impl Node {
    /// The clamp of `SetDValue` (0x25a7): below the minimum -> the minimum, else above the maximum -> the maximum.
    fn clamped(&self, v: &Variant) -> Variant {
        use std::cmp::Ordering::{Greater, Less};
        if let Some(m) = self.min.as_ref().filter(|m| v.order(m) == Some(Less)) {
            return m.clone();
        }
        match &self.max {
            Some(m) if v.order(m) == Some(Greater) => m.clone(),
            _ => v.clone(),
        }
    }
}

/// Where the user's prefs of the logged in character live (`LoadUserConfig` GUI 0x1006bacd: `<prefs>/Prefs.xml`,
/// `<prefs>/<account>/Prefs.xml` + `Login.cfg`, `<prefs>/<account>/Char<id>/Prefs.xml` + `Char.cfg`).
#[derive(Clone, Debug)]
struct UserFiles {
    main: PathBuf,
    login: PathBuf,
    chr: PathBuf,
    login_cfg: PathBuf,
    char_cfg: PathBuf,
}

#[derive(Default)]
pub struct DValues {
    nodes: BTreeMap<String, Node>,
    /// Names whose value changed since [`DValues::take_changed`] (the `DValueChanged` signal).
    changed: Vec<String>,
    /// `IndependentPrefs_t::GetInstance()` (`Login.cfg` / `Char.cfg`).
    pub prefs: IndepPrefs,
    files: Option<UserFiles>,
}

impl DValues {
    /// The defaults of the shipped templates (client dir = the install holding `cd_image`) and of the `IndependentPrefs`
    /// (`SetDefaultLoginPrefs` / `SetDefaultCharPrefs`).
    pub fn new(client_dir: &Path) -> Self {
        let mut s = Self { prefs: IndepPrefs::with_defaults(), ..Self::default() };
        for (file, cat) in [("Variables.xml", CAT_VARIABLES), ("MainPrefs.xml", CAT_MAIN), ("LoginPrefs.xml", CAT_LOGIN), ("CharPrefs.xml", CAT_CHAR)] {
            if let Ok(t) = std::fs::read_to_string(client_dir.join("cd_image/gui/Default").join(file)) {
                s.load_config(&t, cat, true);
            }
        }
        // the three server-side flag options `MiscOptionsMonitor_c` (GUI 0x100bff11) creates with `DistributedValue_c(name)` and fills from the own stat
        // `0x15d` bits 1 / 3 / 4 (the options window's check boxes: auto target monsters / players, disable XP gain); no template defines them [INFERENCE: not persisted]
        for n in ["AutoTargetMOB", "AutoTargetPvP", "DisableXPGain"] {
            s.add(n, Variant::Bool(false), false, CAT_VARIABLES, None, None, false);
        }
        // Gamecode NewLevel (0x10075a0c) creates these session-only GUI notification values.
        for n in ["got_ip", "got_perk", "got_tech"] {
            s.add(n, Variant::Bool(false), false, CAT_VARIABLES, None, None, false);
        }
        // aomac-only "Enhanced" options page (not retail: no `Root.xml` control, no `MainPrefs.xml` template entry). Persisted
        // like any other `DV Main` control (account-independent, survives restart in the root `Prefs.xml`); indices map to
        // concrete renderer values in `options.rs::enhanced_page`/`flow.rs` (audit item 2 texture anisotropy, MSAA-option decision).
        s.add("AomacTextureSharpening", Variant::Int(0), true, CAT_MAIN, Some(Variant::Int(0)), Some(Variant::Int(4)), false);
        s.add("AomacEdgeSmoothing", Variant::Int(0), true, CAT_MAIN, Some(Variant::Int(0)), Some(Variant::Int(3)), false);
        for name in ["AomacSunShadows", "AomacAmbientOcclusion", "AomacEmissiveBloom", "AomacGroundGrass", "AomacHdr"] {
            s.add(name, Variant::Bool(false), true, CAT_MAIN, None, None, false);
        }
        s.add("AomacShadowMapSize", Variant::Int(2048), true, CAT_MAIN, Some(Variant::Int(256)), Some(Variant::Int(4096)), false);
        for (name, default, max) in [
            ("AomacShadowStrength", 0.65, 1.0), ("AomacShadowSoftness", 1.5, 4.0),
            ("AomacAoRadius", 1.0, 5.0), ("AomacAoStrength", 1.0, 1.0), ("AomacBloomStrength", 0.6, 1.0),
            ("AomacGrassDensity", 1.0, 2.0), ("AomacGrassDistance", 50.0, 60.0), ("AomacSunBoost", 0.35, 0.6),
        ] {
            s.add(name, Variant::Float(default), true, CAT_MAIN, Some(Variant::Float(0.0)), Some(Variant::Float(max)), false);
        }
        // Interface scale (audit item 2): 0 Auto (today's whole backing-scale rounding), 1/2/3 fixed GUI pixel scale;
        // read at startup (`Play::new` below, `ao_render::Frontend::interface_scale`) so the login screen honours it too.
        s.add("AomacInterfaceScale", Variant::Int(0), true, CAT_MAIN, Some(Variant::Int(0)), Some(Variant::Int(3)), false);
        // Audit item 6: skip the Spaces fullscreen animation and hide the menu bar/dock (winit `with_borderless_game`)
        // when Ctrl+Cmd+F or the green button enters fullscreen. Off by default (today's animated native fullscreen).
        s.add("AomacBorderlessFullscreen", Variant::Bool(false), true, CAT_MAIN, None, None, false);
        // Audit item 8: trackpad pinch gesture zooms the third-person camera (same 0.78-25 m range as the mouse wheel).
        // Opt-in: not retail, and a pinch over the 3D view has no other meaning to conflict with.
        s.add("AomacPinchZoom", Variant::Bool(false), true, CAT_MAIN, None, None, false);
        // Audit item 12: gamma-correct downsample + coverage-preserving alpha mips (`native_texture::build_texture_mips`).
        // Default off: retail's plain sRGB-byte box filter (randy31 `FUN_100193e4`).
        s.add("AomacBetterMipFiltering", Variant::Bool(false), true, CAT_MAIN, None, None, false);
        // Audit item 16, PRK-signed-off (`agent://EnhanceReview`): keeps the 4:3 vertical angle (`camera::lens`); default
        // off: retail's fixed 90 degree horizontal FOV (`camera::FOV_HORIZONTAL`, N3 `n3Camera_t` ctor).
        s.add("AomacWidescreenFOV", Variant::Bool(false), true, CAT_MAIN, None, None, false);
        // Audit item 15: multiplies the player camera's far plane (terrain/statel LOD + fog end move with it, see
        // `camera::far_plane`/`Renderer::set_lens`); never touches `char_view_distance` (dynels.rs) or the 30 m name-tag
        // radius (ao-net `nametag.rs`), which use their own separate distance checks. Default 1.0x = retail `ViewDistance`.
        s.add("AomacSceneryDistance", Variant::Float(1.0), true, CAT_MAIN, Some(Variant::Float(1.0)), Some(Variant::Float(2.0)), false);
        // Audit item 13: rasterizes TrueType glyphs at the window's backing scale with real coverage antialiasing
        // (`ao_gui::font::TtFont::glyph`'s `scale` parameter / `Flatten::fill_aa`) instead of retail's hinted 1-bit
        // bitmap; bitmap fonts (tooltips, shell, chat, name tags) are always pixel art regardless. Default off.
        s.add("AomacSmoothText", Variant::Bool(false), true, CAT_MAIN, None, None, false);
        s
    }

    /// Independent opt-in effects and bounded quality settings, shared by both viewer frontends.
    pub fn enhanced_settings(&self) -> ao_render::EnhancedSettings {
        let defaults = ao_render::EnhancedSettings::default();
        ao_render::EnhancedSettings {
            sun_shadows: self.flag("AomacSunShadows"),
            ambient_occlusion: self.flag("AomacAmbientOcclusion"),
            emissive_bloom: self.flag("AomacEmissiveBloom"),
            ground_grass: self.flag("AomacGroundGrass"),
            hdr: self.flag("AomacHdr"),
            shadow_map_size: self.get_i64("AomacShadowMapSize").unwrap_or(i64::from(defaults.shadow_map_size)).clamp(256, 4096) as u32,
            shadow_strength: self.get_f32("AomacShadowStrength").unwrap_or(defaults.shadow_strength),
            shadow_softness: self.get_f32("AomacShadowSoftness").unwrap_or(defaults.shadow_softness),
            ao_radius: self.get_f32("AomacAoRadius").unwrap_or(defaults.ao_radius),
            ao_strength: self.get_f32("AomacAoStrength").unwrap_or(defaults.ao_strength),
            bloom_strength: self.get_f32("AomacBloomStrength").unwrap_or(defaults.bloom_strength),
            grass_density: self.get_f32("AomacGrassDensity").unwrap_or(defaults.grass_density),
            grass_distance: self.get_f32("AomacGrassDistance").unwrap_or(defaults.grass_distance),
            sun_boost: self.get_f32("AomacSunBoost").unwrap_or(defaults.sun_boost),
        }.clamped()
    }

    /// Disable effects without changing quality knobs, MSAA or unrelated preferences.
    pub fn restore_original_graphics(&mut self) {
        for name in ["AomacSunShadows", "AomacAmbientOcclusion", "AomacEmissiveBloom", "AomacGroundGrass", "AomacHdr"] {
            self.set(name, Variant::Bool(false));
        }
    }

    /// `DistributedValue_c::LoadConfig(file, cat, defaults)` on already read XML text. `defaults` = true adds the variables
    /// (with `min` / `max` / `keep_default`); false only sets the ones that exist, an `<Archive>` first inheriting the default's
    /// children the user's file lacks. False when the text is not XML.
    pub fn load_config(&mut self, text: &str, cat: u8, defaults: bool) -> bool {
        let Ok(root) = xml::parse(text) else { return false };
        for e in &root.children {
            let Some(name) = e.attr("name") else { continue };
            let keep = e.attr("keep_default").is_none_or(|k| k.eq_ignore_ascii_case("true"));
            if e.name.eq_ignore_ascii_case("Archive") {
                if defaults {
                    self.add(name, Variant::Archive(element_xml(e)), keep, cat, None, None, true);
                } else {
                    let merged = match self.nodes.get(name).map(|n| &n.value) {
                        Some(Variant::Archive(d)) => xml::parse(d).ok().map_or_else(|| e.clone(), |d| merge_archive(e, &d)),
                        _ => e.clone(),
                    };
                    self.set(name, Variant::Archive(element_xml(&merged)));
                }
            } else if e.name.eq_ignore_ascii_case("Value") {
                let Some(v) = e.attr("value").and_then(Variant::from_text) else { continue };
                if defaults {
                    let (min, max) = (e.attr("min").and_then(Variant::from_text), e.attr("max").and_then(Variant::from_text));
                    self.add(name, v, keep, cat, min, max, true);
                } else {
                    self.set(name, v);
                }
            }
        }
        true
    }

    /// `DistributedValue_c::SaveConfig(file, cat)`: the persistent variables of `cat` in name order.
    pub fn save_config(&self, cat: u8) -> String {
        let mut out = String::from("<?xml version=\"1.0\" ?>\n<Root>\n");
        for (name, n) in self.nodes.iter().filter(|(_, n)| n.category == cat && n.persist) {
            match &n.value {
                Variant::Archive(x) => out += &format!("    {x}\n"),
                v => out += &format!("    <Value name={} value={} />\n", quote(name), quote(&v.to_text())),
            }
        }
        out + "</Root>\n"
    }

    /// `LoadUserConfig` (GUI 0x1006bacd) for `account` / character `char_id` under the prefs directory `prefs`: the three `Prefs.xml`
    /// files and `Login.cfg` / `Char.cfg` (`Preferences_t::LoadLoginPrefs` / `LoadCharPrefs`, GUI 0x1012526e / 0x101246af). Remembers the
    /// paths for [`DValues::save_user`].
    pub fn open_user(&mut self, prefs: &Path, account: &str, char_id: u32) {
        let (acct, chr) = (prefs.join(account), prefs.join(account).join(format!("Char{char_id}")));
        let f = UserFiles { main: prefs.join("Prefs.xml"), login: acct.join("Prefs.xml"), chr: chr.join("Prefs.xml"), login_cfg: acct.join("Login.cfg"), char_cfg: chr.join("Char.cfg") };
        for (p, cat) in [(&f.main, CAT_MAIN), (&f.login, CAT_LOGIN), (&f.chr, CAT_CHAR)] {
            if let Ok(t) = std::fs::read_to_string(p) {
                self.load_config(&t, cat, false);
            }
        }
        for (p, kind) in [(&f.login_cfg, Kind::Login), (&f.char_cfg, Kind::Char)] {
            if let Ok(t) = std::fs::read_to_string(p) {
                self.prefs.load(&t, kind);
            }
        }
        self.files = Some(f);
        self.take_changed(); // loading the user's files is not a change to save
    }

    /// The character's prefs directory (`<prefs>/<account>/Char<id>/`, where `Prefs.xml` and the `DockAreas/` files of the `DockingController_c` live);
    /// `None` before [`DValues::open_user`].
    pub fn char_dir(&self) -> Option<&Path> {
        self.files.as_ref().and_then(|f| f.chr.parent())
    }

    /// `ControlCenterModule_c::SaveUserConfig` / `SaveAllConfig` (GUI 0x10067f57 / 0x1006806c): the three XML files and the two `.cfg`
    /// files. No-op before [`DValues::open_user`].
    pub fn save_user(&self) {
        let Some(f) = &self.files else { return };
        let write = |p: &Path, t: String| {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if let Err(e) = std::fs::write(p, t) {
                eprintln!("prefs: {}: {e}", p.display());
            }
        };
        write(&f.main, self.save_config(CAT_MAIN));
        write(&f.login, self.save_config(CAT_LOGIN));
        write(&f.chr, self.save_config(CAT_CHAR));
        write(&f.login_cfg, self.prefs.save(Kind::Login));
        write(&f.char_cfg, self.prefs.save(Kind::Char));
    }

    /// `AddVariable(node, overwrite, ...)` (0x2e9e): a new name is inserted; an existing one is replaced only when `overwrite`.
    #[allow(clippy::too_many_arguments)]
    pub fn add(&mut self, name: &str, value: Variant, persist: bool, category: u8, min: Option<Variant>, max: Option<Variant>, overwrite: bool) {
        if category > 4 {
            return;
        }
        let node = Node { value, min, max, category, persist };
        match self.nodes.get_mut(name) {
            None => {
                self.nodes.insert(name.to_string(), node);
            }
            Some(n) if overwrite => *n = node,
            Some(_) => {}
        }
    }

    pub fn exists(&self, name: &str) -> bool {
        self.nodes.contains_key(name)
    }

    /// Whether `SaveConfig` writes this variable back (`Node::persist`; session-only values like the server-driven
    /// `AutoTargetMOB`/`AutoTargetPvP`/`DisableXPGain` do not).
    #[cfg(test)]
    pub fn persists(&self, name: &str) -> bool {
        self.nodes.get(name).is_some_and(|n| n.persist)
    }

    pub fn get(&self, name: &str) -> Option<&Variant> {
        self.nodes.get(name).map(|n| &n.value)
    }

    /// `GetMinMaxValues`: void when the variable has no such bound.
    pub fn min_max(&self, name: &str) -> (Variant, Variant) {
        self.nodes.get(name).map_or((Variant::Void, Variant::Void), |n| (n.min.clone().unwrap_or(Variant::Void), n.max.clone().unwrap_or(Variant::Void)))
    }

    /// `DistributedValue_c::SetDValue(name, value)` (0x25a7): unknown names are ignored; the value is clamped to min / max, and only a
    /// different value (or type) is stored and signalled. True when the variable exists.
    pub fn set(&mut self, name: &str, value: Variant) -> bool {
        let Some(n) = self.nodes.get_mut(name) else { return false };
        let v = n.clamped(&value);
        if n.value != v {
            n.value = v;
            self.changed.push(name.to_string());
        }
        true
    }

    /// The `DValueChanged` signals (and the `IndependentPrefs` changed callbacks) since the last call.
    pub fn take_changed(&mut self) -> Vec<String> {
        let mut v = std::mem::take(&mut self.changed);
        v.extend(self.prefs.take_changed());
        v
    }

    /// Booleans and integers as numbers (the HUD's flag view); floats, strings and archives give `None`.
    pub fn get_i64(&self, name: &str) -> Option<i64> {
        match self.get(name)? {
            Variant::Int(v) => Some(*v),
            Variant::Bool(b) => Some(i64::from(*b)),
            _ => None,
        }
    }

    pub fn flag(&self, name: &str) -> bool {
        self.get_i64(name).is_some_and(|v| v != 0)
    }

    /// Numbers as `f32` (`Variant::operator float`); strings, archives and unknown names give `None`.
    pub fn get_f32(&self, name: &str) -> Option<f32> {
        match self.get(name)? {
            Variant::Float(f) => Some(*f),
            v @ (Variant::Int(_) | Variant::Bool(_)) => Some(v.as_i64() as f32),
            _ => None,
        }
    }

    /// Sets a number / flag keeping the variable's type (a bool stays a bool); an unknown name becomes a temporary integer.
    pub fn set_i64(&mut self, name: &str, v: i64) {
        match self.get(name) {
            None => self.add(name, Variant::Int(v), false, CAT_VARIABLES, None, None, false),
            Some(Variant::Bool(_)) => {
                self.set(name, Variant::Bool(v != 0));
            }
            Some(_) => {
                self.set(name, Variant::Int(v));
            }
        }
    }

    pub fn iter_i64(&self) -> impl Iterator<Item = (&str, i64)> {
        self.nodes.keys().filter_map(|k| Some((k.as_str(), self.get_i64(k)?)))
    }

    /// Far plane scale of the `ViewDistance` pref (0..1 slider, default 0.8; `FUN_1001fc91` N3 0x1001fc91 multiplies it by 1000 m).
    pub fn view_distance(&self) -> f32 {
        self.prefs.get_float("ViewDistance", Kind::Login).unwrap_or(0.8)
    }

    /// The character view distance in metres (`FUN_1001f964` N3 0x1001f964, the slot of the `DisplayCharViewDistance` DValue): the
    /// integer value, 70 when it is outside 5..=80.
    pub fn char_view_distance(&self) -> f32 {
        let v = self.get("DisplayCharViewDistance").map_or(0, Variant::as_i64);
        if (5..=80).contains(&v) { v as f32 } else { 70.0 }
    }
}

/// Children of the user's archive plus the default children it does not have (`Message` merge in `LoadConfig`).
fn merge_archive(user: &Element, default: &Element) -> Element {
    let mut out = user.clone();
    for d in &default.children {
        if !out.children.iter().any(|c| c.attr("name") == d.attr("name")) {
            out.children.push(d.clone());
        }
    }
    out
}

/// TinyXML's attribute quoting: a value with a `"` is written in single quotes (shipped `prefs/Prefs.xml`: `value='&quot;Window2&quot;'`),
/// the characters `& < >` and `"` as entities.
fn quote(s: &str) -> String {
    let esc = s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    if s.contains('"') { format!("'{esc}'") } else { format!("\"{esc}\"") }
}

pub(super) fn element_xml(e: &Element) -> String {
    let mut s = format!("<{}", e.name);
    for (k, v) in &e.attrs {
        s += &format!(" {k}={}", quote(v));
    }
    if e.children.is_empty() {
        return s + " />";
    }
    s += ">";
    for c in &e.children {
        s += &element_xml(c);
    }
    s + &format!("</{}>", e.name)
}

#[cfg(test)]
mod tests;
