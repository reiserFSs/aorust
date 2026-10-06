//! Replays of the live captures behind docs/zone/interact.md (PRK "Ithaca", 2026-10-06; frames only, no login traffic):
//! `zone_npc_dialogue_ithaca.rec` (Neutral Observer dialogue) and `zone_use_object_ithaca.rec` (use of a teleporter with the key item,
//! the vending machine, a corpse). Format `<idx> <dir> <hex frame>`, `<` = server -> client.

use ao_net::frame::Frame;
use ao_net::msg::Identity;
use ao_net::n3::chat::N3Chat;
use ao_net::n3::inventory::{self, InventoryMsg};
use ao_net::n3::knubot::Knubot;
use ao_net::n3::misc::{GenericArgs, Misc};
use ao_net::n3::trade::Trade;
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

#[test]
fn neutral_observer_dialogue() {
    let observer = Identity { kind: 50000, instance: 1000191 };
    let mut seen = vec![];
    for (_, from_server, f) in load(include_str!("../../../docs/captures/zone_npc_dialogue_ithaca.rec")) {
        let m = n3::decode(&f).unwrap();
        let N3::Knubot(k) = m.body else { panic!("not a Knubot message: {:?}", m.body) };
        assert_eq!(k.npc(), observer);
        // the codec writes back exactly the captured bytes
        let mut again = k.encode(m.header.target);
        again[12] = m.header.flag; // the server sends 1 (`ClearToBePassedOn` false), the client 0
        assert_eq!(again, f.payload, "{k:?}");
        assert!(!from_server || !matches!(k, Knubot::Answer { .. }), "the answer is the client's");
        seen.push(k);
    }
    assert!(matches!(seen[0], Knubot::Open { .. }), "{:?}", seen[0]);
    assert!(seen.iter().any(|k| matches!(k, Knubot::AppendText { text, .. } if text.contains("access card"))));
    assert!(seen.iter().any(|k| matches!(k, Knubot::AnswerList { answers, .. } if answers.last().is_some_and(|a| a == "Goodbye."))));
    assert!(seen.iter().filter(|k| matches!(k, Knubot::Answer { .. })).count() >= 2);
}

#[test]
fn use_key_on_teleporter_and_machine_and_corpse() {
    let frames = load(include_str!("../../../docs/captures/zone_use_object_ithaca.rec"));
    let (mut cmd5, mut texts, mut trades, mut loot, mut takes) = (0, vec![], 0, None, 0);
    for (_, from_server, f) in &frames {
        let m = n3::decode(f).unwrap();
        match m.body {
            // `N3Msg_UseItemOnItem` (GenericCmd 5): actor, the key (bag slot 0x44) and the teleporter
            N3::Misc(Misc::GenericCmd(c)) if c.cmd == 5 => {
                let GenericArgs::ItemOnItem { actor, item, target, .. } = c.args else { panic!("{c:?}") };
                assert_eq!((actor, item), (Identity { kind: 50000, instance: OWN }, inventory::item_identity(0x44)));
                assert_eq!(target.kind, 51005);
                assert!(!from_server);
                cmd5 += 1;
            }
            N3::Misc(Misc::GenericCmd(c)) => assert_eq!(c.cmd, 3, "{c:?}"),
            N3::Trade(Trade { .. }) => trades += 1,
            N3::Inventory(InventoryMsg::Update(u)) => loot = Some(u),
            // `FormatFeedbackIIR_t`: the `RemoteFormat` text of the refusal
            N3::Chat(N3Chat::Format(f)) => texts.push(String::from_utf8_lossy(&f.format).into_owned()),
            N3::Unknown(_) => takes += (f.payload[..4] == 0x5469373fu32.to_be_bytes()) as i32,
            _ => {}
        }
    }
    assert_eq!(cmd5, 3);
    // the refusals of the other teleporters ("You can not use this item on the teleporter", "This is not the correct key") are in no capture; the one
    // text here is the teleport feedback (the format `~&!!!":!,ViAi!!!!!~`, no string argument)
    assert_eq!(texts.len(), 1, "{texts:?}");
    assert_eq!(trades, 2, "the machine's TradeIIR_t pair");
    let u = loot.expect("the corpse's InventoryUpdate");
    assert_eq!(u.entries.iter().map(|e| e.item.low_id).collect::<Vec<_>>(), [269202, 284201]);
    assert_eq!(takes, 1);
    // the take is `MoveItemToInventory({0x6a, cell 0}, any bag slot)`
    let take = frames.iter().find(|(_, _, f)| f.payload[..4] == 0x5469373fu32.to_be_bytes()).unwrap();
    assert_eq!(take.2.payload, inventory::move_item_to_inventory(OWN, Identity { kind: 0x6a, instance: 0 }, 0x6f));
}
