//! `DropdownMenu`, `StringListView` (`ListViewBase_c`) and `MultiListView` (list mode) driven by real mouse events (docs/gui.md §13); skipped without a client install.
use ao_gui::view::{ListItem, MultiCell};
use ao_gui::{Event, Gui, InputEvent, MouseButton, WindowId, WindowSize};

const XML: &str = r#"<root><View view_layout="vertical">
  <DropdownMenu name="dd" layout_borders="Rect(5,3,0,0)"/>
  <StringListView name="fl" v_scrollbar_mode="auto" h_scrollbar_mode="auto" min_size="Point(120,100)" max_size="Point(16000,100)"/>
  <MultiListView name="ml" feature_flags="64" min_size="Point(150,100)" max_size="Point(16000,16000)"/>
</View></root>"#;

fn rig() -> Option<(Gui, WindowId)> {
    let mut g = Gui::new(&ao_gui::client_dir(), None).ok()?;
    g.set_screen_size(800, 600);
    let w = g.open_window_xml("t", XML, (10, 10), WindowSize::Fixed(300, 400)).ok()?;
    Some((g, w))
}

fn press(g: &mut Gui, x: f32, y: f32, b: MouseButton) -> Vec<Event> {
    g.input(InputEvent::MouseMove { x, y });
    let mut ev = g.input(InputEvent::MouseDown { x, y, button: b });
    ev.extend(g.input(InputEvent::MouseUp { x, y, button: b }));
    ev
}

fn click(g: &mut Gui, x: f32, y: f32) -> Vec<Event> {
    press(g, x, y, MouseButton::Left)
}

#[test]
fn dropdown_opens_a_popup_and_selects_by_index_with_ids() {
    let Some((mut g, w)) = rig() else { return };
    // inserting at an id as index, the first insert selects item 0 and raises the signal
    g.dropdown_insert(w, "dd", 0, 7, "neutral");
    g.dropdown_insert(w, "dd", 1, 3, "clan");
    g.dropdown_insert(w, "dd", 2, 5, "omni");
    assert_eq!(g.dropdown_selected(w, "dd"), Some(0));
    assert_eq!(g.dropdown_text(w, "dd"), "neutral");
    let r = g.view_rect(w, "dd").unwrap();
    // the label is as wide as the widest item (+ the 6 px client borders) and the arrow is the 26 px art
    let label_w = g.text_width(ao_gui::FontId::Normal, "neutral") as f32;
    assert_eq!((r.r - r.l + 1.0), label_w + 12.0 + 1.0 + 26.0, "{r:?}");
    assert_eq!(r.b - r.t + 1.0, 22.0);
    // a press opens the menu below (left + 5, bottom + 1); picking the 2nd entry selects it
    assert!(click(&mut g, r.l + 10.0, r.t + 10.0).is_empty());
    assert!(g.menu_open());
    let (ax, ay) = (r.l + 5.0, r.b + 1.0);
    let ev = click(&mut g, ax + 12.0, ay + 2.0 + 15.0 + 7.0);
    assert!(!g.menu_open());
    assert!(ev.iter().any(|e| matches!(e, Event::DropdownChanged { index: 1, id: 3, view, .. } if view == "dd")), "{ev:?}");
    assert_eq!(g.dropdown_selected_id(w, "dd"), Some(3));
    assert_eq!(g.dropdown_text(w, "dd"), "clan");
    // a second press while open closes it without a change
    click(&mut g, r.l + 10.0, r.t + 10.0);
    assert!(g.menu_open());
    let ev = click(&mut g, r.l + 10.0, r.t + 10.0);
    assert!(!g.menu_open() && ev.is_empty());
    // programmatic selection by id
    g.dropdown_select_id(w, "dd", 5, false);
    assert_eq!(g.dropdown_text(w, "dd"), "omni");
    // deleting the selected last item selects the one before it (`DeleteItem`)
    g.dropdown_delete(w, "dd", 2);
    assert_eq!(g.dropdown_selected(w, "dd"), Some(1));
    g.dropdown_clear(w, "dd");
    assert_eq!(g.dropdown_selected(w, "dd"), None);
    let r2 = g.view_rect(w, "dd").unwrap();
    assert_eq!(r2.r - r2.l + 1.0, 70.0 + 12.0 + 1.0 + 26.0, "empty width is 70");
}

