//! The GUI handlers of `/option` `/setoption` (GUI 0x100b5adc, one function: the first token decides whether a successful set
//! reports), `/dvalue` (0x100b7127), `/chardist` (0x100b62ed), `/viewdist` (0x100b63b8) and `/char&viewdist` (0x100b6465).
//! Output goes through `FUN_1009b37f(text, 0x51 error | 0x52 info)`; texts are the binary's HTML (`&lt;` `&gt;`), values are
//! `String::Escape(.., "<>&")`d.

use super::indep::Kind;
use super::{DValues, Variant};

/// One chat line of command feedback: `error` = colour 0x51 (`CCChatCmdFeedbackError`), else 0x52 (`CCChatCmdFeedbackInfo`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Out {
    pub error: bool,
    pub text: String,
}

fn err(text: String) -> Out {
    Out { error: true, text }
}

fn info(text: String) -> Out {
    Out { error: false, text }
}

/// `String::Escape(s, "<>&")`.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `_DAT_101bb7a0` (the double 0.01 as a float): `/viewdist` takes percent.
const PERCENT: f32 = 0.01;

impl DValues {
    /// Runs one of the six commands on its tokens (`cmd::tokenize` of the line, command word first); `None` for any other word. A
    /// changed prefs file is not written here: the caller saves ([`DValues::save_user`]).
    pub fn command(&mut self, t: &[String]) -> Option<Vec<Out>> {
        let word = t.first()?.to_ascii_lowercase();
        let n = t.len();
        let few = || vec![err("Error: To few arguments".into())];
        Some(match word.as_str() {
            "/option" | "/setoption" => self.option(t, word == "/option"),
            "/dvalue" => self.dvalue(t),
            "/chardist" if n < 2 => few(),
            "/chardist" => {
                self.set_char_dist(&t[1]);
                vec![]
            }
            "/viewdist" if n < 2 => few(),
            "/viewdist" => {
                self.set_view_dist(&t[1]);
                vec![]
            }
            "/char&viewdist" if n < 3 => few(),
            "/char&viewdist" => {
                self.set_char_dist(&t[1]);
                self.set_view_dist(&t[2]);
                vec![]
            }
            _ => return None,
        })
    }

    /// `DistributedValue_c::SetDValue("DisplayCharViewDistance", Variant(atol(tok)))` (clamped to 5..80 by the variable's min / max).
    fn set_char_dist(&mut self, tok: &str) {
        self.set("DisplayCharViewDistance", Variant::Int(i64::from(super::indep::atol(tok))));
    }

    /// `IndependentPrefs_t::SetPrefFloat("ViewDistance", atof(tok) * 0.01, 0, true)`.
    fn set_view_dist(&mut self, tok: &str) {
        self.prefs.set_float("ViewDistance", super::indep::atof(tok) * PERCENT, Kind::Login);
    }

    fn option(&mut self, t: &[String], silent: bool) -> Vec<Out> {
        if t.len() < 2 {
            return vec![err(format!("Invalid syntax: {} . Use /option &lt;OptionName&gt; [value]", t[0]))];
        }
        let name = &t[1];
        if t.len() == 2 {
            if let Some(m) = self.prefs.get_easy(name) {
                return vec![info(esc(&m))];
            }
            return vec![self.show(name)];
        }
        if let Some(m) = self.prefs.set_easy(name, &t[2]) {
            return if silent { vec![] } else { vec![info(esc(&m))] };
        }
        if !self.exists(name) {
            return vec![err(format!("Can't find option &lt;{}&gt;.", esc(name)))];
        }
        self.assign(name, &t[2], false, silent)
    }

    fn dvalue(&mut self, t: &[String]) -> Vec<Out> {
        if t.len() < 2 {
            return vec![err(format!("Invalid syntax: {} . Use /dvalue &lt;OptionName&gt; [value]", t[0]))];
        }
        if t.len() == 2 {
            return vec![self.show(&t[1])];
        }
        self.assign(&t[1], &t[2], true, false)
    }

