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
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        use windows_sys::Win32::{
            Foundation::{GENERIC_READ, GENERIC_WRITE},
            Storage::FileSystem::DELETE,
        };
        // Retirement and deletion must operate on this retained identity,
        // even if a later creator acquires the canonical marker name.
        options.access_mode(GENERIC_READ | GENERIC_WRITE | DELETE);
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
    #[cfg(windows)]
    {
        let _ = name;
        remove_completed_windows_marker(registry, publication)
    }
    #[cfg(not(windows))]
    {
        let _ = publication;
        match registry.remove_file(name) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).context("removing completed directory publication marker"),
        }
    }
}

#[cfg(windows)]
fn remove_completed_windows_marker(registry: &cap_std::fs::Dir, publication: &File) -> Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };
    if marker_already_removed(publication) {
        return Ok(());
    }
    // Both durability barriers have succeeded. Free the canonical name before
    // deletion becomes pending: Windows otherwise refuses a concurrent open
    // while any creator retains the old file. A crash after this rename leaves
    // only a harmless completed-file name; a crash before it repeats barriers.
    let retired = std::ffi::OsString::from(format!(".completed-{}", uuid::Uuid::new_v4()));
    if let Err(error) = rename_marker_handle(registry, publication, &retired) {
        if marker_already_removed(publication) {
            return Ok(());
        }
        return Err(error);
    }
    #[cfg(test)]
    if let Some(result) = tests::unlink_marker_at_hook(&retired) {
        return result;
    }
    let info = FILE_DISPOSITION_INFO {
        DeleteFile: true.into(),
    };
    // SAFETY: the live retained handle requested DELETE access, and info is a
    // correctly sized native structure. No canonical pathname is reopened.
    let removed = unsafe {
        SetFileInformationByHandle(
            publication.as_raw_handle(),
            FileDispositionInfo,
            (&info as *const FILE_DISPOSITION_INFO).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    };
    if removed != 0 {
        return Ok(());
    }
    // Preserve the failing operation's error before querying handle identity.
    let error = std::io::Error::last_os_error();
    if marker_already_removed(publication) {
        return Ok(());
    }
    Err(error).context("removing completed directory publication marker")
}

/// Rename this retained file, never whichever file now has its original name.
#[cfg(windows)]
fn rename_marker_handle(
    registry: &cap_std::fs::Dir,
    publication: &File,
    destination: &std::ffi::OsStr,
) -> Result<()> {
    use std::os::windows::{ffi::OsStrExt, io::AsRawHandle};
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfo, SetFileInformationByHandle, FILE_RENAME_INFO,
    };
    let encoded: Vec<u16> = destination.encode_wide().collect();
    anyhow::ensure!(!encoded.is_empty(), "completed marker name is empty");
    let name_bytes = encoded
        .len()
        .checked_mul(std::mem::size_of::<u16>())
        .context("completed marker name is too long")?;
    let name_length = u32::try_from(name_bytes).context("completed marker name is too long")?;
    let offset = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    // Windows requires at least sizeof(FILE_RENAME_INFO) plus the name bytes.
    // On 64-bit targets the structure has trailing alignment padding after its
    // one-element FileName field, so offset + name + one WCHAR is two bytes
    // short and SetFileInformationByHandle rejects it with ERROR_INVALID_PARAMETER.
    let bytes = std::mem::size_of::<FILE_RENAME_INFO>()
        .checked_add(name_bytes)
        .context("completed marker rename buffer is too large")?;
    let buffer_length =
        u32::try_from(bytes).context("completed marker rename buffer is too large")?;
    // The flexible UTF-16 tail needs aligned storage. The native structure's
    // existing FileName element and trailing padding leave room beyond the
    // counted string; FileNameLength deliberately excludes that storage.
    const {
        assert!(std::mem::align_of::<usize>() >= std::mem::align_of::<FILE_RENAME_INFO>());
    }
    let mut buffer = vec![0_usize; bytes.div_ceil(std::mem::size_of::<usize>())];
    let header = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    // SAFETY: buffer has native-header alignment and enough initialized bytes
    // for the header, every UTF-16 unit and its terminator. Both borrowed
    // handles remain live through the call. Refuse any existing destination.
    let renamed = unsafe {
        (*header).Anonymous.ReplaceIfExists = false;
        (*header).RootDirectory = registry.as_raw_handle();
        (*header).FileNameLength = name_length;
        std::ptr::copy_nonoverlapping(
            encoded.as_ptr(),
            buffer.as_mut_ptr().cast::<u8>().add(offset).cast::<u16>(),
            encoded.len(),
        );
        SetFileInformationByHandle(
            publication.as_raw_handle(),
            FileRenameInfo,
            buffer.as_ptr().cast(),
            buffer_length,
        )
    };
    if renamed == 0 {
        return Err(std::io::Error::last_os_error())
            .context("retiring completed directory publication marker");
    }
    Ok(())
}

