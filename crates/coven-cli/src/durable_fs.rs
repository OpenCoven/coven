//! Checked namespace barriers for Ward targets, proposal claims and audit files.
use crate::threads_clock::request_diagnostics::{checkpoint, Phase};
use anyhow::{Context, Result};
use std::{fs::File, path::Path};

pub(crate) fn open_directory(path: &Path) -> Result<File> {
    cap_std::fs::Dir::open_ambient_dir(path, cap_std::ambient_authority())
        .map(cap_std::fs::Dir::into_std_file)
        .with_context(|| format!("opening durability directory {}", path.display()))
}

#[cfg(unix)]
fn sync_directory_native(directory: &File) -> Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    checkpoint(Phase::DirectoryNativeReopenBegin);
    // cap-std uses O_PATH on Linux and several other Unix targets. Those
    // descriptors support relative lookup but reject fsync with EBADF. Open
    // the retained directory itself, never its possibly replaced pathname.
    // SAFETY: the borrowed descriptor and NUL-terminated relative name remain
    // valid throughout openat. No mode argument is needed without O_CREAT.
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            c".".as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error())
            .context("reopening retained directory for durability");
    }
    // SAFETY: successful openat returned one newly owned descriptor.
    let readable = unsafe { File::from_raw_fd(descriptor) };
    checkpoint(Phase::DirectoryNativeReopenReady);
    // Rust uses F_FULLFSYNC on macOS, fsync elsewhere. Never ignore unsupported
    // barriers: a rename alone does not prove power-loss durability.
    checkpoint(Phase::DirectoryNativeFlushBegin);
    readable.sync_all().context("syncing durable directory")?;
    checkpoint(Phase::DirectoryNativeFlushReady);
    Ok(())
}