#[test]
fn disabled_dropdown_ignores_presses() {
    let Some((mut g, w)) = rig() else { return };
    g.dropdown_append(w, "dd", 1, "a");
    g.set_enabled(w, "dd", false);
    let r = g.view_rect(w, "dd").unwrap();
    click(&mut g, r.l + 5.0, r.t + 5.0);
    assert!(!g.menu_open());
}

#[test]
fn list_folders_select_and_double_click() {
    let Some((mut g, w)) = rig() else { return };
    let mut folder = ListItem::new("online", "Online (2)", 0xd5, 0xd4);
    folder.folder = true;
    folder.selectable = false;
    assert!(g.list_add(w, "fl", None, folder));
    assert!(g.list_add(w, "fl", Some("online"), ListItem::new("5", "Bob", 0, 0)));
    assert!(g.list_add(w, "fl", Some("online"), ListItem::new("6", "Eve", 0, 0)));
    assert!(!g.list_add(w, "fl", Some("nope"), ListItem::new("9", "x", 0, 0)));
    let r = g.view_rect(w, "fl").unwrap();
    // the folder starts closed: only its row is there, the click opens it (icon 0xd5 while open) and raises the row signal
    let ev = click(&mut g, r.l + 20.0, r.t + 3.0);
    assert!(ev.iter().any(|e| matches!(e, Event::ListItemMouse { id, button: 1, clicks: 1, .. } if id == "online")), "{ev:?}");
    assert!(g.list_item(w, "fl", "online").unwrap().open);
    // rows are 13 px (12 text extent + 1), children indented by the folder's 15
    let ev = click(&mut g, r.l + 25.0, r.t + 13.0 + 3.0);
    assert!(ev.iter().any(|e| matches!(e, Event::ListSelected { id, selected: true, .. } if id == "5")), "{ev:?}");
    assert_eq!(g.list_selected(w, "fl"), ["5"]);
    // single selection: the next one deselects the first (with its signal)
    let ev = click(&mut g, r.l + 25.0, r.t + 26.0 + 3.0);
    assert!(ev.iter().any(|e| matches!(e, Event::ListSelected { id, selected: false, .. } if id == "5")));
    assert_eq!(g.list_selected(w, "fl"), ["6"]);
    // the second press on the same row is a double click, the right button reports button 2
    let ev = click(&mut g, r.l + 25.0, r.t + 26.0 + 3.0);
    assert!(ev.iter().any(|e| matches!(e, Event::ListItemMouse { clicks: 2, id, .. } if id == "6")));
    let ev = press(&mut g, r.l + 25.0, r.t + 13.0 + 3.0, MouseButton::Right);
    assert!(ev.iter().any(|e| matches!(e, Event::ListItemMouse { button: 2, id, .. } if id == "5")), "{ev:?}");
    // folders toggle shut again; closed children are not hit
    click(&mut g, r.l + 20.0, r.t + 3.0);
    assert!(!g.list_item(w, "fl", "online").unwrap().open);
    // multi select toggles
    g.list_clear(w, "fl");
    g.list_set_multi(w, "fl", true);
    g.list_add(w, "fl", None, ListItem::new("a", "A", 0, 0));
    g.list_add(w, "fl", None, ListItem::new("b", "B", 0, 0));
    click(&mut g, r.l + 5.0, r.t + 3.0);
    click(&mut g, r.l + 5.0, r.t + 16.0);
    assert_eq!(g.list_selected(w, "fl"), ["a", "b"]);
    click(&mut g, r.l + 5.0, r.t + 3.0);
    assert_eq!(g.list_selected(w, "fl"), ["b"]);
}

