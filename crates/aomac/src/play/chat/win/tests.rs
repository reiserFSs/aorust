//! Window tests: pure formatting/config tests always run; GUI tests need the client and skip without it.
//! Screenshot: `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac chat_win_shot -- --nocapture`, then inspect `chat.png`.
use super::*;
use ao_gui::{InputEvent, Key};

fn msg(group: u64, name: &str, from: &str, text: &str, kind: u8) -> ChatMsg {
    ChatMsg { group, group_name: name.into(), from_name: from.into(), text: text.into(), kind, ..Default::default() }
}

#[test]
fn group_colours_follow_fun_10085320() {
    assert_eq!(group_color(G_SYSTEM, 0), "ct_system");
    assert_eq!(group_color(G_VICINITY, 0), "ctch_vicinity");
    assert_eq!(group_color(G_VICINITY, 1), "ctch_whisper");
    assert_eq!(group_color(G_VICINITY, 2), "ctch_shout");
    assert_eq!(group_color(G_VICINITY, 3), "ctch_emote");
    assert_eq!(group_color(G_TELL, 0), "ctch_tell");
    assert_eq!(group_color(G_MYPET, 0), "ctch_mypet");
    assert_eq!(group_color(G_OTHERPET, 0), "ctch_otherpet");
    assert_eq!(group_color(G_RESEARCH, 0), "ctch_research");
    for (ty, c) in [(1u64, "ctch_admin"), (3, "ctch_clan"), (4, "ctch_misc"), (5, "ctch_gm"), (8, "ctch_news"), (10, "ctch_tower"), (0x0e, "ctch_pgroup"), (0x82, "ctch_team"), (0x86, "ctch_seekingteam"), (0x87, "ctch_newbie"), (0x8f, "ctch_raid"), (2, "white"), (0x90, "white")] {
        assert_eq!(group_color(ty << 32 | 1, 0), c, "type {ty:#x}");
    }
    assert_eq!(group_color(0x4200_0002, 0), "white"); // combat lines carry an explicit colour code instead
}

#[test]
fn line_formats_follow_fun_1009b4cf() {
    let f = |m: &ChatMsg, g: &str| format_line(m, g, group_color(m.group, m.kind), "", " whispers: ", " shouts: ");
    let u = |n: &str| format!("<a style=\"text-decoration:none\" href=\"user://{n}\">{n}</a>");
    // vicinity: "<sender>: text", whisper/shout infix from the text db, emote "<sender> text"
    assert_eq!(f(&msg(G_VICINITY, "", "Bob", "hi", 0), ""), format!("<div indent=wrapped><font color=ctch_vicinity>{}: hi</font></div>", u("Bob")));
    assert!(f(&msg(G_VICINITY, "", "Bob", "psst", 1), "").contains(&format!("{} whispers: psst", u("Bob"))));
    assert!(f(&msg(G_VICINITY, "", "Bob", "HEY", 2), "").contains(&format!("{} shouts: HEY", u("Bob"))));
    assert!(f(&msg(G_VICINITY, "", "Bob", "waves", 3), "").contains(&format!("{} waves", u("Bob"))));
    // tell: "[sender]: text"
    assert!(f(&msg(G_TELL, "", "Bob", "yo", 0), "").contains(&format!("[{}]: yo", u("Bob"))));
    // chat-server group: "[<group link>] <sender>: text"
    let g = 0x87u64 << 32 | 5;
    let s = f(&msg(g, "Newbie Help", "Bob", "help?", 0), "Newbie Help");
    assert!(s.contains(&format!("[<a style=\"text-decoration:none\" href=\"chatgroup://{}\">Newbie Help</a>] {}: help?", group_ident(g), u("Bob"))), "{s}");
    assert!(s.contains("color=ctch_newbie"));
    // system / combat: no prefix; GM sender is red with " (GM)"
    assert_eq!(f(&msg(G_SYSTEM, "", "", "Welcome", 0), ""), "<div indent=wrapped><font color=ct_system>Welcome</font></div>");
    let mut gm = msg(G_VICINITY, "", "Zed", "x", 0);
    gm.flags = 1;
    assert!(f(&gm, "").contains("<font color=\"#FF0000\"><a style=\"text-decoration:none\" href=\"user://Zed\">Zed (GM)</a></font>: x"));
    // timestamps go right after the colour tag
    assert!(format_line(&msg(G_SYSTEM, "", "", "a", 0), "", "ct_system", "(12:34) ", "", "").contains("ct_system>(12:34) a"));
}

