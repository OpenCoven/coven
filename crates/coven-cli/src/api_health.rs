//! Health response types and capability mapping for the daemon API.
//!
//! This module maps the daemon's supported contracts and request authority
//! into the stable health payload. Store, hub, and event-writer collection
//! remain with the API route orchestrator because they perform I/O.

use serde::{Deserialize, Serialize};

use crate::{daemon::DaemonStatus, request_authority::RequestAuthority, store};

pub const COVEN_API_NAMED_VERSION: &str = "coven.daemon.v1";

/// The build identity reported as `covenVersion` in the health body.
///
/// Deliberately **not** `CARGO_PKG_VERSION`: `crates/coven-cli/Cargo.toml`
/// carries a permanent `0.0.0` placeholder because Coven's real version comes
/// from the release tag, not the manifest. Reading the manifest made every
/// build report `covenVersion: "0.0.0"` while `coven --version` printed the
/// true version from `COVEN_VERSION_DESC` — the same build-script stamp this
/// now reads, so the two can no longer disagree.
pub fn coven_version() -> &'static str {
    build_identity(env!("COVEN_VERSION_DESC"))
}

/// Normalize a build descriptor into the `covenVersion` value.
///
/// The descriptor is `git describe --tags --always --dirty` output, an
/// explicit release stamp, or the build script's no-git fallback. Distance,
/// commit, and `-dirty` suffixes are kept — they *are* the build identity —
/// while the tag's `v` prefix is dropped so the field stays a version string.
/// When nothing resolved a version, report `unknown` instead of republishing
/// the `0.0.0` placeholder as though it were a real release.
fn build_identity(descriptor: &str) -> &str {
    let trimmed = descriptor.trim();
    let sourceless = trimmed
        .strip_suffix("(unknown source)")
        .unwrap_or(trimmed)
        .trim_end();
    let version = sourceless.strip_prefix('v').unwrap_or(sourceless);
    if version.is_empty() || version == "0.0.0" {
        "unknown"
    } else {
        version
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCapabilities {
    pub sessions: bool,
    pub events: bool,
    pub travel: bool,
    pub scheduler: bool,
    pub hub: bool,
    pub executor_dispatch: bool,
    pub event_cursor: String,
    pub structured_errors: bool,
    pub session_handoff: bool,
    /// Whether `POST /sessions` accepts the exact, fail-closed
    /// `launchPolicy` contract documented for unattended Codex work.
    #[serde(default)]
    pub session_launch_policy: bool,
    /// Whether the `afs.*` route family is served at all.
    pub afs: bool,
    /// Mount backend, or `false` when none is available. A client must branch
    /// on this rather than assume mounting works: SDK-only operation is a
    /// supported mode, not a degraded one.
    pub afs_mount: MountCapability,
    /// Whether the daemon can materialize a delta into a git branch.
    pub afs_commit: bool,
    /// Whether `afs.session.commit` accepts the side-effect-free `dryRun`
    /// contract. Clients must not infer this from `afsCommit`: older daemons
    /// accepted commit requests before preview semantics existed.
    #[serde(default)]
    pub afs_commit_dry_run: bool,
    /// Exact execution-binding contracts accepted by bound session
    /// launch/input/kill. Additive: absent/older wire payloads default to
    /// empty rather than failing deserialization.
    #[serde(default)]
    pub execution_binding_contracts: Vec<String>,
    /// Exact request-adoption contracts accepted by dedicated adopted
    /// launch/input routes. Additive: absent/older wire payloads default to
    /// empty rather than failing deserialization.
    #[serde(default)]
    pub request_adoption_contracts: Vec<String>,
    /// Refusal-only admission contracts, advertised only over owner-local IPC.
    #[serde(default)]
    pub session_policy_contracts: Vec<String>,
}

/// `afsMount`: a backend name, or `false`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MountCapability {
    Backend(String),
    Unavailable(bool),
}

impl MountCapability {
    /// What this daemon can actually mount.
    ///
    /// `false` on every platform and build without a backend, and `false` by
    /// default even where one exists: the NFS export serves a single delta
    /// rather than the merged base+delta view DESIGN.md §3.2 specifies (bead
    /// `coven-vlw`), and an agent process could not write through the mount on
    /// macOS (bead `coven-x77`). Advertising a backend before those close
    /// would promise something the daemon cannot deliver, so the opt-in in
    /// `afs_mount` gates it.
    pub fn detect() -> Self {
        match crate::afs_mount::backend() {
            Some(backend) => Self::Backend(backend.to_string()),
            None => Self::Unavailable(false),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubHealth {
    pub role: String,
    pub hub_id: String,
    pub nodes_total: usize,
    pub nodes_available: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
    pub api_version: String,
    pub coven_version: String,
    pub capabilities: HealthCapabilities,
    pub daemon: Option<DaemonStatus>,
    /// Hub control-plane summary (role + node availability). `None` when the
    /// response is built without store access (e.g. CLI status printing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hub: Option<HubHealth>,
    /// Daemon-owned event persistence health. Omitted for status rendering
    /// paths that do not have a live runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_writer: Option<crate::event_writer::EventWriterHealth>,
    /// Local SQLite pressure and bounded-maintenance state. This remains
    /// present when collection fails so health consumers can distinguish a
    /// storage problem from a daemon that is simply not running.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage: Option<store::StorageHealth>,
}

pub fn health_response(daemon: Option<DaemonStatus>) -> HealthResponse {
    health_response_for_authority(daemon, RequestAuthority::OwnerLocalIpc)
}

pub(crate) fn health_response_for_authority(
    daemon: Option<DaemonStatus>,
    authority: RequestAuthority,
) -> HealthResponse {
    HealthResponse {
        ok: true,
        api_version: COVEN_API_NAMED_VERSION.to_string(),
        coven_version: coven_version().to_string(),
        capabilities: HealthCapabilities {
            sessions: true,
            events: true,
            travel: true,
            scheduler: true,
            hub: true,
            executor_dispatch: true,
            event_cursor: "sequence".to_string(),
            structured_errors: true,
            session_handoff: true,
            session_launch_policy: authority.allows_session_launch_policy(),
            afs: true,
            afs_mount: MountCapability::detect(),
            afs_commit: true,
            afs_commit_dry_run: true,
            execution_binding_contracts: vec![crate::execution_binding::CONTRACT.to_string()],
            request_adoption_contracts: vec![crate::request_adoption::CONTRACT.to_string()],
            session_policy_contracts: if authority.allows_session_launch_policy() {
                vec![crate::session_policy::CONTRACT.to_string()]
            } else {
                Vec::new()
            },
        },
        daemon,
        hub: None,
        event_writer: None,
        storage: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Regression for `covenVersion: "0.0.0"`: the health body must report the
    /// build stamp `coven --version` prints, not the `0.0.0` placeholder that
    /// `crates/coven-cli/Cargo.toml` carries permanently.
    #[test]
    fn health_version_is_never_the_manifest_placeholder() {
        assert_ne!(coven_version(), "0.0.0");
        assert_ne!(coven_version(), env!("CARGO_PKG_VERSION"));
        assert!(!coven_version().is_empty());
        assert_eq!(health_response(None).coven_version, coven_version());
    }

    /// Tag prefixes are dropped, `git describe` build identity is kept, and an
    /// unresolvable version says so instead of claiming `0.0.0`.
    #[test]
    fn build_identity_normalizes_every_descriptor_shape() {
        assert_eq!(build_identity("v0.4.2"), "0.4.2");
        assert_eq!(build_identity("0.4.2"), "0.4.2");
        assert_eq!(build_identity("  v0.4.2\n"), "0.4.2");
        assert_eq!(
            build_identity("v0.4.2-14-g3eb633b2-dirty"),
            "0.4.2-14-g3eb633b2-dirty"
        );
        // `git describe --always` with no tags in a shallow clone.
        assert_eq!(build_identity("3eb633b2"), "3eb633b2");
        // The build script's no-git fallback interpolates the placeholder.
        assert_eq!(build_identity("0.0.0 (unknown source)"), "unknown");
        assert_eq!(build_identity("0.0.0"), "unknown");
        assert_eq!(build_identity(""), "unknown");
        assert_eq!(build_identity("   "), "unknown");
        // A real manifest version behind the no-git fallback still resolves.
        assert_eq!(build_identity("0.4.2 (unknown source)"), "0.4.2");
    }

    #[test]
    fn builds_owner_local_health_response() {
        let response = health_response(None);

        assert!(response.ok);
        assert_eq!(response.api_version, COVEN_API_NAMED_VERSION);
        assert_eq!(response.coven_version, coven_version());
        assert!(response.capabilities.sessions);
        assert!(response.capabilities.events);
        assert!(response.capabilities.travel);
        assert!(response.capabilities.scheduler);
        assert!(response.capabilities.hub);
        assert!(response.capabilities.executor_dispatch);
        assert_eq!(response.capabilities.event_cursor, "sequence");
        assert!(response.capabilities.structured_errors);
        assert!(response.capabilities.session_launch_policy);
        assert_eq!(response.daemon, None);
        assert_eq!(response.hub, None);
        assert_eq!(response.event_writer, None);
        assert_eq!(response.storage, None);
    }

    #[test]
    fn session_policy_contract_is_owner_local_only_and_other_health_is_unchanged() {
        let owner = serde_json::to_value(health_response_for_authority(
            None,
            RequestAuthority::OwnerLocalIpc,
        ))
        .unwrap();
        let mut tcp =
            serde_json::to_value(health_response_for_authority(None, RequestAuthority::Tcp))
                .unwrap();
        assert_eq!(
            owner["capabilities"]["sessionPolicyContracts"],
            json!(["coven.session-policy.v1"])
        );
        assert_eq!(tcp["capabilities"]["sessionPolicyContracts"], json!([]));
        tcp["capabilities"]["sessionLaunchPolicy"] = json!(true);
        tcp["capabilities"]["sessionPolicyContracts"] =
            owner["capabilities"]["sessionPolicyContracts"].clone();
        assert_eq!(owner, tcp);
    }

    #[test]
    fn authority_changes_only_the_owner_gated_launch_policy() {
        let owner_local = health_response_for_authority(None, RequestAuthority::OwnerLocalIpc);
        let tcp = health_response_for_authority(None, RequestAuthority::Tcp);
        let mut expected_tcp = owner_local.clone();
        expected_tcp.capabilities.session_launch_policy = false;
        expected_tcp.capabilities.session_policy_contracts.clear();

        assert_eq!(tcp, expected_tcp);
        assert_eq!(
            serde_json::to_value(owner_local).expect("serialize owner-local health")
                ["capabilities"]["sessionLaunchPolicy"],
            true
        );
        assert_eq!(
            serde_json::to_value(tcp).expect("serialize TCP health")["capabilities"]
                ["sessionLaunchPolicy"],
            false
        );
    }

    #[test]
    fn older_health_payloads_default_additive_fields() -> anyhow::Result<()> {
        let mut payload = serde_json::to_value(health_response(None))?;
        let capabilities = payload["capabilities"]
            .as_object_mut()
            .expect("capabilities object");
        capabilities.remove("afsCommitDryRun");
        capabilities.remove("sessionLaunchPolicy");
        capabilities.remove("executionBindingContracts");
        capabilities.remove("requestAdoptionContracts");
        capabilities.remove("sessionPolicyContracts");

        let decoded: HealthResponse = serde_json::from_value(payload)?;
        assert!(!decoded.capabilities.afs_commit_dry_run);
        assert!(!decoded.capabilities.session_launch_policy);
        assert!(decoded.capabilities.execution_binding_contracts.is_empty());
        assert!(decoded.capabilities.request_adoption_contracts.is_empty());
        assert!(decoded.capabilities.session_policy_contracts.is_empty());
        Ok(())
    }

    #[test]
    fn health_advertises_current_additive_contracts() -> anyhow::Result<()> {
        let payload = serde_json::to_value(health_response(None))?;

        assert_eq!(
            payload["capabilities"]["requestAdoptionContracts"],
            json!([crate::request_adoption::CONTRACT])
        );
        assert_eq!(
            payload["capabilities"]["executionBindingContracts"],
            json!([crate::execution_binding::CONTRACT])
        );
        assert!(
            payload["daemon"].is_null(),
            "daemon metadata must retain its null variant"
        );
        Ok(())
    }
}
