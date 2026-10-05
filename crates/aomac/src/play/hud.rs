//! In-world interface (`ControlCenterModule_c`, GUI.dll 0x1006c64d): the control-centre overlay `Views/ControlCenter.xml`
//! with its dock views, the window-opening menus and the sub-windows the other Hud files own. Evidence: docs/gui.md §10.

use super::zone::Zone;
use ao_gui::{Event, Gui, WindowId, WindowSize};

/// Windows the control centre opens (`name=` of the `ActionMenu/*.xml` entries, which are also the `dvalue` names in `CharPrefs.xml`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WindowKind {
    Skills,
    Inventory,
    /// `wear_window`
    Character,
    Target,
    /// `map_window` (playfield map); the planet map is `planetmap_window`.
    Map,
    PlanetMap,
    Nano,
    Ncu,
    Mission,
    Friends,
    Team,
    Perks,
    Faction,
}

impl WindowKind {
    /// The dvalue / menu entry name (`ActionMenu/*.xml` `name=`).
    pub fn dvalue(self) -> &'static str {
        match self {
            WindowKind::Skills => "skill_window",
            WindowKind::Inventory => "inventory_window",
            WindowKind::Character => "wear_window",
            WindowKind::Target => "target_window",
            WindowKind::Map => "map_window",
            WindowKind::PlanetMap => "planetmap_window",
            WindowKind::Nano => "nano_window",
            WindowKind::Ncu => "ncu_window",
            WindowKind::Mission => "mission_window",
            WindowKind::Friends => "friends_window",
            WindowKind::Team => "team_view",
            WindowKind::Perks => "perk_window",
            WindowKind::Faction => "faction_window",
        }
    }
}

pub(super) struct Hud {
    /// The full-screen `ControlCenter.xml` window.
    cc: WindowId,
    size: (u32, u32),
    open: Vec<WindowKind>,
}

impl Hud {
    pub(super) fn new(gui: &mut Gui, size: (u32, u32)) -> anyhow::Result<Self> {
        let cc = gui.open_window("ControlCenter", (0, 0), WindowSize::Fixed(size.0, size.1))?;
        Ok(Hud { cc, size, open: vec![] })
    }

    pub(super) fn resize(&mut self, gui: &mut Gui, size: (u32, u32)) {
        if size != self.size {
            self.size = size;
            gui.resize_window(self.cc, WindowSize::Fixed(size.0, size.1));
        }
    }

    pub(super) fn update(&mut self, _gui: &mut Gui, _zone: &Zone, _dt: f32) {}

    /// `true` when the event was consumed by the HUD.
    pub(super) fn event(&mut self, _gui: &mut Gui, _ev: &Event, _zone: &Zone) -> bool {
        false
    }

    pub(super) fn open(&mut self, kind: WindowKind) {
        if !self.open.contains(&kind) {
            self.open.push(kind);
        }
    }

    pub(super) fn is_open(&self, kind: WindowKind) -> bool {
        self.open.contains(&kind)
    }

    /// Closes the HUD windows (leaving the world).
    pub(super) fn close(self, gui: &mut Gui) {
        gui.close_window(self.cc);
    }
}
