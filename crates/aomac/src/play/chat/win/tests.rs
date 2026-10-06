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

fn rig(screen: (u32, u32)) -> Option<(Gui, ChatWindows)> {
    let client = ao_gui::client_dir();
    if !client.join("cd_image/gui").exists() {
        eprintln!("skipping: no client");
        return None;
    }
    std::env::set_var("AOMAC_PREFS_DIR", std::env::temp_dir().join("aomac-chatgui-test-prefs"));
    let mut gui = Gui::new(&client, None).unwrap();
    let ch = ChatWindows::new(&mut gui, screen).unwrap();
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
    ch.push_msg(gui, &msg(G_OTHERPET, "", "Fido", "Fido barks.", 0));
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
    let dir = std::env::temp_dir().join("aomac-chatgui-test-prefs");
    let _ = std::fs::remove_dir_all(&dir);
    ch.wins[1].cfg.show_timestamps = true;
    ch.save().unwrap();
    let again = ChatWindows::new(&mut gui, (1280, 800)).unwrap();
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
