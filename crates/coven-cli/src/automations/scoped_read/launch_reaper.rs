//! Reaping scoped launch folders a crashed daemon left behind (coven#1607).
//!
//! A scoped launch runs the harness in a private folder under the system's
//! temporary folder ([`super::admission::IsolatedLaunch`]). The harness writes
//! its home there, which can hold transcripts of what it read, and the folder
//! is removed when the launch ends. A daemon that is killed, panics past its
//! guards or loses power never ends the launch, so the folder stays behind.
//! [`super::channel::sweep_stale`] already removes the channel such a launch
//! leaves under `COVEN_HOME`; this module removes the launch folder.
//!
//! **What identifies a folder as ours.** Its name starts with
//! `coven-launch-<digest>-`, where the digest is of this `COVEN_HOME`, and it
//! holds a [`MARKER`] file whose text names the same home. The launch takes an
//! exclusive `flock` on the marker when it creates the folder and holds it for
//! the folder's life. A lock is released when its process ends however it
//! ends, so a folder whose marker can be locked has no live owner.
//!
//! **What may be removed.** Only a folder that is all of: a real directory
//! (never followed through a symlink), owned by this user, closed to group and
//! others, with a regular marker owned by this user that names this home, and
//! whose marker lock is free. Anything else that matches the name is retained
//! and counted as unattributed or live. Nothing else in the temporary folder
//! is examined.
//!
//! **How it is removed.** The folder is opened once, no-follow, and verified
//! through that handle. Its contents are removed relative to that handle and
//! to handles of its subfolders, each opened no-follow and checked against the
//! entry it was opened from. Nothing beneath is ever reached by a path from
//! the temporary folder, so renaming or replacing the folder after it was
//! verified cannot redirect a removal outside it, and a symlink inside is
//! removed itself, never followed. The marker goes last, while still locked.
//! The only step that names the folder from its parent again is the final
//! `rmdir`, which removes nothing but an empty directory: the residual that
//! coven#924 describes for Ward artifacts cannot remove data here.
//!
//! **Bounds.** A sweep examines at most [`Budget::folders`] matching folders,
//! removes at most [`Budget::entries`] entries and stops at [`Budget::time`].
//! A folder it cannot finish keeps its marker, so the next sweep resumes it.
//! Permission failures are counted and left; the sweep never changes a mode
//! to get further.

use std::ffi::{CStr, OsStr};
use std::io::Read;
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rustix::fs::{AtFlags, Dir, FileType, FlockOperation, Mode, OFlags, Stat};
use rustix::io::Errno;

use crate::automations::contract::canonical_json::sha256_hex;

/// The marker file inside each launch folder.
pub(crate) const MARKER: &str = ".coven-launch";
/// The marker's first line. The second is the home digest.
const MARKER_MAGIC: &str = "coven-scoped-launch/1";
/// The most bytes a marker may have.
const MARKER_BYTES: u64 = 256;
/// The most levels below a launch folder the sweep descends.
const MAX_DEPTH: usize = 64;

/// How much one sweep may do.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Budget {
    /// Matching folders examined.
    pub folders: usize,
    /// Entries removed, across every folder.
    pub entries: usize,
    /// Elapsed time.
    pub time: Duration,
}

/// The daemon's startup budget.
pub(crate) const STARTUP_BUDGET: Budget = Budget {
    folders: 64,
    entries: 200_000,
    time: Duration::from_secs(5),
};

/// What one sweep did. It carries counts only: no path, name or content.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Report {
    /// Folders removed whole.
    pub removed: usize,
    /// Folders whose marker lock is held: a launch is live.
    pub live: usize,
    /// Matching entries that are not positively ours: retained.
    pub unattributed: usize,
    /// Folders left part-removed because the budget ran out: resumed next
    /// time.
    pub incomplete: usize,
    /// Folders left because an entry could not be removed.
    pub failed: usize,
    /// Whether the sweep stopped before examining every matching folder.
    pub budget_exhausted: bool,
}

impl Report {
    /// Whether the sweep found anything worth logging.
    pub(crate) fn is_quiet(&self) -> bool {
        *self == Self::default()
    }
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "scoped launch folders: removed={} live={} unattributed={} incomplete={} failed={}{}",
            self.removed,
            self.live,
            self.unattributed,
            self.incomplete,
            self.failed,
            if self.budget_exhausted {
                " budget_exhausted"
            } else {
                ""
            }
        )
    }
}

