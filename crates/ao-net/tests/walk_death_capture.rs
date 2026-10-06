//! `docs/captures/zone_walk_death_borealis.rec`: the live `goto` walk of Aomacvolk (lvl 2) through Borealis (pf 800) that ended in a
//! `ZoneRedirection` to the same playfield around (720, 32, 360) (PRK "Ithaca", 2026-10-06; `<idx> <dir> <hex frame>`, `>` = client -> server).
//! It is the death/respawn flow (docs/zone/combat.md), not a rejection of our movement: the hostile camp on the route attacked the unarmed
//! walker (`AttackIIR_t`, `CharacterAction` 99 = Died), ~2.5 s later the server sent "Locating next playfield server", the redirection and
//! the teleport to the start (679.6, 72.8, 476.7). Excerpt: our `CharDCMove` stream from t >= 195 s and the server frames of the last seconds.

use ao_net::frame::{Frame, PT_N3, PT_SYSTEM};
use ao_net::msg::Message;
use ao_net::n3::dynel::Dynel;
use ao_net::n3::world::World;
use ao_net::n3::{self, N3};

fn load(text: &str) -> Vec<(u32, bool, Frame)> {
    text.lines()
        .map(|l| {
            let mut p = l.split(' ');
            let idx = p.next().unwrap().parse().unwrap();
            let (dir, hex) = (p.next().unwrap(), p.next().unwrap());
            let b: Vec<u8> = (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect();
            (idx, dir == "<", Frame::decode_with(&b, false).unwrap().unwrap().0)
        })
        .collect()
}

const OWN: i32 = 0x82e8;
/// `AttackIIR_t`.
const ATTACK: u32 = 0x2849_4070;

#[test]
fn walk_ends_in_the_death_respawn_not_a_movement_rejection() {
    let frames = load(include_str!("../../../docs/captures/zone_walk_death_borealis.rec"));
    // our stream: only movement-FSM ids (1..8 forward/back/strafe/turn start/stop, 0x16 sync), a smooth path (<= 13 m/s run speed), no teleports
    let mut last: Option<([f32; 3], i32)> = None;
    let (mut moves, mut syncs, mut at) = (0, 0, 0.0f32);
    for (_, _, f) in frames.iter().filter(|f| !f.1) {
        let m = n3::decode(f).unwrap();
        let N3::Dynel(Dynel::CharDCMove(c)) = m.body else { panic!("not a CharDCMove: {:?}", m.body) };
        assert_eq!(m.header.target.instance, OWN);
        assert!(matches!(c.move_type, 1..=8 | 0x16), "move type {:#x}", c.move_type);
        assert_eq!(c.extra, [0.0, 0.0]);
        assert!(c.time >= 0);
        if let Some((p, _)) = last {
            let step = ((c.pos[0] - p[0]).powi(2) + (c.pos[2] - p[2]).powi(2)).sqrt();
            at += c.time as f32 / 1000.0;
            assert!(step <= 13.0 * (c.time as f32 / 1000.0) + 0.5, "teleport-like step {step} m in {} ms", c.time);
        }
        last = Some((c.pos, c.time));
        moves += 1;
        syncs += (c.move_type == 0x16) as u32;
    }
    assert!(moves > 90 && at > 15.0, "{moves} moves over {at} s");
    // zone-border syncs only (FUN_1005a5d6), never the 5 s idle sync while the walk keeps sending
    assert!(syncs <= 6, "{syncs} syncs");

    // the server side: attacks, Died, the "Locating" line, the same-playfield redirection, the teleport to the start
    let (mut died, mut attacks, mut locating, mut redirect, mut teleport) = (None, 0, None, None, None);
    for (i, (_, _, f)) in frames.iter().enumerate().filter(|f| f.1 .1) {
        if f.ptype == PT_SYSTEM {
            let Message::ZoneRedirection { ip, port } = Message::from_frame(f).unwrap() else { panic!("system frame") };
            assert_eq!((ip.to_string().as_str(), port), ("199.241.136.157", 8502), "the same zone server");
            redirect = Some(i);
            continue;
        }
        assert_eq!(f.ptype, PT_N3);
        let m = n3::decode(f).unwrap();
        attacks += (m.header.msg_type == ATTACK) as u32;
        match m.body {
            N3::World(World::CharacterAction(a)) if a.action == 99 => died = Some(i),
            N3::Chat(n3::chat::N3Chat::Text(t)) if t.text.starts_with(b"Locating") => locating = Some(i),
            N3::Teleport(t) => {
                assert!(t.is_zone_change());
                assert_eq!(t.pos.map(|v| (v * 10.0).round() / 10.0), [679.6, 72.8, 476.7]);
                teleport = Some(i);
            }
            _ => {}
        }
    }
    let (died, locating, teleport, redirect) = (died.expect("Died"), locating.expect("Locating"), teleport.expect("teleport"), redirect.expect("redirect"));
    assert!(attacks >= 1, "the walker was attacked");
    assert!(died < locating && locating < teleport && teleport < redirect, "{died} {locating} {teleport} {redirect}");
}
