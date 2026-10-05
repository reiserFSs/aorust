//! The Planet Map window (`planetmap_window`, `PlanetMapView_c`) and the playfield Map window (`map_window`,
//! `PlayfieldMapView_c` + `PFMapRenderer_c`). Evidence: docs/gui.md §12, docs/formats.md *Planet map* / *Playfield map image*.
//!
//! * Planet map: tiles of `textures/PlanetMap/<index>` ([`ao_formats::planetmap`]); markers by `Level::locate`
//!   (`FUN_1004b81a`); zoom levels = the index's levels (`FUN_1004bc1f`: ZoomIn enabled below the last level, ZoomOut above 0).
//! * Playfield map: [`ao_formats::topdown`] image of the ground / room shells of the playfield (built on a worker thread),
//!   centred on the own character; dungeon rooms are lit by exploration (`MapData_c::FUN_100427a1`: current room white,
//!   visited 50 % grey, unexplored black).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use ao_formats::planetmap::PlanetMap;
use ao_formats::topdown::{self, GroundMap, NO_OWNER};
use ao_gui::{CanvasItem, Event, GfxId, Gui, WindowId, WindowSize};

use super::hud::WindowKind;
use super::zone::Zone;

/// Default client size of the Planet Map window (`Rect(0,0,_DAT_101b183c,_DAT_101b1840)` in `PlanetMapView_c` 0x1004d26a).
const PLANET_SIZE: (u32, u32) = (300, 400);
/// First-time frame `Rect(100,100,500,500)` (`_DAT_101ae160`, `_DAT_101b172c`): top-left corner.
const PLANET_POS: (i32, i32) = (100, 100);
/// Default client size of the playfield map (`Rect(0,0,_DAT_101c0bcc,_DAT_101c0bd0)` in `PlayfieldMapView_c` 0x100eb905).
const PF_SIZE: (u32, u32) = (179, 199);
/// UNRESOLVED: the playfield window's first-time frame (only `PFMapWindowConfig`/`ShowButtons` is read by the ctor).
const PF_POS: (i32, i32) = (140, 140);
/// Height of the button row (the 27 px `GFX_GUI_PLANETMAP_*` buttons).
const BUTTONS_H: u32 = 27;
/// Longest side of the playfield map image.
const PF_MAX_PX: u32 = 2048;
/// UNRESOLVED: screen pixels per metre of the playfield map (the renderer's scale, `FUN_100e923a`, was not decoded).
const PF_SCALE: f32 = 1.0;
/// `N3Msg_GetMapCharacters(range 500)` in `FUN_100425eb`: dots are drawn for characters within this distance.
const DOT_RANGE: f32 = 500.0;
/// New planet map tiles decoded per frame.
const TILES_PER_FRAME: usize = 6;
/// Stat `MapNavigation` (140): UNRESOLVED guess for what makes `GetMapCharacters` succeed (the map upgrades of the help text).
const STAT_MAP_NAVIGATION: u32 = 140;

const RUBIKA_INDEX: &str = "Normal/PlanetMapIndexNormal.txt";
const SHADOWLANDS_INDEX: &str = "Shadowlands/ShadowlandsMap.txt";

/// The four `Button_c`s of `PlanetMapView_c` (names from 0x1004d26a) with the skin art of the same name.
const BUTTONS: [(&str, &str); 4] = [
    ("ZoomIn", "GFX_GUI_PLANETMAP_ZOOM_IN"),
    ("ZoomOut", "GFX_GUI_PLANETMAP_ZOOM_OUT"),
    ("Character", "GFX_GUI_PLANETMAP_CENTER_PLAYER"),
    ("Quest", "GFX_GUI_PLANETMAP_CENTER_MISSION"),
];

struct Planet {
    window: WindowId,
    map: Option<PlanetMap>,
    /// Playfield the map was last picked for (`Some(None)`: before the zone told).
    tried: Option<Option<u32>>,
    level: usize,
    /// Canvas pixel (of `level`) shown in the middle of the view.
    center: [f32; 2],
    /// Marker position the view last followed.
    followed: Option<[f32; 2]>,
    tiles: HashMap<(usize, u32, u32), GfxId>,
}

