//! Managed engine (coven-code) resolution and version gating.
//!
//! LICENSE BOUNDARY: the engine is GPL-3.0; coven is MIT. The engine is
//! always a separate process launched by path — never a Cargo dependency.
//! Do not add any claurst-* crate to this workspace.

use anyhow::{anyhow, bail, Context, Result};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
pub const ENGINE_BIN_NAME: &str = "coven-code.exe";
#[cfg(not(windows))]
pub const ENGINE_BIN_NAME: &str = "coven-code";

/// Oldest engine this coven build can drive (contract v1 surfaces).
pub const MIN_ENGINE_VERSION: (u64, u64, u64) = (0, 6, 1);

#[derive(Debug)]
pub enum EngineSource {
    EnvOverride, // COVEN_ENGINE_BIN
    Managed,     // ~/.coven/engine/<current>/
    PathLookup,  // coven-code on PATH
    LegacyHome,  // ~/.coven-code/bin/ (pre-unification installs)
}

#[derive(Debug)]
pub struct ResolvedEngine {
    pub path: PathBuf,
    pub source: EngineSource,
}

pub fn resolve() -> Option<ResolvedEngine> {
    let env_override = std::env::var_os("COVEN_ENGINE_BIN");
    let path_var = std::env::var_os("PATH");
    let home = dirs_next::home_dir();
    resolve_from(
        env_override.as_deref(),
        path_var.as_deref(),
        home.as_deref(),
    )
}

pub fn resolve_from(
    env_override: Option<&OsStr>,
    path_var: Option<&OsStr>,
    home: Option<&Path>,
) -> Option<ResolvedEngine> {
    // 1. COVEN_ENGINE_BIN explicit override
    if let Some(override_path) = env_override {
        let p = PathBuf::from(override_path);
        if is_executable(&p) {
            return Some(ResolvedEngine {
                path: p,
                source: EngineSource::EnvOverride,
            });
        }
        // Set but not executable: fall through to next source
    }

    // 2. Managed ~/.coven/engine/<current>/ENGINE_BIN_NAME
    if let Some(home) = home {
        let current_file = home.join(".coven").join("engine").join("current");
        if let Ok(version) = std::fs::read_to_string(&current_file) {
            let version = version.trim();
            if !version.is_empty() {
                let candidate = home
                    .join(".coven")
                    .join("engine")
                    .join(version)
                    .join(ENGINE_BIN_NAME);
                if is_executable(&candidate) {
                    return Some(ResolvedEngine {
                        path: candidate,
                        source: EngineSource::Managed,
                    });
                }
            }
        }
    }

    // 3. PATH lookup (honor Windows multi-name)
    if let Some(path_var) = path_var {
        for dir in std::env::split_paths(path_var) {
            for name in engine_bin_names() {
                let candidate = dir.join(name);
                if is_executable(&candidate) {
                    return Some(ResolvedEngine {
                        path: candidate,
                        source: EngineSource::PathLookup,
                    });
                }
            }
        }
    }

    // 4. Legacy ~/.coven-code/bin/ (pre-unification installs)
    if let Some(home) = home {
        let bin_dir = home.join(".coven-code").join("bin");
        for name in engine_bin_names() {
            let candidate = bin_dir.join(name);
            if is_executable(&candidate) {
                return Some(ResolvedEngine {
                    path: candidate,
                    source: EngineSource::LegacyHome,
                });
            }
        }
    }

    None
}

/// Resolve or produce the single actionable "engine missing" error.
pub fn require() -> Result<ResolvedEngine> {
    resolve().ok_or_else(|| {
        anyhow!(
            "The Coven engine is not installed.\n\n  Run: coven engine install\n\n\
             (or set COVEN_ENGINE_BIN to an existing coven-code binary)"
        )
    })
}

pub fn engine_version(binary: &Path) -> Result<(u64, u64, u64)> {
    let out = Command::new(binary)
        .arg("--version")
        .output()
        .with_context(|| format!("failed to run {} --version", binary.display()))?;
    if !out.status.success() {
        bail!("{} --version exited nonzero", binary.display());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    parse_version_output(&text)
        .ok_or_else(|| anyhow!("unparseable engine version output: {text:?}"))
}

pub fn parse_version_output(text: &str) -> Option<(u64, u64, u64)> {
    // Take the last whitespace-separated token, strip a leading 'v',
    // split on '.' into 3 parts, parse patch up to the first non-digit.
    // e.g. "coven-code 0.6.1\n" -> Some((0, 6, 1))
    //      "coven-code 0.6.1-rc1" -> Some((0, 6, 1))
    //      "garbage" -> None
    let token = text.split_whitespace().last()?;
    let token = token.strip_prefix('v').unwrap_or(token);
    let mut parts = token.splitn(3, '.');
    let major = parts.next()?.parse::<u64>().ok()?;
    let minor = parts.next()?.parse::<u64>().ok()?;
    let patch_str = parts.next()?;
    // Parse patch up to the first non-digit character
    let digit_end = patch_str
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(patch_str.len());
    let patch = patch_str[..digit_end].parse::<u64>().ok()?;
    Some((major, minor, patch))
}

pub fn version_meets_minimum(v: (u64, u64, u64)) -> bool {
    v >= MIN_ENGINE_VERSION
}

/// Human-readable error when the resolved engine is older than the minimum.
/// Pure (no I/O) so it can be unit-tested; used by the delegation handshake.
pub fn engine_too_old_message(
    binary: &Path,
    actual: (u64, u64, u64),
    min: (u64, u64, u64),
) -> String {
    format!(
        "The Coven engine at {} is version {}.{}.{}, older than the minimum \
         {}.{}.{} this coven build requires.\n\n  Run: coven engine install",
        binary.display(),
        actual.0,
        actual.1,
        actual.2,
        min.0,
        min.1,
        min.2,
    )
}

/// Returns the list of candidate binary names to look up, in priority order.
/// On Windows: exe, cmd, bat shims. On non-Windows: just the bare name.
fn engine_bin_names() -> &'static [&'static str] {
    if cfg!(windows) {
        &["coven-code.exe", "coven-code.cmd", "coven-code.bat"]
    } else {
        &["coven-code"]
    }
}

