//! Copy-on-write overlay: a writable delta filesystem layered over a
//! read-only base filesystem (SPEC overlay section).
//!
//! Lookup order is always: **delta → whiteout → base → not found.**

use rusqlite::{params, OptionalExtension};

use crate::fs::{AgentFs, Metadata};
use crate::path::{normalize, parent};
use crate::{now_parts, Error, Result};

/// A copy-on-write overlay filesystem.
///
/// Whiteouts, origin mappings, and all writes live in the delta database;
/// the base database is never modified.
pub struct OverlayFs {
    delta: AgentFs,
    base: AgentFs,
}

impl OverlayFs {
    /// Layer a writable `delta` over a read-only `base`.
    pub fn new(delta: AgentFs, base: AgentFs) -> Result<Self> {
        if delta.is_read_only() {
            return Err(Error::ReadOnly);
        }
        Ok(Self { delta, base })
    }

    /// Open `delta_path` writable (creating it if missing) over `base_path`
    /// opened read-only.
    pub fn open<P: AsRef<std::path::Path>, Q: AsRef<std::path::Path>>(
        delta_path: P,
        base_path: Q,
    ) -> Result<Self> {
        let base = AgentFs::open_read_only(base_path)?;
        let delta = AgentFs::create_with_chunk_size(delta_path, base.chunk_size())?;
        Self::new(delta, base)
    }

    /// The writable delta layer.
    pub fn delta(&self) -> &AgentFs {
        &self.delta
    }

    /// The read-only base layer.
    pub fn base(&self) -> &AgentFs {
        &self.base
    }

    // ---- whiteouts ---------------------------------------------------------

    /// Whether `path` has a whiteout in the delta layer.
    pub fn has_whiteout(&self, path: &str) -> Result<bool> {
        let npath = normalize(path);
        Ok(self
            .delta
            .conn
            .query_row("SELECT 1 FROM fs_whiteout WHERE path = ?1", [npath], |_| {
                Ok(())
            })
            .optional()?
            .is_some())
    }

    fn create_whiteout(&self, npath: &str) -> Result<()> {
        let (secs, _) = now_parts();
        self.delta.conn.execute(
            "INSERT INTO fs_whiteout (path, parent_path, created_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET created_at = excluded.created_at",
            params![npath, parent(npath), secs],
        )?;
        Ok(())
    }

    /// Rule: a whiteout MUST be removed when a new file is created at that
    /// path.
    fn remove_whiteout(&self, npath: &str) -> Result<()> {
        self.delta
            .conn
            .execute("DELETE FROM fs_whiteout WHERE path = ?1", [npath])?;
        Ok(())
    }