struct Pf {
    window: WindowId,
    playfield: u32,
    job: Option<Receiver<Option<(GroundMap, bool)>>>,
    ground: Option<Ground>,
    /// World position (server `X`, `Z`) in the middle of the view.
    center: [f32; 2],
    followed: Option<[f32; 2]>,
    arrows: HashMap<u8, GfxId>,
}

struct Ground {
    map: GroundMap,
    /// Dungeon: rooms are lit by exploration.
    rooms: bool,
    visited: HashSet<u16>,
    current: u16,
    /// The image of the current (`current`, `visited.len()`) state.
    shown: Option<(u16, usize, GfxId)>,
}

pub(super) struct HudMap {
    client: PathBuf,
    planet: Option<Planet>,
    pf: Option<Pf>,
    closed: Vec<WindowKind>,
}

fn canvas_xml(name: &str, size: (u32, u32)) -> String {
    format!("<CanvasView name=\"{name}\" min_size=\"Point({},{})\" max_size=\"Point({},{})\"/>", size.0, size.1, size.0, size.1)
}

impl HudMap {
    pub(super) fn new(client: &std::path::Path) -> Self {
        HudMap { client: client.to_path_buf(), planet: None, pf: None, closed: vec![] }
    }

    pub(super) fn handles(kind: WindowKind) -> bool {
        matches!(kind, WindowKind::Map | WindowKind::PlanetMap)
    }

    pub(super) fn is_open(&self, kind: WindowKind) -> bool {
        match kind {
            WindowKind::Map => self.pf.is_some(),
            WindowKind::PlanetMap => self.planet.is_some(),
            _ => false,
        }
    }

    /// Windows the user closed with their frame button since the last call.
    pub(super) fn take_closed(&mut self) -> Vec<WindowKind> {
        std::mem::take(&mut self.closed)
    }