/// Check whether a path is an executable file.
/// Unix: file must exist and have at least one executable bit set.
/// Non-Unix: file must exist (is_file()).
fn is_executable(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch_exec(path: &std::path::Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn managed_engine_wins_over_path_and_legacy() {
        let home = tempfile::tempdir().unwrap();
        let managed = home
            .path()
            .join(".coven/engine/0.6.1")
            .join(ENGINE_BIN_NAME);
        let legacy = home.path().join(".coven-code/bin").join(ENGINE_BIN_NAME);
        touch_exec(&managed);
        touch_exec(&legacy);
        fs::write(home.path().join(".coven/engine/current"), "0.6.1").unwrap();
        let r = resolve_from(None, None, Some(home.path())).unwrap();
        assert_eq!(r.path, managed);
        assert!(matches!(r.source, EngineSource::Managed));
    }

    #[test]
    fn env_override_beats_everything() {
        let home = tempfile::tempdir().unwrap();
        let custom = home.path().join("custom-engine");
        touch_exec(&custom);
        let r = resolve_from(Some(custom.as_os_str()), None, Some(home.path())).unwrap();
        assert!(matches!(r.source, EngineSource::EnvOverride));
        assert_eq!(r.path, custom);
    }

    #[test]
    fn falls_back_to_legacy_home_dir() {
        let home = tempfile::tempdir().unwrap();
        let legacy = home.path().join(".coven-code/bin").join(ENGINE_BIN_NAME);
        touch_exec(&legacy);
        let r = resolve_from(None, None, Some(home.path())).unwrap();
        assert!(matches!(r.source, EngineSource::LegacyHome));
    }

    #[test]
    fn resolves_none_when_absent() {
        let home = tempfile::tempdir().unwrap();
        assert!(resolve_from(None, None, Some(home.path())).is_none());
    }

    #[test]
    fn parses_clap_version_line() {
        assert_eq!(parse_version_output("coven-code 0.6.1\n"), Some((0, 6, 1)));
        assert_eq!(parse_version_output("garbage"), None);
    }

    #[test]
    fn min_version_gate() {
        assert!(version_meets_minimum((0, 6, 1)));
        assert!(!version_meets_minimum((0, 5, 9)));
    }

    #[cfg(unix)]
    #[test]
    fn env_override_non_executable_falls_through() {
        let home = tempfile::tempdir().unwrap();
        let not_exec = home.path().join("not-exec");
        fs::write(&not_exec, b"").unwrap(); // written without the exec bit
        let legacy = home.path().join(".coven-code/bin").join(ENGINE_BIN_NAME);
        touch_exec(&legacy);
        let r = resolve_from(Some(not_exec.as_os_str()), None, Some(home.path())).unwrap();
        assert!(matches!(r.source, EngineSource::LegacyHome));
    }

    #[test]
    fn path_lookup_finds_windows_cmd_shim_name() {
        // The resolver's PathLookup must honor every platform bin-name, mirroring
        // the npm .cmd shim discovery the old main.rs helper covered.
        let names = engine_bin_names();
        if cfg!(windows) {
            assert!(names.contains(&"coven-code.cmd"));
        } else {
            assert_eq!(names, &["coven-code"]);
        }
    }

    #[test]
    fn engine_too_old_message_names_version_and_install_command() {
        let msg =
            engine_too_old_message(std::path::Path::new("/x/coven-code"), (0, 5, 9), (0, 6, 1));
        assert!(msg.contains("0.5.9"));
        assert!(msg.contains("0.6.1"));
        assert!(msg.contains("coven engine install"));
        assert!(msg.contains("/x/coven-code"));
    }

    #[test]
    fn whitespace_only_current_file_is_ignored() {
        let home = tempfile::tempdir().unwrap();
        // A managed binary exists, but `current` is blank → managed source must be skipped.
        let managed = home
            .path()
            .join(".coven/engine/0.6.1")
            .join(ENGINE_BIN_NAME);
        touch_exec(&managed);
        fs::write(home.path().join(".coven/engine/current"), "  \n").unwrap();
        let legacy = home.path().join(".coven-code/bin").join(ENGINE_BIN_NAME);
        touch_exec(&legacy);
        let r = resolve_from(None, None, Some(home.path())).unwrap();
        assert!(matches!(r.source, EngineSource::LegacyHome));
    }
}
