//! Explicit release updates belong to the manager, not the managed executable.
//! The release manifest authenticates integrity over GitHub HTTPS; it is not
//! an independent signature or a claim that the latest engine is the CLI pin.

use anyhow::{bail, Context, Result};
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

#[derive(Debug, clap::Args)]
pub(crate) struct UpdateArgs {
    #[arg(
        short = 'v',
        long,
        help = "Choose a release version (default: latest stable)"
    )]
    version: Option<String>,
    #[arg(
        short = 'f',
        long,
        help = "Download and verify again even if already installed"
    )]
    force: bool,
}

#[derive(clap::Parser)]
#[command(
    name = "coven code update",
    about = "Update the engine used by coven, with release checksum verification"
)]
struct CodeUpdate {
    #[command(flatten)]
    args: UpdateArgs,
}

pub(crate) fn run_code(args: &[OsString]) -> Result<()> {
    use clap::Parser;
    let parsed = CodeUpdate::parse_from(
        std::iter::once(OsString::from("coven code update")).chain(args.iter().cloned()),
    );
    run(parsed.args)
}

pub(crate) fn run(args: UpdateArgs) -> Result<()> {
    // A successful managed install cannot change an explicit override. Refuse
    // before any network or file mutation, rather than promising a false update.
    if std::env::var_os("COVEN_ENGINE_BIN").is_some() {
        bail!("COVEN_ENGINE_BIN selects a custom engine. Unset it to update the managed engine with `coven code update`, or update your custom engine separately. No engine was changed.");
    }
    let requested = args.version.as_deref().map(normalize_version).transpose()?;
    let scratch = tempfile::tempdir()?;
    let version = match requested {
        Some(version) => version.to_owned(),
        None => {
            println!("Checking the latest stable Coven Code release...");
            let metadata = scratch.path().join("release.json");
            download(
                "https://api.github.com/repos/OpenCoven/coven-code/releases/latest",
                &metadata,
                1024 * 1024,
            )?;
            #[derive(serde::Deserialize)]
            struct Release {
                tag_name: String,
                draft: bool,
                prerelease: bool,
            }
            let release: Release = serde_json::from_slice(&std::fs::read(metadata)?)
                .context("invalid latest-release response")?;
            if release.draft || release.prerelease {
                bail!("latest release is not a published stable engine");
            }
            normalize_version(&release.tag_name)?.to_owned()
        }
    };
    let target = crate::engine::parse_version_output(&version).context("invalid engine version")?;
    if !crate::engine::version_meets_minimum(target) {
        bail!("engine {version} is below this CLI's minimum supported engine version");
    }
    if let Some(current) = crate::engine::resolve() {
        let actual = crate::engine::engine_version(&current.path);
        if let Ok(actual) = actual {
            println!(
                "Current engine: {}.{}.{} at {}",
                actual.0,
                actual.1,
                actual.2,
                current.path.display()
            );
            if args.version.is_none() && actual > target {
                bail!("latest release {version} would downgrade the selected engine; use --version {version} only if that downgrade is intended");
            }
        } else {
            println!(
                "Current engine: version unavailable at {}",
                current.path.display()
            );
        }
    }
    println!(
        "Target engine: {version} (this CLI's bundled pin: {})",
        crate::engine::pinned_version()
    );
    let (os, arch) = crate::engine_install::current_platform()?;
    let artifact = crate::engine_install::artifact_name(os, arch);
    // Preserve the stronger built-in checksum when this release is pinned.
    let checksum = if version == crate::engine::pinned_version() {
        crate::engine::pinned_sha256(&artifact)
            .with_context(|| format!("no built-in checksum for {artifact}"))?
            .to_owned()
    } else {
        let manifest = scratch.path().join("SHA256SUMS");
        download(
            &crate::engine_install::release_url(&version, "SHA256SUMS"),
            &manifest,
            64 * 1024,
        )?;
        manifest_checksum(&std::fs::read_to_string(manifest)?, &artifact)?
    };
    let (path, outcome) =
        crate::engine_install::install(&version, &checksum, args.force, args.version.is_some())?;
    let resolved = crate::engine::require()?;
    if resolved.path != path {
        bail!(
            "engine {version} was installed at {}, but coven selects {}; run `coven engine status`",
            path.display(),
            resolved.path.display()
        );
    }
    match outcome {
        crate::engine_install::InstallOutcome::Installed => {
            println!("Updated Coven Code to {version}.")
        }
        crate::engine_install::InstallOutcome::AlreadyPresent => {
            println!("Coven Code {version} already present; selected for the next launch.")
        }
    }
    println!("Selected engine: {}", path.display());
    println!("Run `coven code` to start it. Existing sessions keep their running engine.");
    println!("This updates the engine. To update the Coven CLI itself: npm install -g @opencoven/cli@latest");
    Ok(())
}

