//! NFSv3 export of an [`AgentFs`] database.
//!
//! This is the kext-free macOS mount path: serve NFSv3 on loopback and let
//! the kernel's native `mount_nfs` client attach it. Linux can use the same
//! server, though FUSE via `fuser` is the better fit there.
//!
//! Feature-gated behind `mount` so the default build stays free of tokio and
//! the RPC stack — most consumers of this crate want the storage engine, not
//! a server.
//!
//! # Concurrency
//!
//! The filesystem sits behind a [`Mutex`], so RPCs serialize. `rusqlite` holds
//! a single connection and every operation is a short transaction, so this is
//! correct but not concurrent. It bounds what the benchmark can say about
//! parallel workloads; see `specs/coven-agent-fs/MOUNT-SPIKE.md`.
//!
//! # Ownership mapping
//!
//! SPEC v0.4 defaults `fs_inode.uid`/`gid` to 0, so an unmapped export
//! presents a root-owned tree and the client's own permission check rejects
//! every write before a single RPC leaves the machine. Inodes stored as
//! unowned (uid/gid 0) are therefore presented as owned by the serving
//! process's user. Ownership explicitly set through `setattr` is preserved.
//! Nothing is written back: this is presentation only, so the database stays
//! byte-identical to what upstream `agentfs` tooling would produce.
//!
//! # Trust
//!
//! Loopback NFS has no authentication. Anything that can reach the port can
//! read and write the export, so a bare port on a shared host is a hole. Bind
//! only 127.0.0.1, prefer an ephemeral port, and treat this as opt-in until
//! the access-control question in `DESIGN.md` §7 is settled.

use std::sync::Mutex;

use async_trait::async_trait;
use nfsserve::nfs::{
    fattr3, fileid3, filename3, ftype3, nfspath3, nfsstat3, nfstime3, sattr3, set_atime, set_gid3,
    set_mode3, set_mtime, set_size3, set_uid3, specdata3,
};
use nfsserve::vfs::{DirEntry as NfsDirEntry, NFSFileSystem, ReadDirResult, VFSCapabilities};

use crate::fs::{Metadata, ROOT_INO};
use crate::{AgentFs, Error};

/// An [`AgentFs`] exported over NFSv3.
pub struct AfsNfs {
    fs: Mutex<AgentFs>,
    read_only: bool,
    uid: u32,
    gid: u32,
}

impl AfsNfs {
    /// Wrap a filesystem for export. A read-only [`AgentFs`] is exported
    /// read-only.
    pub fn new(fs: AgentFs) -> Self {
        // SAFETY: getuid/getgid are always-succeeding POSIX calls with no
        // preconditions and no memory effects.
        let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
        Self {
            read_only: fs.is_read_only(),
            fs: Mutex::new(fs),
            uid,
            gid,
        }
    }

    /// Present unowned inodes as belonging to the serving user; see the
    /// ownership note at the top of this module.
    fn attributes(&self, meta: &Metadata) -> fattr3 {
        let mut attr = attributes(meta);
        if meta.uid == 0 {
            attr.uid = self.uid;
        }
        if meta.gid == 0 {
            attr.gid = self.gid;
        }
        attr
    }

    fn with<T>(&self, op: impl FnOnce(&mut AgentFs) -> crate::Result<T>) -> Result<T, nfsstat3> {
        let mut guard = self.fs.lock().map_err(|_| nfsstat3::NFS3ERR_SERVERFAULT)?;
        op(&mut guard).map_err(status)
    }
}

/// Map a storage error onto the closest NFSv3 status.
fn status(error: Error) -> nfsstat3 {
    match error {
        Error::NotFound(_) => nfsstat3::NFS3ERR_NOENT,
        Error::AlreadyExists(_) => nfsstat3::NFS3ERR_EXIST,
        Error::NotADirectory(_) => nfsstat3::NFS3ERR_NOTDIR,
        Error::IsADirectory(_) => nfsstat3::NFS3ERR_ISDIR,
        Error::DirectoryNotEmpty(_) => nfsstat3::NFS3ERR_NOTEMPTY,
        Error::NotARegularFile(_) | Error::NotASymlink(_) | Error::InvalidArgument(_) => {
            nfsstat3::NFS3ERR_INVAL
        }
        Error::ReadOnly => nfsstat3::NFS3ERR_ROFS,
        Error::Sqlite(_) | Error::Json(_) => nfsstat3::NFS3ERR_IO,
    }
}

