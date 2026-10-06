//! Data model of the options window (`OptionPanelModule_c`, GUI.dll 0xc2c03; docs/gui.md "Options window"): `OptionPanel/Root.xml` as pages of controls
//! and the binding of every control to its DValue / `IndependentPrefs` variable (`OptionControl_c` ctor `FUN_100c22fc` 0x100c22fc).

use super::super::dvalue::{DValues, Kind, Variant};
use ao_gui::xml::{self, Element};

/// `opt_type` (`+0x1b4`): `int` -> 4 (an int `IndependentPref`), `float` -> 7 (a float `IndependentPref`), anything else (also absent) -> 0x12 (a DValue).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    Variant,
    Int,
    Float,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Ctl {
    /// `OptionCheckBox_c` (`FUN_100c1be9`): a toggle `Button_c` (art 0x8b / 0x8a) and the label.
    Check,
    /// `OptionSlider_c` (`FUN_100c0aa9`): label, value text and a `Slider_c`.
    Slider,
    /// `OptionRadioButtonGroup_c` (`FUN_100c2fd5`): label above a `RadioButtonGroup_c` of `(label, value)` buttons.
    Radio(Vec<(String, i32)>),
}

/// One option control of `Root.xml`.
#[derive(Clone, Debug, PartialEq)]
pub struct Opt {
    pub ctl: Ctl,
    pub var: String,
    pub ty: Ty,
    /// `opt_category` (`+0x1b8`, default 2 = Login) mapped by `FUN_100c203c`: Char (3) -> the char prefs, anything else the login prefs. Only used by int / float prefs.
    pub kind: Kind,
    /// `label` attribute as written (`#Key` is text.mdb category 700).
    pub label: String,
    pub tooltip: Option<(String, String)>,
    /// `view_enable_expression` (`View::SetEnableExpression`): the control is enabled while it holds; empty = always.
    pub enable: String,
    /// `opt_bitnum` of a check box: it shows / sets one bit of the (int) variable.
    pub bit: Option<u32>,
    /// `value_fmt` and `value_scale` (an `ExpressionParser` expression, 1.0 when absent / unparsable) of a slider.
    pub fmt: String,
    pub scale: f64,
    /// `layout_borders` as written.
    pub borders: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Opt(Opt),
    Text { value: String, borders: String },
    /// `VLayoutSpacer` min / max.
    Spacer { min: f32, max: f32 },
    /// A plain `<View>` grouping controls (the control-centre page).
    Group { borders: String, items: Vec<Item> },
}

/// One `ScrollView` of `Root.xml` = one entry of the category tree; `label` as written (`#Key` -> text, `/` separates the tree levels).
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub label: String,
    pub items: Vec<Item>,
}

impl Opt {
    fn from_xml(e: &Element, ctl: Ctl) -> Opt {
        let a = |k: &str| e.attr(k).unwrap_or("").to_string();
        let ty = match e.attr("opt_type").map(str::to_ascii_lowercase).as_deref() {
            Some("int") => Ty::Int,
            Some("float") => Ty::Float,
            _ => Ty::Variant,
        };
        let kind = if e.attr("opt_category").is_some_and(|c| c.eq_ignore_ascii_case("char")) { Kind::Char } else { Kind::Login };
        Opt {
            ctl,
            var: a("opt_variable"),
            ty,
            kind,
            label: a("label"),
            tooltip: e.attr("tooltip").map(|t| (t.to_string(), a("tooltip_body"))),
            enable: a("view_enable_expression"),
            bit: e.attr("opt_bitnum").and_then(|b| b.trim().parse().ok()),
            fmt: a("value_fmt"),
            scale: e.attr("value_scale").and_then(|s| s.trim().parse().ok()).unwrap_or(1.0),
            borders: a("layout_borders"),
        }
    }

    /// The numeric view of the stored value (`Variant::AsDouble`); 0 when the variable does not exist.
    pub fn value(&self, d: &DValues) -> f64 {
        match self.ty {
            Ty::Int => f64::from(d.prefs.get_int(&self.var, self.kind).unwrap_or(0)),
            Ty::Float => f64::from(d.prefs.get_float(&self.var, self.kind).unwrap_or(0.0)),
            Ty::Variant => match d.get(&self.var) {
                Some(Variant::Int(v)) => *v as f64,
                Some(Variant::Bool(b)) => f64::from(u8::from(*b)),
                Some(Variant::Float(f)) => f64::from(*f),
                _ => 0.0,
            },
        }
    }

