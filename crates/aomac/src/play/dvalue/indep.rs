//! `IndependentPrefs_t` (InstanceManager.dll): the second, older prefs store next to the DValues. Two sets (type 0 = login prefs,
//! file `<prefs>/<account>/Login.cfg`; type 1 = character prefs, `<prefs>/<account>/Char<id>/Char.cfg`), each with an int, a float
//! and a string map (`this + 0x54/0x58/0x5c` and `0x60/0x64/0x68`). An int / float pref has value, min and max (+0x28/+0x2c/+0x30);
//! a set clamps. File format (`Save` 0x10001d6a, `Load` 0x100034bf): CRLF text lines `name %d`, `name %f`, `name "string"`,
//! ints first, then floats, then strings, each map in name order; `username` / `password` are never written or read.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Login,
    Char,
}

#[derive(Clone, Debug)]
struct Pref<T> {
    value: T,
    min: T,
    max: T,
}

#[derive(Clone, Debug, Default)]
struct Set {
    ints: BTreeMap<String, Pref<i32>>,
    floats: BTreeMap<String, Pref<f32>>,
    strs: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub struct IndepPrefs {
    login: Set,
    chr: Set,
    changed: Vec<String>,
}

/// `SetDefaultLoginPrefs` (GUI 0x10124b33): `InitDefaultInt(name, default, min, max, 0)`; read off the PUSH operands.
const LOGIN_INTS: &[(&str, i32, i32, i32)] = &[
    ("WasCharacterCreated", 1, 0, 1),
    ("CCSelectedBreed", 0, 0, 7),
    ("CCSelectedHeight", 0, 0, 3),
    ("CCSelectedSize", 0, 0, 3),
    ("CCSelectedHead", 0, 0, 300),
    ("CCSelectedProfession", 0, 0, 14),
    ("PreferredCameraMode", 3, 0, 3),
    ("UseNoBobCamera", 0, 0, 1),
    ("IsChatHidden", 0, 0, 1),
    ("StarRotation", 1, 0, 1),
    ("RealisticMoons", 1, 0, 1),
    ("UseWindEngine", 1, 0, 1),
    ("UseCloudShadows", 1, 0, 1),
    ("UseParticleWeightPhysics", 1, 0, 1),
    ("UseStormParticles", 1, 0, 1),
    ("RealisticWater", 1, 0, 1),
    ("Wildlife", 1, 0, 1),
    ("RealisticClouds", 1, 0, 1),
    ("SimpleClouds", 1, 0, 1),
    ("Shadows", 1, 0, 1),
    ("BuffsFX", 1, 0, 1),
    ("TracersFX", 1, 0, 1),
    ("NanoEffectFX", 1, 0, 1),
    ("MuzzleFlashFX", 1, 0, 1),
    ("EnvironmentFX", 1, 0, 1),
    ("OthersFX", 1, 0, 1),
    ("ShowAllNames", 1, 0, 1),
    ("SmoothAnimations", 1, 0, 1),
    ("ShowIPPerkbutton", 1, 0, 1),
    ("ChatIndicator", 1, 0, 1),
    ("3rdPersonCamera", 1, 0, 1),
    ("ShowNPCQuestions", 1, 0, 1),
    ("MouseLookInverted", 0, 0, 1),
    ("FogMode", 3, 0, 3),
    ("GroundRendering", 3, 0, 3),
];

/// `InitDefaultFloat(name, default, min, max, 0)`. The camera floats are the direction / distance of `PreferredCam*` (docs/zone/camera.md).
const LOGIN_FLOATS: &[(&str, f32, f32, f32)] = &[
    ("PreferredCamPosX", 0.0, -200.0, 200.0),
    ("PreferredCamPosY", 0.316, -200.0, 200.0),
    ("PreferredCamPosZ", -0.948, -200.0, 200.0),
    ("PreferredCamDist", 5.0, 0.0, 347.0),
    ("AspectRation", 4.0 / 3.0, 0.0, 999_999.0),
    ("ViewDistance", 0.8, 0.0, 1.0),
];

/// `SetDefaultCharPrefs` (GUI 0x1012447a): the same operand layout with type 1.
const CHAR_INTS: &[(&str, i32, i32, i32)] = &[
    ("IsFirstTime", 1, 0, 1),
    ("ShowDropItemDialog", 1, 0, 1),
    ("IsOrgNameShownOverHead", 1, 0, 1),
    ("IsSpaceShipsShown", 1, 0, 1),
    ("BuildMenuX", 0, 0, 9999),
    ("BuildMenuY", 0, 0, 9999),
    ("BuildMenuCX", 0, 0, 1000),
    ("BuildMenuCY", 0, 0, 1000),
    ("IsFactionTitleShown", 1, 0, 1),
    ("WaitForVertSync", 0, 0, 1),
    ("MouseLagFix", 0, 0, 1),
    ("UseOffscreenSurfaceTechnology", 0, 0, 1),
    ("FadeCharacter", 1, 0, 1),
];
const CHAR_FLOATS: &[(&str, f32, f32, f32)] = &[("FadeCharacterStartDist", 1.5, 0.1, 10.0), ("FadeCharacterEndDist", 0.7, 0.1, 10.0), ("FadeCharacterEndAlpha", 0.15, 0.0, 1.0)];

impl IndepPrefs {
    /// The registered defaults of both sets (the string pref `CCSelectedName` defaults to "").
    pub fn with_defaults() -> Self {
        let mut p = Self::default();
        for &(n, d, lo, hi) in LOGIN_INTS {
            p.init_int(n, d, lo, hi, Kind::Login);
        }
        for &(n, d, lo, hi) in LOGIN_FLOATS {
            p.init_float(n, d, lo, hi, Kind::Login);
        }
        p.set_string("CCSelectedName", "", Kind::Login);
        for &(n, d, lo, hi) in CHAR_INTS {
            p.init_int(n, d, lo, hi, Kind::Char);
        }
        for &(n, d, lo, hi) in CHAR_FLOATS {
            p.init_float(n, d, lo, hi, Kind::Char);
        }
        p.changed.clear();
        p
    }