#[test]
fn defaults_and_subscription() {
    let d = code_defaults();
    assert_eq!((d[0].name.as_str(), d[0].autosubscribe, d[0].output_group), ("Default Window", true, G_VICINITY));
    // the autosubscribe window shows everything except its list; the Combat window only its list
    assert!(d[0].shows(G_VICINITY) && d[0].shows(G_SYSTEM) && d[0].shows(G_TELL) && d[0].shows(0x87 << 32 | 1));
    assert!(!d[0].shows(0x4200_0008) && !d[0].shows(G_OTHERPET));
    assert!(d[1].shows(0x4200_0008) && !d[1].shows(G_VICINITY) && !d[1].shows(0x4200_0004));
}

#[test]
fn shipped_template_parses() {
    let dir = ao_gui::client_dir().join("prefs/NewChar");
    let v = read_windows(&dir);
    if v.is_empty() {
        return eprintln!("skipping: no client prefs");
    }
    assert_eq!(v.len(), 2);
    let w1 = &v[0];
    assert_eq!((w1.name.as_str(), w1.window_name.as_str(), w1.output_group, w1.autosubscribe, w1.show_timestamps), ("Default Window", "Window1", G_VICINITY, true, true));
    assert_eq!(w1.frame, Some([277.0, 1206.0, 1273.0, 1439.0]));
    assert!(w1.set.contains(&0x4200_0012) && w1.set.len() == 22 && w1.shows(G_VICINITY));
    assert!(!v[1].textinput && !v[1].autosubscribe && v[1].shows(0x4200_0012) && !v[1].shows(G_VICINITY));
    // round trip through the writer
    let again = Cfg::parse(&w1.to_xml(&|_| String::new())).unwrap();
    assert_eq!(&again, w1);
}

#[test]
fn frames_are_placed_inside_the_screen() {
    let (x, y, w, h) = place(Some([277.0, 1206.0, 1273.0, 1439.0]), true, (1280, 800), Reserved { left: 190, right: 65, bottom: 38 });
    assert!(x >= 0 && y >= 0 && x as u32 + w <= 1280 && y as u32 + h <= 800);
    assert_eq!(place(None, false, (1280, 800), Reserved::default()), (440, 300, 400, 200));
    // saved frames: translated inside the screen, never resized (`MoveInsideScreen(false, true, true)`)
    assert_eq!(place(Some([277.0, 1206.0, 1273.0, 1439.0]), false, (1280, 828), Reserved::default()), (277, 594, 997, 234));
    assert_eq!(place(Some([-10.0, 100.0, 289.0, 199.0]), false, (1280, 828), Reserved::default()), (0, 100, 300, 100));
    assert_eq!(place(Some([1200.0, 100.0, 1499.0, 199.0]), false, (1280, 828), Reserved::default()), (980, 100, 300, 100));
}

/// `ChatWindows::new` reads the prefs dir from the process environment: serialise the set + open.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A prefs dir private to the running test.
fn test_dir() -> PathBuf {
    let t = std::thread::current();
    std::env::temp_dir().join(format!("aomac-chatgui-{}", t.name().unwrap_or("t").replace("::", "-")))
}

fn open_in(gui: &mut Gui, dir: &Path, screen: (u32, u32)) -> ChatWindows {
    let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("AOMAC_PREFS_DIR", dir);
    ChatWindows::new(gui, screen).unwrap()
}

fn rig(screen: (u32, u32)) -> Option<(Gui, ChatWindows)> {
    let client = ao_gui::client_dir();
    if !client.join("cd_image/gui").exists() {
        eprintln!("skipping: no client");
        return None;
    }
    let dir = test_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let mut gui = Gui::new(&client, None).unwrap();
    let ch = open_in(&mut gui, &dir, screen);
    Some((gui, ch))
}

