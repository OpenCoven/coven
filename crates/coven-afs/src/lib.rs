//! # coven-afs
//!
//! Native OpenCoven agent filesystem: an [AgentFS SPEC v0.4]-compatible
//! SQLite storage layer with copy-on-write overlay semantics.
//!
//! One SQLite database holds three interfaces:
//!
//! - a POSIX-ish virtual filesystem (`fs_inode` / `fs_dentry` / `fs_data`
//!   chunked blobs, hard links, symlinks),
//! - an insert-only `tool_calls` audit log,
//! - a JSON `kv_store`.
//!
//! [`OverlayFs`] layers a writable delta database over a read-only base
//! database with whiteouts (`fs_whiteout`), full-file copy-up, and kernel
//! inode-cache preservation via `fs_origin`.
//!
//! Spec compatibility is deliberate: databases produced by this crate are
//! readable by upstream `agentfs` tooling and vice versa. See
//! `specs/coven-agent-fs/RESEARCH.md` for the design rationale.
//!
//! This spike intentionally excludes any mount layer (FUSE/NFS); it is the
//! storage engine only.
//!
//! [AgentFS SPEC v0.4]: https://github.com/tursodatabase/agentfs/blob/main/SPEC.md

mod audit;
mod fs;
mod kv;
mod overlay;
mod path;
mod schema;

pub use audit::ToolCall;
pub use fs::{AgentFs, Metadata, ROOT_INO, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG};
pub use overlay::OverlayFs;
pub use path::{basename, components, normalize, parent};

/// Errors produced by coven-afs operations.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("not a directory: {0}")]
    NotADirectory(String),
    #[error("is a directory: {0}")]
    IsADirectory(String),
    #[error("directory not empty: {0}")]
    DirectoryNotEmpty(String),
    #[error("not a regular file: {0}")]
    NotARegularFile(String),
    #[error("not a symlink: {0}")]
    NotASymlink(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    #[error("filesystem is read-only")]
    ReadOnly,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Current Unix time as `(seconds, nanosecond remainder)`.
pub(crate) fn now_parts() -> (i64, i64) {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    (d.as_secs() as i64, i64::from(d.subsec_nanos()))
}
