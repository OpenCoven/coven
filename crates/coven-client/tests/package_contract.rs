//! Package contract for the published `opencoven-coven-client` crate.
//!
//! These assertions run against the crate exactly as `cargo package` ships it:
//! the manifest, the README, and the fixtures are read through `include_str!`,
//! so a tarball missing any of them fails to compile this test rather than
//! passing silently. `scripts/verify-coven-client-package.mjs` checks the same
//! contract from outside Cargo for the release workflow.

const MANIFEST: &str = include_str!("../Cargo.toml");
const LIB: &str = include_str!("../src/lib.rs");
const README: &str = include_str!("../README.md");
const HEALTH_FIXTURE: &str = include_str!("../fixtures/health.json");
const ERROR_FIXTURE: &str = include_str!("../fixtures/error.json");

/// Dependencies a client crate must never pick up: the daemon's own CLI, the
/// agent runtime, and terminal-UI stacks. Each would drag a consumer into a
/// binary's dependency graph for a library that only speaks local IPC.
const FORBIDDEN_DEPENDENCIES: &[&str] = &[
    "coven-cli",
    "coven-agents",
    "clap",
    "ratatui",
    "crossterm",
    "dialoguer",
    "inquire",
];

fn dependency_names() -> Vec<String> {
    let mut names = Vec::new();
    let mut in_dependency_table = false;
    for line in MANIFEST.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_dependency_table = trimmed.ends_with("dependencies]");
            continue;
        }
        if !in_dependency_table || trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((name, _)) = trimmed.split_once('=') {
            names.push(name.trim().trim_matches('"').to_string());
        }
    }
    names
}

#[test]
fn package_name_is_owner_adjacent() {
    assert_eq!(env!("CARGO_PKG_NAME"), "opencoven-coven-client");
}

#[test]
fn version_stays_pre_1_0() {
    assert_eq!(
        env!("CARGO_PKG_VERSION_MAJOR"),
        "0",
        "the crate is owner-adjacent and pre-1.0; a 1.x release needs its own design"
    );
}

#[test]
fn publication_metadata_is_complete() {
    assert_eq!(env!("CARGO_PKG_LICENSE"), "MIT");
    assert_eq!(
        env!("CARGO_PKG_REPOSITORY"),
        "https://github.com/OpenCoven/coven"
    );
    assert_eq!(env!("CARGO_PKG_README"), "README.md");
    assert!(
        env!("CARGO_PKG_DESCRIPTION").contains("coven.daemon.v1"),
        "the description names the contract the crate speaks"
    );
}

#[test]
fn library_keeps_the_coven_client_crate_name() {
    // Compare parsed lines rather than a literal "\n" so a CRLF checkout on
    // Windows reads the same manifest as a LF checkout does.
    let lines: Vec<&str> = MANIFEST.lines().map(str::trim).collect();
    let lib_table = lines
        .iter()
        .position(|line| *line == "[lib]")
        .expect("manifest declares a [lib] table");
    assert_eq!(
        lines.get(lib_table + 1).copied(),
        Some("name = \"coven_client\""),
        "downstream code imports `coven_client::…`; renaming the library is a breaking change"
    );
}

#[test]
fn no_cli_tui_or_agent_runtime_dependency() {
    let names = dependency_names();
    assert!(
        names.iter().any(|name| name == "serde_json"),
        "manifest parse sanity: expected serde_json among {names:?}"
    );
    for forbidden in FORBIDDEN_DEPENDENCIES {
        assert!(
            !names.iter().any(|name| name == forbidden),
            "{forbidden} must not be a dependency of the client crate"
        );
    }
}

#[test]
fn public_api_carries_crate_level_docs() {
    assert!(
        LIB.starts_with("//!"),
        "src/lib.rs must open with crate docs"
    );
    assert!(LIB.contains("coven.daemon.v1"));
}

#[test]
fn fixtures_are_part_of_the_package() {
    let health: serde_json::Value =
        serde_json::from_str(HEALTH_FIXTURE).expect("fixtures/health.json is JSON");
    assert_eq!(health["apiVersion"], coven_client::PROTOCOL_VERSION);
    let error: serde_json::Value =
        serde_json::from_str(ERROR_FIXTURE).expect("fixtures/error.json is JSON");
    assert!(error.is_object());
    assert!(
        MANIFEST.contains("\"fixtures/**\""),
        "the manifest include list must ship the fixtures"
    );
}

#[test]
fn readme_documents_the_contract_and_policy() {
    for needle in [
        "opencoven-coven-client",
        "coven.daemon.v1",
        "pre-1.0",
        "coven_client",
    ] {
        assert!(README.contains(needle), "README must mention {needle}");
    }
}

#[test]
fn protocol_version_is_the_named_contract() {
    assert_eq!(coven_client::PROTOCOL_VERSION, "coven.daemon.v1");
}