    pub(super) fn open(&mut self, gui: &mut Gui, kind: WindowKind) {
        if self.is_open(kind) {
            return;
        }
        match kind {
            WindowKind::PlanetMap => {
                let xml = format!(
                    "<root><View view_layout=\"vertical\">{}{}</View></root>",
                    canvas_xml("map", (PLANET_SIZE.0, PLANET_SIZE.1 - BUTTONS_H)),
                    canvas_xml("buttons", (PLANET_SIZE.0, BUTTONS_H))
                );
                match gui.open_framed_window_xml("PlanetMapView", &xml, PLANET_POS, WindowSize::Fixed(PLANET_SIZE.0, PLANET_SIZE.1)) {
                    Ok(window) => {
                        self.planet = Some(Planet { window, map: None, tried: None, level: 0, center: [0.0; 2], followed: None, tiles: HashMap::new() })
                    }
                    Err(e) => eprintln!("planet map: {e:#}"),
                }
            }
            WindowKind::Map => {
                let xml = format!("<root><View view_layout=\"vertical\">{}</View></root>", canvas_xml("map", PF_SIZE));
                match gui.open_framed_window_xml("PlayfieldMapView", &xml, PF_POS, WindowSize::Fixed(PF_SIZE.0, PF_SIZE.1)) {
                    Ok(window) => {
                        self.pf = Some(Pf { window, playfield: 0, job: None, ground: None, center: [0.0; 2], followed: None, arrows: HashMap::new() })
                    }
                    Err(e) => eprintln!("playfield map: {e:#}"),
                }
            }
            _ => {}
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui, kind: WindowKind) {
        match kind {
            WindowKind::PlanetMap => {
                if let Some(p) = self.planet.take() {
                    gui.close_window(p.window);
                }
            }
            WindowKind::Map => {
                if let Some(p) = self.pf.take() {
                    gui.close_window(p.window);
                }
            }
            _ => {}
        }
    }

    pub(super) fn close_all(&mut self, gui: &mut Gui) {
        self.close(gui, WindowKind::PlanetMap);
        self.close(gui, WindowKind::Map);
    }

    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, _zone: &Zone) -> bool {
        if let Some(p) = self.planet.as_mut() {
            match ev {
                Event::CloseRequested { window } if *window == p.window => {
                    self.close(gui, WindowKind::PlanetMap);
                    self.closed.push(WindowKind::PlanetMap);
                    return true;
                }
                Event::CanvasDrag { window, view, dx, dy } if *window == p.window && view == "map" => {
                    p.center = [p.center[0] - dx, p.center[1] - dy];
                    return true;
                }
                Event::CanvasWheel { window, view, dy, .. } if *window == p.window && view == "map" => {
                    p.zoom(if *dy > 0.0 { 1 } else { -1 });
                    return true;
                }
                Event::CanvasClick { window, view, x, y } if *window == p.window && view == "buttons" => {
                    if let Some(name) = button_at(*x, *y) {
                        match name {
                            "ZoomIn" => p.zoom(1),
                            "ZoomOut" => p.zoom(-1),
                            // `Character`: centre on the own character again (the follow state is dropped)
                            "Character" => p.followed = None,
                            // `Quest`: UNRESOLVED — centres on the active mission's position, which needs mission data
                            _ => {}
                        }
                    }
                    return true;
                }
                _ => {}
            }
        }
        if let Some(p) = self.pf.as_mut() {
            match ev {
                Event::CloseRequested { window } if *window == p.window => {
                    self.close(gui, WindowKind::Map);
                    self.closed.push(WindowKind::Map);
                    return true;
                }
                Event::CanvasDrag { window, view, dx, dy } if *window == p.window && view == "map" => {
                    p.center = [p.center[0] - dx / PF_SCALE, p.center[1] + dy / PF_SCALE];
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &Zone, _dt: f32) {
        self.update_planet(gui, zone);
        self.update_pf(gui, zone);
    }

    // ------------------------------------------------------------------ planet map

    fn update_planet(&mut self, gui: &mut Gui, zone: &Zone) {
        let Some(p) = self.planet.as_mut() else { return };
        if p.tried != Some(zone.playfield) {
            p.tried = Some(zone.playfield);
            match pick_planet_map(&self.client, zone.playfield) {
                Ok(m) => {
                    p.level = p.level.min(m.index.levels.len() - 1);
                    p.map = Some(m);
                    p.tiles.clear();
                    p.followed = None;
                }
                Err(e) => eprintln!("planet map: {e:#}"),
            }
        }
        let Some(map) = p.map.as_ref() else { return };
        let lv = &map.index.levels[p.level];
        // the marker: `FUN_1004b81a` hides it when the playfield has no coordinates entry or the position is unknown
        let marker = zone.playfield.and_then(|pf| map.coords.get(&pf)).zip(zone.own()).map(|(c, d)| lv.locate(c, d.pos[0], d.pos[2]));
        if p.followed != marker {
            // moving the character re-centres the view on it (help text *The PlanetMap Window*)
            if let Some(m) = marker {
                p.center = m;
            }
            if p.followed.is_none() && marker.is_none() && p.center == [0.0; 2] {
                p.center = [(lv.rect[0] + lv.rect[2]) as f32 / 2.0, (lv.rect[1] + lv.rect[3]) as f32 / 2.0];
            }
            p.followed = marker;
        }
        let (cw, ch) = gui.canvas_size(p.window, "map");
        let (cw, ch) = (cw as f32, ch as f32);
        let origin = [p.center[0] - cw / 2.0, p.center[1] - ch / 2.0];
        let ts = lv.texture_size as f32;
        let mut items = vec![CanvasItem::Solid { dst: [0.0, 0.0, cw, ch], color: 0x000000, alpha: 1.0 }];
        let range = |o: f32, len: f32, n: u32| (o / ts).floor().max(0.0) as i64..=(((o + len) / ts).floor() as i64).min(n as i64 - 1);
        let mut fresh = 0;
        for ty in range(origin[1], ch, lv.tiles[1]) {
            for tx in range(origin[0], cw, lv.tiles[0]) {
                let (tx, ty) = (tx as u32, ty as u32);
                let key = (p.level, tx, ty);
                let id = match p.tiles.get(&key) {
                    Some(id) => *id,
                    None if fresh < TILES_PER_FRAME => {
                        fresh += 1;
                        let Ok(t) = map.tile(p.level, tx, ty) else { continue };
                        let id = gui.add_image("planetmap tile", t.rgba, t.width, t.height, false);
                        p.tiles.insert(key, id);
                        id
                    }
                    None => continue,
                };
                let (x, y) = (tx as f32 * ts - origin[0], ty as f32 * ts - origin[1]);
                items.push(CanvasItem::Image { id, src: [0.0, 0.0, ts, ts], dst: [x, y, x + ts, y + ts], alpha: 1.0 });
            }
        }
        if let (Some(m), Some(g)) = (marker, gui.gfx().id("GFX_GUI_PLANETMAP_PLAYER_MARKER")) {
            let (w, h) = gui.gfx().size(g);
            let (x, y) = (m[0] - origin[0] - w as f32 / 2.0, m[1] - origin[1] - h as f32 / 2.0);
            items.push(CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [x.floor(), y.floor(), x.floor() + w as f32, y.floor() + h as f32], alpha: 1.0 });
        }
        gui.set_canvas(p.window, "map", items);
        // button row: ZoomIn/ZoomOut left, Character/Quest right; the unusable ones are not drawn enabled
        let levels = map.index.levels.len();
        let mut row = Vec::new();
        for (i, (name, gfx)) in BUTTONS.iter().enumerate() {
            let Some(g) = gui.gfx().id(gfx) else { continue };
            let (w, h) = gui.gfx().size(g);
            let (x, _) = button_rect(i);
            let enabled = match *name {
                "ZoomIn" => p.level + 1 < levels,
                "ZoomOut" => p.level > 0,
                "Character" => marker.is_some(),
                _ => false,
            };
            row.push(CanvasItem::Image { id: g, src: [0.0, 0.0, w as f32, h as f32], dst: [x, 0.0, x + w as f32, h as f32], alpha: if enabled { 1.0 } else { 0.4 } });
        }
        gui.set_canvas(p.window, "buttons", row);
    }

    // ------------------------------------------------------------------ playfield map

    fn update_pf(&mut self, gui: &mut Gui, zone: &Zone) {
        let Some(p) = self.pf.as_mut() else { return };
        if let Some(pf) = zone.playfield.filter(|&pf| pf != p.playfield) {
            // a new playfield: the map is rebuilt (exploration starts again, "since you last entered")
            p.playfield = pf;
            p.ground = None;
            p.followed = None;
            let (tx, rx) = channel();
            let client = self.client.clone();
            std::thread::spawn(move || {
                let _ = tx.send(build_ground(&client, pf));
            });
            p.job = Some(rx);
        }
        if let Some(Ok(done)) = p.job.as_ref().map(|j| j.try_recv()) {
            p.job = None;
            p.ground = done.map(|(map, rooms)| Ground { map, rooms, visited: HashSet::new(), current: NO_OWNER, shown: None });
        }
        let own = zone.own().map(|d| (d.pos, d.yaw));
        let here = own.map(|(pos, _)| [pos[0], pos[2]]);
        if p.followed != here {
            if let Some(h) = here {
                p.center = h;
            }
            p.followed = here;
        }
        let (cw, ch) = gui.canvas_size(p.window, "map");
        let (cw, ch) = (cw as f32, ch as f32);
        let center = p.center;
        let to_screen = |x: f32, z: f32| [cw / 2.0 + (x - center[0]) * PF_SCALE, ch / 2.0 - (z - center[1]) * PF_SCALE];
        let mut items = Vec::new();
        if let Some(g) = p.ground.as_mut() {
            if let Some(h) = here {
                let px = g.map.to_px(h[0], h[1]);
                let o = if px[0] >= 0.0 && px[1] >= 0.0 && (px[0] as u32) < g.map.width && (px[1] as u32) < g.map.height { g.map.owner[px[1] as usize * g.map.width as usize + px[0] as usize] } else { NO_OWNER };
                if g.rooms && o != NO_OWNER {
                    // `FUN_100428e1`: GetCurrentRoom changed -> the old room goes grey, the new one white
                    if g.current != o {
                        g.visited.insert(o);
                        g.current = o;
                    }
                }
            }
            let key = (g.current, g.visited.len());
            let id = match g.shown {
                Some((c, n, id)) if (c, n) == key => id,
                _ => {
                    let id = gui.add_image("playfield map", lit(g), g.map.width, g.map.height, true);
                    g.shown = Some((key.0, key.1, id));
                    id
                }
            };
            let a = to_screen(g.map.origin[0], g.map.origin[1]);
            let (w, h) = (g.map.width as f32 * g.map.mpp * PF_SCALE, g.map.height as f32 * g.map.mpp * PF_SCALE);
            items.push(CanvasItem::Image { id, src: [0.0, 0.0, g.map.width as f32, g.map.height as f32], dst: [a[0], a[1], a[0] + w, a[1] + h], alpha: 1.0 });
        }
        if let Some((pos, yaw)) = own {
            let upgraded = zone.stat(STAT_MAP_NAVIGATION).is_some_and(|v| v != 0);
            if upgraded {
                // other characters nearby as 4x4 squares (`MapSquare` art), `FUN_100425eb` range 500
                for (id, d) in &zone.dynels {
                    if *id == zone.char_id as i32 {
                        continue;
                    }
                    let (dx, dz) = (d.pos[0] - pos[0], d.pos[2] - pos[2]);
                    if dx * dx + dz * dz > DOT_RANGE * DOT_RANGE {
                        continue;
                    }
                    // UNRESOLVED: the original's choice among MAPSQUARE_CLAN/OMNI/NEUTRAL/MONSTER/TEAMMEMBER/SHOP (by art name and `Side`)
                    let name = match (d.npc, d.side) {
                        (true, _) => "GFX_GUI_MAPSQUARE_MONSTER",
                        (_, 1) => "GFX_GUI_MAPSQUARE_CLAN",
                        (_, 2) => "GFX_GUI_MAPSQUARE_OMNI",
                        _ => "GFX_GUI_MAPSQUARE_NEUTRAL",
                    };
                    if let Some(g) = gui.gfx().id(name) {
                        let s = to_screen(d.pos[0], d.pos[2]);
                        items.push(CanvasItem::Image { id: g, src: [0.0, 0.0, 4.0, 4.0], dst: [s[0].floor() - 2.0, s[1].floor() - 2.0, s[0].floor() + 2.0, s[1].floor() + 2.0], alpha: 1.0 });
                    }
                }
                let s = to_screen(pos[0], pos[2]);
                if let Some(id) = p.arrow(gui, yaw.unwrap_or(0.0)) {
                    items.push(CanvasItem::Image { id, src: [0.0, 0.0, 16.0, 16.0], dst: [s[0].floor() - 8.0, s[1].floor() - 8.0, s[0].floor() + 8.0, s[1].floor() + 8.0], alpha: 1.0 });
                }
            } else {
                // "You are represented by a yellow dot on the map" (help text *The Map Window*)
                let s = to_screen(pos[0], pos[2]);
                items.push(CanvasItem::Solid { dst: [s[0].floor() - 2.0, s[1].floor() - 2.0, s[0].floor() + 2.0, s[1].floor() + 2.0], color: 0xffff00, alpha: 1.0 });
            }
        }
        gui.set_canvas(p.window, "map", items);
    }
}

fn button_rect(i: usize) -> (f32, f32) {
    // zoom buttons on the left, the two centre buttons on the right edge (the original's two HLayout rows)
    let (w, gap) = (27.0, 2.0);
    if i < 2 {
        (gap + i as f32 * (w + gap), w)
    } else {
        (PLANET_SIZE.0 as f32 - (4 - i) as f32 * (w + gap), w)
    }
}

fn button_at(x: f32, y: f32) -> Option<&'static str> {
    (0..BUTTONS.len()).find(|&i| {
        let (l, w) = button_rect(i);
        x >= l && x < l + w && y >= 0.0 && y < BUTTONS_H as f32
    }).map(|i| BUTTONS[i].0)
}

impl Planet {
    /// ZoomIn / ZoomOut (`FUN_1004bc1f`): the level is clamped to `0..levels`; the shown map point stays in the middle.
    fn zoom(&mut self, by: i32) {
        let Some(map) = self.map.as_ref() else { return };
        let to = (self.level as i32 + by).clamp(0, map.index.levels.len() as i32 - 1) as usize;
        if to == self.level {
            return;
        }
        let (a, b) = (&map.index.levels[self.level], &map.index.levels[to]);
        let f = |c: f32, la: u32, ra: u32, lb: u32, rb: u32| lb as f32 + (c - la as f32) / (ra - la).max(1) as f32 * (rb - lb) as f32;
        self.center = [f(self.center[0], a.rect[0], a.rect[2], b.rect[0], b.rect[2]), f(self.center[1], a.rect[1], a.rect[3], b.rect[1], b.rect[3])];
        self.level = to;
        self.followed = None;
    }
}

impl Pf {
    /// `GFX_GUI_MAP_BIG_ARROW` turned to the heading, 16 steps (cached).
    fn arrow(&mut self, gui: &mut Gui, yaw: f32) -> Option<GfxId> {
        let step = ((yaw.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * 16.0).round() as u32 % 16) as u8;
        if let Some(id) = self.arrows.get(&step) {
            return Some(*id);
        }
        let base = gui.gfx().id("GFX_GUI_MAP_BIG_ARROW")?;
        let img = gui.gfx().image(base)?;
        let (w, h) = (img.width as i32, img.height as i32);
        let a = step as f32 / 16.0 * std::f32::consts::TAU;
        let (s, c) = a.sin_cos();
        let mut out = vec![0u8; img.rgba.len()];
        for y in 0..h {
            for x in 0..w {
                // inverse rotation of the destination pixel centre about the image centre (art points up at heading 0)
                let (dx, dy) = (x as f32 + 0.5 - w as f32 / 2.0, y as f32 + 0.5 - h as f32 / 2.0);
                let (sx, sy) = (dx * c + dy * s + w as f32 / 2.0, -dx * s + dy * c + h as f32 / 2.0);
                let (sx, sy) = (sx.floor() as i32, sy.floor() as i32);
                if (0..w).contains(&sx) && (0..h).contains(&sy) {
                    let (si, di) = (((sy * w + sx) * 4) as usize, ((y * w + x) * 4) as usize);
                    out[di..di + 4].copy_from_slice(&img.rgba[si..si + 4]);
                }
            }
        }
        let id = gui.add_image("map arrow", out, w as u32, h as u32, false);
        self.arrows.insert(step, id);
        Some(id)
    }
}

/// The playfield image with the rooms' exploration lighting applied.
fn lit(g: &Ground) -> Vec<u8> {
    let mut rgba = g.map.rgba.clone();
    if !g.rooms {
        return rgba;
    }
    for (px, o) in rgba.chunks_exact_mut(4).zip(&g.map.owner) {
        if *o == NO_OWNER {
            continue;
        }
        let k = if *o == g.current {
            256
        } else if g.visited.contains(o) {
            128
        } else {
            0
        };
        for c in &mut px[..3] {
            *c = (*c as u32 * k / 256) as u8;
        }
    }
    rgba
}

fn build_ground(client: &std::path::Path, pf: u32) -> Option<(GroundMap, bool)> {
    let store = ao_rdb::RecordStore::open(client).ok()?;
    let (scene, report) = match ao_formats::playfield::load_playfield_report(&store, client, pf) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("playfield map {pf}: {e:#}");
            return None;
        }
    };
    let map = topdown::render(&scene, report.ground.clone(), PF_MAX_PX)?;
    Some((map, report.terrain_cells == 0))
}

