//! Native References.bin (references.dll Save 0x10001784 / Load 0x10001d50).
//! Runtime identities and reference keys are deliberately not serialized.

use std::{collections::BTreeMap, fs, io::{self, Read, Write}, path::Path};

const VERSION: u32 = 1003;
const MAX_NAME: usize = 0x7c00;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reference {
    Item { low: u32, high: u32, ql: u32 },
    Character { identity_type: u32, identity_id: u32, name: Vec<u8> },
}

#[derive(Clone, Debug)]
pub struct ReferenceStore {
    entries: BTreeMap<u32, Reference>,
    next_key: u32,
}

impl Default for ReferenceStore {
    fn default() -> Self {
        Self { entries: BTreeMap::new(), next_key: 1 }
    }
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

impl ReferenceStore {
    pub fn entries(&self) -> impl Iterator<Item = (u32, &Reference)> {
        self.entries.iter().map(|(&key, value)| (key, value))
    }

    pub fn add_item(&mut self, low: u32, high: u32, ql: u32) -> io::Result<u32> {
        // RefMan_t::AddItemRef 0x10001a87 rejects zero, deduplicates the triple.
        if low == 0 || high == 0 || ql == 0 {
            return Err(invalid("zero item reference field"));
        }
        let reference = Reference::Item { low, high, ql };
        if let Some((key, _)) = self.entries().find(|(_, value)| **value == reference) {
            return Ok(key);
        }
        self.insert(reference)
    }

    pub fn add_character(&mut self, identity_type: u32, identity_id: u32, mut name: Vec<u8>) -> io::Result<u32> {
        // AddCharRef 0x10001bf3 updates an existing identity's payload in place.
        if identity_type == 0 && identity_id == 0 {
            return Err(invalid("zero character reference identity"));
        }
        name.truncate(MAX_NAME);
        for (&key, reference) in &mut self.entries {
            if let Reference::Character { identity_type: old_type, identity_id: old_id, name: old_name } = reference
                && *old_type == identity_type && *old_id == identity_id
            {
                *old_name = name;
                return Ok(key);
            }
        }
        self.insert(Reference::Character { identity_type, identity_id, name })
    }

    fn insert(&mut self, reference: Reference) -> io::Result<u32> {
        let key = self.next_key;
        self.next_key = key.checked_add(1).ok_or_else(|| invalid("reference key overflow"))?;
        self.entries.insert(key, reference);
        Ok(key)
    }

    pub fn remove(&mut self, key: u32) -> bool {
        self.entries.remove(&key).is_some()
    }

    /// Missing character preferences mean an empty reference list, not a load error.
    pub fn load(path: &Path, max: usize) -> io::Result<Self> {
        match fs::File::open(path) {
            Ok(mut file) => Self::read(&mut file, max),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    pub fn parse(mut bytes: &[u8], max: usize) -> io::Result<Self> {
        Self::read(&mut bytes, max)
    }

    fn read(reader: &mut impl Read, max: usize) -> io::Result<Self> {
        fn word(reader: &mut impl Read) -> io::Result<u32> {
            let mut bytes = [0; 4];
            reader.read_exact(&mut bytes)?;
            Ok(u32::from_be_bytes(bytes))
        }
        if word(reader)? != VERSION {
            return Err(invalid("unsupported reference version"));
        }
        let count = (word(reader)? as usize).min(max);
        let mut store = Self::default();
        for _ in 0..count {
            match word(reader)? {
                1 => {
                    let low = word(reader)?;
                    let high = word(reader)?;
                    let ql = word(reader)?;
                    // Native invalid entries are skipped by AddItemRef.
                    if low != 0 && high != 0 && ql != 0 {
                        store.add_item(low, high, ql)?;
                    }
                }
                0 => {
                    let identity_type = word(reader)?;
                    let identity_id = word(reader)?;
                    let len = word(reader)? as usize;
                    if len > MAX_NAME {
                        return Err(invalid("reference name exceeds native limit"));
                    }
                    let mut name = vec![0; len];
                    reader.read_exact(&mut name)?;
                    // Load constructs std::string from a NUL-terminated buffer.
                    if let Some(end) = name.iter().position(|&byte| byte == 0) {
                        name.truncate(end);
                    }
                    if identity_type != 0 || identity_id != 0 {
                        store.add_character(identity_type, identity_id, name)?;
                    }
                }
                _ => return Err(invalid("unknown reference type")),
            }
        }
        Ok(store)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&VERSION.to_be_bytes());
        bytes.extend_from_slice(&(self.entries.len() as u32).to_be_bytes());
        for reference in self.entries.values() {
            let (tag, first, second, third) = match reference {
                Reference::Item { low, high, ql } => (1u32, *low, *high, *ql),
                Reference::Character { identity_type, identity_id, name } => (0, *identity_type, *identity_id, name.len() as u32),
            };
            for value in [tag, first, second, third] {
                bytes.extend_from_slice(&value.to_be_bytes());
            }
            if let Reference::Character { name, .. } = reference {
                bytes.extend_from_slice(name);
            }
        }
        bytes
    }

    /// The caller owns the character directory and native save lifecycle.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut file = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(&self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_bytes_keys_dedup_and_malformed() {
        // Scalar byte order is BinaryIO File_t::WriteU32 0x10003741, order=0 (BE).
        let fixture = [0,0,3,235, 0,0,0,2, 0,0,0,1, 0,0,0,10, 0,0,0,20, 0,0,0,30,
            0,0,0,0, 0,0,195,80, 0,0,0,42, 0,0,0,2, 0x80, b'x'];
        let mut store = ReferenceStore::parse(&fixture, 500).unwrap();
        assert_eq!(store.encode(), fixture);
        assert_eq!(store.add_item(10, 20, 30).unwrap(), 1);
        assert_eq!(store.add_character(50000, 42, b"new".to_vec()).unwrap(), 2);
        assert!(store.remove(1));
        assert_eq!(store.add_item(40, 50, 60).unwrap(), 3);
        let loaded = ReferenceStore::parse(&store.encode(), 500).unwrap();
        assert_eq!(loaded.entries().map(|(key, _)| key).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(ReferenceStore::parse(&fixture, 1).unwrap().entries().count(), 1);
        for end in 0..fixture.len() {
            assert!(ReferenceStore::parse(&fixture[..end], 500).is_err());
        }
        let mut bad = fixture.to_vec();
        bad[11] = 9;
        assert!(ReferenceStore::parse(&bad, 500).is_err());
        bad = fixture.to_vec();
        bad[36..40].copy_from_slice(&0x7c01u32.to_be_bytes());
        assert!(ReferenceStore::parse(&bad, 500).is_err());
        let path = super::super::prefs::test_dir().join("References.bin");
        assert_eq!(ReferenceStore::load(&path, 500).unwrap().entries().count(), 0);
        store.save(&path).unwrap();
        assert_eq!(ReferenceStore::load(&path, 500).unwrap().encode(), store.encode());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        fs::remove_file(path).unwrap();
        assert!(store.add_item(0, 1, 1).is_err());
        assert!(store.add_character(0, 0, Vec::new()).is_err());
    }
}
