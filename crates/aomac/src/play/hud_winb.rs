//! The Team, Perks, Faction and Pet windows behind one handle for `Hud` (docs/gui.md §11.13; docs/zone/pets.md).

use super::hud::WindowKind;
use super::hud_faction::HudFaction;
use super::hud_perks::HudPerks;
use super::hud_pet::HudPet;
use super::hud_team::HudTeam;
use super::zone::Zone;
use ao_gui::{Event, Gui, InputEvent, MouseButton, WindowId, WindowSize};
use ao_net::frame::Frame;
use std::path::Path;

/// `RaidBarView_c` drag state (+0x138). Only real roster-created windows enter this list.
struct RaidBarView {
    window: WindowId,
    drag: Option<(f32, f32)>,
    member: ao_net::msg::Identity,
    team: i32,
    slot: usize,
}

impl RaidBarView {
    /// GUI 101003d0 reads LockRaidBars during drag, not at construction.
    fn input(&mut self, gui: &mut Gui, event: &InputEvent, prefs: &super::dvalue::DValues, screen: (u32, u32)) {
        match *event {
            InputEvent::MouseDown { x, y, button: MouseButton::Left } => {
                let (left, top) = gui.window_pos(self.window);
                let (width, height) = gui.window_size(self.window);
                if x >= left as f32 && y >= top as f32 && x < left as f32 + width as f32 && y < top as f32 + height as f32 {
                    self.drag = Some((x, y));
                }
            }
            InputEvent::MouseUp { button: MouseButton::Left, .. } => self.drag = None,
            InputEvent::MouseMove { x, y } => {
                if let Some((old_x, old_y)) = self.drag {
                    self.drag = Some((x, y));
                    if !prefs.flag("LockRaidBars") {
                        let (left, top) = gui.window_pos(self.window);
                        let (width, height) = gui.window_size(self.window);
                        let left = (left + (x - old_x) as i32).clamp(0, screen.0.saturating_sub(width) as i32);
                        let top = (top + (y - old_y) as i32).clamp(0, screen.1.saturating_sub(height) as i32);
                        gui.set_window_pos(self.window, (left, top));
                    }
                }
            }
            _ => {}
        }
    }
}

pub(super) struct HudWinB {
    pub(super) team: HudTeam,
    pub(super) perks: HudPerks,
    pub(super) faction: HudFaction,
    pub(super) pet: HudPet,
    raid: Option<WindowId>,
    raid_xml: Option<String>,
    screen: (u32, u32),
    raid_outbox: Vec<Frame>,
    raid_bars: Vec<RaidBarView>,
    in_raid: bool,
    raid_members: Vec<ao_net::n3::team::TeamMember>,
    raid_bar_xml: Option<String>,
    raid_locks: Option<Vec<ao_net::n3::team::RaidLock>>,
    raid_dirty: bool,
    raid_config: Option<String>,
    raid_positions: Vec<(i32, i32)>,
}

impl HudWinB {
    pub(super) fn new(dir: &Path, screen: (u32, u32)) -> Self {
        Self { team: HudTeam::new(dir, screen), perks: HudPerks::new(dir, screen), faction: HudFaction::new(dir, screen), pet: HudPet::new(dir, screen),
            raid: None, raid_xml: std::fs::read_to_string(dir.join("cd_image/gui/Default/Views/Raid.xml")).ok(), screen, raid_outbox: vec![], raid_bars: vec![], in_raid: false,
            raid_members: vec![], raid_bar_xml: std::fs::read_to_string(dir.join("cd_image/gui/Default/Views/RaidBar.xml")).ok(), raid_locks: None,
            raid_dirty: true, raid_config: None, raid_positions: vec![] }
    }