/// Whether the file behind this retained handle is unlinked or marked for
/// deletion. A failed query reports false, so native retirement errors fail closed.
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

    #[cfg(windows)]
    type MarkerUnlinkHook = Box<dyn FnOnce(&std::ffi::OsStr) -> Result<()>>;

    #[cfg(windows)]
    std::thread_local! {
        static BEFORE_MARKER_UNLINK: std::cell::RefCell<Option<MarkerUnlinkHook>> =
            const { std::cell::RefCell::new(None) };
    }

    #[cfg(windows)]
    pub(super) fn unlink_marker_at_hook(name: &std::ffi::OsStr) -> Option<Result<()>> {
        let hook = BEFORE_MARKER_UNLINK.with(|slot| slot.borrow_mut().take());
        hook.map(|hook| hook(name))
    }

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

    #[cfg(windows)]
    fn open_marker_for_retirement(path: &Path) -> Result<File> {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        Ok(std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .access_mode(
                windows_sys::Win32::Foundation::GENERIC_READ
                    | windows_sys::Win32::Foundation::GENERIC_WRITE
                    | DELETE,
            )
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(path)?)
    }

    #[cfg(windows)]
    fn publication_registry_entries(path: &Path) -> Result<Vec<std::ffi::OsString>> {
        let mut entries = std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort();
        Ok(entries)
    }

    #[cfg(windows)]
    struct LocalSecurityDescriptor(*mut std::ffi::c_void);

    #[cfg(windows)]
    impl Drop for LocalSecurityDescriptor {
        fn drop(&mut self) {
            // SAFETY: the security APIs returned one LocalAlloc allocation.
            unsafe {
                windows_sys::Win32::Foundation::LocalFree(self.0);
            }
        }
    }

    #[cfg(windows)]
    struct SavedMarkerDacl {
        file: File,
        _descriptor: LocalSecurityDescriptor,
        dacl: *mut windows_sys::Win32::Security::ACL,
        protected: bool,
        restored: bool,
    }

    #[cfg(windows)]
    impl SavedMarkerDacl {
        fn restore(&mut self) -> Result<()> {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Security::{
                Authorization::{SetSecurityInfo, SE_FILE_OBJECT},
                DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
                UNPROTECTED_DACL_SECURITY_INFORMATION,
            };
            let protection = if self.protected {
                PROTECTED_DACL_SECURITY_INFORMATION
            } else {
                UNPROTECTED_DACL_SECURITY_INFORMATION
            };
            // SAFETY: descriptor owns dacl, and the retained file handle has
            // WRITE_DAC access. Null owner, group and SACL leave them unchanged.
            let status = unsafe {
                SetSecurityInfo(
                    self.file.as_raw_handle(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | protection,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    self.dacl,
                    std::ptr::null_mut(),
                )
            };
            if status != 0 {
                anyhow::bail!("restoring marker DACL: Windows error {status}");
            }
            self.restored = true;
            Ok(())
        }
    }

    #[cfg(windows)]
    impl Drop for SavedMarkerDacl {
        fn drop(&mut self) {
            if !self.restored {
                let _ = self.restore();
            }
        }
    }

    #[cfg(windows)]
    fn deny_marker_write_data(path: &Path) -> Result<SavedMarkerDacl> {
        use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
        use windows_sys::Win32::{
            Security::{
                Authorization::{
                    ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
                    SetSecurityInfo, SE_FILE_OBJECT,
                },
                GetSecurityDescriptorControl, GetSecurityDescriptorDacl, DACL_SECURITY_INFORMATION,
                PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED,
            },
            Storage::FileSystem::{
                FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, READ_CONTROL, WRITE_DAC,
            },
        };
        let file = std::fs::OpenOptions::new()
            .access_mode(READ_CONTROL | WRITE_DAC)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(path)?;
        let mut saved_dacl = std::ptr::null_mut();
        let mut saved_descriptor = std::ptr::null_mut();
        // SAFETY: output pointers are writable and the retained handle has
        // READ_CONTROL access. The descriptor owns the returned DACL pointer.
        let status = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut saved_dacl,
                std::ptr::null_mut(),
                &mut saved_descriptor,
            )
        };
        if status != 0 {
            anyhow::bail!("saving marker DACL: Windows error {status}");
        }
        let saved_descriptor = LocalSecurityDescriptor(saved_descriptor);
        anyhow::ensure!(!saved_dacl.is_null(), "saved marker DACL was null");
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: saved_descriptor is live and points to a native descriptor.
        if unsafe { GetSecurityDescriptorControl(saved_descriptor.0, &mut control, &mut revision) }
            == 0
        {
            return Err(std::io::Error::last_os_error()).context("reading marker DACL control");
        }

        // Deny FILE_WRITE_DATA to everyone on this private fixture, while
        // retaining full control for its owner so the original DACL is restorable.
        let sddl: Vec<u16> = "D:P(D;;0x00000002;;;WD)(A;;FA;;;OW)\0"
            .encode_utf16()
            .collect();
        let mut restricted_descriptor = std::ptr::null_mut();
        // SAFETY: sddl is NUL-terminated; the output receives one LocalAlloc allocation.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                1,
                &mut restricted_descriptor,
                std::ptr::null_mut(),
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error())
                .context("building restrictive marker DACL");
        }
        let restricted_descriptor = LocalSecurityDescriptor(restricted_descriptor);
        let mut present = 0;
        let mut restricted_dacl = std::ptr::null_mut();
        let mut defaulted = 0;
        // SAFETY: restricted_descriptor is live and all outputs are writable.
        if unsafe {
            GetSecurityDescriptorDacl(
                restricted_descriptor.0,
                &mut present,
                &mut restricted_dacl,
                &mut defaulted,
            )
        } == 0
            || present == 0
            || restricted_dacl.is_null()
        {
            anyhow::bail!("restrictive marker security descriptor had no DACL");
        }
        // SAFETY: the retained handle has WRITE_DAC, and the restricted DACL
        // remains live through this call. Other security components are unchanged.
        let status = unsafe {
            SetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                restricted_dacl,
                std::ptr::null_mut(),
            )
        };
        if status != 0 {
            anyhow::bail!("applying restrictive marker DACL: Windows error {status}");
        }
        Ok(SavedMarkerDacl {
            file,
            _descriptor: saved_descriptor,
            dacl: saved_dacl,
            protected: control & SE_DACL_PROTECTED != 0,
            restored: false,
        })
    }

    #[cfg(windows)]
    #[test]
    fn a_creator_succeeds_while_a_completed_marker_is_pending_deletion() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let directory = temp.path().join("shared");
        let marker = publication_marker(&directory).unwrap();
        let registry_path = marker.parent().unwrap().to_path_buf();
        let worker_directory = directory.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let creator = std::thread::spawn(move || {
            BEFORE_MARKER_UNLINK.with(|slot| {
                *slot.borrow_mut() = Some(Box::new(move |name| {
                    let removing = mark_delete_pending(&registry_path.join(name));
                    ready_tx.send(()).expect("report delete-pending marker");
                    release_rx.recv().expect("release marker deletion");
                    drop(removing);
                    Ok(())
                }));
            });
            create_dir_all(&worker_directory)
        });
        // A hang guard, far above fixture work. Early creator failure drops
        // the sole sender; parent unwind drops release so the worker cannot wedge.
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(120))
            .expect("first creator must reach marker deletion");
        let other_directory = directory.clone();
        let (done_tx, done_rx) = std::sync::mpsc::sync_channel(1);
        let concurrent_creator = std::thread::spawn(move || {
            let _ = done_tx.send(create_dir_all(&other_directory));
        });
        let concurrent = done_rx.recv_timeout(std::time::Duration::from_secs(120));
        let _ = release_tx.send(());
        let first = creator.join();
        let other = concurrent_creator.join();
        first.expect("first creator panicked")?;
        other.expect("concurrent creator panicked");
        let concurrent =
            concurrent.expect("concurrent publication must finish while deletion is held");
        concurrent.context("opening a marker while its completed peer is deleting it")?;
        assert!(directory.is_dir());
        assert!(!marker.exists());
        assert!(publication_registry_entries(marker.parent().unwrap())?.is_empty());
        Ok(())
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

        // A marker that is still ours is retired and removed by its handle.
        std::fs::write(&path, b"")?;
        let ours = open_marker_for_retirement(&path)?;
        remove_completed_marker(&registry, name, &ours)?;
        assert!(!path.exists());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn retirement_never_removes_a_later_marker_at_the_canonical_name() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let registry =
            cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority())?;
        let canonical = std::ffi::OsStr::new("shared");
        let canonical_path = temp.path().join(canonical);
        let retired = std::ffi::OsStr::new("retired-by-peer");
        let retired_path = temp.path().join(retired);

        std::fs::write(&canonical_path, b"old")?;
        let old = open_marker_for_retirement(&canonical_path)?;
        rename_marker_handle(&registry, &old, retired)?;
        std::fs::write(&canonical_path, b"replacement")?;

        remove_completed_marker(&registry, canonical, &old)?;
        assert_eq!(std::fs::read(&canonical_path)?, b"replacement");
        drop(old);
        assert!(!retired_path.exists());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn retirement_refuses_to_replace_an_existing_destination() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let registry =
            cap_std::fs::Dir::open_ambient_dir(temp.path(), cap_std::ambient_authority())?;
        let source_path = temp.path().join("source");
        let occupied_path = temp.path().join("occupied");
        std::fs::write(&source_path, b"source")?;
        std::fs::write(&occupied_path, b"occupied")?;
        let source = open_marker_for_retirement(&source_path)?;

        let error = rename_marker_handle(&registry, &source, std::ffi::OsStr::new("occupied"))
            .expect_err("retirement must not replace an occupied destination");
        let io_error = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<std::io::Error>())
            .expect("native rename error");
        assert_eq!(io_error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read(&source_path)?, b"source");
        assert_eq!(std::fs::read(&occupied_path)?, b"occupied");
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn publication_fails_closed_when_marker_write_data_is_denied() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let directory = temp.path().join("new");
        let marker = publication_marker(&directory).unwrap();
        std::fs::create_dir(marker.parent().unwrap())?;
        std::fs::write(&marker, b"guard")?;
        let mut saved_dacl = deny_marker_write_data(&marker)?;

        let result = create_dir_all(&directory);
        saved_dacl.restore()?;
        let error = result.expect_err("write-denied marker must stop publication");
        let io_error = error
            .chain()
            .find_map(|cause| cause.downcast_ref::<std::io::Error>())
            .expect("marker open must retain its native I/O error");
        assert_eq!(io_error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(error
            .to_string()
            .contains("opening directory publication marker"));
        assert_eq!(std::fs::read(&marker)?, b"guard");
        assert!(!directory.exists());
        Ok(())
    }

    #[cfg(any(unix, windows))]
    #[test]
    fn publication_marker_symlink_is_refused_without_touching_target() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let directory = temp.path().join("new");
        let marker = publication_marker(&directory).unwrap();
        std::fs::create_dir(marker.parent().unwrap())?;
        let target = temp.path().join("unrelated");
        std::fs::write(&target, b"unchanged")?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &marker)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, &marker)?;
        assert!(create_dir_all(&directory).is_err());
        assert!(!directory.exists());
        assert_eq!(std::fs::read(&target)?, b"unchanged");
        assert!(marker.symlink_metadata()?.file_type().is_symlink());
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "subprocess fixture invoked by completed_marker_retirement_survives_process_death"]
    fn completed_marker_retirement_process_fixture() -> Result<()> {
        let Some(root) = std::env::var_os("COVEN_RETIRED_MARKER_PROCESS_FIXTURE") else {
            return Ok(());
        };
        let root = std::path::PathBuf::from(root);
        let directory = root.join("published");
        let ready = root.join("ready");
        BEFORE_MARKER_UNLINK.with(|slot| {
            *slot.borrow_mut() = Some(Box::new(move |name| {
                let pending = ready.with_extension("pending");
                std::fs::write(&pending, name.as_encoded_bytes())?;
                std::fs::rename(&pending, &ready)?;
                loop {
                    std::thread::park();
                }
            }));
        });
        create_dir_all(&directory)
    }

    #[cfg(windows)]
    #[test]
    fn completed_marker_retirement_survives_process_death() -> Result<()> {
        struct ChildGuard(Option<std::process::Child>);
        impl Drop for ChildGuard {
            fn drop(&mut self) {
                if let Some(child) = &mut self.0 {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }

        let temp = tempfile::tempdir()?;
        let mut child = ChildGuard(Some(
            std::process::Command::new(std::env::current_exe()?)
                .arg("--exact")
                .arg("durable_fs::tests::completed_marker_retirement_process_fixture")
                .arg("--ignored")
                .arg("--nocapture")
                .env("COVEN_RETIRED_MARKER_PROCESS_FIXTURE", temp.path())
                .spawn()?,
        ));
        let ready = temp.path().join("ready");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        let retired = loop {
            match std::fs::read(&ready) {
                Ok(bytes) => break std::ffi::OsString::from(String::from_utf8(bytes)?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).context("reading process fixture readiness"),
            }
            if let Some(status) = child.0.as_mut().unwrap().try_wait()? {
                anyhow::bail!("retirement process fixture exited early with {status}");
            }
            anyhow::ensure!(
                std::time::Instant::now() < deadline,
                "retirement process fixture did not become ready within 120 seconds"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        child.0.as_mut().unwrap().kill()?;
        let status = child.0.as_mut().unwrap().wait()?;
        child.0 = None;
        assert!(
            !status.success(),
            "killed process fixture unexpectedly succeeded"
        );

        let directory = temp.path().join("published");
        let marker = publication_marker(&directory).unwrap();
        assert!(!marker.exists());
        assert!(marker.parent().unwrap().join(retired).is_file());
        create_dir_all(&directory)?;
        std::fs::write(directory.join("usable"), b"ok")?;
        assert_eq!(std::fs::read(directory.join("usable"))?, b"ok");
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
