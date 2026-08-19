//! Versioned, side-effect-free reporting of Coven's resolved on-disk paths.
//!
//! This module intentionally describes locations without opening stores,
//! creating directories, or probing a daemon. It reads `familiars.toml` only
//! to resolve declared workspace roots. It is the machine-readable contract
//! used by isolated runners to prove which roots a Coven invocation would
//! consume.

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::Serialize;

use crate::{engine, harness, paths, settings, STORE_FILE_NAME};

pub const SCHEMA: &str = "coven.config.paths";
pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathsReport {
    pub schema: &'static str,
    pub version: u32,
    pub surfaces: Vec<PathSurface>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathSurface {
    pub id: &'static str,
    pub status: PathStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub paths: Vec<String>,
    pub source: PathSource,
    pub access: AccessMode,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathStatus {
    Resolved,
    NotApplicable,
    Unsupported,
    Unresolved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PathSource {
    Environment,
    Configuration,
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    ReadOnly,
}

#[derive(Debug)]
struct ProcessRoots {
    coven_home: std::result::Result<PathBuf, String>,
    coven_home_source: PathSource,
    settings_path: Option<PathBuf>,
    settings_source: PathSource,
    managed_engine_root: Option<PathBuf>,
    managed_engine_source: PathSource,
}

impl ProcessRoots {
    fn capture() -> Self {
        let managed_engine_home = dirs_next::home_dir();
        Self {
            coven_home: paths::coven_home_dir().map_err(|error| error.to_string()),
            coven_home_source: coven_home_source(),
            settings_path: settings::user_settings_path(),
            settings_source: settings_source(),
            managed_engine_source: user_home_source(managed_engine_home.as_deref()),
            managed_engine_root: managed_engine_home
                .as_deref()
                .map(paths::managed_engine_root),
        }
    }
}

/// Build the stable report for `coven config paths --json`.
///
/// Filesystem observations are limited to the engine resolver's regular-file
/// check and reading `familiars.toml` for workspace declarations. This does
/// not create state, spawn a process, or inspect workspace contents.
pub fn report() -> PathsReport {
    let roots = ProcessRoots::capture();
    let cwd = std::env::current_dir().unwrap_or_default();
    let mut surfaces = Vec::new();

    match roots.coven_home {
        Ok(home) => append_home_surfaces(&mut surfaces, &home, roots.coven_home_source, &cwd),
        Err(_) => append_unresolved_home_surfaces(&mut surfaces, &cwd),
    }

    push_optional_path(
        &mut surfaces,
        "settings.user",
        roots.settings_path.as_deref(),
        roots.settings_source,
        &cwd,
    );
    push_optional_path(
        &mut surfaces,
        "engine.managed_cache",
        roots.managed_engine_root.as_deref(),
        roots.managed_engine_source,
        &cwd,
    );
    push_engine_binary(&mut surfaces, &cwd, roots.managed_engine_source);

    PathsReport {
        schema: SCHEMA,
        version: SCHEMA_VERSION,
        surfaces,
    }
}

fn append_home_surfaces(
    surfaces: &mut Vec<PathSurface>,
    home: &Path,
    source: PathSource,
    cwd: &Path,
) {
    push_path(surfaces, "coven.home", home, source, cwd);
    push_path(
        surfaces,
        "store.session_ledger",
        &home.join(STORE_FILE_NAME),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "repositories.registry",
        &crate::repos_config::config_path(home),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "privacy.policy",
        &home.join("privacy.toml"),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "adapters.trusted_root",
        &harness::trusted_adapter_dir(home),
        source,
        cwd,
    );
    push_adapter_environment_surfaces(surfaces, cwd);
    push_path(surfaces, "state.mobile", &home.join("mobile"), source, cwd);
    push_path(
        surfaces,
        "state.sensitive_artifact_key",
        &home.join("keys").join("session-artifacts.key"),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "state.daemon_metadata",
        &crate::daemon::daemon_status_path(home),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "state.shared_lock",
        &crate::state_lock::shared_lock_path(home),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "state.reset_transaction",
        &home.join(crate::state_lock::RESET_TRANSACTION_FILE),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "state.daemon_lifecycle_lock",
        &crate::daemon::daemon_lifecycle_lock_path(home),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "state.daemon_serve_lock",
        &crate::daemon::daemon_serve_lock_path(home),
        source,
        cwd,
    );
    #[cfg(unix)]
    push_path(
        surfaces,
        "state.daemon_ipc",
        &crate::daemon::daemon_socket_path(home),
        source,
        cwd,
    );
    #[cfg(windows)]
    match crate::daemon::windows_pipe_path(home) {
        Ok(path) => push_path(surfaces, "state.daemon_ipc", &path, source, cwd),
        Err(_) => {
            push_terminal_with_source(surfaces, "state.daemon_ipc", PathStatus::Unresolved, source)
        }
    }
    push_path(
        surfaces,
        "state.familiar_manifest",
        &home.join("familiars.toml"),
        source,
        cwd,
    );
    match crate::cockpit_sources::familiar_workspaces(home) {
        Ok(paths) if !paths.is_empty() => push_paths(
            surfaces,
            "state.familiar_workspaces",
            paths.iter().map(PathBuf::as_path),
            PathSource::Configuration,
            cwd,
        ),
        Ok(_) => push_path(
            surfaces,
            "state.familiar_workspaces",
            &home.join("familiars"),
            source,
            cwd,
        ),
        Err(_) => push_terminal_with_source(
            surfaces,
            "state.familiar_workspaces",
            PathStatus::Unresolved,
            PathSource::Configuration,
        ),
    }
    push_path(surfaces, "state.skills", &home.join("skills"), source, cwd);
    push_path(
        surfaces,
        "state.coven_calls",
        &crate::coven_calls::calls_path(home),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "state.executor_node_config",
        &home.join("executor.json"),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "state.exports",
        &home.join("exports"),
        source,
        cwd,
    );
    push_path(surfaces, "state.memory", &home.join("memory"), source, cwd);
    push_path(
        surfaces,
        "state.memory_migrations",
        &home.join(crate::memory_import::MIGRATIONS_DIRECTORY),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "state.pending_proposals",
        &home.join("pending"),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "state.research",
        &home.join("research"),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "state.reset_backups",
        &home.join("reset-backups"),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "state.travel_profiles",
        &home.join("travel").join("profiles"),
        source,
        cwd,
    );
    // Coven's redacted event log and optional encrypted raw artifacts are
    // tables in the session SQLite database, not standalone `logs/` files.
    push_path(
        surfaces,
        "logs.redacted_events",
        &home.join(STORE_FILE_NAME),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "logs.daemon_recovery",
        &crate::daemon::daemon_recovery_log_path(home),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "logs.encrypted_artifacts",
        &home.join(STORE_FILE_NAME),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "dashboard.chat_settings",
        &home.join("chat-settings.json"),
        source,
        cwd,
    );
    push_path(
        surfaces,
        "dashboard.chat_conversations",
        &home.join("chat-conversations"),
        source,
        cwd,
    );
    // The optional memory dashboard is a separately installed process. Coven
    // has no owned state-path contract for it, so inventing one would turn a
    // diagnostic into a false isolation attestation.
    push_terminal(
        surfaces,
        "dashboard.memory_companion_state",
        PathStatus::Unsupported,
    );
}

fn append_unresolved_home_surfaces(surfaces: &mut Vec<PathSurface>, cwd: &Path) {
    for id in [
        "coven.home",
        "store.session_ledger",
        "repositories.registry",
        "privacy.policy",
        "adapters.trusted_root",
        "state.mobile",
        "state.sensitive_artifact_key",
        "state.daemon_metadata",
        "state.shared_lock",
        "state.reset_transaction",
        "state.daemon_lifecycle_lock",
        "state.daemon_serve_lock",
        "state.daemon_ipc",
        "state.familiar_manifest",
        "state.familiar_workspaces",
        "state.skills",
        "state.coven_calls",
        "state.executor_node_config",
        "state.exports",
        "state.memory",
        "state.memory_migrations",
        "state.pending_proposals",
        "state.research",
        "state.reset_backups",
        "state.travel_profiles",
        "logs.redacted_events",
        "logs.encrypted_artifacts",
        "logs.daemon_recovery",
        "dashboard.chat_settings",
        "dashboard.chat_conversations",
    ] {
        push_terminal(surfaces, id, PathStatus::Unresolved);
    }
    push_adapter_environment_surfaces(surfaces, cwd);
    push_terminal(
        surfaces,
        "dashboard.memory_companion_state",
        PathStatus::Unsupported,
    );
}

fn push_adapter_environment_surfaces(surfaces: &mut Vec<PathSurface>, cwd: &Path) {
    match std::env::var_os(harness::EXTERNAL_ADAPTER_MANIFEST_ENV) {
        Some(value) if !value.is_empty() => push_path(
            surfaces,
            "adapters.external_manifest",
            Path::new(&value),
            PathSource::Environment,
            cwd,
        ),
        _ => push_terminal(
            surfaces,
            "adapters.external_manifest",
            PathStatus::NotApplicable,
        ),
    }

    let paths: Vec<PathBuf> = std::env::var_os(harness::EXTERNAL_ADAPTER_DIRS_ENV)
        .filter(|value| !value.is_empty())
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default();
    if paths.is_empty() {
        push_terminal(
            surfaces,
            "adapters.external_roots",
            PathStatus::NotApplicable,
        );
    } else {
        push_paths(
            surfaces,
            "adapters.external_roots",
            paths.iter().map(PathBuf::as_path),
            PathSource::Environment,
            cwd,
        );
    }
}

fn push_engine_binary(surfaces: &mut Vec<PathSurface>, cwd: &Path, user_home_source: PathSource) {
    let Some(resolved) = engine::resolve() else {
        push_terminal(
            surfaces,
            "engine.resolved_binary",
            PathStatus::NotApplicable,
        );
        return;
    };
    let source = match resolved.source {
        engine::EngineSource::EnvOverride | engine::EngineSource::PathLookup => {
            PathSource::Environment
        }
        engine::EngineSource::Managed | engine::EngineSource::LegacyHome => user_home_source,
    };
    push_path(
        surfaces,
        "engine.resolved_binary",
        &resolved.path,
        source,
        cwd,
    );
}

fn push_optional_path(
    surfaces: &mut Vec<PathSurface>,
    id: &'static str,
    path: Option<&Path>,
    source: PathSource,
    cwd: &Path,
) {
    match path {
        Some(path) => push_path(surfaces, id, path, source, cwd),
        None => push_terminal_with_source(surfaces, id, PathStatus::Unresolved, source),
    }
}

fn push_path(
    surfaces: &mut Vec<PathSurface>,
    id: &'static str,
    path: &Path,
    source: PathSource,
    cwd: &Path,
) {
    match resolved_path_string(path, cwd) {
        Some(path) => surfaces.push(PathSurface {
            id,
            status: PathStatus::Resolved,
            path: Some(path),
            paths: Vec::new(),
            source,
            access: AccessMode::ReadOnly,
        }),
        None => push_terminal_with_source(surfaces, id, PathStatus::Unresolved, source),
    }
}

fn push_paths<'a>(
    surfaces: &mut Vec<PathSurface>,
    id: &'static str,
    paths: impl IntoIterator<Item = &'a Path>,
    source: PathSource,
    cwd: &Path,
) {
    let paths: Option<Vec<String>> = paths
        .into_iter()
        .map(|path| resolved_path_string(path, cwd))
        .collect();
    match paths {
        Some(paths) => surfaces.push(PathSurface {
            id,
            status: PathStatus::Resolved,
            path: None,
            paths,
            source,
            access: AccessMode::ReadOnly,
        }),
        None => push_terminal_with_source(surfaces, id, PathStatus::Unresolved, source),
    }
}

fn push_terminal(surfaces: &mut Vec<PathSurface>, id: &'static str, status: PathStatus) {
    push_terminal_with_source(surfaces, id, status, PathSource::Default);
}

fn push_terminal_with_source(
    surfaces: &mut Vec<PathSurface>,
    id: &'static str,
    status: PathStatus,
    source: PathSource,
) {
    surfaces.push(PathSurface {
        id,
        status,
        path: None,
        paths: Vec::new(),
        source,
        access: AccessMode::ReadOnly,
    });
}

fn absolute_path(path: &Path, cwd: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    if !cwd.is_absolute() {
        return None;
    }

    #[cfg(windows)]
    {
        use std::path::Component;

        let needs_process_resolution =
            path.has_root() || matches!(path.components().next(), Some(Component::Prefix(_)));
        if needs_process_resolution {
            // Drive-relative paths (for example `C:state`) use per-drive
            // process state that a lexical join cannot reproduce.
            std::path::absolute(path)
                .ok()
                .filter(|resolved| resolved.is_absolute())
        } else {
            Some(cwd.join(path))
        }
    }
    #[cfg(not(windows))]
    {
        Some(cwd.join(path))
    }
}

fn resolved_path_string(path: &Path, cwd: &Path) -> Option<String> {
    absolute_path(path, cwd)?.to_str().map(ToOwned::to_owned)
}

fn nonempty_env(name: &str) -> bool {
    std::env::var_os(name).is_some_and(|value| !value.is_empty())
}

fn coven_home_source() -> PathSource {
    if nonempty_env("COVEN_HOME")
        || nonempty_env("HOME")
        || nonempty_env("USERPROFILE")
        || (nonempty_env("HOMEDRIVE") && nonempty_env("HOMEPATH"))
    {
        PathSource::Environment
    } else {
        PathSource::Default
    }
}

fn settings_source() -> PathSource {
    if nonempty_env("XDG_CONFIG_HOME") || nonempty_env("HOME") {
        PathSource::Environment
    } else {
        PathSource::Default
    }
}

fn user_home_source(home: Option<&Path>) -> PathSource {
    let environment_homes = ["HOME", "USERPROFILE"]
        .into_iter()
        .filter_map(std::env::var_os)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let drive_and_path = std::env::var_os("HOMEDRIVE")
        .filter(|value| !value.is_empty())
        .zip(std::env::var_os("HOMEPATH").filter(|value| !value.is_empty()))
        .map(|(mut drive, path)| {
            drive.push(path);
            PathBuf::from(drive)
        });
    user_home_source_from_candidates(
        home,
        !cfg!(windows),
        environment_homes.chain(drive_and_path),
    )
}

fn user_home_source_from_candidates(
    home: Option<&Path>,
    resolver_uses_environment: bool,
    candidates: impl IntoIterator<Item = PathBuf>,
) -> PathSource {
    let Some(home) = home.filter(|_| resolver_uses_environment) else {
        return PathSource::Default;
    };
    if candidates.into_iter().any(|candidate| candidate == home) {
        PathSource::Environment
    } else {
        PathSource::Default
    }
}

pub fn print_json() -> Result<()> {
    println!("{}", serde_json::to_string_pretty(&report())?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_path_keeps_absolute_paths_and_resolves_relative_paths_lexically() {
        let cwd = std::env::current_dir().expect("absolute process directory");
        let resolved = absolute_path(Path::new("relative/state"), &cwd)
            .expect("resolve relative path against process directory");
        let absolute = cwd.join("absolute/state");

        assert!(resolved.is_absolute());
        assert!(resolved.ends_with(Path::new("relative/state")));
        assert_eq!(
            absolute_path(&absolute, &cwd),
            Some(absolute),
            "absolute paths must remain unchanged"
        );
    }

    #[test]
    fn push_path_fails_closed_without_an_absolute_working_directory() {
        let mut surfaces = Vec::new();

        push_path(
            &mut surfaces,
            "test.relative",
            Path::new("relative/state"),
            PathSource::Environment,
            Path::new("."),
        );

        assert_eq!(surfaces.len(), 1);
        assert!(matches!(surfaces[0].status, PathStatus::Unresolved));
        assert!(surfaces[0].path.is_none());
        assert!(matches!(surfaces[0].source, PathSource::Environment));
    }

    #[test]
    fn push_optional_path_preserves_the_unresolved_source() {
        let mut surfaces = Vec::new();

        push_optional_path(
            &mut surfaces,
            "test.optional",
            None,
            PathSource::Environment,
            Path::new("/tmp"),
        );

        assert_eq!(surfaces.len(), 1);
        assert!(matches!(surfaces[0].status, PathStatus::Unresolved));
        assert!(matches!(surfaces[0].source, PathSource::Environment));
    }

    #[test]
    fn native_profile_home_is_default_even_when_environment_matches() {
        let home = Path::new("/native-profile");

        assert_eq!(
            user_home_source_from_candidates(Some(home), false, [home.to_path_buf()].into_iter()),
            PathSource::Default
        );
    }

    #[cfg(unix)]
    #[test]
    fn push_path_fails_closed_for_non_unicode_paths() {
        use std::os::unix::ffi::OsStringExt;

        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/coven-\xFF".to_vec()));
        let mut surfaces = Vec::new();

        push_path(
            &mut surfaces,
            "test.non_unicode",
            &path,
            PathSource::Configuration,
            Path::new("/tmp"),
        );

        assert_eq!(surfaces.len(), 1);
        assert!(matches!(surfaces[0].status, PathStatus::Unresolved));
        assert!(surfaces[0].path.is_none());
        assert!(matches!(surfaces[0].source, PathSource::Configuration));
    }

    #[cfg(unix)]
    #[test]
    fn push_paths_fails_closed_without_partial_output() {
        use std::os::unix::ffi::OsStringExt;

        let valid = PathBuf::from("/tmp/coven-valid");
        let invalid = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/coven-\xFF".to_vec()));
        let mut surfaces = Vec::new();

        push_paths(
            &mut surfaces,
            "test.non_unicode_array",
            [&valid, &invalid].into_iter().map(PathBuf::as_path),
            PathSource::Environment,
            Path::new("/tmp"),
        );

        assert_eq!(surfaces.len(), 1);
        assert!(matches!(surfaces[0].status, PathStatus::Unresolved));
        assert!(surfaces[0].paths.is_empty());
        assert!(matches!(surfaces[0].source, PathSource::Environment));
    }

    #[cfg(windows)]
    #[test]
    fn absolute_path_resolves_drive_relative_paths() {
        use std::path::{Component, Prefix};

        let resolved = absolute_path(Path::new(r"C:relative\state"), Path::new(r"C:\workspace"))
            .expect("resolve drive-relative path");

        assert!(
            resolved.is_absolute(),
            "drive-relative path remained relative: {}",
            resolved.display()
        );
        assert!(matches!(
            resolved.components().next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(b'C'))
        ));
    }

    #[cfg(windows)]
    #[test]
    fn absolute_path_resolves_plain_relative_paths_against_captured_cwd() {
        let cwd = Path::new(r"C:\workspace");

        assert_eq!(
            absolute_path(Path::new(r"relative\state"), cwd),
            Some(PathBuf::from(r"C:\workspace\relative\state"))
        );
    }

    #[test]
    fn report_has_stable_schema_and_terminal_rows() {
        let report = report();
        assert_eq!(report.schema, SCHEMA);
        assert_eq!(report.version, SCHEMA_VERSION);
        assert!(report
            .surfaces
            .iter()
            .any(|surface| surface.id == "coven.home"));
        assert!(report
            .surfaces
            .iter()
            .any(|surface| surface.id == "dashboard.memory_companion_state"));
        assert!(report
            .surfaces
            .iter()
            .all(|surface| matches!(surface.access, AccessMode::ReadOnly)));
    }
}
