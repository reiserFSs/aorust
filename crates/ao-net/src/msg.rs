//! System messages (`ptype == 1`) on the login path: connect -> auth -> character list ->
//! select -> zone hand-off -> zone login. Layouts reverse-engineered from Interfaces.dll
//! (`Client_t::*`) and MessageProtocol.dll (`CharacterData_t`, `CharacterInfo_c`); cross-checked
//! against CellAO `CellAO.Messages/SystemMessages`. See docs/protocol.md for addresses.

use crate::frame::Frame;
use crate::wire::{Reader, Writer};
use anyhow::{bail, Result};
use std::net::Ipv4Addr;

pub const LOGIN_ERROR: u32 = 0x0D;
pub const CHARACTER_LIST: u32 = 0x0E;
pub const NAME_IN_USE: u32 = 0x10;
pub const REQUEST_REJECTED: u32 = 0x21;
pub const CHARACTER_CREATED: u32 = 0x11;
pub const DELETE_CHARACTER: u32 = 0x14;
pub const CHARACTER_DELETED: u32 = 0x15;
pub const CREATE_CHARACTER: u32 = 0x0F;
pub const SELECT_CHARACTER: u32 = 0x16;
pub const ZONE_INFO: u32 = 0x17;
pub const ZONE_LOGIN: u32 = 0x1B;
pub const USER_LOGIN: u32 = 0x22;
pub const SERVER_SALT: u32 = 0x24;
pub const USER_CREDENTIALS: u32 = 0x25;
pub const ZONE_REDIRECTION: u32 = 0x3C;
pub const RANDOM_NAME_REQUEST: u32 = 0x55;
pub const SUGGEST_NAME: u32 = 0x56;

/// `LoginError` codes (CellAO `LoginError.cs`; client only forwards the int to its UI).
pub const ERR_ALREADY_LOGGED_IN: i32 = 0x14;
pub const ERR_INVALID_USER_OR_PASSWORD: i32 = 0x6A;
pub const ERR_BANNED_OR_NOT_PAID: i32 = 0x6C;

const PLAYFIELD_PROXY_VERSION: u8 = b'a';
const CHARACTER_DATA_VERSION: i32 = 4;
const CHARACTER_INFO_VERSION: i32 = 5;
/// `CharacterInfo_c::InitDefault` (MessageProtocol.dll 0x10001881).
const DEFAULT_AREA: &str = "area unknown";
const MAX_STR: usize = 0xFE; // CharacterInfo_c::ReadStream accepts len < 0xFF

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Identity {
    pub kind: i32,
    pub instance: i32,
}

impl Identity {
    fn read(r: &mut Reader) -> Result<Self> {
        Ok(Self { kind: r.i32()?, instance: r.i32()? })
    }
    fn write(&self, w: &mut Writer) {
        w.i32(self.kind);
        w.i32(self.instance);
    }
}

/// `PlayfieldProxy_t` (MessageProtocol.dll 0x1000322e / 0x100031e8).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PlayfieldProxy {
    pub playfield: Identity,
    pub attribute: i32,
    pub exit_door: i32,
    pub exit_door_id: Identity,
}

/// `CharacterInfo_c` blob (MessageProtocol.dll 0x1000196f read / 0x10001784 write; wire version 5).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CharacterInfo {
    pub id: i32,
    pub org_instance: i32,
    pub name: String,
    pub breed: i32,
    pub gender: i32,
    pub profession: i32,
    pub level: i32,
    pub area: String,
    pub banned: i32,
    pub ban_reason: String,
    pub head: i32,
    pub height: i32,
    pub width: i32,
}

/// One character-list row: `CharacterData_t` (v4) followed by an `i32` status
/// (CellAO `CharacterStatus`; stored by the client in a parallel vector).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CharacterEntry {
    pub id: i32,
    pub proxy: PlayfieldProxy,
    pub created: bool,
    pub info: CharacterInfo,
    pub status: i32,
}

