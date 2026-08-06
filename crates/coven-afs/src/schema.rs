//! Schema DDL and initialization, verbatim from AgentFS SPEC v0.4.
//!
//! Table-name note (research conflict C1): the overlay *guide* upstream shows
//! `fs_block`; the SPEC and every upstream SDK (Rust/Go/Python/TS `schema` /
//! `filesystem` sources) use `fs_data`. `fs_data` is frozen here.

use rusqlite::Connection;

use crate::Result;

/// Default chunk size in bytes (SPEC required config default).
pub const DEFAULT_CHUNK_SIZE: usize = 4096;

/// Root directory mode: `0o040755` (directory, rwxr-xr-x) = 16877.
const ROOT_MODE: i64 = 0o040755;

const DDL: &str = "
CREATE TABLE IF NOT EXISTS tool_calls (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  parameters TEXT,
  result TEXT,
  error TEXT,
  started_at INTEGER NOT NULL,
  completed_at INTEGER NOT NULL,
  duration_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_tool_calls_name ON tool_calls(name);
CREATE INDEX IF NOT EXISTS idx_tool_calls_started_at ON tool_calls(started_at);

CREATE TABLE IF NOT EXISTS fs_config (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS fs_inode (
  ino INTEGER PRIMARY KEY AUTOINCREMENT,
  mode INTEGER NOT NULL,
  nlink INTEGER NOT NULL DEFAULT 0,
  uid INTEGER NOT NULL DEFAULT 0,
  gid INTEGER NOT NULL DEFAULT 0,
  size INTEGER NOT NULL DEFAULT 0,
  atime INTEGER NOT NULL,
  mtime INTEGER NOT NULL,
  ctime INTEGER NOT NULL,
  rdev INTEGER NOT NULL DEFAULT 0,
  atime_nsec INTEGER NOT NULL DEFAULT 0,
  mtime_nsec INTEGER NOT NULL DEFAULT 0,
  ctime_nsec INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS fs_dentry (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  name TEXT NOT NULL,
  parent_ino INTEGER NOT NULL,
  ino INTEGER NOT NULL,
  UNIQUE(parent_ino, name)
);
CREATE INDEX IF NOT EXISTS idx_fs_dentry_parent ON fs_dentry(parent_ino, name);

CREATE TABLE IF NOT EXISTS fs_data (
  ino INTEGER NOT NULL,
  chunk_index INTEGER NOT NULL,
  data BLOB NOT NULL,
  PRIMARY KEY (ino, chunk_index)
);

CREATE TABLE IF NOT EXISTS fs_symlink (
  ino INTEGER PRIMARY KEY,
  target TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS fs_whiteout (
  path TEXT PRIMARY KEY,
  parent_path TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_fs_whiteout_parent ON fs_whiteout(parent_path);

CREATE TABLE IF NOT EXISTS fs_origin (
  delta_ino INTEGER PRIMARY KEY,
  base_ino INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS kv_store (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  created_at INTEGER DEFAULT (unixepoch()),
  updated_at INTEGER DEFAULT (unixepoch())
);
CREATE INDEX IF NOT EXISTS idx_kv_store_created_at ON kv_store(created_at);
";

/// Create all tables and initialize `fs_config` + root inode (ino 1,
/// mode `0o040755`, `nlink=1` — the root has no parent dentry).
///
/// Idempotent: safe to call on an already-initialized database. An existing
/// `chunk_size` is never modified (configuration is immutable after
/// filesystem initialization).
pub fn init(conn: &Connection, chunk_size: usize) -> Result<()> {
    conn.execute_batch(DDL)?;
    conn.execute(
        "INSERT OR IGNORE INTO fs_config (key, value) VALUES ('chunk_size', ?1)",
        [chunk_size.to_string()],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO fs_inode (ino, mode, nlink, uid, gid, size, atime, mtime, ctime)
         VALUES (1, ?1, 1, 0, 0, 0, unixepoch(), unixepoch(), unixepoch())",
        [ROOT_MODE],
    )?;
    Ok(())
}

/// Read the immutable `chunk_size` from `fs_config`.
pub fn chunk_size(conn: &Connection) -> Result<usize> {
    let raw: String = conn.query_row(
        "SELECT value FROM fs_config WHERE key = 'chunk_size'",
        [],
        |r| r.get(0),
    )?;
    raw.parse::<usize>().map_err(|_| {
        crate::Error::InvalidArgument(format!("invalid chunk_size in fs_config: {raw}"))
    })
}
