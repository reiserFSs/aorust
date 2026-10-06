//! `AddPetIIR_c` 0x194E4F76 / `RemovePetIIR_c` 0x58742A0F (server -> client): the own pet list that `/pet`, `/tower` read
//! (`dynel+0x1d8` -> `+0x1c`, a list of `Identity`). Docs: docs/zone/pets.md.
//!
//! Both bodies are one `Identity` (read `FUN_10071429` / `FUN_10076877` [GC], write `FUN_10071449` / `FUN_10076897`). The apply
//! slots (`FUN_1007145e` / `FUN_100768ac`) need the header dynel to exist, call `FUN_10052458` (add: appends the identity when it is not
//! in the list yet, then a stat-0x1ca notification) / `FUN_100523ee` (remove) with the pet identity, then `ClearToBePassedOn`.

use super::N3Header;
use crate::msg::Identity;
use crate::wire::{Reader, Writer};
use anyhow::Result;

pub const ADD_PET: u32 = 0x194E_4F76;
pub const REMOVE_PET: u32 = 0x5874_2A0F;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PetList {
    Add(Identity),
    Remove(Identity),
}

impl PetList {
    pub fn encode(&self, owner: Identity, flag: u8) -> Vec<u8> {
        let (key, pet) = match self {
            PetList::Add(p) => (ADD_PET, p),
            PetList::Remove(p) => (REMOVE_PET, p),
        };
        let mut w = Writer::default();
        w.u32(key);
        owner.write(&mut w);
        w.u8(flag);
        pet.write(&mut w);
        w.0
    }
}

pub fn decode(h: &N3Header, r: &mut Reader) -> Result<Option<PetList>> {
    Ok(match h.msg_type {
        ADD_PET => Some(PetList::Add(Identity::read(r)?)),
        REMOVE_PET => Some(PetList::Remove(Identity::read(r)?)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::n3::outgoing::message_key;

    #[test]
    fn keys_and_layout() {
        assert_eq!(message_key("AddPetIIR_c"), ADD_PET);
        assert_eq!(message_key("RemovePetIIR_c"), REMOVE_PET);
        let owner = Identity { kind: 0xC350, instance: 7 };
        let pet = Identity { kind: 0xC350, instance: 500 };
        let b = PetList::Add(pet).encode(owner, 0);
        assert_eq!(b.len(), 4 + 8 + 1 + 8);
        let (h, mut r) = N3Header::parse(&b).unwrap();
        assert_eq!((h.target, h.msg_type), (owner, ADD_PET));
        assert_eq!(decode(&h, &mut r).unwrap(), Some(PetList::Add(pet)));
        let b = PetList::Remove(pet).encode(owner, 0);
        let (h, mut r) = N3Header::parse(&b).unwrap();
        assert_eq!(decode(&h, &mut r).unwrap(), Some(PetList::Remove(pet)));
        // truncated body: Err, no panic
        let (h, mut r) = N3Header::parse(&b[..b.len() - 3]).unwrap();
        assert!(decode(&h, &mut r).is_err());
    }
}
