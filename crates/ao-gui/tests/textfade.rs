//! Chat message fading (`FUN_1009349d` / `FUN_10092f69` / `FUN_10092241`, docs/chat/gui.md §5.1). Skipped without a client install.
use ao_gui::{DrawCmd, Gui, WindowSize};

const XML: &str = r#"<root><View name="v" view_layout="vertical"><ScrollView name="scroll" v_scrollbar_mode="always" max_size="Point(16000,16000)">
<ScrollViewChild view_layout="vertical" max_size="Point(16000,16000)"><TextView name="text" max_size="Point(16000,-1)" font="CHAT"
feature_flags="TVF_WORD_WRAP|TVF_MULTILINE"/></ScrollViewChild></ScrollView></View></root>"#;

fn glyph_alphas(g: &mut Gui, dt: f32) -> Vec<f32> {
    g.frame(dt).cmds.iter().filter_map(|c| if let DrawCmd::Glyph { alpha, .. } = c { Some(*alpha) } else { None }).collect()
}

#[test]
fn lines_stay_opaque_for_the_delay_then_fade_and_vanish() {
    let Ok(mut g) = Gui::new(&ao_gui::client_dir(), None) else { return };
    let w = g.open_window_xml("t", XML, (10, 10), WindowSize::Fixed(300, 200)).unwrap();
    g.set_window_alpha(w, 0.3); // the lines ignore the window alpha (view flag 0x80)
    g.set_text(w, "text", "<div>first</div>");
    let normal = glyph_alphas(&mut g, 0.0);
    assert!(!normal.is_empty() && normal.iter().all(|a| (*a - 0.3).abs() < 1e-4), "{normal:?}");
    // fading on: the existing text becomes the first fade line, the scroll view is hidden
    g.set_text_fade(w, "scroll", Some((8.0, 0.3)), "<div>first</div>");
    assert_eq!(g.fade_line_count(w, "scroll"), 1);
    g.add_fade_line(w, "scroll", "<div>second</div>");
    assert_eq!(g.fade_line_count(w, "scroll"), 2);
    let a = glyph_alphas(&mut g, 7.9);
    assert!(!a.is_empty() && a.iter().all(|a| *a == 1.0), "{a:?}");
    let a = glyph_alphas(&mut g, 0.25); // 8.15 s: halfway through the 0.3 s fade
    assert!(a.iter().all(|a| (*a - 0.5).abs() < 0.02), "{a:?}");
    g.frame(0.2);
    assert_eq!(g.fade_line_count(w, "scroll"), 0);
    // off again: the text view is back at the window alpha
    g.set_text_fade(w, "scroll", None, "");
    let back = glyph_alphas(&mut g, 0.0);
    assert!(!back.is_empty() && back.iter().all(|a| (*a - 0.3).abs() < 1e-4));
}

#[test]
fn newer_lines_push_older_ones_up_by_one_line_height() {
    let Ok(mut g) = Gui::new(&ao_gui::client_dir(), None) else { return };
    let w = g.open_window_xml("t", XML, (10, 10), WindowSize::Fixed(300, 200)).unwrap();
    g.set_text_fade(w, "scroll", Some((8.0, 0.3)), "");
    g.add_fade_line(w, "scroll", "<div>a</div>");
    let ys = |g: &mut Gui| -> Vec<i32> { g.frame(0.0).cmds.iter().filter_map(|c| if let DrawCmd::Glyph { dst, .. } = c { Some(dst[1]) } else { None }).collect() };
    let one = ys(&mut g);
    g.add_fade_line(w, "scroll", "<div>b</div>");
    let two = ys(&mut g);
    let h = g.font_height(ao_gui::FontId::Chat);
    // `a` moved up by exactly one line height (`h + 1 - ChatTextShadowOffset 1`), `b` took the bottom row
    assert_eq!(one.iter().min().unwrap() - two.iter().min().unwrap(), h);
    // lines scrolled above the top are dropped: 200 px / ~16 px per line
    for _ in 0..40 {
        g.add_fade_line(w, "scroll", "<div>x</div>");
    }
    assert!(g.fade_line_count(w, "scroll") < 20);
}
