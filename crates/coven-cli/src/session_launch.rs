//! Shared session-launch preparation core (#401).
//!
//! `coven run` (main.rs), `coven patch` repair sessions (main.rs), and the
//! daemon's `POST /sessions` (api.rs::launch_session) all perform the same
//! launch steps: resolve the project root and cwd, validate the requested
//! harness, resolve the familiar, and build the `SessionRecord`. Before this
//! module each side carried its own copy and they had already drifted — the
//! CLI and API validated harnesses through different helpers and built
//! record literals independently.
//!
//! This module is the single implementation of those steps. Errors are typed
//! so each edge keeps its own presentation: the CLI attaches its human
//! contexts and exits non-zero; the API maps the same variants onto its
//! structured HTTP error envelope (`400 invalid_request`/`unknown_familiar`,
//! `500 familiar_lookup_failed`) without changing either contract.
//! Caller-specific UX — prompt expansion, TTY detection, stream-json,
//! detach, payload parsing — stays at the edges by design.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::{familiar_identity, harness, project, store};

/// The canonical filesystem coordinates of a launch: the project root and a
/// cwd guaranteed to live inside it.
pub struct LaunchPaths {
    pub project_root: PathBuf,
    pub cwd: PathBuf,
}

/// Which launch-path step failed, so callers can keep their own error
/// presentation for each step (CLI human contexts vs API 400 bodies).
pub enum LaunchPathError {
    /// Canonicalizing the project root failed.
    ProjectRoot(anyhow::Error),
    /// The cwd could not be resolved, or escapes the project root.
    Cwd(anyhow::Error),
}

/// Resolve the launch coordinates: canonicalize `project_root_hint` and
/// resolve `cwd` inside it (rejecting escapes). One semantics for every
/// launch path — the CLI resolves from the process cwd, the API from the
/// request's `projectRoot` field, through this same function.
pub fn resolve_launch_paths(
    project_root_hint: &Path,
    cwd: Option<&Path>,
) -> Result<LaunchPaths, LaunchPathError> {
    let project_root =
        project::canonical_project_root(project_root_hint).map_err(LaunchPathError::ProjectRoot)?;
    let cwd = project::resolve_inside_root(&project_root, cwd).map_err(LaunchPathError::Cwd)?;
    Ok(LaunchPaths { project_root, cwd })
}

/// How strictly to validate the requested harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HarnessCheck {
    /// The harness must be configured (a known adapter). Used by the daemon
    /// API: rejecting unknown harnesses up-front avoids inserting a session
    /// row for a launch that cannot possibly succeed, while a configured but
    /// uninstalled binary is still surfaced by the runtime as a structured
    /// launch failure.
    Configured,
    /// The harness must be configured *and* its executable available. Used
    /// by CLI foreground paths that are about to spawn the binary directly.
    Available,
}

/// Validate the requested harness against the configured adapter set and
/// return its command spec. Both the CLI and the API validate through this
/// function, so the supported-harness id set and the unsupported-harness
/// message cannot drift between the two surfaces.
pub fn validate_harness(
    harness_id: &str,
    check: HarnessCheck,
) -> Result<harness::HarnessCommandSpec> {
    validate_harness_specs(
        harness_id,
        check,
        harness::configured_harness_specs()?,
        harness::harness_available,
    )
}

fn validate_harness_specs(
    harness_id: &str,
    check: HarnessCheck,
    harnesses: Vec<harness::HarnessCommandSpec>,
    is_available: impl Fn(&str) -> bool,
) -> Result<harness::HarnessCommandSpec> {
    let configured_ids = harnesses
        .iter()
        .map(|harness| harness.id.as_str())
        .collect::<Vec<_>>();
    let selected = harnesses
        .iter()
        .find(|harness| harness.id == harness_id)
        .cloned();

    match selected {
        Some(harness) if check == HarnessCheck::Configured || is_available(&harness.executable) => {
            Ok(harness)
        }
        Some(harness) => Err(anyhow::anyhow!(
            "harness `{}` is not available. {}",
            harness.id,
            harness.install_hint
        )),
        None => Err(anyhow::anyhow!(
            "{}",
            harness::unsupported_harness_message(harness_id, &configured_ids)
        )),
    }
}

/// Why a familiar could not be resolved, so the API can distinguish the
/// client error (unknown id → 400) from the server error (roster unreadable
/// → 500) while the CLI folds both into its usual anyhow presentation.
pub enum FamiliarError {
    /// The id is not declared in familiars.toml. Carries the same guidance
    /// error `familiar_identity::unknown_familiar_error` always produced.
    Unknown {
        familiar_id: String,
        error: anyhow::Error,
    },
    /// Reading the roster itself failed.
    LookupFailed(anyhow::Error),
}

impl FamiliarError {
    /// Collapse into the underlying error (the CLI presentation).
    pub fn into_error(self) -> anyhow::Error {
        match self {
            FamiliarError::Unknown { error, .. } => error,
            FamiliarError::LookupFailed(error) => error,
        }
    }
}

/// Resolve an optional familiar id against the roster. `None`, empty, and
/// whitespace-only ids mean "no familiar" — the same normalization both
/// launch surfaces already applied.
pub fn resolve_familiar(
    coven_home: &Path,
    familiar_id: Option<&str>,
) -> Result<Option<harness::FamiliarContext>, FamiliarError> {
    let Some(familiar_id) = familiar_id
        .map(str::trim)
        .filter(|familiar_id| !familiar_id.is_empty())
    else {
        return Ok(None);
    };
    match familiar_identity::resolve(coven_home, familiar_id) {
        Ok(Some(context)) => Ok(Some(context)),
        Ok(None) => Err(FamiliarError::Unknown {
            familiar_id: familiar_id.to_string(),
            error: familiar_identity::unknown_familiar_error(coven_home, familiar_id),
        }),
        Err(error) => Err(FamiliarError::LookupFailed(error)),
    }
}

