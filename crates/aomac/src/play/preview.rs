//! The character-selection preview, as `CharacterViewer_c`/`CCCharacter_t` do it (docs/screens.md §5.5): the default
//! character of the entry's breed/sex with its first head (or the cached appearance, worn cloth included), `idle-stand` played once, then after every finished clip
//! `rand() % 115 < 23` picks one of the 23 socials, else idle again.
//!
//! Poses are CPU-skinned on a worker thread (`load_player` per frame sample); the UI plays the cached frames. Socials are
//! built on demand: a clip that is not cached yet is requested and idle plays meanwhile (the original has no such delay).

use ao_formats::character::{self, Player, Role};
use ao_formats::screens::{self, CharSelectLook};
use ao_rdb::RecordStore;
use ao_scene::Scene;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};

/// Pose samples per second of the cached clips.
pub const FPS: f32 = 20.0;

pub enum Out {
    /// The textured bind/idle pose to upload.
    First(Box<Scene>),
    /// All frames of a clip (`None` = idle).
    Clip(Option<usize>, Vec<Scene>),
    Failed(String),
}

pub struct Worker {
    req: Sender<Option<usize>>,
    pub rx: Receiver<Out>,
}

impl Worker {
    /// Starts building the preview of `(breed, sex)`; dropping the worker stops the thread.
    pub fn start(dir: PathBuf, breed: i32, sex: i32, char_id: i32, head: i32) -> Worker {
        let (req, req_rx) = channel::<Option<usize>>();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            if let Err(e) = run(&dir, breed, sex, char_id, head, &req_rx, &tx) {
                let _ = tx.send(Out::Failed(format!("{e:#}")));
            }
        });
        Worker { req, rx }
    }

    #[cfg(test)]
    pub(super) fn queued_first(scene: Scene) -> Worker {
        let (req, _) = channel();
        let (tx, rx) = channel();
        tx.send(Out::First(Box::new(scene))).unwrap();
        Worker { req, rx }
    }

    /// Asks for the frames of social `i` (index into `screens::SOCIALS`) or idle (`None`).
    pub fn request(&self, clip: Option<usize>) {
        let _ = self.req.send(clip);
    }
}

/// `CharacterViewerModule_c::GetData(charId)` without a live world (docs/screens.md §5.5): the appearance cache entry of the
/// character if there is one — first aomac's `CharacterViewer.xml`, then the original client's `prefs/` — else `None`
/// (= `CCCharacter_t::SetBreed`, the default character of the entry's breed/sex).
fn cached(client: &std::path::Path, char_id: i32) -> Option<character::CachedCharacter> {
    let mut dirs = vec![super::prefs::dir()?];
    dirs.push(client.join("prefs"));
    dirs.iter().find_map(|d| character::ViewerCache::load(d).0.remove(&char_id))
}

/// The creation reply supplies the identity, not the appearance. Keep the submitted mesh ids in the existing viewer cache.
pub(super) fn remember_created(client: &std::path::Path, id: i32, req: &ao_net::msg::CreateCharacterRequest) -> anyhow::Result<()> {
    let dir = super::prefs::dir().ok_or_else(|| anyhow::anyhow!("preferences directory unavailable"))?;
    let store = RecordStore::open(client)?;
    let (breed, gender) = screens::wire_breed_sex(req.breed, req.gender)?;
    let mesh = character::player_model_build(&store, breed, gender, req.width.clamp(0, 2) as u8)?;
    remember_created_in(&dir, id, req, mesh as i32)
}

fn remember_created_in(dir: &std::path::Path, id: i32, req: &ao_net::msg::CreateCharacterRequest, mesh_id: i32) -> anyhow::Result<()> {
    let mut cache = character::ViewerCache::load(dir);
    cache.update(character::CachedCharacter {
        id,
        time: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)?.as_secs().min(i32::MAX as u64) as i32,
        mesh_id,
        head_id: req.head,
        breed: req.breed,
        sex: req.gender,
        fatness: req.width,
        ..Default::default()
    });
    cache.save(dir)?;
    Ok(())
}

