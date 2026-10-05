//! Texture records: plain JPEG/PNG blobs in several rdb types (1010004, 1010008, 1010009, ...).

use anyhow::{Context, Result};
use ao_rdb::RecordStore;
use ao_scene::{Texture, TextureKey};

pub fn decode_texture(bytes: &[u8]) -> Result<Texture> {
    let img = image::load_from_memory(bytes).context("decoding texture")?.to_rgba8();
    Ok(Texture { width: img.width(), height: img.height(), rgba: img.into_raw() })
}

pub fn load_texture(store: &RecordStore, key: TextureKey) -> Result<Option<Texture>> {
    store.get(key.rdb_type, key.id)?.map(|b| decode_texture(&b)).transpose()
}