    /// GUI10101558/101013f9: genuine roster plus t%dp%dEnabled, never synthetic bars.
    pub(super) fn update_raid(&mut self, gui: &mut Gui, zone: &Zone, prefs: &mut super::dvalue::DValues) {
        if let Some(locks) = &self.raid_locks {
            if locks.is_empty() {
                self.team.lines.push("No raid locks.\n".into());
                self.raid_locks = None;
            } else if let Some(now) = zone.game_seconds() {
                self.team.lines.push("Raid locks:\n".into());
                for lock in locks {
                    let remaining = lock.expires.wrapping_sub(now as u32);
                    if remaining != 0 {
                        let pf = lock.playfield.instance;
                        let name = ao_formats::map_areas::area(pf as u32).map_or("Unknown", |(_, name)| name);
                        self.team.lines.push(format!("{name}({pf}): {} - {}h {}m {}s\n", lock.instance.instance, remaining / 3600, remaining % 3600 / 60, remaining % 60));
                    }
                }
                self.raid_locks = None;
            }
        }
        if !self.in_raid {
            for bar in self.raid_bars.drain(..) { gui.close_window(bar.window); }
            return;
        }
        for bar in &self.raid_bars {
            if let Some(member) = self.team.state.members.iter().find(|m| m.id == bar.member) {
                let fraction = |(value, max): (i32, i32)| if max > 0 { value as f32 / max as f32 } else { 0.0 };
                let (health, nano) = if member.id.instance == zone.char_id as i32 {
                    ((zone.skill_value(27).unwrap_or(0), zone.skill_value(1).unwrap_or(0)), (zone.skill_value(214).unwrap_or(0), zone.skill_value(221).unwrap_or(0)))
                } else {
                    (zone.dynels.get(&member.id.instance).filter(|d| d.max_health > 0).map_or(member.health, |d| (d.health, d.max_health)), member.nano)
                };
                gui.set_progress(bar.window, "health_bar", fraction(health));
                gui.set_progress(bar.window, "nano_bar", fraction(nano));
            }
        }
        let Some(super::dvalue::Variant::Archive(text)) = prefs.get("RaidBarConfig") else { return };
        let moved = self.raid_bars.iter().zip(&self.raid_positions).any(|(bar, old)| bar.drag.is_none() && gui.window_pos(bar.window) != *old);
        if !self.raid_dirty && self.raid_config.as_deref() == Some(text.as_str()) && !moved { return; }
        let Ok(mut config) = ao_gui::xml::parse(text) else { return };
        let Some(source) = &self.raid_bar_xml else { return };
        let mut slots = [0usize; 6];
        let mut wanted = Vec::new();
        if let Some(w) = self.raid {
            for team in 1..=6 {
                for slot in 1..=6 {
                    gui.set_text(w, &format!("t{team}p{slot}_Name"), "");
                    gui.set_text(w, &format!("t{team}p{slot}_Level"), "");
                    gui.set_bitmap(w, &format!("t{team}p{slot}_Prof"), None);
                }
            }
        }
        for member in &self.raid_members {
            let Ok(team) = usize::try_from(member.sub_team) else { continue };
            if team >= 6 || slots[team] >= 6 { continue; }
            let slot = slots[team];
            slots[team] += 1;
            if let Some(w) = self.raid {
                gui.set_text(w, &format!("t{}p{}_Name", team + 1, slot + 1), &member.name);
                gui.set_text(w, &format!("t{}p{}_Level", team + 1, slot + 1), &member.level.to_string());
                let name = format!("t{}p{}_Name", team + 1, slot + 1);
                gui.set_text_color(w, &name, if self.team.state.leader == Some(member.member) { 0x11eeaa } else { 0xffffff });
                let profession = gui.gfx().id(&format!("GFX_GUI_ICON_PROFESSION_{}", member.profession));
                gui.set_bitmap(w, &format!("t{}p{}_Prof", team + 1, slot + 1), profession);
            }
            let enabled = format!("t{team}p{slot}Enabled");
            if !config.children.iter().any(|e| e.attr("name") == Some(enabled.as_str()) && e.attr("value") == Some("true")) { continue; }
            wanted.push((team as i32, slot, member.member));
            if !self.raid_bars.iter().any(|b| b.team == team as i32 && b.slot == slot && b.member == member.member) {
                let xml = source.replace("name=\"profession_icon\"", &format!("name=\"profession_icon\" bitmap_id=\"GFX_GUI_ICON_PROFESSION_{}\"", member.profession));
                match gui.open_window_xml(&format!("RaidBarWindowt{team}p{slot}"), &xml, (0, 0), WindowSize::Preferred) {
                    Ok(window) => {
                        let (width, height) = gui.window_size(window);
                        let mut position = (((self.screen.0 as i32 - width as i32) / 2).max(0), ((self.screen.1 as i32 - height as i32) / 2).max(0));
                        let key = format!("Configt{team}p{slot}");
                        if let Some(saved) = config.children.iter().find(|e| e.attr("name") == Some(key.as_str())) {
                            if let Some(frame) = super::hud_wincfg::Cfg::parse(saved).frame {
                                position = super::hud_wincfg::inside_screen((frame[0] as i32, frame[1] as i32, width, height), self.screen);
                            }
                        }
                        gui.set_window_pos(window, position);
                        self.raid_bars.push(RaidBarView { window, drag: None, member: member.member, team: team as i32, slot });
                    }
                    Err(e) => eprintln!("raid bar: {e:#}"),
                }
            }
        }
        self.raid_bars.retain(|bar| {
            let keep = wanted.contains(&(bar.team, bar.slot, bar.member));
            if !keep { gui.close_window(bar.window); }
            keep
        });
        let mut changed = false;
        for bar in &self.raid_bars {
            if bar.drag.is_none() && gui.window_moved(bar.window) {
                let key = format!("Configt{}p{}", bar.team, bar.slot);
                if !config.children.iter().any(|e| e.attr("name") == Some(key.as_str())) {
                    if let Ok(saved) = ao_gui::xml::parse(&format!("<Archive name=\"{key}\" code=\"0\"/>")) {
                        config.children.push(saved);
                    }
                }
                if let Some(saved) = config.children.iter_mut().find(|e| e.attr("name") == Some(key.as_str())) {
                    if let Some((x, y, width, height)) = gui.window_outer_frame(bar.window) {
                        let frame = [x as f32, y as f32, (x + width as i32 - 1) as f32, (y + height as i32 - 1) as f32];
                        let mut cfg = super::hud_wincfg::Cfg::parse(saved);
                        if cfg.frame != Some(frame) {
                            cfg.frame = Some(frame);
                            cfg.write_into(saved);
                            changed = true;
                        }
                    }
                }
            }
        }
        if changed { prefs.set("RaidBarConfig", super::dvalue::Variant::Archive(super::dvalue::element_xml(&config))); }
        self.raid_dirty = false;
        self.raid_config = match prefs.get("RaidBarConfig") { Some(super::dvalue::Variant::Archive(t)) => Some(t.clone()), _ => None };
        self.raid_positions.clear();
        self.raid_positions.extend(self.raid_bars.iter().map(|bar| gui.window_pos(bar.window)));
    }

