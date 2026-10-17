//! The part of the FXS tweak language that the sky objects use: `Object` blocks, left-to-right expressions and
//! quaternions (`v(axis), degrees`, `q(x,y,z,w)`, `a [ROT] b`).
//!
//! Evaluation context: native `GameDayTime` / `GameSun*Rotation` remain live; the final GAME object maps them to
//! `CurrentDayTime` / `DayTimeFactor` / `Sun*Rotation` (ICC freezes those interfaces). Per-frame delta, wind, integrated
//! counters and `GameWaveCurve*` are zero in a baked still frame; the renderer animates the waves
//! (see `layers::wave_spins` / `uv_wave`).

use std::collections::HashMap;

/// One `Object <name> { ... }`: its own fields (later definitions and `Expansion` blocks override earlier ones; every
/// expansion is active, Project Rubi-Ka runs the full client), plus the render states / texture of its `StateBlob`s.
#[derive(Debug, Default, Clone)]
pub struct Obj {
    pub name: String,
    /// `Type Name: expr` (also without the colon), expression text as written.
    pub fields: HashMap<String, String>,
    /// `RenderState: Unsigned 0u, Unsigned e_D3DRENDERSTATE_<NAME>, Unsigned <value>` as `NAME -> value`; texture stage
    /// states as `TSS_<NAME> -> value`.
    pub states: HashMap<String, String>,
    /// `Texture: ..., String "name.png"` (first one).
    pub texture: Option<String>,
}

impl Obj {
    pub fn field(&self, name: &str) -> Option<&str> {
        self.fields.get(name).map(String::as_str)
    }

    /// `String Mesh: "x.abiff"` → `x.abiff`.
    pub fn string(&self, name: &str) -> Option<&str> {
        self.field(name)?.split('"').nth(1)
    }

    /// `FXID` with the `e_` prefix removed.
    pub fn fxid(&self) -> &str {
        self.field("FXID").map_or("", |s| s.trim().trim_start_matches("e_"))
    }