#[test]
fn list_scrolls_with_the_wheel_and_the_bar() {
    let Some((mut g, w)) = rig() else { return };
    for i in 0..30 {
        g.list_add(w, "fl", None, ListItem::new(&i.to_string(), &format!("row {i}"), 0, 0));
    }
    let r = g.view_rect(w, "fl").unwrap();
    assert_eq!(g.scroll_offset(w, "fl"), 0.0);
    g.input(InputEvent::MouseMove { x: r.l + 10.0, y: r.t + 10.0 });
    g.input(InputEvent::Wheel { x: r.l + 10.0, y: r.t + 10.0, dy: -1.0 });
    assert!(g.scroll_offset(w, "fl") > 0.0);
    // clicking after scrolling hits the row under the pointer (offset applied)
    let off = g.scroll_offset(w, "fl");
    let ev = click(&mut g, r.l + 10.0, r.t + 2.0);
    let row = ((off + 2.0) / 13.0) as usize;
    assert!(ev.iter().any(|e| matches!(e, Event::ListItemMouse { id, .. } if *id == row.to_string())), "{ev:?}");
}

#[test]
fn multi_list_sorts_by_header_selects_and_resizes() {
    let Some((mut g, w)) = rig() else { return };
    g.multi_add_column(w, "ml", 0, "Name", 60.0, 0xe);
    g.multi_add_column(w, "ml", 1, "Lvl", 30.0, 0xe);
    for (id, n, l) in [(1, "Zed", 20), (2, "Amy", 100), (3, "Bob", 9)] {
        g.multi_add_row(w, "ml", id, vec![MultiCell::text(n), MultiCell::num(l)], true);
    }
    // rows are inserted in the active sort order (first sortable column, ascending)
    assert_eq!(g.multi_row_ids(w, "ml"), [2, 3, 1]);
    let r = g.view_rect(w, "ml").unwrap();
    let hy = r.t + 5.0;
    // a click on the 2nd header cell (x = 60 + 6 + ..) sorts by level ascending, again descending
    let cx = r.l + 66.0 + 10.0;
    click(&mut g, cx, hy);
    assert_eq!(g.multi_row_ids(w, "ml"), [3, 1, 2]);
    assert_eq!(g.multi_sort_state(w, "ml"), Some((1, false)));
    click(&mut g, cx, hy);
    assert_eq!(g.multi_row_ids(w, "ml"), [2, 1, 3]);
    // the header sits above the rows: row 0 is at header height 19 + 1
    let row0 = r.t + 19.0 + 1.0 + 3.0;
    let ev = click(&mut g, r.l + 10.0, row0);
    assert!(ev.iter().any(|e| matches!(e, Event::MultiMouse { id: Some(2), button: 1, .. })), "{ev:?}");
    // the application selects (single selection flag 0x40)
    g.multi_select(w, "ml", 2, true, true);
    g.multi_select(w, "ml", 1, true, true);
    assert_eq!(g.multi_selected(w, "ml"), [1]);
    // dragging the right edge of the first header cell resizes it
    g.input(InputEvent::MouseMove { x: r.l + 60.0, y: hy });
    g.input(InputEvent::MouseDown { x: r.l + 60.0, y: hy, button: MouseButton::Left });
    g.input(InputEvent::MouseMove { x: r.l + 80.0, y: hy });
    let ev = g.input(InputEvent::MouseUp { x: r.l + 80.0, y: hy, button: MouseButton::Left });
    assert_eq!(g.multi_columns(w, "ml")[0].1, 80.0);
    assert!(ev.iter().any(|e| matches!(e, Event::MultiColumnResized { col: 0, width, .. } if *width == 80.0)), "{ev:?}");
}

#[test]
fn rig_is_real() {
    // the other tests return early without a client: make sure they ran against one when it is installed
    if ao_gui::client_dir().join("cd_image/gui").exists() {
        assert!(rig().is_some());
    }
}