/// The caller-specific fields of a new session row. Everything not listed
/// here (exit code, archive state, external flag, transcript path) is a
/// fixed invariant of a fresh launch and set by [`new_session_record`].
pub struct NewSessionParams {
    pub id: String,
    pub project_root: String,
    pub harness: String,
    pub title: String,
    /// `created` on CLI foreground paths (the run loop advances it),
    /// `running` on the daemon path (the runtime launch already happened by
    /// the time the row is read back).
    pub status: String,
    /// Row creation time; also used as `updated_at`.
    pub now: String,
    pub conversation_id: Option<String>,
    pub familiar_id: Option<String>,
    pub labels: Vec<String>,
    /// `None` means the default (`private`).
    pub visibility: Option<String>,
}

/// Build the session row for a fresh (non-resume) launch. The single place
/// the launch-time record shape lives: a future `SessionRecord` field gets a
/// launch default here once, for every launch surface.
pub fn new_session_record(params: NewSessionParams) -> store::SessionRecord {
    store::SessionRecord {
        id: params.id,
        project_root: params.project_root,
        harness: params.harness,
        title: params.title,
        status: params.status,
        exit_code: None,
        archived_at: None,
        created_at: params.now.clone(),
        updated_at: params.now,
        conversation_id: params.conversation_id,
        familiar_id: params.familiar_id,
        labels: params.labels,
        visibility: params.visibility.unwrap_or_else(|| "private".to_string()),
        external: false,
        transcript_path: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_launch_paths_canonicalizes_and_rejects_escapes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("repo");
        std::fs::create_dir_all(root.join("child"))?;

        let paths = match resolve_launch_paths(&root, Some(Path::new("child"))) {
            Ok(paths) => paths,
            Err(LaunchPathError::ProjectRoot(error) | LaunchPathError::Cwd(error)) => {
                return Err(error);
            }
        };
        assert_eq!(paths.cwd, paths.project_root.join("child"));

        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside)?;
        let error = resolve_launch_paths(&root, Some(&outside));
        match error {
            Err(LaunchPathError::Cwd(error)) => {
                assert!(
                    error.to_string().contains("outside the Coven project root"),
                    "unexpected error: {error}"
                );
            }
            Err(LaunchPathError::ProjectRoot(error)) => {
                panic!("escape must be a Cwd error, got ProjectRoot: {error}")
            }
            Ok(_) => panic!("cwd outside the root must be rejected"),
        }
        Ok(())
    }

    #[test]
    fn resolve_launch_paths_distinguishes_missing_root() {
        let error = resolve_launch_paths(Path::new("/nonexistent/coven-launch-test"), None);
        assert!(
            matches!(error, Err(LaunchPathError::ProjectRoot(_))),
            "a missing root must be a ProjectRoot error"
        );
    }

    #[test]
    fn configured_harness_validation_returns_spec_without_probing_executables() -> Result<()> {
        let selected = validate_harness_specs(
            "codex",
            HarnessCheck::Configured,
            harness::configured_harness_specs()?,
            |_| panic!("configured-only validation must not probe executable availability"),
        )?;

        assert_eq!(selected.id, "codex");
        assert_eq!(selected.executable, "codex");
        assert!(selected.supports_model());
        Ok(())
    }

    #[test]
    fn resolve_familiar_normalizes_blank_ids_to_none() {
        let temp = tempfile::tempdir().expect("tempdir");
        for id in [None, Some(""), Some("   ")] {
            let resolved = resolve_familiar(temp.path(), id);
            assert!(
                matches!(resolved, Ok(None)),
                "blank familiar ids must resolve to None"
            );
        }
    }

    #[test]
    fn resolve_familiar_distinguishes_unknown_from_lookup_failure() -> Result<()> {
        let temp = tempfile::tempdir()?;
        std::fs::write(
            temp.path().join("familiars.toml"),
            "[[familiar]]\nid = \"sage\"\ndisplay_name = \"Sage\"\nrole = \"Research\"\ndescription = \"Reads.\"\n",
        )?;

        let resolved =
            resolve_familiar(temp.path(), Some("sage")).map_err(FamiliarError::into_error)?;
        assert_eq!(resolved.expect("known familiar").id, "sage");

        match resolve_familiar(temp.path(), Some("ghost")) {
            Err(FamiliarError::Unknown { familiar_id, error }) => {
                assert_eq!(familiar_id, "ghost");
                assert!(error.to_string().contains("unknown familiar `ghost`"));
                assert!(error.to_string().contains("sage"));
            }
            Err(FamiliarError::LookupFailed(error)) => {
                panic!("unknown id must not be a lookup failure: {error}")
            }
            Ok(_) => panic!("unknown familiar must not resolve"),
        }
        Ok(())
    }

    #[test]
    fn new_session_record_sets_launch_invariants() {
        let record = new_session_record(NewSessionParams {
            id: "session-1".into(),
            project_root: "/repo".into(),
            harness: "codex".into(),
            title: "Fix tests".into(),
            status: "created".into(),
            now: "2026-01-01T00:00:00Z".into(),
            conversation_id: None,
            familiar_id: Some("sage".into()),
            labels: vec!["ci".into()],
            visibility: None,
        });
        assert_eq!(record.visibility, "private");
        assert_eq!(record.created_at, record.updated_at);
        assert_eq!(record.exit_code, None);
        assert_eq!(record.archived_at, None);
        assert!(!record.external);
        assert_eq!(record.transcript_path, None);
        assert_eq!(record.familiar_id.as_deref(), Some("sage"));
        assert_eq!(record.labels, vec!["ci".to_string()]);
    }
}
