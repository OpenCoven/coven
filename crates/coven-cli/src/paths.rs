//! Single source of truth for Coven's on-disk path policy.
//!
//! Every `.coven`-family location is resolved here so the fallback chain
//! cannot drift between call sites (CLI, daemon plumbing, TUI, engine
//! management). Two roots exist on purpose:
//!
//! - **Coven home** ([`coven_home_dir`]) — `$COVEN_HOME`, else
//!   `<user home>/.coven`. Holds the store, daemon state, and exports.
//! - **Managed engine root** ([`managed_engine_root`]) — always
//!   `<user home>/.coven/engine`, *independent of* `$COVEN_HOME`, because the
//!   managed engine install is per-user, not per-store
//!   (see `docs/ENGINE-CONTRACT.md`).
//!
//! User *settings* live under the XDG config dir instead (see
//! [`crate::settings`]); that is a deliberate third root, not drift.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

pub(crate) const DEFAULT_COVEN_HOME_DIR: &str = ".coven";

/// Resolve the Coven home directory from the process environment.
///
/// Chain: `COVEN_HOME` → `HOME` → `USERPROFILE` → `HOMEDRIVE`+`HOMEPATH` →
/// platform home. Fails (rather than guessing a cwd-relative path) when no
/// home can be determined.
pub(crate) fn coven_home_dir() -> Result<PathBuf> {
    coven_home_from_env(
        std::env::var_os("COVEN_HOME"),
        std::env::var_os("HOME"),
        std::env::var_os("USERPROFILE"),
        std::env::var_os("HOMEDRIVE"),
        std::env::var_os("HOMEPATH"),
        dirs_next::home_dir().map(OsString::from),
    )
}

fn coven_home_from_env(
    coven_home: Option<OsString>,
    home: Option<OsString>,
    user_profile: Option<OsString>,
    home_drive: Option<OsString>,
    home_path: Option<OsString>,
    platform_home: Option<OsString>,
) -> Result<PathBuf> {
    if let Some(coven_home) = coven_home.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(coven_home));
    }

    let home = home
        .filter(|value| !value.is_empty())
        .or_else(|| user_profile.filter(|value| !value.is_empty()))
        .or_else(|| windows_home_from_drive_and_path(home_drive, home_path))
        .or_else(|| platform_home.filter(|value| !value.is_empty()))
        .ok_or_else(|| {
            anyhow!(
                "could not find a home directory for Coven. Set COVEN_HOME to choose a store path, \
for example `COVEN_HOME=$HOME/.coven` on macOS/Linux or \
`$env:COVEN_HOME=\"$env:USERPROFILE\\.coven\"` in PowerShell."
            )
        })?;
    Ok(PathBuf::from(home).join(DEFAULT_COVEN_HOME_DIR))
}

fn windows_home_from_drive_and_path(
    home_drive: Option<OsString>,
    home_path: Option<OsString>,
) -> Option<OsString> {
    let drive = home_drive?.into_string().ok()?;
    let path = home_path?.into_string().ok()?;
    if drive.is_empty() || path.is_empty() {
        return None;
    }
    Some(OsString::from(format!("{drive}{path}")))
}

/// The managed engine root under a user home: `<home>/.coven/engine`.
///
/// Deliberately keyed on the *user home*, not `$COVEN_HOME` — the managed
/// engine is a per-user install shared by every store.
pub(crate) fn managed_engine_root(home: &Path) -> PathBuf {
    home.join(DEFAULT_COVEN_HOME_DIR).join("engine")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coven_home_from_env_respects_coven_home() -> Result<()> {
        let path = coven_home_from_env(
            Some(OsString::from("/tmp/custom-coven-home")),
            Some(OsString::from("/tmp/ignored-home")),
            None,
            None,
            None,
            None,
        )?;

        assert_eq!(path, PathBuf::from("/tmp/custom-coven-home"));
        Ok(())
    }

    #[test]
    fn coven_home_from_env_defaults_under_home() -> Result<()> {
        let path = coven_home_from_env(
            None,
            Some(OsString::from("/tmp/user-home")),
            None,
            None,
            None,
            None,
        )?;

        assert_eq!(path, PathBuf::from("/tmp/user-home").join(".coven"));
        Ok(())
    }

    #[test]
    fn coven_home_from_env_uses_windows_drive_and_path_when_needed() -> Result<()> {
        let path = coven_home_from_env(
            None,
            None,
            None,
            Some(OsString::from("C:")),
            Some(OsString::from("\\Users\\hostname")),
            None,
        )?;

        assert_eq!(
            path,
            PathBuf::from("C:\\Users\\hostname").join(DEFAULT_COVEN_HOME_DIR)
        );
        Ok(())
    }

    #[test]
    fn coven_home_from_env_ignores_empty_values() -> Result<()> {
        let path = coven_home_from_env(
            Some(OsString::new()),
            Some(OsString::new()),
            Some(OsString::from("/tmp/profile-home")),
            None,
            None,
            None,
        )?;

        assert_eq!(
            path,
            PathBuf::from("/tmp/profile-home").join(DEFAULT_COVEN_HOME_DIR)
        );
        Ok(())
    }

    #[test]
    fn coven_home_from_env_fails_without_any_home() {
        let err = coven_home_from_env(None, None, None, None, None, None)
            .expect_err("must fail closed instead of guessing a path");
        assert!(err.to_string().contains("COVEN_HOME"));
    }

    #[test]
    fn managed_engine_root_is_user_home_scoped() {
        assert_eq!(
            managed_engine_root(Path::new("/tmp/user-home")),
            PathBuf::from("/tmp/user-home/.coven/engine")
        );
    }
}
