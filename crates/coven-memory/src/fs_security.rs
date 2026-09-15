//! Filesystem permission hardening for local archival memory state.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Ensure the directory that will contain sensitive archival memory files is private.
#[cfg(unix)]
pub fn ensure_private_parent(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    ensure_private_dir(parent)
}

/// Ensure the directory that will contain sensitive archival memory files exists and is private.
#[cfg(not(unix))]
pub fn ensure_private_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating memory directory {}", parent.display()))?;
    }
    Ok(())
}

/// Ensure an existing sensitive file is owned by the current user and not a symlink.
#[cfg(unix)]
pub fn validate_existing_private_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("checking {}", path.display())),
    };
    if metadata.file_type().is_symlink() {
        anyhow::bail!("refusing to use {}: path is a symlink", path.display());
    }
    if !metadata.file_type().is_file() {
        anyhow::bail!(
            "refusing to use {}: path is not a regular file",
            path.display()
        );
    }
    check_owned_by_current_user(path, metadata.uid())
}

#[cfg(not(unix))]
pub fn validate_existing_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

/// Force a sensitive file, and SQLite sidecar files if present, to be readable only by the owner.
#[cfg(unix)]
pub fn set_private_file(path: &Path) -> Result<()> {
    set_file_mode(path, 0o600)?;
    for sidecar in sqlite_sidecar_paths(path) {
        if sidecar.exists() {
            validate_existing_private_file(&sidecar)?;
            set_file_mode(&sidecar, 0o600)?;
        }
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn set_private_file(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn ensure_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};

    validate_requested_private_dirs(path)?;

    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .with_context(|| format!("creating private memory directory {}", path.display()))?;

    for dir in private_dir_chain(path)? {
        let metadata = std::fs::symlink_metadata(&dir)
            .with_context(|| format!("checking memory directory {}", dir.display()))?;
        if metadata.file_type().is_symlink() {
            anyhow::bail!(
                "refusing to use memory directory {}: path is a symlink",
                dir.display()
            );
        }
        if !metadata.file_type().is_dir() {
            anyhow::bail!(
                "refusing to use memory directory {}: path is not a directory",
                dir.display()
            );
        }
        check_owned_by_current_user(&dir, metadata.uid())?;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("setting private permissions on {}", dir.display()))?;
    }

    Ok(())
}

#[cfg(unix)]
fn validate_requested_private_dirs(path: &Path) -> Result<()> {
    let home = dirs_next::home_dir();
    let coven_home = home.as_ref().map(|home| home.join(".coven"));

    for ancestor in path.ancestors() {
        if ancestor == path
            || ancestor.file_name().is_some_and(|name| name == ".coven")
            || coven_home
                .as_ref()
                .is_some_and(|coven_home| ancestor == coven_home)
        {
            validate_existing_private_dir(ancestor)?;
        }
    }

    Ok(())
}

#[cfg(unix)]
fn validate_existing_private_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("checking {}", path.display())),
    };
    if metadata.file_type().is_symlink() {
        anyhow::bail!(
            "refusing to use memory directory {}: path is a symlink",
            path.display()
        );
    }
    if !metadata.file_type().is_dir() {
        anyhow::bail!(
            "refusing to use memory directory {}: path is not a directory",
            path.display()
        );
    }
    check_owned_by_current_user(path, metadata.uid())
}

#[cfg(unix)]
fn private_dir_chain(path: &Path) -> Result<Vec<PathBuf>> {
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalizing memory directory {}", path.display()))?;
    let home = dirs_next::home_dir().and_then(|home| home.canonicalize().ok());
    let coven_home = home.as_ref().map(|home| home.join(".coven"));

    let mut dirs = Vec::new();
    for ancestor in canonical.ancestors() {
        let should_harden = ancestor == canonical
            || ancestor.file_name().is_some_and(|name| name == ".coven")
            || coven_home
                .as_ref()
                .is_some_and(|coven_home| ancestor == coven_home);
        if should_harden {
            dirs.push(ancestor.to_path_buf());
        }
    }
    dirs.reverse();
    dirs.dedup();
    Ok(dirs)
}

#[cfg(unix)]
fn check_owned_by_current_user(path: &Path, owner_uid: u32) -> Result<()> {
    let euid = unsafe { libc::geteuid() };
    if owner_uid != euid {
        anyhow::bail!(
            "refusing to use {}: it is owned by uid {owner_uid}, not the current user (uid {euid})",
            path.display()
        );
    }
    Ok(())
}

#[cfg(unix)]
fn set_file_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("setting private permissions on {}", path.display()))
}

#[cfg(unix)]
fn sqlite_sidecar_paths(path: &Path) -> [PathBuf; 2] {
    [
        PathBuf::from(format!("{}-wal", path.display())),
        PathBuf::from(format!("{}-shm", path.display())),
    ]
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn hardens_memory_parent_permissions() {
        let root = std::env::temp_dir().join(format!(
            "coven-memory-security-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let db = root.join(".coven/memory/archival.sqlite3");

        ensure_private_parent(&db).unwrap();

        let coven_mode = std::fs::metadata(root.join(".coven"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let memory_mode = std::fs::metadata(root.join(".coven/memory"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(coven_mode, 0o700);
        assert_eq!(memory_mode, 0o700);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_symlinked_coven_home() {
        let root = std::env::temp_dir().join(format!(
            "coven-memory-symlink-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let target = std::env::temp_dir().join(format!(
            "coven-memory-symlink-target-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, root.join(".coven")).unwrap();
        let db = root.join(".coven/memory/archival.sqlite3");

        let error = ensure_private_parent(&db).unwrap_err().to_string();
        assert!(error.contains("path is a symlink"), "{error}");

        std::fs::remove_file(root.join(".coven")).unwrap();
        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(target).unwrap();
    }
}