impl CharacterEntry {
    fn read(r: &mut Reader) -> Result<Self> {
        let ver = r.i32()?;
        if ver < CHARACTER_DATA_VERSION {
            bail!("unsupported CharacterData version {ver}");
        }
        let id = r.i32()?;
        if r.u8()? != PLAYFIELD_PROXY_VERSION {
            bail!("Invalid playfieldproxy version");
        }
        let proxy = PlayfieldProxy {
            playfield: Identity::read(r)?,
            attribute: r.i32()?,
            exit_door: r.i32()?,
            exit_door_id: Identity::read(r)?,
        };
        let created = r.i32()? != 0;
        let iver = r.i32()?;
        if iver < 3 {
            bail!("unsupported CharacterInfo version {iver}");
        }
        let info_id = r.i32()?;
        let org_instance = if iver > 4 { r.i32()? } else { 0 };
        let name = r.str_i32(MAX_STR)?;
        let (breed, gender, profession, level) = (r.i32()?, r.i32()?, r.i32()?, r.i32()?);
        let area = r.str_i32(MAX_STR)?;
        let banned = r.i32()?;
        let ban_reason = r.str_i32(usize::MAX >> 1)?;
        let (head, height, width) =
            if iver > 3 { (r.i32()?, r.i32()?, r.i32()?) } else { (0, 0, 0) };
        let status = r.i32()?;
        Ok(Self {
            id, // CharacterData_t::ReadBlobStream overwrites info.id with this (0x100011c5)
            proxy,
            created,
            info: CharacterInfo {
                id: info_id,
                org_instance,
                name,
                breed,
                gender,
                profession,
                level,
                area,
                banned,
                ban_reason,
                head,
                height,
                width,
            },
            status,
        })
    }

