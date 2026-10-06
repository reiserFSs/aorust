//! `/filter` and the chat filter of `FUN_10084f9e` (GUI.dll). Evidence: docs/chat/dialogs.md §6 (`/filter`), docs/chat/gui.md §4.
//!
//! * `ChatFilterEnabled` (bool DValue) and `ChatFilterRules` (a `Message`: name -> string, names are the decimal rule numbers) are
//!   read by `FUN_10084f9e` on every message of the chat server (`HandleVicinityMessage` 0x10086728, `HandleGroupMessage` 0x100863ff,
//!   the tell windows `FUN_100a6210` / `FUN_100a592c`, `FUN_10083898`): for every name of the message the first string is compiled
//!   with `RegExp` (Utils.dll 0x1000c7ba) and `RegExp::Compare(text)` decides; one hit drops the message.
//! * `RegExp` is Henry Spencer's V8 `regexp(3)` (`regcomp` 0x1000c670: magic byte 0x9c at +0xac, `regexec` 0x1000c469 tries every start
//!   offset, the "must contain" pre-check at +0xa4): [`V8Regex`] below. A rule that does not compile makes `RegExp::Compare` run an
//!   `int 3` in the original (`FUN_1000c469` with a null program); the port treats such a rule as never matching.
//! * `/filter [list|del|add|enable|disable|clear]` is `FUN_100b8d4e` (GUI 0x100b8d4e).

use std::cell::Cell;
use std::path::PathBuf;

// ------------------------------------------------------------------------------------------------ V8 regexp

#[derive(Debug, Clone)]
enum Node {
    Char(u8),
    Any,
    Class(Box<[bool; 256]>),
    Bol,
    Eol,
    Group(Vec<Vec<Piece>>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Rep {
    One,
    Opt,
    Star,
    Plus,
}

#[derive(Debug, Clone)]
struct Piece {
    node: Node,
    rep: Rep,
}

/// A compiled V8 regular expression (`| ( ) * + ? . [] [^] ^ $ \x`; `{` is an ordinary character, `\` quotes anything).
#[derive(Debug, Clone)]
pub struct V8Regex(Vec<Vec<Piece>>);

struct Parser<'a> {
    p: &'a [u8],
    i: usize,
    groups: u32,
}

fn node_width(n: &Node) -> bool {
    match n {
        Node::Char(_) | Node::Any | Node::Class(_) => true,
        Node::Bol | Node::Eol => false,
        // `reg()`: HASWIDTH survives only when every branch has width
        Node::Group(alts) => alts.iter().all(|a| a.iter().any(piece_width)),
    }
}

fn piece_width(p: &Piece) -> bool {
    matches!(p.rep, Rep::One | Rep::Plus) && node_width(&p.node)
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.p.get(self.i).copied()
    }

    /// `reg()`: alternatives up to `)` (nested) or the end.
    fn alts(&mut self, nested: bool) -> Option<Vec<Vec<Piece>>> {
        let mut out = vec![];
        loop {
            let mut seq = vec![];
            while let Some(c) = self.peek() {
                if c == b'|' || c == b')' {
                    break;
                }
                seq.push(self.piece()?);
            }
            out.push(seq);
            match self.peek() {
                Some(b'|') => self.i += 1,
                Some(b')') if nested => return Some(out),
                Some(b')') => return None, // unmatched ()
                _ => return if nested { None } else { Some(out) },
            }
        }
    }

    fn piece(&mut self) -> Option<Piece> {
        let node = self.atom()?;
        let rep = match self.peek() {
            Some(b'*') => Rep::Star,
            Some(b'+') => Rep::Plus,
            Some(b'?') => Rep::Opt,
            _ => return Some(Piece { node, rep: Rep::One }),
        };
        self.i += 1;
        // "*+ operand could be empty" / "nested *?+"
        if (rep != Rep::Opt && !node_width(&node)) || matches!(self.peek(), Some(b'*' | b'+' | b'?')) {
            return None;
        }
        Some(Piece { node, rep })
    }