    fn child_whiteouts(&self, dir_npath: &str) -> Result<Vec<String>> {
        let mut stmt = self
            .delta
            .conn
            .prepare("SELECT path FROM fs_whiteout WHERE parent_path = ?1")?;
        let rows = stmt
            .query_map([dir_npath], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    // ---- origin tracking ---------------------------------------------------

    /// Origin inode for a delta inode, if the file was copied up from base.
    pub fn origin_ino(&self, delta_ino: i64) -> Result<Option<i64>> {
        Ok(self
            .delta
            .conn
            .query_row(
                "SELECT base_ino FROM fs_origin WHERE delta_ino = ?1",
                [delta_ino],
                |r| r.get(0),
            )
            .optional()?)
    }

    fn store_origin(&self, delta_ino: i64, base_ino: i64) -> Result<()> {
        self.delta.conn.execute(
            "INSERT OR REPLACE INTO fs_origin (delta_ino, base_ino) VALUES (?1, ?2)",
            params![delta_ino, base_ino],
        )?;
        Ok(())
    }

    // ---- overlay operations ------------------------------------------------

    /// Overlay existence check following delta → whiteout → base.
    pub fn exists(&self, path: &str) -> Result<bool> {
        let npath = normalize(path);
        if self.delta.exists(&npath)? {
            return Ok(true);
        }
        if self.has_whiteout(&npath)? {
            return Ok(false);
        }
        self.base.exists(&npath)
    }

    /// Overlay stat. For delta inodes with an origin mapping, the base inode
    /// number is returned (SPEC: preserves kernel inode caches across
    /// copy-up).
    pub fn stat(&self, path: &str) -> Result<Metadata> {
        let npath = normalize(path);
        if let Some(delta_ino) = self.delta.resolve(&npath)? {
            let mut meta = self.delta.stat_ino(delta_ino)?;
            if let Some(base_ino) = self.origin_ino(delta_ino)? {
                meta.ino = base_ino;
            }
            return Ok(meta);
        }
        if self.has_whiteout(&npath)? {
            return Err(Error::NotFound(npath));
        }
        self.base.stat(&npath)
    }

    /// Overlay read following delta → whiteout → base.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let npath = normalize(path);
        if self.delta.exists(&npath)? {
            return self.delta.read_file(&npath);
        }
        if self.has_whiteout(&npath)? {
            return Err(Error::NotFound(npath));
        }
        self.base.read_file(&npath)
    }

    /// Create or overwrite a file in the delta layer. Clears any whiteout at
    /// the path and along its ancestor directories; if the path shadows a
    /// base file, the base inode is recorded in `fs_origin`.
    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<i64> {
        let npath = normalize(path);
        let shadowed_base_ino = if self.delta.resolve(&npath)?.is_none() {
            match self.base.resolve(&npath)? {
                Some(ino) if self.base.stat_ino(ino)?.is_file() => Some(ino),
                _ => None,
            }
        } else {
            None
        };
        self.remove_whiteout(&npath)?;
        // Ancestor directories being re-created must also lose their whiteouts.
        let mut anc = parent(&npath);
        while anc != "/" {
            self.remove_whiteout(&anc)?;
            anc = parent(&anc);
        }
        let ino = self.delta.write_file(&npath, data)?;
        if let Some(base_ino) = shadowed_base_ino {
            self.store_origin(ino, base_ino)?;
        }
        Ok(ino)
    }

    /// Copy a file from the base layer into the delta layer (full-file
    /// copy-up), preserving metadata and recording the origin mapping.
    /// No-op if the path already lives in the delta layer.
    pub fn copy_up(&mut self, path: &str) -> Result<i64> {
        let npath = normalize(path);
        if let Some(ino) = self.delta.resolve(&npath)? {
            return Ok(ino);
        }
        if self.has_whiteout(&npath)? {
            return Err(Error::NotFound(npath));
        }
        let base_meta = self.base.stat(&npath)?;
        if !base_meta.is_file() {
            return Err(Error::NotARegularFile(npath));
        }
        let data = self.base.read_file(&npath)?;
        let delta_ino = self.delta.write_file(&npath, &data)?;
        self.delta.conn.execute(
            "UPDATE fs_inode SET mode = ?1, uid = ?2, gid = ?3,
                    atime = ?4, mtime = ?5, ctime = ?6,
                    atime_nsec = ?7, mtime_nsec = ?8, ctime_nsec = ?9
             WHERE ino = ?10",
            params![
                base_meta.mode,
                base_meta.uid,
                base_meta.gid,
                base_meta.atime,
                base_meta.mtime,
                base_meta.ctime,
                base_meta.atime_nsec,
                base_meta.mtime_nsec,
                base_meta.ctime_nsec,
                delta_ino
            ],
        )?;
        self.store_origin(delta_ino, base_meta.ino)?;
        Ok(delta_ino)
    }

    /// Delete a file from the overlay. Removes it from the delta layer when
    /// present; records a whiteout when the path exists in the base layer
    /// (SPEC rule 2).
    pub fn remove_file(&mut self, path: &str) -> Result<()> {
        let npath = normalize(path);
        let in_delta = self.delta.resolve(&npath)?.is_some();
        let in_base = self.base.exists(&npath)?;
        if !in_delta && (self.has_whiteout(&npath)? || !in_base) {
            return Err(Error::NotFound(npath));
        }
        if in_delta {
            self.delta.remove_file(&npath)?;
        }
        if in_base {
            self.create_whiteout(&npath)?;
        }
        Ok(())
    }

    /// Merged directory listing: delta entries plus base entries minus
    /// whiteouts, deduplicated, sorted ascending.
    pub fn readdir(&self, path: &str) -> Result<Vec<String>> {
        let npath = normalize(path);
        let delta_dir =
            matches!(self.delta.resolve(&npath)?, Some(ino) if self.delta.stat_ino(ino)?.is_dir());
        let whited_out = self.has_whiteout(&npath)?;
        let base_dir = !whited_out
            && matches!(self.base.resolve(&npath)?, Some(ino) if self.base.stat_ino(ino)?.is_dir());
        if !delta_dir && !base_dir {
            return Err(Error::NotFound(npath));
        }

        let mut names: Vec<String> = if delta_dir {
            self.delta.readdir(&npath)?
        } else {
            Vec::new()
        };
        if base_dir {
            let whiteouts = self.child_whiteouts(&npath)?;
            for name in self.base.readdir(&npath)? {
                let child = if npath == "/" {
                    format!("/{name}")
                } else {
                    format!("{npath}/{name}")
                };
                if whiteouts.contains(&child) || names.contains(&name) {
                    continue;
                }
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }
}
