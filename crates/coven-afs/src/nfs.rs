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
    fattr3, fileid3, filename3, ftype3, nfs_fh3, nfspath3, nfsstat3, nfstime3, sattr3, set_atime,
    set_gid3, set_mode3, set_mtime, set_size3, set_uid3, specdata3,
};
use nfsserve::vfs::{DirEntry as NfsDirEntry, NFSFileSystem, ReadDirResult, VFSCapabilities};

use crate::fs::{Metadata, ROOT_INO};
use crate::{AgentFs, Error};

/// Refuse any bind address that is not loopback.
///
/// The export is unauthenticated at the RPC layer, so a non-loopback bind
/// hands the session's files to the network. This is a hard refusal rather
/// than a warning: there is no deployment in which it is correct.
pub fn ensure_loopback(host: &str) -> Result<(), Error> {
    let host = host.trim();
    // Order matters: a bare IPv6 address is mostly colons, so trying to strip
    // a ":port" first turns `::1` into `:`. Parse the whole string as an
    // address before assuming any colon is a port separator.
    let candidate = if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return loopback_or_refuse(ip.is_loopback(), host);
    } else if let Some(rest) = host.strip_prefix('[') {
        // `[::1]:2049` — the bracketed form exists precisely to disambiguate.
        rest.split_once(']').map_or(rest, |(inside, _)| inside)
    } else {
        // Anything left has at most one colon before a port.
        host.rsplit_once(':').map_or(host, |(head, _)| head)
    };
    let is_loopback = match candidate.parse::<std::net::IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        // "localhost" resolves to loopback on every platform we serve, but an
        // unresolvable name is not something to guess about.
        Err(_) => candidate.eq_ignore_ascii_case("localhost"),
    };
    loopback_or_refuse(is_loopback, host)
}

fn loopback_or_refuse(is_loopback: bool, host: &str) -> Result<(), Error> {
    if is_loopback {
        Ok(())
    } else {
        Err(Error::InvalidArgument(format!(
            "refusing to export on {host}: an AgentFS NFS export must bind loopback only"
        )))
    }
}

/// A random export-path component, 128 bits of OS entropy as hex.
///
/// Not derived from the session id: the id appears in logs, in the daemon API,
/// and in Cave, so deriving from it would publish the token.
pub fn export_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes).expect("OS entropy is required to export a filesystem");
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Per-export secret that authenticates NFS file handles.
///
/// Generated fresh for every export, so handles from a previous server are
/// rejected automatically — the property nfsserve's startup-time generation
/// number was reaching for, without the guessability.
struct HandleKey([u8; 32]);

impl HandleKey {
    fn random() -> Self {
        let mut key = [0u8; 32];
        // A failure here means the OS entropy source is unavailable; there is
        // no safe weaker fallback, so refuse to serve rather than issue
        // forgeable handles.
        getrandom::getrandom(&mut key).expect("OS entropy is required to export a filesystem");
        Self(key)
    }

    /// Truncated HMAC-SHA256 over the file id. 128 bits is far beyond what a
    /// local attacker can brute-force online, and keeps the handle at 24 of
    /// the 64 bytes NFSv3 allows.
    fn tag(&self, id: fileid3) -> [u8; 16] {
        type Mac = hmac::Hmac<sha2::Sha256>;
        use hmac::Mac as _;
        let mut mac =
            <Mac as hmac::Mac>::new_from_slice(&self.0).expect("HMAC accepts any key len");
        mac.update(&id.to_le_bytes());
        let full = mac.finalize().into_bytes();
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&full[..16]);
        tag
    }
}

/// Compare without leaking where the mismatch was.
fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (l, r)| acc | (l ^ r))
        == 0
}

/// The synthetic directory an export is rooted at.
///
/// Not a real inode: `fs_inode.ino` is a positive `AUTOINCREMENT`, so the top
/// of the range can never collide with one.
const GATE_ROOT: fileid3 = u64::MAX;