/// The 64-hex digest naming the home at `coven_home`, canonicalized when it
/// exists.
pub(crate) fn home_digest(coven_home: &Path) -> String {
    let home = std::fs::canonicalize(coven_home).unwrap_or_else(|_| coven_home.to_path_buf());
    sha256_hex(home.as_os_str().as_bytes())
}

/// The name prefix of every launch folder for the home with `digest`.
pub(crate) fn folder_prefix(digest: &str) -> String {
    format!("coven-launch-{}-", &digest[..16])
}

/// The marker's exact text for the home with `digest`.
fn marker_text(digest: &str) -> String {
    format!("{MARKER_MAGIC}\n{digest}\n")
}

/// Writes the marker into the new, empty launch folder `root` and locks it.
/// The returned file holds the lock: keep it for the folder's life.
pub(crate) fn mark(root: &Path, digest: &str) -> Result<std::fs::File> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(root.join(MARKER))
        .context("creating the launch folder's marker")?;
    rustix::fs::flock(&file, FlockOperation::NonBlockingLockExclusive)
        .context("locking the launch folder's marker")?;
    file.write_all(marker_text(digest).as_bytes())
        .context("writing the launch folder's marker")?;
    file.sync_all().ok();
    Ok(file)
}

/// Removes the stale launch folders of `coven_home` from the system's
/// temporary folder, within the startup budget.
pub(crate) fn reap_stale(coven_home: &Path) -> Result<Report> {
    reap_stale_in(
        &std::env::temp_dir(),
        coven_home,
        STARTUP_BUDGET,
        &mut |_| {},
    )
}

/// What became of one matching entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Disposition {
    Removed,
    Live,
    Unattributed,
    Incomplete,
    Failed,
}

/// Running totals against a [`Budget`].
struct Spend {
    budget: Budget,
    started: Instant,
    entries: usize,
}

impl Spend {
    fn exhausted(&self) -> bool {
        self.entries >= self.budget.entries || self.started.elapsed() >= self.budget.time
    }
}