    /// `(min, max)` of the variable (`GetMinMaxValues` / `Get{Int,Float}MinMaxValue`); `(0, 1)` for a DValue without a range (the bool-like options).
    pub fn range(&self, d: &DValues) -> (f64, f64) {
        match self.ty {
            Ty::Int => d.prefs.int_range(&self.var, self.kind).map_or((0.0, 1.0), |(a, b)| (f64::from(a), f64::from(b))),
            Ty::Float => d.prefs.float_range(&self.var, self.kind).map_or((0.0, 1.0), |(a, b)| (f64::from(a), f64::from(b))),
            Ty::Variant => {
                let (lo, hi) = d.min_max(&self.var);
                (num(&lo).unwrap_or(0.0), num(&hi).unwrap_or(1.0))
            }
        }
    }

    /// Check box state: the variable as a bool, or the `opt_bitnum` bit of it.
    pub fn checked(&self, d: &DValues) -> bool {
        let v = self.value(d) as i64;
        match self.bit {
            Some(b) => v >> b.min(63) & 1 != 0,
            None => v != 0,
        }
    }

    /// `FUN_100c2052` (`SetValue(v, true)` of the control) for a number: an int pref gets the truncated int (`Variant::AsInt32`), a float pref the float (both
    /// clamped to the pref's range, `SetPrefInt` 0x10002e5c / `SetPrefFloat` 0x10002f4f), a DValue `SetDValue` (clamped to its min / max). The original passes
    /// a `Variant` of the slider's double; the DValue keeps the type it has here (our readers are typed: a bool stays a bool, an int an int).
    pub fn set_value(&self, d: &mut DValues, v: f64) {
        match self.ty {
            Ty::Int => {
                d.prefs.set_int(&self.var, v as i32, self.kind);
            }
            Ty::Float => {
                d.prefs.set_float(&self.var, v as f32, self.kind);
            }
            Ty::Variant => match d.get(&self.var) {
                Some(Variant::Int(_)) => {
                    d.set(&self.var, Variant::Int(v as i64));
                }
                Some(Variant::Bool(_)) => {
                    d.set(&self.var, Variant::Bool(v != 0.0));
                }
                _ => {
                    d.set(&self.var, Variant::Float(v as f32));
                }
            },
        }
    }

    /// Check box click (`FUN_100c1ae6`): without `opt_bitnum` the bool, with it the bit set / cleared in the int (`AsInt32 | 1 << bit`).
    pub fn set_checked(&self, d: &mut DValues, on: bool) {
        match self.bit {
            None => self.set_value(d, f64::from(u8::from(on))),
            Some(b) => {
                let cur = self.value(d) as i64 as u32;
                let bit = 1u32 << (b & 31);
                self.set_value(d, f64::from(if on { cur | bit } else { cur & !bit } as i32));
            }
        }
    }

    /// Slider value text: `String::Format(value_fmt, value * value_scale)` (`FUN_100c082a`), empty without a `value_fmt`.
    pub fn value_text(&self, v: f64) -> String {
        if self.fmt.is_empty() { String::new() } else { format_c(&self.fmt, v * self.scale) }
    }
}

fn num(v: &Variant) -> Option<f64> {
    match v {
        Variant::Int(i) => Some(*i as f64),
        Variant::Bool(b) => Some(f64::from(u8::from(*b))),
        Variant::Float(f) => Some(f64::from(*f)),
        _ => None,
    }
}

/// The `printf` subset `value_fmt` uses: `%%`, `%.Nf`, `%f`, `%d`; any other text is copied.
pub fn format_c(fmt: &str, v: f64) -> String {
    let mut out = String::new();
    let mut it = fmt.chars().peekable();
    while let Some(c) = it.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        if it.peek() == Some(&'%') {
            it.next();
            out.push('%');
            continue;
        }
        let mut prec = None;
        if it.peek() == Some(&'.') {
            it.next();
            let mut n = 0usize;
            while let Some(d) = it.peek().and_then(|c| c.to_digit(10)) {
                n = n * 10 + d as usize;
                it.next();
            }
            prec = Some(n);
        }
        match it.next() {
            Some('f') => out += &format!("{v:.*}", prec.unwrap_or(6)),
            Some('d') | Some('i') => out += &format!("{}", v as i64),
            Some(o) => {
                out.push('%');
                out.push(o);
            }
            None => out.push('%'),
        }
    }
    out
}

