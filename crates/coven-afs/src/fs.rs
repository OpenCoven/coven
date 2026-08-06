//! Single-layer virtual filesystem over one SQLite database.

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::path::{basename, components, normalize, parent};
use crate::{now_parts, schema, Error, Result};

/// Inode 1 MUST be the root directory (SPEC).
pub const ROOT_INO: i64 = 1;

/// File type mask (`S_IFMT`).
pub const S_IFMT: u32 = 0o170000;
/// Regular file (`S_IFREG`).
pub const S_IFREG: u32 = 0o100000;
/// Directory (`S_IFDIR`).
pub const S_IFDIR: u32 = 0o040000;
/// Symbolic link (`S_IFLNK`).
pub const S_IFLNK: u32 = 0o120000;

/// File metadata, mirroring an `fs_inode` row (SPEC stat operation).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metadata {
    pub ino: i64,
    pub mode: u32,
    pub nlink: i64,
    pub uid: i64,
    pub gid: i64,
    pub size: i64,
    pub atime: i64,
    pub mtime: i64,
    pub ctime: i64,
    pub rdev: i64,
    pub atime_nsec: i64,
    pub mtime_nsec: i64,
    pub ctime_nsec: i64,
}

impl Metadata {
    pub fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }

    pub fn is_file(&self) -> bool {
        self.mode & S_IFMT == S_IFREG
    }

    pub fn is_symlink(&self) -> bool {
        self.mode & S_IFMT == S_IFLNK
    }
}

/// A single-layer AgentFS-compatible filesystem.
pub struct AgentFs {
    pub(crate) conn: Connection,
    chunk_size: usize,
    read_only: bool,
}

