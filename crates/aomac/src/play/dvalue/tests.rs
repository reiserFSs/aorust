use super::*;
use crate::play::camera::far_plane;
use std::path::PathBuf;

fn client() -> Option<PathBuf> {
    let d = PathBuf::from(std::env::var_os("HOME")?).join("Games/ProjectRubiKa/client");
    d.join("cd_image/gui/Default/LoginPrefs.xml").exists().then_some(d)
}

fn toks(l: &str) -> Vec<String> {
    l.split_whitespace().map(String::from).collect()
}

/// A store with a few templates of the shapes the real files have (min / max, bool, string, archive, keep_default).
fn small() -> DValues {
    let mut s = DValues { prefs: IndepPrefs::with_defaults(), ..Default::default() };
    s.load_config(
        r#"<?xml version="1.0" ?><Root>
        <Value name="DisplayCharViewDistance" value="80" max="80" min="5"/>
        <Value name="NumHotbars" max="10" min="1" value="1"/>
        <Value name="LockHotbars" value="false"/>
        <Value name="ChatFontName" value="&quot;Verdana&quot;"/>
        <Value name="Fade" max="1.0" min="0.0" value="0.5"/>
        <Value name="Scratch" value="1" keep_default="false"/>
        <Archive name="Conf" code="0"><Rect name="WindowFrame" value="Rect(1.000000,2.000000,3.000000,4.000000)" /></Archive>
        </Root>"#,
        CAT_CHAR,
        true,
    );
    s
}

#[test]
fn variant_text_follows_load_and_save_to_string() {
    use Variant::*;
    assert_eq!(Variant::from_text("TRUE"), Some(Bool(true)));
    assert_eq!(Variant::from_text("-12"), Some(Int(-12)));
    assert_eq!(Variant::from_text("0.25"), Some(Float(0.25)));
    assert_eq!(Variant::from_text("\"Verdana\""), Some(Str("Verdana".into())));
    assert_eq!(Variant::from_text("\"\""), Some(Str(String::new())));
    assert_eq!(Variant::from_text("Rect(1.000000,2.000000,3.000000,4.000000)"), Some(Raw("Rect(1.000000,2.000000,3.000000,4.000000)".into())));
    assert_eq!(Variant::from_text("CAMERA_3RD_LOCK"), None, "unquoted words fail to load");
    assert_eq!(Float(0.25).to_text(), "0.250000");
    assert_eq!(Str("a b".into()).to_text(), "\"a b\"");
    assert_eq!(Float(0.8).as_string(), "0.800000");
    assert_eq!(Bool(false).as_string(), "false");
}

#[test]
fn set_clamps_to_min_max_and_signals_once() {
    let mut s = small();
    assert!(s.set("NumHotbars", Variant::Int(99)));
    assert_eq!(s.get("NumHotbars"), Some(&Variant::Int(10)));
    s.set("NumHotbars", Variant::Int(-4));
    assert_eq!(s.get("NumHotbars"), Some(&Variant::Int(1)));
    s.set("NumHotbars", Variant::Int(1)); // same value: no signal
    assert_eq!(s.take_changed(), ["NumHotbars", "NumHotbars"]);
    assert!(!s.set("NoSuchVariable", Variant::Int(1)), "unknown names are ignored");
    assert!(!s.exists("NoSuchVariable"));
    // the HUD's flag view keeps the type of a bool
    s.set_i64("LockHotbars", 1);
    assert_eq!(s.get("LockHotbars"), Some(&Variant::Bool(true)));
    assert!(s.flag("LockHotbars") && s.get_i64("Fade").is_none());
}

