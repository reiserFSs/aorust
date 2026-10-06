//! Style-0 frame interaction (move / resize / tabs), popup menu and read-only text selection (docs/chat/gui.md §10-12). Skipped without a client install.
use ao_gui::{Event, Gui, InputEvent, MenuItem, MouseButton, WindowSize};

const XML: &str = r#"<root><View view_layout="vertical"><ScrollView name="s" v_scrollbar_mode="always" max_size="Point(16000,16000)"><ScrollViewChild view_layout="vertical" max_size="Point(16000,16000)"><TextView name="t" max_size="Point(16000,-1)" font="CHAT" feature_flags="TVF_WORD_WRAP|TVF_MULTILINE|TVF_ALLOW_TEXT_SELECTION|TVF_ACCEPT_MOUSE_INPUT"/></ScrollViewChild></ScrollView></View></root>"#;

fn gui() -> Option<Gui> {
    Gui::new(&ao_gui::client_dir(), None).ok()
}

fn press(g: &mut Gui, x: f32, y: f32) -> Vec<Event> {
    g.input(InputEvent::MouseMove { x, y });
    g.input(InputEvent::MouseDown { x, y, button: MouseButton::Left })
}
fn drag_to(g: &mut Gui, x: f32, y: f32) -> Vec<Event> {
    g.input(InputEvent::MouseMove { x, y })
}
fn release(g: &mut Gui, x: f32, y: f32) -> Vec<Event> {
    g.input(InputEvent::MouseUp { x, y, button: MouseButton::Left })
}

fn framed(g: &mut Gui, pos: (i32, i32), size: (u32, u32)) -> usize {
    let w = g.open_tabbed_window_xml("t", "Chat", XML, pos, WindowSize::Fixed(size.0, size.1)).unwrap();
    g.set_window_frame(w, true, true);
    w
}

#[test]
fn framed_root_text_right_click_captures_before_context_dispatch() {
    let Some(mut g) = gui() else { return };
    // ChatView is a root TextView: unlike child content it must not rely on covers().
    let xml = r#"<root><TextView name="chat" font="CHAT" max_size="Point(16000,16000)" feature_flags="TVF_WORD_WRAP|TVF_MULTILINE|TVF_DISABLE_RC_MENU"/></root>"#;
    let w = g.open_tabbed_window_xml("chat", "Chat", xml, (314, 187), WindowSize::Fixed(430, 203)).unwrap();
    g.set_window_context(w, true);
    for (x, y) in [(596.0, 384.0), (315.0, 419.0)] {
        g.input(InputEvent::MouseMove { x, y });
        assert!(g.wants_mouse(x, y), "capture must precede Play's world mouse routing");
        let events = g.input(InputEvent::MouseDown { x, y, button: MouseButton::Right });
        assert!(events.iter().any(|e| matches!(e, Event::ContextMenu { window, .. } if *window == w)));
        g.input(InputEvent::MouseUp { x, y, button: MouseButton::Right });
        assert!(g.wants_mouse(x, y));
    }
    assert!(!g.wants_mouse(800.0, 500.0));
    assert!(g.wants_mouse(344.0, 197.0), "title frame also captures world input");
    g.set_window_frame(w, true, true);
    press(&mut g, 344.0, 197.0);
    assert!(g.wants_mouse(1000.0, 700.0), "frame drag keeps capture outside its rectangle");
    release(&mut g, 1000.0, 700.0);
    assert!(!g.wants_mouse(1000.0, 700.0));
    g.set_window_visible(w, false);
    assert!(!g.wants_mouse(596.0, 384.0));
    // Empty borderless roots remain transparent to world input.
    g.open_window_xml("overlay", "<root><View/></root>", (0, 0), WindowSize::Fixed(1280, 800)).unwrap();
    assert!(!g.wants_mouse(596.0, 384.0));
}

#[test]
fn drag_the_strip_moves_and_resizing_clamps() {
    let Some(mut g) = gui() else { return };
    let w = framed(&mut g, (100, 100), (300, 150));
    assert_eq!(g.window_outer_frame(w), Some((100, 100, 310, 181)));
    // strip row 12 right of the tab: moves by the drag delta
    let evs = press(&mut g, 300.0, 112.0);
    assert!(evs.is_empty());
    let evs = drag_to(&mut g, 340.0, 92.0);
    assert!(evs.contains(&Event::WindowFrame { window: w }));
    release(&mut g, 340.0, 92.0);
    assert_eq!(g.window_outer_frame(w), Some((140, 80, 310, 181)));
    // bottom-right corner grows the window, the top-left stays
    press(&mut g, 449.0, 260.0);
    drag_to(&mut g, 479.0, 290.0);
    release(&mut g, 479.0, 290.0);
    assert_eq!(g.window_outer_frame(w), Some((140, 80, 340, 211)));
    // limits: the client may not get smaller than 100x60 nor bigger than 400x300
    g.set_window_size_limits(w, (100, 60), (400, 300));
    press(&mut g, 479.0, 290.0);
    drag_to(&mut g, 0.0, 0.0);
    release(&mut g, 0.0, 0.0);
    assert_eq!(g.window_outer_frame(w), Some((140, 80, 110, 91)));
    press(&mut g, 249.0, 170.0);
    drag_to(&mut g, 2000.0, 2000.0);
    release(&mut g, 2000.0, 2000.0);
    assert_eq!(g.window_outer_frame(w), Some((140, 80, 410, 331)));
    // a window that is not movable / resizable ignores the same drags
    g.set_window_frame(w, false, false);
    press(&mut g, 300.0, 92.0);
    drag_to(&mut g, 400.0, 192.0);
    release(&mut g, 400.0, 192.0);
    assert_eq!(g.window_outer_frame(w), Some((140, 80, 410, 331)));
}