impl AgentFs {
    /// Create or open a filesystem database at `path` with the default
    /// 4096-byte chunk size.
    pub fn create<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        Self::create_with_chunk_size(path, schema::DEFAULT_CHUNK_SIZE)
    }

    /// Create or open a filesystem database with an explicit chunk size.
    ///
    /// The chunk size only applies on first initialization; an existing
    /// database keeps its immutable configured value.
    pub fn create_with_chunk_size<P: AsRef<std::path::Path>>(
        path: P,
        chunk_size: usize,
    ) -> Result<Self> {
        if chunk_size == 0 {
            return Err(Error::InvalidArgument("chunk_size must be > 0".into()));
        }
        let conn = Connection::open(path)?;
        schema::init(&conn, chunk_size)?;
        Self::from_conn(conn, false)
    }

    /// Open an existing filesystem database read-only.
    ///
    /// Reads on a read-only filesystem skip `atime` maintenance; mutating
    /// operations fail with [`Error::ReadOnly`].
    pub fn open_read_only<P: AsRef<std::path::Path>>(path: P) -> Result<Self> {
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        Self::from_conn(conn, true)
    }

    /// Create an in-memory filesystem (tests, scratch space).
    pub fn in_memory() -> Result<Self> {
        Self::in_memory_with_chunk_size(schema::DEFAULT_CHUNK_SIZE)
    }

    /// Create an in-memory filesystem with an explicit chunk size.
    pub fn in_memory_with_chunk_size(chunk_size: usize) -> Result<Self> {
        if chunk_size == 0 {
            return Err(Error::InvalidArgument("chunk_size must be > 0".into()));
        }
        let conn = Connection::open_in_memory()?;
        schema::init(&conn, chunk_size)?;
        Self::from_conn(conn, false)
    }

    fn from_conn(conn: Connection, read_only: bool) -> Result<Self> {
        let chunk_size = schema::chunk_size(&conn)?;
        Ok(Self {
            conn,
            chunk_size,
            read_only,
        })
    }

    /// The immutable chunk size configured for this filesystem.
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    pub fn is_read_only(&self) -> bool {
        self.read_only
    }

    fn ensure_writable(&self) -> Result<()> {
        if self.read_only {
            Err(Error::ReadOnly)
        } else {
            Ok(())
        }
    }

    // ---- lookup ----------------------------------------------------------

    /// Resolve a path to an inode number (SPEC path resolution). Returns
    /// `None` if any component is missing.
    pub fn resolve(&self, path: &str) -> Result<Option<i64>> {
        let npath = normalize(path);
        let mut ino = ROOT_INO;
        for comp in components(&npath) {
            match self.lookup_child(ino, comp)? {
                Some(child) => ino = child,
                None => return Ok(None),
            }
        }
        Ok(Some(ino))
    }

    fn lookup_child(&self, parent_ino: i64, name: &str) -> Result<Option<i64>> {
        Ok(self
            .conn
            .query_row(
                "SELECT ino FROM fs_dentry WHERE parent_ino = ?1 AND name = ?2",
                params![parent_ino, name],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Whether a path exists.
    pub fn exists(&self, path: &str) -> Result<bool> {
        Ok(self.resolve(path)?.is_some())
    }

    /// Read file metadata (SPEC stat), including the link count.
    pub fn stat(&self, path: &str) -> Result<Metadata> {
        let ino = self
            .resolve(path)?
            .ok_or_else(|| Error::NotFound(normalize(path)))?;
        self.stat_ino(ino)
    }

    /// Read metadata for a known inode.
    pub fn stat_ino(&self, ino: i64) -> Result<Metadata> {
        self.conn
            .query_row(
                "SELECT ino, mode, nlink, uid, gid, size, atime, mtime, ctime, rdev,
                        atime_nsec, mtime_nsec, ctime_nsec
                 FROM fs_inode WHERE ino = ?1",
                [ino],
                |r| {
                    Ok(Metadata {
                        ino: r.get(0)?,
                        mode: r.get(1)?,
                        nlink: r.get(2)?,
                        uid: r.get(3)?,
                        gid: r.get(4)?,
                        size: r.get(5)?,
                        atime: r.get(6)?,
                        mtime: r.get(7)?,
                        ctime: r.get(8)?,
                        rdev: r.get(9)?,
                        atime_nsec: r.get(10)?,
                        mtime_nsec: r.get(11)?,
                        ctime_nsec: r.get(12)?,
                    })
                },
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("inode {ino}")))
    }

    // ---- directories -----------------------------------------------------

    /// Create a directory and any missing ancestors (SPEC: parent directories
    /// are created implicitly as needed). Returns the directory inode.
    pub fn mkdir_p(&mut self, path: &str) -> Result<i64> {
        self.ensure_writable()?;
        let npath = normalize(path);
        let (secs, nsec) = now_parts();
        let mut ino = ROOT_INO;
        for comp in components(&npath) {
            match self.lookup_child(ino, comp)? {
                Some(child) => {
                    let meta = self.stat_ino(child)?;
                    if !meta.is_dir() {
                        return Err(Error::NotADirectory(npath.clone()));
                    }
                    ino = child;
                }
                None => {
                    let tx = self.conn.transaction()?;
                    let child: i64 = tx.query_row(
                        "INSERT INTO fs_inode (mode, uid, gid, size, atime, mtime, ctime,
                                               atime_nsec, mtime_nsec, ctime_nsec)
                         VALUES (?1, 0, 0, 0, ?2, ?2, ?2, ?3, ?3, ?3)
                         RETURNING ino",
                        params![S_IFDIR | 0o755, secs, nsec],
                        |r| r.get(0),
                    )?;
                    tx.execute(
                        "INSERT INTO fs_dentry (name, parent_ino, ino) VALUES (?1, ?2, ?3)",
                        params![comp, ino, child],
                    )?;
                    tx.execute(
                        "UPDATE fs_inode SET nlink = nlink + 1 WHERE ino = ?1",
                        [child],
                    )?;
                    tx.commit()?;
                    ino = child;
                }
            }
        }
        Ok(ino)
    }

    /// List directory entry names, sorted ascending (SPEC listing).
    pub fn readdir(&self, path: &str) -> Result<Vec<String>> {
        let npath = normalize(path);
        let ino = self
            .resolve(&npath)?
            .ok_or_else(|| Error::NotFound(npath.clone()))?;
        if !self.stat_ino(ino)?.is_dir() {
            return Err(Error::NotADirectory(npath));
        }
        let mut stmt = self
            .conn
            .prepare("SELECT name FROM fs_dentry WHERE parent_ino = ?1 ORDER BY name ASC")?;
        let names = stmt
            .query_map([ino], |r| r.get::<_, String>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(names)
    }

    /// Remove an empty directory.
    pub fn remove_dir(&mut self, path: &str) -> Result<()> {
        self.ensure_writable()?;
        let npath = normalize(path);
        if npath == "/" {
            return Err(Error::InvalidArgument("cannot remove root".into()));
        }
        let ino = self
            .resolve(&npath)?
            .ok_or_else(|| Error::NotFound(npath.clone()))?;
        if !self.stat_ino(ino)?.is_dir() {
            return Err(Error::NotADirectory(npath));
        }
        let child_count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM fs_dentry WHERE parent_ino = ?1",
            [ino],
            |r| r.get(0),
        )?;
        if child_count > 0 {
            return Err(Error::DirectoryNotEmpty(npath));
        }
        let parent_ino = self
            .resolve(&parent(&npath))?
            .ok_or_else(|| Error::NotFound(parent(&npath)))?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM fs_dentry WHERE parent_ino = ?1 AND name = ?2",
            params![parent_ino, basename(&npath)],
        )?;
        tx.execute("DELETE FROM fs_inode WHERE ino = ?1", [ino])?;
        tx.commit()?;
        Ok(())
    }

    // ---- files -----------------------------------------------------------

    /// Create or overwrite a regular file with `data`, chunked at the
    /// configured chunk size (SPEC file creation). Missing parent directories
    /// are created implicitly. Returns the file inode.
    pub fn write_file(&mut self, path: &str, data: &[u8]) -> Result<i64> {
        self.ensure_writable()?;
        let npath = normalize(path);
        if npath == "/" {
            return Err(Error::IsADirectory(npath));
        }
        let parent_ino = self.mkdir_p(&parent(&npath))?;
        let name = basename(&npath).to_string();
        let (secs, nsec) = now_parts();
        let chunk_size = self.chunk_size;

        let tx = self.conn.transaction()?;
        let existing: Option<i64> = tx
            .query_row(
                "SELECT ino FROM fs_dentry WHERE parent_ino = ?1 AND name = ?2",
                params![parent_ino, name],
                |r| r.get(0),
            )
            .optional()?;
        let ino = match existing {
            Some(ino) => {
                let mode: u32 =
                    tx.query_row("SELECT mode FROM fs_inode WHERE ino = ?1", [ino], |r| {
                        r.get(0)
                    })?;
                if mode & S_IFMT != S_IFREG {
                    return Err(Error::NotARegularFile(npath));
                }
                tx.execute("DELETE FROM fs_data WHERE ino = ?1", [ino])?;
                ino
            }
            None => {
                let ino: i64 = tx.query_row(
                    "INSERT INTO fs_inode (mode, uid, gid, size, atime, mtime, ctime,
                                           atime_nsec, mtime_nsec, ctime_nsec)
                     VALUES (?1, 0, 0, 0, ?2, ?2, ?2, ?3, ?3, ?3)
                     RETURNING ino",
                    params![S_IFREG | 0o644, secs, nsec],
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
                ino
            }
        };
        for (idx, chunk) in data.chunks(chunk_size).enumerate() {
            tx.execute(
                "INSERT INTO fs_data (ino, chunk_index, data) VALUES (?1, ?2, ?3)",
                params![ino, idx as i64, chunk],
            )?;
        }
        tx.execute(
            "UPDATE fs_inode SET size = ?1, mtime = ?2, mtime_nsec = ?3,
                                 ctime = ?2, ctime_nsec = ?3
             WHERE ino = ?4",
            params![data.len() as i64, secs, nsec, ino],
        )?;
        tx.commit()?;
        Ok(ino)
    }

    /// Read an entire regular file (SPEC read: concatenate chunks in order,
    /// then update `atime` — skipped on read-only connections).
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let npath = normalize(path);
        let ino = self
            .resolve(&npath)?
            .ok_or_else(|| Error::NotFound(npath.clone()))?;
        let meta = self.stat_ino(ino)?;
        if meta.is_dir() {
            return Err(Error::IsADirectory(npath));
        }
        if !meta.is_file() {
            return Err(Error::NotARegularFile(npath));
        }
        let mut stmt = self
            .conn
            .prepare("SELECT data FROM fs_data WHERE ino = ?1 ORDER BY chunk_index ASC")?;
        let mut out = Vec::with_capacity(meta.size as usize);
        for chunk in stmt.query_map([ino], |r| r.get::<_, Vec<u8>>(0))? {
            out.extend_from_slice(&chunk?);
        }
        if !self.read_only {
            let (secs, nsec) = now_parts();
            self.conn.execute(
                "UPDATE fs_inode SET atime = ?1, atime_nsec = ?2 WHERE ino = ?3",
                params![secs, nsec, ino],
            )?;
        }
        Ok(out)
    }

    /// Read `length` bytes starting at byte `offset` (SPEC offset read).
    /// Returns fewer bytes when the file ends first.
    pub fn read_at(&self, path: &str, offset: u64, length: usize) -> Result<Vec<u8>> {
        let npath = normalize(path);
        let ino = self
            .resolve(&npath)?
            .ok_or_else(|| Error::NotFound(npath.clone()))?;
        let meta = self.stat_ino(ino)?;
        if !meta.is_file() {
            return Err(Error::NotARegularFile(npath));
        }
        if length == 0 || offset >= meta.size as u64 {
            return Ok(Vec::new());
        }
        let cs = self.chunk_size as u64;
        let start_chunk = (offset / cs) as i64;
        let end_chunk = ((offset + length as u64 - 1) / cs) as i64;
        let mut stmt = self.conn.prepare(
            "SELECT chunk_index, data FROM fs_data
             WHERE ino = ?1 AND chunk_index >= ?2 AND chunk_index <= ?3
             ORDER BY chunk_index ASC",
        )?;
        let mut raw = Vec::new();
        for row in stmt.query_map(params![ino, start_chunk, end_chunk], |r| {
            r.get::<_, Vec<u8>>(1)
        })? {
            raw.extend_from_slice(&row?);
        }
        let skip = (offset % cs) as usize;
        if skip >= raw.len() {
            return Ok(Vec::new());
        }
        let end = (skip + length).min(raw.len());
        Ok(raw[skip..end].to_vec())
    }

    /// Delete a regular file or symlink (SPEC deletion: remove dentry,
    /// decrement `nlink`, drop inode + data on last link).
    pub fn remove_file(&mut self, path: &str) -> Result<()> {
        self.ensure_writable()?;
        let npath = normalize(path);
        let ino = self
            .resolve(&npath)?
            .ok_or_else(|| Error::NotFound(npath.clone()))?;
        if self.stat_ino(ino)?.is_dir() {
            return Err(Error::IsADirectory(npath));
        }
        let parent_ino = self
            .resolve(&parent(&npath))?
            .ok_or_else(|| Error::NotFound(parent(&npath)))?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM fs_dentry WHERE parent_ino = ?1 AND name = ?2",
            params![parent_ino, basename(&npath)],
        )?;
        tx.execute(
            "UPDATE fs_inode SET nlink = nlink - 1 WHERE ino = ?1",
            [ino],
        )?;
        let nlink: i64 = tx.query_row("SELECT nlink FROM fs_inode WHERE ino = ?1", [ino], |r| {
            r.get(0)
        })?;
        if nlink <= 0 {
            tx.execute("DELETE FROM fs_inode WHERE ino = ?1", [ino])?;
            tx.execute("DELETE FROM fs_data WHERE ino = ?1", [ino])?;
            tx.execute("DELETE FROM fs_symlink WHERE ino = ?1", [ino])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Rename/move a file or directory. An existing destination file is
    /// replaced; an existing destination directory must be empty.
    pub fn rename(&mut self, src: &str, dst: &str) -> Result<()> {
        self.ensure_writable()?;
        let nsrc = normalize(src);
        let ndst = normalize(dst);
        if nsrc == "/" || ndst == "/" {
            return Err(Error::InvalidArgument("cannot rename root".into()));
        }
        if nsrc == ndst {
            return Ok(());
        }
        if ndst.starts_with(&format!("{nsrc}/")) {
            return Err(Error::InvalidArgument(format!(
                "cannot move {nsrc} into itself"
            )));
        }
        let src_ino = self
            .resolve(&nsrc)?
            .ok_or_else(|| Error::NotFound(nsrc.clone()))?;
        if let Some(dst_ino) = self.resolve(&ndst)? {
            if dst_ino == src_ino {
                return Ok(());
            }
            if self.stat_ino(dst_ino)?.is_dir() {
                self.remove_dir(&ndst)?;
            } else {
                self.remove_file(&ndst)?;
            }
        }
        let src_parent_ino = self
            .resolve(&parent(&nsrc))?
            .ok_or_else(|| Error::NotFound(parent(&nsrc)))?;
        let dst_parent_ino = self.mkdir_p(&parent(&ndst))?;
        let (secs, nsec) = now_parts();
        let tx = self.conn.transaction()?;
        tx.execute(
            "UPDATE fs_dentry SET name = ?1, parent_ino = ?2
             WHERE parent_ino = ?3 AND name = ?4",
            params![
                basename(&ndst),
                dst_parent_ino,
                src_parent_ino,
                basename(&nsrc)
            ],
        )?;
        tx.execute(
            "UPDATE fs_inode SET ctime = ?1, ctime_nsec = ?2 WHERE ino = ?3",
            params![secs, nsec, src_ino],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Create a hard link to a regular file (SPEC hard link).
    pub fn hardlink(&mut self, src: &str, dst: &str) -> Result<()> {
        self.ensure_writable()?;
        let nsrc = normalize(src);
        let ndst = normalize(dst);
        let src_ino = self
            .resolve(&nsrc)?
            .ok_or_else(|| Error::NotFound(nsrc.clone()))?;
        if !self.stat_ino(src_ino)?.is_file() {
            return Err(Error::NotARegularFile(nsrc));
        }
        if self.resolve(&ndst)?.is_some() {
            return Err(Error::AlreadyExists(ndst));
        }
        let dst_parent_ino = self.mkdir_p(&parent(&ndst))?;
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO fs_dentry (name, parent_ino, ino) VALUES (?1, ?2, ?3)",
            params![basename(&ndst), dst_parent_ino, src_ino],
        )?;
        tx.execute(
            "UPDATE fs_inode SET nlink = nlink + 1 WHERE ino = ?1",
            [src_ino],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Create a symbolic link at `linkpath` pointing at `target`.
    pub fn symlink(&mut self, target: &str, linkpath: &str) -> Result<i64> {
        self.ensure_writable()?;
        let npath = normalize(linkpath);
        if self.resolve(&npath)?.is_some() {
            return Err(Error::AlreadyExists(npath));
        }
        let parent_ino = self.mkdir_p(&parent(&npath))?;
        let (secs, nsec) = now_parts();
        let tx = self.conn.transaction()?;
        let ino: i64 = tx.query_row(
            "INSERT INTO fs_inode (mode, uid, gid, size, atime, mtime, ctime,
                                   atime_nsec, mtime_nsec, ctime_nsec)
             VALUES (?1, 0, 0, ?2, ?3, ?3, ?3, ?4, ?4, ?4)
             RETURNING ino",
            params![S_IFLNK | 0o777, target.len() as i64, secs, nsec],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO fs_dentry (name, parent_ino, ino) VALUES (?1, ?2, ?3)",
            params![basename(&npath), parent_ino, ino],
        )?;
        tx.execute(
            "UPDATE fs_inode SET nlink = nlink + 1 WHERE ino = ?1",
            [ino],
        )?;
        tx.execute(
            "INSERT INTO fs_symlink (ino, target) VALUES (?1, ?2)",
            params![ino, target],
        )?;
        tx.commit()?;
        Ok(ino)
    }

    /// Read a symlink's target.
    pub fn read_link(&self, path: &str) -> Result<String> {
        let npath = normalize(path);
        let ino = self
            .resolve(&npath)?
            .ok_or_else(|| Error::NotFound(npath.clone()))?;
        if !self.stat_ino(ino)?.is_symlink() {
            return Err(Error::NotASymlink(npath));
        }
        Ok(self
            .conn
            .query_row("SELECT target FROM fs_symlink WHERE ino = ?1", [ino], |r| {
                r.get(0)
            })?)
    }

    // ---- consistency -----------------------------------------------------

    /// Verify the SPEC filesystem consistency rules plus chunk-shape
    /// invariants. Returns a list of human-readable violations (empty when
    /// consistent).
    pub fn check_consistency(&self) -> Result<Vec<String>> {
        let mut violations = Vec::new();
        let c = &self.conn;

        // Rule 1: root inode must exist and be a directory.
        let root_mode: Option<u32> = c
            .query_row("SELECT mode FROM fs_inode WHERE ino = 1", [], |r| r.get(0))
            .optional()?;
        match root_mode {
            None => violations.push("rule 1: root inode (ino=1) missing".into()),
            Some(m) if m & S_IFMT != S_IFDIR => {
                violations.push("rule 1: root inode is not a directory".into())
            }
            _ => {}
        }

        // Rules 2 + 3: every dentry references a valid inode and parent inode.
        let orphan_inos: i64 = c.query_row(
            "SELECT COUNT(*) FROM fs_dentry d
             WHERE NOT EXISTS (SELECT 1 FROM fs_inode i WHERE i.ino = d.ino)",
            [],
            |r| r.get(0),
        )?;
        if orphan_inos > 0 {
            violations.push(format!(
                "rule 2: {orphan_inos} dentries reference missing inodes"
            ));
        }
        let orphan_parents: i64 = c.query_row(
            "SELECT COUNT(*) FROM fs_dentry d
             WHERE NOT EXISTS (SELECT 1 FROM fs_inode i WHERE i.ino = d.parent_ino)",
            [],
            |r| r.get(0),
        )?;
        if orphan_parents > 0 {
            violations.push(format!(
                "rule 3: {orphan_parents} dentries reference missing parent inodes"
            ));
        }

        // Rule 4: no duplicate names within a directory.
        let dupes: i64 = c.query_row(
            "SELECT COUNT(*) FROM (
               SELECT parent_ino, name FROM fs_dentry
               GROUP BY parent_ino, name HAVING COUNT(*) > 1)",
            [],
            |r| r.get(0),
        )?;
        if dupes > 0 {
            violations.push(format!("rule 4: {dupes} duplicate (parent, name) pairs"));
        }

        // Rule 5: every inode used as a dentry parent is a directory.
        let bad_parents: i64 = c.query_row(
            "SELECT COUNT(*) FROM fs_dentry d
             JOIN fs_inode i ON i.ino = d.parent_ino
             WHERE (i.mode & ?1) != ?2",
            params![S_IFMT, S_IFDIR],
            |r| r.get(0),
        )?;
        if bad_parents > 0 {
            violations.push(format!(
                "rule 5: {bad_parents} dentries under non-directories"
            ));
        }

        // Rule 6: inodes holding data chunks are regular files, and
        // directories hold no chunks.
        let bad_data_owners: i64 = c.query_row(
            "SELECT COUNT(DISTINCT d.ino) FROM fs_data d
             JOIN fs_inode i ON i.ino = d.ino
             WHERE (i.mode & ?1) != ?2",
            params![S_IFMT, S_IFREG],
            |r| r.get(0),
        )?;
        if bad_data_owners > 0 {
            violations.push(format!(
                "rule 6: {bad_data_owners} non-regular-file inodes own data chunks"
            ));
        }

        // Rule 7: file size matches the total size of its data chunks.
        let size_mismatches: i64 = c.query_row(
            "SELECT COUNT(*) FROM fs_inode i
             WHERE (i.mode & ?1) = ?2
               AND i.size != COALESCE(
                 (SELECT SUM(LENGTH(d.data)) FROM fs_data d WHERE d.ino = i.ino), 0)",
            params![S_IFMT, S_IFREG],
            |r| r.get(0),
        )?;
        if size_mismatches > 0 {
            violations.push(format!(
                "rule 7: {size_mismatches} inodes with size != sum of chunk lengths"
            ));
        }

        // Rule 8: every inode except root has at least one dentry.
        let unreferenced: i64 = c.query_row(
            "SELECT COUNT(*) FROM fs_inode i
             WHERE i.ino != 1
               AND NOT EXISTS (SELECT 1 FROM fs_dentry d WHERE d.ino = i.ino)",
            [],
            |r| r.get(0),
        )?;
        if unreferenced > 0 {
            violations.push(format!("rule 8: {unreferenced} inodes with no dentry"));
        }

        // Chunk shape: all chunks except the last are exactly chunk_size.
        let bad_chunks: i64 = c.query_row(
            "SELECT COUNT(*) FROM fs_data d
             WHERE LENGTH(d.data) != ?1
               AND d.chunk_index != (SELECT MAX(chunk_index) FROM fs_data WHERE ino = d.ino)",
            [self.chunk_size as i64],
            |r| r.get(0),
        )?;
        if bad_chunks > 0 {
            violations.push(format!(
                "chunking: {bad_chunks} non-final chunks not exactly {} bytes",
                self.chunk_size
            ));
        }

        Ok(violations)
    }
}
