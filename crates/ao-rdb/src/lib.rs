//! Read access to the PRK client's resource database (`cd_image/rdb.db`).
//!
//! PRK ships the Funcom ResourceDatabase converted to SQLite: one table per record type,
//! named `rdb_<type>`, schema `(id INTEGER PRIMARY KEY, version INTEGER, data BLOB)`.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension};

pub struct RecordStore {
    conn: Connection,
}

impl RecordStore {
    /// Opens `<client_dir>/cd_image/rdb.db` read-only.
    pub fn open(client_dir: &Path) -> Result<Self> {
        let path = client_dir.join("cd_image").join("rdb.db");
        let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("opening {}", path.display()))?;
        Ok(Self { conn })
    }

    /// Record types present in the database, ascending.
    pub fn types(&self) -> Result<Vec<u32>> {
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name LIKE 'rdb_%'")?;
        let mut types = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .filter_map(|n| n.ok()?.strip_prefix("rdb_")?.parse().ok())
            .collect::<Vec<u32>>();
        types.sort_unstable();
        Ok(types)
    }

    /// Record ids of `rdb_type`, ascending. Empty if the type has no table.
    pub fn ids(&self, rdb_type: u32) -> Result<Vec<u32>> {
        if !self.has_type(rdb_type)? {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT id FROM rdb_{rdb_type} ORDER BY id"))?;
        let ids = stmt
            .query_map([], |r| r.get::<_, u32>(0))?
            .collect::<rusqlite::Result<Vec<u32>>>()?;
        Ok(ids)
    }

    /// Raw record payload, or `None` if the type or id is absent.
    pub fn get(&self, rdb_type: u32, id: u32) -> Result<Option<Vec<u8>>> {
        if !self.has_type(rdb_type)? {
            return Ok(None);
        }
        self.conn
            .query_row(
                &format!("SELECT data FROM rdb_{rdb_type} WHERE id = ?1"),
                [id],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()
            .with_context(|| format!("reading rdb record {rdb_type}:{id}"))
    }

    /// Like [`RecordStore::get`] with the record's `version` column (the data format selector of the DbObject).
    pub fn get_versioned(&self, rdb_type: u32, id: u32) -> Result<Option<(u32, Vec<u8>)>> {
        if !self.has_type(rdb_type)? {
            return Ok(None);
        }
        self.conn
            .query_row(&format!("SELECT version, data FROM rdb_{rdb_type} WHERE id = ?1"), [id], |r| Ok((r.get::<_, u32>(0)?, r.get::<_, Vec<u8>>(1)?)))
            .optional()
            .with_context(|| format!("reading rdb record {rdb_type}:{id}"))
    }

    fn has_type(&self, rdb_type: u32) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name = ?1",
                [format!("rdb_{rdb_type}")],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
}