fn time(seconds: i64, nanoseconds: i64) -> nfstime3 {
    nfstime3 {
        seconds: seconds.clamp(0, i64::from(u32::MAX)) as u32,
        nseconds: nanoseconds.clamp(0, i64::from(u32::MAX)) as u32,
    }
}

fn attributes(meta: &Metadata) -> fattr3 {
    let ftype = if meta.is_dir() {
        ftype3::NF3DIR
    } else if meta.is_symlink() {
        ftype3::NF3LNK
    } else {
        ftype3::NF3REG
    };
    fattr3 {
        ftype,
        mode: meta.mode & 0o7777,
        nlink: meta.nlink.max(0) as u32,
        uid: meta.uid.max(0) as u32,
        gid: meta.gid.max(0) as u32,
        size: meta.size.max(0) as u64,
        used: meta.size.max(0) as u64,
        rdev: specdata3 {
            specdata1: 0,
            specdata2: 0,
        },
        fsid: 0,
        fileid: meta.ino as u64,
        atime: time(meta.atime, meta.atime_nsec),
        mtime: time(meta.mtime, meta.mtime_nsec),
        ctime: time(meta.ctime, meta.ctime_nsec),
    }
}

fn name_of(name: &filename3) -> Result<String, nfsstat3> {
    String::from_utf8(name.0.clone()).map_err(|_| nfsstat3::NFS3ERR_INVAL)
}

/// Apply the mutable parts of a `sattr3` to an inode. Size is applied first so
/// a truncate-on-create carries the requested length.
fn apply_sattr(fs: &mut AgentFs, ino: i64, attr: &sattr3) -> crate::Result<Metadata> {
    if let set_size3::size(size) = attr.size {
        fs.truncate_ino(ino, size)?;
    }
    let mode = match attr.mode {
        set_mode3::mode(bits) => Some(bits),
        set_mode3::Void => None,
    };
    let uid = match attr.uid {
        set_uid3::uid(id) => Some(i64::from(id)),
        set_uid3::Void => None,
    };
    let gid = match attr.gid {
        set_gid3::gid(id) => Some(i64::from(id)),
        set_gid3::Void => None,
    };
    let atime = match attr.atime {
        set_atime::SET_TO_CLIENT_TIME(stamp) => {
            Some((i64::from(stamp.seconds), i64::from(stamp.nseconds)))
        }
        set_atime::SET_TO_SERVER_TIME => Some(crate::now_parts()),
        set_atime::DONT_CHANGE => None,
    };
    let mtime = match attr.mtime {
        set_mtime::SET_TO_CLIENT_TIME(stamp) => {
            Some((i64::from(stamp.seconds), i64::from(stamp.nseconds)))
        }
        set_mtime::SET_TO_SERVER_TIME => Some(crate::now_parts()),
        set_mtime::DONT_CHANGE => None,
    };
    fs.setattr_ino(ino, mode, uid, gid, atime, mtime)
}

#[async_trait]
impl NFSFileSystem for AfsNfs {
    fn capabilities(&self) -> VFSCapabilities {
        if self.read_only {
            VFSCapabilities::ReadOnly
        } else {
            VFSCapabilities::ReadWrite
        }
    }

    fn root_dir(&self) -> fileid3 {
        ROOT_INO as u64
    }

    async fn lookup(&self, dirid: fileid3, filename: &filename3) -> Result<fileid3, nfsstat3> {
        let name = name_of(filename)?;
        let dir = dirid as i64;
        self.with(|fs| match name.as_str() {
            "." => Ok(dir),
            ".." => Ok(fs.parent_of(dir)?.unwrap_or(ROOT_INO)),
            _ => fs
                .lookup_ino(dir, &name)?
                .ok_or_else(|| Error::NotFound(name.clone())),
        })
        .map(|ino| ino as u64)
    }

