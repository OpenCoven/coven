//! Inode-addressed operations.
//!
//! The path-addressed API in [`crate::fs`] is what callers want; a mount
//! backend cannot use it. NFSv3 and FUSE both address objects by a 64-bit file
//! id and a `(parent, name)` pair — they never hand the filesystem a path —
//! and reconstructing a path from an inode would cost a walk up `fs_dentry` on
//! every operation.
//!
//! These operations are the same SPEC v0.4 semantics as their path-addressed
//! counterparts, expressed against inode numbers. No new tables and no new
//! columns: this module is pure query shape.

use rusqlite::{params, OptionalExtension};

use crate::fs::{Metadata, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG};
use crate::{now_parts, AgentFs, Error, Result};

/// One directory entry with its metadata, as a mount backend needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub name: String,
    pub meta: Metadata,
}

const METADATA_COLUMNS: &str = "i.ino, i.mode, i.nlink, i.uid, i.gid, i.size, i.atime, i.mtime,
     i.ctime, i.rdev, i.atime_nsec, i.mtime_nsec, i.ctime_nsec";

fn metadata_from_row(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<Metadata> {
    Ok(Metadata {
        ino: row.get(offset)?,
        mode: row.get(offset + 1)?,
        nlink: row.get(offset + 2)?,
        uid: row.get(offset + 3)?,
        gid: row.get(offset + 4)?,
        size: row.get(offset + 5)?,
        atime: row.get(offset + 6)?,
        mtime: row.get(offset + 7)?,
        ctime: row.get(offset + 8)?,
        rdev: row.get(offset + 9)?,
        atime_nsec: row.get(offset + 10)?,
        mtime_nsec: row.get(offset + 11)?,
        ctime_nsec: row.get(offset + 12)?,
    })
}

impl AgentFs {
    fn writable(&self) -> Result<()> {
        if self.is_read_only() {
            Err(Error::ReadOnly)
        } else {
            Ok(())
        }
    }

    fn require_dir(&self, ino: i64) -> Result<()> {
        if self.stat_ino(ino)?.is_dir() {
            Ok(())
        } else {
            Err(Error::NotADirectory(format!("inode {ino}")))
        }
    }

    // ---- lookup ----------------------------------------------------------

    /// Resolve one path component inside a directory inode.
    pub fn lookup_ino(&self, parent_ino: i64, name: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT ino FROM fs_dentry WHERE parent_ino = ?1 AND name = ?2",
                params![parent_ino, name],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// List a directory's entries with metadata, ordered by inode.
    ///
    /// Ordering by inode rather than name is what makes NFSv3's `start_after`
    /// pagination exact: resuming is a `WHERE ino > start_after`, not a scan
    /// for a name whose neighbours may have shifted between calls.
    pub fn readdir_ino(
        &self,
        parent_ino: i64,
        start_after: i64,
        limit: usize,
    ) -> Result<Vec<DirEntry>> {
        self.require_dir(parent_ino)?;
        let sql = format!(
            "SELECT d.name, {METADATA_COLUMNS}
             FROM fs_dentry d JOIN fs_inode i ON i.ino = d.ino
             WHERE d.parent_ino = ?1 AND d.ino > ?2
             ORDER BY d.ino ASC LIMIT ?3"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let entries = stmt
            .query_map(params![parent_ino, start_after, limit as i64], |r| {
                Ok(DirEntry {
                    name: r.get(0)?,
                    meta: metadata_from_row(r, 1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(entries)
    }

    /// The directory holding an inode, or `None` for the root and for any
    /// inode with no dentry. A hard-linked file has several parents; this
    /// returns the lowest-numbered one, which is enough for resolving `..`.
    pub fn parent_of(&self, ino: i64) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT parent_ino FROM fs_dentry WHERE ino = ?1 ORDER BY parent_ino ASC LIMIT 1",
                [ino],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Number of entries in a directory.
    pub fn child_count(&self, parent_ino: i64) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM fs_dentry WHERE parent_ino = ?1",
            [parent_ino],
            |r| r.get(0),
        )?)
    }

    // ---- reads -----------------------------------------------------------

    /// Read `count` bytes at `offset` from a file inode.
    ///
    /// Returns the bytes and whether the read reached end of file, which is
    /// the shape both NFSv3 READ and FUSE expect.
    pub fn read_ino_at(&self, ino: i64, offset: u64, count: usize) -> Result<(Vec<u8>, bool)> {
        let meta = self.stat_ino(ino)?;
        if !meta.is_file() {
            return Err(Error::NotARegularFile(format!("inode {ino}")));
        }
        let size = meta.size as u64;
        if offset >= size || count == 0 {
            return Ok((Vec::new(), true));
        }
        let chunk = self.chunk_size() as u64;
        let end = (offset + count as u64).min(size);
        let first = (offset / chunk) as i64;
        let last = ((end - 1) / chunk) as i64;
        let mut stmt = self.conn.prepare(
            "SELECT chunk_index, data FROM fs_data
             WHERE ino = ?1 AND chunk_index >= ?2 AND chunk_index <= ?3
             ORDER BY chunk_index ASC",
        )?;
        let mut out = vec![0_u8; (end - offset) as usize];
        for row in stmt.query_map(params![ino, first, last], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
        })? {
            let (index, data) = row?;
            // Where this chunk lands in the output window, clipped at both ends.
            let chunk_start = index as u64 * chunk;
            let copy_start = chunk_start.max(offset);
            let copy_end = (chunk_start + data.len() as u64).min(end);
            if copy_end <= copy_start {
                continue;
            }
            let dst = (copy_start - offset) as usize;
            let src = (copy_start - chunk_start) as usize;
            let len = (copy_end - copy_start) as usize;
            out[dst..dst + len].copy_from_slice(&data[src..src + len]);
        }
        Ok((out, end >= size))
    }

    /// Read a symlink target by inode.
    pub fn readlink_ino(&self, ino: i64) -> Result<String> {
        if !self.stat_ino(ino)?.is_symlink() {
            return Err(Error::NotASymlink(format!("inode {ino}")));
        }
        Ok(self
            .conn
            .query_row("SELECT target FROM fs_symlink WHERE ino = ?1", [ino], |r| {
                r.get(0)
            })?)
    }

    // ---- writes ----------------------------------------------------------

    /// Write `data` at `offset` into a file inode, rewriting only the chunks
    /// the range actually touches.
    ///
    /// This is the operation a mount stands or falls on: a client writes a
    /// file in ~64 KiB chunks, so a whole-file rewrite per write would make
    /// writing an N-byte file cost O(N²). Holes created by writing past the
    /// end are materialized as zeroes, because [`AgentFs::read_ino_at`]
    /// reconstructs a file by chunk index and a missing chunk would otherwise
    /// silently shorten it.
    pub fn write_ino_at(&mut self, ino: i64, offset: u64, data: &[u8]) -> Result<Metadata> {
        self.writable()?;
        let meta = self.stat_ino(ino)?;
        if !meta.is_file() {
            return Err(Error::NotARegularFile(format!("inode {ino}")));
        }
        if data.is_empty() {
            return Ok(meta);
        }
        let chunk = self.chunk_size() as u64;
        let size = meta.size as u64;
        let end = offset + data.len() as u64;
        let new_size = size.max(end);
        let first = offset / chunk;
        let last = (end - 1) / chunk;
        let (secs, nsec) = now_parts();

        let tx = self.conn.transaction()?;
        // Materialize any hole between the old end of file and the write.
        if offset > size {
            for index in (size / chunk)..first {
                let start = index * chunk;
                let want = (chunk).min(new_size - start) as usize;
                let existing: Option<Vec<u8>> = tx
                    .query_row(
                        "SELECT data FROM fs_data WHERE ino = ?1 AND chunk_index = ?2",
                        params![ino, index as i64],
                        |r| r.get(0),
                    )
                    .optional()?;
                let mut buf = existing.unwrap_or_default();
                if buf.len() < want {
                    buf.resize(want, 0);
                    tx.execute(
                        "INSERT INTO fs_data (ino, chunk_index, data) VALUES (?1, ?2, ?3)
                         ON CONFLICT(ino, chunk_index) DO UPDATE SET data = excluded.data",
                        params![ino, index as i64, buf],
                    )?;
                }
            }
        }
        for index in first..=last {
            let start = index * chunk;
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT data FROM fs_data WHERE ino = ?1 AND chunk_index = ?2",
                    params![ino, index as i64],
                    |r| r.get(0),
                )
                .optional()?;
            let mut buf = existing.unwrap_or_default();
            let want = chunk.min(new_size - start) as usize;
            if buf.len() < want {
                buf.resize(want, 0);
            }
            let write_start = offset.max(start);
            let write_end = end.min(start + chunk);
            let dst = (write_start - start) as usize;
            let src = (write_start - offset) as usize;
            let len = (write_end - write_start) as usize;
            buf[dst..dst + len].copy_from_slice(&data[src..src + len]);
            tx.execute(
                "INSERT INTO fs_data (ino, chunk_index, data) VALUES (?1, ?2, ?3)
                 ON CONFLICT(ino, chunk_index) DO UPDATE SET data = excluded.data",
                params![ino, index as i64, buf],
            )?;
        }
        tx.execute(
            "UPDATE fs_inode SET size = ?1, mtime = ?2, mtime_nsec = ?3,
                                 ctime = ?2, ctime_nsec = ?3
             WHERE ino = ?4",
            params![new_size as i64, secs, nsec, ino],
        )?;
        tx.commit()?;
        self.stat_ino(ino)
    }

    /// Set a file's length, dropping or zero-extending chunks as needed.
    pub fn truncate_ino(&mut self, ino: i64, size: u64) -> Result<Metadata> {
        self.writable()?;
        let meta = self.stat_ino(ino)?;
        if !meta.is_file() {
            return Err(Error::NotARegularFile(format!("inode {ino}")));
        }
        let old = meta.size as u64;
        if size == old {
            return Ok(meta);
        }
        if size > old {
            // Grow by writing one zero byte at the new end; the hole-filling
            // path above materializes everything before it.
            self.write_ino_at(ino, size - 1, &[0])?;
            return self.stat_ino(ino);
        }
        let chunk = self.chunk_size() as u64;
        let (secs, nsec) = now_parts();
        let tx = self.conn.transaction()?;
        if size == 0 {
            tx.execute("DELETE FROM fs_data WHERE ino = ?1", [ino])?;
        } else {
            let last = (size - 1) / chunk;
            tx.execute(
                "DELETE FROM fs_data WHERE ino = ?1 AND chunk_index > ?2",
                params![ino, last as i64],
            )?;
            let keep = (size - last * chunk) as usize;
            let existing: Option<Vec<u8>> = tx
                .query_row(
                    "SELECT data FROM fs_data WHERE ino = ?1 AND chunk_index = ?2",
                    params![ino, last as i64],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(mut buf) = existing {
                if buf.len() > keep {
                    buf.truncate(keep);
                    tx.execute(
                        "UPDATE fs_data SET data = ?3 WHERE ino = ?1 AND chunk_index = ?2",
                        params![ino, last as i64, buf],
                    )?;
                }
            }
        }
        tx.execute(
            "UPDATE fs_inode SET size = ?1, mtime = ?2, mtime_nsec = ?3,
                                 ctime = ?2, ctime_nsec = ?3
             WHERE ino = ?4",
            params![size as i64, secs, nsec, ino],
        )?;
        tx.commit()?;
        self.stat_ino(ino)
    }

    /// Update ownership, permission bits, and timestamps. `None` leaves a
    /// field untouched; `mode` carries permission bits only, never the type.
    pub fn setattr_ino(
        &mut self,
        ino: i64,
        mode: Option<u32>,
        uid: Option<i64>,
        gid: Option<i64>,
        atime: Option<(i64, i64)>,
        mtime: Option<(i64, i64)>,
    ) -> Result<Metadata> {
        self.writable()?;
        let meta = self.stat_ino(ino)?;
        let (secs, nsec) = now_parts();
        let new_mode = mode.map_or(meta.mode, |bits| (meta.mode & S_IFMT) | (bits & 0o7777));
        let (atime_s, atime_ns) = atime.unwrap_or((meta.atime, meta.atime_nsec));
        let (mtime_s, mtime_ns) = mtime.unwrap_or((meta.mtime, meta.mtime_nsec));
        self.conn.execute(
            "UPDATE fs_inode SET mode = ?1, uid = ?2, gid = ?3,
                                 atime = ?4, atime_nsec = ?5,
                                 mtime = ?6, mtime_nsec = ?7,
                                 ctime = ?8, ctime_nsec = ?9
             WHERE ino = ?10",
            params![
                new_mode,
                uid.unwrap_or(meta.uid),
                gid.unwrap_or(meta.gid),
                atime_s,
                atime_ns,
                mtime_s,
                mtime_ns,
                secs,
                nsec,
                ino
            ],
        )?;
        self.stat_ino(ino)
    }

    // ---- namespace -------------------------------------------------------

    fn insert_child(
        &mut self,
        parent_ino: i64,
        name: &str,
        mode: u32,
        size: i64,
        symlink_target: Option<&str>,
    ) -> Result<(i64, Metadata)> {
        self.writable()?;
        self.require_dir(parent_ino)?;
        if self.lookup_ino(parent_ino, name)?.is_some() {
            return Err(Error::AlreadyExists(name.to_string()));
        }
        let (secs, nsec) = now_parts();
        let tx = self.conn.transaction()?;
        let ino: i64 = tx.query_row(
            "INSERT INTO fs_inode (mode, uid, gid, size, atime, mtime, ctime,
                                   atime_nsec, mtime_nsec, ctime_nsec)
             VALUES (?1, 0, 0, ?2, ?3, ?3, ?3, ?4, ?4, ?4)
             RETURNING ino",
            params![mode, size, secs, nsec],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO fs_dentry (name, parent_ino, ino) VALUES (?1, ?2, ?3)",
            params![name, parent_ino, ino],
        )?;
        tx.execute(
            "UPDATE fs_inode SET nlink = nlink + 1 WHERE ino = ?1",
            [ino],
        )?;
        if let Some(target) = symlink_target {
            tx.execute(
                "INSERT INTO fs_symlink (ino, target) VALUES (?1, ?2)",
                params![ino, target],
            )?;
        }
        tx.commit()?;
        let meta = self.stat_ino(ino)?;
        Ok((ino, meta))
    }

    /// Create an empty regular file in a directory.
    pub fn create_child(
        &mut self,
        parent_ino: i64,
        name: &str,
        mode: u32,
    ) -> Result<(i64, Metadata)> {
        self.insert_child(parent_ino, name, S_IFREG | (mode & 0o7777), 0, None)
    }

    /// Create a subdirectory.
    pub fn mkdir_ino(&mut self, parent_ino: i64, name: &str, mode: u32) -> Result<(i64, Metadata)> {
        self.insert_child(parent_ino, name, S_IFDIR | (mode & 0o7777), 0, None)
    }

    /// Create a symlink.
    pub fn symlink_ino(
        &mut self,
        parent_ino: i64,
        name: &str,
        target: &str,
    ) -> Result<(i64, Metadata)> {
        self.insert_child(
            parent_ino,
            name,
            S_IFLNK | 0o777,
            target.len() as i64,
            Some(target),
        )
    }

    /// Remove a directory entry: an empty directory, or one link to a file or
    /// symlink (dropping the inode and its data on the last link).
    pub fn remove_ino(&mut self, parent_ino: i64, name: &str) -> Result<()> {
        self.writable()?;
        let ino = self
            .lookup_ino(parent_ino, name)?
            .ok_or_else(|| Error::NotFound(name.to_string()))?;
        let meta = self.stat_ino(ino)?;
        if meta.is_dir() && self.child_count(ino)? > 0 {
            return Err(Error::DirectoryNotEmpty(name.to_string()));
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM fs_dentry WHERE parent_ino = ?1 AND name = ?2",
            params![parent_ino, name],
        )?;
        if meta.is_dir() {
            tx.execute("DELETE FROM fs_inode WHERE ino = ?1", [ino])?;
        } else {
            tx.execute(
                "UPDATE fs_inode SET nlink = nlink - 1 WHERE ino = ?1",
                [ino],
            )?;
            let nlink: i64 =
                tx.query_row("SELECT nlink FROM fs_inode WHERE ino = ?1", [ino], |r| {
                    r.get(0)
                })?;
            if nlink <= 0 {
                tx.execute("DELETE FROM fs_inode WHERE ino = ?1", [ino])?;
                tx.execute("DELETE FROM fs_data WHERE ino = ?1", [ino])?;
                tx.execute("DELETE FROM fs_symlink WHERE ino = ?1", [ino])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Move an entry between directories, replacing an existing destination.
    pub fn rename_ino(
        &mut self,
        from_parent: i64,
        from_name: &str,
        to_parent: i64,
        to_name: &str,
    ) -> Result<()> {
        self.writable()?;
        let ino = self
            .lookup_ino(from_parent, from_name)?
            .ok_or_else(|| Error::NotFound(from_name.to_string()))?;
        if from_parent == to_parent && from_name == to_name {
            return Ok(());
        }
        self.require_dir(to_parent)?;
        if let Some(existing) = self.lookup_ino(to_parent, to_name)? {
            if existing != ino {
                self.remove_ino(to_parent, to_name)?;
            }
        }
        let (secs, nsec) = now_parts();
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE fs_dentry SET name = ?1, parent_ino = ?2
             WHERE parent_ino = ?3 AND name = ?4",
            params![to_name, to_parent, from_parent, from_name],
        )?;
        tx.execute(
            "UPDATE fs_inode SET ctime = ?1, ctime_nsec = ?2 WHERE ino = ?3",
            params![secs, nsec, ino],
        )?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fs() -> AgentFs {
        AgentFs::in_memory_with_chunk_size(16).unwrap()
    }

    #[test]
    fn partial_write_only_rewrites_touched_chunks() {
        let mut fs = fs();
        let ino = fs.write_file("/a.bin", &[b'a'; 64]).unwrap();
        fs.write_ino_at(ino, 20, b"ZZZZ").unwrap();
        let (data, eof) = fs.read_ino_at(ino, 0, 64).unwrap();
        let mut want = vec![b'a'; 64];
        want[20..24].copy_from_slice(b"ZZZZ");
        assert_eq!(data, want);
        assert!(eof);
        assert_eq!(fs.stat_ino(ino).unwrap().size, 64);
    }

    #[test]
    fn sequential_chunked_write_reconstructs_the_whole_file() {
        let mut fs = fs();
        let (ino, _) = fs.create_child(1, "big.bin", 0o644).unwrap();
        let payload: Vec<u8> = (0..250_u32).map(|i| (i % 251) as u8).collect();
        for (index, piece) in payload.chunks(7).enumerate() {
            fs.write_ino_at(ino, (index * 7) as u64, piece).unwrap();
        }
        let (data, _) = fs.read_ino_at(ino, 0, payload.len()).unwrap();
        assert_eq!(data, payload);
    }

    #[test]
    fn writing_past_the_end_zero_fills_the_hole() {
        let mut fs = fs();
        let (ino, _) = fs.create_child(1, "sparse.bin", 0o644).unwrap();
        fs.write_ino_at(ino, 0, b"head").unwrap();
        fs.write_ino_at(ino, 100, b"tail").unwrap();
        let (data, _) = fs.read_ino_at(ino, 0, 104).unwrap();
        assert_eq!(data.len(), 104);
        assert_eq!(&data[0..4], b"head");
        assert_eq!(&data[100..104], b"tail");
        assert!(data[4..100].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn truncate_shrinks_and_grows() {
        let mut fs = fs();
        let ino = fs.write_file("/t.bin", &[b'x'; 100]).unwrap();
        fs.truncate_ino(ino, 10).unwrap();
        let (data, _) = fs.read_ino_at(ino, 0, 100).unwrap();
        assert_eq!(data, vec![b'x'; 10]);
        fs.truncate_ino(ino, 40).unwrap();
        let (grown, _) = fs.read_ino_at(ino, 0, 100).unwrap();
        assert_eq!(grown.len(), 40);
        assert_eq!(&grown[0..10], &vec![b'x'; 10][..]);
        assert!(grown[10..].iter().all(|byte| *byte == 0));
    }

    #[test]
    fn readdir_paginates_by_inode_without_repeating_entries() {
        let mut fs = fs();
        let (dir, _) = fs.mkdir_ino(1, "d", 0o755).unwrap();
        for index in 0..10 {
            fs.create_child(dir, &format!("f{index}"), 0o644).unwrap();
        }
        let mut seen = Vec::new();
        let mut cursor = 0;
        loop {
            let page = fs.readdir_ino(dir, cursor, 3).unwrap();
            if page.is_empty() {
                break;
            }
            cursor = page.last().unwrap().meta.ino;
            seen.extend(page.into_iter().map(|entry| entry.name));
        }
        seen.sort();
        let mut want: Vec<String> = (0..10).map(|index| format!("f{index}")).collect();
        want.sort();
        assert_eq!(seen, want);
    }

    #[test]
    fn rename_replaces_the_destination_and_remove_frees_data() {
        let mut fs = fs();
        let (dir, _) = fs.mkdir_ino(1, "d", 0o755).unwrap();
        let src = fs.write_file("/d/src", b"source").unwrap();
        let dst = fs.write_file("/d/dst", b"destination").unwrap();
        fs.rename_ino(dir, "src", dir, "dst").unwrap();
        assert_eq!(fs.lookup_ino(dir, "src").unwrap(), None);
        assert_eq!(fs.lookup_ino(dir, "dst").unwrap(), Some(src));
        assert!(fs.stat_ino(dst).is_err());
        assert_eq!(fs.read_file("/d/dst").unwrap(), b"source");

        fs.remove_ino(dir, "dst").unwrap();
        let orphaned: i64 = fs
            .conn
            .query_row("SELECT COUNT(*) FROM fs_data WHERE ino = ?1", [src], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(orphaned, 0);
        assert!(fs.check_consistency().unwrap().is_empty());
    }

    #[test]
    fn non_empty_directory_removal_is_refused() {
        let mut fs = fs();
        let (dir, _) = fs.mkdir_ino(1, "d", 0o755).unwrap();
        fs.create_child(dir, "child", 0o644).unwrap();
        assert!(matches!(
            fs.remove_ino(1, "d"),
            Err(Error::DirectoryNotEmpty(_))
        ));
    }

    #[test]
    fn setattr_preserves_the_file_type_bits() {
        let mut fs = fs();
        let (ino, _) = fs.create_child(1, "f", 0o644).unwrap();
        let meta = fs
            .setattr_ino(ino, Some(0o600), Some(501), Some(20), None, None)
            .unwrap();
        assert!(meta.is_file());
        assert_eq!(meta.mode & 0o7777, 0o600);
        assert_eq!(meta.uid, 501);
        assert_eq!(meta.gid, 20);
    }

    #[test]
    fn symlinks_round_trip_by_inode() {
        let mut fs = fs();
        let (ino, meta) = fs.symlink_ino(1, "link", "/target/path").unwrap();
        assert!(meta.is_symlink());
        assert_eq!(fs.readlink_ino(ino).unwrap(), "/target/path");
    }
}
