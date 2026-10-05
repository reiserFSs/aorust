mod demo;

use anyhow::{bail, Context, Result};
use ao_rdb::RecordStore;
use ao_scene::Scene;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Native macOS Anarchy Online client")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Download/patch the game client in place.
    Install {
        #[arg(long)]
        client: Option<PathBuf>,
    },
    /// Open the free-fly viewer (or write a screenshot).
    View {
        #[command(flatten)]
        opts: ViewOpts,
        #[command(subcommand)]
        what: What,
    },
}

#[derive(Args)]
struct ViewOpts {
    #[arg(long, global = true)]
    client: Option<PathBuf>,
    /// Render offscreen to this PNG and exit.
    #[arg(long, global = true)]
    screenshot: Option<PathBuf>,
    /// Camera position for --screenshot, "x,y,z" (default: framed on scene bounds).
    #[arg(long, global = true, value_parser = vec3, allow_hyphen_values = true)]
    eye: Option<[f32; 3]>,
    /// Look-at target for --screenshot, "x,y,z".
    #[arg(long, global = true, value_parser = vec3, allow_hyphen_values = true)]
    at: Option<[f32; 3]>,
    /// Screenshot size, "WxH".
    #[arg(long, global = true, default_value = "1280x800", value_parser = size)]
    size: (u32, u32),
}

#[derive(Subcommand)]
enum What {
    /// Static mesh record id.
    Mesh { id: u32 },
    /// Playfield id (or --list).
    Pf {
        id: Option<u32>,
        #[arg(long)]
        list: bool,
    },
    /// Synthetic test scene.
    Demo {
        /// Number of cube instances.
        #[arg(long, default_value_t = 600)]
        count: usize,
    },
}

fn vec3(s: &str) -> Result<[f32; 3], String> {
    let v: Vec<f32> = s.split(',').map(|p| p.trim().parse().map_err(|e| format!("{e}"))).collect::<Result<_, _>>()?;
    v.try_into().map_err(|_| "expected x,y,z".to_string())
}

fn size(s: &str) -> Result<(u32, u32), String> {
    let (w, h) = s.split_once('x').ok_or("expected WxH")?;
    Ok((w.parse().map_err(|e| format!("{e}"))?, h.parse().map_err(|e| format!("{e}"))?))
}

fn client_dir(arg: Option<PathBuf>) -> Result<PathBuf> {
    match arg {
        Some(p) => Ok(p),
        None => Ok(PathBuf::from(std::env::var("HOME").context("HOME unset")?).join("Games/ProjectRubiKa/client")),
    }
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Install { client } => ao_install::run(&client_dir(client)?),
        Cmd::View { opts, what } => {
            let dir = client_dir(opts.client.clone())?;
            let scene: Scene = match what {
                What::Demo { count } => demo::scene(count),
                What::Mesh { id } => ao_formats::mesh::load_mesh(&RecordStore::open(&dir)?, id)?,
                What::Pf { list: true, .. } => {
                    for (id, name) in ao_formats::playfield::list_playfields(&RecordStore::open(&dir)?)? {
                        println!("{id}\t{name}");
                    }
                    return Ok(());
                }
                What::Pf { id: Some(id), .. } => ao_formats::playfield::load_playfield(&RecordStore::open(&dir)?, &dir, id)?,
                What::Pf { id: None, .. } => bail!("view pf: give an id or --list"),
            };
            match opts.screenshot {
                Some(path) => {
                    let (eye, at) = ao_render::default_view(&scene);
                    let eye = opts.eye.unwrap_or(eye.into());
                    let at = opts.at.unwrap_or(at.into());
                    ao_render::render_to_png(&scene, eye, at, opts.size.0, opts.size.1, &path)
                }
                None => ao_render::run_viewer(scene),
            }
        }
    }
}