    fn atom(&mut self) -> Option<Node> {
        let c = self.peek()?;
        self.i += 1;
        Some(match c {
            b'^' => Node::Bol,
            b'$' => Node::Eol,
            b'.' => Node::Any,
            b'(' => {
                self.groups += 1;
                if self.groups > 9 {
                    return None; // NSUBEXP
                }
                let a = self.alts(true)?;
                self.i += 1; // ')'
                Node::Group(a)
            }
            b'*' | b'+' | b'?' => return None, // "?+* follows nothing"
            b'\\' => {
                let l = self.peek()?; // trailing \
                self.i += 1;
                Node::Char(l)
            }
            b'[' => self.class()?,
            c => Node::Char(c),
        })
    }

    fn class(&mut self) -> Option<Node> {
        let mut set = Box::new([false; 256]);
        let neg = self.peek() == Some(b'^');
        if neg {
            self.i += 1;
        }
        if matches!(self.peek(), Some(b']' | b'-')) {
            set[self.peek()? as usize] = true;
            self.i += 1;
        }
        loop {
            let c = self.peek()?; // unmatched []
            self.i += 1;
            if c == b']' {
                break;
            }
            if c == b'-' {
                match self.peek() {
                    None => return None,
                    Some(b']') => set[b'-' as usize] = true,
                    Some(hi) => {
                        let lo = self.p[self.i - 2];
                        if lo > hi {
                            return None; // invalid [] range
                        }
                        for b in lo..=hi {
                            set[b as usize] = true;
                        }
                        self.i += 1;
                    }
                }
            } else {
                set[c as usize] = true;
            }
        }
        if neg {
            for b in set.iter_mut() {
                *b = !*b;
            }
            set[0] = false;
        }
        Some(Node::Class(set))
    }
}

/// Work limit of one match (the original backtracks without a bound; this keeps a hostile rule from hanging the UI).
const STEP_LIMIT: u32 = 2_000_000;

struct Matcher<'a> {
    s: &'a [u8],
    steps: Cell<u32>,
}

type Cont<'k> = &'k mut dyn FnMut(usize) -> bool;

impl Matcher<'_> {
    fn seq(&self, seq: &[Piece], pos: usize, k: Cont) -> bool {
        match seq.split_first() {
            None => k(pos),
            Some((p, rest)) => self.piece(p, pos, &mut |np| self.seq(rest, np, &mut *k)),
        }
    }

    fn piece(&self, p: &Piece, pos: usize, k: Cont) -> bool {
        match p.rep {
            Rep::One => self.node(&p.node, pos, k),
            Rep::Opt => self.node(&p.node, pos, &mut *k) || k(pos),
            Rep::Star => self.star(&p.node, pos, k),
            Rep::Plus => self.node(&p.node, pos, &mut |np| self.star(&p.node, np, &mut *k)),
        }
    }

    fn star(&self, n: &Node, pos: usize, k: Cont) -> bool {
        self.node(n, pos, &mut |np| np != pos && self.star(n, np, &mut *k)) || k(pos)
    }

    fn node(&self, n: &Node, pos: usize, k: Cont) -> bool {
        let steps = self.steps.get() + 1;
        self.steps.set(steps);
        if steps > STEP_LIMIT {
            return false;
        }
        let one = |ok: bool, k: Cont| ok && k(pos + 1);
        match n {
            Node::Char(c) => one(self.s.get(pos) == Some(c), k),
            Node::Any => one(pos < self.s.len(), k),
            Node::Class(set) => one(self.s.get(pos).is_some_and(|&b| set[b as usize]), k),
            Node::Bol => pos == 0 && k(pos),
            Node::Eol => pos == self.s.len() && k(pos),
            Node::Group(alts) => alts.iter().any(|a| self.seq(a, pos, &mut *k)),
        }
    }
}

impl V8Regex {
    /// `regcomp`; `None` for every syntax error of the original.
    pub fn new(rule: &[u8]) -> Option<Self> {
        let mut p = Parser { p: rule, i: 0, groups: 0 };
        p.alts(false).map(V8Regex)
    }

    /// `RegExp::Compare` (`regexec`): true when the expression matches anywhere in `text`.
    pub fn is_match(&self, text: &[u8]) -> bool {
        let m = Matcher { s: text, steps: Cell::new(0) };
        (0..=text.len()).any(|start| self.0.iter().any(|a| m.seq(a, start, &mut |_| true)))
    }
}