    /// Render state as boolean (`e_TRUE` / `e_FALSE`).
    pub fn flag(&self, state: &str) -> Option<bool> {
        match self.states.get(state)?.as_str() {
            "e_TRUE" => Some(true),
            "e_FALSE" => Some(false),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Block {
    Plain,
    Expansion,
    StateBlob,
}

const TYPES: [&str; 9] = ["Unsigned", "Float", "Vector", "Quaternion", "String", "Matrix", "Object", "Color", "Sound"];

/// Named objects of the flattened script; a later definition replaces the whole object (FXS1000132f).
pub fn parse_objects(text: &str) -> Vec<Obj> {
    let mut out: Vec<Obj> = Vec::new();
    let mut cur: Option<Obj> = None;
    let mut stack: Vec<Block> = Vec::new();
    let mut pending = Block::Plain;
    let mut lines = text.lines();
    while let Some(raw) = lines.next() {
        let t = raw.trim();
        let Some(obj) = cur.as_mut() else {
            if let Some(n) = t.strip_prefix("Object ") {
                cur = Some(Obj { name: n.trim().to_string(), ..Default::default() });
                stack.clear();
                pending = Block::Plain;
            }
            continue;
        };
        if t.starts_with("Expansion") {
            pending = Block::Expansion;
        } else if t.starts_with("StateBlob") {
            pending = Block::StateBlob;
        }
        // braces never share a line with fields in the data
        if t == "{" {
            stack.push(std::mem::replace(&mut pending, Block::Plain));
            continue;
        }
        if t == "}" {
            if stack.pop().is_none() || stack.is_empty() {
                if let Some(object) = cur.take() {
                    if let Some(previous) = out.iter_mut().find(|o| o.name == object.name && object.name != "Noname") {
                        *previous = object;
                    } else {
                        out.push(object);
                    }
                }
            }
            continue;
        }
        if stack.is_empty() || t.is_empty() {
            continue;
        }
        let in_blob = stack.contains(&Block::StateBlob);
        if let Some(rest) = t.strip_prefix("RenderState:") {
            let parts: Vec<&str> = rest.split(',').map(str::trim).collect();
            if let [_, state, value] = parts[..] {
                let state = state.trim_start_matches("Unsigned").trim().trim_start_matches("e_D3DRENDERSTATE_");
                obj.states.insert(state.to_string(), value.trim_start_matches("Unsigned").trim().to_string());
            }
        } else if let Some(rest) = t.strip_prefix("TextureStage:") {
            // TextureStage: state blob, texture stage, e_D3DTSS_<NAME>, value.
            let parts: Vec<&str> = rest.split(',').map(|p| p.trim().trim_start_matches("Unsigned").trim()).collect();
            if let [_, stage, name, value] = parts[..] {
                let stage: u32 = stage.trim_end_matches('u').parse().unwrap_or(0);
                let prefix = if stage == 0 { "TSS_".to_string() } else { format!("TSS{stage}_") };
                obj.states.insert(name.replace("e_D3DTSS_", &prefix), value.to_string());
            }
        } else if t.starts_with("Texture:") {
            if obj.texture.is_none() {
                obj.texture = t.split('"').nth(1).map(str::to_string);
            }
        } else if !in_blob {
            let mut parts = t.splitn(2, char::is_whitespace);
            let (ty, rest) = (parts.next().unwrap_or(""), parts.next().unwrap_or("").trim());
            if !TYPES.contains(&ty) {
                continue;
            }
            let end = rest.find(|c: char| c.is_whitespace() || c == '[' || c == ':').unwrap_or(rest.len());
            let name = rest[..end].to_string();
            let mut value = rest[end..].trim_start();
            let array = value.starts_with('[');
            if array {
                value = value.split_once(']').map_or("", |(_, v)| v).trim_start();
            }
            let mut expr = value.trim_start_matches(':').trim().to_string();
            // matrices and arrays continue on the following lines until the parentheses balance
            let depth = |s: &str| s.matches('(').count() as i32 - s.matches(')').count() as i32;
            while depth(&expr) > 0 {
                let Some(next) = lines.next() else { break };
                expr.push(' ');
                expr.push_str(next.trim());
            }
            if array && ty == "Float" {
                let mut peek = lines.clone();
                while let Some(next) = peek.next().filter(|line| line.trim_start().starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))) {
                    expr.push(' ');
                    expr.push_str(next.trim());
                    lines = peek.clone();
                }
            }
            // `Vector Vertex [5]:` lists one `v( .. )` per line
            if array && expr.contains("v(") {
                let mut peek = lines.clone();
                while let Some(n) = peek.next().filter(|n| n.trim_start().starts_with("v(")) {
                    expr.push(' ');
                    expr.push_str(n.trim());
                    lines = peek.clone();
                }
            }
            // `Vector UniversePosition [N]` waypoint lists: one expression per following line (`RKWP.Pos_X`,
            // `RKPP.Jobe - PlayfieldData.UniversePosition + v( .. )`, a leading `,`), stored newline separated; the
            // declared size is kept as `UniversePosition[]`
            if array && name == "UniversePosition" && !expr.contains("v(") {
                let size = rest[end..].trim_start().trim_start_matches('[').split(']').next().unwrap_or("").trim().to_string();
                let mut entries: Vec<String> = Some(expr.clone()).filter(|e| !e.is_empty()).into_iter().collect();
                let mut peek = lines.clone();
                while let Some(n) = peek.next() {
                    let n = n.trim().trim_start_matches(',').trim();
                    if n.is_empty() || n == "{" || n == "}" || TYPES.contains(&n.split_whitespace().next().unwrap_or("")) || n.starts_with("Expansion") || n.starts_with("StateBlob") {
                        break;
                    }
                    entries.push(n.to_string());
                    lines = peek.clone();
                }
                expr = entries.join("\n");
                obj.fields.insert("UniversePosition[]".to_string(), size);
            }
            obj.fields.insert(name, expr);
        }
    }
    out
}

/// `a * b + c / d % 1`: numbers (`1.0f`, `0u`, `0x01000000`), identifiers, parentheses, left to right.
pub fn eval(expr: &str, var: &dyn Fn(&str) -> Option<f32>) -> Option<f32> {
    let b = expr.as_bytes();
    let mut pos = 0;
    let v = binary(b, &mut pos, var, 0)?;
    skip_ws(b, &mut pos);
    (pos == b.len()).then_some(v)
}

fn skip_ws(b: &[u8], p: &mut usize) {
    while *p < b.len() && (b[*p] as char).is_whitespace() {
        *p += 1;
    }
}

/// Nesting limit of parentheses / unary minus (the data never exceeds 3; deeper input is rejected, not recursed into).
const MAX_NEST: u32 = 64;

fn binary(b: &[u8], p: &mut usize, var: &dyn Fn(&str) -> Option<f32>, nest: u32) -> Option<f32> {
    let mut acc = primary(b, p, var, nest)?;
    loop {
        skip_ws(b, p);
        let Some(&op) = b.get(*p) else { return Some(acc) };
        let clamp = (op == b'<' && b.get(*p + 1) == Some(&b'|')) || (op == b'|' && b.get(*p + 1) == Some(&b'>'));
        if !b"+-*/%".contains(&op) && !clamp {
            return Some(acc);
        }
        *p += if clamp { 2 } else { 1 };
        let rhs = primary(b, p, var, nest)?;
        acc = match op {
            b'+' => acc + rhs,
            b'-' => acc - rhs,
            b'*' => acc * rhs,
            b'/' => acc / rhs,
            b'<' => acc.min(rhs),
            b'|' => acc.max(rhs),
            _ => acc % rhs,
        };
    }
}

fn primary(b: &[u8], p: &mut usize, var: &dyn Fn(&str) -> Option<f32>, nest: u32) -> Option<f32> {
    if nest > MAX_NEST {
        return None;
    }
    skip_ws(b, p);
    match *b.get(*p)? {
        b'-' => {
            *p += 1;
            Some(-primary(b, p, var, nest + 1)?)
        }
        b'(' => {
            *p += 1;
            let v = binary(b, p, var, nest + 1)?;
            skip_ws(b, p);
            (b.get(*p) == Some(&b')')).then(|| *p += 1)?;
            Some(v)
        }
        c if c.is_ascii_digit() || c == b'.' => {
            let st = *p;
            if b[st..].starts_with(b"0x") {
                *p += 2;
                while *p < b.len() && b[*p].is_ascii_hexdigit() {
                    *p += 1;
                }
                let v = u32::from_str_radix(std::str::from_utf8(&b[st + 2..*p]).ok()?, 16).ok()?;
                return Some(v as f32);
            }
            while *p < b.len() && (b[*p].is_ascii_digit() || b[*p] == b'.') {
                *p += 1;
            }
            let v = std::str::from_utf8(&b[st..*p]).ok()?.parse().ok()?;
            if *p < b.len() && matches!(b[*p], b'f' | b'u') {
                *p += 1;
            }
            Some(v)
        }
        c if c.is_ascii_alphabetic() || c == b'_' => {
            let st = *p;
            while *p < b.len() && (b[*p].is_ascii_alphanumeric() || b[*p] == b'_' || b[*p] == b'.') {
                *p += 1;
            }
            let name = std::str::from_utf8(&b[st..*p]).ok()?;
            skip_ws(b, p);
            if b.get(*p) == Some(&b'[') {
                *p += 1;
                let index = binary(b, p, var, nest + 1)?;
                skip_ws(b, p);
                (b.get(*p) == Some(&b']')).then(|| *p += 1)?;
                return var(&format!("{name}[{index}]"));
            }
            var(name)
        }
        _ => None,
    }
}

/// Entries of a `m( .. )` matrix body: commas separate entries, rows are only separated by whitespace, so the piece
/// holding the fourth entry of a row also starts the next row (the fourth entry is always a plain number).
pub fn eval_matrix(obj: &Obj, ctx: &Ctx, inner: &str) -> Option<Vec<f32>> {
    let mut out: Vec<f32> = Vec::new();
    for piece in inner.split(',') {
        let piece = piece.trim();
        let mut parts = vec![piece];
        if out.len() % 4 == 3 {
            if let Some((a, rest)) = piece.split_once(char::is_whitespace) {
                parts = vec![a, rest.trim()];
            }
        }
        for e in parts {
            out.push(eval_field(obj, ctx, e, 0)?);
        }
    }
    Some(out)
}

/// Unit quaternion in AO's left handed space (D3DX convention: `v' = q v q^-1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub w: f32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Quat {
    pub const IDENTITY: Quat = Quat { w: 1.0, x: 0.0, y: 0.0, z: 0.0 };