    pub(super) fn raid_bar_input(&mut self, gui: &mut Gui, event: &InputEvent, prefs: &super::dvalue::DValues) {
        for bar in &mut self.raid_bars {
            bar.input(gui, event, prefs, self.screen);
        }
    }

    pub(super) fn set_screen(&mut self, screen: (u32, u32)) {
        self.team.set_screen(screen);
        self.perks.set_screen(screen);
        self.faction.set_screen(screen);
        self.pet.set_screen(screen);
        self.screen = screen;
    }

    pub(super) fn open(&mut self, gui: &mut Gui, kind: WindowKind) {
        match kind {
            WindowKind::Team => self.team.open(gui),
            WindowKind::Perks => self.perks.open(gui),
            WindowKind::Faction => self.faction.open(gui),
            WindowKind::Pet => self.pet.open(gui),
            WindowKind::Raid => {
                // GUI10101558 ->101013f9 creates configured bars only after real
                // RaidCreated/roster data; no bars are synthesized before that stage.
                if self.raid.is_some() { return; }
                let Some(xml) = &self.raid_xml else { return };
                match gui.open_tabbed_window_xml("Raid", "Raid", xml, (0, 0), WindowSize::Preferred) {
                    Ok(w) => {
                        let (width, height) = gui.window_size(w);
                        gui.set_window_pos(w, (((self.screen.0 as i32 - width as i32) / 2).max(0), ((self.screen.1 as i32 - height as i32) / 2).max(0)));
                        // GUI 10102b79: not in a team -> conversion disabled.
                        gui.set_enabled(w, "CreateRaid", self.team.state.in_team() && !self.in_raid);
                        self.raid = Some(w);
                        self.raid_dirty = true;
                    }
                    Err(e) => eprintln!("hud: raid_window: {e:#}"),
                }
            }
            _ => {}
        }
    }