/// Text as the client's 8-bit string (`zone::text_bytes`).
fn bytes(s: &str) -> Vec<u8> {
    super::zone::text_bytes(s)
}

/// `RegExp(rule).Compare(text)`.
pub fn matches(rule: &str, text: &str) -> bool {
    V8Regex::new(&bytes(rule)).is_some_and(|r| r.is_match(&bytes(text)))
}

// ------------------------------------------------------------------------------------------------ state + /filter

/// `ChatFilterEnabled` + `ChatFilterRules`: the `Message` keeps insertion-ordered named fields, each holding a list of strings
/// (the BMessage-style API `AddString` / `FindString(name, out, index)` / `CountNames` / `GetName` / `RemoveName`; field order and
/// append-on-duplicate are the BMessage behaviour, **[GUESS]** for Funcom's clone).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FilterState {
    pub enabled: bool,
    fields: Vec<(String, Vec<String>)>,
}

fn path() -> Option<PathBuf> {
    super::super::prefs::dir().map(|d| d.join("ChatFilter.xml"))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

impl FilterState {
    /// First string of every named field, in field order = the rules `FUN_10084f9e` compiles.
    #[cfg(test)]
    pub fn rules(&self) -> Vec<String> {
        self.fields.iter().filter_map(|(_, v)| v.first().cloned()).collect()
    }

    /// `FUN_10084f9e`: the message is dropped when the filter is on and a rule matches.
    pub fn drops(&self, text: &str) -> bool {
        self.enabled && self.fields.iter().filter_map(|(_, v)| v.first()).any(|r| matches(r, text))
    }

    /// The archive written to `<prefs>/ChatFilter.xml` (**[GUESS]** layout: the original keeps both DValues in MainPrefs).
    pub fn to_xml(&self) -> String {
        let mut o = format!("<Archive code=\"0\">\n    <Bool name=\"ChatFilterEnabled\" value=\"{}\" />\n    <Message name=\"ChatFilterRules\">\n", u8::from(self.enabled));
        for (n, vs) in &self.fields {
            for v in vs {
                o += &format!("        <String name=\"{}\" value=\"{}\" />\n", esc(n), esc(v));
            }
        }
        o + "    </Message>\n</Archive>\n"
    }

    pub fn from_xml(src: &str) -> Self {
        let mut s = Self::default();
        let Ok(root) = ao_gui::xml::parse(src) else { return s };
        for c in &root.children {
            match c.name.as_str() {
                "Bool" => s.enabled = c.attr("value").is_some_and(|v| v == "1" || v == "true"),
                "Message" => {
                    for e in &c.children {
                        if let (Some(n), Some(v)) = (e.attr("name"), e.attr("value")) {
                            s.add(n, v);
                        }
                    }
                }
                _ => {}
            }
        }
        s
    }

    pub fn load() -> Self {
        path().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| Self::from_xml(&s)).unwrap_or_default()
    }

    fn save(&self) {
        if let Some(p) = path() {
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(p, self.to_xml());
        }
    }

    /// `Message::AddString(name, value)`.
    fn add(&mut self, name: &str, value: &str) {
        match self.fields.iter_mut().find(|f| f.0 == name) {
            Some(f) => f.1.push(value.to_owned()),
            None => self.fields.push((name.to_owned(), vec![value.to_owned()])),
        }
    }

    /// `/filter` (`FUN_100b8d4e`): `t` = tokens (`/filter`, sub command, rest of the line). Returns the info lines (colour 0x52) and saves
    /// the DValues when a rule or the switch changed. Sub commands are compared case-sensitively; unknown ones print nothing.
    pub fn command(&mut self, t: &[String]) -> Vec<String> {
        if t.len() < 2 {
            return vec![format!("Usage: {} [list|del|add|enable|disable|clear]", t.first().map_or("/filter", String::as_str))];
        }
        let arg = t.get(2);
        let out = match (t[1].as_str(), arg) {
            ("list", _) => {
                return self.fields.iter().filter_map(|(n, v)| Some(format!("{:3}: {}", super::cmd::atoi(n), v.first()?))).collect();
            }
            ("del", Some(a)) => {
                self.fields.retain(|f| &f.0 != a);
                "Deleted.".to_owned()
            }
            ("add", Some(a)) => {
                let n = self.fields.len();
                self.add(&n.to_string(), a);
                format!("Added as {n}.")
            }
            ("enable", _) => {
                self.enabled = true;
                "Enabled.".into()
            }
            ("disable", _) => {
                self.enabled = false;
                "Disabled.".into()
            }
            ("clear", _) => {
                self.fields.clear();
                "Cleared.".into()
            }
            _ => return vec![],
        };
        self.save();
        vec![out]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v8_syntax() {
        let m = |r: &str, t: &str| matches(r, t);
        assert!(m("gold", "sell gold now"));
        assert!(m("^sell", "sell gold") && !m("^gold", "sell gold"));
        assert!(m("gold$", "sell gold") && !m("sell$", "sell gold"));
        assert!(m("a.c", "xabcx") && !m("a.c", "ac"));
        assert!(m("colou?r", "color") && m("colou?r", "colour"));
        assert!(m("ab+c", "abbbc") && !m("ab+c", "ac"));
        assert!(m("ab*c", "ac") && m("(foo|bar)+z", "xbarfooz"));
        assert!(m("[a-c]x", "bx") && !m("[a-c]x", "dx") && m("[^a-c]x", "dx") && m("[]x]y", "]y") && m("[a-]y", "-y"));
        // `{` is an ordinary character, `\` quotes
        assert!(m("a{2}", "a{2}") && !m("a{2}", "aa"));
        assert!(m("a\\.b", "a.b") && !m("a\\.b", "axb") && m("\\*", "2*3"));
        // backslash in a class is a literal
        assert!(m("[\\]x", "\\x"));
        // case sensitive
        assert!(!m("Gold", "gold"));
    }

    #[test]
    fn v8_errors_never_match() {
        for bad in ["(", ")", "*a", "a**", "[a", "[z-a]", "a\\", "(a*)*", "^*", "(a|)+", "((((((((((a))))))))))"] {
            assert!(V8Regex::new(bad.as_bytes()).is_none(), "{bad}");
            assert!(!matches(bad, "aaa"));
        }
        // empty branches are fine in alternation, `a?` may be empty
        assert!(matches("a|", "zzz") && matches("(a?)b", "b"));
    }

    #[test]
    fn pathological_rule_terminates() {
        let t = "a".repeat(60);
        assert!(!matches("(a|aa)+b", &t));
    }

    #[test]
    fn filter_command_flow() {
        let tk = |s: &str| super::super::cmd::tokenize(s, 3);
        let mut f = FilterState::default();
        assert_eq!(f.command(&tk("/filter")), ["Usage: /filter [list|del|add|enable|disable|clear]"]);
        assert!(f.command(&tk("/filter bogus")).is_empty());
        assert_eq!(f.command(&tk("/filter add spam+")), ["Added as 0."]);
        assert_eq!(f.command(&tk("/filter add ^buy")), ["Added as 1."]);
        assert_eq!(f.command(&tk("/filter list")), ["  0: spam+", "  1: ^buy"]);
        assert!(!f.drops("spam"), "disabled");
        assert_eq!(f.command(&tk("/filter enable")), ["Enabled."]);
        assert!(f.drops("lots of spamm") && f.drops("buy now") && !f.drops("sell now"));
        // delete 0, then the next number is the field count (1) and joins the existing field 1 as a second string, which is
        // neither listed nor applied (BMessage semantics)
        assert_eq!(f.command(&tk("/filter del 0")), ["Deleted."]);
        assert_eq!(f.command(&tk("/filter add zzz")), ["Added as 1."]);
        assert_eq!(f.command(&tk("/filter list")), ["  1: ^buy"]);
        assert!(!f.drops("zzz"));
        let back = FilterState::from_xml(&f.to_xml());
        assert_eq!(back, f);
        assert_eq!(f.command(&tk("/filter disable")), ["Disabled."]);
        assert_eq!(f.command(&tk("/filter clear")), ["Cleared."]);
        assert!(f.rules().is_empty());
    }
}
