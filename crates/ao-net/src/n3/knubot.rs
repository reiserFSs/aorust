//! NPC dialogue (`Knubot*IIR_c`, Gamecode.dll): the messages behind the NPC chat window. Layouts and addresses: docs/zone/interact.md.
//!
//! Every Knubot message derives from `KnubotBaseIIR_c` (ctor `FUN_1012782a` [GC]): the N3 header target is the sender's own character
//! identity, the "to be passed on" byte is **0** (`ClearToBePassedOn`), and the body starts with `FUN_101278d6` (write) /
//! `FUN_10127893` (read): `i16 2`, the NPC [`Identity`] (`+0x18`). Strings are `i32 len` + raw bytes (`BinaryStream::operator<<(uint)` +
//! `write`), no terminator.

use super::N3Header;
use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};

/// `KnubotOpenChatWindowIIR_c` read `FUN_1000ba62` [GC 0x1000df58]; `Activate` `FUN_10127ea0` emits GlobalSignals `+0xec`.
pub const OPEN_CHAT_WINDOW: u32 = 0x3B13_2D64;
/// `KnubotCloseChatWindowIIR_c`; `Activate` `FUN_10127a59` emits `+0xf0`.
pub const CLOSE_CHAT_WINDOW: u32 = 0x270A_4C62;
/// `KnubotAppendTextIIR_c`; `Activate` `FUN_101275d3` emits `+0xf4`.
pub const APPEND_TEXT: u32 = 0x5D70_532A;
/// `KnubotAnswerListIIR_c`; `Activate` `FUN_10127228` emits `+0xf8`.
pub const ANSWER_LIST: u32 = 0x5570_4D31;
/// `KnubotAnswerIIR_c` (client -> server); `Activate` `FUN_101271a3` does nothing.
pub const ANSWER: u32 = 0x2103_247D;
/// `KnubotNPCDescriptionIIR_c` (client -> server).
pub const NPC_DESCRIPTION: u32 = 0x000A_0C5A;
/// `KnubotStartTradeIIR_c`; `Activate` `FUN_10128554` emits `+0xfc`.
pub const START_TRADE: u32 = 0x7864_401D;
/// `KnubotFinishTradeIIR_c`; `Activate` does nothing.
pub const FINISH_TRADE: u32 = 0x5568_2B24;
/// `KnubotTradeIIR_c`; `Activate` does nothing.
pub const TRADE: u32 = 0x3A1B_2C0C;
/// `KnubotRejectedItemsIIR_c`; `Activate` `FUN_101281c6` re-lays the inventory and emits `+0x100`.
pub const REJECTED_ITEMS: u32 = 0x2D21_2407;

/// `AppendText` text types (`NPCChatTextType_e`, the switch of the window's `FUN_100586fd` [GUI]).
pub mod text {
    /// NPC speech (`CCNPCChatText`).
    pub const SPEECH: i32 = 0;
    /// Out of character (`CCNPCOOCText`).
    pub const OOC: i32 = 1;
    /// The player's own question, echoed (`CCNPCChatQuestion`).
    pub const QUESTION: i32 = 2;
    /// System line (`CCNPCChatSystem`).
    pub const SYSTEM: i32 = 3;
    /// Emote (`CCNPCChatEmote`).
    pub const EMOTE: i32 = 4;
    /// Description (`CCNPCChatDescription`).
    pub const DESCRIPTION: i32 = 5;
    /// Plain text without the `<br>` of the first line (`CCNPCChatText`).
    pub const PLAIN: i32 = 6;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Knubot {
    /// `b20` = body's second `i32 == 1`, `b21` = its first (`+0x21`, `+0x20`); they enable two buttons of the window's button bar.
    Open { npc: Identity, b20: bool, b21: bool },
    /// `value` is `+0x20` (sign-extended to the int64 the signal passes), `text` the string.
    Close { npc: Identity, value: i32, text: String },
    AppendText { npc: Identity, kind: i32, text: String },
    AnswerList { npc: Identity, answers: Vec<String> },
    StartTrade { npc: Identity, value: i32, text: String },
    FinishTrade { npc: Identity, flag: bool, value: i32 },
    Trade { npc: Identity, op: i32, a: Identity, b: Identity },
    RejectedItems { npc: Identity, items: Vec<(Identity, i32, i32)>, value: i32 },
    Answer { npc: Identity, index: i32 },
    Description { npc: Identity },
}

impl Knubot {
    pub fn npc(&self) -> Identity {
        match *self {
            Knubot::Open { npc, .. }
            | Knubot::Close { npc, .. }
            | Knubot::AppendText { npc, .. }
            | Knubot::AnswerList { npc, .. }
            | Knubot::StartTrade { npc, .. }
            | Knubot::FinishTrade { npc, .. }
            | Knubot::Trade { npc, .. }
            | Knubot::RejectedItems { npc, .. }
            | Knubot::Answer { npc, .. }
            | Knubot::Description { npc } => npc,
        }
    }