    pub fn axis_angle(axis: [f32; 3], degrees: f32) -> Quat {
        let len = axis.iter().map(|c| c * c).sum::<f32>().sqrt();
        if len < 1e-9 {
            return Quat::IDENTITY;
        }
        let (s, c) = (degrees.to_radians() * 0.5).sin_cos();
        Quat { w: c, x: axis[0] / len * s, y: axis[1] / len * s, z: axis[2] / len * s }
    }

    /// Apply `self`, then `next` (`self [ROT] next` in script order).
    pub fn then(self, n: Quat) -> Quat {
        Quat {
            w: n.w * self.w - n.x * self.x - n.y * self.y - n.z * self.z,
            x: n.w * self.x + n.x * self.w + n.y * self.z - n.z * self.y,
            y: n.w * self.y - n.x * self.z + n.y * self.w + n.z * self.x,
            z: n.w * self.z + n.x * self.y - n.y * self.x + n.z * self.w,
        }
    }

    /// The rotation `s` with `base.then(s) == self` (`s` acts after `base`), as (unit axis, degrees in `0..=180`); `None`
    /// when both are the same rotation.
    pub fn spin_from(self, base: Quat) -> Option<([f32; 3], f32)> {
        let inv = Quat { w: base.w, x: -base.x, y: -base.y, z: -base.z };
        let s = inv.then(self);
        let sign = if s.w < 0.0 { -1.0 } else { 1.0 };
        let (w, v) = (s.w * sign, [s.x * sign, s.y * sign, s.z * sign]);
        let sin = v.iter().map(|c| c * c).sum::<f32>().sqrt();
        (sin > 1e-6).then(|| (v.map(|c| c / sin), 2.0 * sin.atan2(w).to_degrees()))
    }