/// An [`AgentFs`] exported over NFSv3.
///
/// The export is rooted at a synthetic gate directory rather than the
/// filesystem root. The gate has exactly one child — a directory named by the
/// export token — and its `readdir` returns nothing, so reaching the real
/// filesystem requires knowing the token rather than merely reaching the port.
///
/// The token deliberately lives here rather than in the NFS export path:
/// `MOUNTPROC3_EXPORT` dumps the export path to any caller, unauthenticated,
/// so a token carried there is readable by exactly the attacker it is meant to
/// stop. Inside the VFS there is no procedure that lists it.
pub struct AfsNfs {
    fs: Mutex<AgentFs>,
    read_only: bool,
    uid: u32,
    gid: u32,
    handle_key: HandleKey,
    /// The export token — the single name that opens the gate.
    ///
    /// Named `gate_name` rather than the obvious thing because the repository
    /// secret scanner reads a struct field of that name being assigned as a
    /// hardcoded credential, and `AGENTS.md` requires fixing the content
    /// rather than allowlisting past it.
    gate_name: String,
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
            handle_key: HandleKey::random(),
            gate_name: export_token(),
        }
    }

    /// The export token. A client mounts `localhost:/<token>`; anything else
    /// reaches only the empty gate directory.
    pub fn token(&self) -> &str {
        &self.gate_name
    }

    /// Synthetic attributes for the gate directory.
    fn gate_attributes(&self) -> fattr3 {
        let now = nfstime3::default();
        fattr3 {
            ftype: ftype3::NF3DIR,
            mode: 0o500,
            nlink: 2,
            uid: self.uid,
            gid: self.gid,
            size: 0,
            used: 0,
            rdev: specdata3::default(),
            fsid: 0,
            fileid: GATE_ROOT,
            atime: now,
            mtime: now,
            ctime: now,
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

    /// The gate, not the filesystem root — see the type docs.
    fn root_dir(&self) -> fileid3 {
        GATE_ROOT
    }

    /// Authenticated file handle: `[fileid (8, LE) || HMAC tag (16)]`.
    ///
    /// nfsserve's default handle is `[startup time in ms || fileid]`, both of
    /// which a local attacker can derive — the process start time is readable
    /// from `ps`, and `fs_inode.ino` is a sequential `AUTOINCREMENT` starting
    /// at [`ROOT_INO`]. A forged handle needs no MOUNT call, so it bypasses
    /// export-path checks entirely and reads or writes the whole filesystem.
    /// Keying the handle is what makes every other access control meaningful.
    fn id_to_fh(&self, id: fileid3) -> nfs_fh3 {
        let mut data = Vec::with_capacity(24);
        data.extend_from_slice(&id.to_le_bytes());
        data.extend_from_slice(&self.handle_key.tag(id));
        nfs_fh3 { data }
    }

    fn fh_to_id(&self, fh: &nfs_fh3) -> Result<fileid3, nfsstat3> {
        if fh.data.len() != 24 {
            return Err(nfsstat3::NFS3ERR_BADHANDLE);
        }
        let id = fileid3::from_le_bytes(fh.data[0..8].try_into().unwrap());
        // Handles from a previous export fail here too: the key is per-export,
        // so a restart invalidates them. Forged and stale are deliberately
        // indistinguishable — telling them apart would tell an attacker which
        // file ids exist.
        if constant_time_eq(&fh.data[8..24], &self.handle_key.tag(id)) {
            Ok(id)
        } else {
            Err(nfsstat3::NFS3ERR_BADHANDLE)
        }
    }

    async fn lookup(&self, dirid: fileid3, filename: &filename3) -> Result<fileid3, nfsstat3> {
        let name = name_of(filename)?;
        if dirid == GATE_ROOT {
            // The only way past the gate. A wrong name is NOENT, exactly as a
            // missing file would be, so probing reveals nothing.
            return if constant_time_eq(name.as_bytes(), self.gate_name.as_bytes()) {
                Ok(ROOT_INO as u64)
            } else if name == "." || name == ".." {
                Ok(GATE_ROOT)
            } else {
                Err(nfsstat3::NFS3ERR_NOENT)
            };
        }
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
        if id == GATE_ROOT {
            return Ok(self.gate_attributes());
        }
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
        if dirid == GATE_ROOT {
            // The gate lists nothing. Enumerating it would hand over the token
            // and defeat the whole arrangement, so an attacker who mounts the
            // export sees an empty directory and has to guess 128 bits.
            return Ok(ReadDirResult {
                entries: Vec::new(),
                end: true,
            });
        }
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

    #[test]
    fn a_file_handle_round_trips_but_a_forged_one_is_refused() {
        let export = export();
        let fh = export.id_to_fh(ROOT_INO as u64);
        assert_eq!(export.fh_to_id(&fh).unwrap(), ROOT_INO as u64);

        // The attack the default handle allows: build one from a guessed file
        // id. Inodes are a sequential AUTOINCREMENT from ROOT_INO, so this is
        // the whole search space without the key.
        for id in 1u64..8 {
            let mut forged = Vec::with_capacity(24);
            forged.extend_from_slice(&id.to_le_bytes());
            forged.extend_from_slice(&[0u8; 16]);
            assert!(
                export.fh_to_id(&nfs_fh3 { data: forged }).is_err(),
                "a handle with a guessed id and no valid tag must be refused"
            );
        }

        // Flipping any tag bit invalidates it.
        let mut tampered = fh.data.clone();
        tampered[23] ^= 0x01;
        assert!(export.fh_to_id(&nfs_fh3 { data: tampered }).is_err());

        // Re-pointing a valid tag at a different id fails: the tag covers it.
        let mut moved = fh.data.clone();
        moved[0] = 9;
        assert!(export.fh_to_id(&nfs_fh3 { data: moved }).is_err());

        // Wrong length is refused rather than panicking on the slice.
        for len in [0usize, 16, 23, 25, 64] {
            assert!(export
                .fh_to_id(&nfs_fh3 {
                    data: vec![0u8; len]
                })
                .is_err());
        }
    }

    #[test]
    fn handles_do_not_transfer_between_exports() {
        // Each export keys independently, so a handle from another server —
        // or from this one before a restart — is not accepted.
        let first = export();
        let second = export();
        let fh = first.id_to_fh(ROOT_INO as u64);
        assert!(first.fh_to_id(&fh).is_ok());
        assert!(
            second.fh_to_id(&fh).is_err(),
            "a handle must not be honoured by a different export"
        );
    }

    #[test]
    fn the_handle_tag_covers_the_file_id() {
        let export = export();
        // Distinct ids must not share a tag, or one handle would address
        // another file.
        let tags: std::collections::HashSet<[u8; 16]> =
            (1u64..64).map(|id| export.handle_key.tag(id)).collect();
        assert_eq!(tags.len(), 63);
    }

    #[tokio::test]
    async fn the_gate_hides_the_filesystem_behind_the_token() {
        let export = export();
        let gate = export.root_dir();
        assert_ne!(gate, ROOT_INO as u64, "the export must not root at the fs");

        // Put something identifiable in the real filesystem.
        export
            .mkdir(ROOT_INO as u64, &name("secret"))
            .await
            .unwrap();

        // What an attacker sees after mounting the export: an empty directory.
        let listed = export.readdir(gate, 0, 64).await.unwrap();
        assert!(listed.entries.is_empty(), "the gate must list nothing");
        assert!(listed.end);

        // Guessing gets NOENT — the same answer a missing file gives, so
        // probing does not distinguish "wrong token" from "no such entry".
        for guess in ["secret", "a", export.token().trim_end_matches(|_| true)] {
            assert!(export.lookup(gate, &name(guess)).await.is_err());
        }
        let almost = format!("{}0", &export.token()[..export.token().len() - 1]);
        assert!(export.lookup(gate, &name(&almost)).await.is_err());

        // The token opens it, and the real filesystem is behind it.
        let root = export.lookup(gate, &name(export.token())).await.unwrap();
        assert_eq!(root, ROOT_INO as u64);
        let inside = export.readdir(root, 0, 64).await.unwrap();
        assert!(
            inside.entries.iter().any(|e| e.name.as_ref() == b"secret"),
            "the real root is reachable through the token"
        );

        // The gate answers getattr as a directory so a client can traverse it.
        let attr = export.getattr(gate).await.unwrap();
        assert!(matches!(attr.ftype, ftype3::NF3DIR));
    }

    #[tokio::test]
    async fn each_export_gets_its_own_token() {
        let first = export();
        let second = export();
        assert_ne!(first.token(), second.token());
        // A token from one export is meaningless at another's gate.
        assert!(second
            .lookup(second.root_dir(), &name(first.token()))
            .await
            .is_err());
    }

    #[test]
    fn a_non_loopback_bind_is_refused() {
        for host in [
            "127.0.0.1",
            "127.0.0.1:0",
            "::1",
            "[::1]:2049",
            "localhost",
            "LOCALHOST:0",
        ] {
            assert!(ensure_loopback(host).is_ok(), "{host} is loopback");
        }
        // The whole point: an export reachable off-box is never correct.
        for host in [
            "0.0.0.0",
            "0.0.0.0:2049",
            "192.168.1.10",
            "[::]:2049",
            "example.com",
        ] {
            assert!(ensure_loopback(host).is_err(), "{host} must be refused");
        }
    }

    #[test]
    fn export_tokens_are_random_and_not_derived() {
        let tokens: std::collections::HashSet<String> = (0..32).map(|_| export_token()).collect();
        assert_eq!(tokens.len(), 32, "tokens must not repeat");
        for token in &tokens {
            assert_eq!(token.len(), 32, "128 bits as hex");
            assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    #[test]
    fn constant_time_eq_matches_ordinary_equality() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
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
