//! The part of the FXS tweak language that the sky objects use: `Object` blocks, left-to-right expressions and
//! quaternions (`v(axis), degrees`, `q(w,x,y,z)`, `a [ROT] b`).
//!
//! Evaluation context (offline, no game running): `GAME.CurrentDayTime` / `DayTimeFactor` come from the requested time of
//! day; every other value the game writes every frame (`GameWaveCurve*`, `GameDeltaTime`, wind, the `Counter` fields that
//! integrate delta time) is 0, i.e. the sky is a still frame at that time.

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

const TYPES: [&str; 8] = ["Unsigned", "Float", "Vector", "Quaternion", "String", "Matrix", "Object", "Color"];

/// All objects of the flattened (comment free) script text, in file order.
pub fn parse_objects(text: &str) -> Vec<Obj> {
    let mut out = Vec::new();
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
                out.extend(cur.take());
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
            // TextureStage: stage, stage, e_D3DTSS_<NAME>, value (stage 0 only is used by the sky)
            let parts: Vec<&str> = rest.split(',').map(|p| p.trim().trim_start_matches("Unsigned").trim()).collect();
            if let [_, _, name, value] = parts[..] {
                obj.states.insert(name.replace("e_D3DTSS_", "TSS_"), value.to_string());
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
        if !b"+-*/%".contains(&op) {
            return Some(acc);
        }
        *p += 1;
        let rhs = primary(b, p, var, nest)?;
        acc = match op {
            b'+' => acc + rhs,
            b'-' => acc - rhs,
            b'*' => acc * rhs,
            b'/' => acc / rhs,
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
            var(std::str::from_utf8(&b[st..*p]).ok()?)
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

    /// Shortest rotation taking the unit vector `from` to the unit vector `to`.
    pub fn between(from: [f32; 3], to: [f32; 3]) -> Quat {
        let dot: f32 = (0..3).map(|k| from[k] * to[k]).sum();
        let cross = [from[1] * to[2] - from[2] * to[1], from[2] * to[0] - from[0] * to[2], from[0] * to[1] - from[1] * to[0]];
        if dot < -0.9999 {
            return Quat::axis_angle([0.0, 1.0, 0.0], 180.0);
        }
        let l = ((1.0 + dot) * 2.0).sqrt();
        Quat { w: l / 2.0, x: cross[0] / l, y: cross[1] / l, z: cross[2] / l }
    }

    /// Apply `self`, then `next` (`next [ROT] self` in script order).
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
    /// `GAME.ThickCloudsIntensity` (written by the game from the weather).
    pub cloud_intensity: f32,
    /// `GAME.OffsetFromHQ_X/Z`: the playfield's universe position relative to Omni-HQ.
    pub hq_offset: [f32; 2],
    /// `GAME.GameDeltaTime`: 0 for a still frame, 1 to read per-second rates out of the per-frame integrators.
    pub delta_time: f32,
    /// `GAME.HighAltitudeWindX/Z` (the game writes `speed * direction * dt * k` every frame, `FUN_100be767`).
    pub wind: [f32; 2],
    /// `Object.Counter` references of other objects: their growth per second (the counters integrate `GameDeltaTime`).
    pub counters: std::collections::HashMap<String, f32>,
}

impl Ctx {
    /// A context for expressions that do not depend on the game (offsets, counters, colours).
    pub fn at(day_time: f32) -> Ctx {
        Ctx { day_time, sun1: Quat::IDENTITY, sun2: Quat::IDENTITY, cloud_intensity: 0.0, hq_offset: [0.0; 2], delta_time: 0.0, wind: [0.0; 2], night: 0.0, counters: Default::default() }
    }
}

const DAY_LENGTH: f32 = 6480.0;

thread_local! {
    /// Remaining variable/rotation-term evaluations of the current top-level evaluation.
    static BUDGET: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

    /// `GAME.CurrentNightIntensity` = `NightIntensity[DayTimeFactor]` (`Tweak_GAME.txt`).
    pub night: f32,
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
fn variable(obj: &Obj, ctx: &Ctx, name: &str, depth: u32) -> Option<f32> {
    match name {
        "GAME.CurrentDayTime" | "GAME.GameDayTime" => Some(ctx.day_time),
        "GAME.DayTimeFactor" => Some(ctx.day_time / DAY_LENGTH),
        "GAME.ThickCloudsIntensity" => Some(ctx.cloud_intensity),
        "GAME.OffsetFromHQ_X" => Some(ctx.hq_offset[0]),
        "GAME.OffsetFromHQ_Z" => Some(ctx.hq_offset[1]),
        "GAME.GameDeltaTime" => Some(ctx.delta_time),
        "GAME.HighAltitudeWindX" => Some(ctx.wind[0]),
        "GAME.HighAltitudeWindZ" => Some(ctx.wind[1]),
        _ if name.starts_with("GAME.") => Some(0.0),
        _ if name.ends_with(".Counter") => {
            let key = name.strip_prefix("This.").map_or_else(|| name.to_string(), |f| format!("{}.{f}", obj.name));
            Some(ctx.counters.get(&key).map_or(0.0, |rate| rate * ctx.delta_time))
        }
        _ => {
            let field = name.strip_prefix("This.")?;
            // a self-integrating field (`ScrollU: This.ScrollU + ...`, counters) is 0 inside its own expression; elsewhere its
            // per-second growth (`ctx.counters`, filled by `layers`) times the elapsed time
            if obj.field(field).is_some_and(|e| e.contains(name)) {
                return Some(ctx.counters.get(&format!("{}.{field}", obj.name)).map_or(0.0, |rate| rate * ctx.delta_time));
            }
            spend()?;
            (depth < 4).then(|| eval_field(obj, ctx, obj.field(field)?, depth + 1)).flatten().or(Some(0.0))
        }
    }
}

        "GAME.CurrentNightIntensity" => Some(ctx.night),
        "e_Yes" | "e_TRUE" => Some(1.0),
        "e_No" | "e_FALSE" => Some(0.0),
pub fn eval_field(obj: &Obj, ctx: &Ctx, expr: &str, depth: u32) -> Option<f32> {
    if depth == 0 {
        reset_budget();
    }
    eval(expr, &|n| variable(obj, ctx, n, depth))
}

/// `Float` field value (e.g. `Scale`), `default` when absent or not computable.
pub fn float(obj: &Obj, ctx: &Ctx, name: &str, default: f32) -> f32 {
    obj.field(name).and_then(|e| eval_field(obj, ctx, e, 0)).unwrap_or(default)
}

/// `[x, y, z]` of a `v( .. )` literal.
pub fn vector(obj: &Obj, ctx: &Ctx, expr: &str, depth: u32) -> Option<[f32; 3]> {
    let inner = expr.trim().strip_prefix("v(")?.split(')').next()?;
    let c: Vec<f32> = inner.split(',').map(|e| eval_field(obj, ctx, e, depth)).collect::<Option<_>>()?;
    c.try_into().ok()
}

/// Rotation expression → quaternion; `None` for anything the sky does not use.
pub fn rotation(obj: &Obj, ctx: &Ctx, expr: &str, depth: u32) -> Option<Quat> {
    if depth > 4 {
        return None;
    }
    if depth == 0 {
        reset_budget();
    }
    // `a [ROT] b` is the Hamilton product `a * b`: `b` acts first (FXS `FUN_10004f91` computes `param * this`; the only
    // order that stands Old Athen's / the horizon map's Max Z-up meshes upright under `Rz(90) [ROT] Ry(90)`)
    expr.split("[ROT]").try_fold(Quat::IDENTITY, |total, term| {
        spend()?;
        Some(term_rotation(obj, ctx, term.trim(), depth)?.then(total))
    })
}

fn term_rotation(obj: &Obj, ctx: &Ctx, term: &str, depth: u32) -> Option<Quat> {
    if let Some(f) = term.strip_prefix("This.") {
        return rotation(obj, ctx, obj.field(f)?, depth + 1);
    }
    match term {
        "GAME.Sun1Rotation" => return Some(ctx.sun1),
        "GAME.Sun2Rotation" => return Some(ctx.sun2),
        _ => {}
    }
    if let Some(rest) = term.strip_prefix("v(") {
        let (axis, angle) = rest.split_once(')')?;
        let axis = vector(obj, ctx, &format!("v({axis})"), depth)?;
        return Some(Quat::axis_angle(axis, eval_field(obj, ctx, angle.trim().trim_start_matches(',').trim(), depth)?));
    }
    let rest = term.strip_prefix("q(")?;
    let c: Vec<f32> = rest.split(')').next()?.split(',').map(|e| eval_field(obj, ctx, e, depth)).collect::<Option<_>>()?;
    // FXS stores the four values in written order = (x, y, z, w)
    let [x, y, z, w]: [f32; 4] = c.try_into().ok()?;
    Some(Quat { w, x, y, z })
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn expressions_run_left_to_right() {
        let none = |_: &str| None;
        assert_eq!(eval("2 + 3 * 4", &none), Some(20.0));
        assert_eq!(eval("1.0f - 0.5 * -4", &none), Some(-2.0));
        assert_eq!(eval("255u * 0.5 * 0x01000000", &none), Some(127.5 * 16777216.0));
        assert_eq!(eval("(1 + 2) * 3 % 4", &none), Some(1.0));
        assert!(eval("1 +", &none).is_none());
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
    fn rot_operator_acts_right_operand_first_like_the_real_horizon_objects() {
        // Tweak_Rubi-Ka_Horizon: `Rz(90) [ROT] Ry(90)` must carry the Max Z-up mesh's up axis (+z) to world +y
        let text = "Object H\n{\n  Quaternion U1: v( 0,0,1 ), 90\n  Quaternion U2: v( 0,1,0 ), 90\n  Quaternion Rotation: This.U1 [ROT] This.U2\n}\n";
        let o = parse_objects(text);
        let q = rotation(&o[0], &ctx(0.0), o[0].field("Rotation").unwrap(), 0).unwrap();
        let up = q.rotate([0.0, 0.0, 1.0]);
        assert!((up[1].abs() - 1.0).abs() < 1e-5 && up[0].abs() < 1e-5 && up[2].abs() < 1e-5, "{up:?}");
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
