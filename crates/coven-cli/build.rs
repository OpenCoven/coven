// Stamp the binary with a version descriptor so `coven --version` is
// meaningful for source builds. npm-published binaries are still invoked
// through the wrapper at `npm/coven/bin/coven.js`, which intercepts
// `--version` before exec and prints the wrapper's package.json version
// stamped at publish time, so this build script primarily affects users who
// invoke the native binary directly (`cargo install --path ...`,
// `cargo build`, the bin inside an extracted npm tarball, etc.).
//
// Resolution order:
//   1. `$COVEN_BUILD_VERSION` when a release build explicitly supplies it.
//      Recovery tags use the stable package version here so native binaries
//      do not expose the recovery tag suffix.
//   2. `git describe --tags --always --dirty` against the working tree.
//      Local source builds and any CI that does a full fetch see this.
//   3. `$GITHUB_REF_NAME` if it looks like a tag (starts with `v`). Our
//      release workflow is tag-triggered, and `actions/checkout` defaults
//      to a shallow fetch with no tags, so step 2 fails in CI and we land
//      here with `v0.0.17` (or whatever the release tag is).
//   4. `<CARGO_PKG_VERSION> (unknown source)` — git unavailable AND not
//      running under a tag-triggered GitHub Actions build (e.g. downloaded
//      source tarball without `.git`).

use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // Rebuild when HEAD or tags change so the stamped version stays accurate
    // across commits, branch switches, and tag pushes. These are best-effort:
    // missing paths are silently ignored by Cargo.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/tags");
    // Cargo only re-runs the build script when a tracked env var changes;
    // declaring it here lets a CI re-run with a different tag re-stamp the
    // binary without `cargo clean`.
    println!("cargo:rerun-if-env-changed=COVEN_BUILD_VERSION");
    println!("cargo:rerun-if-env-changed=GITHUB_REF_NAME");

    let describe = explicit_build_version()
        .or_else(describe_from_git)
        .or_else(github_ref_name_if_tag)
        .unwrap_or_else(|| format!("{} (unknown source)", env!("CARGO_PKG_VERSION")));

    println!("cargo:rustc-env=COVEN_VERSION_DESC={}", describe);
}

fn explicit_build_version() -> Option<String> {
    std::env::var("COVEN_BUILD_VERSION")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn describe_from_git() -> Option<String> {
    let output = Command::new("git")
        .args(["describe", "--tags", "--always", "--dirty"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn github_ref_name_if_tag() -> Option<String> {
    std::env::var("GITHUB_REF_NAME")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| s.starts_with('v') && !s.is_empty())
}
