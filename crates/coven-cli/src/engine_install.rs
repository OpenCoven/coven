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

/// Download, verify, extract, and activate the engine. Returns the installed
/// binary path and whether it was freshly installed or already present.
/// `expected_sha256` is the sha256 of the ARCHIVE and is required: archives
/// are never extracted or activated without a caller-provided or built-in
/// pin (see `engine::pinned_sha256` and the `--sha256` install flag).
pub fn install(
    version: &str,
    expected_sha256: &str,
    force: bool,
    allow_downgrade: bool,
) -> Result<(PathBuf, InstallOutcome)> {
    let version = crate::engine_update::normalize_version(version)?;
    if !is_sha256_hex(expected_sha256) {
        bail!("invalid sha256 pin: expected 64 hexadecimal characters");
    }
    let home = dirs_next::home_dir().context("cannot determine home directory")?;
    let engine_root = crate::paths::managed_engine_root(&home);
    std::fs::create_dir_all(&engine_root)?;
    // Serialize installers across versions: both activation and replacement
    // belong to one transaction. OS locks release automatically on process exit.
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(engine_root.join("install.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .context("another engine install is running; retry after it completes")?;
    // Recheck under the writer lock: another updater may have activated a
    // newer engine while this caller was fetching release metadata.
    if !allow_downgrade {
        if let Some(selected) = crate::engine::resolve() {
            if let Ok(actual) = crate::engine::engine_version(&selected.path) {
                let target = crate::engine::parse_version_output(version)
                    .context("invalid target engine version")?;
                if actual > target {
                    bail!("refusing to downgrade the currently selected engine to {version}; use an explicit --version if intended");
                }
            }
        }
    }
    let dest_dir = engine_root.join(version);
    let dest = dest_dir.join(crate::engine::ENGINE_BIN_NAME);
    if !force && verify_candidate(&dest, version).is_ok() {
        activate(&engine_root, version)?;
        return Ok((dest, InstallOutcome::AlreadyPresent));
    }
    let (os, arch) = current_platform()?;
    let artifact = artifact_name(os, arch);
    let url = release_url(version, &artifact);

    // Private, unique staging on the destination filesystem. Failure leaves
    // the previous engine and current pointer untouched; TempDir cleans up.
    let scratch = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(&engine_root)?;
    let archive = scratch.path().join(&artifact);
    crate::engine_update::download(&url, &archive, 512 * 1024 * 1024)?;

    // 2. Verify the ARCHIVE checksum BEFORE extraction (fail closed).
    let bytes = std::fs::read(&archive)
        .with_context(|| format!("reading downloaded archive {}", archive.display()))?;
    let digest = hex_digest(&bytes);
    if !digest.eq_ignore_ascii_case(expected_sha256) {
        bail!(
            "checksum mismatch for {artifact}: expected {expected_sha256}, got {digest}. Refusing to install."
        );
    }

    // Extract only the expected root member, never unrelated archive paths.
    let stage_dir = scratch.path().join("extracted");
    std::fs::create_dir(&stage_dir)?;
    let untar = Command::new("tar")
        .arg("-xf")
        .arg(&archive)
        .arg("-C")
        .arg(&stage_dir)
        .arg("--")
        .arg(inner_binary_name(os))
        .status()
        .context("failed to run tar (required to unpack the engine archive)")?;
    if !untar.success() {
        bail!("failed to extract {artifact}");
    }
    let extracted = stage_dir.join(inner_binary_name(os));
    if !std::fs::symlink_metadata(&extracted)?.file_type().is_file() {
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
    verify_candidate(&extracted, version)
        .context("downloaded engine failed validation; previous engine remains selected")?;
    std::fs::create_dir_all(&dest_dir)?;
    std::fs::rename(&extracted, &dest)
        .context("could not replace engine; close sessions using this version and retry")?;

    // Publish only after the candidate is verified and in its version directory.
    activate(&engine_root, version)?;
    Ok((dest, InstallOutcome::Installed))
}

fn verify_candidate(binary: &Path, version: &str) -> Result<()> {
    if !std::fs::symlink_metadata(binary)?.file_type().is_file()
        || !crate::install_conflict::is_runnable(binary)
    {
        bail!("engine candidate is not a regular executable");
    }
    let (status, stdout) = crate::engine::probe_stdout(
        binary,
        &["--version"],
        std::time::Instant::now() + crate::engine::PROBE_TIMEOUT,
        16 * 1024,
    )
    .context("engine version probe unavailable")?;
    if !status.success() {
        bail!("engine version probe exited nonzero");
    }
    let text = std::str::from_utf8(&stdout).context("engine version response is not UTF-8")?;
    let reported = text
        .split_whitespace()
        .last()
        .context("missing engine version")?;
    let reported = crate::engine_update::normalize_version(reported)?;
    let actual = crate::engine::parse_version_output(reported).context("invalid engine version")?;
    if reported != version || !crate::engine::version_meets_minimum(actual) {
        bail!("engine version mismatch: expected {version}, got {reported}");
    }
    Ok(())
}

fn activate(engine_root: &Path, version: &str) -> Result<()> {
    use std::io::Write;
    // Tempfile persist replaces the text pointer atomically on Windows too.
    let mut pointer = tempfile::NamedTempFile::new_in(engine_root)?;
    pointer.write_all(version.as_bytes())?;
    pointer.as_file().sync_all()?;
    pointer
        .persist(engine_root.join("current"))
        .context("could not activate engine; previous selection is unchanged")?;
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

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
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

    #[test]
    fn sha256_pin_validation_requires_full_hex_digest() {
        assert!(is_sha256_hex(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        ));
        assert!(!is_sha256_hex("abc"));
        assert!(!is_sha256_hex(
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdeg"
        ));
    }
}
