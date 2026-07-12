//! Engine download/verify/install. The release ships COMPRESSED ARCHIVES
//! (.tar.gz on unix, .zip on windows), each containing a single `coven-code`
//! binary at the root. Flow: download archive (curl) -> verify archive
//! sha256 (sha2 crate) -> extract inner binary (tar) -> atomically install.
//! Shelling out to curl+tar avoids adding archive/http Rust crates and
//! matches the official install.sh. The pinned checksum is of the ARCHIVE.

use anyhow::{bail, Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Dev-mode default engine version, used when `coven engine install` is run
/// without `--version` and before Task 2.1's engine.lock provides the pin.
pub const DEFAULT_ENGINE_VERSION: &str = "0.6.1";

#[derive(Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    Installed,
    AlreadyPresent,
}

pub fn artifact_name(os: &str, arch: &str) -> String {
    match os {
        "windows" => format!("coven-code-windows-{arch}.zip"),
        _ => format!("coven-code-{os}-{arch}.tar.gz"),
    }
}

pub fn inner_binary_name(os: &str) -> &'static str {
    if os == "windows" {
        "coven-code.exe"
    } else {
        "coven-code"
    }
}

pub fn release_url(version: &str, artifact: &str) -> String {
    format!("https://github.com/OpenCoven/coven-code/releases/download/v{version}/{artifact}")
}

pub fn current_platform() -> Result<(&'static str, &'static str)> {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        other => bail!("unsupported platform for managed engine install: {other}"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => bail!("unsupported architecture: {other}"),
    };
    Ok((os, arch))
}

/// Removes scratch paths (the downloaded archive and the extraction stage
/// dir) when it drops — on success or on any early `?`/`bail!` return. The
/// final installed binary lives in `dest_dir`, not in these paths, so
/// unconditional cleanup is correct.
struct ScratchGuard {
    paths: Vec<PathBuf>,
}

impl ScratchGuard {
    fn new() -> Self {
        Self { paths: Vec::new() }
    }
    fn watch(&mut self, path: PathBuf) {
        self.paths.push(path);
    }
}

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }
}

/// Download, verify, extract, and activate the engine. Returns the installed
/// binary path and whether it was freshly installed or already present.
/// `expected_sha256` is the sha256 of the ARCHIVE; `None` skips
/// verification with a loud warning (dev mode).
pub fn install(
    version: &str,
    expected_sha256: Option<&str>,
    force: bool,
) -> Result<(PathBuf, InstallOutcome)> {
    let home = dirs_next::home_dir().context("cannot determine home directory")?;
    let engine_root = home.join(".coven").join("engine");
    let dest_dir = engine_root.join(version);
    let dest = dest_dir.join(crate::engine::ENGINE_BIN_NAME);
    if dest.exists() && !force {
        activate(&engine_root, version)?;
        return Ok((dest, InstallOutcome::AlreadyPresent));
    }
    let (os, arch) = current_platform()?;
    let artifact = artifact_name(os, arch);
    let url = release_url(version, &artifact);

    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("creating {}", dest_dir.display()))?;

    let mut scratch = ScratchGuard::new();

    // 1. Download the archive.
    let archive = dest_dir.join(&artifact);
    scratch.watch(archive.clone());
    let status = Command::new("curl")
        .args(["-fSL", "--retry", "3", "-o"])
        .arg(&archive)
        .arg(&url)
        .status()
        .context("failed to run curl (required for engine install)")?;
    if !status.success() {
        bail!("download failed: {url}");
    }

    // 2. Verify the ARCHIVE checksum BEFORE extraction (fail closed).
    let bytes = std::fs::read(&archive)
        .with_context(|| format!("reading downloaded archive {}", archive.display()))?;
    let digest = hex_digest(&bytes);
    match expected_sha256 {
        Some(expected) if !digest.eq_ignore_ascii_case(expected) => {
            bail!(
                "checksum mismatch for {artifact}: expected {expected}, got {digest}. Refusing to install."
            );
        }
        None => eprintln!(
            "warning: no pinned checksum for {artifact}; installing unverified (dev mode)"
        ),
        _ => {}
    }

    // `.stage` is a fixed per-version path; concurrent `install` of the SAME
    // version would race here. That is not a supported scenario (the fast-path
    // above returns when the binary already exists), so no lock is taken.

    // 3. Extract the inner binary using the system `tar`. On macOS this is
    //    bsdtar (also reads .zip); on Linux it is GNU tar. The non-Windows
    //    artifact is always .tar.gz, so .zip is only ever extracted on
    //    Windows, where tar.exe (bsdtar) handles it.
    let stage_dir = dest_dir.join(".stage");
    scratch.watch(stage_dir.clone());
    let _ = std::fs::remove_dir_all(&stage_dir);
    std::fs::create_dir_all(&stage_dir)
        .with_context(|| format!("creating stage dir {}", stage_dir.display()))?;
    let untar = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&stage_dir)
        .status()
        .context("failed to run tar (required to unpack the engine archive)")?;
    if !untar.success() {
        bail!("failed to extract {artifact}");
    }
    let extracted = stage_dir.join(inner_binary_name(os));
    if !extracted.is_file() {
        bail!(
            "archive {artifact} did not contain the expected binary {}",
            inner_binary_name(os)
        );
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&extracted, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(&extracted, &dest)?; // atomic within the same dir

    // 4. Publish the `current` pointer. The scratch guard cleans up the
    //    archive and stage dir when it drops at function end.
    activate(&engine_root, version)?;
    Ok((dest, InstallOutcome::Installed))
}

fn activate(engine_root: &Path, version: &str) -> Result<()> {
    // `current` is a plain text file (not a symlink): identical on Windows,
    // written via temp+rename for atomicity.
    let tmp = engine_root.join("current.tmp");
    std::fs::write(&tmp, version)?;
    std::fs::rename(tmp, engine_root.join("current"))?;
    Ok(())
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn artifact_name_is_a_platform_archive() {
        assert_eq!(
            artifact_name("macos", "aarch64"),
            "coven-code-macos-aarch64.tar.gz"
        );
        assert_eq!(
            artifact_name("linux", "x86_64"),
            "coven-code-linux-x86_64.tar.gz"
        );
        assert_eq!(
            artifact_name("windows", "x86_64"),
            "coven-code-windows-x86_64.zip"
        );
    }

    #[test]
    fn inner_binary_name_matches_platform() {
        assert_eq!(inner_binary_name("linux"), "coven-code");
        assert_eq!(inner_binary_name("windows"), "coven-code.exe");
    }

    #[test]
    fn release_url_targets_the_engine_repo() {
        assert_eq!(
            release_url("0.6.1", "coven-code-macos-aarch64.tar.gz"),
            "https://github.com/OpenCoven/coven-code/releases/download/v0.6.1/coven-code-macos-aarch64.tar.gz"
        );
    }
}