#[cfg(windows)]
fn sync_directory_native(directory: &File) -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::{
        Wdk::{
            Foundation::OBJECT_ATTRIBUTES,
            Storage::FileSystem::{
                NtCreateFile, FILE_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_FOR_BACKUP_INTENT,
                FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
            },
        },
        Win32::{
            Foundation::{CloseHandle, RtlNtStatusToDosError, GENERIC_WRITE, UNICODE_STRING},
            Storage::FileSystem::{
                FlushFileBuffers, GetVolumeInformationByHandleW, FILE_READ_ATTRIBUTES,
                FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE,
            },
            System::IO::IO_STATUS_BLOCK,
        },
    };
    let mut filesystem = [0_u16; 32];
    checkpoint(Phase::DirectoryNativeVolumeBegin);
    // MS-FSA 2.1.5.7 and product behavior note 80 guarantee directory structure
    // persistence only on NTFS; other filesystems may return a successful no-op.
    // SAFETY: the retained directory and output buffer are live for the call.
    let queried = unsafe {
        GetVolumeInformationByHandleW(
            directory.as_raw_handle(),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            filesystem.as_mut_ptr(),
            filesystem.len() as u32,
        )
    };
    if queried == 0 {
        return Err(std::io::Error::last_os_error())
            .context("querying durable directory filesystem");
    }
    checkpoint(Phase::DirectoryNativeVolumeReady);
    let length = filesystem
        .iter()
        .position(|ch| *ch == 0)
        .unwrap_or(filesystem.len());
    anyhow::ensure!(
        String::from_utf16_lossy(&filesystem[..length]).eq_ignore_ascii_case("NTFS"),
        "durable Ward directory writes require NTFS on Windows"
    );

    // ReOpenFile rejects directory handles. An empty native relative name
    // reopens the retained object itself, even if its pathname was replaced.
    let name = UNICODE_STRING::default();
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: directory.as_raw_handle(),
        ObjectName: &name,
        ..Default::default()
    };
    let mut status = IO_STATUS_BLOCK::default();
    let mut handle = std::ptr::null_mut();
    checkpoint(Phase::DirectoryNativeReopenBegin);
    // SAFETY: all pointers refer to live, correctly sized native structures;
    // successful creation gives this function ownership of exactly one handle.
    let result = unsafe {
        NtCreateFile(
            &mut handle,
            GENERIC_WRITE | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
            &attributes,
            &mut status,
            std::ptr::null(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_OPEN,
            FILE_DIRECTORY_FILE
                | FILE_SYNCHRONOUS_IO_NONALERT
                | FILE_OPEN_REPARSE_POINT
                | FILE_OPEN_FOR_BACKUP_INTENT,
            std::ptr::null(),
            0,
        )
    };
    if result < 0 {
        // SAFETY: translating a returned NTSTATUS has no pointer requirements.
        return Err(std::io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ))
        .context("reopening retained directory for durability");
    }
    checkpoint(Phase::DirectoryNativeReopenReady);
    checkpoint(Phase::DirectoryNativeFlushBegin);
    // SAFETY: handle was acquired above, is exclusively owned here, and is
    // closed exactly once even when flushing fails. Preserve the first error.
    let flushed = unsafe { FlushFileBuffers(handle) };
    let flush_error = (flushed == 0).then(std::io::Error::last_os_error);
    if flushed != 0 {
        checkpoint(Phase::DirectoryNativeFlushReady);
    }
    checkpoint(Phase::DirectoryNativeCloseBegin);
    // SAFETY: this remains the single close of the owned native handle.
    let closed = unsafe { CloseHandle(handle) };
    // Capture the native error before diagnostics can change GetLastError.
    let close_error = (closed == 0).then(std::io::Error::last_os_error);
    if closed != 0 {
        checkpoint(Phase::DirectoryNativeCloseReady);
    }
    if let Some(error) = flush_error {
        return Err(error).context("syncing durable NTFS directory");
    }
    if let Some(error) = close_error {
        return Err(error).context("closing durability directory handle");
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn sync_directory_native(_directory: &File) -> Result<()> {
    anyhow::bail!("durable directory writes are unsupported on this platform")
}

/// A recovery target may already contain the approved bytes. Sync its retained
/// identity too, before finalizing; never reopen an untrusted pathname.
pub(crate) fn sync_file(file: &File) -> Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle};
        use windows_sys::Win32::{
            Foundation::{GENERIC_WRITE, INVALID_HANDLE_VALUE},
            Storage::FileSystem::{
                ReOpenFile, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
                FILE_SHARE_WRITE,
            },
        };
        // SAFETY: ReOpenFile operates on the live retained regular file. The
        // returned handle is owned by File and closed on every exit.
        let handle = unsafe {
            ReOpenFile(
                file.as_raw_handle(),
                GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                FILE_FLAG_OPEN_REPARSE_POINT,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error())
                .context("reopening retained file for durability");
        }
        let writable = unsafe { File::from_raw_handle(handle) };
        writable.sync_all().context("syncing retained file")
    }
    #[cfg(not(windows))]
    file.sync_all().context("syncing retained file")
}

pub(crate) fn sync_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| {
            if path.is_absolute() {
                path
            } else {
                Path::new(".")
            }
        });
    sync_directory(&open_directory(parent)?)
}