#[test]
fn files_round_trip_through_the_prefs_dir() {
    let dir = std::env::temp_dir().join(format!("aomac-dvalue-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut a = small();
    a.open_user(&dir, "acct", 7);
    a.set("NumHotbars", Variant::Int(4));
    a.set("ChatFontName", Variant::Str("Say \"x\" & <y>".into()));
    a.set("Scratch", Variant::Int(9));
    a.prefs.set_float("ViewDistance", 0.5, Kind::Login);
    a.prefs.set_int("FadeCharacter", 0, Kind::Char);
    a.prefs.set_string("CCSelectedName", "Bob", Kind::Login);
    a.save_user();
    let xml = std::fs::read_to_string(dir.join("acct/Char7/Prefs.xml")).unwrap();
    assert!(xml.contains("<Value name=\"NumHotbars\" value=\"4\" />"), "{xml}");
    assert!(xml.contains("<Archive name=\"Conf\" code=\"0\"><Rect name=\"WindowFrame\""), "{xml}");
    assert!(!xml.contains("Scratch"), "keep_default=false variables are not saved");
    let cfg = std::fs::read_to_string(dir.join("acct/Login.cfg")).unwrap();
    assert!(cfg.contains("ViewDistance 0.500000\r\n") && cfg.contains("CCSelectedName \"Bob\"\r\n"), "{cfg:?}");

    let mut b = small();
    b.open_user(&dir, "acct", 7);
    assert_eq!(b.get("NumHotbars"), Some(&Variant::Int(4)));
    assert_eq!(b.get("ChatFontName"), Some(&Variant::Str("Say \"x\" & <y>".into())));
    assert_eq!(b.get("Scratch"), Some(&Variant::Int(1)));
    assert!(matches!(b.get("Conf"), Some(Variant::Archive(x)) if x.contains("WindowFrame")));
    assert_eq!(b.view_distance(), 0.5);
    assert_eq!(b.prefs.get_int("FadeCharacter", Kind::Char), Some(0));
    assert_eq!(b.prefs.get_string("CCSelectedName", Kind::Login), Some("Bob"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn user_archive_inherits_the_default_children() {
    let mut s = small();
    s.load_config(r#"<Root><Archive name="Conf" code="0"><Bool name="open" value="true" /></Archive></Root>"#, CAT_CHAR, false);
    let Some(Variant::Archive(x)) = s.get("Conf") else { panic!() };
    assert!(x.contains("name=\"open\"") && x.contains("WindowFrame"), "{x}");
}

#[test]
fn independent_prefs_file_format() {
    let mut p = IndepPrefs::with_defaults();
    p.load("// comment\nUseWindEngine 0\nusername bob\nViewDistance 2.5\nFogMode 0x2\nCCSelectedName \"A B\"\n#x 1\nNewName 12\n", Kind::Login);
    assert_eq!(p.get_int("UseWindEngine", Kind::Login), Some(0));
    assert_eq!(p.get_float("ViewDistance", Kind::Login), Some(1.0), "clamped to max 1");
    assert_eq!(p.get_int("FogMode", Kind::Login), Some(2));
    assert_eq!(p.get_string("CCSelectedName", Kind::Login), Some("A B"));
    assert_eq!(p.get_int("NewName", Kind::Login), Some(12), "unregistered names are registered unbounded");
    let t = p.save(Kind::Login);
    assert!(!t.to_ascii_lowercase().contains("username"));
    // ints (sorted), then floats, then strings
    let (i, f, s) = (t.find("WasCharacterCreated 1\r\n").unwrap(), t.find("AspectRation ").unwrap(), t.find("CCSelectedName").unwrap());
    assert!(i < f && f < s);
}

#[test]
fn option_and_setoption_forms() {
    let mut s = small();
    let say = |s: &mut DValues, l: &str| s.command(&toks(l)).unwrap();
    // prefs first: `GetPrefEasy` text, escaped
    assert_eq!(say(&mut s, "/option ViewDistance"), [Out { error: false, text: "Login-pref &lt;ViewDistance&gt; is &lt;0.800000&gt;".into() }]);
    assert_eq!(say(&mut s, "/option FadeCharacterEndDist")[0].text, "Char-pref &lt;FadeCharacterEndDist&gt; is &lt;0.700000&gt;");
    assert_eq!(say(&mut s, "/option CCSelectedBreed")[0].text, "Login-pref &lt;CCSelectedBreed&gt; is &lt;0&gt;");
    // then DValues
    assert_eq!(say(&mut s, "/option NumHotbars")[0].text, "Variable &lt;NumHotbars&gt; is &lt;1&gt;");
    assert_eq!(say(&mut s, "/option Nope"), [Out { error: true, text: "Can't find option &lt;Nope&gt;.".into() }]);
    assert_eq!(say(&mut s, "/option"), [Out { error: true, text: "Invalid syntax: /option . Use /option &lt;OptionName&gt; [value]".into() }]);
    // /option sets silently, /setoption reports
    assert!(say(&mut s, "/option NumHotbars 3").is_empty());
    assert_eq!(s.get("NumHotbars"), Some(&Variant::Int(3)));
    assert_eq!(say(&mut s, "/setoption NumHotbars 99")[0].text, "Changed variable &lt;NumHotbars&gt; from &lt;3&gt; to &lt;10&gt;");
    assert_eq!(say(&mut s, "/setoption NumHotbars 2*(1+3)")[0].text, "Changed variable &lt;NumHotbars&gt; from &lt;10&gt; to &lt;8&gt;");
    assert_eq!(say(&mut s, "/setoption NumHotbars bogus")[0], Out { error: true, text: "Failed to parse expression &lt;bogus&gt;.".into() });
    assert_eq!(say(&mut s, "/setoption Nope 1")[0].text, "Can't find option &lt;Nope&gt;.");
    assert_eq!(say(&mut s, "/setoption Fade 0.25")[0].text, "Changed variable &lt;Fade&gt; from &lt;0.500000&gt; to &lt;0.250000&gt;");
    assert_eq!(say(&mut s, "/setoption Fade 3")[0].text, "Changed variable &lt;Fade&gt; from &lt;0.250000&gt; to &lt;1.000000&gt;", "an int above the max is replaced by the float max");
    assert_eq!(say(&mut s, "/setoption LockHotbars true")[0].text, "Changed variable &lt;LockHotbars&gt; from &lt;false&gt; to &lt;true&gt;");
    assert_eq!(say(&mut s, "/setoption ChatFontName \"A<b>\"")[0].text, "Changed variable &lt;ChatFontName&gt; from &lt;Verdana&gt; to &lt;A&lt;b&gt;&gt;");
    // a pref set reports the clamped value (`SetPrefFloat` result)
    assert_eq!(say(&mut s, "/setoption ViewDistance 7")[0].text, "Changed login-pref &lt;ViewDistance&gt; to &lt;1.000000&gt;");
    assert!(say(&mut s, "/option ViewDistance 0.4").is_empty());
    assert_eq!(s.view_distance(), 0.4);
}

#[test]
fn dvalue_creates_missing_variables() {
    let mut s = small();
    let say = |s: &mut DValues, l: &str| s.command(&toks(l)).unwrap();
    assert_eq!(say(&mut s, "/dvalue Brand 5")[0].text, "Changed variable &lt;Brand&gt; from &lt;none&gt; to &lt;5&gt;");
    assert_eq!(say(&mut s, "/dvalue Brand")[0].text, "Variable &lt;Brand&gt; is &lt;5&gt;");
    assert_eq!(say(&mut s, "/dvalue Brand 6")[0].text, "Changed variable &lt;Brand&gt; from &lt;5&gt; to &lt;6&gt;");
    assert_eq!(say(&mut s, "/dvalue Nope")[0].text, "Can't find option &lt;Nope&gt;.");
    assert_eq!(say(&mut s, "/dvalue")[0].text, "Invalid syntax: /dvalue . Use /dvalue &lt;OptionName&gt; [value]");
    assert!(!s.save_config(CAT_VARIABLES).contains("Brand"), "created variables are temporary");
    assert!(s.command(&toks("/help")).is_none());
}

#[test]
fn distance_commands_set_what_the_renderer_reads() {
    let mut s = small();
    let say = |s: &mut DValues, l: &str| s.command(&toks(l)).unwrap();
    assert_eq!(s.char_view_distance(), 80.0);
    assert_eq!(far_plane(s.view_distance()), 800.0, "default ViewDistance 0.8 -> 800 m");
    assert!(say(&mut s, "/viewdist 50").is_empty());
    assert_eq!(far_plane(s.view_distance()), 500.0);
    assert!(say(&mut s, "/viewdist 250").is_empty());
    assert_eq!(far_plane(s.view_distance()), 1000.0, "the pref is clamped to 1");
    assert!(say(&mut s, "/chardist 30").is_empty());
    assert_eq!(s.char_view_distance(), 30.0);
    say(&mut s, "/chardist 500");
    assert_eq!(s.char_view_distance(), 80.0, "DisplayCharViewDistance is clamped to 5..80");
    say(&mut s, "/chardist 1");
    assert_eq!(s.char_view_distance(), 5.0);
    assert!(say(&mut s, "/char&viewdist 40 60").is_empty());
    assert_eq!(s.char_view_distance(), 40.0);
    assert!((far_plane(s.view_distance()) - 600.0).abs() < 0.01, "{}", far_plane(s.view_distance()));
    for l in ["/chardist", "/viewdist", "/char&viewdist", "/char&viewdist 10"] {
        assert_eq!(say(&mut s, l), [Out { error: true, text: "Error: To few arguments".into() }], "{l}");
    }
    // without the variable `FUN_1001f964` falls back to 70 m
    assert_eq!(DValues::default().char_view_distance(), 70.0);
}

#[test]
fn shipped_templates_load_with_their_ranges() {
    let Some(dir) = client() else { return };
    let s = DValues::new(&dir);
    assert_eq!(s.get("DisplayCharViewDistance"), Some(&Variant::Int(80)));
    assert_eq!(s.min_max("DisplayCharViewDistance"), (Variant::Int(5), Variant::Int(80)));
    assert_eq!(s.get("NumHotbars"), Some(&Variant::Int(1)));
    assert_eq!(s.get("SoundOnOff"), Some(&Variant::Bool(true)), "MainPrefs.xml");
    assert_eq!(s.get("planet_map_available"), Some(&Variant::Bool(false)), "Variables.xml");
    assert_eq!(s.get("ChatFontName"), Some(&Variant::Str("Verdana".into())));
    assert!(matches!(s.get("KeyBindings"), Some(Variant::Archive(_))));
    assert_eq!(s.view_distance(), 0.8);
    assert_eq!(s.prefs.get_int("FadeCharacter", Kind::Char), Some(1));
    // the saved Login category reloads to the same variables
    let mut t = DValues::new(&dir);
    t.load_config(&s.save_config(CAT_LOGIN), CAT_LOGIN, false);
    assert_eq!(t.get("DisplayCharViewDistance"), s.get("DisplayCharViewDistance"));
}