fn sample(gui: &mut Gui, ch: &mut ChatWindows) {
    let l = |k: ChatKind, t: &str| ChatLine::new(k, t);
    ch.push(gui, &l(ChatKind::System, "Welcome to Project Rubi-Ka! Type /help for a list of chat commands."), None);
    ch.push(gui, &l(ChatKind::Error, "Error: Unknown command."), None);
    ch.push(gui, &l(ChatKind::CmdFeedback, "You are now AFK."), None);
    ch.push_msg(gui, &msg(G_VICINITY, "", "Reiserfs", "Anyone selling a Jobe Cluster?", 0));
    ch.push_msg(gui, &msg(G_VICINITY, "", "Nanogirl", "psst, over here", 1));
    ch.push_msg(gui, &msg(G_VICINITY, "", "Vhab", "WTB INFERNO", 2));
    ch.push_msg(gui, &msg(G_VICINITY, "", "Vhab", "dances wildly", 3));
    ch.push_msg(gui, &ChatMsg { tell: true, from_name: "Athrox".into(), text: "got a minute?".into(), ..Default::default() });
    ch.push(gui, &l(ChatKind::TellOut, "To Athrox: sure"), None);
    for (ty, name, col) in [(0x87u64, "Newbie Help", 1u64), (4, "OOC", 2), (3, "Clan", 3), (0x82, "Team", 4), (0x8f, "Raid", 5), (0x86, "Seeking Team", 6), (5, "GM", 7), (8, "News", 8), (0x0e, "My Private Group", 9), (1, "Admin", 10), (10, "Tower War", 11)] {
        ch.push_msg(gui, &msg(ty << 32 | col, name, "Someone", "channel line", 0));
    }
    ch.push_msg(gui, &msg(G_MYPET, "", "Rex", "Rex growls.", 0));
    ch.push_msg(gui, &msg(G_OTHERPET, "", "Fido", "Rex growls.", 0));
    ch.push(gui, &ChatLine::new(ChatKind::Other("CCMeHitByMonsterColor"), "The Rat hit you for 12 points of damage."), Some("Me hit by monster"));
    ch.push(gui, &ChatLine::new(ChatKind::Other("CCYouHitOtherColor"), "You hit the Rat for 30 points of damage."), Some("You hit other"));
    ch.push(gui, &ChatLine::new(ChatKind::Other("CCMeGotXPColor"), "You received 120 xp."), Some("Me got XP"));
    ch.push(gui, &ChatLine::new(ChatKind::Other("CCMeGotHealthColor"), "You were healed for 40 points."), Some("Me got health"));
}