    fn set_of(&self, k: Kind) -> &Set {
        match k {
            Kind::Login => &self.login,
            Kind::Char => &self.chr,
        }
    }

    fn set_mut(&mut self, k: Kind) -> &mut Set {
        match k {
            Kind::Login => &mut self.login,
            Kind::Char => &mut self.chr,
        }
    }

    /// `InitDefaultInt` (0x10002bb5): registers a pref, or on an existing one replaces min / max and re-sets the value to `def`.
    pub fn init_int(&mut self, name: &str, def: i32, min: i32, max: i32, k: Kind) {
        match self.set_mut(k).ints.get_mut(name) {
            None => {
                self.set_mut(k).ints.insert(name.into(), Pref { value: def, min, max });
            }
            Some(p) => {
                (p.min, p.max) = (min, max);
                self.set_int(name, def, k);
            }
        }
    }

    /// `InitDefaultFloat` (0x10002d04).
    pub fn init_float(&mut self, name: &str, def: f32, min: f32, max: f32, k: Kind) {
        match self.set_mut(k).floats.get_mut(name) {
            None => {
                self.set_mut(k).floats.insert(name.into(), Pref { value: def, min, max });
            }
            Some(p) => {
                (p.min, p.max) = (min, max);
                self.set_float(name, def, k);
            }
        }
    }

    /// `SetPrefInt` (0x10002e5c): an unregistered name is registered first with an unbounded range; the value is clamped, and the
    /// change callbacks run (here: [`IndepPrefs::take_changed`]). Returns the stored value.
    pub fn set_int(&mut self, name: &str, v: i32, k: Kind) -> i32 {
        let Some(p) = self.set_mut(k).ints.get_mut(name) else {
            self.init_int(name, v, i32::MIN, i32::MAX, k);
            return self.set_int(name, v, k);
        };
        p.value = v.max(p.min).min(p.max);
        let out = p.value;
        self.changed.push(name.into());
        out
    }

    /// `SetPrefFloat` (0x10002f4f); the range of an unregistered name is `-FLT_MAX..FLT_MAX` (0x10008264).
    pub fn set_float(&mut self, name: &str, v: f32, k: Kind) -> f32 {
        let Some(p) = self.set_mut(k).floats.get_mut(name) else {
            self.init_float(name, v, f32::MIN, f32::MAX, k);
            return self.set_float(name, v, k);
        };
        p.value = if v < p.min { p.min } else if p.max < v { p.max } else { v };
        let out = p.value;
        self.changed.push(name.into());
        out
    }

    pub fn set_string(&mut self, name: &str, v: &str, k: Kind) {
        self.set_mut(k).strs.insert(name.into(), v.into());
        self.changed.push(name.into());
    }

    #[cfg(test)]
    pub fn get_int(&self, name: &str, k: Kind) -> Option<i32> {
        self.set_of(k).ints.get(name).map(|p| p.value)
    }

    /// The int pref `name` of whichever set has it (login first), for readers outside the DValue code (`ShowNPCQuestions` of the NPC chat view).
    pub fn int_any(&self, name: &str) -> Option<i32> {
        [Kind::Login, Kind::Char].into_iter().find_map(|k| self.set_of(k).ints.get(name).map(|p| p.value))
    }

    pub fn get_float(&self, name: &str, k: Kind) -> Option<f32> {
        self.set_of(k).floats.get(name).map(|p| p.value)
    }

    #[cfg(test)]
    pub fn get_string(&self, name: &str, k: Kind) -> Option<&str> {
        self.set_of(k).strs.get(name).map(String::as_str)
    }