/// Publish new components top-down. A marker precedes mkdir and remains until
/// the child and parent barriers succeed. A process-crash retry can therefore
/// distinguish our unfinished publication from trusted existing ancestry (which
/// may have an unwritable parent, notably Windows user profiles).
pub(crate) fn create_dir_all(path: &Path) -> Result<()> {
    use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt};
    use fs2::FileExt;
    let Some(marker) = publication_marker(path) else {
        anyhow::ensure!(path.is_dir(), "directory has no publishable component");
        return Ok(());
    };
    if path.is_dir() {
        match std::fs::symlink_metadata(&marker) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error).context("inspecting directory publication marker"),
            Ok(_) => {}
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    create_dir_all(parent)?;
    let parent_dir = cap_std::fs::Dir::open_ambient_dir(parent, cap_std::ambient_authority())?;
    match parent_dir.create_dir(PUBLICATION_DIRECTORY) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).context("creating directory publication registry"),
    }
    // Keep this directory permanently: unlinking it would detach another
    // creator's retained registry. Child basenames preserve filesystem case and
    // normalization rules, unlike hashes of raw filename bytes.
    let registry = parent_dir.open_dir_nofollow(PUBLICATION_DIRECTORY)?;
    #[cfg(windows)]
    ensure_matching_case_rules(&parent_dir, &registry)?;
    let name = path.file_name().context("directory has no component")?;
    let mut options = cap_std::fs::OpenOptions::new();
    options
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .follow(FollowSymlinks::No);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NONBLOCK);
    }
    // On macOS, concurrent creation or unlink of this basename can make even
    // O_CREAT return ENOENT. Retry through the retained registry with the same
    // no-follow options; disappearance never proves publication succeeded.
    // Bound the attempts so persistent namespace churn still fails closed.
    let mut retries = 15;
    let publication = loop {
        match registry.open_with(name, &options) {
            Ok(file) => break file.into_std(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && retries > 0 => {
                retries -= 1;
            }
            Err(error) => return Err(error).context("opening directory publication marker"),
        }
    };
    let metadata = publication.metadata()?;
    anyhow::ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "directory publication marker is not a regular file"
    );
    publication
        .lock_exclusive()
        .context("locking directory publication")?;
    let created = match parent_dir.create_dir(name) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => false,
        Err(error) => {
            return Err(error)
                .with_context(|| format!("creating durable directory {}", path.display()))
        }
    };
    let result = (|| {
        sync_directory(&parent_dir.open_dir_nofollow(name)?.into_std_file())?;
        sync_directory(&parent_dir.try_clone()?.into_std_file())
    })();
    if let Err(error) = result {
        if created {
            let _ = parent_dir.remove_dir(name);
        }
        // Keep the marker even if removing an empty component fails. Existence
        // must not let the next attempt skip its failed publication barrier.
        return Err(error);
    }
    remove_completed_marker(&registry, name, &publication)
}

/// The marker needs no independent sync: process death retains it, and after
/// OS recovery any surviving directory entry is already on stable storage.
/// Its removal is safe only after the parent barrier. A stale marker after
/// power loss merely repeats a successful publication.
fn remove_completed_marker(
    registry: &cap_std::fs::Dir,
    name: &std::ffi::OsStr,
    publication: &File,
) -> Result<()> {
    // A creator that shared our marker and finished first has already removed
    // it. The name may by now belong to a later creator's marker, which that
    // creator removes itself. On Windows a second removal of that name is not
    // harmless: while the other removal is between marking the file and
    // closing it, ours fails with access denied (#1653). Remove only our own.
    #[cfg(windows)]
    if marker_already_removed(publication) {
        return Ok(());
    }
    #[cfg(not(windows))]
    let _ = publication;
    match registry.remove_file(name) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("removing completed directory publication marker"),
    }
}

/// Whether the file behind this retained handle is unlinked or marked for
/// deletion. A failed query reports false, which keeps the removal by name.
#[cfg(windows)]
fn marker_already_removed(marker: &File) -> bool {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileStandardInfo, GetFileInformationByHandleEx, FILE_STANDARD_INFO,
    };
    let mut info = FILE_STANDARD_INFO::default();
    // SAFETY: live retained handle and correctly sized writable structure.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            marker.as_raw_handle(),
            FileStandardInfo,
            (&mut info as *mut FILE_STANDARD_INFO).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    };
    ok != 0 && (u8::from(info.DeletePending) != 0 || info.NumberOfLinks == 0)
}

#[cfg(windows)]
fn ensure_matching_case_rules(
    parent: &cap_std::fs::Dir,
    registry: &cap_std::fs::Dir,
) -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileCaseSensitiveInfo, GetFileInformationByHandleEx, FILE_CASE_SENSITIVE_INFO,
    };
    let flags = |directory: &cap_std::fs::Dir| -> Result<u32> {
        let file = directory.try_clone()?.into_std_file();
        let mut info = FILE_CASE_SENSITIVE_INFO::default();
        // SAFETY: live retained handle and correctly sized writable structure.
        let ok = unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileCaseSensitiveInfo,
                (&mut info as *mut FILE_CASE_SENSITIVE_INFO).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        };
        if ok == 0 {
            return Err(std::io::Error::last_os_error()).context("querying directory case rules");
        }
        Ok(info.Flags)
    };
    anyhow::ensure!(
        flags(parent)? == flags(registry)?,
        "directory publication registry must match its parent's case sensitivity"
    );
    Ok(())
}

