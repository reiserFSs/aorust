//! HTML subset of `TextView_c` (`HTMLParser_c::_ParseTag` 0x1015c9ad, `TextRenderer_c::_ReWrap` 0x10161ba4 / `_AddLineDesc` 0x10161b44 /
//! `_RenderLine` 0x10161112): inline `<img src=tdb://id:NAME>`, `<div indent=wrapped>` blocks, `<a style=text-decoration:none>`.
//! Skipped without a client install.
use ao_gui::text::layout_text;
use ao_gui::{tvf, DrawCmd, FontId, Gui, WindowSize};

const FLAGS: u32 = tvf::MULTILINE | tvf::WORD_WRAP;
const BULLET: &str = "GFX_GUI_NPCCHAT_BULLET";

fn rig() -> Option<Gui> {
    Gui::new(&ao_gui::client_dir(), None).ok()
}

fn layout(g: &mut Gui, html: &str, wrap: i32) -> ao_gui::text::TextLayout {
    let (fonts, colors) = g.text_parts();
    layout_text(fonts, colors, FontId::Normal, html, FLAGS, Some(wrap))
}

#[test]
fn bullet_is_an_inline_image_run_with_its_gfx_size() {
    let Some(mut g) = rig() else { return };
    let (w, h) = g.gfx().size(g.gfx().id(BULLET).unwrap());
    let l = layout(&mut g, &format!("<img src=tdb://id:{BULLET}> Hello"), 300);
    assert_eq!(l.lines.len(), 1);
    let run = &l.lines[0].runs[0];
    assert_eq!(run.img, Some((BULLET.to_string(), w as i32, h as i32)));
    assert!(l.lines[0].width > w as i32, "the text follows the image: {}", l.lines[0].width);
    // an unknown name is dropped like an unresolved `src` (`_ParseTag` returns false)
    let l = layout(&mut g, "<img src=tdb://id:NO_SUCH_GFX>x", 300);
    assert!(l.lines[0].runs.iter().all(|r| r.img.is_none()));
}

#[test]
fn divs_are_blocks_and_wrapped_lines_are_indented_by_ten() {
    let Some(mut g) = rig() else { return };
    // two answers, no <br>: one line each
    let two = "<div indent=wrapped>First</div><div indent=wrapped>Second</div>";
    let l = layout(&mut g, two, 300);
    assert_eq!(l.lines.len(), 2, "{:?}", l.lines.iter().map(|l| l.runs.len()).collect::<Vec<_>>());
    assert!(l.lines.iter().all(|l| l.indent == 0));
    // a <br> right behind </div> adds no empty line (the chat window joins its divs with it)
    let br = layout(&mut g, "<div indent=wrapped>First</div><br><div indent=wrapped>Second</div>", 300);
    assert_eq!(br.lines.len(), 2);
    // a long answer wraps: the continuation lines are indented by 10 px (`_AddLineDesc`), the first line is not
    let long = "<div indent=wrapped>alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi omicron pi rho sigma tau upsilon</div>";
    let l = layout(&mut g, long, 120);
    assert!(l.lines.len() >= 3);
    assert_eq!(l.lines[0].indent, 0);
    assert!(l.lines[1..].iter().all(|l| l.indent == 10));
    assert!(l.lines.iter().all(|l| l.width <= 120 + 1), "{:?}", l.lines.iter().map(|l| l.width).collect::<Vec<_>>());
    // outside the div the indent is gone again
    let l = layout(&mut g, &format!("{long}<br>plain text that is fairly long and wraps too, plain text that is fairly long"), 120);
    assert_eq!(l.lines.last().unwrap().indent, 0);
}

#[test]
fn plain_link_keeps_the_font_colour_and_default_link_is_link_coloured() {
    let Some(mut g) = rig() else { return };
    let l = layout(&mut g, "<a href=1 style=text-decoration:none><font color=CCNPCChatQuestion>Yes</font></a><a href=2>Link</a>", 300);
    let runs = &l.lines[0].runs;
    assert_eq!(runs[0].href, "1");
    assert!(!runs[0].link, "text-decoration:none: no link colour");
    assert!(runs[0].color.is_some());
    assert_eq!(runs[1].href, "2");
    assert!(runs[1].link);
}

#[test]
fn text_view_draws_the_bullet_next_to_the_answer() {
    let Some(mut g) = rig() else { return };
    let xml = "<root><View view_layout=\"vertical\"><TextView feature_flags=\"TVF_MULTILINE|TVF_WORD_WRAP\" font=\"NORMAL\" name=\"t\" min_size=\"Point(200,60)\" max_size=\"Point(200,60)\"/></View></root>";
    let w = g.open_window_xml("t", xml, (10, 10), WindowSize::Fixed(200, 60)).unwrap();
    g.set_text(w, "t", &format!("<div indent=wrapped><img src=tdb://id:{BULLET}> <a href=0 style=text-decoration:none>Where am I?</a></div>"));
    let bullet = g.gfx().id(BULLET).unwrap();
    let list = g.frame(0.0);
    let hit = list.cmds.iter().filter(|c| matches!(c, DrawCmd::Gfx { id, .. } if *id == bullet)).count();
    assert_eq!(hit, 1, "one bullet drawn");
}