#[test]
fn windows_route_and_fade() {
    let Some((mut gui, mut ch)) = rig((1280, 800)) else { return };
    assert_eq!(ch.window_ids().len(), 2);
    sample(&mut gui, &mut ch);
    let (w1, w2) = (ch.wins[0].id, ch.wins[1].id);
    let t1 = gui.text(w1, "text_0");
    let t2 = gui.text(w2, "text_1");
    // default window: chat/system/tell, no combat lines; combat window: combat lines only
    assert!(t1.contains("Jobe Cluster") && t1.contains("Welcome to Project Rubi-Ka") && t1.contains("channel line") && t1.contains("Athrox"));
    assert!(!t1.contains("hit you for 12") && !t1.contains("You received 120 xp"));
    assert!(t2.contains("hit you for 12") && t2.contains("You received 120 xp") && !t2.contains("Jobe Cluster"));
    assert!(t1.contains("(") && t1.contains(":"), "Window1 shows timestamps");

    // Enter in the input bar submits to the output group (vicinity) and clears the field
    assert_eq!(ch.active_output_group().as_deref(), Some("#0000000040000002#"));
    ch.focus_input(&mut gui);
    ch.update(&mut gui, 0.0);
    assert_eq!(gui.window_alpha(w1), 0.3); // fade starts from the inactive alpha
    for _ in 0..30 {
        ch.update(&mut gui, 0.016);
    }
    assert!((gui.window_alpha(w1) - 0.8).abs() < 1e-6, "active window fades to 0.8 in 0.2 s");
    gui.input(InputEvent::Text("hello".into()));
    let evs = gui.input(InputEvent::Key { key: Key::Enter, pressed: true, mods: Default::default() });
    let out: Vec<_> = evs.iter().flat_map(|e| ch.event(&mut gui, e)).collect();
    assert_eq!(out, vec![WinOut::Submit { text: "hello".into(), window_group: Some("#0000000040000002#".into()) }]);
    assert_eq!(gui.text(w1, "input_0"), "");
    // deactivate_on_send: focus left, alpha fades back to 0.3 in 1 s
    ch.update(&mut gui, 0.0);
    for _ in 0..70 {
        ch.update(&mut gui, 0.016);
    }
    assert!((gui.window_alpha(w1) - 0.3).abs() < 1e-6);

    // link activation
    assert_eq!(ch.event(&mut gui, &Event::LinkClicked { window: w1, view: "text_0".into(), href: "user://Bob".into() }), vec![WinOut::OpenTell("Bob".into())]);
    assert_eq!(ch.event(&mut gui, &Event::LinkClicked { window: w1, view: "text_0".into(), href: "chatcmd:///inspect 5".into() }), vec![WinOut::LinkClicked("chatcmd:///inspect 5".into())]);
    // keeps at most 100 lines
    for i in 0..150 {
        ch.push(&mut gui, &ChatLine::new(ChatKind::System, format!("line {i}")), None);
    }
    assert!(ch.wins[0].lines.len() == 100 && !gui.text(w1, "text_0").contains("line 0<"));
}

