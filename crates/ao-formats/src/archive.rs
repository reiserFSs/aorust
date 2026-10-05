//! Funcom "ObjectArchive" reader (randy31.dll `fun::ObjectArchive_c` / `fun::Message_c`).
//!
//! Layout (all little endian), see `docs/formats.md` § meshes:
//! ```text
//! u32 version (=3), u32 range_count
//! range_count x { u32, char[32] name }     named animation ranges ("idle", "walk", ...), skipped
//! u32, u32 (=1)                            unused
//! u32 name_count; name_count x { cstr kind, cstr name }   class/member name table
//! u32 object_count; (object_count + 1) x object           object 0 is the root holder
//! object := u32 type_id, u32 version, u32 member_count, member_count x member
//! member := u8 name_index, u32 type, u32 elem_size, u32 total_size, [total_size bytes]
//! ```
//! Objects reference each other by index into the object list *after* the root holder
//! (`-1` = null). Member names are global indices into the name table.

use anyhow::{bail, ensure, Result};

pub struct Member<'a> {
    pub name: &'a str,
    pub data: &'a [u8],
}

pub struct Object<'a> {
    pub members: Vec<Member<'a>>,
}

pub struct Archive<'a> {
    /// Index of the root object in `objects` (the `obj` member of the root holder).
    pub root: usize,
    pub objects: Vec<Object<'a>>,
}

const ANIM_RANGE_SIZE: usize = 36;

struct Reader<'a> {
    d: &'a [u8],
    o: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        ensure!(self.d.len() - self.o >= n, "truncated archive at {:#x}", self.o);
        let s = &self.d[self.o..self.o + n];
        self.o += n;
        Ok(s)
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn cstr(&mut self) -> Result<&'a str> {
        let rest = &self.d[self.o..];
        let n = rest.iter().position(|&b| b == 0).ok_or_else(|| anyhow::anyhow!("unterminated string"))?;
        self.o += n + 1;
        Ok(std::str::from_utf8(&rest[..n])?)
    }
}

impl<'a> Archive<'a> {
    pub fn parse(d: &'a [u8]) -> Result<Self> {
        let mut r = Reader { d, o: 0 };
        let version = r.u32()?;
        ensure!(version == 3, "not an object archive (version word {version})");
        let anims = r.u32()? as usize;
        ensure!(anims <= 256, "implausible animation range count ({anims})");
        r.take(anims * ANIM_RANGE_SIZE + 8)?; // named ranges, then two words the reader does not use
        let name_count = r.u32()? as usize;
        ensure!(name_count <= 256, "implausible name table ({name_count})");
        let mut names = Vec::with_capacity(name_count);
        for _ in 0..name_count {
            r.cstr()?;
            names.push(r.cstr()?);
        }
        let object_count = r.u32()? as usize;
        ensure!(object_count <= d.len(), "implausible object count {object_count}");
        let mut objects = Vec::with_capacity(object_count);
        let mut root = None;
        for k in 0..=object_count {
            r.u32()?; // type id
            r.u32()?; // object version
            let n = r.u32()?;
            let mut members = Vec::new();
            for _ in 0..n {
                let idx = r.take(1)?[0] as usize;
                r.u32()?; // value type (self-describing, not needed)
                r.u32()?; // element size
                let total = r.u32()? as usize;
                let data = r.take(total)?;
                let Some(&name) = names.get(idx) else { bail!("member name index {idx} out of range") };
                members.push(Member { name, data });
            }
            if k == 0 {
                let obj = members.iter().find(|m| m.name == "obj");
                root = obj.and_then(|m| m.data.get(..4)).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize);
            } else {
                objects.push(Object { members });
            }
        }
        let root = root.filter(|&r| r < objects.len());
        Ok(Archive { root: root.ok_or_else(|| anyhow::anyhow!("archive has no root object"))?, objects })
    }
}

