mod demo;
mod play;

use anyhow::{bail, Context, Result};
use ao_rdb::RecordStore;
use ao_scene::Scene;
use clap::{Args, Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Native macOS Anarchy Online client")]
struct Cli {
    /// Defaults to `play` (also what a macOS .app bundle launches).
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Login screen -> character select -> world (the real client).
    Play {
        #[arg(long)]
        client: Option<PathBuf>,
        /// Debug: skip login and show a built-in character list (offline 3D preview).
        #[arg(long, hide = true)]
        fake_charlist: bool,
        /// Debug: character index selected first with --fake-charlist.
        #[arg(long, hide = true, default_value_t = 0)]
        select: usize,
    },
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
    /// Camera position (screenshot and interactive start), "x,y,z" (default: framed on scene bounds).
    #[arg(long, global = true, value_parser = vec3, allow_hyphen_values = true)]
    eye: Option<[f32; 3]>,
    /// Look-at target (screenshot and interactive start), "x,y,z".
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
        /// Game day time in seconds, 0..6480 (0 midnight, ~3240 noon; default: the client's frozen 2648.69).
        #[arg(long)]
        time_of_day: Option<f32>,
    },
    /// Character model record id, optionally posed by an animation clip.
    Char {
        id: u32,
        #[arg(long)]
        anim: Option<u32>,
        /// Clip time in seconds (with --anim).
        #[arg(long, default_value_t = 0.0)]
        time: f32,
        /// Head mesh id (rdb 1010001) attached to the body.
        #[arg(long)]
        head: Option<u32>,
        /// Animation role (clip name, e.g. walk, run, idle, social-bow) instead of --anim.
        #[arg(long, conflicts_with = "anim")]
        role: Option<ao_formats::character::Role>,
    },
    /// Player character assembled from creation-screen choices (body + head + naked skin).
    Player {
        breed: ao_formats::character::Breed,
        gender: ao_formats::character::Gender,
        /// Head number `NN` of head_<race><sex>NN.abiff (default: lowest).
        #[arg(long)]
        head: Option<u32>,
        /// Solitus skin tone: caucasian (default), asian, african.
        #[arg(long, default_value = "caucasian")]
        skin: ao_formats::character::Skin,
        #[arg(long)]
        role: Option<ao_formats::character::Role>,
        /// Clip time in seconds (with --role).
        #[arg(long, default_value_t = 0.0)]
        time: f32,
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
    match Cli::parse().cmd.unwrap_or(Cmd::Play { client: None, fake_charlist: false, select: 0 }) {
        Cmd::Play { client, fake_charlist, select } => play::run(client_dir(client)?, fake_charlist.then_some(select)),
        Cmd::Install { client } => ao_install::run(&client_dir(client)?),
        Cmd::View { opts, what } => {
            let dir = client_dir(opts.client.clone())?;
            let scene: Scene = match what {
                What::Demo { count } => demo::scene(count),
                What::Char { id, anim, time, head, role } => {
                    let store = RecordStore::open(&dir)?;
                    let anim = match role {
                        Some(r) => Some(ao_formats::character::role_anim(&store, id, &r)?),
                        None => anim,
                    };
                    match (head, anim) {
                        (Some(h), a) => ao_formats::character::load_character_with_head(&store, id, h, a.map(|a| (a, time)))?,
                        (None, None) => ao_formats::character::load_character(&store, id)?,
                        (None, Some(a)) => ao_formats::character::load_character_posed(&store, id, a, time)?,
                    }
                }
                What::Player { breed, gender, head, skin, role, time } => {
                    let player = ao_formats::character::Player { breed, gender, skin, head };
                    ao_formats::character::load_player(&RecordStore::open(&dir)?, &player, role.map(|r| (r, time)))?
                }
                What::Mesh { id } => ao_formats::mesh::load_mesh(&RecordStore::open(&dir)?, id)?,
                What::Pf { list: true, .. } => {
                    let mut out = std::io::stdout().lock();
                    for (id, name) in ao_formats::playfield::list_playfields(&RecordStore::open(&dir)?)? {
                        // Closed pipe (`| head`) is a normal way to stop reading.
                        if writeln!(out, "{id}\t{name}").is_err() {
                            break;
                        }
                    }
                    return Ok(());
                }
                What::Pf { id: Some(id), time_of_day, .. } => {
                    ao_formats::playfield::load_playfield_at(&RecordStore::open(&dir)?, &dir, id, time_of_day.unwrap_or(ao_formats::playfield::DEFAULT_DAY_TIME))?
                }
                What::Pf { id: None, .. } => bail!("view pf: give an id or --list"),
            };
            match opts.screenshot {
                Some(path) => {
                    let (eye, at) = ao_render::default_view(&scene);
                    let eye = opts.eye.unwrap_or(eye.into());
                    let at = opts.at.unwrap_or(at.into());
                    ao_render::render_to_png(&scene, eye, at, opts.size.0, opts.size.1, &path)
                }
                None => {
                    // --eye/--at also place the interactive camera.
                    let mut scene = scene;
                    scene.spawn = opts.eye.or(scene.spawn);
                    scene.spawn_look_at = opts.at.or(scene.spawn_look_at);
                    ao_render::run_viewer(scene)
                }
            }
        }
    }
}
