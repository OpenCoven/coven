use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
use cap_std::{ambient_authority, fs::Dir};
use fs2::FileExt;

const STATE_LOCK_FILE: &str = "state.lock";
pub(crate) const RESET_TRANSACTION_FILE: &str = "reset-transaction.json";

pub(crate) struct StateLock {
    file: std::fs::File,
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

pub(crate) fn acquire_shared(coven_home: &Path) -> Result<StateLock> {
    crate::daemon::ensure_private_coven_home(coven_home)?;
    let dir = Dir::open_ambient_dir(coven_home, ambient_authority())
        .with_context(|| format!("failed to open COVEN_HOME {}", coven_home.display()))?;
    let path = coven_home.join(STATE_LOCK_FILE);
    let file = open_lock_file_in(&dir, STATE_LOCK_FILE, &path)?;
    file.lock_shared()
        .with_context(|| format!("failed to acquire shared state lock {}", path.display()))?;
    match dir.symlink_metadata(RESET_TRANSACTION_FILE) {
        Ok(_) => anyhow::bail!(
            "an interrupted reset transaction requires recovery; rerun the intended `coven reset ... --apply` command before using other Coven commands"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "failed to inspect reset transaction marker in {}",
                    coven_home.display()
                )
            });
        }
    }
    Ok(StateLock { file })
}

#[cfg(test)]
pub(crate) fn try_acquire_exclusive(coven_home: &Path) -> Result<Option<StateLock>> {
    crate::daemon::ensure_private_coven_home(coven_home)?;
    let dir = Dir::open_ambient_dir(coven_home, ambient_authority())
        .with_context(|| format!("failed to open COVEN_HOME {}", coven_home.display()))?;
    try_acquire_exclusive_in(&dir, coven_home)
}

pub(crate) fn try_acquire_exclusive_in(dir: &Dir, coven_home: &Path) -> Result<Option<StateLock>> {
    let path = coven_home.join(STATE_LOCK_FILE);
    let file = open_lock_file_in(dir, STATE_LOCK_FILE, &path)?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(Some(StateLock { file })),
        Err(error) if is_lock_contended(&error) => Ok(None),
        Err(error) => Err(error)
            .with_context(|| format!("failed to acquire exclusive state lock {}", path.display())),
    }
}

pub(crate) fn open_lock_file(path: &Path) -> Result<std::fs::File> {
    let parent = path
        .parent()
        .context("state lock has no parent directory")?;
    let name = path.file_name().context("state lock has no file name")?;
    let dir = Dir::open_ambient_dir(parent, ambient_authority())
        .with_context(|| format!("failed to open state lock parent {}", parent.display()))?;
    open_lock_file_in(&dir, name, path)
}

pub(crate) fn open_lock_file_in(
    dir: &Dir,
    name: impl AsRef<Path>,
    display_path: &Path,
) -> Result<std::fs::File> {
    let mut options = cap_std::fs::OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    options.follow(FollowSymlinks::No);
    let file = dir
        .open_with(name, &options)
        .with_context(|| format!("failed to open state lock {}", display_path.display()))?
        .into_std();
    validate_lock_file(&file, display_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to protect state lock {}", display_path.display()))?;
    }
    Ok(file)
}

pub(crate) fn is_lock_contended(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::WouldBlock
        || error.raw_os_error() == fs2::lock_contended_error().raw_os_error()
}

#[cfg(unix)]
fn validate_lock_file(file: &std::fs::File, path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file.metadata()?;
    if metadata.file_type().is_symlink() || metadata.nlink() != 1 {
        anyhow::bail!(
            "refusing symlinked or multiply linked state lock {}",
            path.display()
        );
    }
    Ok(())
}

#[cfg(windows)]
fn validate_lock_file(file: &std::fs::File, path: &Path) -> Result<()> {
    use std::os::windows::fs::MetadataExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileStandardInfo, GetFileInformationByHandleEx, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_STANDARD_INFO,
    };

    if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        anyhow::bail!("refusing reparse-point state lock {}", path.display());
    }
    let mut info = FILE_STANDARD_INFO::default();
    // SAFETY: `file` owns a valid handle and `info` is a correctly sized,
    // writable FILE_STANDARD_INFO buffer that remains alive for the call.
    let result = unsafe {
        GetFileInformationByHandleEx(
            file.as_raw_handle() as _,
            FileStandardInfo,
            std::ptr::addr_of_mut!(info).cast(),
            u32::try_from(std::mem::size_of::<FILE_STANDARD_INFO>())
                .expect("FILE_STANDARD_INFO size fits in u32"),
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("reading state lock identity {}", path.display()));
    }
    if info.NumberOfLinks != 1 {
        anyhow::bail!("refusing multiply linked state lock {}", path.display());
    }
    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn validate_lock_file(_file: &std::fs::File, _path: &Path) -> Result<()> {
    Ok(())
}

/// Path of the profile-wide shared-state coordination lock.
pub(crate) fn shared_lock_path(coven_home: &Path) -> PathBuf {
    coven_home.join(STATE_LOCK_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_locks_block_exclusive_until_all_readers_exit() -> Result<()> {
        let home = tempfile::tempdir()?;
        let first = acquire_shared(home.path())?;
        let second = acquire_shared(home.path())?;
        assert!(try_acquire_exclusive(home.path())?.is_none());
        drop(first);
        assert!(try_acquire_exclusive(home.path())?.is_none());
        drop(second);
        assert!(try_acquire_exclusive(home.path())?.is_some());
        Ok(())
    }

    #[test]
    fn platform_lock_contention_is_recognized() {
        assert!(is_lock_contended(&fs2::lock_contended_error()));
    }

    #[cfg(unix)]
    #[test]
    fn anchored_exclusive_lock_stays_with_opened_home_after_path_replacement() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path().join("coven-home");
        let moved = temp.path().join("moved-home");
        std::fs::create_dir(&home)?;
        let dir = Dir::open_ambient_dir(&home, ambient_authority())?;
        std::fs::rename(&home, &moved)?;
        std::fs::create_dir(&home)?;

        let anchored = try_acquire_exclusive_in(&dir, &moved)?.expect("original home lock");
        let replacement =
            try_acquire_exclusive(&home)?.expect("replacement home has a distinct lock");
        assert!(try_acquire_exclusive_in(&dir, &moved)?.is_none());
        drop(replacement);
        drop(anchored);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn lock_open_refuses_symlinks_without_mutating_the_target() -> Result<()> {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let home = tempfile::tempdir()?;
        let outside = tempfile::NamedTempFile::new()?;
        outside
            .as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o640))?;
        symlink(outside.path(), shared_lock_path(home.path()))?;

        assert!(acquire_shared(home.path()).is_err());
        assert_eq!(
            outside.as_file().metadata()?.permissions().mode() & 0o777,
            0o640
        );
        Ok(())
    }
}