    pub(super) fn close(&mut self, gui: &mut Gui, kind: WindowKind) {
        match kind {
            WindowKind::Team => self.team.close(gui),
            WindowKind::Perks => self.perks.close(gui),
            WindowKind::Faction => self.faction.close(gui),
            WindowKind::Pet => self.pet.close(gui),
            WindowKind::Raid => {
                if let Some(w) = self.raid.take() {
                    gui.close_window(w);
                }
            }
            _ => {}
        }
    }

    pub(super) fn close_all(&mut self, gui: &mut Gui) {
        for k in [WindowKind::Team, WindowKind::Perks, WindowKind::Faction, WindowKind::Pet, WindowKind::Raid] {
            self.close(gui, k);
        }
        for bar in self.raid_bars.drain(..) { gui.close_window(bar.window); }
        self.in_raid = false;
        self.raid_members.clear();
    }

    /// Every zone frame (team members, invitations, the own perk map, perk updates).
    pub(super) fn on_zone_frame(&mut self, f: &Frame, zone: &mut Zone) {
        let own = zone.char_id as i32;
        self.team.on_frame(f, own);
        self.perks.on_frame(f, zone);
        if let Ok(message) = ao_net::n3::decode(f) {
            if message.header.target.instance == own {
                if let ao_net::n3::N3::Unknown(body) = message.body {
                    if matches!(message.header.msg_type, ao_net::n3::team::RAID | ao_net::n3::team::TEAM_MEMBER) { self.raid_dirty = true; }
                    match message.header.msg_type {
                        ao_net::n3::team::RAID => match ao_net::n3::team::parse_raid(&body) {
                            Ok(ao_net::n3::team::RaidEvent::Created) => {
                                self.in_raid = true;
                                // GC100a3b6a switches the existing ordinary team to raid group0.
                                for member in &mut self.raid_members {
                                    if member.sub_team < 0 { member.sub_team = 0; }
                                }
                            }
                            Ok(ao_net::n3::team::RaidEvent::Locks(locks)) => self.raid_locks = Some(locks),
                            Err(e) => eprintln!("raid: {e:#}"),
                        },
                        ao_net::n3::team::TEAM_MEMBER => match ao_net::n3::team::parse_team_member(&body) {
                            Ok(member) => {
                                if member.team.instance != 0 {
                                    // GC100657e9: real own team-index >=0 means raid (also on relog).
                                    if member.member.instance == own { self.in_raid = member.sub_team >= 0; }
                                    if let Some(old) = self.raid_members.iter_mut().find(|m| m.member == member.member) {
                                        *old = member;
                                    } else {
                                        self.raid_members.push(member);
                                    }
                                } else if member.member.instance == own {
                                    self.in_raid = false;
                                    self.raid_members.clear();
                                } else {
                                    self.raid_members.retain(|m| m.member != member.member);
                                }
                            }
                            Err(e) => eprintln!("raid member: {e:#}"),
                        },
                        _ => {}
                    }
                }
            }
        }
    }

    pub(super) fn update(&mut self, gui: &mut Gui, zone: &mut Zone, dt: f32) {
        self.team.update(gui, zone, dt);
        self.perks.update(gui, zone);
        self.faction.update(gui, zone);
        self.pet.update(gui, zone, dt);
        if let Some(w) = self.raid {
            gui.set_enabled(w, "CreateRaid", self.team.state.in_team() && !self.in_raid);
        }
    }

    /// `true` when a window of this group consumed the event; the kinds whose frame close button was pressed are returned in `closed`.
    pub(super) fn event(&mut self, gui: &mut Gui, ev: &Event, zone: &Zone, closed: &mut Vec<WindowKind>) -> bool {
        if matches!(ev, Event::CloseRequested { window } if Some(*window) == self.raid) {
            self.close(gui, WindowKind::Raid);
            closed.push(WindowKind::Raid);
            return true;
        }
        if let Event::Clicked { window, view, .. } = ev {
            if Some(*window) == self.raid {
                let command = match view.as_str() {
                    "CreateRaid" if self.team.state.is_leader() => Some(1),
                    "ListLocks" => Some(6),
                    _ => None,
                };
                if let Some(command) = command {
                    let zero = ao_net::msg::Identity::default();
                    self.raid_outbox.push(ao_net::n3::outgoing::n3_frame(0, zone.char_id, ao_net::n3::textcmd::raid_cmd(zone.char_id as i32, command, zero, zero, zero)));
                    return true;
                }
            }
        }
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
        v.append(&mut self.raid_outbox);
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
            [team::TEAM_MEMBER, team::TEAM_MEMBER_INFO, team::TEAM_INVITE, team::PERK_UPDATE, world::FULL_CHARACTER, world::CHARACTER_ACTION, ao_net::n3::misc::BUFF, ao_net::n3::outgoing::message_key("RaidIIR_c")].contains(&k)
        })
}

