//! Texture records: plain JPEG/PNG blobs in several rdb types (1010004, 1010008, 1010009, ...).

use anyhow::{Context, Result};
use ao_rdb::RecordStore;
use ao_scene::{Texture, TextureKey};

/// Seven rdb 1010004 records (shared by 178 character materials) lost the first four PNG signature
/// bytes (`89 50 4E 47`) to a `02 00 00 00` prefix; everything after is a valid PNG.
const BROKEN_PNG_PREFIX: [u8; 8] = [2, 0, 0, 0, 0x0D, 0x0A, 0x1A, 0x0A];

pub fn decode_texture(bytes: &[u8]) -> Result<Texture> {
    let fixed;
    let bytes = if bytes.starts_with(&BROKEN_PNG_PREFIX) {
        fixed = [&[0x89, b'P', b'N', b'G'][..], &bytes[4..]].concat();
        &fixed[..]
    } else {
        bytes
    };
    let img = image::load_from_memory(bytes).context("decoding texture")?.to_rgba8();
    Ok(Texture { width: img.width(), height: img.height(), rgba: img.into_raw() })
}

pub fn load_texture(store: &RecordStore, key: TextureKey) -> Result<Option<Texture>> {
    store.get(key.rdb_type, key.id)?.map(|b| decode_texture(&b)).transpose()
}