fn items(e: &Element) -> Vec<Item> {
    let mut out = vec![];
    for c in &e.children {
        let a = |k: &str| c.attr(k).unwrap_or("").to_string();
        match c.name.as_str() {
            "OptionCheckBox" => out.push(Item::Opt(Opt::from_xml(c, Ctl::Check))),
            "OptionSlider" => out.push(Item::Opt(Opt::from_xml(c, Ctl::Slider))),
            "OptionRadioButtonGroup" => {
                let radios = c.children.iter().filter(|r| r.name == "RadioButton").map(|r| (r.attr("label").unwrap_or("").to_string(), r.attr("value").and_then(|v| v.trim().parse().ok()).unwrap_or(0))).collect();
                out.push(Item::Opt(Opt::from_xml(c, Ctl::Radio(radios))));
            }
            "TextView" => out.push(Item::Text { value: a("value"), borders: a("layout_borders") }),
            "VLayoutSpacer" => {
                let f = |k: &str, d: f32| c.attr(k).and_then(|s| s.trim().parse().ok()).unwrap_or(d);
                out.push(Item::Spacer { min: f("min_size", 0.0), max: f("max_size", 16000.0) });
            }
            "View" => out.push(Item::Group { borders: a("layout_borders"), items: self::items(c) }),
            _ => {}
        }
    }
    out
}

/// `OptionPanelModule` loader (`FUN_100c3691`): every top-level element of `Root.xml` that is a `View` (the `ScrollView`s) is a page; its contents are the
/// controls of the `ScrollViewChild`'s view.
pub fn parse(text: &str) -> Vec<Page> {
    let Ok(root) = xml::parse(text) else { return vec![] };
    root.children
        .iter()
        .filter(|sv| sv.name == "ScrollView")
        .map(|sv| {
            let body = sv.children.iter().find(|c| c.name == "ScrollViewChild").and_then(|c| c.children.iter().find(|v| v.name == "View").or(Some(c)));
            Page { label: sv.attr("label").unwrap_or("").to_string(), items: body.map(items).unwrap_or_default() }
        })
        .collect()
}

impl Page {
    /// Every option control of the page in order (groups flattened).
    pub fn opts(&self) -> Vec<&Opt> {
        fn walk<'a>(items: &'a [Item], out: &mut Vec<&'a Opt>) {
            for i in items {
                match i {
                    Item::Opt(o) => out.push(o),
                    Item::Group { items, .. } => walk(items, out),
                    _ => {}
                }
            }
        }
        let mut v = vec![];
        walk(&self.items, &mut v);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r##"<root>
      <ScrollView label="#A/b" scroll_client="s"><ScrollViewChild name="s"><View view_layout="vertical">
        <OptionCheckBox label="#X" opt_variable="flag" opt_type="variant"/>
        <OptionCheckBox label="bit" opt_variable="VisualFlags" opt_bitnum="2" opt_category="Char" opt_type="variant" view_enable_expression="dvalue:flag"/>
        <OptionSlider label="s" opt_variable="vol" value_fmt="&lt;font color=#70C4D0&gt;%.0f%%&lt;/font&gt;" value_scale="100"/>
        <View layout_borders="Rect(10,0,0,0)"><OptionRadioButtonGroup label="r" opt_variable="mode"><RadioButton label="a" value="0"/><RadioButton label="b" value="2"/></OptionRadioButtonGroup></View>
        <VLayoutSpacer max_size="5" min_size="5"/>
      </View></ScrollViewChild></ScrollView></root>"##;

    fn store() -> DValues {
        let mut d = DValues::default();
        d.load_config(
            r#"<Root><Value name="flag" value="true"/><Value name="vol" value="0.5" min="0.0" max="1.0"/><Value name="mode" value="0"/><Value name="VisualFlags" value="31"/><Value name="n" value="3" min="1" max="10"/></Root>"#,
            super::super::super::dvalue::CAT_CHAR,
            true,
        );
        d
    }