#[test]
fn tabs_select_and_drop_onto_another_strip() {
    let Some(mut g) = gui() else { return };
    let a = framed(&mut g, (10, 10), (300, 150));
    let b = framed(&mut g, (10, 300), (300, 150));
    g.set_window_tabs(a, &["One".into(), "Two".into()], 0);
    g.set_window_tabs(b, &["Three".into()], 0);
    // the second tab of window a: tabs start 20 px right of the TabView edge (outer x + 3)
    let tab2_x = (10 + 3 + 20) as f32 + 60.0;
    let evs = press(&mut g, tab2_x, 10.0 + 7.0 + 6.0);
    assert!(evs.contains(&Event::TabSelected { window: a, index: 1 }));
    assert_eq!(g.window_tabs(a).1, 1);
    // drag it onto the strip of window b and release: dropped there at index 0 (left of Three's centre)
    drag_to(&mut g, 40.0, 300.0 + 13.0);
    let evs = release(&mut g, 40.0, 300.0 + 13.0);
    assert_eq!(evs, vec![Event::TabDropped { window: a, tab: 1, x: 40, y: 313, target: Some((b, 0)) }]);
    // dropped on empty desktop: no target (the app tears the tab out)
    press(&mut g, tab2_x, 10.0 + 7.0 + 6.0);
    drag_to(&mut g, 700.0, 500.0);
    let evs = release(&mut g, 700.0, 500.0);
    assert_eq!(evs, vec![Event::TabDropped { window: a, tab: 1, x: 700, y: 500, target: None }]);
    // a plain click does not drop anything
    press(&mut g, tab2_x, 10.0 + 7.0 + 6.0);
    assert!(release(&mut g, tab2_x, 10.0 + 7.0 + 6.0).is_empty());
}

#[test]
fn popup_menu_picks_checks_and_closes() {
    let Some(mut g) = gui() else { return };
    g.open_menu(
        (50, 50),
        (800, 600),
        vec![MenuItem::submenu("Mode", vec![MenuItem::entry(1, "Normal"), MenuItem::entry(2, "Border")]), MenuItem::separator(), MenuItem::check(3, "Timestamps", true), MenuItem::entry(4, "Disabled").disabled()],
    );
    assert!(g.menu_open());
    // hover the sub-menu item: its panel opens to the right; click an entry there
    g.input(InputEvent::MouseMove { x: 60.0, y: 60.0 });
    let f = g.frame(0.0);
    assert!(!f.cmds.is_empty());
    let evs = g.input(InputEvent::MouseDown { x: 150.0, y: 60.0, button: MouseButton::Left });
    assert!(evs.is_empty() || matches!(evs[0], Event::MenuPicked { .. }));
    g.close_menu();
    g.open_menu((50, 50), (800, 600), vec![MenuItem::check(3, "Timestamps", true), MenuItem::entry(4, "Disabled").disabled()]);
    // the disabled item does nothing, the check item is picked
    assert!(g.input(InputEvent::MouseDown { x: 60.0, y: 70.0, button: MouseButton::Left }).is_empty());
    assert!(g.menu_open());
    let evs = g.input(InputEvent::MouseDown { x: 60.0, y: 53.0, button: MouseButton::Left });
    assert_eq!(evs, vec![Event::MenuPicked { id: 3 }]);
    assert!(!g.menu_open());
    // a click outside closes without picking and without reaching the windows underneath
    g.open_menu((50, 50), (800, 600), vec![MenuItem::entry(9, "X")]);
    assert!(g.input(InputEvent::MouseDown { x: 700.0, y: 500.0, button: MouseButton::Left }).is_empty());
    assert!(!g.menu_open());
}