/// [`reap_stale`] beneath `base`. `verified` runs after a folder is verified
/// and locked and before anything in it is removed, with the folder's name:
/// tests use it to replace the folder at that moment.
pub(crate) fn reap_stale_in(
    base: &Path,
    coven_home: &Path,
    budget: Budget,
    verified: &mut dyn FnMut(&OsStr),
) -> Result<Report> {
    let digest = home_digest(coven_home);
    let prefix = folder_prefix(&digest);
    let base = rustix::fs::open(
        base,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .with_context(|| format!("opening the temporary folder {}", base.display()))?;
    let mut names = Vec::new();
    for entry in Dir::read_from(&base).context("listing the temporary folder")? {
        let entry = entry.context("listing the temporary folder")?;
        let name = entry.file_name().to_bytes();
        if name.starts_with(prefix.as_bytes()) {
            names.push(name.to_vec());
        }
    }
    names.sort();

    let mut report = Report::default();
    let mut spend = Spend {
        budget,
        started: Instant::now(),
        entries: 0,
    };
    for (index, name) in names.iter().enumerate() {
        if index >= budget.folders || spend.exhausted() {
            report.budget_exhausted = true;
            break;
        }
        let name = OsStr::from_bytes(name);
        match reap_one(&base, name, &digest, &mut spend, verified) {
            Disposition::Removed => report.removed += 1,
            Disposition::Live => report.live += 1,
            Disposition::Unattributed => report.unattributed += 1,
            Disposition::Incomplete => {
                report.incomplete += 1;
                report.budget_exhausted = true;
            }
            Disposition::Failed => report.failed += 1,
        }
    }
    Ok(report)
}

/// The effective user id.
fn euid() -> u32 {
    // SAFETY: geteuid() only reads the calling process's effective uid.
    unsafe { libc::geteuid() }
}

fn is_private_directory(stat: &Stat) -> bool {
    FileType::from_raw_mode(stat.st_mode) == FileType::Directory
        && stat.st_uid == euid()
        && stat.st_mode & 0o077 == 0
}

fn same_object(a: &Stat, b: &Stat) -> bool {
    a.st_dev == b.st_dev && a.st_ino == b.st_ino
}

fn open_directory_nofollow<Fd: AsFd>(parent: Fd, name: &OsStr) -> rustix::io::Result<OwnedFd> {
    rustix::fs::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
}

fn reap_one(
    base: &OwnedFd,
    name: &OsStr,
    digest: &str,
    spend: &mut Spend,
    verified: &mut dyn FnMut(&OsStr),
) -> Disposition {
    // 1. A real, private directory of ours, held open from here on.
    let Ok(folder) = open_directory_nofollow(base, name) else {
        return Disposition::Unattributed;
    };
    let Ok(folder_stat) = rustix::fs::fstat(&folder) else {
        return Disposition::Unattributed;
    };
    if !is_private_directory(&folder_stat) {
        return Disposition::Unattributed;
    }
    // 2. Its marker names this home.
    let Ok(marker) = rustix::fs::openat(
        &folder,
        MARKER,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return Disposition::Unattributed;
    };
    let marker_owned = rustix::fs::fstat(&marker).is_ok_and(|stat| {
        FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
            && stat.st_uid == euid()
            && u64::try_from(stat.st_size).is_ok_and(|size| size <= MARKER_BYTES)
    });
    if !marker_owned {
        return Disposition::Unattributed;
    }
    let mut marker = std::fs::File::from(marker);
    let mut text = String::new();
    if (&mut marker)
        .take(MARKER_BYTES)
        .read_to_string(&mut text)
        .is_err()
        || text != marker_text(digest)
    {
        return Disposition::Unattributed;
    }
    // 3. No live owner. The lock is held until this function returns.
    match rustix::fs::flock(&marker, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {}
        Err(Errno::WOULDBLOCK) => return Disposition::Live,
        Err(_) => return Disposition::Failed,
    }
    verified(name);

    // 4. The contents, through the held handle, marker last.
    match remove_contents(&folder, 0, spend, true) {
        Removal::Done => {}
        Removal::Budget => return Disposition::Incomplete,
        Removal::Failed => return Disposition::Failed,
    }
    if rustix::fs::unlinkat(&folder, MARKER, AtFlags::empty()).is_err() {
        return Disposition::Failed;
    }
    // 5. The folder itself, if its name still holds it. `rmdir` removes
    //    only an empty directory.
    let still_ours = rustix::fs::statat(base, name, AtFlags::SYMLINK_NOFOLLOW)
        .is_ok_and(|stat| same_object(&stat, &folder_stat));
    if !still_ours {
        return Disposition::Failed;
    }
    match rustix::fs::unlinkat(base, name, AtFlags::REMOVEDIR) {
        Ok(()) => Disposition::Removed,
        Err(_) => Disposition::Failed,
    }
}

enum Removal {
    Done,
    Budget,
    Failed,
}

/// Removes everything in the directory `directory` holds, except the marker
/// when `top`. Every entry is reached relative to `directory`.
fn remove_contents(directory: &OwnedFd, depth: usize, spend: &mut Spend, top: bool) -> Removal {
    if depth > MAX_DEPTH {
        return Removal::Failed;
    }
    let names: Vec<Vec<u8>> = match Dir::read_from(directory) {
        Ok(entries) => {
            let mut names = Vec::new();
            for entry in entries {
                let Ok(entry) = entry else {
                    return Removal::Failed;
                };
                let name: &CStr = entry.file_name();
                let bytes = name.to_bytes();
                if bytes == b"." || bytes == b".." || (top && bytes == MARKER.as_bytes()) {
                    continue;
                }
                names.push(bytes.to_vec());
            }
            names
        }
        Err(_) => return Removal::Failed,
    };
    let mut failed = false;
    for name in names {
        if spend.exhausted() {
            return Removal::Budget;
        }
        let name = OsStr::from_bytes(&name);
        let Ok(stat) = rustix::fs::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) else {
            failed = true;
            continue;
        };
        if FileType::from_raw_mode(stat.st_mode) == FileType::Directory {
            let Ok(child) = open_directory_nofollow(directory, name) else {
                failed = true;
                continue;
            };
            let opened = rustix::fs::fstat(&child);
            if !opened.is_ok_and(|opened| same_object(&opened, &stat)) {
                failed = true;
                continue;
            }
            match remove_contents(&child, depth + 1, spend, false) {
                Removal::Done => {}
                Removal::Budget => return Removal::Budget,
                Removal::Failed => {
                    failed = true;
                    continue;
                }
            }
            if rustix::fs::unlinkat(directory, name, AtFlags::REMOVEDIR).is_err() {
                failed = true;
                continue;
            }
        } else if rustix::fs::unlinkat(directory, name, AtFlags::empty()).is_err() {
            failed = true;
            continue;
        }
        spend.entries += 1;
    }
    if failed {
        Removal::Failed
    } else {
        Removal::Done
    }
}

#[cfg(test)]
#[path = "launch_reaper_tests.rs"]
mod tests;
