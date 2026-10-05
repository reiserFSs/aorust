//! Criteria expressions of the view XML (`activate_criteria`, `criteria`, `bgicon`): the subset of GUI.dll's
//! `ExpressionParser_c` (UTILS.DLL, `ParseExpression`) the shipped `Views/ControlCenter.xml` and
//! `ActionMenu/*.xml` use: `dvalue:NAME` (distributed value), `stat:NAME` (own character stat), `id:GFX_NAME`
//! (skin id), decimal / `0x` integers, `! && || == != < > <= >= & | + -` and parentheses. Truthiness = non-zero.
//! Unknown names evaluate to 0; a parse error evaluates to 0 (false), like a failed `ParseExpression`.

/// Resolves `kind:name` (`dvalue`, `stat`, `id`) to a value.
pub type Resolver<'a> = &'a dyn Fn(&str, &str) -> Option<i64>;

pub fn eval(src: &str, res: Resolver) -> i64 {
    let mut p = P { s: src.as_bytes(), i: 0, res };
    let v = p.or();
    p.ws();
    if p.i == p.s.len() {
        v
    } else {
        0
    }
}

pub fn truthy(src: &str, res: Resolver) -> bool {
    eval(src, res) != 0
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
    res: Resolver<'a>,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_whitespace()) {
            self.i += 1;
        }
    }
    fn eat(&mut self, t: &str) -> bool {
        self.ws();
        if self.s[self.i..].starts_with(t.as_bytes()) {
            self.i += t.len();
            true
        } else {
            false
        }
    }
    /// An operator that must not be the prefix of a longer one (`&` vs `&&`, `<` vs `<=`).
    fn op(&mut self, t: &str, longer: &[&str]) -> bool {
        self.ws();
        if longer.iter().any(|l| self.s[self.i..].starts_with(l.as_bytes())) {
            return false;
        }
        self.eat(t)
    }
    fn or(&mut self) -> i64 {
        let mut v = self.and();
        while self.eat("||") {
            let r = self.and();
            v = (v != 0 || r != 0) as i64;
        }
        v
    }
    fn and(&mut self) -> i64 {
        let mut v = self.bitor();
        while self.eat("&&") {
            let r = self.bitor();
            v = (v != 0 && r != 0) as i64;
        }
        v
    }
    fn bitor(&mut self) -> i64 {
        let mut v = self.bitand();
        while self.op("|", &["||"]) {
            v |= self.bitand();
        }
        v
    }
    fn bitand(&mut self) -> i64 {
        let mut v = self.equality();
        while self.op("&", &["&&"]) {
            v &= self.equality();
        }
        v
    }
    fn equality(&mut self) -> i64 {
        let mut v = self.relational();
        loop {
            if self.eat("==") {
                v = (v == self.relational()) as i64;
            } else if self.eat("!=") {
                v = (v != self.relational()) as i64;
            } else {
                return v;
            }
        }
    }
    fn relational(&mut self) -> i64 {
        let mut v = self.additive();
        loop {
            if self.eat("<=") {
                v = (v <= self.additive()) as i64;
            } else if self.eat(">=") {
                v = (v >= self.additive()) as i64;
            } else if self.op("<", &["<="]) {
                v = (v < self.additive()) as i64;
            } else if self.op(">", &[">="]) {
                v = (v > self.additive()) as i64;
            } else {
                return v;
            }
        }
    }
    fn additive(&mut self) -> i64 {
        let mut v = self.unary();
        loop {
            if self.eat("+") {
                v += self.unary();
            } else if self.eat("-") {
                v -= self.unary();
            } else {
                return v;
            }
        }
    }
    fn unary(&mut self) -> i64 {
        if self.op("!", &["!="]) {
            return (self.unary() == 0) as i64;
        }
        if self.eat("-") {
            return -self.unary();
        }
        self.primary()
    }
    fn primary(&mut self) -> i64 {
        self.ws();
        if self.eat("(") {
            let v = self.or();
            self.eat(")");
            return v;
        }
        let start = self.i;
        while self.s.get(self.i).is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'.')) {
            self.i += 1;
        }
        let tok = std::str::from_utf8(&self.s[start..self.i]).unwrap_or("");
        if let Some(h) = tok.strip_prefix("0x").or_else(|| tok.strip_prefix("0X")) {
            return i64::from_str_radix(h, 16).unwrap_or(0);
        }
        if let Ok(n) = tok.parse::<i64>() {
            return n;
        }
        match tok.split_once(':') {
            Some((k, n)) => (self.res)(k, n).unwrap_or(0),
            None => match tok {
                "true" => 1,
                _ => 0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(k: &str, n: &str) -> Option<i64> {
        match (k, n) {
            ("dvalue", "a") | ("dvalue", "b") => Some(1),
            ("dvalue", "off") => Some(0),
            ("stat", "side") => Some(3),
            ("stat", "level") => Some(20),
            ("stat", "expansion") => Some(0x22),
            ("id", "GFX_X") => Some(77),
            _ => None,
        }
    }

    #[test]
    fn control_center_and_menu_criteria() {
        assert!(truthy("dvalue:a && dvalue:b", &r));
        assert!(!truthy("dvalue:a && dvalue:off", &r));
        assert!(truthy("dvalue:off || dvalue:a", &r));
        assert!(!truthy("stat:side!=3", &r));
        assert!(truthy("stat:level>=15", &r));
        assert!(truthy("(stat:level>=10 || stat:alienlevel>=1) && ((stat:expansion & 0x2) || (stat:expansion & 0x8))", &r));
        assert!(truthy("stat:expansion & 32", &r));
        assert!(!truthy("stat:expansion & 0x4", &r));
        assert_eq!(eval("id:GFX_X", &r), 77);
        assert!(!truthy("dvalue:a &&", &r));
    }
}