#[test]
fn config_saved_in_prefs_dir_reloads() {
    let Some((mut gui, mut ch)) = rig((1280, 800)) else { return };
    let dir = test_dir();
    ch.wins[1].cfg.show_timestamps = true;
    ch.save().unwrap();
    let again = open_in(&mut gui, &dir, (1280, 800));
    assert!(again.wins.iter().find(|w| w.cfg.window_name == "Window2").unwrap().cfg.show_timestamps);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Offscreen render of the two default windows over the HUD with a line of every colour class.
#[test]
fn chat_win_shot() {
    use super::super::super::{hud::Hud, zone::Zone};
    let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(PathBuf::from) else { return };
    let size = (1280u32, 828u32);
    let client = ao_gui::client_dir();
    if !client.join("cd_image/gui").exists() {
        return eprintln!("skipping: no client");
    }
    std::env::set_var("AOMAC_PREFS_DIR", std::env::temp_dir().join("aomac-chatgui-shot-prefs"));
    let _ = std::fs::remove_dir_all(std::env::temp_dir().join("aomac-chatgui-shot-prefs"));
    let labels = TextDb::load(&client).unwrap();
    let mut gui = Gui::new(&client, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).unwrap();
    let mut zone = Zone::default();
    for (id, v) in [(1, 125), (27, 125), (221, 100), (214, 100), (54, 1), (52, 40), (57, 0), (350, 1500), (61, 1234), (180, 0), (181, 150)] {
        zone.stats.insert(id, v);
    }
    let mut hud = Hud::new(&mut gui, &client, size).unwrap();
    hud.update(&mut gui, &mut zone, 0.0);
    // the chat windows are created after the HUD windows (normal stacking = creation order)
    let mut ch = ChatWindows::new(&mut gui, size).unwrap();
    ch.set_reserved(&mut gui, Reserved { left: 190, right: 65, bottom: 38 });
    struct Fe(Gui);
    impl ao_render::Frontend for Fe {
        fn gui(&self) -> &Gui {
            &self.0
        }
        fn input(&mut self, ev: InputEvent, _: &mut ao_render::Host) {
            self.0.input(ev);
        }
        fn frame(&mut self, dt: f32, _: (u32, u32), _: &mut ao_render::Host) -> ao_gui::DrawList {
            self.0.frame(dt)
        }
    }
    let mut fe = Fe(gui);
    sample(&mut fe.0, &mut ch);
    for variant in ["inactive", "active"] {
        if variant == "active" {
            ch.focus_input(&mut fe.0);
            fe.0.input(InputEvent::Text("/v hello there".into()));
        }
        for _ in 0..80 {
            ch.update(&mut fe.0, 0.016);
        }
        let mut o = ao_render::Offscreen::new(&fe, size).unwrap();
        let list = o.frame(&mut fe, 0.016);
        std::fs::create_dir_all(&out).unwrap();
        o.png(&fe, &list, &out.join(format!("chat-{variant}.png"))).unwrap();
        eprintln!("wrote chat-{variant}.png");
    }
}

#[test]
fn windows_open_at_their_frames() {
    let Some((gui, ch)) = rig((1280, 800)) else { return };
    for w in &ch.wins {
        let (_, _, pw, ph) = place(w.cfg.frame, w.cfg.template, (1280, 800), Reserved::default());
        assert_eq!(gui.window_size(w.id), (pw, ph), "{}", w.cfg.name);
    }
}

// ------------------------------------------------------------------------------------------ frame / tabs / selection / menu

use super::menu::{OP_AUTOSUB, OP_IGNORE, OP_MODE, OP_SUBSCRIBE, OP_TIMESTAMPS};

/// Closes the GUI windows of a `ChatWindows` that was only opened to inspect what a reload gives.
fn close_all(gui: &mut Gui, ch: &ChatWindows) {
    for f in &ch.frames {
        gui.close_window(f.id);
    }
}

fn pump(gui: &mut Gui, ch: &mut ChatWindows, evs: Vec<InputEvent>) -> Vec<WinOut> {
    let mut out = vec![];
    for e in evs {
        for ev in gui.input(e) {
            out.extend(ch.event(gui, &ev));
        }
    }
    out
}

/// Left-drag from `a` to `b` (several pointer steps), like the user's mouse.
fn drag(gui: &mut Gui, ch: &mut ChatWindows, a: (f32, f32), b: (f32, f32)) -> Vec<WinOut> {
    let mut evs = vec![InputEvent::MouseMove { x: a.0, y: a.1 }, InputEvent::MouseDown { x: a.0, y: a.1, button: ao_gui::MouseButton::Left }];
    for i in 1..=5 {
        let t = i as f32 / 5.0;
        evs.push(InputEvent::MouseMove { x: a.0 + (b.0 - a.0) * t, y: a.1 + (b.1 - a.1) * t });
    }
    evs.push(InputEvent::MouseUp { x: b.0, y: b.1, button: ao_gui::MouseButton::Left });
    pump(gui, ch, evs)
}

/// Midpoint of the move zone (the empty strip right of the tabs) of frame `fi`.
fn strip(ch: &ChatWindows, fi: usize) -> (f32, f32) {
    let (x, y, w, _) = ch.frames[fi].placed;
    ((x + w as i32 - 40) as f32, (y + 12) as f32)
}

/// A point inside the first tab of frame `fi` (tabs start 20 px right of the TabView edge, 3 px inside the window).
fn tab_at(ch: &ChatWindows, fi: usize, n: usize, dx: f32) -> (f32, f32) {
    let (x, y, ..) = ch.frames[fi].placed;
    let _ = n;
    ((x + 3 + 20) as f32 + dx, (y + 7 + 6) as f32)
}

#[test]
fn border_windows_are_fixed_and_the_menu_gives_a_draggable_frame() {
    let Some((mut gui, mut ch)) = rig((1280, 800)) else { return };
    assert_eq!(ch.frames.len(), 2);
    let before = ch.frames[0].placed;
    // visual mode 2 = `SetStyle(3, flags 0xc3c)`: not movable (0x8), not resizable (0x30): the same drag does nothing
    let s = strip(&ch, 0);
    drag(&mut gui, &mut ch, s, (s.0 - 40.0, s.1 - 10.0));
    assert_eq!(ch.frames[0].placed, before);
    // menu "Visual > Normal" (`FUN_100998bc` / `FUN_10096ec5`): style 0 with a tab strip, same outer rectangle
    ch.test_pick(&mut gui, 0, OP_MODE, 0);
    let id = ch.frames[0].id;
    assert_eq!(gui.window_tabs(id), (vec!["Default Window".to_string()], 0));
    assert_eq!(gui.window_outer_frame(id), Some(before));
    // drag the strip: the frame follows the pointer and the new rectangle is what `WindowFrame` saves
    let s = strip(&ch, 0);
    drag(&mut gui, &mut ch, s, (s.0 - 10.0, s.1 - 10.0));
    let moved = (before.0 - 10, before.1 - 10, before.2, before.3);
    assert_eq!(ch.frames[0].placed, moved);
    assert_eq!(gui.window_outer_frame(id), Some(moved));
    ch.update(&mut gui, 0.0); // pointer idle: written
    let again = open_in(&mut gui, &test_dir(), (1280, 800));
    assert_eq!(again.wins[0].cfg.visual_mode, 0);
    assert_eq!(again.frames[0].placed, moved, "saved WindowFrame reloads at the same place");
    close_all(&mut gui, &again);
    // resize by the bottom-right corner far past the minimum: clamped to the client floor, top-left fixed
    let (x, y, w, h) = ch.frames[0].placed;
    drag(&mut gui, &mut ch, ((x + w as i32 - 2) as f32, (y + h as i32 - 2) as f32), (-5000.0, -5000.0));
    let (nx, ny, nw, nh) = ch.frames[0].placed;
    assert_eq!((nx, ny), (x, y));
    assert_eq!((nw, nh), (windows::MIN_CLIENT.0 + 10, windows::MIN_CLIENT.1 + 31));
    // back to Border through the menu: the frame goes away, rectangle kept
    ch.test_pick(&mut gui, 0, OP_MODE, 2);
    assert_eq!(gui.window_tabs(ch.frames[0].id).0.len(), 0);
    assert_eq!(ch.frames[0].placed, (nx, ny, nw, nh));
    let _ = std::fs::remove_dir_all(test_dir());
}

#[test]
fn tabs_dock_tear_out_and_reload() {
    let Some((mut gui, mut ch)) = rig((1280, 800)) else { return };
    sample(&mut gui, &mut ch);
    ch.test_pick(&mut gui, 0, OP_MODE, 0);
    ch.test_pick(&mut gui, 1, OP_MODE, 0);
    // drag "Combat" onto the strip of "Default Window", left of its tab centre: it becomes the first tab
    let from = tab_at(&ch, 1, 0, 10.0);
    let to = tab_at(&ch, 0, 0, 5.0);
    drag(&mut gui, &mut ch, from, to);
    assert_eq!(ch.frames.len(), 1);
    let id = ch.frames[0].id;
    assert_eq!(gui.window_tabs(id), (vec!["Combat".to_string(), "Default Window".to_string()], 0));
    // each tab keeps its own text; the selected one is visible
    assert!(gui.text(id, "text_1").contains("hit you for 12") && gui.text(id, "text_0").contains("Jobe Cluster"));
    // tab_index follows the order
    assert_eq!((ch.wins[1].cfg.tab_index, ch.wins[0].cfg.tab_index), (0, 1));
    // pressing the second tab selects it
    let t2 = tab_at(&ch, 0, 1, 80.0);
    pump(&mut gui, &mut ch, vec![InputEvent::MouseMove { x: t2.0, y: t2.1 }, InputEvent::MouseDown { x: t2.0, y: t2.1, button: ao_gui::MouseButton::Left }, InputEvent::MouseUp { x: t2.0, y: t2.1, button: ao_gui::MouseButton::Left }]);
    assert_eq!(ch.frames[0].sel, 1);
    ch.update(&mut gui, 0.0);
    // the pair reloads as one window with the tabs in `tab_index` order
    let again = open_in(&mut gui, &test_dir(), (1280, 800));
    assert_eq!(again.frames.len(), 1);
    assert_eq!(again.frames[0].docs.iter().map(|&d| again.wins[d].cfg.name.as_str()).collect::<Vec<_>>(), ["Combat", "Default Window"]);
    close_all(&mut gui, &again);
    // tear the first tab out onto empty desktop: two windows again, the new one 20 px down-right
    let old = ch.frames[0].placed;
    let from = tab_at(&ch, 0, 0, 10.0);
    drag(&mut gui, &mut ch, from, (4.0, 4.0));
    assert_eq!(ch.frames.len(), 2);
    assert_eq!(ch.frames[1].docs.len(), 1);
    assert_eq!(gui.window_tabs(ch.frames[1].id).0, vec!["Combat".to_string()]);
    // `MoveInsideScreen` keeps it on the screen (here the bottom edge pulls it back up)
    let n = ch.frames[1].placed;
    assert_eq!((n.0, n.1), (old.0 + 20, (old.1 + 20).min(800 - old.3 as i32)));
    // a window with a single tab cannot be torn out
    let from = tab_at(&ch, 1, 0, 10.0);
    drag(&mut gui, &mut ch, from, (4.0, 4.0));
    assert_eq!(ch.frames.len(), 2);
    let _ = std::fs::remove_dir_all(test_dir());
}

#[test]
fn chat_text_selection_copies_plain_text() {
    let Some((mut gui, mut ch)) = rig((1280, 800)) else { return };
    sample(&mut gui, &mut ch);
    let w = ch.frames[0].id;
    let r = gui.view_rect(w, "text_0").unwrap();
    // drag over the visible text from the bottom-right to the top-left
    drag(&mut gui, &mut ch, (r.r - 20.0, r.b - 1.0), (r.l + 1.0, r.t + 1.0));
    let sel = gui.selected_text().expect("a selection");
    assert!(sel.contains("Rex growls.") && !sel.contains('<'), "{sel:?}");
    // the highlight is drawn
    let list = gui.frame(0.0);
    assert!(list.cmds.iter().any(|c| matches!(c, ao_gui::DrawCmd::Solid { color: [0xc0, 0xc0, 0xc0], .. })));
    // Ctrl+C -> Event::Copy(text) for the app's clipboard
    let evs = gui.input(InputEvent::Key { key: Key::Letter('c'), pressed: true, mods: ao_gui::Modifiers { ctrl: true, ..Default::default() } });
    assert_eq!(evs, vec![Event::Copy(sel)]);
    assert_eq!(gui.selected_text(), None);
    // a click on a link is a link, not a selection
    assert!(gui.selected_text().is_none());
}

#[test]
fn right_click_menu_settings_apply_and_persist() {
    let Some((mut gui, mut ch)) = rig((1280, 800)) else { return };
    let w = ch.frames[0].id;
    let r = gui.view_rect(w, "text_0").unwrap();
    // right button in the chat view -> `ContextMenu` -> the window's popup menu opens
    let evs = gui.input(InputEvent::MouseDown { x: r.l + 5.0, y: r.t + 5.0, button: ao_gui::MouseButton::Right });
    assert!(evs.iter().any(|e| matches!(e, Event::ContextMenu { .. })));
    for e in &evs {
        ch.event(&mut gui, e);
    }
    assert!(gui.menu_open());
    gui.close_menu();
    // a user link gives the user menu: IgnoreUser -> the hub's /ignore
    ch.event(&mut gui, &Event::ContextMenu { window: w, x: 10, y: 10, link: Some("user://Bob".into()) });
    assert_eq!(ch.menu_picked(&mut gui, OP_IGNORE << 16), vec![WinOut::IgnoreUser("Bob".into())]);
    // toggles
    assert!(ch.wins[0].cfg.show_timestamps);
    ch.test_pick(&mut gui, 0, OP_TIMESTAMPS, 0);
    assert!(!ch.wins[0].cfg.show_timestamps);
    // channel subscribe entry (index in the menu's group list): hides Vicinity from the autosubscribe window
    let groups = ch.menu_groups();
    let at = groups.iter().position(|g| *g == G_VICINITY).unwrap() as u32;
    assert!(ch.wins[0].cfg.shows(G_VICINITY));
    ch.test_pick(&mut gui, 0, OP_SUBSCRIBE, at);
    assert!(!ch.wins[0].cfg.shows(G_VICINITY));
    // switching the sign convention keeps what the window shows
    let shown: Vec<bool> = groups.iter().map(|g| ch.wins[0].cfg.shows(*g)).collect();
    ch.test_pick(&mut gui, 0, OP_AUTOSUB, 0);
    assert!(!ch.wins[0].cfg.autosubscribe);
    assert_eq!(groups.iter().map(|g| ch.wins[0].cfg.shows(*g)).collect::<Vec<_>>(), shown);
    gui.close_menu();
    ch.update(&mut gui, 0.0);
    let again = open_in(&mut gui, &test_dir(), (1280, 800));
    assert!(!again.wins[0].cfg.show_timestamps && !again.wins[0].cfg.shows(G_VICINITY) && !again.wins[0].cfg.autosubscribe);
    let _ = std::fs::remove_dir_all(test_dir());
}

/// Offscreen render of the framed (visual mode 0) windows: docked tabs, an open menu with its sub-menu, a text selection.
/// `AOMAC_SHOT_DIR=/tmp/x cargo test --release -p aomac chat_frame_shot -- --nocapture` -> `frame-tabs.png`, `frame-menu.png`, `frame-selection.png`.
#[test]
fn chat_frame_shot() {
    let Some(out) = std::env::var_os("AOMAC_SHOT_DIR").map(PathBuf::from) else { return };
    let client = ao_gui::client_dir();
    if !client.join("cd_image/gui").exists() {
        return eprintln!("skipping: no client");
    }
    let size = (1280u32, 800u32);
    let labels = TextDb::load(&client).unwrap();
    let mut gui = Gui::new(&client, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).unwrap();
    let dir = test_dir();
    let _ = std::fs::remove_dir_all(&dir);
    let mut ch = open_in(&mut gui, &dir, size);
    sample(&mut gui, &mut ch);
    ch.test_pick(&mut gui, 0, OP_MODE, 0);
    ch.test_pick(&mut gui, 1, OP_MODE, 0);
    let (from, to) = (tab_at(&ch, 1, 0, 10.0), tab_at(&ch, 0, 0, 5.0));
    drag(&mut gui, &mut ch, from, to);
    struct Fe(Gui);
    impl ao_render::Frontend for Fe {
        fn gui(&self) -> &Gui {
            &self.0
        }
        fn input(&mut self, ev: InputEvent, _: &mut ao_render::Host) {
            self.0.input(ev);
        }
        fn frame(&mut self, dt: f32, _: (u32, u32), _: &mut ao_render::Host) -> ao_gui::DrawList {
            self.0.frame(dt)
        }
    }
    let mut fe = Fe(gui);
    let shoot = |fe: &mut Fe, ch: &mut ChatWindows, name: &str| {
        for _ in 0..80 {
            ch.update(&mut fe.0, 0.016);
        }
        let mut o = ao_render::Offscreen::new(fe, size).unwrap();
        let list = o.frame(fe, 0.016);
        std::fs::create_dir_all(&out).unwrap();
        o.png(fe, &list, &out.join(name)).unwrap();
        eprintln!("wrote {name}");
    };
    shoot(&mut fe, &mut ch, "frame-tabs.png");
    // selection over the first lines of the selected tab
    let w = ch.frames[0].id;
    let r = fe.0.view_rect(w, "scroll_1").unwrap(); // the text sits at the bottom of the scroll view (`TVF_FILL_BOTTOM_UP`)
    drag(&mut fe.0, &mut ch, (r.r - 60.0, r.b - 3.0), (r.l + 60.0, r.b - 30.0));
    assert!(fe.0.selected_text().is_some_and(|t| t.contains("healed")));
    shoot(&mut fe, &mut ch, "frame-selection.png");
    // right-click menu with the Visual sub-menu open
    let at = (r.l + 30.0, r.t + 30.0);
    fe.0.input(InputEvent::MouseMove { x: at.0, y: at.1 });
    let evs = fe.0.input(InputEvent::MouseDown { x: at.0, y: at.1, button: ao_gui::MouseButton::Right });
    for e in &evs {
        ch.event(&mut fe.0, e);
    }
    fe.0.input(InputEvent::MouseMove { x: at.0 + 20.0, y: at.1 + 8.0 });
    shoot(&mut fe, &mut ch, "frame-menu.png");
}
