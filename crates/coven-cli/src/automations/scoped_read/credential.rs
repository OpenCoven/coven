//! The owner's Anthropic API key for Runtime Authority runs.
//!
//! Maintainer decision, 2026-10-20 (#857): the owner provides an API key, and
//! a `claude --bare` launch reads only that. The daemon keeps it in a file
//! under `COVEN_HOME`, readable only by the owner, and passes it only in the
//! environment of a Runtime Authority harness. It never enters a file the
//! model can read: the scoped read service refuses `COVEN_HOME`, and the
//! harness has no other tool.

use std::fs;
#[cfg(unix)]
use std::fs::{DirBuilder, OpenOptions};
#[cfg(unix)]
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use zeroize::Zeroizing;

/// The longest key accepted. Anthropic keys are about a hundred characters.
const KEY_BYTES: usize = 256;

/// Where the key lives under `coven_home`.
pub(crate) fn key_path(coven_home: &Path) -> PathBuf {
    coven_home.join("credentials").join("anthropic-api-key")
}

/// The key as it would be stored: `text` without its line ending, which must
/// be one run of printable ASCII and no longer than [`KEY_BYTES`].
pub(crate) fn parse_key(text: &str) -> Result<&str> {
    let key = text.trim_end_matches(['\n', '\r']);
    if key.is_empty() {
        bail!("no API key was given on standard input");
    }
    if key.len() > KEY_BYTES || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        bail!("that is not an API key: expected one line of printable characters");
    }
    Ok(key)
}

/// Saves `key`, replacing any saved key. The file is written whole beside the
/// old one, readable only by the owner, then renamed over it.
#[cfg(unix)]
pub(crate) fn store(coven_home: &Path, key: &str) -> Result<()> {
    let key = parse_key(key)?;
    let path = key_path(coven_home);
    let directory = path.parent().expect("the key has a folder");
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(directory)
        .with_context(|| format!("creating {}", directory.display()))?;
    let metadata = fs::symlink_metadata(directory)?;
    // SAFETY: geteuid() only reads the calling process's effective uid.
    if !metadata.is_dir() || metadata.uid() != unsafe { libc::geteuid() } {
        bail!(
            "refusing to keep the API key in {}: not a folder owned by this user",
            directory.display()
        );
    }
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))?;
    let staged = directory.join(".anthropic-api-key.new");
    let _ = fs::remove_file(&staged);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged)
        .with_context(|| format!("writing {}", staged.display()))?;
    file.write_all(key.as_bytes())?;
    file.sync_all()?;
    drop(file);
    fs::rename(&staged, &path).with_context(|| format!("saving {}", path.display()))?;
    Ok(())
}

/// The saved key, or `None` when there is none. A key file that is a link,
/// not owned by this user, or readable by anyone else is refused rather
/// than used.
#[cfg(unix)]
pub(crate) fn load(coven_home: &Path) -> Result<Option<Zeroizing<String>>> {
    let path = key_path(coven_home);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    // SAFETY: geteuid() only reads the calling process's effective uid.
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
    {
        bail!(
            "refusing the API key in {}: it must be a file only this user can read; save it again with `coven automations api-key set`",
            path.display()
        );
    }
    let text = Zeroizing::new(
        fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?,
    );
    let key = parse_key(&text)
        .with_context(|| format!("the API key in {} is not valid", path.display()))?;
    Ok(Some(Zeroizing::new(key.to_owned())))
}

/// Removes the saved key. Returns whether there was one.
pub(crate) fn remove(coven_home: &Path) -> Result<bool> {
    let path = key_path(coven_home);
    match fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("removing {}", path.display())),
    }
}

#[cfg(not(unix))]
pub(crate) fn store(_coven_home: &Path, _key: &str) -> Result<()> {
    bail!("Runtime Authority runs are not available on this platform")
}

#[cfg(not(unix))]
pub(crate) fn load(_coven_home: &Path) -> Result<Option<Zeroizing<String>>> {
    bail!("Runtime Authority runs are not available on this platform")
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn mode(path: &Path) -> u32 {
        fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn a_key_is_one_line_of_printable_text() {
        assert_eq!(parse_key("sk-ant-abc\n").unwrap(), "sk-ant-abc");
        assert_eq!(parse_key("sk-ant-abc\r\n").unwrap(), "sk-ant-abc");
        for bad in ["", "\n", "sk ant", "sk-ant\tx", "a\nb", "ключ"] {
            assert!(parse_key(bad).is_err(), "{bad:?}");
        }
        assert!(parse_key(&"k".repeat(KEY_BYTES)).is_ok());
        assert!(parse_key(&"k".repeat(KEY_BYTES + 1)).is_err());
    }

    #[test]
    fn a_saved_key_is_private_and_replaced_whole() {
        let home = tempfile::tempdir().unwrap();
        assert!(load(home.path()).unwrap().is_none());
        store(home.path(), "sk-ant-first\n").unwrap();
        store(home.path(), "sk-ant-second").unwrap();
        let path = key_path(home.path());
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
        assert_eq!(
            load(home.path()).unwrap().unwrap().as_str(),
            "sk-ant-second"
        );
        assert_eq!(
            fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1,
            "nothing staged is left behind"
        );
        assert!(remove(home.path()).unwrap());
        assert!(!remove(home.path()).unwrap());
        assert!(load(home.path()).unwrap().is_none());
    }

    #[test]
    fn a_key_others_could_read_or_a_link_is_refused() {
        let home = tempfile::tempdir().unwrap();
        store(home.path(), "sk-ant-key").unwrap();
        let path = key_path(home.path());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load(home.path()).is_err());

        fs::remove_file(&path).unwrap();
        let elsewhere = home.path().join("elsewhere");
        fs::write(&elsewhere, "sk-ant-planted").unwrap();
        fs::set_permissions(&elsewhere, fs::Permissions::from_mode(0o600)).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        assert!(load(home.path()).is_err());
    }
}
