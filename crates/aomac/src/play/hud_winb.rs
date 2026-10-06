//! The Team, Perks, Faction and Pet windows behind one handle for `Hud` (docs/gui.md §11.13; docs/zone/pets.md).

use super::hud::WindowKind;
use super::hud_faction::HudFaction;
use super::hud_perks::HudPerks;
use super::hud_pet::HudPet;
use super::hud_team::HudTeam;
use super::zone::Zone;
use ao_gui::{Event, Gui};
use ao_net::frame::Frame;
use std::path::Path;

pub(super) struct HudWinB {
    pub(super) team: HudTeam,
    pub(super) perks: HudPerks,
    pub(super) faction: HudFaction,
    pub(super) pet: HudPet,
}

impl HudWinB {
    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> Self {
        Self { team: HudTeam::new(dir, screen), perks: HudPerks::new(dir, screen), faction: HudFaction::new(dir, screen), pet: HudPet::new(dir, screen) }
    }

    pub(super) fn set_screen(&mut self, screen: (u32, u32)) {
        self.team.set_screen(screen);
        self.perks.set_screen(screen);
        self.faction.set_screen(screen);
        self.pet.set_screen(screen);
    }

    pub(super) fn open(&mut self, gui: &mut Gui, kind: WindowKind) {
        match kind {
            WindowKind::Team => self.team.open(gui),
            WindowKind::Perks => self.perks.open(gui),
            WindowKind::Faction => self.faction.open(gui),
            WindowKind::Pet => self.pet.open(gui),
            _ => {}
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui, kind: WindowKind) {
        match kind {
            WindowKind::Team => self.team.close(gui),
            WindowKind::Perks => self.perks.close(gui),
            WindowKind::Faction => self.faction.close(gui),
            WindowKind::Pet => self.pet.close(gui),
            _ => {}
        }
    }

    pub(super) fn close_all(&mut self, gui: &mut Gui) {
        for k in [WindowKind::Team, WindowKind::Perks, WindowKind::Faction, WindowKind::Pet] {
            self.close(gui, k);
        }
    }

    /// Every zone frame (team members, invitations, the own perk map, perk updates).
    pub(super) fn on_zone_frame(&mut self, f: &Frame, own: i32) {
        self.team.on_frame(f, own);
        self.perks.on_frame(f, own);
        self.pet.on_frame(f);
    }

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &mut Zone, dt: f32) {
        self.team.update(gui, zone, dt);
        self.perks.update(gui, zone);
        self.faction.update(gui, zone);
        self.pet.update(gui, zone, dt);
    }

    /// `true` when a window of this group consumed the event; the kinds whose frame close button was pressed are returned in `closed`.
    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone, closed: &mut Vec<WindowKind>) -> bool {
        let used = self.team.event(gui, ev, zone) | self.perks.event(gui, ev, zone) | self.faction.event(gui, ev) | self.pet.event(gui, ev, zone);
        for (kind, was) in [(WindowKind::Team, self.team.take_closed()), (WindowKind::Perks, self.perks.take_closed()), (WindowKind::Faction, self.faction.take_closed()), (WindowKind::Pet, self.pet.take_closed())] {
            if was {
                closed.push(kind);
            }
        }
        used
    }

    /// Zone frames to send (leave / kick / transfer / join request / request reply, train / untrain perk).
    pub(super) fn take_outbox(&mut self) -> Vec<Frame> {
        let mut v = std::mem::take(&mut self.team.outbox);
        v.append(&mut self.perks.outbox);
        v.append(&mut self.pet.outbox);
        v
    }

    /// System-window lines (`GlobalSignals+0x17c`) the team window produced.
    pub(super) fn take_lines(&mut self) -> Vec<String> {
        std::mem::take(&mut self.team.lines)
    }
}

/// The frames [`HudWinB::on_zone_frame`] reads: the team messages, `PerkUpdateIIR` and `FullCharacterIIR_t` (the own perk map), by message key.
pub(super) fn wants(f: &Frame) -> bool {
    use ao_net::n3::{team, world};
    f.ptype == ao_net::frame::PT_N3
        && f.payload.get(..4).is_some_and(|k| {
            let k = u32::from_be_bytes([k[0], k[1], k[2], k[3]]);
            [team::TEAM_MEMBER, team::TEAM_MEMBER_INFO, team::TEAM_INVITE, team::PERK_UPDATE, world::FULL_CHARACTER, world::CHARACTER_ACTION, ao_net::n3::misc::BUFF].contains(&k)
        })
}