    fn write(&self, w: &mut Writer) {
        w.i32(CHARACTER_DATA_VERSION);
        w.i32(self.id);
        w.u8(PLAYFIELD_PROXY_VERSION);
        self.proxy.playfield.write(w);
        w.i32(self.proxy.attribute);
        w.i32(self.proxy.exit_door);
        self.proxy.exit_door_id.write(w);
        w.i32(self.created as i32);
        let i = &self.info;
        w.i32(CHARACTER_INFO_VERSION);
        w.i32(i.id);
        w.i32(i.org_instance);
        w.str_i32(&i.name);
        for v in [i.breed, i.gender, i.profession, i.level] {
            w.i32(v);
        }
        w.str_i32(&i.area);
        w.i32(i.banned);
        w.str_i32(&i.ban_reason);
        for v in [i.head, i.height, i.width] {
            w.i32(v);
        }
        w.i32(self.status);
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CharacterList {
    pub characters: Vec<CharacterEntry>,
    pub allowed_characters: i32,
    pub expansions: i32,
    /// Third trailing int: `s_nSLProfsEnabled = (value != 0)` (Interfaces.dll ProcessMessage 0x0E).
    pub sl_profs_enabled: i32,
}

/// Arguments of `Client_t::CreateCharacter` (Interfaces.dll 0x10001928; called from `NameScene_t::SetState(0x1006)`,
/// GUI.dll 0x1011f75e). Everything else of the `CharacterData_t` is the default-constructed value
/// (id 0, zero playfield proxy, not created, level 0, area "area unknown", no ban).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CreateCharacterRequest {
    pub breed: i32,
    pub gender: i32,
    pub profession: i32,
    /// Head mesh id (`CCCharacter_t::GetHeadMeshID`), stored in `CharacterInfo_c+0x70`.
    pub head: i32,
    /// Height in percent (GUI: 90 / 100 / 110), `CharacterInfo_c+0x74`.
    pub height: i32,
    /// `CharacterInfo_c+0x78` (GUI passes the `CCSelectedSize` pref).
    pub width: i32,
    pub name: String,
    /// Trailing `i32` after the `CharacterData_t` (CellAO `StarterArea`); GUI passes `NameScene_t+0x74` (always 0 there).
    pub starter_area: i32,
}

impl CreateCharacterRequest {
    fn entry(&self) -> CharacterEntry {
        CharacterEntry {
            info: CharacterInfo {
                name: self.name.clone(),
                breed: self.breed,
                gender: self.gender,
                profession: self.profession,
                area: DEFAULT_AREA.into(),
                head: self.head,
                height: self.height,
                width: self.width,
                ..Default::default()
            },
            status: self.starter_area,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZoneInfo {
    pub char_id: i32,
    pub ip: Ipv4Addr,
    pub port: u16,
    pub cookie1: u32,
    pub cookie2: u32,
    /// `Client_t::s_nEventServerType`; absent (0) in CellAO's 22-byte variant.
    pub event_server_type: u32,
    /// `Client_t::s_nPlayerID`; absent (0) in CellAO's 22-byte variant.
    pub player_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// 0x22 client->server.
    UserLogin { protocol: i32, name: String, client_version: String },
    /// 0x24 server->client: 32 raw bytes.
    ServerSalt([u8; 32]),
    /// 0x25 client->server: the `"<dhX>-<cipher>"` string from [`crate::crypto`].
    UserCredentials { name: String, response: String },
    /// 0x0D server->client.
    LoginError(i32),
    /// 0x0E server->client.
    CharacterList(CharacterList),
    /// 0x16 client->server.
    SelectCharacter { char_id: i32 },
    /// 0x17 server->client.
    ZoneInfo(ZoneInfo),
    /// 0x1B client->zone server.
    ZoneLogin { char_id: i32, cookie1: u32, cookie2: u32 },
    /// 0x3C server->client (zone teleport between servers).
    ZoneRedirection { ip: Ipv4Addr, port: u16 },
    /// 0x0F client->server: `CharacterData_t` + `i32` (Client_t::CreateCharacter).
    CreateCharacter(CreateCharacterRequest),
    /// 0x55 client->server (Client_t::SuggestNickName 0x1000188e): three `i32`.
    RandomNameRequest { breed: i32, gender: i32, profession: i32 },
    /// 0x56 server->client: `i16`-prefixed string.
    SuggestName(String),
    /// 0x14 client->server.
    DeleteCharacter { char_id: i32 },
    /// 0x15 server->client; the client reads no body (CellAO sends the id).
    CharacterDeleted { char_id: Option<i32> },
    /// 0x11 server->client; client then auto-sends SelectCharacter for this id.
    CharacterCreated { char_id: i32 },
    /// 0x10 server->client.
    NameInUse(i32),
    /// 0x21 server->client: live PRK answer to an undecryptable `UserCredentials` (detail 9 observed).
    RequestRejected(i32),
}

impl Message {
    pub fn msg_type(&self) -> u32 {
        match self {
            Message::UserLogin { .. } => USER_LOGIN,
            Message::ServerSalt(_) => SERVER_SALT,
            Message::UserCredentials { .. } => USER_CREDENTIALS,
            Message::LoginError(_) => LOGIN_ERROR,
            Message::CharacterList(_) => CHARACTER_LIST,
            Message::SelectCharacter { .. } => SELECT_CHARACTER,
            Message::ZoneInfo(_) => ZONE_INFO,
            Message::ZoneLogin { .. } => ZONE_LOGIN,
            Message::ZoneRedirection { .. } => ZONE_REDIRECTION,
            Message::CreateCharacter(_) => CREATE_CHARACTER,
            Message::RandomNameRequest { .. } => RANDOM_NAME_REQUEST,
            Message::SuggestName(_) => SUGGEST_NAME,
            Message::DeleteCharacter { .. } => DELETE_CHARACTER,
            Message::CharacterDeleted { .. } => CHARACTER_DELETED,
            Message::CharacterCreated { .. } => CHARACTER_CREATED,
            Message::NameInUse(_) => NAME_IN_USE,
            Message::RequestRejected(_) => REQUEST_REJECTED,
        }
    }

    pub fn encode_body(&self) -> Vec<u8> {
        let mut w = Writer::default();
        match self {
            Message::UserLogin { protocol, name, client_version } => {
                w.i32(*protocol);
                w.fixed_str(name, 40);
                w.fixed_str(client_version, 20);
            }
            Message::ServerSalt(s) => w.bytes(s),
            Message::UserCredentials { name, response } => {
                w.fixed_str(name, 40);
                // length counts the trailing NUL, which is sent (AuthClient, 0x10001b65)
                w.i32(response.len() as i32 + 1);
                w.bytes(response.as_bytes());
                w.u8(0);
            }
            Message::LoginError(c) | Message::NameInUse(c) | Message::RequestRejected(c) => w.i32(*c),
            Message::CharacterList(l) => {
                w.i32(l.characters.len() as i32);
                for c in &l.characters {
                    c.write(&mut w);
                }
                w.i32(l.allowed_characters);
                w.i32(l.expansions);
                w.i32(l.sl_profs_enabled);
            }
            Message::SelectCharacter { char_id }
            | Message::DeleteCharacter { char_id }
            | Message::CharacterCreated { char_id } => w.i32(*char_id),
            Message::CharacterDeleted { char_id } => {
                if let Some(c) = char_id {
                    w.i32(*c)
                }
            }
            Message::ZoneInfo(z) => {
                w.i32(z.char_id);
                w.bytes(&z.ip.octets());
                w.u16(z.port);
                w.u32(z.cookie1);
                w.u32(z.cookie2);
                w.u32(z.event_server_type);
                w.u32(z.player_id);
            }
            Message::ZoneLogin { char_id, cookie1, cookie2 } => {
                w.i32(*char_id);
                w.u32(*cookie1);
                w.u32(*cookie2);
            }
            Message::ZoneRedirection { ip, port } => {
                w.bytes(&ip.octets());
                w.u16(*port);
            }
            Message::CreateCharacter(c) => c.entry().write(&mut w),
            Message::RandomNameRequest { breed, gender, profession } => {
                for v in [breed, gender, profession] {
                    w.i32(*v);
                }
            }
            Message::SuggestName(n) => w.str_i16(n),
        }
        w.0
    }

    pub fn decode(msg_type: u32, body: &[u8]) -> Result<Message> {
        let mut r = Reader::new(body);
        Ok(match msg_type {
            USER_LOGIN => Message::UserLogin {
                protocol: r.i32()?,
                name: r.fixed_str(40)?,
                client_version: r.fixed_str(20)?,
            },
            SERVER_SALT => Message::ServerSalt(r.bytes(32)?.try_into()?),
            USER_CREDENTIALS => Message::UserCredentials {
                name: r.fixed_str(40)?,
                response: r.str_i32(0x1_0000)?,
            },
            LOGIN_ERROR => Message::LoginError(r.i32()?),
            NAME_IN_USE => Message::NameInUse(r.i32()?),
            REQUEST_REJECTED => Message::RequestRejected(r.i32()?),
            CHARACTER_LIST => {
                let n = r.i32()?;
                if !(0..=1024).contains(&n) {
                    bail!("character count {n} out of range");
                }
                let characters =
                    (0..n).map(|_| CharacterEntry::read(&mut r)).collect::<Result<Vec<_>>>()?;
                Message::CharacterList(CharacterList {
                    characters,
                    allowed_characters: r.i32()?,
                    expansions: r.i32()?,
                    // Live PRK sends only two trailing ints (docs/protocol.md §8): the client's third read hits
                    // end of stream and leaves its zero-initialised value.
                    sl_profs_enabled: if r.remaining() >= 4 { r.i32()? } else { 0 },
                })
            }
            SELECT_CHARACTER => Message::SelectCharacter { char_id: r.i32()? },
            DELETE_CHARACTER => Message::DeleteCharacter { char_id: r.i32()? },
            CHARACTER_CREATED => Message::CharacterCreated { char_id: r.i32()? },
            CHARACTER_DELETED => {
                Message::CharacterDeleted { char_id: if r.remaining() >= 4 { Some(r.i32()?) } else { None } }
            }
            ZONE_INFO => {
                let char_id = r.i32()?;
                let ip = Ipv4Addr::from(<[u8; 4]>::try_from(r.bytes(4)?)?);
                let (port, cookie1, cookie2) = (r.u16()?, r.u32()?, r.u32()?);
                // Client reads two more ints; CellAO omits them, so tolerate absence.
                let (event_server_type, player_id) =
                    if r.remaining() >= 8 { (r.u32()?, r.u32()?) } else { (0, 0) };
                Message::ZoneInfo(ZoneInfo {
                    char_id,
                    ip,
                    port,
                    cookie1,
                    cookie2,
                    event_server_type,
                    player_id,
                })
            }
            ZONE_LOGIN => Message::ZoneLogin { char_id: r.i32()?, cookie1: r.u32()?, cookie2: r.u32()? },
            ZONE_REDIRECTION => Message::ZoneRedirection {
                ip: Ipv4Addr::from(<[u8; 4]>::try_from(r.bytes(4)?)?),
                port: r.u16()?,
            },
            CREATE_CHARACTER => {
                let e = CharacterEntry::read(&mut r)?;
                let i = e.info;
                Message::CreateCharacter(CreateCharacterRequest {
                    breed: i.breed,
                    gender: i.gender,
                    profession: i.profession,
                    head: i.head,
                    height: i.height,
                    width: i.width,
                    name: i.name,
                    starter_area: e.status,
                })
            }
            RANDOM_NAME_REQUEST => {
                Message::RandomNameRequest { breed: r.i32()?, gender: r.i32()?, profession: r.i32()? }
            }
            SUGGEST_NAME => Message::SuggestName(r.str_i16()?),
            t => bail!("unhandled system message {t:#x}"),
        })
    }

    /// (sender, receiver) header ids as the original sends/accepts them: client messages use
    /// `(0, 1)` (login server) except ZoneLogin `(charId, 2)`; server messages `(1, 0)`.
    fn header_ids(&self) -> (u32, u32) {
        match self {
            Message::ZoneLogin { char_id, .. } => (*char_id as u32, 2),
            Message::UserLogin { .. }
            | Message::UserCredentials { .. }
            | Message::SelectCharacter { .. }
            | Message::CreateCharacter(_)
            | Message::RandomNameRequest { .. }
            | Message::DeleteCharacter { .. } => (0, 1),
            _ => (1, 0),
        }
    }

    pub fn to_frame(&self, seq: u16) -> Frame {
        let (s, r) = self.header_ids();
        Frame::system(seq, s, r, self.msg_type(), &self.encode_body())
    }

    pub fn from_frame(f: &Frame) -> Result<Message> {
        let (t, body) = f.system_parts()?;
        Message::decode(t, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(m: Message) {
        let bytes = m.to_frame(3).encode().unwrap();
        let (f, n) = Frame::decode(&bytes).unwrap().unwrap();
        assert_eq!(n, bytes.len());
        assert_eq!(Message::from_frame(&f).unwrap(), m);
    }

    fn entry() -> CharacterEntry {
        CharacterEntry {
            id: 0x1234,
            proxy: PlayfieldProxy {
                playfield: Identity { kind: 0xC79C, instance: 4001 },
                attribute: 1,
                exit_door: 0,
                exit_door_id: Identity::default(),
            },
            created: true,
            info: CharacterInfo {
                id: 0x1234,
                org_instance: 0,
                name: "Testguy".into(),
                breed: 1,
                gender: 2,
                profession: 6,
                level: 220,
                area: "Newland City".into(),
                ..Default::default()
            },
            status: 1,
        }
    }

    #[test]
    fn roundtrips() {
        rt(Message::UserLogin { protocol: 2, name: "acct".into(), client_version: "00.7.2_EP1".into() });
        rt(Message::ServerSalt([0x2A; 32]));
        rt(Message::UserCredentials { name: "acct".into(), response: "ab-cd".into() });
        rt(Message::LoginError(ERR_INVALID_USER_OR_PASSWORD));
        rt(Message::CharacterList(CharacterList {
            characters: vec![entry(), entry()],
            allowed_characters: 6,
            expansions: 0x7FFF,
            sl_profs_enabled: 1,
        }));
        rt(Message::CharacterList(CharacterList::default()));
        rt(Message::SelectCharacter { char_id: 77 });
        rt(Message::ZoneInfo(ZoneInfo {
            char_id: 77,
            ip: Ipv4Addr::new(199, 241, 136, 157),
            port: 7501,
            cookie1: 0xDEADBEEF,
            cookie2: 0x01020304,
            event_server_type: 1,
            player_id: 77,
        }));
        rt(Message::ZoneLogin { char_id: 77, cookie1: 1, cookie2: 2 });
        rt(Message::ZoneRedirection { ip: Ipv4Addr::new(10, 0, 0, 1), port: 9000 });
        rt(Message::DeleteCharacter { char_id: 5 });
        rt(Message::CharacterDeleted { char_id: Some(5) });
        rt(Message::CharacterDeleted { char_id: None });
        rt(Message::CharacterCreated { char_id: 5 });
        rt(Message::NameInUse(30));
        rt(Message::RequestRejected(9));
    }

    /// Real PRK (Ithaca, 2026-10) CharacterList frame: one character, only two trailing ints (no slProfs).
    #[test]
    fn live_character_list() {
        let b: Vec<u8> = (0..HEX.len() / 2).map(|i| u8::from_str_radix(&HEX[2 * i..2 * i + 2], 16).unwrap()).collect();
        const HEX: &str = "0002000100010092000000010000615b0000000e000000010000000400006584610000c79d000011e60000000100000000000000000000000000000001000000050000658400000000000000055465737479000000010000000300000001000000010000000c6172656120756e6b6e6f776e000000000000000000000000000000000000000000000001000000320000001b0000";
        let (f, used) = Frame::decode(&b).unwrap().unwrap();
        assert_eq!(used, b.len());
        let Message::CharacterList(l) = Message::from_frame(&f).unwrap() else { panic!() };
        assert_eq!((l.allowed_characters, l.expansions, l.sl_profs_enabled), (50, 27, 0));
        let c = &l.characters[0];
        assert_eq!((c.info.name.as_str(), c.id, c.status, c.created), ("Testy", 0x6584, 1, true));
    }

    // Hand-derived from AuthClient / InitAuth: 'name[40] | i32 len+1 | bytes | NUL'.
    #[test]
    fn user_login_bytes() {
        let b = Message::UserLogin { protocol: 2, name: "bob".into(), client_version: "0.7.2".into() }
            .to_frame(1)
            .encode()
            .unwrap();
        assert_eq!(b.len(), 16 + 4 + 4 + 40 + 20);
        assert_eq!(&b[..16], &[0, 1, 0, 1, 0, 1, 0, 84, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(&b[16..24], &[0, 0, 0, 0x22, 0, 0, 0, 2]);
        assert_eq!(&b[24..28], b"bob\0");
        assert_eq!(&b[64..69], b"0.7.2");
    }

    #[test]
    fn credentials_bytes() {
        let b = Message::UserCredentials { name: "n".into(), response: "xy".into() }.encode_body();
        assert_eq!(&b[40..], &[0, 0, 0, 3, b'x', b'y', 0]);
    }

    #[test]
    fn zone_info_layout_and_cellao_short_form() {
        let mut body = vec![0, 0, 0, 9, 199, 241, 136, 157, 0x1D, 0x4D];
        body.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD, 0, 0, 0, 5]);
        match Message::decode(ZONE_INFO, &body).unwrap() {
            Message::ZoneInfo(z) => {
                assert_eq!((z.char_id, z.port, z.cookie1, z.cookie2), (9, 7501, 0xAABBCCDD, 5));
                assert_eq!(z.ip, Ipv4Addr::new(199, 241, 136, 157));
                assert_eq!((z.event_server_type, z.player_id), (0, 0));
            }
            _ => unreachable!(),
        }
    }

    /// Deterministic xorshift so failures reproduce.
    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn decoders_never_panic() {
        let types = [
            USER_LOGIN, SERVER_SALT, USER_CREDENTIALS, LOGIN_ERROR, CHARACTER_LIST, NAME_IN_USE,
            CHARACTER_CREATED, DELETE_CHARACTER, CHARACTER_DELETED, SELECT_CHARACTER, ZONE_INFO,
            ZONE_LOGIN, ZONE_REDIRECTION, 0, 0xFFFF_FFFF,
        ];
        let valid = [
            Message::CharacterList(CharacterList { characters: vec![entry()], ..Default::default() })
                .encode_body(),
            Message::UserCredentials { name: "n".into(), response: "ab-cd".into() }.encode_body(),
            Message::UserLogin { protocol: 2, name: "a".into(), client_version: "v".into() }.encode_body(),
        ];
        let mut s = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..20_000 {
            let r = rng(&mut s);
            let mut buf: Vec<u8> = if r & 1 == 0 {
                (0..(rng(&mut s) % 96)).map(|_| rng(&mut s) as u8).collect()
            } else {
                let mut v = valid[(r >> 1) as usize % valid.len()].clone();
                for _ in 0..(1 + rng(&mut s) % 4) {
                    let i = rng(&mut s) as usize % v.len();
                    v[i] = rng(&mut s) as u8;
                }
                v.truncate(rng(&mut s) as usize % (v.len() + 1));
                v
            };
            if buf.len() >= 8 && rng(&mut s) & 3 == 0 {
                buf[4..8].copy_from_slice(&[0x7F, 0xFF, 0xFF, 0xFF]);
            }
            for t in types {
                let _ = Message::decode(t, &buf);
            }
            let _ = Frame::decode(&buf);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(Message::decode(CHARACTER_LIST, &[0, 0, 0, 1]).is_err());
        assert!(Message::decode(0x9999, &[]).is_err());
        let mut b = Message::CharacterList(CharacterList { characters: vec![entry()], ..Default::default() })
            .encode_body();
        b[4 + 8] = b'b'; // corrupt PlayfieldProxy version byte
        assert!(Message::decode(CHARACTER_LIST, &b).is_err());
    }

    #[test]
    fn create_character_layout_matches_client_stream() {
        let req = CreateCharacterRequest {
            breed: 1, gender: 2, profession: 6, head: 4, height: 100, width: 1, name: "Ab".into(), starter_area: 7,
        };
        let m = Message::CreateCharacter(req);
        let b = m.encode_body();
        // 49-byte CharacterData prefix (CellAO CreateCharacterMessage.Unknown1), then i32-string name
        assert_eq!(&b[..8], &[0, 0, 0, 4, 0, 0, 0, 0]); // version 4, id 0
        assert_eq!(b[8], b'a');
        assert_eq!(&b[45..49], &[0, 0, 0, 0]); // info.org_instance
        assert_eq!(&b[49..55], &[0, 0, 0, 2, b'A', b'b']);
        assert_eq!(&b[b.len() - 4..], &[0, 0, 0, 7]);
        rt(m);
        rt(Message::RandomNameRequest { breed: 1, gender: 2, profession: 6 });
        assert_eq!(Message::SuggestName("Zed".into()).encode_body(), [0, 3, b'Z', b'e', b'd']);
        rt(Message::SuggestName("Zed".into()));
        rt(Message::SuggestName(String::new()));
    }
}