    async fn getattr(&self, id: fileid3) -> Result<fattr3, nfsstat3> {
        self.with(|fs| fs.stat_ino(id as i64))
            .map(|m| self.attributes(&m))
    }

    async fn setattr(&self, id: fileid3, setattr: sattr3) -> Result<fattr3, nfsstat3> {
        self.with(|fs| apply_sattr(fs, id as i64, &setattr))
            .map(|m| self.attributes(&m))
    }

    async fn read(
        &self,
        id: fileid3,
        offset: u64,
        count: u32,
    ) -> Result<(Vec<u8>, bool), nfsstat3> {
        self.with(|fs| fs.read_ino_at(id as i64, offset, count as usize))
    }

    async fn write(&self, id: fileid3, offset: u64, data: &[u8]) -> Result<fattr3, nfsstat3> {
        self.with(|fs| fs.write_ino_at(id as i64, offset, data))
            .map(|m| self.attributes(&m))
    }

    async fn create(
        &self,
        dirid: fileid3,
        filename: &filename3,
        attr: sattr3,
    ) -> Result<(fileid3, fattr3), nfsstat3> {
        let name = name_of(filename)?;
        let dir = dirid as i64;
        self.with(|fs| {
            // CREATE is not exclusive: an existing file is opened and
            // truncated per the requested attributes, not rejected.
            let ino = match fs.lookup_ino(dir, &name)? {
                Some(existing) => existing,
                None => fs.create_child(dir, &name, 0o644)?.0,
            };
            let meta = apply_sattr(fs, ino, &attr)?;
            Ok((ino as u64, meta))
        })
        .map(|(id, meta)| (id, self.attributes(&meta)))
    }

    async fn create_exclusive(
        &self,
        dirid: fileid3,
        filename: &filename3,
    ) -> Result<fileid3, nfsstat3> {
        let name = name_of(filename)?;
        self.with(|fs| {
            fs.create_child(dirid as i64, &name, 0o644)
                .map(|(ino, _)| ino)
        })
        .map(|ino| ino as u64)
    }

    async fn mkdir(
        &self,
        dirid: fileid3,
        dirname: &filename3,
    ) -> Result<(fileid3, fattr3), nfsstat3> {
        let name = name_of(dirname)?;
        self.with(|fs| {
            let (ino, meta) = fs.mkdir_ino(dirid as i64, &name, 0o755)?;
            Ok((ino as u64, meta))
        })
        .map(|(id, meta)| (id, self.attributes(&meta)))
    }

    async fn remove(&self, dirid: fileid3, filename: &filename3) -> Result<(), nfsstat3> {
        let name = name_of(filename)?;
        self.with(|fs| fs.remove_ino(dirid as i64, &name))
    }

    async fn rename(
        &self,
        from_dirid: fileid3,
        from_filename: &filename3,
        to_dirid: fileid3,
        to_filename: &filename3,
    ) -> Result<(), nfsstat3> {
        let from = name_of(from_filename)?;
        let to = name_of(to_filename)?;
        self.with(|fs| fs.rename_ino(from_dirid as i64, &from, to_dirid as i64, &to))
    }

    async fn readdir(
        &self,
        dirid: fileid3,
        start_after: fileid3,
        max_entries: usize,
    ) -> Result<ReadDirResult, nfsstat3> {
        let dir = dirid as i64;
        let (entries, exhausted) = self.with(|fs| {
            let page = fs.readdir_ino(dir, start_after as i64, max_entries)?;
            let exhausted = page.len() < max_entries;
            Ok((page, exhausted))
        })?;
        Ok(ReadDirResult {
            entries: entries
                .into_iter()
                .map(|entry| NfsDirEntry {
                    fileid: entry.meta.ino as u64,
                    name: entry.name.into_bytes().into(),
                    attr: self.attributes(&entry.meta),
                })
                .collect(),
            end: exhausted,
        })
    }