    pub fn key(&self) -> u32 {
        match self {
            Knubot::Open { .. } => OPEN_CHAT_WINDOW,
            Knubot::Close { .. } => CLOSE_CHAT_WINDOW,
            Knubot::AppendText { .. } => APPEND_TEXT,
            Knubot::AnswerList { .. } => ANSWER_LIST,
            Knubot::StartTrade { .. } => START_TRADE,
            Knubot::FinishTrade { .. } => FINISH_TRADE,
            Knubot::Trade { .. } => TRADE,
            Knubot::RejectedItems { .. } => REJECTED_ITEMS,
            Knubot::Answer { .. } => ANSWER,
            Knubot::Description { .. } => NPC_DESCRIPTION,
        }
    }

    /// The full N3 payload: header (`target` = the character the message is for / from, flag byte 0) and the body in the order of the
    /// classes' `Write` slots.
    pub fn encode(&self, target: Identity) -> Vec<u8> {
        let mut w = Writer::default();
        w.u32(self.key());
        target.write(&mut w);
        w.u8(0);
        w.i16(2);
        self.npc().write(&mut w);
        match self {
            // `FUN_10127e51`: `+0x21` first, then `+0x20`
            Knubot::Open { b20, b21, .. } => {
                w.i32(*b21 as i32);
                w.i32(*b20 as i32);
            }
            Knubot::Close { value, text, .. } => {
                w.i32(*value);
                w.str_i32(text);
            }
            Knubot::AppendText { kind, text, .. } => {
                w.i32(*kind);
                w.str_i32(text);
            }
            Knubot::AnswerList { answers, .. } => {
                w.i32(answers.len() as i32);
                for a in answers {
                    w.str_i32(a);
                }
            }
            Knubot::StartTrade { value, text, .. } => {
                w.i32(*value);
                w.str_i32(text);
            }
            Knubot::FinishTrade { flag, value, .. } => {
                w.i32(*flag as i32);
                w.i32(*value);
            }
            Knubot::Trade { op, a, b, .. } => {
                w.i32(*op);
                a.write(&mut w);
                b.write(&mut w);
            }
            Knubot::RejectedItems { items, value, .. } => {
                w.i32(items.len() as i32);
                for (id, x, y) in items {
                    id.write(&mut w);
                    w.i32(*x);
                    w.i32(*y);
                }
                w.i32(*value);
            }
            Knubot::Answer { index, .. } => w.i32(*index),
            Knubot::Description { .. } => {}
        }
        w.0
    }
}

/// `N3Msg_DefaultActionOnDynel` [GC 0x100291da] on a character with the NPC dialogue flag: `KnubotOpenChatWindowIIR_c(own, npc, 0, 0)` (`FUN_10127f4d`).
pub fn open_chat_window(own: Identity, npc: Identity) -> Vec<u8> {
    Knubot::Open { npc, b20: false, b21: false }.encode(own)
}

/// `N3Msg_SendNPCChatAnswer` [GC 0x1001902a]: the clicked link's index into the last answer list.
pub fn answer(own: Identity, npc: Identity, index: i32) -> Vec<u8> {
    Knubot::Answer { npc, index }.encode(own)
}

/// `N3Msg_NPCChatCloseWindow` [GC 0x1001ce62]: value 0, empty string.
pub fn close_window(own: Identity, npc: Identity) -> Vec<u8> {
    Knubot::Close { npc, value: 0, text: String::new() }.encode(own)
}

/// `N3Msg_NPCChatRequestDescription` [GC 0x10017e56].
pub fn request_description(own: Identity, npc: Identity) -> Vec<u8> {
    Knubot::Description { npc }.encode(own)
}

/// `N3Msg_NPCChatStartTrade` [GC 0x1001cee9]: value 0, empty string.
pub fn start_trade(own: Identity, npc: Identity) -> Vec<u8> {
    Knubot::StartTrade { npc, value: 0, text: String::new() }.encode(own)
}

/// `N3Msg_NPCChatEndTrade` [GC 0x10017fb1]: `flag` is `!param_4` (true when the trade is finished by the player's accept, false when aborted),
/// `value` is `param_3` (the money / item count).
pub fn end_trade(own: Identity, npc: Identity, value: i32, flag: bool) -> Vec<u8> {
    Knubot::FinishTrade { npc, flag, value }.encode(own)
}

/// `N3Msg_NPCChatAddTradeItem` / `RemoveTradeItem` [GC 0x10017ea0 / 0x10017f31]: `op` 0 add, 1 remove; the first identity stays zero.
pub fn trade_item(own: Identity, npc: Identity, op: i32, item: Identity) -> Vec<u8> {
    Knubot::Trade { npc, op, a: Identity::default(), b: item }.encode(own)
}

fn string(r: &mut Reader, max: i32) -> Result<String> {
    let n = r.i32()?;
    if !(0..=max).contains(&n) {
        bail!("Knubot string length {n} out of range");
    }
    Ok(String::from_utf8_lossy(r.bytes(n as usize)?).into_owned())
}

/// Decode a Knubot body; `Ok(None)` when `h.msg_type` is not a Knubot message.
pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<Knubot>> {
    if !matches!(h.msg_type, OPEN_CHAT_WINDOW | CLOSE_CHAT_WINDOW | APPEND_TEXT | ANSWER_LIST | ANSWER | NPC_DESCRIPTION | START_TRADE | FINISH_TRADE | TRADE | REJECTED_ITEMS) {
        return Ok(None);
    }
    // `FUN_10127893`: only version 2 carries the identity; anything else leaves it zero and skips the identity read
    let ver = r.i16()?;
    let npc = if ver == 2 { Identity::read(r)? } else { Identity::default() };
    Ok(Some(match h.msg_type {
        OPEN_CHAT_WINDOW => {
            let b21 = r.i32()? == 1;
            let b20 = r.i32()? == 1;
            Knubot::Open { npc, b20, b21 }
        }
        // `FUN_10127bdb`: length < 0x3e9; the original does not reject negatives, a negative length would be a bad stream
        CLOSE_CHAT_WINDOW => Knubot::Close { npc, value: r.i32()?, text: string(r, 1000)? },
        // `FUN_101276d3`: 0..=10000
        APPEND_TEXT => Knubot::AppendText { npc, kind: r.i32()?, text: string(r, 10000)? },
        // `FUN_10127416`: 1..=1000 entries of 1..=1000 bytes
        ANSWER_LIST => {
            let n = r.i32()?;
            if !(1..=1000).contains(&n) {
                bail!("Knubot answer count {n} out of range");
            }
            let mut answers = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let s = string(r, 1000)?;
                if s.is_empty() {
                    bail!("empty Knubot answer");
                }
                answers.push(s);
            }
            Knubot::AnswerList { npc, answers }
        }
        ANSWER => Knubot::Answer { npc, index: r.i32()? },
        NPC_DESCRIPTION => Knubot::Description { npc },
        // `FUN_101286e5`: 1..=10000
        START_TRADE => {
            let value = r.i32()?;
            let n = r.i32()?;
            if !(1..=10000).contains(&n) {
                bail!("Knubot trade text length {n} out of range");
            }
            Knubot::StartTrade { npc, value, text: String::from_utf8_lossy(r.bytes(n as usize)?).into_owned() }
        }
        FINISH_TRADE => Knubot::FinishTrade { npc, flag: r.i32()? == 1, value: r.i32()? },
        TRADE => Knubot::Trade { npc, op: r.i32()?, a: Identity::read(r)?, b: Identity::read(r)? },
        REJECTED_ITEMS => {
            let n = r.i32()?;
            if !(0..=100_000).contains(&n) {
                bail!("Knubot rejected item count {n} out of range");
            }
            let mut items = Vec::with_capacity((n as usize).min(1024));
            for _ in 0..n {
                items.push((Identity::read(r)?, r.i32()?, r.i32()?));
            }
            Knubot::RejectedItems { npc, items, value: r.i32()? }
        }
        _ => unreachable!(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::outgoing::message_key;

    const OWN: Identity = Identity { kind: 0xC350, instance: 0x82e8 };
    const NPC: Identity = Identity { kind: 0xC350, instance: 4711 };

    fn round(k: Knubot) {
        let b = k.encode(OWN);
        let (h, mut r) = N3Header::parse(&b).unwrap();
        assert_eq!((h.msg_type, h.target, h.flag), (k.key(), OWN, 0));
        assert_eq!(decode(&h, &mut r).unwrap(), Some(k));
        assert_eq!(r.remaining(), 0);
        for cut in 13..b.len() {
            let (h, mut r) = N3Header::parse(&b[..cut]).unwrap();
            assert!(decode(&h, &mut r).is_err() || cut == b.len(), "truncated at {cut}");
        }
    }

    #[test]
    fn keys_are_the_class_name_hashes() {
        for (key, name) in [
            (OPEN_CHAT_WINDOW, "KnubotOpenChatWindowIIR_c"),
            (CLOSE_CHAT_WINDOW, "KnubotCloseChatWindowIIR_c"),
            (APPEND_TEXT, "KnubotAppendTextIIR_c"),
            (ANSWER_LIST, "KnubotAnswerListIIR_c"),
            (ANSWER, "KnubotAnswerIIR_c"),
            (NPC_DESCRIPTION, "KnubotNPCDescriptionIIR_c"),
            (START_TRADE, "KnubotStartTradeIIR_c"),
            (FINISH_TRADE, "KnubotFinishTradeIIR_c"),
            (TRADE, "KnubotTradeIIR_c"),
            (REJECTED_ITEMS, "KnubotRejectedItemsIIR_c"),
        ] {
            assert_eq!(message_key(name), key, "{name}");
        }
    }

    #[test]
    fn every_message_round_trips() {
        round(Knubot::Open { npc: NPC, b20: true, b21: false });
        round(Knubot::Close { npc: NPC, value: -3, text: "bye".into() });
        round(Knubot::AppendText { npc: NPC, kind: text::QUESTION, text: "Hello\\nthere".into() });
        round(Knubot::AnswerList { npc: NPC, answers: vec!["Yes".into(), "No".into()] });
        round(Knubot::StartTrade { npc: NPC, value: 7, text: "sell".into() });
        round(Knubot::FinishTrade { npc: NPC, flag: true, value: 9 });
        round(Knubot::Trade { npc: NPC, op: 1, a: Identity::default(), b: Identity { kind: 0x1869F, instance: 12 } });
        round(Knubot::RejectedItems { npc: NPC, items: vec![(Identity { kind: 1, instance: 2 }, 3, 4)], value: 5 });
        round(Knubot::Answer { npc: NPC, index: 2 });
        round(Knubot::Description { npc: NPC });
    }

    /// The bytes the original writes for `N3Msg_SendNPCChatAnswer`: key, own identity, flag 0, version 2, NPC identity, answer index.
    #[test]
    fn answer_wire_layout() {
        let b = answer(OWN, NPC, 1);
        let mut want = message_key("KnubotAnswerIIR_c").to_be_bytes().to_vec();
        want.extend([0, 0, 0xC3, 0x50, 0, 0, 0x82, 0xe8, 0, 0, 2, 0, 0, 0xC3, 0x50, 0, 0, 0x12, 0x67, 0, 0, 0, 1]);
        assert_eq!(b, want);
    }

    #[test]
    fn other_messages_and_bad_lengths_are_rejected() {
        let b = super::super::outgoing::char_in_play(1);
        let (h, mut r) = N3Header::parse(&b).unwrap();
        assert_eq!(decode(&h, &mut r).unwrap(), None);
        let mut b = Knubot::AnswerList { npc: NPC, answers: vec!["x".into()] }.encode(OWN);
        b[13 + 2 + 8 + 3] = 0; // count 1 -> 0
        let (h, mut r) = N3Header::parse(&b).unwrap();
        assert!(decode(&h, &mut r).is_err());
        let mut b = Knubot::AppendText { npc: NPC, kind: 0, text: "x".into() }.encode(OWN);
        let at = 13 + 2 + 8 + 4;
        b[at..at + 4].copy_from_slice(&0x7fff_ffffi32.to_be_bytes());
        let (h, mut r) = N3Header::parse(&b).unwrap();
        assert!(decode(&h, &mut r).is_err());
    }
}
