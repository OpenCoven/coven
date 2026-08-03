//! coven-memory — archival memory layer for Coven familiars
//!
//! Combines:
//! - fastembed (nomic-embed-text-v1.5, 768-dim) for local, air-gapped embeddings
//! - turbovec IdMapIndex (4-bit TurboQuant) for compressed, fast ANN search
//! - SQLite metadata store for document records, stable ids, and staleness tracking

pub mod db;
pub mod embed;
pub mod index;
pub mod ingest;
pub mod promotion;

use std::path::PathBuf;

/// Default paths under ~/.coven/memory/
pub fn default_memory_dir() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".coven/memory")
}

pub fn default_index_path() -> PathBuf {
    default_memory_dir().join("archival.tvim")
}

pub fn default_db_path() -> PathBuf {
    default_memory_dir().join("archival.sqlite3")
}

/// A single memory document record
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryDoc {
    /// Stable u64 id (primary key in index + db)
    pub id: u64,
    /// Source path on disk
    pub path: String,
    /// Familiar this belongs to (e.g. "sage", "echo", "coven")
    pub familiar: String,
    /// Chunk text (the actual content embedded)
    pub chunk: String,
    /// Byte offset of chunk in source file
    pub chunk_offset: usize,
    /// SHA-256 hex of the chunk text (for staleness detection)
    pub content_hash: String,
    /// Unix timestamp when ingested
    pub ingested_at: i64,
}

/// Search result
#[derive(Debug)]
pub struct SearchResult {
    pub doc: MemoryDoc,
    pub score: f32,
}