    #[test]
    fn root_parses_into_pages_and_controls() {
        let p = parse(XML);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].label, "#A/b");
        let o = p[0].opts();
        assert_eq!(o.len(), 4);
        assert_eq!((o[1].bit, o[1].kind, o[1].enable.as_str()), (Some(2), Kind::Char, "dvalue:flag"));
        assert_eq!((o[2].scale, o[2].ctl.clone()), (100.0, Ctl::Slider));
        assert_eq!(o[3].ctl, Ctl::Radio(vec![("a".into(), 0), ("b".into(), 2)]));
        assert!(matches!(p[0].items.last(), Some(Item::Spacer { min, max }) if *min == 5.0 && *max == 5.0));
    }

    #[test]
    fn value_format_and_scale() {
        assert_eq!(format_c("<font color=#70C4D0>%.0f%%</font>", 0.8 * 100.0), "<font color=#70C4D0>80%</font>");
        assert_eq!(format_c("%.1fs", 8.0), "8.0s");
        assert_eq!(format_c("%.0f m", 80.0), "80 m");
        let o = &parse(XML)[0].opts()[2].clone();
        assert_eq!(o.value_text(0.333), "<font color=#70C4D0>33%</font>");
    }

    #[test]
    fn control_writes_the_variable_and_clamps() {
        let mut d = store();
        let p = parse(XML);
        let o = p[0].opts();
        assert!(o[0].checked(&d));
        o[0].set_checked(&mut d, false);
        assert_eq!(d.get("flag"), Some(&Variant::Bool(false)), "a bool stays a bool");
        // slider: DValue range 0..1, value above the max is clamped by SetDValue
        o[2].set_value(&mut d, 1.7);
        assert_eq!(o[2].value(&d), 1.0);
        assert_eq!(o[2].range(&d), (0.0, 1.0));
        o[2].set_value(&mut d, 0.25);
        assert_eq!(d.get("vol"), Some(&Variant::Float(0.25)));
        // radio value
        o[3].set_value(&mut d, 2.0);
        assert_eq!(d.get("mode"), Some(&Variant::Int(2)));
        assert!(d.take_changed().contains(&"mode".to_string()));
    }

    #[test]
    fn bit_checkbox_sets_one_bit_of_the_int() {
        let mut d = store();
        let p = parse(XML);
        let o = p[0].opts()[1].clone();
        assert!(o.checked(&d));
        o.set_checked(&mut d, false);
        assert_eq!(d.get("VisualFlags"), Some(&Variant::Int(31 & !4)));
        assert!(!o.checked(&d));
        o.set_checked(&mut d, true);
        assert_eq!(d.get("VisualFlags"), Some(&Variant::Int(31)));
    }

    #[test]
    fn int_and_float_prefs_use_their_registered_ranges() {
        let mut d = DValues::default();
        d.prefs = super::super::super::dvalue::IndepPrefs::with_defaults();
        let int = Opt { ctl: Ctl::Check, var: "RealisticWater".into(), ty: Ty::Int, kind: Kind::Login, label: String::new(), tooltip: None, enable: String::new(), bit: None, fmt: String::new(), scale: 1.0, borders: String::new() };
        assert_eq!((int.checked(&d), int.range(&d)), (true, (0.0, 1.0)));
        int.set_checked(&mut d, false);
        assert!(!int.checked(&d));
        int.set_value(&mut d, 5.0);
        assert_eq!(int.value(&d), 1.0, "clamped to the registered 0..1");
        let fl = Opt { var: "ViewDistance".into(), ty: Ty::Float, ctl: Ctl::Slider, ..int.clone() };
        assert_eq!(fl.value(&d), 0.8_f32 as f64);
        fl.set_value(&mut d, 0.5);
        assert_eq!(d.view_distance(), 0.5);
        let ch = Opt { var: "FadeCharacterStartDist".into(), ty: Ty::Float, kind: Kind::Char, ..fl.clone() };
        assert_eq!(ch.range(&d), (0.1_f32 as f64, 10.0));
    }
}