fn listed_appearance(id: i32, breed: i32, sex: i32, mesh_id: i32, head: i32) -> Option<character::CachedCharacter> {
    (head > 0).then_some(character::CachedCharacter { id, mesh_id, head_id: head, breed, sex, fatness: 1, ..Default::default() })
}

fn scene(store: &RecordStore, look: &CharSelectLook, cache: Option<&character::CachedCharacter>, pose: (Role, f32)) -> anyhow::Result<Scene> {
    match cache {
        Some(c) => character::load_cached_character(store, c, Some(pose)),
        None => character::load_player(store, &Player::new(look.breed, look.gender, look.skin, Some(look.head.0)), Some(pose)),
    }
}

fn frames(store: &RecordStore, look: &CharSelectLook, cache: Option<&character::CachedCharacter>, role: Role) -> anyhow::Result<Vec<Scene>> {
    let model = match cache {
        Some(c) => c.mesh_id as u32,
        None => character::player_model(store, look.breed, look.gender)?,
    };
    let anim = character::load_anim(store, character::role_anim(store, model, &role)?)?;
    let n = ((anim.duration / 1000.0 * FPS).round() as usize).max(1);
    (0..n)
        .map(|k| {
            let mut s = scene(store, look, cache, (role.clone(), k as f32 / FPS))?;
            s.textures.clear();
            Ok(s)
        })
        .collect()
}

fn run(dir: &std::path::Path, breed: i32, sex: i32, char_id: i32, head: i32, req: &Receiver<Option<usize>>, tx: &Sender<Out>) -> anyhow::Result<()> {
    let store = RecordStore::open(dir)?;
    let look = screens::char_select_look(&store, breed, sex)?;
    let cache = cached(dir, char_id).or_else(|| listed_appearance(char_id, breed, sex, look.model as i32, head));
    if tx.send(Out::First(Box::new(scene(&store, &look, cache.as_ref(), (Role::Idle, 0.0))?))).is_err() {
        return Ok(());
    }
    let mut done = vec![];
    let mut queue = vec![None];
    loop {
        let clip = match queue.pop() {
            Some(c) => c,
            None => match req.recv() {
                Ok(c) => c,
                Err(_) => return Ok(()),
            },
        };
        while let Ok(more) = req.try_recv() {
            queue.push(more);
        }
        if done.contains(&clip) {
            continue;
        }
        done.push(clip);
        let role = clip.map_or(Role::Idle, |i| Role::Emote(screens::SOCIALS[i].to_string()));
        let t0 = std::time::Instant::now();
        let f = frames(&store, &look, cache.as_ref(), role)?;
        if std::env::var_os("AOMAC_PERF").is_some() {
            eprintln!("preview clip {clip:?}: {} frames in {:.0} ms", f.len(), t0.elapsed().as_secs_f32() * 1000.0);
        }
        if tx.send(Out::Clip(clip, f)).is_err() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn created_head_survives_reload_under_acknowledged_identity() {
        let dir = std::env::temp_dir().join(format!("aomac-created-preview-{}", std::process::id()));
        let req = ao_net::msg::CreateCharacterRequest { breed: 4, gender: 2, head: 40682, width: 2, ..Default::default() };
        remember_created_in(&dir, 33512, &req, 5907).unwrap();
        let req2 = ao_net::msg::CreateCharacterRequest { head: 40683, ..req.clone() };
        remember_created_in(&dir, 33513, &req2, 5908).unwrap();
        let cache = character::ViewerCache::load(&dir);
        assert_eq!(cache.0[&33512].head_mesh(), 40682);
        assert_eq!(cache.0[&33513].head_mesh(), 40683);
        assert_eq!((cache.0[&33512].mesh_id, cache.0[&33512].fatness), (5907, 2));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod listed_tests {
    #[test]
    fn supplied_list_head_is_used_and_missing_legacy_head_falls_back() {
        let c = super::listed_appearance(9, 4, 2, 5907, 40683).unwrap();
        assert_eq!((c.id, c.head_mesh()), (9, 40683));
        assert!(super::listed_appearance(9, 4, 2, 5907, 0).is_none());
        assert!(super::listed_appearance(9, 4, 2, 5907, -1).is_none());
    }
}