    /// Names set since the last call (the int / float / string changed callbacks).
    pub fn take_changed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.changed)
    }

    /// `GetPrefEasy` (0x10002368): looks the name up in the login int, float, string maps then the char ones and describes the value
    /// (`Login-pref <%s> is <%d|%f|%s>`, `Char-pref ...`). `None` when no map has it.
    pub fn get_easy(&self, name: &str) -> Option<String> {
        for (k, label) in [(Kind::Login, "Login"), (Kind::Char, "Char")] {
            let s = self.set_of(k);
            let v = if let Some(p) = s.ints.get(name) {
                p.value.to_string()
            } else if let Some(p) = s.floats.get(name) {
                format!("{:.6}", f64::from(p.value))
            } else if let Some(v) = s.strs.get(name) {
                v.clone()
            } else {
                continue;
            };
            return Some(format!("{label}-pref <{name}> is <{v}>"));
        }
        None
    }

    /// `SetPrefEasy` (0x10003076): the value text is read as `atol` / `atof` / string by the type of the registered pref
    /// (`Changed login-pref <%s> to <%d|%f|%s>`). The int message shows the parsed number, the float message the clamped value
    /// (the decompile drops the int / string format arguments: [INFERENCE]). `None` when no map has the name.
    pub fn set_easy(&mut self, name: &str, value: &str) -> Option<String> {
        for (k, label) in [(Kind::Login, "login"), (Kind::Char, "char")] {
            let s = self.set_of(k);
            let (is_int, is_float, is_str) = (s.ints.contains_key(name), s.floats.contains_key(name), s.strs.contains_key(name));
            let shown = if is_int {
                let n = atol(value);
                self.set_int(name, n, k);
                n.to_string()
            } else if is_float {
                format!("{:.6}", f64::from(self.set_float(name, atof(value), k)))
            } else if is_str {
                self.set_string(name, value, k);
                value.to_string()
            } else {
                continue;
            };
            return Some(format!("Changed {label}-pref <{name}> to <{shown}>"));
        }
        None
    }

    /// `Load` (0x100034bf) of the text of one `.cfg` file: first token = name, second = value; blank lines and lines starting with
    /// `/` or `#` are skipped, `username` / `password` too; a value starting with `"` is the string up to the next `"` (kept only when
    /// not empty); a value with a `.` is a float (`%f`), anything else an int (`%i`: `0x` hex, leading `0` octal).
    pub fn load(&mut self, text: &str, k: Kind) {
        for line in text.lines() {
            let mut it = line.split_whitespace();
            let Some(name) = it.next().filter(|n| !n.starts_with(['/', '#'])) else { continue };
            if name.eq_ignore_ascii_case("username") || name.eq_ignore_ascii_case("password") {
                continue;
            }
            let val = it.next().unwrap_or("");
            if val.starts_with('"') {
                let body = line.split_once('"').map_or("", |(_, r)| r);
                let s = body.split('"').next().unwrap_or("");
                if !s.is_empty() {
                    self.set_string(name, s, k);
                }
            } else if val.contains('.') {
                self.set_float(name, atof(val), k);
            } else if !val.is_empty() {
                self.set_int(name, scan_int(val), k);
            }
        }
        self.changed.clear();
    }

    /// `Save` (0x10001d6a): `name %d\r\n` ints, `name %f\r\n` floats, `name "%s"\r\n` strings.
    pub fn save(&self, k: Kind) -> String {
        let s = self.set_of(k);
        let mut out = String::new();
        for (n, p) in &s.ints {
            out += &format!("{n} {}\r\n", p.value);
        }
        for (n, p) in &s.floats {
            out += &format!("{n} {:.6}\r\n", f64::from(p.value));
        }
        for (n, v) in s.strs.iter().filter(|(n, _)| !n.eq_ignore_ascii_case("username") && !n.eq_ignore_ascii_case("password")) {
            out += &format!("{n} \"{v}\"\r\n");
        }
        out
    }
}

/// C `atol`: optional blanks and sign, then digits (0 when there are none; saturating instead of wrapping).
pub(super) fn atol(s: &str) -> i32 {
    let t = s.trim_start();
    let n = t.char_indices().take_while(|(i, c)| c.is_ascii_digit() || (*i == 0 && (*c == '-' || *c == '+'))).count();
    t[..n].parse::<i64>().unwrap_or(0).clamp(i32::MIN.into(), i32::MAX.into()) as i32
}

/// C `atof` on the longest numeric prefix.
pub(super) fn atof(s: &str) -> f32 {
    let t = s.trim_start();
    (1..=t.len()).rev().find_map(|n| t.get(..n)?.parse::<f32>().ok().filter(|_| !t[..n].ends_with(['e', 'E', '+', '-']))).unwrap_or(0.0)
}

/// `sscanf("%i")`: decimal, `0x` hex or leading-`0` octal; 0 when nothing parses.
fn scan_int(s: &str) -> i32 {
    let (neg, t) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (radix, digits) = match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        Some(h) => (16, h),
        None if t.len() > 1 && t.starts_with('0') => (8, &t[1..]),
        None => (10, t),
    };
    let n = digits.chars().take_while(|c| c.is_digit(radix)).count();
    let v = i64::from_str_radix(&digits[..n], radix).unwrap_or(0);
    (if neg { -v } else { v }).clamp(i32::MIN.into(), i32::MAX.into()) as i32
}
