//! Health response types and capability mapping for the daemon API.
//!
//! This module maps the daemon's supported contracts and request authority
//! into the stable health payload. Store, hub, and event-writer collection
//! remain with the API route orchestrator because they perform I/O.

use serde::{Deserialize, Serialize};

use crate::{daemon::DaemonStatus, request_authority::RequestAuthority, store};

pub const COVEN_API_NAMED_VERSION: &str = "coven.daemon.v1";
pub const COVEN_VERSION: &str = env!("CARGO_PKG_VERSION");

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
        coven_version: COVEN_VERSION.to_string(),
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

    #[test]
    fn builds_owner_local_health_response() {
        let response = health_response(None);

        assert!(response.ok);
        assert_eq!(response.api_version, COVEN_API_NAMED_VERSION);
        assert_eq!(response.coven_version, COVEN_VERSION);
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
    fn authority_changes_only_the_owner_gated_launch_policy() {
        let owner_local = health_response_for_authority(None, RequestAuthority::OwnerLocalIpc);
        let tcp = health_response_for_authority(None, RequestAuthority::Tcp);
        let mut expected_tcp = owner_local.clone();
        expected_tcp.capabilities.session_launch_policy = false;

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

        let decoded: HealthResponse = serde_json::from_value(payload)?;
        assert!(!decoded.capabilities.afs_commit_dry_run);
        assert!(!decoded.capabilities.session_launch_policy);
        assert!(decoded.capabilities.execution_binding_contracts.is_empty());
        assert!(decoded.capabilities.request_adoption_contracts.is_empty());
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
