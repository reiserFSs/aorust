//! `CanvasView` extras (docs/gui.md 12): tooltip rectangles, press / release / double-click events, and the style-0 window with its tab
//! title (skipped without a client install).
use ao_gui::{CanvasTip, Event, Gui, InputEvent, MouseButton, WindowSize};

const XML: &str = r#"<root><View view_layout="vertical"><CanvasView name="c" min_size="Point(200,100)" max_size="Point(200,100)"/></View></root>"#;

fn tip(x0: f32, x1: f32, title: &str) -> CanvasTip {
    CanvasTip { rect: [x0, 0.0, x1, 27.0], title: title.into(), body: String::new() }
}

#[test]
fn canvas_tip_rectangles_follow_the_pointer() {
    let Ok(mut g) = Gui::new(&ao_gui::client_dir(), None) else { return };
    let w = g.open_window_xml("t", XML, (10, 10), WindowSize::Fixed(200, 100)).unwrap();
    g.set_screen_size(800, 600);
    g.set_canvas_tips(w, "c", vec![tip(0.0, 27.0, "First"), tip(30.0, 57.0, "Second")]);
    let r = g.view_rect(w, "c").unwrap();
    let at = |g: &mut Gui, x: f32| g.input(InputEvent::MouseMove { x: r.l + x, y: r.t + 5.0 });
    at(&mut g, 5.0);
    g.frame(0.6);
    assert_eq!(g.tooltip_shown().unwrap().0, "First");
    // the gap between the rectangles has no tip: the shown one closes
    at(&mut g, 28.0);
    assert!(g.tooltip_shown().is_none());
    g.frame(1.0);
    assert!(g.tooltip_shown().is_none());
    // another rectangle of the same view is another tip (the rest timer restarts)
    at(&mut g, 40.0);
    g.frame(0.3);
    assert!(g.tooltip_shown().is_none());
    g.frame(0.3);
    assert_eq!(g.tooltip_shown().unwrap().0, "Second");
}

#[test]
fn canvas_press_release_and_double_click() {
    let Ok(mut g) = Gui::new(&ao_gui::client_dir(), None) else { return };
    let w = g.open_window_xml("t", XML, (10, 10), WindowSize::Fixed(200, 100)).unwrap();
    let r = g.view_rect(w, "c").unwrap();
    let (x, y) = (r.l + 20.0, r.t + 10.0);
    let press = |g: &mut Gui, b| g.input(InputEvent::MouseDown { x, y, button: b });
    let clicks = |evs: &[Event]| evs.iter().filter_map(|e| if let Event::CanvasPress { clicks, x, y, .. } = e { Some((*clicks, *x, *y)) } else { None }).collect::<Vec<_>>();
    // left press / release: Press(1) .. Release, then a second press within the double-click time is a double click
    let evs = press(&mut g, MouseButton::Left);
    assert_eq!(clicks(&evs), vec![(1, 20.0, 10.0)]);
    let evs = g.input(InputEvent::MouseUp { x, y, button: MouseButton::Left });
    assert!(evs.iter().any(|e| matches!(e, Event::CanvasRelease { .. })));
    g.frame(0.2);
    assert_eq!(clicks(&press(&mut g, MouseButton::Left)), vec![(2, 20.0, 10.0)]);
    g.input(InputEvent::MouseUp { x, y, button: MouseButton::Left });
    // the third press starts over; a press after the time limit is a single click again
    assert_eq!(clicks(&press(&mut g, MouseButton::Left))[0].0, 1);
    g.input(InputEvent::MouseUp { x, y, button: MouseButton::Left });
    g.frame(ao_gui::DOUBLE_CLICK_TIME + 0.1);
    assert_eq!(clicks(&press(&mut g, MouseButton::Left))[0].0, 1);
    g.input(InputEvent::MouseUp { x, y, button: MouseButton::Left });
    // the right button doubles on its own; a left press in between breaks the chain
    g.frame(1.0);
    assert_eq!(clicks(&press(&mut g, MouseButton::Right))[0].0, 1);
    assert_eq!(clicks(&press(&mut g, MouseButton::Right))[0].0, 2);
}

#[test]
fn tabbed_window_metrics_and_title() {
    let Ok(mut g) = Gui::new(&ao_gui::client_dir(), None) else { return };
    let w = g.open_tabbed_window_xml("t", "Planet Map", XML, (100, 50), WindowSize::Fixed(200, 100)).unwrap();
    // outer border (3, 7, 3, 3) + TabView borders (2, 19, 2, 2)
    assert_eq!(g.outer_size(w), (210, 131));
    assert_eq!(g.view_rect(w, "c").map(|r| (r.l, r.t)), Some((105.0, 76.0)));
    g.set_window_pos(w, (10, 20));
    assert_eq!(g.view_rect(w, "c").map(|r| (r.l, r.t)), Some((15.0, 46.0)));
    // the title is drawn as glyphs inside the strip (above the client, below the outer top edge)
    let list = g.frame(0.0);
    let glyphs: Vec<_> = list.cmds.iter().filter_map(|c| if let ao_gui::DrawCmd::Glyph { dst, .. } = c { Some(*dst) } else { None }).collect();
    assert!(glyphs.len() >= 8, "{} glyphs", glyphs.len());
    assert!(glyphs.iter().all(|d| d[1] >= 27 && d[1] < 46 && d[0] >= 10 + 3 + 20 + 5), "{glyphs:?}");
}