#[test]
fn selection_extracts_text_and_copy_clears_it() {
    let Some(mut g) = gui() else { return };
    let w = g.open_window_xml("t", XML, (10, 10), WindowSize::Fixed(300, 120)).unwrap();
    g.set_text(w, "t", "alpha beta<br>gamma delta");
    let r = g.view_rect(w, "t").unwrap();
    g.frame(0.0);
    // nothing selected yet
    assert_eq!(g.selected_text(), None);
    // drag from the start of the text to the end of the second line
    press(&mut g, r.l + 1.0, r.t + 1.0);
    drag_to(&mut g, r.l + 290.0, r.t + 40.0);
    release(&mut g, r.l + 290.0, r.t + 40.0);
    assert_eq!(g.selected_text().as_deref(), Some("alpha beta\ngamma delta"));
    // the highlight (0xc0c0c0 solids) is part of the draw list
    let list = g.frame(0.0);
    assert!(list.cmds.iter().any(|c| matches!(c, ao_gui::DrawCmd::Solid { color: [0xc0, 0xc0, 0xc0], .. })));
    // Ctrl+C copies the text and clears the selection
    let evs = g.input(InputEvent::Key { key: ao_gui::Key::Letter('c'), pressed: true, mods: ao_gui::Modifiers { ctrl: true, ..Default::default() } });
    assert_eq!(evs, vec![Event::Copy("alpha beta\ngamma delta".into())]);
    assert_eq!(g.selected_text(), None);
    // a click elsewhere drops a selection; a partial drag selects only the covered characters
    press(&mut g, r.l + 1.0, r.t + 1.0);
    drag_to(&mut g, r.l + 30.0, r.t + 3.0);
    release(&mut g, r.l + 30.0, r.t + 3.0);
    let part = g.selected_text().unwrap();
    assert!(part.starts_with('a') && part.len() < 10, "{part:?}");
    press(&mut g, r.l + 290.0, r.b + 40.0);
    assert_eq!(g.selected_text(), None);
}

#[test]
fn popup_slider_item_reports_values_and_stays_open() {
    let Some(mut g) = gui() else { return };
    g.open_menu((50, 50), (800, 600), vec![MenuItem::slider(7, 0.0)]);
    // press in the middle of the slider row, drag to the far right, release: values 0..1, menu still open
    let evs = g.input(InputEvent::MouseDown { x: 110.0, y: 58.0, button: MouseButton::Left });
    let v = match evs.as_slice() {
        [Event::MenuSlider { id: 7, value }] => *value,
        other => panic!("{other:?}"),
    };
    assert!((0.3..0.7).contains(&v), "{v}");
    let evs = g.input(InputEvent::MouseMove { x: 400.0, y: 58.0 });
    assert_eq!(evs, vec![Event::MenuSlider { id: 7, value: 1.0 }]);
    assert!(g.input(InputEvent::MouseUp { x: 400.0, y: 58.0, button: MouseButton::Left }).is_empty());
    assert!(g.menu_open());
}

#[test]
fn tab_titles_are_html_and_size_by_their_visible_text() {
    let Some(mut g) = gui() else { return };
    // `FUN_100ab980`: name + ` <font color=green>[group]</font>`; the markup adds no width
    let plain = g.tab_title_width("Default Window [Clan OOC]");
    assert_eq!(g.tab_title_width("Default Window <font color=green>[Clan OOC]</font>"), plain);
    assert!(plain > g.tab_title_width("Default Window"));
}

#[test]
fn chat_font_follows_the_prefs_and_unknown_faces_are_refused() {
    let Some(mut g) = gui() else { return };
    let base = g.font_height(ao_gui::FontId::Chat);
    assert!(g.set_chat_font("Verdana", "Bold", 200));
    assert!(g.font_height(ao_gui::FontId::Chat) > base);
    assert!(!g.set_chat_font("No Such Face", "Regular", 140), "unknown face keeps the current font");
    assert!(g.set_chat_font("Verdana", "Regular", 140));
    assert_eq!(g.font_height(ao_gui::FontId::Chat), base);
}

#[test]
fn empty_input_shows_the_hint_until_text_is_typed() {
    let Some(mut g) = gui() else { return };
    let xml = r#"<root><View view_layout="vertical"><TextView name="i" font="CHAT" max_size="Point(16000,-1)" feature_flags="TVF_ACCEPT_TXT_INPUT"/></View></root>"#;
    let w = g.open_window_xml("t", xml, (10, 10), WindowSize::Fixed(200, 30)).unwrap();
    let glyphs = |g: &mut Gui| g.frame(0.0).cmds.iter().filter(|c| matches!(c, ao_gui::DrawCmd::Glyph { .. })).count();
    assert_eq!(glyphs(&mut g), 0);
    g.set_text_hint(w, "i", "Clan OOC");
    assert_eq!(glyphs(&mut g), 7, "\"Clan OOC\" without the space");
    g.set_text(w, "i", "x");
    assert_eq!(glyphs(&mut g), 1, "typed text replaces the prompt");
}