    pub fn rotate(self, v: [f32; 3]) -> [f32; 3] {
        let Quat { w, x, y, z } = self;
        let r = [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
            [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
            [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
        ];
        std::array::from_fn(|i| r[i][0] * v[0] + r[i][1] * v[1] + r[i][2] * v[2])
    }
}

/// What the evaluator knows about the game state.
#[derive(Clone)]
pub struct Ctx {
    pub day_time: f32,
    pub sun1: Quat,
    pub sun2: Quat,
    /// Native exports remain live even when the script's interface time/suns are frozen.
    pub game_exports: Option<(f32, [Quat; 2])>,
    pub day_factor: Option<f32>,
    /// `GAME.ThickCloudsIntensity` (written by the game from the weather).
    pub cloud_intensity: f32,
    /// `GAME.OffsetFromHQ_X/Z`: the playfield's universe position relative to Omni-HQ.
    pub hq_offset: [f32; 2],
    /// `GAME.GameDeltaTime`: 0 for a still frame, 1 to read per-second rates out of the per-frame integrators.
    pub delta_time: f32,
    /// `GAME.HighAltitudeWindX/Z` (the game writes `speed * direction * dt * k` every frame, `FUN_100be767`).
    pub wind: [f32; 2],
    /// `GAME.CurrentNightIntensity` = `NightIntensity[DayTimeFactor]` (`Tweak_GAME.txt`).
    pub night: f32,
    /// `Object.Counter` references of other objects: their growth per second (the counters integrate `GameDeltaTime`).
    pub counters: std::collections::HashMap<String, f32>,
    /// `GAME.GameWaveCurve0..15` (`ao_scene::wave_curves`): 0 in the baked frame; the renderer adds the live waves, see `SkyWaveSpin`
    /// and `Submesh::uv_wave`.
    pub waves: [f32; 16],
}

impl Ctx {
    /// A context for expressions that do not depend on the game (offsets, counters, colours).
    pub fn at(day_time: f32) -> Ctx {
        Ctx { day_time, sun1: Quat::IDENTITY, sun2: Quat::IDENTITY, game_exports: None, day_factor: None, cloud_intensity: 0.0, hq_offset: [0.0; 2], delta_time: 0.0, wind: [0.0; 2], night: 0.0, counters: Default::default(), waves: [0.0; 16] }
    }

    /// DisplaySystem1005cdcf writes GameDayTime/GameSun*Rotation, not the authored interface fields.
    pub fn bind_game(&mut self, game: &Obj) {
        self.game_exports = Some((self.day_time, [self.sun1, self.sun2]));
        let time = game.field("CurrentDayTime").and_then(|e| eval_field(game, self, e, 0));
        let factor = game.field("DayTimeFactor").and_then(|e| eval_field(game, self, e, 0));
        let sun1 = game.field("Sun1Rotation").and_then(|e| rotation(game, self, e, 0));
        let sun2 = game.field("Sun2Rotation").and_then(|e| rotation(game, self, e, 0));
        if let Some(time) = time { self.day_time = time; }
        self.day_factor = factor;
        if let Some(sun1) = sun1 { self.sun1 = sun1; }
        if let Some(sun2) = sun2 { self.sun2 = sun2; }
    }
}

const DAY_LENGTH: f32 = 6480.0;

thread_local! {
    /// Remaining variable/rotation-term evaluations of the current top-level evaluation.
    static BUDGET: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Evaluations one top-level expression may spend on `This.X` references and `[ROT]` terms. The sky scripts need < 20;
/// a hostile script with many references per level would otherwise multiply out to `refs^depth`.
const BUDGET_PER_EVAL: u32 = 2000;

fn reset_budget() {
    BUDGET.with(|b| b.set(BUDGET_PER_EVAL));
}

/// Spends one evaluation; `None` once the budget is gone.
fn spend() -> Option<()> {
    BUDGET.with(|b| b.get().checked_sub(1).map(|v| b.set(v)))
}

/// Value of `name` in `obj`'s expressions (`This.X` follows the field, depth limited because the `Counter` fields
/// integrate themselves).
fn variable(objects: &[Obj], obj: &Obj, ctx: &Ctx, name: &str, depth: u32) -> Option<f32> {
    match name {
        "GAME.GameDayTime" => Some(ctx.game_exports.map_or(ctx.day_time, |e| e.0)),
        "This.GameDayTime" if obj.name == "GAME" => Some(ctx.game_exports.map_or(ctx.day_time, |e| e.0)),
        "GAME.CurrentDayTime" => Some(ctx.day_time),
        "GAME.DayTimeFactor" => Some(ctx.day_factor.unwrap_or(ctx.day_time / DAY_LENGTH)),
        "GAME.ThickCloudsIntensity" => Some(ctx.cloud_intensity),
        "GAME.OffsetFromHQ_X" => Some(ctx.hq_offset[0]),
        "GAME.OffsetFromHQ_Z" => Some(ctx.hq_offset[1]),
        "GAME.GameDeltaTime" => Some(ctx.delta_time),
        "GAME.HighAltitudeWindX" => Some(ctx.wind[0]),
        "GAME.HighAltitudeWindZ" => Some(ctx.wind[1]),
        "GAME.CurrentNightIntensity" => Some(ctx.night),
        "e_Yes" | "e_TRUE" => Some(1.0),
        "e_No" | "e_FALSE" => Some(0.0),
        _ if name.starts_with("GAME.GameWaveCurve") => name["GAME.GameWaveCurve".len()..].parse::<usize>().ok().and_then(|k| ctx.waves.get(k).copied()).or(Some(0.0)),
        _ if name.starts_with("GAME.") => Some(0.0),
        _ if name.ends_with(".Counter") => {
            let rate = if let Some(field) = name.strip_prefix("This.") {
                counter(ctx, &obj.name, field)
            } else {
                ctx.counters.get(name).copied().unwrap_or(0.0)
            };
            Some(rate * ctx.delta_time)
        }
        _ if name.contains('[') => {
            let (base, index) = name.split_once('[')?;
            let index: f32 = index.strip_suffix(']')?.parse().ok()?;
            let (owner, field) = field_owner(objects, obj, base)?;
            let values = super::eval_list(owner.field(field)?);
            (!values.is_empty()).then(|| super::sample(&values, index))
        }
        _ => {
            let (owner, field) = field_owner(objects, obj, name)?;
            if owner.field(field).is_some_and(|e| e.contains(name)) {
                return Some(counter(ctx, &owner.name, field) * ctx.delta_time);
            }
            spend()?;
            (depth < MAX_NEST).then(|| eval_field_in(objects, owner, ctx, owner.field(field)?, depth + 1)).flatten()
        }
    }
}

fn counter(ctx: &Ctx, object: &str, field: &str) -> f32 {
    // ponytail: scan the small cached counter map; nested maps if many live counters need fast borrowed lookups.
    ctx.counters.iter().find_map(|(key, &rate)| {
        (key.strip_prefix(object)?.strip_prefix('.')? == field).then_some(rate)
    }).unwrap_or(0.0)
}

fn field_owner<'a>(objects: &'a [Obj], obj: &'a Obj, name: &'a str) -> Option<(&'a Obj, &'a str)> {
    let (owner, field) = name.split_once('.')?;
    Some((if owner == "This" { obj } else { objects.iter().find(|o| o.name == owner)? }, field))
}

/// Objects-aware scalar lookup for cross-object colour tracks and authored field chains.
pub fn eval_field_in(objects: &[Obj], obj: &Obj, ctx: &Ctx, expr: &str, depth: u32) -> Option<f32> {
    if depth == 0 { reset_budget(); }
    eval(expr, &|name| variable(objects, obj, ctx, name, depth))
}

/// Packed ARGB expressions retain every unsigned bit instead of round-tripping through f32.
pub fn unsigned(objects: &[Obj], obj: &Obj, ctx: &Ctx, expr: &str, depth: u32) -> Option<u32> {
    if depth == 0 { reset_budget(); }
    spend()?;
    if depth >= MAX_NEST { return None; }
    let expr = expr.trim();
    if expr.contains('|') {
        return expr.split('|').try_fold(0, |acc, term| Some(acc | unsigned(objects, obj, ctx, term, depth + 1)?));
    }
    if let Some(hex) = expr.strip_prefix("0x") {
        if let Ok(value) = u32::from_str_radix(hex.trim_end_matches('u'), 16) { return Some(value); }
    }
    if let Some((owner, field)) = field_owner(objects, obj, expr) {
        if let Some(value) = owner.field(field) { return unsigned(objects, owner, ctx, value, depth + 1); }
    }
    let bytes = expr.as_bytes();
    let mut position = 0;
    let variable = |name: &str| variable(objects, obj, ctx, name, depth + 1);
    let mut value = primary(bytes, &mut position, &variable, 0)? as u32;
    loop {
        skip_ws(bytes, &mut position);
        let Some(&op) = bytes.get(position) else { return Some(value) };
        if !b"+-*/%".contains(&op) { return None; }
        position += 1;
        let rhs = primary(bytes, &mut position, &variable, 0)?;
        let result = match op {
            b'+' => value as f32 + rhs,
            b'-' => value as f32 - rhs,
            b'*' => value as f32 * rhs,
            b'/' => value as f32 / rhs,
            _ => value as f32 % rhs,
        };
        // FXS10005600 unsigned/float arithmetic and1000a9b7 final conversion use this biased FISTP.
        value = (result - 0.49999).round_ties_even() as i64 as u32;
    }
}

pub fn eval_field(obj: &Obj, ctx: &Ctx, expr: &str, depth: u32) -> Option<f32> {
    eval_field_in(&[], obj, ctx, expr, depth)
}

/// `Float` field value (e.g. `Scale`), `default` when absent or not computable.
pub fn float(obj: &Obj, ctx: &Ctx, name: &str, default: f32) -> f32 {
    obj.field(name).and_then(|e| eval_field(obj, ctx, e, 0)).unwrap_or(default)
}

/// `[x, y, z]` of a `v( .. )` literal.
pub fn vector(obj: &Obj, ctx: &Ctx, expr: &str, depth: u32) -> Option<[f32; 3]> {
    let inner = expr.trim().strip_prefix("v(")?.split(')').next()?;
    components(obj, ctx, inner, depth)
}

fn components<const N: usize>(obj: &Obj, ctx: &Ctx, inner: &str, depth: u32) -> Option<[f32; N]> {
    let mut expressions = inner.split(',');
    let mut values = [0.0; N];
    for value in &mut values { *value = eval_field(obj, ctx, expressions.next()?, depth)?; }
    expressions.next().is_none().then_some(values)
}

/// Rotation expression → quaternion; `None` for anything the sky does not use.
pub fn rotation(obj: &Obj, ctx: &Ctx, expr: &str, depth: u32) -> Option<Quat> {
    if depth > 4 {
        return None;
    }
    if depth == 0 {
        reset_budget();
    }
    // FXS10008a11 appends operands in source order;10008914 traverses forward.
    // Evaluator10005600 calls10004f91 with accumulator=this,RHS=param, computing
    // RHS*accumulator: `a [ROT] b` applies a first, then b.
    expr.split("[ROT]").try_fold(Quat::IDENTITY, |total, term| {
        spend()?;
        Some(total.then(term_rotation(obj, ctx, term.trim(), depth)?))
    })
}

fn term_rotation(obj: &Obj, ctx: &Ctx, term: &str, depth: u32) -> Option<Quat> {
    if obj.name == "GAME" {
        match term {
            "This.GameSun1Rotation" => return Some(ctx.game_exports.map_or(ctx.sun1, |e| e.1[0])),
            "This.GameSun2Rotation" => return Some(ctx.game_exports.map_or(ctx.sun2, |e| e.1[1])),
            _ => {}
        }
    }
    if let Some(f) = term.strip_prefix("This.") {
        return rotation(obj, ctx, obj.field(f)?, depth + 1);
    }
    match term {
        "GAME.Sun1Rotation" => return Some(ctx.sun1),
        "GAME.Sun2Rotation" => return Some(ctx.sun2),
        "GAME.GameSun1Rotation" => return Some(ctx.game_exports.map_or(ctx.sun1, |e| e.1[0])),
        "GAME.GameSun2Rotation" => return Some(ctx.game_exports.map_or(ctx.sun2, |e| e.1[1])),
        _ => {}
    }
    if let Some(rest) = term.strip_prefix("v(") {
        let (axis, angle) = rest.split_once(')')?;
        let axis = components(obj, ctx, axis, depth)?;
        return Some(Quat::axis_angle(axis, eval_field(obj, ctx, angle.trim().trim_start_matches(',').trim(), depth)?));
    }
    let rest = term.strip_prefix("q(")?;
    // FXS stores the four values in written order = (x, y, z, w).
    let [x, y, z, w] = components(obj, ctx, rest.split(')').next()?, depth)?;
    Some(Quat { w, x, y, z })
}

/// Metadata-only scalar lookup: do not alter the existing sky evaluator's interfaces.
fn eval_expr_field(objects: &[Obj], obj: &Obj, ctx: &Ctx, expr: &str, depth: u32) -> Option<f32> {
    eval(expr, &|name| if name == "GAME.CurrentDayIntensity" {
        let game = objects.iter().find(|o| o.name == "GAME")?;
        eval_field(game, ctx, game.field("CurrentDayIntensity")?, depth + 1)
    } else if name.starts_with("GAME.") && !matches!(name,
        "GAME.GameDayTime" | "GAME.CurrentDayTime" | "GAME.DayTimeFactor" |
        "GAME.ThickCloudsIntensity" | "GAME.OffsetFromHQ_X" | "GAME.OffsetFromHQ_Z" |
        "GAME.GameDeltaTime" | "GAME.HighAltitudeWindX" | "GAME.HighAltitudeWindZ" |
        "GAME.CurrentNightIntensity") && !name.starts_with("GAME.GameWaveCurve") {
        None
    } else {
        variable(objects, obj, ctx, name, depth)
    })
}

fn components_expr<const N: usize>(objects: &[Obj], obj: &Obj, ctx: &Ctx, inner: &str, depth: u32) -> Option<[f32; N]> {
    let mut values = [0.0; N];
    if !inner.contains(',') {
        let mut expressions = inner.split_whitespace();
        for value in &mut values { *value = eval_expr_field(objects, obj, ctx, expressions.next()?, depth)?; }
        return expressions.next().is_none().then_some(values);
    }
    let mut expressions = inner.split(',');
    for value in &mut values { *value = eval_expr_field(objects, obj, ctx, expressions.next()?, depth)?; }
    expressions.next().is_none().then_some(values)
}

/// Native left-to-right vector expressions (FXS10005600), isolated to celestial metadata.
/// Unsupported typed operators fail closed rather than truncating to the first literal.
pub fn vector_expr(objects: &[Obj], obj: &Obj, ctx: &Ctx, expr: &str, depth: u32) -> Option<[f32; 3]> {
    if depth > MAX_NEST { return None; }
    if depth == 0 { reset_budget(); }
    spend()?;
    let b = expr.as_bytes();
    let mut pos = 0;
    let mut value = vector_operand(objects, obj, ctx, b, &mut pos, depth)?;
    loop {
        skip_ws(b, &mut pos);
        if pos == b.len() { return Some(value); }
        let rest = std::str::from_utf8(&b[pos..]).ok()?;
        if let Some(rhs) = rest.strip_prefix("[ROT]") {
            let mut end = 0;
            typed_atom(rhs.as_bytes(), &mut end)?;
            let q = rotation_expr(objects, obj, ctx, rhs[..end].trim(), depth + 1)?;
            // FXS10005600 vector/RHS quaternion/op12 ->100054af.
            value = q.rotate(value);
            pos += 5 + end;
            continue;
        }
        let normalize = rest.starts_with("[NORMALIZE]");
        let op = *b.get(pos)?;
        if normalize { pos += 11; } else if b"+-*/".contains(&op) { pos += 1; } else { return None; }
        let mut end = pos;
        let token = typed_atom(b, &mut end)?;
        if normalize {
            let scale = eval_expr_field(objects, obj, ctx, token, depth + 1)?;
            let length = value.iter().map(|v| v * v).sum::<f32>().sqrt();
            if length > 0.0 { value = value.map(|v| v / length * scale); }
        } else if let Some(rhs) = vector_expr(objects, obj, ctx, token, depth + 1) {
            value = std::array::from_fn(|i| match op { b'+' => value[i] + rhs[i], b'-' => value[i] - rhs[i],
                b'*' => value[i] * rhs[i], _ => f32::NAN });
            if op == b'/' { return None; }
        } else {
            let rhs = eval_expr_field(objects, obj, ctx, token, depth + 1)?;
            value = match op { b'*' => value.map(|v| v * rhs),
                b'/' if rhs != 0.0 => value.map(|v| v / rhs), b'/' => [0.0; 3], _ => return None };
        }
        pos = end;
    }
}

/// One typed operand; scalar formulas inside vector components retain scalar evaluation.
fn typed_atom<'a>(b: &'a [u8], pos: &mut usize) -> Option<&'a str> {
    skip_ws(b, pos);
    let start = *pos;
    if b.get(*pos..*pos + 2).is_some_and(|s| s == b"v(" || s == b"q(") {
        *pos += 2;
        let mut nest = 1;
        while let Some(&c) = b.get(*pos) {
            *pos += 1;
            if c == b'(' { nest += 1; }
            if c == b')' { nest -= 1; if nest == 0 { break; } }
        }
        if nest != 0 { return None; }
    } else {
        if b.get(*pos) == Some(&b'-') { *pos += 1; }
        while b.get(*pos).is_some_and(|c| c.is_ascii_alphanumeric() || b"._".contains(c)) { *pos += 1; }
    }
    (*pos > start).then(|| std::str::from_utf8(&b[start..*pos]).ok()).flatten()
}