    /// `Variable <name> is <value>` (colour 0x52) or the not-found error.
    fn show(&self, name: &str) -> Out {
        match self.get(name) {
            Some(v) => info(format!("Variable &lt;{}&gt; is &lt;{}&gt;", esc(name), esc(&v.as_string()))),
            None => err(format!("Can't find option &lt;{}&gt;.", esc(name))),
        }
    }

    /// The set branch: parse the expression, clamp it to the variable's min / max (`GetMinMaxValues`), `SetDValue` (`/dvalue` creates a
    /// missing, temporary variable with `AddVariable(name, v, false, false)`), then `Changed variable <n> from <old> to <new>`.
    fn assign(&mut self, name: &str, expr: &str, create: bool, silent: bool) -> Vec<Out> {
        let old = if self.exists(name) { esc(&self.get(name).map_or_else(String::new, Variant::as_string)) } else { "none".into() };
        let Some(v) = parse_expr(expr) else {
            return vec![err(format!("Failed to parse expression &lt;{}&gt;.", esc(expr)))];
        };
        let (min, max) = self.min_max(name);
        let v = if min != Variant::Void && v.order(&min) == Some(std::cmp::Ordering::Less) {
            min
        } else if max != Variant::Void && v.order(&max) == Some(std::cmp::Ordering::Greater) {
            max
        } else {
            v
        };
        if self.exists(name) {
            self.set(name, v.clone());
        } else if create {
            self.add(name, v.clone(), false, super::CAT_VARIABLES, None, None, false);
        }
        if silent {
            return vec![];
        }
        vec![info(format!("Changed variable &lt;{}&gt; from &lt;{old}&gt; to &lt;{}&gt;", esc(name), esc(&v.as_string())))]
    }
}

/// `ExpressionParser_c::ParseExpression(text, &out, 0, 0)` (Utils 0x10016f1d) with the operators the shipped GUI files use: literals
/// (decimal / `0x` integers, floats, `"strings"`, `true` / `false`), `( ) ! -`, `* / + - < > <= >= == != & | && ||`. The original is
/// a yacc table parser over `Variant` operators (tables at Utils 0x1001fa08..0x1001fc48) whose lexer also resolves constants and
/// `dvalue:` / `stat:` contexts; with no context given only the literals matter here. Ints stay ints, a float operand makes a float.
pub fn parse_expr(src: &str) -> Option<Variant> {
    let mut p = P { s: src.as_bytes(), i: 0 };
    let v = p.bin(0)?;
    p.ws();
    (p.i == p.s.len()).then_some(v)
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
}

/// Binary operator levels, loosest first (the order `ao_gui::expr` uses for the same GUI expressions).
const LEVELS: &[&[&str]] = &[&["||"], &["&&"], &["|"], &["&"], &["==", "!="], &["<=", ">=", "<", ">"], &["+", "-"], &["*", "/"]];

