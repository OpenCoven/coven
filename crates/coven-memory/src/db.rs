//! SQLite metadata store — document records, id allocation, staleness tracking

use crate::{fs_security, MemoryDoc};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};

pub struct MetaDb {
    conn: Connection,
    path: PathBuf,
}

impl MetaDb {
    pub fn open(path: &Path) -> Result<Self> {
        fs_security::ensure_private_parent(path)?;
        fs_security::validate_existing_private_file(path)?;
        let conn =
            Connection::open(path).with_context(|| format!("opening db at {}", path.display()))?;
        fs_security::set_private_file(path)?;
        let db = Self {
            conn,
            path: path.to_owned(),
        };
        db.init()?;
        fs_security::set_private_file(path)?;
        Ok(db)
    }

    fn init(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;

            CREATE TABLE IF NOT EXISTS docs (
                id              INTEGER PRIMARY KEY,
                path            TEXT NOT NULL,
                familiar        TEXT NOT NULL DEFAULT 'coven',
                chunk           TEXT NOT NULL,
                chunk_offset    INTEGER NOT NULL DEFAULT 0,
                content_hash    TEXT NOT NULL,
                ingested_at     INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_docs_path ON docs(path);
            CREATE INDEX IF NOT EXISTS idx_docs_familiar ON docs(familiar);
            CREATE INDEX IF NOT EXISTS idx_docs_hash ON docs(content_hash);

            CREATE TABLE IF NOT EXISTS meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
        ",
        )?;
        Ok(())
    }

    /// Insert a new doc, returning the assigned id
    pub fn insert(&self, doc: &MemoryDoc) -> Result<u64> {
        let now = chrono::Utc::now().timestamp();
        self.conn.execute(
            "INSERT INTO docs (path, familiar, chunk, chunk_offset, content_hash, ingested_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &doc.path,
                &doc.familiar,
                &doc.chunk,
                doc.chunk_offset as i64,
                &doc.content_hash,
                now
            ],
        )?;
        fs_security::set_private_file(&self.path)?;
        Ok(self.conn.last_insert_rowid() as u64)
    }

    /// Get a doc by id
    pub fn get(&self, id: u64) -> Result<Option<MemoryDoc>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, familiar, chunk, chunk_offset, content_hash, ingested_at
             FROM docs WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id as i64])?;
        if let Some(row) = rows.next()? {
            Ok(Some(MemoryDoc {
                id: row.get::<_, i64>(0)? as u64,
                path: row.get(1)?,
                familiar: row.get(2)?,
                chunk: row.get(3)?,
                chunk_offset: row.get::<_, i64>(4)? as usize,
                content_hash: row.get(5)?,
                ingested_at: row.get(6)?,
            }))
        } else {
            Ok(None)
        }
    }

    /// Get multiple docs by ids (for batch result lookup)
    pub fn get_many(&self, ids: &[u64]) -> Result<Vec<MemoryDoc>> {
        if ids.is_empty() {
            return Ok(vec![]);
        }
        // Build parameterised query
        let placeholders: String = ids
            .iter()
            .enumerate()
            .map(|(i, _)| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT id, path, familiar, chunk, chunk_offset, content_hash, ingested_at
             FROM docs WHERE id IN ({})",
            placeholders
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let params_vec: Vec<rusqlite::types::Value> = ids
            .iter()
            .map(|&id| rusqlite::types::Value::Integer(id as i64))
            .collect();
        let rows = stmt.query(rusqlite::params_from_iter(params_vec.iter()))?;
        let mut docs = Vec::new();
        let mut rows = rows;
        while let Some(row) = rows.next()? {
            docs.push(MemoryDoc {
                id: row.get::<_, i64>(0)? as u64,
                path: row.get(1)?,
                familiar: row.get(2)?,
                chunk: row.get(3)?,
                chunk_offset: row.get::<_, i64>(4)? as usize,
                content_hash: row.get(5)?,
                ingested_at: row.get(6)?,
            });
        }
        Ok(docs)
    }

    /// Delete a doc by id
    pub fn delete(&self, id: u64) -> Result<()> {
        self.conn
            .execute("DELETE FROM docs WHERE id = ?1", params![id as i64])?;
        fs_security::set_private_file(&self.path)?;
        Ok(())
    }

    /// Check if a content hash already exists (staleness / dedup)
    pub fn hash_exists(&self, hash: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM docs WHERE content_hash = ?1",
            params![hash],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// List all ids for a given familiar (for allowlist-filtered search)
    pub fn ids_for_familiar(&self, familiar: &str) -> Result<Vec<u64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM docs WHERE familiar = ?1")?;
        let ids = stmt
            .query_map(params![familiar], |row| row.get::<_, i64>(0))?
            .map(|r| r.map(|id| id as u64))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    /// Total doc count
    pub fn count(&self) -> Result<u64> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM docs", [], |row| row.get(0))?;
        Ok(n as u64)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn open_and_insert_harden_database_and_sidecars() {
        let root = std::env::temp_dir().join(format!(
            "coven-memory-db-security-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let db_path = root.join(".coven/memory/archival.sqlite3");
        let db = MetaDb::open(&db_path).unwrap();
        db.insert(&MemoryDoc {
            id: 0,
            path: "/private/note.txt".to_string(),
            familiar: "coven".to_string(),
            chunk: "secret chunk".to_string(),
            chunk_offset: 0,
            content_hash: "hash".to_string(),
            ingested_at: 0,
        })
        .unwrap();

        assert_eq!(mode(&root.join(".coven")), 0o700);
        assert_eq!(mode(&root.join(".coven/memory")), 0o700);
        assert_eq!(mode(&db_path), 0o600);
        for suffix in ["-wal", "-shm"] {
            let sidecar = PathBuf::from(format!("{}{}", db_path.display(), suffix));
            if sidecar.exists() {
                assert_eq!(mode(&sidecar), 0o600);
            }
        }

        drop(db);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }
}