    async fn symlink(
        &self,
        dirid: fileid3,
        linkname: &filename3,
        symlink: &nfspath3,
        attr: &sattr3,
    ) -> Result<(fileid3, fattr3), nfsstat3> {
        let name = name_of(linkname)?;
        let target = String::from_utf8(symlink.0.clone()).map_err(|_| nfsstat3::NFS3ERR_INVAL)?;
        let attr = *attr;
        self.with(|fs| {
            let (ino, _) = fs.symlink_ino(dirid as i64, &name, &target)?;
            // A symlink has no size to set; only ownership and times apply.
            let meta = apply_sattr(
                fs,
                ino,
                &sattr3 {
                    size: set_size3::Void,
                    ..attr
                },
            )?;
            Ok((ino as u64, meta))
        })
        .map(|(id, meta)| (id, self.attributes(&meta)))
    }

    async fn readlink(&self, id: fileid3) -> Result<nfspath3, nfsstat3> {
        self.with(|fs| fs.readlink_ino(id as i64))
            .map(|target| target.into_bytes().into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn export() -> AfsNfs {
        AfsNfs::new(AgentFs::in_memory().unwrap())
    }

    fn name(text: &str) -> filename3 {
        text.as_bytes().to_vec().into()
    }

    #[tokio::test]
    async fn create_write_read_round_trips_through_the_vfs() {
        let export = export();
        let (id, _) = export
            .create(1, &name("hello.txt"), sattr3::default())
            .await
            .unwrap();
        export.write(id, 0, b"hello ").await.unwrap();
        let attr = export.write(id, 6, b"world").await.unwrap();
        assert_eq!(attr.size, 11);
        let (data, eof) = export.read(id, 0, 64).await.unwrap();
        assert_eq!(data, b"hello world");
        assert!(eof);
    }

    #[tokio::test]
    async fn lookup_resolves_dot_and_dotdot() {
        let export = export();
        let (dir, _) = export.mkdir(1, &name("sub")).await.unwrap();
        assert_eq!(export.lookup(dir, &name(".")).await.unwrap(), dir);
        assert_eq!(export.lookup(dir, &name("..")).await.unwrap(), 1);
        assert_eq!(export.lookup(1, &name("..")).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn missing_entries_report_noent_not_a_server_fault() {
        let export = export();
        assert!(matches!(
            export.lookup(1, &name("nope")).await.unwrap_err(),
            nfsstat3::NFS3ERR_NOENT
        ));
    }

    #[tokio::test]
    async fn readdir_pages_and_terminates() {
        let export = export();
        for index in 0..5 {
            export
                .create(1, &name(&format!("f{index}")), sattr3::default())
                .await
                .unwrap();
        }
        let first = export.readdir(1, 0, 2).await.unwrap();
        assert_eq!(first.entries.len(), 2);
        assert!(!first.end);
        let cursor = first.entries.last().unwrap().fileid;
        let rest = export.readdir(1, cursor, 10).await.unwrap();
        assert_eq!(rest.entries.len(), 3);
        assert!(rest.end);
    }

    #[tokio::test]
    async fn setattr_truncates_and_sets_mode() {
        let export = export();
        let (id, _) = export
            .create(1, &name("t.txt"), sattr3::default())
            .await
            .unwrap();
        export.write(id, 0, b"0123456789").await.unwrap();
        let attr = export
            .setattr(
                id,
                sattr3 {
                    size: set_size3::size(4),
                    mode: set_mode3::mode(0o600),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(attr.size, 4);
        assert_eq!(attr.mode, 0o600);
        let (data, _) = export.read(id, 0, 64).await.unwrap();
        assert_eq!(data, b"0123");
    }

    #[tokio::test]
    async fn read_only_export_refuses_writes_with_rofs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ro.db");
        {
            let mut fs = AgentFs::create(&path).unwrap();
            fs.write_file("/a.txt", b"data").unwrap();
        }
        let export = AfsNfs::new(AgentFs::open_read_only(&path).unwrap());
        assert!(matches!(export.capabilities(), VFSCapabilities::ReadOnly));
        let id = export.lookup(1, &name("a.txt")).await.unwrap();
        assert!(matches!(
            export.write(id, 0, b"x").await.unwrap_err(),
            nfsstat3::NFS3ERR_ROFS
        ));
    }
}