fn vector_operand(objects: &[Obj], obj: &Obj, ctx: &Ctx, b: &[u8], pos: &mut usize, depth: u32) -> Option<[f32; 3]> {
    let token = typed_atom(b, pos)?;
    if let Some(inner) = token.strip_prefix("v(").and_then(|s| s.strip_suffix(')')) {
        return components_expr(objects, obj, ctx, inner, depth + 1);
    }
    match token {
        "GAME.CutY" => Some([1.0, 0.0, 1.0]),
        "GAME.UpDirection" => Some([0.0, 1.0, 0.0]),
        _ => vector_expr(objects, obj, ctx, obj.field(token.strip_prefix("This.")?)?, depth + 1),
    }
}

/// Metadata-only quaternion evaluation, including vector-to-vector [ROT].
pub fn rotation_expr(objects: &[Obj], obj: &Obj, ctx: &Ctx, expr: &str, depth: u32) -> Option<Quat> {
    if depth > MAX_NEST { return None; }
    if depth == 0 { reset_budget(); }
    spend()?;
    if let Some((left, right)) = expr.split_once("[ROT]") {
        if let (Some(a), Some(b)) = (vector_expr(objects, obj, ctx, left.trim(), depth + 1), vector_expr(objects, obj, ctx, right.trim(), depth + 1)) {
            let [x, y, z, w] = ao_scene::mover::rot_between(a, b);
            return Some(Quat { x, y, z, w });
        }
        return Some(rotation_expr(objects, obj, ctx, left.trim(), depth + 1)?.then(rotation_expr(objects, obj, ctx, right.trim(), depth + 1)?));
    }
    let expr = expr.trim();
    if let Some(field) = expr.strip_prefix("This.") {
        if obj.name != "GAME" || !matches!(field, "GameSun1Rotation" | "GameSun2Rotation") {
            return rotation_expr(objects, obj, ctx, obj.field(field)?, depth + 1);
        }
    }
    if let Some(rest) = expr.strip_prefix("v(") {
        let (axis, angle) = rest.split_once(')')?;
        return Some(Quat::axis_angle(components_expr(objects, obj, ctx, axis, depth + 1)?,
            eval_expr_field(objects, obj, ctx, angle.trim().trim_start_matches(',').trim(), depth + 1)?));
    }
    if let Some(rest) = expr.strip_prefix("q(").and_then(|s| s.strip_suffix(')')) {
        let [x, y, z, w] = components_expr(objects, obj, ctx, rest, depth + 1)?;
        return Some(Quat { x, y, z, w });
    }
    match expr {
        "GAME.Sun1Rotation" | "GAME.Sun2Rotation" | "GAME.GameSun1Rotation" | "GAME.GameSun2Rotation" => term_rotation(obj, ctx, expr, depth),
        "This.GameSun1Rotation" | "This.GameSun2Rotation" if obj.name == "GAME" => term_rotation(obj, ctx, expr, depth),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn celestial_expressions_are_strict_without_changing_legacy_vectors() {
        let objects = parse_objects(concat!(
            "Object Source\n{\n Vector Loop: This.Loop\n Quaternion LoopRotation: This.LoopRotation\n",
            " Quaternion Unit1Rotation: v(0,0,1), 90\n Quaternion Unit2Rotation: v(0,1,0), 30\n}\n"
        ));
        let obj = &objects[0];
        let ctx = Ctx::at(0.0);
        assert_eq!(vector(obj, &ctx, "v(1,2,3", 0), Some([1.0, 2.0, 3.0]), "DotStars legacy caller omits closing paren");
        assert_eq!(vector_expr(&objects, obj, &ctx, "v(1,2,3", 0), None);
        assert_eq!(vector_expr(&objects, obj, &ctx, "This.Unit1Rotation", 0), None, "axis-angle is not a vector");
        assert_eq!(vector_expr(&objects, obj, &ctx, "This.Loop", 0), None);
        assert!(rotation_expr(&objects, obj, &ctx, "This.LoopRotation", 0).is_none());
        assert_eq!(vector_expr(&objects, obj, &ctx, "v( 0 1 0 )", 0), Some([0.0, 1.0, 0.0]));
        let expression = "This.Unit1Rotation [ROT] This.Unit2Rotation";
        let expected = rotation(obj, &ctx, expression, 0).unwrap().rotate([0.0, 0.0, 1.0]);
        let actual = rotation_expr(&objects, obj, &ctx, expression, 0).unwrap().rotate([0.0, 0.0, 1.0]);
        assert!((0..3).all(|k| (actual[k] - expected[k]).abs() < 1e-6));
        assert!(rotation_expr(&objects, obj, &ctx, "q(0,0,0,1) junk", 0).is_none());
    }

    const SCRIPT: &str = "\
Object Moon2
{
  Unsigned    FXID:               e_GenericMeshObject
  Unsigned    Sorting:            BackgroundSort.Moon2
  Quaternion  Rotation:           v( 0,1,1 ), GAME.CurrentDayTime / 3240 * 360 + 90
  Float       Scale:              11
  String      Mesh:               \"moonmesh_small.abiff\"
  Expansion EP01:
  {
    String      Mesh:             \"other.abiff\"
    StateBlob 0u:
    {
      Criteria:	Game.RenderEngine == e_RenderEngine_DX7
      Texture:        Unsigned 0u, Unsigned 0u, String \"moon.png\"
      RenderState:      Unsigned 0u, Unsigned e_D3DRENDERSTATE_ALPHABLENDENABLE  , Unsigned e_TRUE
      RenderState:      Unsigned 0u, Unsigned e_D3DRENDERSTATE_DESTBLEND         , Unsigned e_D3DBLEND_ONE
    }
  }
}
Object Next
{
  Quaternion  Unit1Rotation:      v( 0,0,1 ), 90
  Quaternion  Unit2Rotation:      v( 0,1,0 ), This.Counter * -1.0
  Float       Counter:            GAME.GameDeltaTime * 3 + This.Counter % 360
  Quaternion  Rotation:           This.Unit1Rotation [ROT] This.Unit2Rotation
  Matrix      ScrollMatrix:       m( 1, 0, 0, 0
                                     0, 1, 0, 0 )
  Float       After:              7
}
";

    fn ctx(t: f32) -> Ctx {
        Ctx::at(t)
    }

    #[test]
    fn parses_objects_fields_expansions_and_states() {
        let o = parse_objects(SCRIPT);
        assert_eq!(o.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["Moon2", "Next"]);
        let m = &o[0];
        assert_eq!((m.fxid(), m.string("Mesh"), m.texture.as_deref()), ("GenericMeshObject", Some("other.abiff"), Some("moon.png")));
        assert_eq!((m.flag("ALPHABLENDENABLE"), m.states["DESTBLEND"].as_str()), (Some(true), "e_D3DBLEND_ONE"));
        assert_eq!(o[1].field("After"), Some("7"));
        assert!(o[1].field("ScrollMatrix").unwrap().ends_with("1, 0, 0 )"));
        let m = eval_matrix(&o[1], &ctx(0.0), o[1].field("ScrollMatrix").unwrap().trim_start_matches("m(").trim_end_matches(')')).unwrap();
        assert_eq!(m, [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
    }

    #[test]
    fn texture_stages_keep_independent_state_and_texture_factor_binding() {
        let objects = parse_objects(concat!(
            "Object Station\n{\n Unsigned TFACTOR: 0x80402010\n StateBlob 0u:\n{\n",
            " RenderState: Unsigned 0u, Unsigned e_D3DRENDERSTATE_TEXTUREFACTOR, This.TFACTOR\n",
            " TextureStage: Unsigned 0u, Unsigned 0u, Unsigned e_D3DTSS_COLORARG2, Unsigned e_D3DTA_DIFFUSE\n",
            " TextureStage: Unsigned 0u, Unsigned 1u, Unsigned e_D3DTSS_COLORARG2, Unsigned e_D3DTA_TFACTOR\n",
            " TextureStage: Unsigned 0u, Unsigned 0u, Unsigned e_D3DTSS_ALPHAARG2, Unsigned e_D3DTA_TEXTURE\n",
            " TextureStage: Unsigned 0u, Unsigned 1u, Unsigned e_D3DTSS_ALPHAARG2, Unsigned e_D3DTA_TFACTOR\n}\n}\n"
        ));
        let o = &objects[0];
        assert_eq!(o.states["TSS_COLORARG2"], "e_D3DTA_DIFFUSE");
        assert_eq!(o.states["TSS1_COLORARG2"], "e_D3DTA_TFACTOR");
        assert_eq!(o.states["TSS_ALPHAARG2"], "e_D3DTA_TEXTURE");
        assert_eq!(o.states["TSS1_ALPHAARG2"], "e_D3DTA_TFACTOR");
        assert_eq!(o.states["TEXTUREFACTOR"], "This.TFACTOR");
        assert_eq!(o.field("TFACTOR"), Some("0x80402010"));
    }

    #[test]
    fn expressions_run_left_to_right() {
        let none = |_: &str| None;
        assert_eq!(eval("2 + 3 * 4", &none), Some(20.0));
        assert_eq!(eval("1.0f - 0.5 * -4", &none), Some(-2.0));
        assert_eq!(eval("255u * 0.5 * 0x01000000", &none), Some(127.5 * 16777216.0));
        assert_eq!(eval("(1 + 2) * 3 % 4", &none), Some(1.0));
        assert!(eval("1 +", &none).is_none());
    }

    #[test]
    fn packed_atmosphere_tint_preserves_unsigned_bits_and_cross_object_array_samples() {
        let objects = parse_objects(concat!(
            "Object Atmosphere\n{\n Float ColorTopR [4]: 0, 0.2,\n 0.4, 0.8\n}\n",
            "Object Station\n{\n Float ColorR: Atmosphere.ColorTopR[GAME.DayTimeFactor] + 0.3 <| 1\n",
            " Unsigned AtmR: 0xFF * This.ColorR * 0x00010000\n Unsigned AtmI: 0x80000000\n",
            " Unsigned AtmColor: This.AtmI | This.AtmR | 0x00001234\n Unsigned TFACTOR: This.AtmColor\n}\n"
        ));
        let o = &objects[1];
        let ctx = Ctx::at(0.4 * DAY_LENGTH);
        assert!((eval_field_in(&objects, o, &ctx, "This.ColorR", 0).unwrap() - 0.62).abs() < 1e-6);
        let red = 158u32 << 16; // 255*.62 = 158.1 -> unsigned158 before shifting.
        assert_eq!(unsigned(&objects, o, &ctx, "This.TFACTOR", 0), Some(0x80000000 | red | 0x1234));
        assert_eq!(eval("2 + 3 <| 4 * 2", &|_| None), Some(8.0));
        assert_eq!(eval("-1 |> 0", &|_| None), Some(0.0));
    }

    #[test]
    fn moon_rotation_follows_the_day_time() {
        let o = parse_objects(SCRIPT);
        let moon = &o[0];
        // half a moon period: 3240 * 360 / 360 -> 360 degrees + 90, a quarter turn about the tilted axis
        let q = rotation(moon, &ctx(0.0), moon.field("Rotation").unwrap(), 0).unwrap();
        let q2 = Quat::axis_angle([0.0, 1.0, 1.0], 90.0);
        assert!((q.w - q2.w).abs() < 1e-5 && (q.y - q2.y).abs() < 1e-5);
        // the moon's tilted orbit really leaves the horizon: +Z reaches a positive height at some time
        let up = (0..48).map(|k| rotation(moon, &ctx(k as f32 * 135.0), moon.field("Rotation").unwrap(), 0).unwrap().rotate([0.0, 0.0, 1.0])[1]).fold(f32::MIN, f32::max);
        assert!(up > 0.5, "{up}");
    }

    #[test]
    fn counter_lookups_borrow_qualified_and_self_names() {
        let objects = parse_objects("Object A\n{\n Float Counter: This.Counter + 1\n Float ScrollU: This.ScrollU + 1\n}\n");
        let ctx = Ctx {
            delta_time: 2.0,
            counters: [("A.Counter".into(), 3.0), ("A.ScrollU".into(), 5.0), ("AB.Counter".into(), 99.0)].into(),
            ..Ctx::at(0.0)
        };
        for (expression, expected) in [("This.Counter", 6.0), ("A.Counter", 6.0), ("This.ScrollU", 10.0), ("Missing.Counter", 0.0)] {
            assert_eq!(eval_field(&objects[0], &ctx, expression, 0), Some(expected));
        }
    }

    #[test]
    fn composed_rotation_and_counters_are_zero() {
        let o = parse_objects(SCRIPT);
        let n = &o[1];
        let q = rotation(n, &ctx(0.0), n.field("Rotation").unwrap(), 0).unwrap();
        // 90 degrees about z takes +x to +y; the zero-angle second rotation leaves it
        let v = q.rotate([1.0, 0.0, 0.0]);
        assert!((v[1] - 1.0).abs() < 1e-5 && v[0].abs() < 1e-5, "{v:?}");
        // `Quat::then` = apply self first
        let both = Quat::axis_angle([0.0, 0.0, 1.0], 90.0).then(Quat::axis_angle([0.0, 1.0, 0.0], 90.0));
        let w = both.rotate([1.0, 0.0, 0.0]);
        assert!((w[1] - 1.0).abs() < 1e-5, "{w:?}");
    }

    #[test]
    fn rot_operator_applies_left_operand_first() {
        // Source-ordered FXS evaluation computes b*a, not the old inferred a*b.
        let text = "Object H\n{\n  Quaternion U1: v( 0,0,1 ), 90\n  Quaternion U2: v( 0,1,0 ), 90\n  Quaternion Rotation: This.U1 [ROT] This.U2\n}\n";
        let o = parse_objects(text);
        let q = rotation(&o[0], &ctx(0.0), o[0].field("Rotation").unwrap(), 0).unwrap();
        let up = q.rotate([1.0, 0.0, 0.0]);
        assert!((up[1] - 1.0).abs() < 1e-5 && up[0].abs() < 1e-5 && up[2].abs() < 1e-5, "{up:?}");
        let right = q.rotate([0.0, 0.0, 1.0]);
        assert!((right[0] - 1.0).abs() < 1e-5 && right[1].abs() < 1e-5 && right[2].abs() < 1e-5, "{right:?}");
    }

    #[test]
    fn scroll_integrators_give_per_second_rates_and_q_is_xyzw() {
        let text = "Object C\n{\n  Float ScrollU: This.ScrollU + GAME.HighAltitudeWindX % 1 * 10\n  Float ScrollV: GAME.GameDeltaTime * -0.004 + This.ScrollV % 1\n  Quaternion Q: q( 0, 0, 0.70710678, 0.70710678 )\n}\n";
        let o = parse_objects(text);
        let ctx = Ctx { delta_time: 1.0, wind: [0.0004, 0.0], ..Ctx::at(0.0) };
        let rate = |f: &str| eval_field(&o[0], &ctx, o[0].field(f).unwrap(), 0).unwrap();
        assert!((rate("ScrollU") - 0.004).abs() < 1e-6 && (rate("ScrollV") + 0.004).abs() < 1e-6);
        // a still frame (delta 0, no wind) does not move
        let still = eval_field(&o[0], &Ctx::at(0.0), o[0].field("ScrollV").unwrap(), 0).unwrap();
        assert_eq!(still, 0.0);
        // q(a, b, c, d) = (x, y, z, w): 90 degrees about z takes +x to +y
        let v = rotation(&o[0], &ctx, o[0].field("Q").unwrap(), 0).unwrap().rotate([1.0, 0.0, 0.0]);
        assert!((v[1] - 1.0).abs() < 1e-5 && v[0].abs() < 1e-5, "{v:?}");
    }

    #[test]
    fn hostile_nesting_and_fan_out_terminate() {
        let none = |_: &str| None;
        let deep = format!("{}1{}", "(".repeat(100_000), ")".repeat(100_000));
        assert!(eval(&deep, &none).is_none());
        assert!(eval(&"-".repeat(100_000), &none).is_none());
        assert_eq!(eval(&format!("{}1{}", "(".repeat(30), ")".repeat(30)), &none), Some(1.0));
        // 80 references per level, 4 levels: 80^4 evaluations without a budget
        let refs = (0..80).map(|i| format!("This.F{i}")).collect::<Vec<_>>().join(" + ");
        let mut text = String::from("Object Bomb\n{\n");
        for i in 0..80 {
            text += &format!("  Float F{i}: {refs}\n");
        }
        text += "  Quaternion R: ";
        text += &(0..80).map(|i| format!("This.Q{i}")).collect::<Vec<_>>().join(" [ROT] ");
        text += "\n";
        for i in 0..80 {
            text += &format!("  Quaternion Q{i}: {}\n", (0..80).map(|j| format!("This.Q{j}")).collect::<Vec<_>>().join(" [ROT] "));
        }
        text += "  Float Top: 1\n}\n";
        let o = &parse_objects(&text)[0];
        let t = std::time::Instant::now();
        let _ = eval_field(o, &ctx(0.0), o.field("F0").unwrap(), 0);
        let _ = rotation(o, &ctx(0.0), o.field("R").unwrap(), 0);
        assert!(t.elapsed().as_secs_f32() < 1.0, "{:?}", t.elapsed());
    }
}