impl P<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(u8::is_ascii_whitespace) {
            self.i += 1;
        }
    }

    /// An operator token at the cursor that is not the start of a longer one (`&` vs `&&`, `<` vs `<=`, `|` vs `||`).
    fn op(&mut self, ops: &[&'static str]) -> Option<&'static str> {
        self.ws();
        let rest = &self.s[self.i..];
        let found = ops.iter().copied().find(|o| rest.starts_with(o.as_bytes()))?;
        if LEVELS.iter().flat_map(|l| l.iter()).any(|o| o.len() > found.len() && o.starts_with(found) && rest.starts_with(o.as_bytes())) {
            return None;
        }
        self.i += found.len();
        Some(found)
    }

    fn bin(&mut self, level: usize) -> Option<Variant> {
        let Some(ops) = LEVELS.get(level) else { return self.unary() };
        let mut l = self.bin(level + 1)?;
        while let Some(o) = self.op(ops) {
            let r = self.bin(level + 1)?;
            l = apply(o, &l, &r)?;
        }
        Some(l)
    }

    fn unary(&mut self) -> Option<Variant> {
        self.ws();
        match self.s.get(self.i)? {
            b'!' if self.s.get(self.i + 1) != Some(&b'=') => {
                self.i += 1;
                Some(Variant::Bool(!truthy(&self.unary()?)))
            }
            b'-' => {
                self.i += 1;
                match self.unary()? {
                    Variant::Int(v) => Some(Variant::Int(v.checked_neg()?)),
                    Variant::Float(f) => Some(Variant::Float(-f)),
                    _ => None,
                }
            }
            b'(' => {
                self.i += 1;
                let v = self.bin(0)?;
                self.ws();
                (self.s.get(self.i) == Some(&b')')).then(|| self.i += 1)?;
                Some(v)
            }
            b'"' => {
                let end = self.s[self.i + 1..].iter().position(|&c| c == b'"')? + self.i + 1;
                let v = String::from_utf8_lossy(&self.s[self.i + 1..end]).into_owned();
                self.i = end + 1;
                Some(Variant::Str(v))
            }
            c if c.is_ascii_digit() || *c == b'.' => {
                let n = self.s[self.i..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'.').count();
                let t = std::str::from_utf8(&self.s[self.i..self.i + n]).ok()?;
                self.i += n;
                if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                    return i64::from_str_radix(h, 16).ok().map(Variant::Int);
                }
                if t.contains('.') { t.parse().ok().map(Variant::Float) } else { t.parse().ok().map(Variant::Int) }
            }
            c if c.is_ascii_alphabetic() => {
                let n = self.s[self.i..].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_').count();
                let w = std::str::from_utf8(&self.s[self.i..self.i + n]).ok()?;
                self.i += n;
                match w.to_ascii_lowercase().as_str() {
                    "true" => Some(Variant::Bool(true)),
                    "false" => Some(Variant::Bool(false)),
                    _ => None,
                }
            }
            _ => None,
        }
    }
}

fn truthy(v: &Variant) -> bool {
    match v {
        Variant::Str(s) => !s.is_empty(),
        Variant::Float(f) => *f != 0.0,
        v => v.as_i64() != 0,
    }
}

/// `Variant::operator <op>`: ints stay ints (division truncates), any float operand gives a float, strings concatenate with `+`.
fn apply(op: &str, l: &Variant, r: &Variant) -> Option<Variant> {
    use std::cmp::Ordering::*;
    let ord = || l.order(r);
    Some(match op {
        "||" => Variant::Bool(truthy(l) || truthy(r)),
        "&&" => Variant::Bool(truthy(l) && truthy(r)),
        "|" => Variant::Int(l.as_i64() | r.as_i64()),
        "&" => Variant::Int(l.as_i64() & r.as_i64()),
        "==" => Variant::Bool(l == r || ord() == Some(Equal)),
        "!=" => Variant::Bool(!(l == r || ord() == Some(Equal))),
        "<" => Variant::Bool(ord()? == Less),
        ">" => Variant::Bool(ord()? == Greater),
        "<=" => Variant::Bool(ord()? != Greater),
        ">=" => Variant::Bool(ord()? != Less),
        _ => match (l, r) {
            (Variant::Str(a), Variant::Str(b)) if op == "+" => Variant::Str(format!("{a}{b}")),
            (Variant::Int(a), Variant::Int(b)) => Variant::Int(match op {
                "+" => a.checked_add(*b)?,
                "-" => a.checked_sub(*b)?,
                "*" => a.checked_mul(*b)?,
                _ => a.checked_div(*b)?,
            }),
            _ => {
                let (a, b) = (l.num()? as f32, r.num()? as f32);
                Variant::Float(match op {
                    "+" => a + b,
                    "-" => a - b,
                    "*" => a * b,
                    _ => a / b,
                })
            }
        },
    })
}