#[cfg(test)]
mod raid_tests {
    use super::*;

    #[test]
    fn native_created_reply_enables_raid_state() {
        let dir = ao_gui::client_dir();
        let mut windows = HudWinB::new(&dir, (1280, 800));
        let mut wire = ao_net::wire::Writer::default();
        wire.u32(ao_net::n3::outgoing::message_key("RaidIIR_c"));
        ao_net::msg::Identity { kind: 50000, instance: 7 }.write(&mut wire);
        wire.u8(1);
        wire.u16(0);
        let frame = ao_net::n3::outgoing::n3_frame(0, 7, wire.0);
        assert!(wants(&frame));
        windows.on_zone_frame(&frame, &mut Zone::new(7));
        assert!(windows.in_raid);
        if dir.join("cd_image/gui/Default/Views/RaidBar.xml").exists() {
            let mut gui = Gui::new(&dir, None).unwrap();
            let mut prefs = super::super::dvalue::DValues::new(&dir);
            prefs.set("RaidBarConfig", super::super::dvalue::Variant::Archive("<Archive name=\"RaidBarConfig\" code=\"0\"><Bool name=\"t0p0Enabled\" value=\"true\"/></Archive>".into()));
            windows.update_raid(&mut gui, &Zone::default(), &mut prefs);
            assert!(windows.raid_bars.is_empty(), "no invented roster");
            let own = ao_net::msg::Identity { kind: 50000, instance: 7 };
            let member = ao_net::n3::team::TeamMember { member: own, team: ao_net::msg::Identity { kind: ao_net::n3::team::TEAM_KIND, instance: 1 }, sub_team: 0, profession: 1, level: 1, name: "Native fixture".into() };
            windows.on_zone_frame(&ao_net::n3::outgoing::n3_frame(0, 7, ao_net::n3::team::team_member(own, &member)), &mut Zone::new(7));
            windows.update_raid(&mut gui, &Zone::default(), &mut prefs);
            assert_eq!(windows.raid_bars.len(), 1);
            assert_eq!(windows.raid_bars[0].member, own);
        }
    }

    #[test]
    fn native_raid_bar_drag_reads_lock_at_each_move() {
        let dir = ao_gui::client_dir();
        let Ok(xml) = std::fs::read_to_string(dir.join("cd_image/gui/Default/Views/RaidBar.xml")) else { return };
        let mut gui = Gui::new(&dir, None).unwrap();
        let window = gui.open_window_xml("RaidBarView", &xml, (100, 100), WindowSize::Preferred).unwrap();
        let mut bar = RaidBarView { window, drag: None, member: Default::default(), team: 0, slot: 0 };
        let mut prefs = super::super::dvalue::DValues::new(&dir);
        prefs.set_i64("LockRaidBars", 1);
        bar.input(&mut gui, &InputEvent::MouseDown { x: 101.0, y: 101.0, button: MouseButton::Left }, &prefs, (1280, 800));
        bar.input(&mut gui, &InputEvent::MouseMove { x: 111.0, y: 111.0 }, &prefs, (1280, 800));
        assert_eq!(gui.window_pos(window), (100, 100));
        prefs.set_i64("LockRaidBars", 0);
        bar.input(&mut gui, &InputEvent::MouseMove { x: 121.0, y: 121.0 }, &prefs, (1280, 800));
        assert_eq!(gui.window_pos(window), (110, 110));
        bar.input(&mut gui, &InputEvent::MouseUp { x: 121.0, y: 121.0, button: MouseButton::Left }, &prefs, (1280, 800));
        bar.input(&mut gui, &InputEvent::MouseMove { x: 131.0, y: 131.0 }, &prefs, (1280, 800));
        assert_eq!(gui.window_pos(window), (110, 110));
    }
}