pub(crate) const PUBLICATION_DIRECTORY: &str = ".coven-directory-publications";

fn publication_marker(path: &Path) -> Option<std::path::PathBuf> {
    let name = path.file_name()?;
    Some(path.parent()?.join(PUBLICATION_DIRECTORY).join(name))
}

pub(crate) fn sync_directory(directory: &File) -> Result<()> {
    #[cfg(test)]
    if DIRECTORY_SYNC_FAILURE.with(|remaining| match remaining.get() {
        Some(0) => {
            remaining.set(None);
            true
        }
        Some(count) => {
            remaining.set(Some(count - 1));
            false
        }
        None => false,
    }) {
        anyhow::bail!("injected directory sync failure");
    }
    sync_directory_native(directory)
}

#[cfg(test)]
thread_local! {
    static DIRECTORY_SYNC_FAILURE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}
#[cfg(test)]
pub(crate) fn fail_next_directory_sync() {
    fail_directory_sync_after(0);
}

#[cfg(test)]
pub(crate) fn fail_directory_sync_after(successful_barriers: usize) {
    DIRECTORY_SYNC_FAILURE.with(|remaining| remaining.set(Some(successful_barriers)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concurrent_directory_publication_succeeds_for_every_creator() -> Result<()> {
        let temp = tempfile::tempdir()?;
        for round in 0..32 {
            let directory = temp.path().join(format!("shared-{round}"));
            let barrier = std::sync::Barrier::new(8);
            let results = std::thread::scope(|scope| {
                let creators: Vec<_> = (0..8)
                    .map(|_| {
                        scope.spawn(|| {
                            barrier.wait();
                            create_dir_all(&directory)
                        })
                    })
                    .collect();
                creators
                    .into_iter()
                    .map(|creator| creator.join().expect("directory creator panicked"))
                    .collect::<Vec<_>>()
            });
            for result in results {
                result.with_context(|| format!("concurrent publication round {round}"))?;
            }
            assert!(directory.is_dir());
            assert!(!publication_marker(&directory).unwrap().exists());
        }
        Ok(())
    }

    /// Marks a file for deletion the way a concurrent removal does, and keeps
    /// it in that state until the returned handle is dropped.
    #[cfg(windows)]
    fn mark_delete_pending(path: &Path) -> File {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            FileDispositionInfo, SetFileInformationByHandle, DELETE, FILE_DISPOSITION_INFO,
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let file = std::fs::OpenOptions::new()
            .access_mode(DELETE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(path)
            .unwrap();
        let info = FILE_DISPOSITION_INFO {
            DeleteFile: true.into(),
        };
        // SAFETY: live handle opened with DELETE access and a correctly sized
        // readable structure.
        let ok = unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle(),
                FileDispositionInfo,
                (&info as *const FILE_DISPOSITION_INFO).cast(),
                std::mem::size_of_val(&info) as u32,
            )
        };
        assert_ne!(ok, 0, "{}", std::io::Error::last_os_error());
        file
    }

    /// #1653 in a fixed order. Our marker was removed by a creator that shared
    /// it, a later creator's marker took the name, and that marker is in the
    /// middle of its own removal when we finish.
    #[cfg(windows)]
    #[test]
    fn a_completed_creator_leaves_a_later_marker_under_its_name_alone() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let registry =
            cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority())?;
        let name = std::ffi::OsStr::new("shared");
        let path = temp.path().join(name);

        // Our marker, removed by a peer while we still hold it.
        std::fs::write(&path, b"")?;
        let ours = File::open(&path)?;
        assert!(!marker_already_removed(&ours));
        std::fs::remove_file(&path)?;
        assert!(marker_already_removed(&ours));

        // A later marker under the same name, caught mid-removal.
        std::fs::write(&path, b"")?;
        let removing = mark_delete_pending(&path);
        // Removing by name is what failed: this is the reported error.
        let by_name = registry.remove_file(name).unwrap_err();
        assert_eq!(by_name.kind(), std::io::ErrorKind::PermissionDenied);

        remove_completed_marker(&registry, name, &ours)?;
        drop(removing);
        assert!(!path.exists());

        // A marker that is still ours is removed by name as before.
        std::fs::write(&path, b"")?;
        let ours = File::open(&path)?;
        remove_completed_marker(&registry, name, &ours)?;
        assert!(!path.exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn publication_marker_symlink_is_refused_without_touching_target() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let directory = temp.path().join("new");
        let marker = publication_marker(&directory).unwrap();
        std::fs::create_dir(marker.parent().unwrap())?;
        let target = temp.path().join("unrelated");
        std::fs::write(&target, b"unchanged")?;
        std::os::unix::fs::symlink(&target, &marker)?;
        assert!(create_dir_all(&directory).is_err());
        assert!(!directory.exists());
        assert_eq!(std::fs::read(&target)?, b"unchanged");
        assert!(marker.symlink_metadata()?.file_type().is_symlink());
        Ok(())
    }

    #[test]
    fn directory_barrier_retains_identity_after_path_replacement() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let original = temp.path().join("original");
        create_dir_all(&original.join("nested/leaf"))?;
        #[cfg(not(windows))]
        let directory = open_directory(&original)?;
        #[cfg(windows)]
        let directory = {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
            };
            // Production cap-std handles forbid deletion. Permit it here to
            // exercise the sync primitive against a deliberately moved object.
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
                .open(&original)?
        };
        std::fs::rename(&original, temp.path().join("moved"))?;
        // A file at the old pathname would make a path-based reopen fail.
        std::fs::write(&original, b"replacement")?;
        for _ in 0..32 {
            sync_directory(&directory)?;
        }
        Ok(())
    }

    #[test]
    fn trusted_existing_ancestor_does_not_require_a_parent_flush() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fail_next_directory_sync();
        create_dir_all(temp.path())?;
        // No pending marker means no flush above the trusted boundary. The
        // injected failure must still be armed for our next actual barrier.
        assert!(sync_directory(&open_directory(temp.path())?).is_err());
        Ok(())
    }

    #[test]
    fn existing_directory_repeats_interrupted_publication_barrier() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let directory = temp.path().join("interrupted");
        // Model process death after marker + mkdir but before publication.
        let marker = publication_marker(&directory).unwrap();
        std::fs::create_dir(marker.parent().unwrap())?;
        std::fs::write(&marker, b"")?;
        std::fs::create_dir(&directory)?;
        fail_next_directory_sync();
        assert!(create_dir_all(&directory).is_err());
        create_dir_all(&directory)?;
        Ok(())
    }

    #[test]
    fn publication_retry_uses_filesystem_filename_equivalence() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let directory = temp.path().join("MixedCase");
        let alias = temp.path().join("mixedcase");
        let marker = publication_marker(&directory).unwrap();
        std::fs::create_dir(marker.parent().unwrap())?;
        std::fs::write(&marker, b"")?;
        std::fs::create_dir(&directory)?;
        if alias.is_dir() {
            fail_next_directory_sync();
            assert!(create_dir_all(&alias).is_err());
            create_dir_all(&alias)?;
            assert!(!marker.exists());
        }
        Ok(())
    }

    #[test]
    fn failed_directory_creation_barrier_does_not_publish_children() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("new");
        fail_next_directory_sync();
        assert!(create_dir_all(&root.join("child")).is_err());
        assert!(!root.exists());
        create_dir_all(&root.join("child"))?;
        assert!(root.join("child").is_dir());
        Ok(())
    }
}