impl<'a> Object<'a> {
    pub fn get(&self, name: &str) -> Option<&'a [u8]> {
        self.members.iter().find(|m| m.name == name).map(|m| m.data)
    }
    pub fn all<'s>(&'s self, name: &'s str) -> impl Iterator<Item = &'a [u8]> + 's {
        self.members.iter().filter(move |m| m.name == name).map(|m| m.data)
    }
    pub fn i32(&self, name: &str) -> Option<i32> {
        self.get(name).and_then(|d| d.get(..4)).map(|b| i32::from_le_bytes(b.try_into().unwrap()))
    }
    pub fn f32s<const N: usize>(&self, name: &str) -> Option<[f32; N]> {
        let d = self.get(name)?.get(..4 * N)?;
        let mut out = [0f32; N];
        for (o, c) in out.iter_mut().zip(d.as_chunks::<4>().0) {
            *o = f32::from_le_bytes(*c);
        }
        Some(out)
    }
    /// Object references of a member (one `obj`-typed member holds `n` refs); null refs are dropped.
    pub fn refs(&self, name: &str) -> Vec<usize> {
        self.all(name).flat_map(|d| d.as_chunks::<4>().0.iter()).filter_map(|c| usize::try_from(i32::from_le_bytes(*c)).ok()).collect()
    }
    pub fn ref1(&self, name: &str) -> Option<usize> {
        self.refs(name).first().copied()
    }
    /// Length-prefixed blob (`u32 len` + bytes), as used for `vertices`, `triangles` and strings.
    pub fn blob(&self, name: &str) -> Option<&'a [u8]> {
        let d = self.get(name)?;
        let n = u32::from_le_bytes(d.get(..4)?.try_into().unwrap()) as usize;
        d.get(4..4 + n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(idx: u8, ty: u32, es: u32, data: &[u8]) -> Vec<u8> {
        let mut v = vec![idx];
        for x in [ty, es, data.len() as u32] {
            v.extend(x.to_le_bytes());
        }
        v.extend(data);
        v
    }

    /// Byte-for-byte shape of record 1010001/287726 (an empty RRefFrame_t) with shortened members.
    fn fixture() -> Vec<u8> {
        let mut b = vec![];
        for x in [3u32, 0, 0, 1, 3] { // version, no ranges, 2 unused words, name count
            b.extend(x.to_le_bytes());
        }
        b.extend(b"\0RRefFrame_t\0");
        b.extend(b"1\0local_pos\0");
        b.extend(b"0\0obj\0");
        b.extend(1u32.to_le_bytes());
        // root holder: obj -> object 0
        for x in [0x19u32, 1, 1] {
            b.extend(x.to_le_bytes());
        }
        b.extend(member(2, 0x11, 4, &0i32.to_le_bytes()));
        // object 0
        for x in [0x93u32, 1, 2] {
            b.extend(x.to_le_bytes());
        }
        let pos: Vec<u8> = [1.0f32, 2.0, 3.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        b.extend(member(1, 0xc, 12, &pos));
        b.extend(member(2, 0x11, 8, &[(-1i32).to_le_bytes(), 0i32.to_le_bytes()].concat()));
        b.extend([0u8; 12]);
        b
    }

    #[test]
    fn parses_fixture() {
        let bytes = fixture();
        let a = Archive::parse(&bytes).unwrap();
        assert_eq!(a.root, 0);
        assert_eq!(a.objects.len(), 1);
        let o = &a.objects[0];
        assert_eq!(o.f32s::<3>("local_pos"), Some([1.0, 2.0, 3.0]));
        assert_eq!(o.refs("obj"), vec![0]); // null ref dropped
        assert!(o.get("missing").is_none());
    }

    #[test]
    fn skips_animation_ranges() {
        let plain = fixture();
        let mut b = plain[..4].to_vec();
        b.extend(1u32.to_le_bytes()); // one named range
        b.extend([7, 0, 0, 0]);
        b.extend([b"idle".as_slice(), &[0u8; 28]].concat());
        b.extend(&plain[8..]);
        let a = Archive::parse(&b).unwrap();
        assert_eq!(a.objects[0].f32s::<3>("local_pos"), Some([1.0, 2.0, 3.0]));
    }

    #[test]
    fn rejects_garbage_and_truncation() {
        assert!(Archive::parse(b"\x03\x00\x00\x00\x03\x00\x00\x00\x00\x00\x00\x00shotgun").is_err());
        let bytes = fixture();
        assert!(Archive::parse(&bytes[..bytes.len() - 30]).is_err());
    }
}
