//! Minimal TinyXML-compatible parser for the client's GUI XML.
//!
//! The client parses with TinyXML, which (unlike strict XML parsers) accepts `<` and raw newlines
//! inside quoted attribute values; `Views/Skills.xml` relies on that.  Only elements, attributes,
//! text-less content, comments and declarations are needed.

use anyhow::{bail, ensure, Result};

#[derive(Clone, Debug, Default)]
pub struct Element {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Element>,
}

impl Element {
    pub fn attr(&self, key: &str) -> Option<&str> {
        // TinyXML attribute lookup is case sensitive (`XMLObject_c::GetAttr*`).
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }
}

fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    // numeric character references (`&#8216;` in Skills.xml), then the five predefined entities (`&amp;` last)
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("&#") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 2..];
        let (digits, radix) = match tail.strip_prefix(['x', 'X']) {
            Some(h) => (h, 16),
            None => (tail, 10),
        };
        let n = digits.find(';').and_then(|e| u32::from_str_radix(&digits[..e], radix).ok().map(|v| (v, e)));
        match n.and_then(|(v, e)| char::from_u32(v).map(|c| (c, e))) {
            Some((c, e)) => {
                out.push(c);
                rest = &digits[e + 1..];
            }
            None => {
                out.push_str("&#");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out.replace("&quot;", "\"").replace("&apos;", "'").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

struct P<'a> {
    s: &'a [u8],
    i: usize,
    depth: u32,
}

const MAX_DEPTH: u32 = 64;

impl<'a> P<'a> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn starts(&self, t: &str) -> bool {
        self.s[self.i..].starts_with(t.as_bytes())
    }
    fn skip_until(&mut self, t: &str) -> Result<()> {
        while self.i < self.s.len() {
            if self.starts(t) {
                self.i += t.len();
                return Ok(());
            }
            self.i += 1;
        }
        bail!("xml: unterminated construct, expected {t}")
    }
    fn name(&mut self) -> String {
        let st = self.i;
        while self.i < self.s.len() {
            let c = self.s[self.i];
            if c.is_ascii_whitespace() || matches!(c, b'=' | b'>' | b'/' | b'<') {
                break;
            }
            self.i += 1;
        }
        String::from_utf8_lossy(&self.s[st..self.i]).into_owned()
    }
    fn element(&mut self) -> Result<Element> {
        ensure!(self.depth < MAX_DEPTH, "xml: nesting deeper than {MAX_DEPTH}");
        self.depth += 1;
        let r = self.element_inner();
        self.depth -= 1;
        r
    }
    fn element_inner(&mut self) -> Result<Element> {
        // at '<'
        self.i += 1;
        let mut e = Element { name: self.name(), ..Default::default() };
        loop {
            self.ws();
            if self.i >= self.s.len() {
                bail!("xml: eof in tag <{}>", e.name);
            }
            if self.starts("/>") {
                self.i += 2;
                return Ok(e);
            }
            if self.starts(">") {
                self.i += 1;
                break;
            }
            let k = self.name();
            if k.is_empty() {
                bail!("xml: bad attribute in <{}> at byte {}", e.name, self.i);
            }
            self.ws();
            if !self.starts("=") {
                bail!("xml: attribute {k} without value in <{}>", e.name);
            }
            self.i += 1;
            self.ws();
            let q = self.s.get(self.i).copied().unwrap_or(0);
            if q != b'"' && q != b'\'' {
                bail!("xml: unquoted attribute {k} in <{}>", e.name);
            }
            self.i += 1;
            let st = self.i;
            while self.i < self.s.len() && self.s[self.i] != q {
                self.i += 1;
            }
            let v = String::from_utf8_lossy(&self.s[st..self.i]).into_owned();
            self.i += 1;
            e.attrs.push((k, decode(&v)));
        }
        // children
        loop {
            // skip character data (not used by GUI XML)
            while self.i < self.s.len() && self.s[self.i] != b'<' {
                self.i += 1;
            }
            if self.i >= self.s.len() {
                bail!("xml: eof inside <{}>", e.name);
            }
            if self.starts("<!--") {
                self.skip_until("-->")?;
            } else if self.starts("<?") {
                self.skip_until("?>")?;
            } else if self.starts("</") {
                self.skip_until(">")?;
                return Ok(e);
            } else {
                e.children.push(self.element()?);
            }
        }
    }
}

/// Parses a document and returns its root element.
pub fn parse(src: &str) -> Result<Element> {
    let mut p = P { s: src.as_bytes(), i: 0, depth: 0 };
    loop {
        while p.i < p.s.len() && p.s[p.i] != b'<' {
            p.i += 1;
        }
        if p.i >= p.s.len() {
            bail!("xml: no root element");
        }
        if p.starts("<!--") {
            p.skip_until("-->")?;
        } else if p.starts("<?") || p.starts("<!") {
            p.skip_until(">")?;
        } else {
            return p.element();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tolerates_lt_and_newline_in_attr() {
        let e = parse("<?xml version=\"1.0\"?><root><A v=\"<b>x</b>\ny\" w='1'/><B/></root>").unwrap();
        assert_eq!(e.children.len(), 2);
        assert_eq!(e.children[0].attr("v"), Some("<b>x</b>\ny"));
        assert_eq!(e.children[0].attr("w"), Some("1"));
    }

    #[test]
    fn malformed_input_is_an_error_not_a_panic() {
        assert!(parse("<a b=").is_err());
        assert!(parse("<a b=   ").is_err());
        assert!(parse(&"<a>".repeat(200_000)).is_err());
        assert!(parse(&format!("{}{}", "<a>".repeat(60), "</a>".repeat(60))).is_ok());
    }
}