/// Rubi-Ka map unless the playfield is only known to the Shadowlands coordinates (`Type Rubika|Shadowlands` of the index files;
/// UNRESOLVED: the client's own switch between `PlanetMapIndexFile` and `ShadowlandMapIndexFile`).
fn pick_planet_map(client: &std::path::Path, pf: Option<u32>) -> anyhow::Result<PlanetMap> {
    let rubika = PlanetMap::load(client, RUBIKA_INDEX)?;
    if let Some(pf) = pf.filter(|pf| !rubika.coords.contains_key(pf)) {
        if let Ok(sl) = PlanetMap::load(client, SHADOWLANDS_INDEX) {
            if sl.coords.contains_key(&pf) {
                return Ok(sl);
            }
        }
    }
    Ok(rubika)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::play::zone::DynelState;
    use ao_formats::screens::TextDb;
    use ao_gui::{DrawList, InputEvent, MouseButton};
    use ao_render::{Frontend, Host, Offscreen};
    use std::time::{Duration, Instant};

    struct Shot {
        gui: Gui,
        map: HudMap,
        zone: Zone,
    }

    impl Frontend for Shot {
        fn gui(&self) -> &Gui {
            &self.gui
        }
        fn input(&mut self, ev: InputEvent, _host: &mut Host) {
            for e in self.gui.input(ev) {
                self.map.event(&mut self.gui, &e, &self.zone);
            }
        }
        fn frame(&mut self, dt: f32, _size: (u32, u32), _host: &mut Host) -> DrawList {
            self.map.update(&mut self.gui, &self.zone, dt);
            self.gui.frame(dt)
        }
    }

    fn shot() -> Option<(Shot, Offscreen)> {
        let dir = ao_gui::client_dir();
        if !dir.join("cd_image/textures/PlanetMap").exists() {
            eprintln!("skipping: no client");
            return None;
        }
        let labels = TextDb::load(&dir).unwrap();
        let gui = Gui::new(&dir, Some(Box::new(move |s: &str| Some(labels.label(s)).filter(|r| r != s)))).unwrap();
        let s = Shot { gui, map: HudMap::new(&dir), zone: Zone::default() };
        let o = Offscreen::new(&s, (640, 600)).unwrap();
        Some((s, o))
    }

    fn png(s: &mut Shot, o: &mut Offscreen, name: &str) {
        let mut list = DrawList::default();
        for _ in 0..4 {
            list = o.frame(s, 0.016);
        }
        if let Some(dir) = std::env::var_os("AOMAC_SHOT_DIR") {
            std::fs::create_dir_all(&dir).unwrap();
            o.png(s, &list, &std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
        }
    }

    fn own_at(s: &mut Shot, playfield: u32, pos: [f32; 3], yaw: f32) {
        s.zone.char_id = 7;
        s.zone.playfield = Some(playfield);
        s.zone.dynels.insert(7, DynelState { name: "Testy".into(), pos, yaw: Some(yaw), npc: false, side: 0, level: 1, health: 1, max_health: 1 });
    }

    /// Plays frames until the playfield map's worker delivered the ground image.
    fn wait_ground(s: &mut Shot, o: &mut Offscreen) {
        let t = Instant::now();
        while s.map.pf.as_ref().is_some_and(|p| p.ground.is_none()) && t.elapsed() < Duration::from_secs(60) {
            o.frame(s, 0.016);
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// World position of an opaque ground pixel near the middle of the opaque area.
    fn some_floor(s: &Shot) -> [f32; 3] {
        let g = &s.map.pf.as_ref().unwrap().ground.as_ref().expect("no ground map").map;
        let (mut sx, mut sy, mut n) = (0u64, 0u64, 0u64);
        for (i, a) in g.rgba.chunks_exact(4).enumerate() {
            if a[3] != 0 {
                sx += i as u64 % g.width as u64;
                sy += i as u64 / g.width as u64;
                n += 1;
            }
        }
        // the opaque pixel closest to the centroid
        let (cx, cy) = ((sx / n) as i64, (sy / n) as i64);
        let best = g.rgba.chunks_exact(4).enumerate().filter(|(_, a)| a[3] != 0).map(|(i, _)| (i as i64 % g.width as i64, i as i64 / g.width as i64)).min_by_key(|&(x, y)| (x - cx).pow(2) + (y - cy).pow(2)).unwrap();
        [g.origin[0] + (best.0 as f32 + 0.5) * g.mpp, 0.0, g.origin[1] - (best.1 as f32 + 0.5) * g.mpp]
    }

    #[test]
    fn playfield_maps() {
        for (pf, tag) in [(4604u32, "arrival-hall"), (4582, "icc-shuttleport"), (566, "newland-city")] {
            let Some((mut s, mut o)) = shot() else { return };
            s.map.open(&mut s.gui, WindowKind::Map);
            own_at(&mut s, pf, [0.0; 3], 0.0);
            wait_ground(&mut s, &mut o);
            let pos = some_floor(&s);
            own_at(&mut s, pf, pos, 0.6);
            png(&mut s, &mut o, &format!("map-{tag}"));
            // a map upgrade (stat MapNavigation) turns the dot into the heading arrow
            s.zone.stats.insert(STAT_MAP_NAVIGATION, 1);
            png(&mut s, &mut o, &format!("map-{tag}-upgraded"));
        }
    }

    #[test]
    fn planet_map_follows_the_character_and_zooms() {
        let Some((mut s, mut o)) = shot() else { return };
        s.map.open(&mut s.gui, WindowKind::PlanetMap);
        // Newland City, 100 m east of its origin
        own_at(&mut s, 566, [100.0, 0.0, 200.0], 0.0);
        png(&mut s, &mut o, "planet-566-level0");
        let p = s.map.planet.as_ref().unwrap();
        let lv = &p.map.as_ref().unwrap().index.levels[0];
        let m = lv.locate(&p.map.as_ref().unwrap().coords[&566], 100.0, 200.0);
        assert_eq!(p.center, m);
        // the zoom-in button (first of the row), then drag the map: the view moves, the follow state is kept until the character moves
        let w = s.map.planet.as_ref().unwrap().window;
        let r = s.gui.view_rect(w, "buttons").unwrap();
        let (x, y) = (r.l + 2.0 + 13.0, r.t + 13.0);
        s.input(InputEvent::MouseMove { x, y }, &mut o.host);
        s.input(InputEvent::MouseDown { x, y, button: MouseButton::Left }, &mut o.host);
        s.input(InputEvent::MouseUp { x, y, button: MouseButton::Left }, &mut o.host);
        assert_eq!(s.map.planet.as_ref().unwrap().level, 1);
        png(&mut s, &mut o, "planet-566-level1");
        let c = s.map.planet.as_ref().unwrap().center;
        let mr = s.gui.view_rect(w, "map").unwrap();
        let (x, y) = (mr.l + 100.0, mr.t + 100.0);
        s.input(InputEvent::MouseMove { x, y }, &mut o.host);
        s.input(InputEvent::MouseDown { x, y, button: MouseButton::Left }, &mut o.host);
        s.input(InputEvent::MouseMove { x: x + 30.0, y: y + 10.0 }, &mut o.host);
        s.input(InputEvent::MouseUp { x: x + 30.0, y: y + 10.0, button: MouseButton::Left }, &mut o.host);
        let d = s.map.planet.as_ref().unwrap().center;
        assert_eq!([d[0] - c[0], d[1] - c[1]], [-30.0, -10.0]);
        // an indoor playfield has no entry in the coordinates: no marker, the view keeps its place
        own_at(&mut s, 4604, [10.0, 0.0, 10.0], 0.0);
        png(&mut s, &mut o, "planet-4604-no-marker");
        // Shadowlands playfield: the Shadowlands map is picked
        let some_sl = ao_formats::planetmap::PlanetMap::load(&ao_gui::client_dir(), SHADOWLANDS_INDEX).unwrap().coords.keys().copied().find(|k| !ao_formats::planetmap::PlanetMap::load(&ao_gui::client_dir(), RUBIKA_INDEX).unwrap().coords.contains_key(k));
        if let Some(id) = some_sl {
            own_at(&mut s, id, [100.0, 0.0, 100.0], 0.0);
            png(&mut s, &mut o, "planet-shadowlands");
            assert_eq!(s.map.planet.as_ref().unwrap().map.as_ref().unwrap().index.kind, "Shadowlands");
        }
    }
}
