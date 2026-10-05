//! `cargo run --release -p ao-gui --example dump_view -- LoginWindow out.png [--size WxH] [--sample]`
//!
//! Renders a `Views/<name>.xml` window offscreen through `ao_render::GuiRenderer` and writes a PNG.
//! `--sample` fills the view with representative data (character rows, progress, combo items).

use ao_gui::{Gui, WindowSize};
use ao_render::{GuiRenderer, Renderer};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let name = args.first().cloned().unwrap_or_else(|| "LoginWindow".into());
    let out = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| format!("{name}.png")));
    let mut size: Option<(u32, u32)> = None;
    if let Some(i) = args.iter().position(|a| a == "--size") {
        let s = &args[i + 1];
        let (w, h) = s.split_once('x').unwrap();
        size = Some((w.parse()?, h.parse()?));
    }
    let sample = args.iter().any(|a| a == "--sample");

    let client = ao_gui::client_dir();
    let text = ao_formats::screens::TextDb::load(&client).ok();
    let localize: Option<ao_gui::Localize> = text.map(|t| {
        Box::new(move |s: &str| {
            let r = t.label(s);
            (r != s).then_some(r)
        }) as ao_gui::Localize
    });
    let mut gui = Gui::new(&client, localize)?;
    let (w, h) = match size {
        Some((sw, sh)) => {
            let id = gui.open_window(&name, (0, 0), WindowSize::Fixed(sw, sh))?;
            let _ = id;
            (sw, sh)
        }
        None => {
            let pad = 40;
            let id = if args.iter().any(|a| a == "--frame") { gui.open_framed_window(&name, (pad, pad), WindowSize::Preferred)? } else { gui.open_window(&name, (pad, pad), WindowSize::Preferred)? };
            let (ww, wh) = gui.outer_size(id);
            (ww + 2 * pad as u32, wh + 2 * pad as u32)
        }
    };
    if sample {
        fill_sample(&mut gui, &name);
    }
    for wmsg in &gui.warnings {
        eprintln!("warning: {wmsg}");
    }

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let r = Renderer::new(&instance, None)?;
    let mut gr = GuiRenderer::new(&r, &gui);
    let tex = r.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: r.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&Default::default());
    // background: dark blue-grey like the 3D backdrop behind the login windows (only so translucency is visible)
    let mut enc = r.device.create_command_encoder(&Default::default());
    {
        let bg = wgpu::Color { r: 0.02, g: 0.03, b: 0.05, a: 1.0 };
        let _ = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, depth_slice: None, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(bg), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    r.queue.submit([enc.finish()]);
    let list = gui.frame(0.0);
    gr.draw(&r, &view, (w, h), 1, &gui, &list);

    let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buf = r.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (row * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
    let mut enc = r.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    r.queue.submit([enc.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buf.slice(..).map_async(wgpu::MapMode::Read, move |res| tx.send(res).unwrap());
    r.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| anyhow::anyhow!("{e}"))?;
    rx.recv()??;
    let mapped = buf.slice(..).get_mapped_range();
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        px.extend_from_slice(&mapped[(y * row) as usize..(y * row + w * 4) as usize]);
    }
    let mut enc = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&out)?), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&px)?;
    println!("{} ({}x{}, {} draw commands)", out.display(), w, h, list.cmds.len());
    Ok(())
}

fn fill_sample(gui: &mut Gui, name: &str) {
    match name {
        "LoginWindow" => {
            gui.set_visible(0, "steam_btn", false);
            gui.combo_set_items(0, "username", vec!["Vhab".into(), "Athrox".into()]);
            gui.set_text(0, "username", "Vhab");
            gui.set_feature_flags(0, "password", ao_gui::tvf::PASSWORD);
            gui.set_text(0, "password", "secret");
        }
        "ProgressDialog" => {
            gui.set_text(0, "message", "Connecting to server...");
            gui.set_progress(0, "progress_bar", 0.4);
        }
        "CharacterSelectionWindow" => {
            gui.set_layout_vertical(0, "characters_view", true);
            // row 0 selected + activated, row 1 unselected + Inactive, row 2 unselected + activated
            for (i, (n, lvl, br, prof)) in [("Vhab", "220", "Atrox", "Soldier"), ("Reiserfs", "14", "Solitus", "Enforcer"), ("Nanogirl", "88", "Nanomage", "Doctor")].iter().enumerate() {
                if let Ok(it) = gui.add_view(0, "characters_view", "CharacterSelectionItem") {
                    gui.set_text_in(it, "name_btn", n);
                    gui.set_text_in(it, "level", lvl);
                    gui.set_text_in(it, "breed", br);
                    gui.set_text_in(it, "profession", prof);
                    gui.set_text_in(it, "gender", if i == 0 { "Male" } else { "Female" });
                    gui.set_text_in(it, "location", "Newland City (566)");
                    gui.set_item_selected(it, i == 0);
                    if i == 1 {
                        gui.set_text_in(it, "status", "Inactive");
                        gui.set_color_in(it, "status", 0xEE4444);
                    } else {
                        for v in ["status", "status_left", "status_right", "status_lbl"] {
                            gui.remove_view_in(it, v);
                        }
                    }
                }
            }
            gui.set_text(0, "slots_available", "6/7 slots available");
        }
        "CharacterActivateWindow" => gui.set_text(0, "confirmation_text", "Activating Vhab will use a free character slot. Do you want to continue?"),
        "CharacterDeleteWindow" => gui.set_text(0, "confirmation_text", "Are you sure you want to delete 'Vhab'?"),
        _ => {}
    }
}