/// Only stable numeric release names can enter managed paths or release URLs.
pub(crate) fn normalize_version(value: &str) -> Result<&str> {
    let value = value.strip_prefix('v').unwrap_or(value);
    let parts: Vec<_> = value.split('.').collect();
    if parts.len() != 3
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.bytes().all(|c| c.is_ascii_digit())
                || (part.len() > 1 && part.starts_with('0'))
                || part.parse::<u64>().is_err()
        })
    {
        bail!("invalid engine version; expected X.Y.Z (for example 0.9.0)");
    }
    Ok(value)
}

fn manifest_checksum(manifest: &str, artifact: &str) -> Result<String> {
    let mut seen = std::collections::HashSet::new();
    let mut selected = None;
    for line in manifest.lines() {
        let (digest, name) = line
            .split_once("  ")
            .context("malformed SHA256SUMS entry")?;
        if digest.len() != 64
            || !digest.bytes().all(|b| b.is_ascii_hexdigit())
            || name.is_empty()
            || name.contains(char::is_whitespace)
        {
            bail!("malformed SHA256SUMS entry");
        }
        if !seen.insert(name) {
            bail!("duplicate SHA256SUMS entry for {name}");
        }
        if name == artifact {
            selected = Some(digest.to_owned());
        }
    }
    selected.with_context(|| format!("SHA256SUMS has no checksum for {artifact}"))
}

pub(crate) fn download(url: &str, destination: &Path, max_bytes: u64) -> Result<()> {
    let status = Command::new("curl")
        // Ignore curlrc: it must not disable TLS verification or alter output.
        .args([
            "--disable",
            "--fail",
            "--show-error",
            "--silent",
            "--location",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--tlsv1.2",
            "--connect-timeout",
            "30",
            "--max-time",
            "300",
            "--max-filesize",
        ])
        .arg(max_bytes.to_string())
        .arg("--output")
        .arg(destination)
        .arg("--")
        .arg(url)
        .stdin(std::process::Stdio::null())
        .status()
        .context("failed to run curl (required for engine updates)")?;
    if !status.success() {
        bail!("engine download failed: {url}; the previous engine remains selected");
    }
    if std::fs::metadata(destination)?.len() > max_bytes {
        bail!("engine download exceeds size limit: {url}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_are_safe_canonical_release_components() {
        assert_eq!(normalize_version("v0.9.0").unwrap(), "0.9.0");
        for invalid in [
            "",
            "../x",
            "0.9.0/../x",
            "0.9",
            "00.9.0",
            "0.9.0-rc1",
            " 0.9.0",
            "0.9.0?x",
            "0.9.18446744073709551616",
        ] {
            assert!(normalize_version(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn manifest_requires_one_exact_valid_checksum() {
        let hash = "a".repeat(64);
        let entry = format!("{hash}  coven-code-linux-x86_64.tar.gz\n");
        assert_eq!(
            manifest_checksum(&entry, "coven-code-linux-x86_64.tar.gz").unwrap(),
            hash
        );
        for invalid in [
            String::new(),
            entry.repeat(2),
            entry.replace("  ", " "),
            entry.replace('a', "g"),
            entry.replace(".tar.gz", ".tar.gz.sig"),
        ] {
            assert!(manifest_checksum(&invalid, "coven-code-linux-x86_64.tar.gz").is_err());
        }
    }
}
