use std::{
    borrow::Cow,
    collections::{BTreeMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use anyhow::{Context, Result};
use base64::Engine;
use chrono::{Duration, SecondsFormat, Utc};
use flate2::{write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    control_plane,
    daemon::DaemonStatus,
    encrypted_artifacts::SensitiveArtifactStore,
    harness::{ConversationHint, HarnessLaunchMode},
    privacy, project, store, ward,
};

const MAX_EVENTS_LIMIT: i64 = 1_000;
pub const COVEN_API_VERSION: &str = "v1";
pub const COVEN_API_NAMED_VERSION: &str = "coven.daemon.v1";
pub const COVEN_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SUPPORTED_API_VERSIONS: [&str; 1] = [COVEN_API_VERSION];

fn proposal_decision_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProposalDecisionFailpoint {
    ClaimBeforeValidation,
    ApplyBeforeAudit,
    AuditBeforeCleanup,
}

#[cfg(test)]
fn proposal_decision_failpoint(
) -> &'static Mutex<std::collections::HashMap<String, ProposalDecisionFailpoint>> {
    static FAILPOINTS: OnceLock<
        Mutex<std::collections::HashMap<String, ProposalDecisionFailpoint>>,
    > = OnceLock::new();
    FAILPOINTS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

#[cfg(test)]
fn set_proposal_decision_failpoint(failpoint: Option<(ProposalDecisionFailpoint, String)>) {
    let mut failpoints = proposal_decision_failpoint()
        .lock()
        .expect("proposal decision failpoint lock poisoned");
    match failpoint {
        Some((checkpoint, proposal_id)) => {
            failpoints.insert(proposal_id, checkpoint);
        }
        None => failpoints.clear(),
    }
}

#[cfg(test)]
fn forced_recovery_ward_refusals() -> &'static Mutex<HashSet<String>> {
    static REFUSALS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    REFUSALS.get_or_init(|| Mutex::new(HashSet::new()))
}

#[cfg(test)]
fn force_recovery_ward_refusal(proposal_id: impl Into<String>) {
    forced_recovery_ward_refusals()
        .lock()
        .expect("recovery refusal test hook lock poisoned")
        .insert(proposal_id.into());
}

fn recovery_authorization(
    proposal_id: &str,
    authorization: &ward::Authorization,
) -> ward::Authorization {
    #[cfg(test)]
    if forced_recovery_ward_refusals()
        .lock()
        .expect("recovery refusal test hook lock poisoned")
        .remove(proposal_id)
    {
        return ward::Authorization::unsigned();
    }
    #[cfg(not(test))]
    let _ = proposal_id;
    authorization.clone()
}

fn maybe_fail_proposal_decision(
    checkpoint: ProposalDecisionFailpoint,
    proposal_id: &str,
) -> Result<()> {
    #[cfg(test)]
    {
        let mut failpoints = proposal_decision_failpoint()
            .lock()
            .expect("proposal decision failpoint lock poisoned");
        if failpoints.get(proposal_id) == Some(&checkpoint) {
            failpoints.remove(proposal_id);
            anyhow::bail!("injected proposal decision interruption at {checkpoint:?}");
        }
    }
    #[cfg(not(test))]
    let _ = (checkpoint, proposal_id);
    Ok(())
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventCursor {
    pub after_seq: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsResponse {
    pub events: Vec<store::EventRecord>,
    pub next_cursor: Option<EventCursor>,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiResponse {
    pub status: u16,
    pub content_type: &'static str,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionLaunch {
    pub id: String,
    pub project_root: String,
    pub cwd: String,
    pub harness: String,
    /// Optional namespaced model id (for example `openai/gpt-5.6-sol`). The
    /// runtime keeps the provider prefix here and lets the harness adapter
    /// normalize it for the underlying CLI.
    pub model: Option<String>,
    pub launch_mode: HarnessLaunchMode,
    pub prompt: String,
    pub title: String,
    pub conversation: Option<ConversationHint>,
    /// Caller-supplied id used to group this session with other turns of the
    /// same chat conversation in `/sessions`. Independent of the harness
    /// CLI's own session-resume mechanism (see `ConversationHint`); the
    /// chat layer typically passes a chat-generated UUID stable for the
    /// lifetime of the conversation. `None` = ungrouped (one-off run).
    pub conversation_id: Option<String>,
    /// Optional familiar id (e.g. `"charm"`) whose identity should be injected
    /// into the harness invocation. The daemon resolves this to a `FamiliarContext`
    /// using the local familiars config and passes it to the harness arg builder.
    /// `None` = no identity injection (backwards-compatible default).
    pub familiar_id: Option<String>,
    /// Optional id of the familiar that spawned this session (i.e. the caller in
    /// a `sessions_spawn` / `sessions_send` delegation). When set alongside
    /// `familiar_id`, the daemon records the delegation in `cave-coven-calls.json`
    /// so the Coven Calls graph in coven-cave has data to render.
    /// `None` = direct user launch, not a delegation.
    pub caller_familiar_id: Option<String>,
}

pub trait SessionRuntime {
    fn launch_session(&self, launch: &SessionLaunch) -> Result<()>;
    fn send_input(&self, session_id: &str, payload: &Value) -> Result<()>;
    fn kill_session(&self, session_id: &str) -> Result<()>;
}

pub struct NoopSessionRuntime;

impl SessionRuntime for NoopSessionRuntime {
    fn launch_session(&self, _launch: &SessionLaunch) -> Result<()> {
        Ok(())
    }

    fn send_input(&self, _session_id: &str, _payload: &Value) -> Result<()> {
        Ok(())
    }

    fn kill_session(&self, _session_id: &str) -> Result<()> {
        Ok(())
    }
}

pub fn health_response(daemon: Option<DaemonStatus>) -> HealthResponse {
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
        },
        daemon,
        hub: None,
    }
}

fn health_response_with_hub(coven_home: &Path, daemon: Option<DaemonStatus>) -> HealthResponse {
    let mut response = health_response(daemon);
    if let Ok(summary) = crate::hub::hub_health_summary(coven_home) {
        response.hub = serde_json::from_value(summary).ok();
    }
    response
}

#[allow(dead_code)]
pub fn handle_request(
    method: &str,
    path: &str,
    coven_home: &Path,
    daemon: Option<DaemonStatus>,
) -> Result<ApiResponse> {
    handle_request_with_body(method, path, coven_home, daemon, None)
}

pub fn handle_request_with_body(
    method: &str,
    path: &str,
    coven_home: &Path,
    daemon: Option<DaemonStatus>,
    body: Option<&str>,
) -> Result<ApiResponse> {
    handle_request_with_runtime(method, path, coven_home, daemon, body, &NoopSessionRuntime)
}

pub fn handle_request_with_runtime(
    method: &str,
    path: &str,
    coven_home: &Path,
    daemon: Option<DaemonStatus>,
    body: Option<&str>,
    runtime: &dyn SessionRuntime,
) -> Result<ApiResponse> {
    let (route, query) = split_path_query(path);
    let route = match normalize_api_route(route) {
        ApiRoute::Route(route) => route,
        ApiRoute::Unsupported(version) => {
            return api_error(
                404,
                "invalid_request",
                "Unsupported API version.",
                Some(json!({
                    "apiVersion": version,
                    "supportedApiVersions": SUPPORTED_API_VERSIONS,
                })),
            );
        }
        ApiRoute::Malformed => {
            return api_error(404, "not_found", "Route not found.", None);
        }
    };
    match (method, route.as_ref()) {
        ("GET", "/api-version") => json_response(
            200,
            &json!({
                "apiVersion": COVEN_API_VERSION,
                "supportedApiVersions": SUPPORTED_API_VERSIONS,
            }),
        ),
        ("GET", "/health") => json_response(200, &health_response_with_hub(coven_home, daemon)),
        ("GET", "/capabilities") => json_response(200, &control_plane::capabilities()),
        ("GET", "/overview") => overview_response(coven_home),
        ("POST", "/actions") => {
            let payload = match parse_body(body) {
                Ok(payload) => payload,
                Err(error) => {
                    return json_response(
                        400,
                        &control_plane::rejected_action("(unknown)", error.to_string()),
                    );
                }
            };
            let (status, response) = control_plane::route_action(payload);
            json_response(status, &response)
        }
        ("POST", "/cast") => submit_cast(coven_home, body),
        ("GET", "/cast-codes") => cast_codes_response(),
        // Filesystem-backed reads under ~/.coven/. Missing files return [].
        ("GET", "/familiars") => {
            json_response(200, &crate::cockpit_sources::read_familiars(coven_home)?)
        }
        ("PUT", path) if path.starts_with("/familiars/") && path.ends_with("/icon") => {
            let id = path
                .trim_start_matches("/familiars/")
                .trim_end_matches("/icon");
            update_familiar_icon(coven_home, id, body)
        }
        // The declared Ward surface for one familiar — the read-side twin of
        // the `/edits` write path below (same config, same fail-closed 404s).
        ("GET", path) if path.starts_with("/familiars/") && path.ends_with("/ward") => {
            let id = path
                .trim_start_matches("/familiars/")
                .trim_end_matches("/ward");
            familiar_ward_response(coven_home, id)
        }
        // The Ward-enforced write path into a familiar home. Every write is
        // adjudicated by `ward::Ward::apply` (Gates 1–2, fail-closed, audited).
        ("POST", path) if path.starts_with("/familiars/") && path.ends_with("/edits") => {
            let id = path
                .trim_start_matches("/familiars/")
                .trim_end_matches("/edits");
            apply_familiar_edits(coven_home, id, body)
        }
        ("GET", "/threads/weaves") => threads_weaves_response(coven_home),
        ("GET", "/threads/proposals") => threads_proposals_response(coven_home, None),
        ("GET", path) if path.starts_with("/threads/proposals/") => {
            let id = path.trim_start_matches("/threads/proposals/");
            if Uuid::parse_str(id).is_err() {
                api_error(
                    400,
                    "invalid_request",
                    "Proposal id must be a UUID.",
                    Some(serde_json::json!({ "id": id })),
                )
            } else {
                threads_proposals_response(coven_home, Some(id))
            }
        }
        ("POST", path) if path.starts_with("/threads/proposals/") && path.ends_with("/approve") => {
            let id = path
                .trim_start_matches("/threads/proposals/")
                .trim_end_matches("/approve");
            decide_threads_proposal(coven_home, id, "approve", body)
        }
        ("POST", path) if path.starts_with("/threads/proposals/") && path.ends_with("/reject") => {
            let id = path
                .trim_start_matches("/threads/proposals/")
                .trim_end_matches("/reject");
            decide_threads_proposal(coven_home, id, "reject", body)
        }
        ("GET", "/skills") => json_response(200, &crate::cockpit_sources::scan_skills(coven_home)?),
        ("GET", p) if p.starts_with("/skills/eval-loop/") && !p.ends_with("/run") => {
            let familiar_id = p.trim_start_matches("/skills/eval-loop/");
            match crate::eval_loop::get_eval_loop_state(coven_home, familiar_id)? {
                Some(state) => {
                    json_response(200, &serde_json::json!({ "ok": true, "state": state }))
                }
                None => api_error(
                    404,
                    "skill_not_active",
                    "eval-loop skill is not active for this familiar.",
                    Some(serde_json::json!({ "familiarId": familiar_id })),
                ),
            }
        }
        ("POST", p) if p.starts_with("/skills/eval-loop/") && p.ends_with("/run") => {
            let familiar_id = p
                .trim_start_matches("/skills/eval-loop/")
                .trim_end_matches("/run");
            let track = body
                .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                .and_then(|v| v.get("track").and_then(|t| t.as_str()).map(str::to_string))
                .unwrap_or_else(|| "synthesis".to_string());
            match crate::eval_loop::enqueue_run(coven_home, familiar_id, &track) {
                Ok(spec) => json_response(
                    202,
                    &serde_json::json!({ "ok": true, "runId": spec.run_id, "track": spec.track }),
                ),
                Err(err) => {
                    let msg = err.to_string();
                    if msg.contains("already in progress") {
                        api_error(
                            409,
                            "run_in_progress",
                            &msg,
                            Some(serde_json::json!({ "familiarId": familiar_id })),
                        )
                    } else if msg.contains("track must be") {
                        api_error(400, "invalid_request", &msg, None)
                    } else {
                        Err(err)
                    }
                }
            }
        }
        ("DELETE", p) if p.starts_with("/skills/eval-loop/") && p.ends_with("/run-lock") => {
            let familiar_id = p
                .trim_start_matches("/skills/eval-loop/")
                .trim_end_matches("/run-lock");
            let force = body
                .and_then(|b| serde_json::from_str::<serde_json::Value>(b).ok())
                .and_then(|v| v.get("force").and_then(|force| force.as_bool()))
                .unwrap_or(false);
            match crate::eval_loop::clear_eval_loop_lock(coven_home, familiar_id, force) {
                Ok(cleared) => json_response(
                    200,
                    &serde_json::json!({
                        "ok": true,
                        "cleared": cleared,
                        "familiarId": familiar_id,
                    }),
                ),
                Err(err) => {
                    let msg = err.to_string();
                    if msg.contains("not stale") {
                        api_error(
                            409,
                            "lock_not_stale",
                            &msg,
                            Some(serde_json::json!({ "familiarId": familiar_id })),
                        )
                    } else {
                        Err(err)
                    }
                }
            }
        }
        // Harness-native capability manifests. The bare `/capabilities` path is
        // the control-plane catalog (matched above), so the aggregate lives at
        // the reserved `harnesses` segment.
        ("GET", "/capabilities/harnesses") => {
            let refresh = query.is_some_and(|q| query_param(q, "refresh") == Some("1"));
            json_response(200, &crate::capabilities::get_all(coven_home, refresh))
        }
        ("GET", p) if p.starts_with("/capabilities/") => {
            let harness_id = p.trim_start_matches("/capabilities/");
            let refresh = query.is_some_and(|q| query_param(q, "refresh") == Some("1"));
            match crate::capabilities::get_one(coven_home, harness_id, refresh) {
                Some(m) => json_response(200, &m),
                None => api_error(
                    404,
                    "harness_not_found",
                    "Harness id is not a known capability scan target.",
                    Some(serde_json::json!({ "harnessId": harness_id })),
                ),
            }
        }
        // Coven Calls delegation ledger
        ("GET", "/coven-calls") => {
            let calls = crate::coven_calls::load_calls(coven_home)?;
            json_response(200, &serde_json::json!({ "ok": true, "calls": calls }))
        }
        ("GET", path) if path.starts_with("/coven-calls/") => {
            let call_id = path.trim_start_matches("/coven-calls/");
            let calls = crate::coven_calls::load_calls(coven_home)?;
            match calls.into_iter().find(|c| c.id == call_id) {
                Some(call) => json_response(200, &serde_json::json!({ "ok": true, "call": call })),
                None => api_error(
                    404,
                    "call_not_found",
                    "Coven call was not found.",
                    Some(serde_json::json!({ "callId": call_id })),
                ),
            }
        }

        ("GET", "/memory") => json_response(200, &crate::cockpit_sources::scan_memory(coven_home)?),
        ("GET", "/research") => {
            json_response(200, &crate::cockpit_sources::read_research(coven_home)?)
        }
        ("POST", "/store/vacuum") => vacuum_store(coven_home),
        ("POST", "/travel/profiles") => generate_travel_profile(coven_home, body),
        ("POST", "/travel/deltas") => {
            let q = query.unwrap_or_default();
            upload_travel_delta(coven_home, body, q)
        }
        ("GET", "/travel/state") => {
            let q = query.unwrap_or_default();
            travel_state(coven_home, q)
        }
        ("POST", "/scheduler/decisions") => scheduler_decision(coven_home, body),
        ("POST", "/scheduler/redispatch") => scheduler_redispatch(coven_home, body),
        ("GET", "/hub/status") => crate::hub::hub_status(coven_home),
        ("POST", "/hub/nodes") => crate::hub::register_node(coven_home, body),
        ("GET", "/hub/nodes") => crate::hub::list_nodes(coven_home),
        ("POST", path) if path.starts_with("/hub/nodes/") && path.ends_with("/health") => {
            let node_id = path
                .trim_start_matches("/hub/nodes/")
                .trim_end_matches("/health");
            crate::hub::report_node_health(coven_home, node_id, body)
        }
        ("POST", path) if path.starts_with("/hub/nodes/") && path.ends_with("/poll") => {
            let node_id = path
                .trim_start_matches("/hub/nodes/")
                .trim_end_matches("/poll");
            crate::hub::poll_node(coven_home, node_id)
        }
        ("POST", path) if path.starts_with("/hub/nodes/") && path.ends_with("/dispatch") => {
            let node_id = path
                .trim_start_matches("/hub/nodes/")
                .trim_end_matches("/dispatch");
            crate::hub::dispatch_to_node(coven_home, node_id, body)
        }
        ("GET", path) if path.starts_with("/hub/dispatches/") => {
            let job_id = path.trim_start_matches("/hub/dispatches/");
            crate::hub::get_dispatch(coven_home, job_id)
        }
        ("GET", path) if path.starts_with("/hub/nodes/") => {
            let node_id = path.trim_start_matches("/hub/nodes/");
            crate::hub::get_node(coven_home, node_id)
        }
        ("POST", "/hub/jobs") => crate::hub::enqueue_job(coven_home, body),
        ("GET", "/hub/jobs") => {
            let q = query.unwrap_or_default();
            crate::hub::list_jobs(coven_home, q)
        }
        ("POST", path) if path.starts_with("/hub/jobs/") && path.ends_with("/assign") => {
            let job_id = path
                .trim_start_matches("/hub/jobs/")
                .trim_end_matches("/assign");
            crate::hub::assign_job(coven_home, job_id, body)
        }
        ("POST", path) if path.starts_with("/hub/jobs/") && path.ends_with("/complete") => {
            let job_id = path
                .trim_start_matches("/hub/jobs/")
                .trim_end_matches("/complete");
            crate::hub::complete_job(coven_home, job_id, body)
        }
        ("GET", path) if path.starts_with("/hub/jobs/") => {
            let job_id = path.trim_start_matches("/hub/jobs/");
            crate::hub::get_job(coven_home, job_id)
        }
        ("GET", "/hub/routing") => crate::hub::list_routing_table(coven_home),
        ("GET", path) if path.starts_with("/scheduler/decisions/") => {
            let decision_id = path.trim_start_matches("/scheduler/decisions/");
            get_scheduler_decision(coven_home, decision_id)
        }
        ("GET", path) if path.starts_with("/scheduler/loops/") => {
            let loop_id = path.trim_start_matches("/scheduler/loops/");
            get_scheduler_loop_state(coven_home, loop_id)
        }
        ("GET", "/sessions") => {
            let conn = store::open_store(&store_path(coven_home))?;
            reap_stale_created_sessions_throttled(&conn);
            let sessions = store::list_sessions(&conn)?;
            json_response(200, &sessions)
        }
        ("POST", "/sessions") => launch_session(coven_home, body, runtime),
        ("POST", "/sessions/external") => register_external_session(coven_home, body),
        ("POST", path) if path.starts_with("/sessions/") && path.ends_with("/complete") => {
            let session_id = session_action_id(path, "/complete");
            complete_external_session(coven_home, session_id, body)
        }
        ("POST", path) if path.starts_with("/sessions/") && path.ends_with("/input") => {
            let session_id = session_action_id(path, "/input");
            record_input(coven_home, session_id, body, runtime)
        }
        ("POST", path) if path.starts_with("/sessions/") && path.ends_with("/kill") => {
            let session_id = session_action_id(path, "/kill");
            kill_session(coven_home, session_id, runtime)
        }
        ("GET", path) if path.starts_with("/sessions/") && path.ends_with("/log") => {
            let session_id = session_action_id(path, "/log");
            list_session_log(coven_home, session_id)
        }
        ("GET", path) if path.starts_with("/sessions/") && path.ends_with("/events") => {
            let session_id = session_action_id(path, "/events");
            let q = query.unwrap_or_default();
            list_session_events(coven_home, session_id, q)
        }
        ("GET", path) if path.starts_with("/sessions/") && path.contains("/artifacts/") => {
            let q = query.unwrap_or_default();
            get_session_artifact(coven_home, path, q)
        }
        ("GET", path) if path.starts_with("/sessions/") => {
            let session_id = path.trim_start_matches("/sessions/");
            let conn = store::open_store(&store_path(coven_home))?;
            match store::get_session(&conn, session_id)? {
                Some(session) => json_response(200, &session),
                None => api_error(
                    404,
                    "session_not_found",
                    "Session was not found.",
                    Some(json!({ "sessionId": session_id })),
                ),
            }
        }
        ("GET", "/events") => {
            let q = query.unwrap_or_default();
            match query_param(q, "sessionId") {
                Some(session_id) => list_session_events(coven_home, session_id, q),
                None => api_error(
                    400,
                    "invalid_request",
                    "sessionId query parameter is required.",
                    None,
                ),
            }
        }
        _ => api_error(404, "not_found", "Route not found.", None),
    }
}

enum ApiRoute<'a> {
    Route(Cow<'a, str>),
    Unsupported(String),
    Malformed,
}

fn normalize_api_route(route: &str) -> ApiRoute<'_> {
    let Some(rest) = route.strip_prefix("/api/") else {
        return ApiRoute::Route(Cow::Borrowed(route));
    };
    let Some((version, suffix)) = rest.split_once('/') else {
        return ApiRoute::Malformed;
    };
    if version != COVEN_API_VERSION {
        return ApiRoute::Unsupported(version.to_string());
    }
    if suffix.is_empty() {
        return ApiRoute::Malformed;
    }
    ApiRoute::Route(Cow::Owned(format!("/{suffix}")))
}

fn store_path(coven_home: &Path) -> std::path::PathBuf {
    coven_home.join("coven.sqlite3")
}

fn vacuum_store(coven_home: &Path) -> Result<ApiResponse> {
    let store_path = store_path(coven_home);
    match store::vacuum_store_path(&store_path) {
        Ok(report) => json_response(
            200,
            &json!({
                "ok": true,
                "eventIndexRebuilt": report.event_index_rebuilt,
                "integrityCheck": report.integrity_check,
            }),
        ),
        Err(error) => api_error(
            500,
            "store_vacuum_failed",
            "Failed to vacuum Coven store.",
            Some(json!({
                "storePath": store_path.display().to_string(),
                "error": error.to_string(),
            })),
        ),
    }
}

fn generate_travel_profile(coven_home: &Path, body: Option<&str>) -> Result<ApiResponse> {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };
    let Some(familiar_id) = payload.get("familiarId").and_then(Value::as_str) else {
        return api_error(400, "invalid_request", "familiarId is required.", None);
    };
    if familiar_id.trim().is_empty() {
        return api_error(400, "invalid_request", "familiarId is required.", None);
    }
    let workspace_id = payload
        .get("workspaceId")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("default");
    let expires_in_seconds = payload
        .get("expiresInSeconds")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(7 * 24 * 60 * 60);
    let stale_after_seconds = payload
        .get("staleAfterSeconds")
        .and_then(Value::as_i64)
        .filter(|value| *value > 0)
        .unwrap_or(2 * 24 * 60 * 60)
        .min(expires_in_seconds);

    let conn = store::open_store(&store_path(coven_home))?;
    let source_hub_id = store::get_or_insert_store_meta(
        &conn,
        "travel_source_hub_id",
        &format!("hub_{}", Uuid::new_v4()),
    )?;

    let now = Utc::now();
    let generated_at = now.to_rfc3339_opts(SecondsFormat::Nanos, true);
    let expires_at =
        (now + Duration::seconds(expires_in_seconds)).to_rfc3339_opts(SecondsFormat::Nanos, true);
    let stale_after =
        (now + Duration::seconds(stale_after_seconds)).to_rfc3339_opts(SecondsFormat::Nanos, true);
    let profile_id = format!("travel_{}", Uuid::new_v4());
    let source_revision = json!({
        "memoryRevision": format!("mem_{}", Uuid::new_v4()),
        "loopRevision": format!("loop_{}", Uuid::new_v4()),
    });
    let permissions = json!({
        "mode": "travel-read-only",
        "allowedLocalAgents": ["lightweight"],
        "allowMemoryOverwrite": false,
        "allowHeavyweightLocalWork": false,
    });
    let scope = json!({
        "familiarId": familiar_id,
        "workspaceId": workspace_id,
    });
    let source_hub = json!({
        "hubId": source_hub_id,
        "displayName": "Coven hub",
    });
    let memory_context: Vec<_> = crate::cockpit_sources::scan_memory(coven_home)?
        .into_iter()
        .filter(|memory| memory.familiar_id == familiar_id)
        .collect();
    let profile_payload = json!({
        "version": "0.1",
        "profileId": profile_id,
        "generatedAt": generated_at,
        "expiresAt": expires_at,
        "staleAfter": stale_after,
        "sourceHub": source_hub,
        "scope": scope,
        "sourceRevision": source_revision,
        "permissions": permissions,
        "payload": {
            "memoryContext": memory_context,
            "workspaceContext": [],
            "policyContext": [],
        },
    });
    let profile_bytes =
        serde_json::to_vec(&profile_payload).context("failed to serialize travel profile")?;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(&profile_bytes)
        .context("failed to compress travel profile")?;
    let compressed = encoder
        .finish()
        .context("failed to finish travel profile")?;
    let profile_blob = base64::engine::general_purpose::STANDARD.encode(&compressed);
    let mut hasher = Sha256::new();
    hasher.update(&compressed);
    let digest = hasher.finalize();
    let hex_digest: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let content_hash = format!("sha256:{hex_digest}");
    let profile_dir = coven_home.join("travel").join("profiles");
    std::fs::create_dir_all(&profile_dir)
        .with_context(|| format!("failed to create {}", profile_dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&profile_dir, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("failed to protect {}", profile_dir.display()))?;
    }
    let profile_path = profile_dir.join(format!("{profile_id}.json.gz"));
    std::fs::write(&profile_path, &compressed)
        .with_context(|| format!("failed to write {}", profile_path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&profile_path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("failed to protect {}", profile_path.display()))?;
    }
    let mut file_permissions = std::fs::metadata(&profile_path)
        .with_context(|| format!("failed to inspect {}", profile_path.display()))?
        .permissions();
    file_permissions.set_readonly(true);
    std::fs::set_permissions(&profile_path, file_permissions)
        .with_context(|| format!("failed to mark {} read-only", profile_path.display()))?;

    store::insert_travel_profile(
        &conn,
        &store::TravelProfileRecord {
            id: profile_id.clone(),
            familiar_id: familiar_id.to_string(),
            workspace_id: workspace_id.to_string(),
            version: "0.1".to_string(),
            generated_at: generated_at.clone(),
            expires_at: expires_at.clone(),
            stale_after: stale_after.clone(),
            source_hub_id: source_hub_id.clone(),
            source_revision_json: source_revision.to_string(),
            permissions_json: permissions.to_string(),
            payload_json: profile_payload["payload"].to_string(),
            encoding: "gzip+base64".to_string(),
            content_hash: content_hash.clone(),
            profile_blob: profile_blob.clone(),
            created_at: generated_at.clone(),
        },
    )?;

    json_response(
        201,
        &json!({
            "profileId": profile_id,
            "version": "0.1",
            "generatedAt": generated_at,
            "expiresAt": expires_at,
            "staleAfter": stale_after,
            "sourceHub": source_hub,
            "scope": scope,
            "sourceRevision": source_revision,
            "permissions": permissions,
            "encoding": "gzip+base64",
            "contentHash": content_hash,
            "profileBlob": profile_blob,
        }),
    )
}

fn upload_travel_delta(coven_home: &Path, body: Option<&str>, query: &str) -> Result<ApiResponse> {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };
    let Some(profile_id) = payload.get("profileId").and_then(Value::as_str) else {
        return api_error(400, "invalid_request", "profileId is required.", None);
    };
    let Some(source_hub_id) = payload.get("sourceHubId").and_then(Value::as_str) else {
        return api_error(400, "invalid_request", "sourceHubId is required.", None);
    };
    let Some(client_id) = payload.get("clientId").and_then(Value::as_str) else {
        return api_error(400, "invalid_request", "clientId is required.", None);
    };

    let conn = store::open_store(&store_path(coven_home))?;
    let Some(profile) = store::get_travel_profile(&conn, profile_id)? else {
        return api_error(
            404,
            "travel_profile_not_found",
            "Travel profile was not found.",
            Some(json!({ "profileId": profile_id })),
        );
    };
    if profile.source_hub_id != source_hub_id {
        return api_error(
            409,
            "source_hub_mismatch",
            "Offline delta source hub does not match the travel profile.",
            Some(json!({
                "profileId": profile_id,
                "expectedSourceHubId": profile.source_hub_id,
                "sourceHubId": source_hub_id,
            })),
        );
    }
    if profile_freshness(&profile) == "expired" {
        return api_error(
            409,
            "travel_profile_expired",
            "Travel profile has expired and cannot accept offline deltas.",
            Some(json!({
                "profileId": profile_id,
                "expiresAt": profile.expires_at,
            })),
        );
    }

    let accepted_events = payload
        .get("events")
        .and_then(Value::as_array)
        .map(|events| events.len() as i64)
        .unwrap_or(0);
    let accepted_artifacts = payload
        .get("artifacts")
        .and_then(Value::as_array)
        .map(|artifacts| artifacts.len() as i64)
        .unwrap_or(0);
    let memory_review_state = if payload
        .get("proposedMemoryAdditions")
        .and_then(Value::as_array)
        .is_some_and(|items| !items.is_empty())
    {
        "queued"
    } else {
        "none"
    };
    let state = match query_param(query, "state") {
        Some("handoff_pending") => "handoff_pending",
        Some("syncing_delta") => "syncing_delta",
        Some("hub_resumed") => "hub_resumed",
        Some(other) => {
            return api_error(
                400,
                "invalid_request",
                "travel delta state must be `handoff_pending`, `syncing_delta`, or `hub_resumed`.",
                Some(json!({ "state": other })),
            );
        }
        None if query_param(query, "defer") == Some("1") => "handoff_pending",
        None => "hub_resumed",
    };
    let now = current_timestamp();
    let delta_id = format!("delta_{}", Uuid::new_v4());
    let reconciliation_session_id = format!("travel-{delta_id}");
    store::insert_session(
        &conn,
        &store::SessionRecord {
            id: reconciliation_session_id.clone(),
            project_root: coven_home.to_string_lossy().into_owned(),
            harness: "travel".to_string(),
            title: format!("Travel delta from {client_id}"),
            status: "completed".to_string(),
            exit_code: Some(0),
            archived_at: None,
            created_at: now.clone(),
            updated_at: now.clone(),
            conversation_id: Some(client_id.to_string()),
            familiar_id: Some(profile.familiar_id.clone()),
            labels: vec!["travel".to_string(), "offline-delta".to_string()],
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        },
    )?;
    if let Some(events) = payload.get("events").and_then(Value::as_array) {
        for event in events {
            insert_event(
                &conn,
                coven_home,
                &reconciliation_session_id,
                "travel.offline_event",
                event.clone(),
            )?;
        }
    }
    if let Some(artifacts) = payload.get("artifacts").and_then(Value::as_array) {
        for artifact in artifacts {
            insert_event(
                &conn,
                coven_home,
                &reconciliation_session_id,
                "travel.offline_artifact",
                artifact.clone(),
            )?;
        }
    }
    store::insert_travel_delta(
        &conn,
        &store::TravelDeltaRecord {
            id: delta_id.clone(),
            profile_id: profile_id.to_string(),
            source_hub_id: source_hub_id.to_string(),
            client_id: client_id.to_string(),
            state: state.to_string(),
            raw_delta_json: payload.to_string(),
            accepted_events,
            accepted_artifacts,
            memory_review_state: memory_review_state.to_string(),
            canonical_memory_overwrite_applied: false,
            created_at: now.clone(),
            updated_at: now,
        },
    )?;

    json_response(
        202,
        &json!({
            "deltaId": delta_id,
            "state": state,
            "acceptedEvents": accepted_events,
            "acceptedArtifacts": accepted_artifacts,
            "memoryReviewState": memory_review_state,
            "canonicalMemoryOverwriteApplied": false,
            "reconciliationSessionId": reconciliation_session_id,
            "hubRevision": {
                "memoryRevision": format!("mem_{}", Uuid::new_v4()),
                "loopRevision": format!("loop_{}", Uuid::new_v4()),
            },
        }),
    )
}

fn travel_state(coven_home: &Path, query: &str) -> Result<ApiResponse> {
    let Some(client_id) = query_param(query, "clientId") else {
        return api_error(
            400,
            "invalid_request",
            "clientId query parameter is required.",
            None,
        );
    };
    let conn = store::open_store(&store_path(coven_home))?;
    let latest = store::latest_travel_delta_for_client(&conn, client_id)?;
    let requested_profile = match query_param(query, "profileId") {
        Some(profile_id) => match store::get_travel_profile(&conn, profile_id)? {
            Some(profile) => Some(profile),
            None => {
                return api_error(
                    404,
                    "travel_profile_not_found",
                    "Travel profile was not found.",
                    Some(json!({ "profileId": profile_id })),
                );
            }
        },
        None => None,
    };
    let valid_states = [
        "hub_active",
        "travel_local",
        "travel_stale",
        "handoff_pending",
        "syncing_delta",
        "hub_resumed",
    ];
    match latest {
        Some(delta) => {
            let profile = match store::get_travel_profile(&conn, &delta.profile_id)? {
                Some(profile) => Some(profile),
                None => requested_profile,
            };
            let freshness = profile.as_ref().map(profile_freshness).unwrap_or("unknown");
            let pending_delta_bytes =
                if delta.state == "handoff_pending" || delta.state == "syncing_delta" {
                    delta.raw_delta_json.len()
                } else {
                    0
                };
            json_response(
                200,
                &json!({
                    "state": delta.state,
                    "profileId": delta.profile_id,
                    "pendingDeltaBytes": pending_delta_bytes,
                    "lastSyncError": null,
                    "hubReachable": true,
                    "profileFreshness": freshness,
                    "travelExecutionAllowed": freshness != "expired",
                    "validStates": valid_states,
                }),
            )
        }
        None => {
            if let Some(profile) = requested_profile {
                let freshness = profile_freshness(&profile);
                let state = match freshness {
                    "fresh" => "travel_local",
                    "stale" | "expired" => "travel_stale",
                    _ => "travel_local",
                };
                json_response(
                    200,
                    &json!({
                        "state": state,
                        "profileId": profile.id,
                        "pendingDeltaBytes": 0,
                        "lastSyncError": null,
                        "hubReachable": false,
                        "profileFreshness": freshness,
                        "travelExecutionAllowed": freshness != "expired",
                        "validStates": valid_states,
                    }),
                )
            } else {
                json_response(
                    200,
                    &json!({
                        "state": "hub_active",
                        "profileId": null,
                        "pendingDeltaBytes": 0,
                        "lastSyncError": null,
                        "hubReachable": true,
                        "profileFreshness": "none",
                        "travelExecutionAllowed": true,
                        "validStates": valid_states,
                    }),
                )
            }
        }
    }
}

fn profile_freshness(profile: &store::TravelProfileRecord) -> &'static str {
    if timestamp_is_past_or_now(&profile.expires_at) {
        "expired"
    } else if timestamp_is_past_or_now(&profile.stale_after) {
        "stale"
    } else {
        "fresh"
    }
}

fn timestamp_is_past_or_now(timestamp: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .map(|dt| dt.with_timezone(&Utc) <= Utc::now())
        .unwrap_or(true)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchedulerDecisionRequest {
    job_id: String,
    #[serde(default)]
    required_capabilities: Vec<String>,
    #[serde(default)]
    task_weight: Option<String>,
    #[serde(default)]
    travel_state: Option<String>,
    #[serde(default)]
    allow_heavyweight_local_work: bool,
    #[serde(default)]
    nodes: Vec<SchedulerNodeInput>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchedulerNodeInput {
    node_id: String,
    role: String,
    #[serde(default)]
    available: bool,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default)]
    queue_pressure: i64,
    #[serde(default)]
    battery_percent: Option<i64>,
    #[serde(default)]
    power_source: Option<String>,
    #[serde(default)]
    queued_job_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SchedulerRedispatchRequest {
    loop_id: String,
    job_id: String,
    current_node_id: String,
    #[serde(default)]
    required_capabilities: Vec<String>,
    #[serde(default)]
    loop_resumable: bool,
    #[serde(default)]
    nodes: Vec<SchedulerNodeInput>,
}

/// Load scheduler candidates from the persistent hub node registry (#301).
/// Used when a scheduler request omits its `nodes` snapshot, so ad-hoc
/// snapshots and hub-registered nodes share one source of truth. Subqueue
/// contents come from the persistent per-executor queues.
fn scheduler_nodes_from_registry(conn: &rusqlite::Connection) -> Result<Vec<SchedulerNodeInput>> {
    let queued_by_node: std::collections::HashMap<String, Vec<String>> =
        store::list_executor_queues(conn)?
            .into_iter()
            .map(|queue| {
                let job_ids: Vec<String> =
                    serde_json::from_str(&queue.job_ids_json).unwrap_or_default();
                (queue.node_id, job_ids)
            })
            .collect();
    Ok(store::list_nodes(conn)?
        .into_iter()
        .map(|node| {
            let capabilities: Vec<String> =
                serde_json::from_str(&node.capabilities_json).unwrap_or_default();
            SchedulerNodeInput {
                queued_job_ids: queued_by_node
                    .get(&node.node_id)
                    .cloned()
                    .unwrap_or_default(),
                node_id: node.node_id,
                role: node.role,
                available: node.available,
                capabilities,
                queue_pressure: node.queue_pressure,
                battery_percent: None,
                power_source: None,
            }
        })
        .collect())
}

fn scheduler_decision(coven_home: &Path, body: Option<&str>) -> Result<ApiResponse> {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };
    let mut request: SchedulerDecisionRequest = match serde_json::from_value(payload) {
        Ok(request) => request,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };
    if request.job_id.trim().is_empty() {
        return api_error(400, "invalid_request", "jobId is required.", None);
    }
    let conn = store::open_store(&store_path(coven_home))?;
    let nodes_source = if request.nodes.is_empty() {
        request.nodes = scheduler_nodes_from_registry(&conn)?;
        "hub_registry"
    } else {
        "request_snapshot"
    };
    if request.nodes.is_empty() {
        return api_error(
            409,
            "no_scheduler_target",
            "No scheduler nodes were supplied and the hub node registry is empty.",
            Some(json!({ "jobId": request.job_id, "nodesSource": nodes_source })),
        );
    }

    let heavy_travel_local = request.task_weight.as_deref() == Some("heavyweight")
        && matches!(
            request.travel_state.as_deref(),
            Some("travel_local") | Some("travel_stale")
        )
        && !request.allow_heavyweight_local_work;
    let battery_blocked_any = request.nodes.iter().any(|node| {
        node.available
            && node_supports_capabilities(node, &request.required_capabilities)
            && travel_battery_blocks_laptop_local(node, request.travel_state.as_deref())
    });
    let mut candidates: Vec<&SchedulerNodeInput> = request
        .nodes
        .iter()
        .filter(|node| node.available)
        .filter(|node| node_supports_capabilities(node, &request.required_capabilities))
        .filter(|node| !(heavy_travel_local && node.role == "laptop_local"))
        .filter(|node| !travel_battery_blocks_laptop_local(node, request.travel_state.as_deref()))
        .collect();
    candidates.sort_by(|left, right| {
        left.queue_pressure
            .cmp(&right.queue_pressure)
            .then_with(|| scheduler_role_rank(&left.role).cmp(&scheduler_role_rank(&right.role)))
            .then_with(|| left.node_id.cmp(&right.node_id))
    });
    let Some(target_node) = candidates.first().copied() else {
        return api_error(
            409,
            "no_scheduler_target",
            "No available scheduler node matches the requested capabilities and policy.",
            Some(json!({
                "jobId": request.job_id,
                "requiredCapabilities": request.required_capabilities,
                "travelState": request.travel_state.unwrap_or_else(|| "hub_active".to_string()),
                "batteryAware": battery_blocked_any,
            })),
        );
    };

    let decision_id = format!("sched_{}", Uuid::new_v4());
    let target = json!({
        "role": target_node.role,
        "nodeId": target_node.node_id,
    });
    let travel_state = request
        .travel_state
        .clone()
        .unwrap_or_else(|| "hub_active".to_string());
    let inputs = json!({
        "requiredCapabilities": request.required_capabilities,
        "queuePressure": queue_pressure_label(target_node.queue_pressure),
        "travelState": travel_state,
        "taskWeight": request.task_weight.unwrap_or_else(|| "normal".to_string()),
        "nodesSource": nodes_source,
    });
    let reason = format!(
        "{} has required capability set and {} queue pressure",
        target_node.role,
        queue_pressure_label(target_node.queue_pressure)
    );
    let now = current_timestamp();
    let record = store::SchedulerDecisionRecord {
        id: decision_id,
        job_id: request.job_id,
        target_role: target_node.role.clone(),
        target_node_id: Some(target_node.node_id.clone()),
        target_json: target.to_string(),
        reason,
        inputs_json: inputs.to_string(),
        created_at: now,
    };
    store::insert_scheduler_decision(&conn, &record)?;
    let response = scheduler_decision_response(&record)?;
    json_response(201, &response)
}

fn get_scheduler_decision(coven_home: &Path, decision_id: &str) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    match store::get_scheduler_decision(&conn, decision_id)? {
        Some(record) => json_response(200, &scheduler_decision_response(&record)?),
        None => api_error(
            404,
            "scheduler_decision_not_found",
            "Scheduler decision was not found.",
            Some(json!({ "decisionId": decision_id })),
        ),
    }
}

fn scheduler_redispatch(coven_home: &Path, body: Option<&str>) -> Result<ApiResponse> {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };
    let mut request: SchedulerRedispatchRequest = match serde_json::from_value(payload) {
        Ok(request) => request,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };
    if request.loop_id.trim().is_empty() {
        return api_error(400, "invalid_request", "loopId is required.", None);
    }
    if request.job_id.trim().is_empty() {
        return api_error(400, "invalid_request", "jobId is required.", None);
    }
    let conn = store::open_store(&store_path(coven_home))?;
    let nodes_source = if request.nodes.is_empty() {
        request.nodes = scheduler_nodes_from_registry(&conn)?;
        "hub_registry"
    } else {
        "request_snapshot"
    };
    let Some(current_node) = request
        .nodes
        .iter()
        .find(|node| node.node_id == request.current_node_id)
    else {
        return api_error(
            400,
            "invalid_request",
            if nodes_source == "hub_registry" {
                "currentNodeId must refer to a node in the hub registry."
            } else {
                "currentNodeId must refer to a supplied node."
            },
            Some(json!({
                "currentNodeId": request.current_node_id,
                "nodesSource": nodes_source,
            })),
        );
    };
    let preserved_job_ids = if current_node.queued_job_ids.is_empty() {
        vec![request.job_id.clone()]
    } else {
        current_node.queued_job_ids.clone()
    };
    let node_availability: Vec<Value> = request
        .nodes
        .iter()
        .map(|node| {
            json!({
                "nodeId": node.node_id,
                "role": node.role,
                "available": node.available,
                "queuePressure": queue_pressure_label(node.queue_pressure),
            })
        })
        .collect();
    let mut candidates: Vec<&SchedulerNodeInput> = request
        .nodes
        .iter()
        .filter(|node| node.node_id != request.current_node_id)
        .filter(|node| node.available)
        .filter(|node| node_supports_capabilities(node, &request.required_capabilities))
        .collect();
    candidates.sort_by(|left, right| {
        left.queue_pressure
            .cmp(&right.queue_pressure)
            .then_with(|| scheduler_role_rank(&left.role).cmp(&scheduler_role_rank(&right.role)))
            .then_with(|| left.node_id.cmp(&right.node_id))
    });
    let target_node = candidates
        .first()
        .copied()
        .filter(|_| request.loop_resumable);
    let (state, target, reason) = match target_node {
        Some(node) => (
            "redispatched",
            json!({
                "role": node.role,
                "nodeId": node.node_id,
            }),
            format!(
                "{} went offline; redispatched resumable loop to {}",
                request.current_node_id, node.node_id
            ),
        ),
        None => (
            "paused",
            json!({
                "role": "paused",
                "nodeId": null,
            }),
            format!(
                "{} went offline; preserved subqueue and paused loop",
                request.current_node_id
            ),
        ),
    };
    let inputs = json!({
        "requiredCapabilities": request.required_capabilities,
        "failedNodeId": request.current_node_id,
        "loopResumable": request.loop_resumable,
        "nodeAvailability": node_availability,
        "nodesSource": nodes_source,
    });
    let now = current_timestamp();
    let decision_id = format!("sched_{}", Uuid::new_v4());
    let record = store::SchedulerDecisionRecord {
        id: decision_id.clone(),
        job_id: request.job_id.clone(),
        target_role: target["role"].as_str().unwrap_or("unknown").to_string(),
        target_node_id: target["nodeId"].as_str().map(str::to_string),
        target_json: target.to_string(),
        reason: reason.clone(),
        inputs_json: inputs.to_string(),
        created_at: now.clone(),
    };
    store::insert_scheduler_decision(&conn, &record)?;
    let preserved_job_ids_json =
        serde_json::to_string(&preserved_job_ids).context("failed to serialize preserved queue")?;
    store::upsert_executor_queue(
        &conn,
        &store::ExecutorQueueRecord {
            node_id: request.current_node_id.clone(),
            job_ids_json: preserved_job_ids_json,
            updated_at: now.clone(),
        },
    )?;
    let node_availability_json = serde_json::to_string(&node_availability)
        .context("failed to serialize node availability")?;
    store::upsert_scheduler_loop_state(
        &conn,
        &store::SchedulerLoopStateRecord {
            loop_id: request.loop_id.clone(),
            job_id: request.job_id.clone(),
            state: state.to_string(),
            decision_id: decision_id.clone(),
            target_json: target.to_string(),
            preserved_subqueue_node_id: request.current_node_id.clone(),
            node_availability_json,
            reason: reason.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )?;
    // When the job is tracked in the hub's persistent global queue, keep the
    // hub job, routing table, and executor subqueues consistent with this
    // redispatch decision (#301).
    let hub_job_synced = crate::hub::apply_redispatch_outcome(
        &conn,
        &request.job_id,
        target["nodeId"].as_str(),
        &decision_id,
        &reason,
        &now,
    )?;
    json_response(
        202,
        &json!({
            "decisionId": decision_id,
            "state": state,
            "loopId": request.loop_id,
            "jobId": request.job_id,
            "target": target,
            "reason": reason,
            "preservedSubqueue": {
                "nodeId": request.current_node_id,
                "jobIds": preserved_job_ids,
            },
            "nodeAvailability": node_availability,
            "hubJobSynced": hub_job_synced,
            "createdAt": now,
        }),
    )
}

fn get_scheduler_loop_state(coven_home: &Path, loop_id: &str) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    let Some(record) = store::get_scheduler_loop_state(&conn, loop_id)? else {
        return api_error(
            404,
            "scheduler_loop_not_found",
            "Scheduler loop state was not found.",
            Some(json!({ "loopId": loop_id })),
        );
    };
    let target: Value =
        serde_json::from_str(&record.target_json).context("failed to parse scheduler target")?;
    let node_availability: Value = serde_json::from_str(&record.node_availability_json)
        .context("failed to parse scheduler node availability")?;
    let queue = store::get_executor_queue(&conn, &record.preserved_subqueue_node_id)?;
    let job_ids: Value = match queue {
        Some(queue) => serde_json::from_str(&queue.job_ids_json)
            .context("failed to parse executor queue job ids")?,
        None => json!([]),
    };
    json_response(
        200,
        &json!({
            "decisionId": record.decision_id,
            "state": record.state,
            "loopId": record.loop_id,
            "jobId": record.job_id,
            "target": target,
            "reason": record.reason,
            "preservedSubqueue": {
                "nodeId": record.preserved_subqueue_node_id,
                "jobIds": job_ids,
            },
            "nodeAvailability": node_availability,
            "createdAt": record.created_at,
            "updatedAt": record.updated_at,
        }),
    )
}

fn scheduler_decision_response(record: &store::SchedulerDecisionRecord) -> Result<Value> {
    let target: Value =
        serde_json::from_str(&record.target_json).context("failed to parse scheduler target")?;
    let inputs: Value =
        serde_json::from_str(&record.inputs_json).context("failed to parse scheduler inputs")?;
    Ok(json!({
        "decisionId": record.id,
        "jobId": record.job_id,
        "target": target,
        "reason": record.reason,
        "inputs": inputs,
        "createdAt": record.created_at,
    }))
}

fn scheduler_role_rank(role: &str) -> i32 {
    match role {
        "compute_executor" => 0,
        "stationary_executor" => 1,
        "hub" => 2,
        "laptop_local" => 3,
        _ => 4,
    }
}

fn node_supports_capabilities(node: &SchedulerNodeInput, required_capabilities: &[String]) -> bool {
    required_capabilities.iter().all(|required| {
        node.capabilities
            .iter()
            .any(|capability| capability == required)
    })
}

fn travel_battery_blocks_laptop_local(
    node: &SchedulerNodeInput,
    travel_state: Option<&str>,
) -> bool {
    if node.role != "laptop_local" {
        return false;
    }
    if !matches!(travel_state, Some("travel_local") | Some("travel_stale")) {
        return false;
    }
    if node.power_source.as_deref() != Some("battery") {
        return false;
    }
    node.battery_percent.is_some_and(|percent| percent <= 15)
}

fn queue_pressure_label(queue_pressure: i64) -> &'static str {
    match queue_pressure {
        i64::MIN..=2 => "low",
        3..=6 => "medium",
        _ => "high",
    }
}

fn launch_session(
    coven_home: &Path,
    body: Option<&str>,
    runtime: &dyn SessionRuntime,
) -> Result<ApiResponse> {
    // Client-side validation errors (malformed JSON, bad fields,
    // unsupported launchMode, malformed `conversation` object, …) must
    // become structured 400 responses. Bubbling them up as Err crashes
    // the daemon, since the api-server loop `?`-propagates errors out
    // of the accept loop and terminates the process.
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };
    let mut launch = match session_launch_from_payload(payload) {
        Ok(launch) => launch,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };
    let familiar_ctx = match launch.familiar_id.as_deref() {
        Some(familiar_id) => match crate::familiar_identity::resolve(coven_home, familiar_id) {
            Ok(Some(familiar_ctx)) => Some(familiar_ctx),
            Ok(None) => {
                let error =
                    crate::familiar_identity::unknown_familiar_error(coven_home, familiar_id);
                return api_error(
                    400,
                    "unknown_familiar",
                    &error.to_string(),
                    Some(json!({ "familiarId": familiar_id })),
                );
            }
            Err(error) => {
                return api_error(500, "familiar_lookup_failed", &error.to_string(), None);
            }
        },
        None => None,
    };
    launch.familiar_id = familiar_ctx.as_ref().map(|familiar| familiar.id.clone());
    let conn = store::open_store(&store_path(coven_home))?;
    let now = current_timestamp();
    let record = store::SessionRecord {
        id: launch.id.clone(),
        project_root: launch.project_root.clone(),
        harness: launch.harness.clone(),
        title: launch.title.clone(),
        status: "running".to_string(),
        exit_code: None,
        archived_at: None,
        created_at: now.clone(),
        updated_at: now,
        conversation_id: launch.conversation_id.clone(),
        familiar_id: familiar_ctx.as_ref().map(|familiar| familiar.id.clone()),
        labels: Vec::new(),
        visibility: "private".to_string(),
        external: false,
        transcript_path: None,
    };
    store::insert_session(&conn, &record)?;
    if let Err(error) = runtime.launch_session(&launch) {
        // Don't propagate to the accept loop — that crashes the daemon.
        // Runtime launch failures are user-facing (missing harness CLI,
        // missing auth, child closed stdin during stream-mode init):
        // mark the session row failed and return a structured response
        // so the client surfaces the cause and the daemon stays up.
        store::update_session_status(&conn, &record.id, "failed", None, &current_timestamp())?;
        return api_error(
            500,
            "launch_failed",
            &error.to_string(),
            Some(json!({ "sessionId": record.id })),
        );
    }
    // Record the inter-familiar delegation in cave-coven-calls.json so the
    // Coven Calls graph in coven-cave has data to render. Best-effort: a
    // write failure must not abort a successful launch.
    if let (Some(caller_id), Some(callee_id)) = (&launch.caller_familiar_id, &launch.familiar_id) {
        if let Err(_err) = crate::coven_calls::emit_running(
            coven_home,
            caller_id,
            callee_id,
            &launch.prompt,
            Some(record.id.as_str()),
        ) {
            eprintln!("[coven-calls] warn: failed to record delegation: {_err}");
        }
    }
    json_response(201, &record)
}

fn register_external_session(coven_home: &Path, body: Option<&str>) -> Result<ApiResponse> {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let id = match required_string(&payload, "id") {
        Ok(id) => id,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let project_root = match required_string(&payload, "projectRoot") {
        Ok(r) => r,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let harness = match required_string(&payload, "harness") {
        Ok(h) => h,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("External session")
        .to_string();
    let transcript_path = payload
        .get("transcriptPath")
        .and_then(Value::as_str)
        .map(str::to_string);
    let now = current_timestamp();
    let record = store::SessionRecord {
        id,
        project_root,
        harness,
        title,
        status: "running".to_string(),
        exit_code: None,
        archived_at: None,
        created_at: now.clone(),
        updated_at: now,
        conversation_id: None,
        familiar_id: None,
        labels: Vec::new(),
        visibility: "private".to_string(),
        external: true,
        transcript_path,
    };
    let conn = store::open_store(&store_path(coven_home))?;
    // Idempotent: if a row with this id already exists, return 200 with the
    // existing record rather than failing.
    let inserted = store::insert_session_if_absent(&conn, &record)?;
    let status = if inserted { 201 } else { 200 };
    // Re-read so the response always reflects the persisted row.
    let persisted = store::get_session(&conn, &record.id)?.unwrap_or(record);
    // If the insert was skipped (inserted == false) and the existing row is
    // NOT external, a daemon-managed session already holds this id — reject
    // rather than silently aliasing it.
    if !inserted && !persisted.external {
        return api_error(
            409,
            "session_id_conflict",
            "A daemon-managed session with this id already exists.",
            Some(json!({ "sessionId": &persisted.id })),
        );
    }
    json_response(status, &persisted)
}

fn complete_external_session(
    coven_home: &Path,
    session_id: &str,
    body: Option<&str>,
) -> Result<ApiResponse> {
    let payload: Value = body
        .and_then(|b| serde_json::from_str(b).ok())
        .unwrap_or(Value::Null);
    let exit_code: Option<i32> = payload
        .get("exitCode")
        .and_then(Value::as_i64)
        .map(|c| c as i32);
    let status = match exit_code {
        Some(code) if code != 0 => "failed",
        _ => "completed",
    };
    let conn = store::open_store(&store_path(coven_home))?;
    match store::get_session(&conn, session_id)? {
        None => api_error(
            404,
            "session_not_found",
            "Session was not found.",
            Some(json!({ "sessionId": session_id })),
        ),
        Some(session) if !session.external => api_error(
            422,
            "not_external_session",
            "POST /sessions/<id>/complete is only valid for externally-registered sessions. Use POST /sessions/<id>/kill for daemon-managed sessions.",
            Some(json!({ "sessionId": session_id })),
        ),
        Some(_) => {
            store::update_session_status(
                &conn,
                session_id,
                status,
                exit_code,
                &current_timestamp(),
            )?;
            let updated = store::get_session(&conn, session_id)?.expect("session vanished");
            json_response(200, &updated)
        }
    }
}

fn session_launch_from_payload(payload: Value) -> Result<SessionLaunch> {
    let project_root = required_string(&payload, "projectRoot")?;
    let cwd = payload.get("cwd").and_then(Value::as_str);
    let canonical_project_root = project::canonical_project_root(Path::new(&project_root))
        .context("failed to resolve projectRoot")?;
    let canonical_cwd = project::resolve_inside_root(&canonical_project_root, cwd.map(Path::new))?;
    let harness = required_string(&payload, "harness")?;
    // Validate against the supported harness set up-front (client error)
    // instead of letting the runtime's arg builder surface it later as a
    // 500. Bonus: rejecting here means we never insert a session row for
    // a launch that can't possibly succeed.
    let supported_specs = crate::harness::configured_harness_specs()?;
    let supported: Vec<&str> = supported_specs
        .iter()
        .map(|spec| spec.id.as_str())
        .collect();
    if !supported.contains(&harness.as_str()) {
        anyhow::bail!(
            "{}",
            crate::harness::unsupported_harness_message(&harness, &supported)
        );
    }
    let launch_mode = launch_mode_from_payload(&payload)?;
    let model = payload
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToOwned::to_owned);
    let prompt = required_string(&payload, "prompt")?;
    let title = payload
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or(&prompt)
        .to_string();

    let conversation = conversation_from_payload(&payload)?;
    let conversation_id = payload
        .get("conversationId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned);
    let familiar_id = payload
        .get("familiarId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned);
    let caller_familiar_id = payload
        .get("callerFamiliarId")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned);

    Ok(SessionLaunch {
        id: Uuid::new_v4().to_string(),
        project_root: canonical_project_root.to_string_lossy().into_owned(),
        cwd: canonical_cwd.to_string_lossy().into_owned(),
        harness,
        model,
        launch_mode,
        prompt,
        title,
        conversation,
        conversation_id,
        familiar_id,
        caller_familiar_id,
    })
}

fn launch_mode_from_payload(payload: &Value) -> Result<HarnessLaunchMode> {
    match payload.get("launchMode").and_then(Value::as_str) {
        Some("interactive") | None => Ok(HarnessLaunchMode::Interactive),
        Some("nonInteractive") => Ok(HarnessLaunchMode::NonInteractive),
        Some("stream") => Ok(HarnessLaunchMode::Stream),
        Some(other) => anyhow::bail!(
            "launchMode must be `interactive`, `nonInteractive`, or `stream`, got `{other}`"
        ),
    }
}

fn conversation_from_payload(payload: &Value) -> Result<Option<ConversationHint>> {
    let Some(value) = payload.get("conversation") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let object = value
        .as_object()
        .context("conversation must be an object with `mode` and `id` fields")?;
    let mode = object
        .get("mode")
        .and_then(Value::as_str)
        .context("conversation.mode is required and must be `init` or `resume`")?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .context("conversation.id is required and must be a non-empty string")?
        .to_string();
    // The id is forwarded verbatim as the value of the harness CLI's
    // `--session-id`/`--resume`/`resume` argument. Restrict it to an unambiguous,
    // shell-safe charset so untrusted text can never inject extra arguments or —
    // on Windows, where a `.cmd` shim re-parses the command line through cmd.exe —
    // shell metacharacters. UUIDs and opaque slugs pass; whitespace and
    // metacharacters (& | < > ^ % " $ etc.) do not.
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        anyhow::bail!("conversation.id must contain only letters, digits, '-', '_', or '.'");
    }
    match mode {
        "init" => Ok(Some(ConversationHint::Init { id })),
        "resume" => Ok(Some(ConversationHint::Resume { id })),
        other => anyhow::bail!("conversation.mode must be `init` or `resume`, got `{other}`"),
    }
}

fn required_string(payload: &Value, field: &str) -> Result<String> {
    payload
        .get(field)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .with_context(|| format!("request body requires string field `{field}`"))
}

fn record_input(
    coven_home: &Path,
    session_id: &str,
    body: Option<&str>,
    runtime: &dyn SessionRuntime,
) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    let Some(session) = store::get_session(&conn, session_id)? else {
        return api_error(
            404,
            "session_not_found",
            "Session was not found.",
            Some(json!({ "sessionId": session_id })),
        );
    };
    if session.status != "running" {
        return session_not_live_response(session_id);
    }

    // Same structured-error pattern as `launch_session`: malformed JSON
    // or runtime send failures must NOT propagate to the accept loop
    // (that crashes the daemon process). Parse errors → 400; runtime
    // errors → 500 except for "not live" which is the dedicated 409.
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error(
                400,
                "invalid_request",
                &error.to_string(),
                Some(json!({ "sessionId": session_id })),
            );
        }
    };
    // Validate `data` shape here (client error) instead of letting the
    // runtime surface it as a 500. Required field, must be a string.
    if !payload.get("data").map(|v| v.is_string()).unwrap_or(false) {
        return api_error(
            400,
            "invalid_request",
            "input payload requires string field `data`",
            Some(json!({ "sessionId": session_id })),
        );
    }
    if let Err(error) = runtime.send_input(session_id, &payload) {
        // Match the typed sentinel from the daemon runtime instead of
        // substring-matching the error message — refactoring the prose
        // later can't accidentally route the not-live case to the
        // generic 500 path.
        if error
            .downcast_ref::<crate::daemon::NotLiveError>()
            .is_some()
        {
            return session_not_live_response(session_id);
        }
        return api_error(
            500,
            "send_input_failed",
            &error.to_string(),
            Some(json!({ "sessionId": session_id })),
        );
    }
    insert_event(&conn, coven_home, session_id, "input", payload)?;
    json_response(202, &json!({ "ok": true, "accepted": true }))
}

fn kill_session(
    coven_home: &Path,
    session_id: &str,
    runtime: &dyn SessionRuntime,
) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    let Some(session) = store::get_session(&conn, session_id)? else {
        return api_error(
            404,
            "session_not_found",
            "Session was not found.",
            Some(json!({ "sessionId": session_id })),
        );
    };
    if session.status != "running" {
        return session_not_live_response(session_id);
    }
    if session.external {
        return api_error(
            422,
            "external_session_not_killable",
            "External sessions are not managed by the daemon; use POST /sessions/<id>/complete to mark them finished.",
            Some(json!({ "sessionId": session_id })),
        );
    }

    // Same structured-error pattern as the launch + input handlers: a
    // runtime kill failure (libc::kill returning EPERM, etc.) must
    // become a 500 response, not an Err that brings down the daemon.
    if let Err(error) = runtime.kill_session(session_id) {
        if error
            .downcast_ref::<crate::daemon::NotLiveError>()
            .is_some()
        {
            return session_not_live_response(session_id);
        }
        return api_error(
            500,
            "kill_failed",
            &error.to_string(),
            Some(json!({ "sessionId": session_id })),
        );
    }
    let now = current_timestamp();
    store::update_session_status(&conn, session_id, "killed", None, &now)?;
    insert_event(
        &conn,
        coven_home,
        session_id,
        "kill",
        json!({ "status": "killed" }),
    )?;
    json_response(202, &json!({ "ok": true, "accepted": true }))
}

#[derive(Debug, Clone, Serialize)]
struct CastResultDto {
    accepted: bool,
    cast_id: String,
    echo: String,
}

#[derive(Debug, Clone, Serialize)]
struct OverviewDto {
    active_familiars: u32,
    total_familiars: u32,
    open_sessions: u32,
    skills_count: u32,
    average_skill_score: u32,
    research_iterations: u32,
    last_research_delta: i32,
}

fn overview_response(coven_home: &Path) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    let sessions = store::list_sessions(&conn)?;
    let open: Vec<&store::SessionRecord> = sessions
        .iter()
        .filter(|s| s.status == "running" || s.status == "active")
        .collect();
    let open_sessions = open.len() as u32;

    // Dashboard semantics: a partial overview beats a 500, so unreadable
    // side sources degrade to empty. The leaf routes (/familiars, /skills,
    // /research) still report their own read errors loudly.
    let familiars = crate::cockpit_sources::read_familiars(coven_home).unwrap_or_default();
    let skills = crate::cockpit_sources::scan_skills(coven_home).unwrap_or_default();
    let research = crate::cockpit_sources::read_research(coven_home).unwrap_or_default();

    let roster: HashSet<&str> = familiars.iter().map(|f| f.id.as_str()).collect();
    let active_familiars = open
        .iter()
        .filter_map(|s| s.familiar_id.as_deref())
        .filter(|id| roster.contains(id))
        .collect::<HashSet<_>>()
        .len() as u32;

    let average_skill_score = if skills.is_empty() {
        0
    } else {
        let sum: f64 = skills.iter().map(|s| s.score).sum();
        (sum / skills.len() as f64).round() as u32
    };

    json_response(
        200,
        &OverviewDto {
            active_familiars,
            total_familiars: familiars.len() as u32,
            open_sessions,
            skills_count: skills.len() as u32,
            average_skill_score,
            research_iterations: research.len() as u32,
            last_research_delta: research
                .last()
                .map(|row| row.delta.round() as i32)
                .unwrap_or(0),
        },
    )
}

#[derive(Debug, Clone, Serialize)]
struct CastCodeDto {
    code: &'static str,
    description: &'static str,
    #[serde(rename = "type")]
    code_type: &'static str,
}

fn cast_codes_response() -> Result<ApiResponse> {
    let codes = [
        CastCodeDto {
            code: "~?",
            description: "Status all familiars",
            code_type: "status",
        },
        CastCodeDto {
            code: "~?{familiar}",
            description: "Status of specific familiar",
            code_type: "status",
        },
        CastCodeDto {
            code: "~>{familiar}",
            description: "Switch to familiar",
            code_type: "switch",
        },
        CastCodeDto {
            code: "~delegate:{familiar}",
            description: "Delegate task to familiar",
            code_type: "delegate",
        },
        CastCodeDto {
            code: "~broadcast *",
            description: "Broadcast to all familiars",
            code_type: "broadcast",
        },
        CastCodeDto {
            code: "~^ resume",
            description: "Resume interrupted context",
            code_type: "resume",
        },
        CastCodeDto {
            code: "~<handoff",
            description: "Hand off context to target",
            code_type: "handoff",
        },
    ];
    json_response(200, &codes)
}

fn submit_cast(coven_home: &Path, body: Option<&str>) -> Result<ApiResponse> {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };
    let code = match required_string(&payload, "code") {
        Ok(code) => code,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };
    let target = payload
        .get("target")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_string);
    let cast_id = format!("cast-{}", Uuid::new_v4().simple());
    let echo = match &target {
        Some(t) => format!("{code} → {t}"),
        None => code.clone(),
    };

    let conn = store::open_store(&store_path(coven_home))?;
    let session_id = target.as_deref().unwrap_or("__cockpit__");
    if session_id == "__cockpit__" {
        ensure_cockpit_session(&conn)?;
    } else if store::get_session(&conn, session_id)?.is_none() {
        return api_error(
            404,
            "session_not_found",
            "Target session was not found.",
            Some(json!({ "sessionId": session_id })),
        );
    }
    insert_event(
        &conn,
        coven_home,
        session_id,
        "cast",
        json!({ "cast_id": cast_id, "code": code, "target": target }),
    )?;
    json_response(
        202,
        &CastResultDto {
            accepted: true,
            cast_id,
            echo,
        },
    )
}

fn ensure_cockpit_session(conn: &rusqlite::Connection) -> Result<()> {
    // INSERT OR IGNORE keeps this atomic under concurrent Unix + TCP accept
    // loops — both transports can race the first cast through this path.
    let now = current_timestamp();
    let record = store::SessionRecord {
        id: "__cockpit__".into(),
        project_root: "(cockpit)".into(),
        harness: "cockpit".into(),
        title: "Cockpit Cast Codes".into(),
        status: "idle".into(),
        exit_code: None,
        archived_at: None,
        created_at: now.clone(),
        updated_at: now,
        conversation_id: None,
        familiar_id: None,
        labels: Vec::new(),
        visibility: "private".to_string(),
        external: false,
        transcript_path: None,
    };
    store::insert_session_if_absent(conn, &record)?;
    Ok(())
}

fn session_not_live_response(session_id: &str) -> Result<ApiResponse> {
    api_error(
        409,
        "session_not_live",
        "Session is not live.",
        Some(json!({ "sessionId": session_id })),
    )
}

fn list_session_events(coven_home: &Path, session_id: &str, query: &str) -> Result<ApiResponse> {
    let after_seq = match query_param(query, "afterSeq") {
        Some(v) => match v.parse::<i64>() {
            Ok(n) => Some(n),
            Err(_) => {
                return api_error(
                    400,
                    "invalid_request",
                    "afterSeq must be an integer.",
                    Some(json!({ "afterSeq": v })),
                );
            }
        },
        None => None,
    };
    let after_event_id = query_param(query, "afterEventId").map(str::to_string);
    let limit = match query_param(query, "limit") {
        Some(v) => match v.parse::<i64>() {
            Ok(n) => Some(n.clamp(1, MAX_EVENTS_LIMIT)),
            Err(_) => {
                return api_error(
                    400,
                    "invalid_request",
                    "limit must be an integer.",
                    Some(json!({ "limit": v })),
                );
            }
        },
        None => None,
    };

    let conn = store::open_store(&store_path(coven_home))?;
    if store::get_session(&conn, session_id)?.is_none() {
        return api_error(
            404,
            "session_not_found",
            "Session was not found.",
            Some(json!({ "sessionId": session_id })),
        );
    }

    let opts = store::EventsQueryOptions {
        after_seq,
        after_event_id,
        limit,
    };

    let events = store::list_events_with_options(&conn, session_id, &opts)?;
    let next_cursor = events.last().map(|e| EventCursor { after_seq: e.seq });
    let has_more = if let Some(lim) = limit {
        events.len() as i64 == lim
    } else {
        false
    };

    json_response(
        200,
        &EventsResponse {
            events,
            next_cursor,
            has_more,
        },
    )
}

#[derive(Debug, Clone, Serialize)]
struct LogLineDto {
    ts: String,
    level: &'static str,
    message: String,
}

fn list_session_log(coven_home: &Path, session_id: &str) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    if store::get_session(&conn, session_id)?.is_none() {
        return api_error(
            404,
            "session_not_found",
            "Session was not found.",
            Some(json!({ "sessionId": session_id })),
        );
    }
    let opts = store::EventsQueryOptions::default();
    let events = store::list_events_with_options(&conn, session_id, &opts)?;
    let lines: Vec<LogLineDto> = events.into_iter().map(event_to_log_line).collect();
    json_response(200, &lines)
}

fn get_session_artifact(coven_home: &Path, path: &str, query: &str) -> Result<ApiResponse> {
    let Some(rest) = path.strip_prefix("/sessions/") else {
        return api_error(404, "not_found", "Route not found.", None);
    };
    let Some((session_id, artifact_id)) = rest.split_once("/artifacts/") else {
        return api_error(404, "not_found", "Route not found.", None);
    };
    if query_param(query, "raw") != Some("1") {
        return api_error(
            400,
            "raw_artifact_requires_raw_flag",
            "Raw artifact retrieval requires raw=1.",
            Some(json!({ "sessionId": session_id, "artifactId": artifact_id })),
        );
    }
    let config = privacy::load_config(coven_home).unwrap_or_default();
    if !config.persist_raw_artifacts {
        return api_error(
            403,
            "raw_artifacts_disabled",
            "Raw artifact persistence is not enabled.",
            Some(json!({ "sessionId": session_id, "artifactId": artifact_id })),
        );
    }

    let conn = store::open_store(&store_path(coven_home))?;
    if store::get_session(&conn, session_id)?.is_none() {
        return api_error(
            404,
            "session_not_found",
            "Session was not found.",
            Some(json!({ "sessionId": session_id })),
        );
    }
    let Some(artifact) = store::get_sensitive_artifact(&conn, session_id, artifact_id)? else {
        return api_error(
            404,
            "artifact_not_found",
            "Sensitive artifact was not found.",
            Some(json!({ "sessionId": session_id, "artifactId": artifact_id })),
        );
    };
    if artifact.expires_at <= current_timestamp() {
        return api_error(
            404,
            "artifact_expired",
            "Sensitive artifact has expired.",
            Some(json!({ "sessionId": session_id, "artifactId": artifact_id })),
        );
    }
    let plaintext = SensitiveArtifactStore::load(coven_home)?.decrypt(
        session_id,
        &artifact.event_id,
        &artifact.kind,
        &store::artifact_payload(&artifact),
    )?;
    let payload = serde_json::from_slice::<Value>(&plaintext)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&plaintext).into_owned()));
    json_response(
        200,
        &json!({
            "sessionId": session_id,
            "artifactId": artifact.id,
            "eventId": artifact.event_id,
            "kind": artifact.kind,
            "payload": payload,
        }),
    )
}

fn event_to_log_line(event: store::EventRecord) -> LogLineDto {
    let payload: Value = serde_json::from_str(&event.payload_json).unwrap_or(Value::Null);
    let preview = payload_preview(&payload);
    let (level, message) = match event.kind.as_str() {
        "input" => ("info", format!("> {preview}")),
        "output" => ("info", preview),
        "tool_call" => ("tool", preview),
        "error" => ("error", preview),
        other => ("info", format!("{other}: {preview}")),
    };
    LogLineDto {
        ts: event.created_at,
        level,
        message,
    }
}

fn payload_preview(payload: &Value) -> String {
    privacy::payload_preview(payload, 240)
}

fn insert_event(
    conn: &rusqlite::Connection,
    coven_home: &Path,
    session_id: &str,
    kind: &str,
    payload: Value,
) -> Result<()> {
    store::insert_event_with_privacy(
        conn,
        coven_home,
        &store::EventRecord {
            // seq is populated by SQLite's rowid on insertion; the 0 here is a
            // placeholder that the INSERT statement ignores.
            seq: 0,
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            payload_json: serde_json::to_string(&payload)
                .context("failed to serialize event payload")?,
            created_at: current_timestamp(),
        },
    )
}

fn update_familiar_icon(
    coven_home: &Path,
    familiar_id: &str,
    body: Option<&str>,
) -> Result<ApiResponse> {
    if familiar_id.is_empty() || familiar_id.contains('/') {
        return api_error(
            400,
            "invalid_request",
            "Familiar id is required and must not contain '/'.",
            None,
        );
    }
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };
    // Accept `{ "icon": "ph:cat-fill" }`, `{ "icon": "🐈" }`, `{ "icon": null }`,
    // or an empty body `{}` (treated as null → clear). Reject any non-string,
    // non-null `icon` value so a typo doesn't silently write `[1,2,3]`.
    let icon: Option<String> = match payload.get("icon") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(_) => {
            return api_error(
                400,
                "invalid_request",
                "Field `icon` must be a string or null.",
                None,
            );
        }
    };
    let outcome =
        crate::cockpit_sources::write_familiar_icon(coven_home, familiar_id, icon.as_deref())?;
    use crate::cockpit_sources::WriteFamiliarIconOutcome;
    match outcome {
        WriteFamiliarIconOutcome::Updated => json_response(
            200,
            &json!({ "ok": true, "action": "updated", "id": familiar_id }),
        ),
        WriteFamiliarIconOutcome::Cleared => json_response(
            200,
            &json!({ "ok": true, "action": "cleared", "id": familiar_id }),
        ),
        WriteFamiliarIconOutcome::NotFound => api_error(
            404,
            "familiar_not_found",
            "No familiar with that id is declared in familiars.toml.",
            Some(json!({ "id": familiar_id })),
        ),
    }
}

/// `POST /familiars/{id}/edits` — the Ward-enforced write path into a familiar
/// home.
///
/// The daemon is the sole write authority for familiar homes it manages, and
/// this endpoint is deliberately the *only* daemon surface that writes
/// arbitrary files there: every edit is adjudicated by [`crate::ward::Ward::apply`],
/// the fail-closed Gates 1–2 + Gate 4 audit boundary. Fail-closed extends to
/// configuration: a familiar without a `ward.toml` in its workspace cannot be
/// written through this endpoint at all.
///
/// Request body:
///
/// ```json
/// {
///   "edits": [{ "target": "notes/today.md", "contents": "..." }],
///   "principalKeyFingerprint": "optional-signing-key-fingerprint"
/// }
/// ```
///
/// Responses: `200` applied (with Gate 4 audit records), `202` held for
/// Gate 3 coherence review (nothing written), `403` refused (nothing
/// written), `409` no ward.toml.
fn apply_familiar_edits(
    coven_home: &Path,
    familiar_id: &str,
    body: Option<&str>,
) -> Result<ApiResponse> {
    use crate::ward;

    if familiar_id.is_empty() || familiar_id.contains('/') {
        return api_error(
            400,
            "invalid_request",
            "Familiar id is required and must not contain '/'.",
            None,
        );
    }
    if crate::familiar_identity::resolve(coven_home, familiar_id)?.is_none() {
        return api_error(
            404,
            "familiar_not_found",
            "No familiar with that id is declared in familiars.toml.",
            Some(json!({ "id": familiar_id })),
        );
    }
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };

    let Some(raw_edits) = payload.get("edits").and_then(Value::as_array) else {
        return api_error(
            400,
            "invalid_request",
            "Field `edits` must be an array of { target, contents } objects.",
            None,
        );
    };
    if raw_edits.is_empty() {
        return api_error(
            400,
            "invalid_request",
            "Field `edits` must not be empty.",
            None,
        );
    }
    let mut edits = Vec::with_capacity(raw_edits.len());
    for (index, edit) in raw_edits.iter().enumerate() {
        let (Some(target), Some(contents)) = (
            edit.get("target").and_then(Value::as_str),
            edit.get("contents").and_then(Value::as_str),
        ) else {
            return api_error(
                400,
                "invalid_request",
                "Each edit must carry string `target` and `contents` fields.",
                Some(json!({ "index": index })),
            );
        };
        edits.push(ward::FileEdit::new(target, contents.to_owned()));
    }

    let authorization = match payload.get("principalKeyFingerprint") {
        None | Some(Value::Null) => ward::Authorization::unsigned(),
        Some(Value::String(fingerprint)) => ward::Authorization::signed_by(fingerprint.clone()),
        Some(_) => {
            return api_error(
                400,
                "invalid_request",
                "Field `principalKeyFingerprint` must be a string or null.",
                None,
            );
        }
    };

    let workspace = crate::cockpit_sources::familiar_workspace(coven_home, familiar_id);
    let config = match ward::WardConfig::load(&workspace) {
        Ok(Some(config)) => config,
        Ok(None) => {
            return api_error(
                409,
                "ward_not_configured",
                "This familiar has no ward.toml; the daemon refuses unwarded writes \
                 into a familiar home. Declare the familiar's surface in ward.toml first.",
                Some(json!({
                    "id": familiar_id,
                    "workspace": workspace.display().to_string(),
                })),
            );
        }
        Err(error) => {
            return api_error(500, "ward_config_invalid", &format!("{error:#}"), None);
        }
    };
    let ward = match ward::Ward::new(&workspace, config.clone()) {
        Ok(ward) => ward,
        Err(error) => {
            return api_error(500, "ward_config_invalid", &format!("{error:#}"), None);
        }
    };

    // The coven-threads gate (Phase 2, OpenCoven/coven-threads §5): protected
    // (Tier 0) targets are validated against the familiar's weave — the typed
    // authority state of each surface — before the Ward's own apply boundary.
    // Editable-tier targets stay the Ward tiers' lane. Adjudication is pure
    // (`Ward::evaluate`), so resolving targets here does not write anything.
    let adjudication = ward.evaluate(&ward::Proposal {
        targets: edits.iter().map(|e| e.target.clone()).collect(),
        authorization: authorization.clone(),
    });
    // A proposal with any Blocked target (traversal/symlink escape, case
    // collision, unauthorized Tier-0) is refused as a unit BEFORE the threads
    // gate runs: a blocked target must never ride into a staged proposal, and
    // 403 here matches Ward::apply's own all-or-nothing refusal shape.
    if adjudication.is_blocked() {
        let report = ward.apply(&edits, &authorization)?;
        let changes: Vec<Value> = report.changes.iter().map(ward_change_json).collect();
        return api_error(
            403,
            "ward_refused",
            "The Ward refused the proposal; nothing was written.",
            Some(json!({ "changes": changes })),
        );
    }
    let gated_targets: Vec<String> = adjudication
        .decisions
        .iter()
        .filter(|d| d.tier == ward::Tier::Protected && !d.verdict.is_blocked())
        .map(|d| d.resolved.clone())
        .collect();
    let gate_report = {
        let conn = store::open_store(&store_path(coven_home))?;
        match crate::threads_gate::gate_protected_edits(
            &conn,
            &crate::threads_gate::GateRequest {
                coven_home,
                familiar_id,
                workspace: &workspace,
                config: &config,
                edits: &edits,
                gated_targets: &gated_targets,
                authorization: &authorization,
            },
        ) {
            Ok(report) => report,
            Err(error) => {
                // Fail closed: a gate that cannot run is a refusal, never a
                // pass-through (RFC-0001 §5.4 Gate 4).
                return api_error(
                    500,
                    "threads_gate_unavailable",
                    &format!("The authority gate could not adjudicate the proposal: {error:#}"),
                    None,
                );
            }
        }
    };
    match &gate_report.outcome {
        crate::threads_gate::GateOutcome::Rejected => {
            return api_error(
                403,
                "ward_refused",
                "The authority gate rejected the proposal; nothing was written.",
                Some(json!({ "threadsGate": gate_report.to_json() })),
            );
        }
        crate::threads_gate::GateOutcome::Staged { .. } => {
            // §5 DegradeToProposal: staged at ~/.coven/pending/, principal
            // notified via the pending file + audit ledger; no write happens.
            return json_response(
                202,
                &json!({
                    "ok": true,
                    "disposition": "staged",
                    "threadsGate": gate_report.to_json(),
                }),
            );
        }
        crate::threads_gate::GateOutcome::Permitted => {}
    }

    let report = ward.apply(&edits, &authorization)?;
    let changes: Vec<Value> = report.changes.iter().map(ward_change_json).collect();
    let threads_gate_json = gate_report.to_json();
    if report.is_refused() {
        return api_error(
            403,
            "ward_refused",
            "The Ward refused the proposal; nothing was written.",
            Some(json!({ "changes": changes, "threadsGate": threads_gate_json })),
        );
    }
    if report.is_held() {
        // Gate 3 (docs/design/ward-gate3-coherence.md G3.1): a proposal held
        // *solely* for Tier-1 coherence review is staged for the principal
        // instead of dead-ending. Any authorized-protected hold keeps the
        // plain `held` shape — mixed proposals stay all-or-nothing.
        // Stage only when every verdict is cleared-or-coherence — i.e. the
        // *only* hold reason is Tier-1 review. Anything else (authorized
        // protected changes today, future verdicts by default) keeps the
        // plain held shape: fail closed toward the authority lane.
        let coherence_only = report.changes.iter().all(|change| {
            matches!(
                change.decision.verdict,
                ward::Verdict::Allow
                    | ward::Verdict::AllowWithLog
                    | ward::Verdict::RequiresCoherenceReview
            )
        });
        if coherence_only {
            let conn = store::open_store(&store_path(coven_home))?;
            let (pending_path, proposal_id) = crate::threads_gate::stage_coherence_proposal(
                &conn,
                coven_home,
                familiar_id,
                &workspace,
                &config,
                &edits,
                &authorization,
            )?;
            return json_response(
                202,
                &json!({
                    "ok": true,
                    "disposition": "staged",
                    "reviewKind": "coherence",
                    "proposalId": proposal_id,
                    "pendingPath": pending_path.display().to_string(),
                    "changes": changes,
                    "threadsGate": threads_gate_json,
                }),
            );
        }
        return json_response(
            202,
            &json!({
                "ok": true,
                "disposition": "held",
                "changes": changes,
                "threadsGate": threads_gate_json,
            }),
        );
    }
    advance_applied_protected_baselines(coven_home, familiar_id, &workspace, &report.changes)?;
    json_response(
        200,
        &json!({
            "ok": true,
            "disposition": "applied",
            "changes": changes,
            "threadsGate": threads_gate_json,
        }),
    )
}

fn advance_applied_protected_baselines(
    coven_home: &Path,
    familiar_id: &str,
    workspace: &Path,
    changes: &[ward::AppliedChange],
) -> Result<()> {
    let protected: Vec<String> = changes
        .iter()
        .filter(|change| {
            change.disposition == ward::Disposition::Applied
                && change.decision.tier == ward::Tier::Protected
        })
        .map(|change| change.decision.resolved.clone())
        .collect();
    if protected.is_empty() {
        return Ok(());
    }
    let conn = store::open_store(&store_path(coven_home))?;
    for surface in protected {
        crate::threads_gate::advance_surface_baseline(&conn, familiar_id, workspace, &surface)?;
    }
    Ok(())
}

/// `GET /familiars/{id}/ward` — the declared Ward surface for one familiar.
///
/// Read-only observability twin of [`apply_familiar_edits`]: it loads the same
/// `ward.toml` the write path enforces and reports the tiers as adjudicated —
/// no separate source of truth. Fail-closed shapes mirror the write path: an
/// unknown familiar and a missing `ward.toml` are structured 404s, an invalid
/// config is a 500 (`ward_config_invalid`), never a silent default.
fn familiar_ward_response(coven_home: &Path, familiar_id: &str) -> Result<ApiResponse> {
    if familiar_id.is_empty() || familiar_id.contains('/') {
        return api_error(
            400,
            "invalid_request",
            "Familiar id is required and must not contain '/'.",
            None,
        );
    }
    let known = crate::cockpit_sources::read_familiars(coven_home)?
        .into_iter()
        .any(|familiar| familiar.id == familiar_id);
    if !known {
        return api_error(
            404,
            "familiar_not_found",
            "No familiar with that id is declared in familiars.toml.",
            Some(json!({ "id": familiar_id })),
        );
    }
    let workspace = crate::cockpit_sources::familiar_workspace(coven_home, familiar_id);
    let config = match ward::WardConfig::load(&workspace) {
        Ok(Some(config)) => config,
        Ok(None) => {
            return api_error(
                404,
                "ward_not_configured",
                "This familiar has no ward.toml; the Ward-enforced write path is unavailable.",
                Some(json!({
                    "id": familiar_id,
                    "workspace": workspace.to_string_lossy(),
                })),
            );
        }
        Err(error) => {
            return api_error(500, "ward_config_invalid", &format!("{error:#}"), None);
        }
    };
    json_response(
        200,
        &json!({
            "ok": true,
            "familiarId": familiar_id,
            "workspace": workspace.to_string_lossy(),
            "ward": {
                "principalKeyFingerprint": config.principal_key_fingerprint,
                "defaultTier": config.default_tier,
                "surface": config.surface,
                "protectedSurface": config.protected_surface,
            },
        }),
    )
}

const DEGRADED_WARD_CONFIG_UNPARSEABLE: &str = "ward-config-unparseable";

/// `GET /api/v1/threads/proposals[/:id]` — the pending-proposal read surface
/// (Gate 3 PR 3, `docs/design/ward-gate3-coherence.md` G3.3).
///
/// Lists what is waiting at `~/.coven/pending/` for the principal: both
/// authority-lane (Tier-0 `DegradeToProposal`) and coherence-lane (Tier-1
/// hold) proposals, distinguished by `reviewKind` (absent in a staged file ⇒
/// `authority`). A missing directory is an empty list; an unreadable or
/// corrupt pending file is reported as a `degraded` entry rather than
/// aborting the fleet read (same posture as `/threads/weaves`).
fn threads_proposals_response(coven_home: &Path, id: Option<&str>) -> Result<ApiResponse> {
    let pending_dir = coven_home.join("pending");
    let entries = match fs::read_dir(&pending_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return match id {
                None => json_response(200, &json!({ "proposals": [] })),
                Some(id) => api_error(
                    404,
                    "proposal_not_found",
                    "No pending proposal with that id.",
                    Some(json!({ "id": id })),
                ),
            }
        }
        Err(err) => return Err(err).with_context(|| format!("reading {}", pending_dir.display())),
    };

    let mut proposals = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(raw) = fs::read_to_string(&path).ok() else {
            proposals.push(json!({
                "degraded": { "file": file_name, "reason": "proposal-unparseable" },
            }));
            continue;
        };
        let Some(raw_value) = serde_json::from_str::<Value>(&raw).ok() else {
            proposals.push(json!({
                "degraded": { "file": file_name, "reason": "proposal-unparseable" },
            }));
            continue;
        };
        let mut proposal_value = raw_value.clone();
        if let Some(object) = proposal_value.as_object_mut() {
            object.remove("decisionRequest");
            object.remove("decisionState");
        }
        let phase5_shape = is_phase5_proposal_shape(&proposal_value);
        let scheduled = if phase5_shape {
            match serde_json::from_value::<crate::proposal_scheduler::ScheduledProposal>(
                proposal_value.clone(),
            ) {
                Ok(scheduled) => Some(scheduled),
                Err(_) => {
                    proposals.push(json!({
                        "degraded": { "file": file_name, "reason": "proposal-unparseable" },
                    }));
                    continue;
                }
            }
        } else {
            None
        };
        let legacy = if scheduled.is_none() {
            match serde_json::from_value::<coven_threads_core::PendingProposal>(
                proposal_value.clone(),
            ) {
                Ok(proposal) => Some(proposal),
                Err(_) => {
                    proposals.push(json!({
                        "degraded": { "file": file_name, "reason": "proposal-unparseable" },
                    }));
                    continue;
                }
            }
        } else {
            None
        };
        let proposal = scheduled
            .as_ref()
            .map(crate::proposal_scheduler::ScheduledProposal::pending)
            .or(legacy.as_ref())
            .expect("scheduled or legacy proposal parsed");
        let Some(familiar_id) = human_familiar_id_for_weave(coven_home, proposal.familiar_id)?
        else {
            // A proposal whose familiar vanished from familiars.toml cannot
            // be decided; degrade it instead of emitting familiarId: null.
            proposals.push(json!({
                "degraded": { "file": file_name, "reason": "proposal-familiar-missing" },
            }));
            continue;
        };
        let targets: Vec<String> = proposal
            .edits
            .iter()
            .map(|edit| edit.surface.as_str().to_string())
            .collect();
        let format = time::format_description::well_known::Rfc3339;
        let mut view = json!({
            "proposalId": proposal.id.0.to_string(),
            "familiarId": familiar_id,
            "familiarUuid": proposal.familiar_id.0.to_string(),
            "writer": proposal.writer.as_str(),
            "stagedAt": proposal.staged_at.format(&format).ok(),
            "targets": targets,
            "proposalRevision": proposal_revision(&proposal_value)?,
        });
        if let Some(scheduled) = &scheduled {
            let approval_path = coven_threads_core::ApprovalPathWireEnvelope::from_classification(
                scheduled.classification(),
                Some(proposal.staged_at),
            )
            .map_err(anyhow::Error::msg)
            .context("building approval path wire envelope")?;
            let affected_regions: Vec<&str> = scheduled
                .classification()
                .affected_regions
                .iter()
                .map(coven_threads_core::SurfaceRegionId::as_str)
                .collect();
            let view = view
                .as_object_mut()
                .expect("proposal view is always a JSON object");
            view.insert(
                "approvalPath".to_string(),
                serde_json::to_value(approval_path)?,
            );
            let lifecycle = serde_json::to_value(scheduled.lifecycle())?;
            view.insert(
                "lifecycle".to_string(),
                lifecycle.get("state").cloned().unwrap_or(Value::Null),
            );
            if let Some(reason) = lifecycle.get("reason") {
                view.insert("blockedReason".to_string(), reason.clone());
            }
            view.insert(
                "earliestClose".to_string(),
                json!(scheduled
                    .earliest_close()
                    .and_then(|value| value.format(&format).ok())),
            );
            view.insert("affectedRegions".to_string(), json!(affected_regions));
        } else {
            let review_kind = raw_value
                .get("reviewKind")
                .and_then(Value::as_str)
                .unwrap_or("authority");
            view.as_object_mut()
                .expect("proposal view is always a JSON object")
                .insert("reviewKind".to_string(), json!(review_kind));
        }
        proposals.push(view);
    }

    match id {
        None => {
            // Deterministic order for scripts: newest first, ties broken by
            // proposal id, degraded entries (no stagedAt) last.
            proposals.sort_by_cached_key(|value| {
                let staged = value["stagedAt"].as_str().map(str::to_owned);
                let id = value["proposalId"].as_str().unwrap_or_default().to_owned();
                (std::cmp::Reverse(staged), id)
            });
            json_response(200, &json!({ "proposals": proposals }))
        }
        Some(id) => match proposals
            .into_iter()
            .find(|proposal| proposal["proposalId"] == id)
        {
            Some(proposal) => json_response(200, &json!({ "proposal": proposal })),
            None => api_error(
                404,
                "proposal_not_found",
                "No pending proposal with that id.",
                Some(json!({ "id": id })),
            ),
        },
    }
}

fn threads_weaves_response(coven_home: &Path) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    let mut entries = Vec::new();
    for familiar in crate::cockpit_sources::read_familiars(coven_home)? {
        let workspace = crate::cockpit_sources::familiar_workspace(coven_home, &familiar.id);
        let config = match ward::WardConfig::load(&workspace) {
            Ok(Some(config)) => config,
            Ok(None) => continue,
            Err(error) => {
                let message = format!(
                    "threads/weaves: skipping familiar {} because ward config failed to load: {error:#}",
                    familiar.id
                );
                eprintln!("coven daemon: {message}");
                crate::daemon::append_daemon_recovery_log(coven_home, &message);
                entries.push(json!({
                    "degraded": {
                        "familiarId": familiar.id,
                        "reason": DEGRADED_WARD_CONFIG_UNPARSEABLE,
                        "error": sanitize_ward_config_error(&error, &workspace),
                    }
                }));
                continue;
            }
        };
        let state = crate::threads_gate::build_weave_state(
            &conn,
            &familiar.id,
            &workspace,
            &config,
            &[],
            false,
        )?;
        let coherence = state.weave.coherence();
        let mut weave = serde_json::to_value(state.weave.to_record())
            .context("serializing weave record for Cave threads response")?;
        if let Some(obj) = weave.as_object_mut() {
            obj.insert("familiar_id".to_string(), json!(familiar.id));
        }
        entries.push(json!({ "weave": weave, "coherence": coherence }));
    }
    json_response(200, &entries)
}

fn sanitize_ward_config_error(error: &anyhow::Error, workspace: &Path) -> String {
    let ward_path = workspace.join(ward::WARD_CONFIG_FILE);
    let mut sanitized = format!("{error:#}");
    let ward_path_display = ward_path.display().to_string();
    if !ward_path_display.is_empty() {
        sanitized = sanitized.replace(&ward_path_display, ward::WARD_CONFIG_FILE);
    }
    let workspace_display = workspace.display().to_string();
    if !workspace_display.is_empty() {
        sanitized = sanitized.replace(&workspace_display, ".");
    }
    sanitized.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_phase5_proposal_shape(value: &Value) -> bool {
    [
        "schema",
        "pending",
        "classification",
        "materialized_diff",
        "region_evidence",
        "lifecycle",
        "veto_deadline",
        "earliest_close",
    ]
    .iter()
    .any(|field| value.get(field).is_some())
}

struct ProposalDecisionSemantics {
    rejection_event: coven_threads_core::AuditEventType,
    rejection_decision: &'static str,
    approval_path_label: String,
    window_close: Option<coven_threads_core::ProposalWindowCloseAuditDetail>,
}

fn proposal_decision_semantics(
    scheduled: Option<&crate::proposal_scheduler::ScheduledProposal>,
    decision: &str,
    note: Option<&str>,
    now: time::OffsetDateTime,
) -> std::result::Result<ProposalDecisionSemantics, &'static str> {
    let Some(scheduled) = scheduled else {
        return Ok(ProposalDecisionSemantics {
            rejection_event: coven_threads_core::AuditEventType::ProposalRejected,
            rejection_decision: "rejected",
            approval_path_label: "human_review".to_string(),
            window_close: None,
        });
    };
    let path = &scheduled.classification().approval_path;
    let approval_path_label = path.display_label().to_string();
    match (decision, path) {
        ("approve", coven_threads_core::ApprovalPath::HumanApproval) => {}
        ("approve", coven_threads_core::ApprovalPath::HumanApprovalWithRationale) => {
            if note.is_none_or(|note| note.trim().is_empty()) {
                return Err("proposal-rationale-required");
            }
        }
        (
            "approve",
            coven_threads_core::ApprovalPath::AutoRegression { veto: Some(_) }
            | coven_threads_core::ApprovalPath::FamiliarCoherence { .. },
        ) => {
            if scheduled.earliest_close().is_some_and(|close| now < close) {
                return Err("proposal-minimum-visibility-open");
            }
            if scheduled
                .veto_deadline()
                .is_some_and(|deadline| now < deadline)
            {
                return Err("proposal-veto-window-open");
            }
        }
        ("approve", coven_threads_core::ApprovalPath::AutoRegression { veto: None }) => {}
        (
            "reject",
            coven_threads_core::ApprovalPath::AutoRegression { veto: Some(_) }
            | coven_threads_core::ApprovalPath::FamiliarCoherence { .. },
        ) => {
            if scheduled
                .veto_deadline()
                .is_some_and(|deadline| now >= deadline)
            {
                return Err("proposal-veto-window-closed");
            }
            return Ok(ProposalDecisionSemantics {
                rejection_event: coven_threads_core::AuditEventType::ProposalVetoed,
                rejection_decision: "vetoed",
                approval_path_label,
                window_close: Some(coven_threads_core::ProposalWindowCloseAuditDetail {
                    reason: coven_threads_core::WindowCloseReason::Vetoed,
                    replay_hash_matched: None,
                    rationale: note.map(str::to_string),
                }),
            });
        }
        (
            "reject",
            coven_threads_core::ApprovalPath::HumanApproval
            | coven_threads_core::ApprovalPath::HumanApprovalWithRationale,
        ) => {
            return Ok(ProposalDecisionSemantics {
                rejection_event: coven_threads_core::AuditEventType::ProposalRejected,
                rejection_decision: "rejected",
                approval_path_label,
                window_close: None,
            });
        }
        ("reject", coven_threads_core::ApprovalPath::AutoRegression { veto: None }) => {
            return Err("proposal-not-human-decidable");
        }
        _ => return Err("proposal-decision-invalid"),
    }

    let window_close =
        scheduled
            .veto_deadline()
            .map(|_| coven_threads_core::ProposalWindowCloseAuditDetail {
                reason: coven_threads_core::WindowCloseReason::Applied,
                replay_hash_matched: Some(true),
                rationale: note.map(str::to_string),
            });
    Ok(ProposalDecisionSemantics {
        rejection_event: coven_threads_core::AuditEventType::ProposalRejected,
        rejection_decision: "rejected",
        approval_path_label,
        window_close,
    })
}

fn revalidate_scheduled_materialized_before(
    workspace: &Path,
    scheduled: &crate::proposal_scheduler::ScheduledProposal,
) -> std::result::Result<(), &'static str> {
    for surface in scheduled.materialized_diff().surfaces() {
        let Some(expected_before) = surface.before.as_deref() else {
            return Err("proposal-atomic-create-unsupported");
        };
        let current = crate::threads_gate::read_surface(workspace, surface.surface.as_str())
            .map_err(|_| "proposal-evidence-replay-failed")?;
        if current != expected_before {
            return Err("proposal-evidence-diverged");
        }
    }
    Ok(())
}

fn proposal_revision(authority_value: &Value) -> Result<String> {
    fn canonicalize(value: &Value) -> Value {
        match value {
            Value::Array(values) => Value::Array(values.iter().map(canonicalize).collect()),
            Value::Object(values) => {
                let sorted: std::collections::BTreeMap<_, _> = values
                    .iter()
                    .map(|(key, value)| (key.clone(), canonicalize(value)))
                    .collect();
                serde_json::to_value(sorted).expect("canonical JSON map is serializable")
            }
            value => value.clone(),
        }
    }

    let bytes = serde_json::to_vec(&canonicalize(authority_value))
        .context("serializing canonical proposal revision authority")?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn ward_tier_number(tier: ward::Tier) -> u8 {
    match tier {
        ward::Tier::Protected => 0,
        ward::Tier::Reviewed => 1,
        ward::Tier::Logged => 2,
        ward::Tier::Free => 3,
    }
}

pub(crate) fn decide_threads_proposal(
    coven_home: &Path,
    proposal_id: &str,
    decision: &str,
    body: Option<&str>,
) -> Result<ApiResponse> {
    decide_threads_proposal_inner(coven_home, proposal_id, decision, body, true)
}

fn decide_threads_proposal_automatic(
    coven_home: &Path,
    proposal_id: &str,
    decision: &str,
    body: Option<&str>,
) -> Result<ApiResponse> {
    decide_threads_proposal_inner(coven_home, proposal_id, decision, body, false)
}

fn decide_threads_proposal_inner(
    coven_home: &Path,
    proposal_id: &str,
    decision: &str,
    body: Option<&str>,
    revision_required: bool,
) -> Result<ApiResponse> {
    let _decision_guard = proposal_decision_lock()
        .lock()
        .map_err(|_| anyhow::anyhow!("proposal decision lock poisoned"))?;
    let proposal_uuid = match Uuid::parse_str(proposal_id) {
        Ok(uuid) => uuid,
        Err(_) => {
            return json_response(
                404,
                &json!({ "blocked": true, "why": "proposal-not-found" }),
            )
        }
    };
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(_) => return json_response(400, &json!({ "blocked": true, "why": "invalid-json" })),
    };
    let note = match payload.get("note") {
        None | Some(Value::Null) => None,
        Some(Value::String(note)) => Some(note.clone()),
        Some(_) => return json_response(400, &json!({ "blocked": true, "why": "invalid-note" })),
    };
    let expected_revision = match payload.get("expectedRevision") {
        None | Some(Value::Null) => None,
        Some(Value::String(revision))
            if revision.len() == 64 && revision.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
        {
            Some(revision.to_ascii_lowercase())
        }
        Some(_) => {
            return json_response(
                400,
                &json!({ "blocked": true, "why": "invalid-proposal-revision" }),
            )
        }
    };
    let conn = store::open_store(&store_path(coven_home))?;
    if let Some(terminal) = proposal_terminal_event(&conn, proposal_id)? {
        let matches_request = matches!(
            (decision, terminal.event_type.as_str()),
            ("approve", "proposal_approved")
                | ("reject", "proposal_rejected")
                | ("reject", "proposal_vetoed")
        );
        cleanup_terminal_proposal_artifacts(coven_home, proposal_uuid)?;
        if !matches_request {
            return json_response(
                409,
                &json!({
                    "blocked": true,
                    "why": "proposal-already-decided",
                    "eventType": terminal.event_type,
                }),
            );
        }
        return json_response(
            200,
            &json!({
                "ok": true,
                "decision": terminal_decision_label(&terminal.event_type),
                "proposalId": proposal_id,
                "filesTouched": terminal.files_touched,
                "idempotent": true,
            }),
        );
    }
    let mut claim = match PendingDecisionClaim::acquire(
        coven_home,
        proposal_uuid,
        decision,
        note.as_deref(),
        expected_revision.as_deref(),
        revision_required,
    ) {
        Ok(Some(claim)) => claim,
        Ok(None) => {
            return json_response(
                404,
                &json!({ "blocked": true, "why": "proposal-not-found" }),
            )
        }
        Err(error)
            if error
                .chain()
                .any(|cause| cause.downcast_ref::<serde_json::Error>().is_some()) =>
        {
            return json_response(409, &json!({ "blocked": true, "why": "proposal-corrupt" }))
        }
        Err(error) if error.to_string().contains("already claimed") => {
            return json_response(
                409,
                &json!({ "blocked": true, "why": "proposal-decision-in-progress" }),
            )
        }
        Err(error) if error.downcast_ref::<ProposalRevisionMismatch>().is_some() => {
            return json_response(
                409,
                &json!({ "blocked": true, "why": "proposal-revision-mismatch" }),
            )
        }
        Err(error) => return Err(error),
    };
    if claim.recovery {
        claim.preserve();
    }
    let raw = fs::read_to_string(&claim.path)
        .with_context(|| format!("reading proposal decision claim {}", claim.path.display()))?;
    let mut raw_value: Value = match serde_json::from_str(&raw) {
        Ok(value) => value,
        Err(_) => {
            return json_response(409, &json!({ "blocked": true, "why": "proposal-corrupt" }))
        }
    };
    let durable_request = proposal_decision_request(&raw_value)?;
    maybe_fail_proposal_decision(
        ProposalDecisionFailpoint::ClaimBeforeValidation,
        proposal_id,
    )?;
    if durable_request
        .as_ref()
        .is_some_and(|request| request.decision != decision)
    {
        return json_response(
            409,
            &json!({ "blocked": true, "why": "proposal-decision-conflict" }),
        );
    }
    if let Some(request) = durable_request.as_ref() {
        if note.is_some() && note != request.rationale {
            return json_response(
                409,
                &json!({ "blocked": true, "why": "proposal-recovery-request-conflict" }),
            );
        }
        if expected_revision.is_some() && expected_revision != request.expected_revision {
            return json_response(
                409,
                &json!({ "blocked": true, "why": "proposal-recovery-request-conflict" }),
            );
        }
    }
    let note = durable_request
        .as_ref()
        .map(|request| request.rationale.clone())
        .unwrap_or(note);
    let applying_state = proposal_applying_state(&raw_value)?;
    if applying_state.is_some() {
        claim.preserve();
    }
    let mut authority_value = raw_value.clone();
    if let Some(object) = authority_value.as_object_mut() {
        object.remove("decisionState");
        object.remove("decisionRequest");
    }
    let phase5_shape = is_phase5_proposal_shape(&authority_value);
    let actual_revision = proposal_revision(&authority_value)?;
    if let Some(request) = durable_request.as_ref() {
        if request
            .expected_revision
            .as_ref()
            .is_some_and(|expected| expected != &actual_revision)
        {
            if applying_state.is_none() {
                claim.restore_pending(&mut raw_value)?;
            }
            return json_response(
                409,
                &json!({ "blocked": true, "why": "proposal-revision-mismatch" }),
            );
        }
        if phase5_shape && request.revision_required && request.expected_revision.is_none() {
            if applying_state.is_none() {
                claim.restore_pending(&mut raw_value)?;
            }
            return json_response(
                409,
                &json!({ "blocked": true, "why": "proposal-revision-required" }),
            );
        }
    }
    let scheduled = if phase5_shape {
        match serde_json::from_value::<crate::proposal_scheduler::ScheduledProposal>(
            authority_value.clone(),
        ) {
            Ok(scheduled) => Some(scheduled),
            Err(_) => {
                return json_response(409, &json!({ "blocked": true, "why": "proposal-corrupt" }))
            }
        }
    } else {
        None
    };
    let pending: coven_threads_core::PendingProposal = match &scheduled {
        Some(scheduled) => scheduled.pending().clone(),
        None => match serde_json::from_value(authority_value.clone()) {
            Ok(pending) => pending,
            Err(_) => {
                return json_response(409, &json!({ "blocked": true, "why": "proposal-corrupt" }))
            }
        },
    };
    if pending.id.0 != proposal_uuid {
        return json_response(409, &json!({ "blocked": true, "why": "proposal-corrupt" }));
    }
    let note = if let Some(applying) = applying_state.as_ref() {
        if applying.decision != decision {
            return json_response(
                409,
                &json!({ "blocked": true, "why": "proposal-decision-conflict" }),
            );
        }
        if note != applying.rationale {
            return json_response(
                409,
                &json!({ "blocked": true, "why": "proposal-recovery-request-conflict" }),
            );
        }
        applying.rationale.clone()
    } else {
        note
    };
    let decision_semantics = match proposal_decision_semantics(
        scheduled.as_ref(),
        decision,
        note.as_deref(),
        durable_request
            .as_ref()
            .map(|request| request.claimed_at)
            .unwrap_or_else(time::OffsetDateTime::now_utc),
    ) {
        Ok(semantics) => semantics,
        Err(reason) => {
            if applying_state.is_none() {
                claim.restore_pending(&mut raw_value)?;
            }
            return json_response(
                409,
                &json!({
                    "blocked": true,
                    "why": reason,
                    "proposalId": proposal_id,
                }),
            );
        }
    };
    let Some(familiar_id) = human_familiar_id_for_weave(coven_home, pending.familiar_id)? else {
        return json_response(
            409,
            &json!({ "blocked": true, "why": "proposal-familiar-missing" }),
        );
    };
    let workspace = crate::cockpit_sources::familiar_workspace(coven_home, &familiar_id);
    let Some(config) = ward::WardConfig::load(&workspace)? else {
        return json_response(
            409,
            &json!({ "blocked": true, "why": "ward-not-configured" }),
        );
    };
    let authorization = authorization_from_writer(&pending.writer);
    let edits = match staged_edits_to_ward_edits(&pending) {
        Ok(edits) => edits,
        Err(_) => {
            return json_response(409, &json!({ "blocked": true, "why": "proposal-corrupt" }))
        }
    };
    let targets: Vec<String> = edits.iter().map(|edit| edit.target.clone()).collect();
    let ward = ward::Ward::new(&workspace, config.clone())?;
    let adjudication = ward.evaluate(&ward::Proposal {
        targets: targets.clone(),
        authorization: authorization.clone(),
    });
    if let Some(terminal) = proposal_terminal_event(&conn, proposal_id)? {
        let matches_request = matches!(
            (decision, terminal.event_type.as_str()),
            ("approve", "proposal_approved")
                | ("reject", "proposal_rejected")
                | ("reject", "proposal_vetoed")
        );
        claim.consume()?;
        if !matches_request {
            return json_response(
                409,
                &json!({
                    "blocked": true,
                    "why": "proposal-already-decided",
                    "eventType": terminal.event_type,
                }),
            );
        }
        return json_response(
            200,
            &json!({
                "ok": true,
                "decision": terminal_decision_label(&terminal.event_type),
                "proposalId": proposal_id,
                "filesTouched": terminal.files_touched,
                "idempotent": true,
            }),
        );
    }
    if let Some(applying) = applying_state.as_ref() {
        let recorded_intent = load_proposal_apply_intent(&conn, proposal_id)?;
        if recorded_intent.as_ref() != Some(applying) {
            return json_response(
                409,
                &json!({
                    "blocked": true,
                    "why": "proposal-recovery-intent-unverifiable",
                    "proposalId": proposal_id,
                }),
            );
        }
        let recovery_commitment =
            proposal_recovery_commitment(&conn, &config, &authority_value, &familiar_id, &targets)?;
        if applying.recovery_commitment != recovery_commitment {
            return json_response(
                409,
                &json!({
                    "blocked": true,
                    "why": "proposal-recovery-evidence-diverged",
                    "proposalId": proposal_id,
                }),
            );
        }
        if adjudication.is_blocked() {
            return json_response(
                409,
                &json!({
                    "blocked": true,
                    "why": "proposal-recovery-revalidation-failed",
                    "proposalId": proposal_id,
                }),
            );
        }
    }
    if scheduled.is_some() && applying_state.is_none() && adjudication.is_blocked() {
        let state = crate::threads_gate::build_weave_state_for_writer(
            &conn,
            &familiar_id,
            &workspace,
            &config,
            &[],
            false,
            Some(&pending.writer),
        )?;
        claim.preserve();
        append_proposal_decision_audit(
            &conn,
            ProposalDecisionAudit {
                event_type: coven_threads_core::AuditEventType::ProposalRejected,
                proposal_id,
                familiar_id: &familiar_id,
                weave_hash: state.weave.weave_hash(),
                approver: Some(&pending.writer),
                files_touched: &targets,
                decision: "proposal-live-adjudication-failed",
                approval_rationale: note.as_deref(),
                approval_path_label: &decision_semantics.approval_path_label,
                window_close: None,
                channel: pending.channel,
            },
        )?;
        claim.consume()?;
        return json_response(
            409,
            &json!({
                "blocked": true,
                "why": "proposal-live-adjudication-failed",
                "proposalId": proposal_id,
            }),
        );
    }
    if adjudication.is_blocked() {
        let state = crate::threads_gate::build_weave_state(
            &conn,
            &familiar_id,
            &workspace,
            &config,
            &[],
            false,
        )?;
        append_proposal_refusal_audit(
            &conn,
            proposal_id,
            &familiar_id,
            state.weave.weave_hash(),
            &pending.writer,
            &targets,
            pending.channel,
        )?;
        claim.restore_pending(&mut raw_value)?;
        return json_response(
            409,
            &json!({ "blocked": true, "why": "proposal-revalidation-failed" }),
        );
    }
    let gated_targets: Vec<String> = adjudication
        .decisions
        .iter()
        .filter(|d| {
            !d.verdict.is_blocked() && (scheduled.is_some() || d.tier == ward::Tier::Protected)
        })
        .map(|d| d.resolved.clone())
        .collect();
    let state = if scheduled.is_some() {
        crate::threads_gate::build_weave_state_for_writer(
            &conn,
            &familiar_id,
            &workspace,
            &config,
            &gated_targets,
            false,
            Some(&pending.writer),
        )?
    } else {
        crate::threads_gate::build_weave_state(
            &conn,
            &familiar_id,
            &workspace,
            &config,
            &gated_targets,
            false,
        )?
    };
    if decision == "approve" && gated_targets.is_empty() {
        append_proposal_refusal_audit(
            &conn,
            proposal_id,
            &familiar_id,
            state.weave.weave_hash(),
            &pending.writer,
            &targets,
            pending.channel,
        )?;
        claim.restore_pending(&mut raw_value)?;
        return json_response(
            409,
            &json!({ "blocked": true, "why": "proposal-revalidation-failed" }),
        );
    }

    if decision == "reject" {
        claim.preserve();
        append_proposal_decision_audit(
            &conn,
            ProposalDecisionAudit {
                event_type: decision_semantics.rejection_event,
                proposal_id,
                familiar_id: &familiar_id,
                weave_hash: state.weave.weave_hash(),
                approver: Some(&pending.writer),
                files_touched: &targets,
                decision: decision_semantics.rejection_decision,
                approval_rationale: note.as_deref(),
                approval_path_label: &decision_semantics.approval_path_label,
                window_close: decision_semantics.window_close.as_ref(),
                channel: pending.channel,
            },
        )?;
        maybe_fail_proposal_decision(ProposalDecisionFailpoint::AuditBeforeCleanup, proposal_id)?;
        claim.consume()?;
        return json_response(
            200,
            &json!({
                "ok": true,
                "decision": decision_semantics.rejection_decision,
                "proposalId": proposal_id,
                "filesTouched": targets,
                "note": note,
            }),
        );
    }

    if applying_state.is_none() {
        if let Some(scheduled) = scheduled.as_ref() {
            let live_tier_escalated = adjudication.decisions.iter().any(|decision| {
                ward_tier_number(decision.tier) < scheduled.classification().path_tier_floor
            });
            let rejection = if live_tier_escalated {
                Some("proposal-live-tier-escalated")
            } else {
                revalidate_scheduled_materialized_before(&workspace, scheduled).err()
            };
            if let Some(reason) = rejection {
                claim.preserve();
                append_proposal_decision_audit(
                    &conn,
                    ProposalDecisionAudit {
                        event_type: coven_threads_core::AuditEventType::ProposalRejected,
                        proposal_id,
                        familiar_id: &familiar_id,
                        weave_hash: state.weave.weave_hash(),
                        approver: Some(&pending.writer),
                        files_touched: &targets,
                        decision: reason,
                        approval_rationale: note.as_deref(),
                        approval_path_label: &decision_semantics.approval_path_label,
                        window_close: None,
                        channel: pending.channel,
                    },
                )?;
                claim.consume()?;
                return json_response(
                    409,
                    &json!({
                        "blocked": true,
                        "why": reason,
                        "proposalId": proposal_id,
                    }),
                );
            }
        }
    }

    if !ward::supports_atomic_approved_writes() {
        return json_response(
            409,
            &json!({
                "blocked": true,
                "why": "proposal-atomic-commit-unsupported",
                "proposalId": proposal_id,
            }),
        );
    }
    if let Some(applying) = applying_state {
        if let Err(error) = verify_recoverable_apply_state(&workspace, &pending, &applying) {
            return json_response(
                409,
                &json!({
                    "blocked": true,
                    "why": "proposal-recovery-surface-diverged",
                    "proposalId": proposal_id,
                    "error": error.to_string(),
                }),
            );
        }
        let expected_before = proposal_expected_before(&applying)?;
        let recovery_authorization = recovery_authorization(proposal_id, &authorization);
        if !ward_config_is_unchanged(&workspace, &config)? {
            return json_response(
                409,
                &json!({
                    "blocked": true,
                    "why": "proposal-recovery-evidence-diverged",
                    "proposalId": proposal_id,
                }),
            );
        }
        let report =
            ward.apply_after_threads_approval(&edits, &recovery_authorization, &expected_before)?;
        if report.is_refused() {
            let rollback_edits = proposal_rollback_edits(&applying)?;
            let expected_after = proposal_expected_after(&edits);
            let rollback = ward.apply_after_threads_approval(
                &rollback_edits,
                &authorization,
                &expected_after,
            )?;
            if rollback.is_refused() {
                anyhow::bail!(
                    "Ward refused both recovery and restoration for proposal {proposal_id}"
                );
            }
            append_proposal_refusal_audit(
                &conn,
                proposal_id,
                &familiar_id,
                &applying.weave_hash,
                &pending.writer,
                &targets,
                pending.channel,
            )?;
            claim.restore_pending(&mut raw_value)?;
            return json_response(
                409,
                &json!({
                    "blocked": true,
                    "why": "proposal-recovery-revalidation-failed",
                    "proposalId": proposal_id,
                }),
            );
        }
        let approved_bytes = approved_bytes_by_resolved(&report, &edits)?;
        finalize_approved_proposal(
            &conn,
            ApprovedProposalFinalization {
                proposal_id,
                familiar_id: &familiar_id,
                workspace: &workspace,
                weave_hash: &applying.weave_hash,
                approver: &pending.writer,
                gated_targets: &gated_targets,
                approved_bytes: &approved_bytes,
                files_touched: &targets,
                rationale: applying.rationale.as_deref(),
                approval_path_label: &decision_semantics.approval_path_label,
                window_close: decision_semantics.window_close.as_ref(),
                channel: pending.channel,
            },
        )?;
        maybe_fail_proposal_decision(ProposalDecisionFailpoint::AuditBeforeCleanup, proposal_id)?;
        claim.consume()?;
        return json_response(
            200,
            &json!({
                "ok": true,
                "decision": "approved",
                "proposalId": proposal_id,
                "filesTouched": targets,
                "recovered": true,
            }),
        );
    }

    for target in &gated_targets {
        let request = coven_threads_core::MutationRequest {
            surface: coven_threads_core::SurfaceId::new(target.clone()),
            writer: pending.writer.clone(),
            channel: pending.channel,
            identity_context: None,
        };
        let verdict = coven_threads_core::validate_fail_closed(&state.weave, &request);
        crate::threads_gate::append_audit_row(
            &conn,
            &familiar_id,
            &state.familiar_uuid,
            state.weave.weave_hash(),
            &request,
            &verdict,
            time::OffsetDateTime::now_utc(),
        )?;
        if !verdict.permits_write() {
            append_proposal_refusal_audit(
                &conn,
                proposal_id,
                &familiar_id,
                state.weave.weave_hash(),
                &pending.writer,
                &targets,
                pending.channel,
            )?;
            return json_response(
                409,
                &json!({
                    "blocked": true,
                    "why": "proposal-revalidation-failed",
                    "proposalId": proposal_id,
                    "verdict": verdict,
                }),
            );
        }
    }

    let applying = ProposalApplyingState {
        decision: "approve".to_string(),
        recovery_commitment: proposal_recovery_commitment(
            &conn,
            &config,
            &authority_value,
            &familiar_id,
            &targets,
        )?,
        weave_hash: state.weave.weave_hash().to_vec(),
        before_images: proposal_before_images(&workspace, &targets, scheduled.as_ref())?,
        rationale: note.clone(),
    };
    append_proposal_apply_intent(
        &conn,
        proposal_id,
        &familiar_id,
        &pending.writer,
        &targets,
        &applying,
        pending.channel,
    )?;
    persist_proposal_applying_state(&claim.path, &mut raw_value, &applying)?;
    claim.preserve();
    if !ward_config_is_unchanged(&workspace, &config)? {
        return json_response(
            409,
            &json!({
                "blocked": true,
                "why": "proposal-recovery-evidence-diverged",
                "proposalId": proposal_id,
            }),
        );
    }
    let expected_before = proposal_expected_before(&applying)?;
    let report = ward.apply_after_threads_approval(&edits, &authorization, &expected_before)?;
    if report.is_refused() {
        append_proposal_refusal_audit(
            &conn,
            proposal_id,
            &familiar_id,
            state.weave.weave_hash(),
            &pending.writer,
            &targets,
            pending.channel,
        )?;
        claim.restore_pending(&mut raw_value)?;
        return json_response(
            409,
            &json!({ "blocked": true, "why": "proposal-revalidation-failed" }),
        );
    }
    let approved_bytes = approved_bytes_by_resolved(&report, &edits)?;
    maybe_fail_proposal_decision(ProposalDecisionFailpoint::ApplyBeforeAudit, proposal_id)?;
    finalize_approved_proposal(
        &conn,
        ApprovedProposalFinalization {
            proposal_id,
            familiar_id: &familiar_id,
            workspace: &workspace,
            weave_hash: state.weave.weave_hash(),
            approver: &pending.writer,
            gated_targets: &gated_targets,
            approved_bytes: &approved_bytes,
            files_touched: &targets,
            rationale: note.as_deref(),
            approval_path_label: &decision_semantics.approval_path_label,
            window_close: decision_semantics.window_close.as_ref(),
            channel: pending.channel,
        },
    )?;
    maybe_fail_proposal_decision(ProposalDecisionFailpoint::AuditBeforeCleanup, proposal_id)?;
    claim.consume()?;
    json_response(
        200,
        &json!({
            "ok": true,
            "decision": "approved",
            "proposalId": proposal_id,
            "filesTouched": targets,
            "note": note,
        }),
    )
}

fn find_pending_proposal(coven_home: &Path, proposal_id: Uuid) -> Result<Option<PathBuf>> {
    let pending_dir = coven_home.join("pending");
    let entries = match fs::read_dir(&pending_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("reading {}", pending_dir.display())),
    };
    let needle = proposal_id.to_string();
    for entry in entries {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.contains(&needle) {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

pub(crate) fn process_due_threads_proposals(coven_home: &Path) -> Result<usize> {
    let pending_dir = coven_home.join("pending");
    let entries = match fs::read_dir(&pending_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("reading proposal scheduler {}", pending_dir.display()))
        }
    };
    let mut claims = Vec::new();
    let mut scheduled = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.ends_with(".deciding") {
            claims.push(path);
        } else if name.ends_with(".json") {
            scheduled.push(path);
        }
    }
    claims.sort();
    scheduled.sort();

    let mut completed = 0;
    for claim_path in claims {
        match recover_proposal_claim(coven_home, &claim_path) {
            Ok(true) => completed += 1,
            Ok(false) => {}
            Err(error) => crate::daemon::append_daemon_recovery_log(
                coven_home,
                &format!(
                    "threads scheduler: claim recovery failed for {}: {error:#}",
                    claim_path.display()
                ),
            ),
        }
    }

    let now = time::OffsetDateTime::now_utc();
    for path in scheduled {
        let result = (|| -> Result<bool> {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("reading scheduled proposal {}", path.display()))?;
            let mut value: Value = serde_json::from_str(&raw)
                .with_context(|| format!("parsing scheduled proposal {}", path.display()))?;
            let request = proposal_decision_request(&value)?;
            if let Some(object) = value.as_object_mut() {
                object.remove("decisionRequest");
                object.remove("decisionState");
            }
            let proposal: crate::proposal_scheduler::ScheduledProposal =
                serde_json::from_value(value)
                    .with_context(|| format!("parsing scheduled proposal {}", path.display()))?;
            if let Some(request) = request {
                let body = request
                    .rationale
                    .map(|note| json!({ "note": note }).to_string());
                let response = decide_threads_proposal_automatic(
                    coven_home,
                    &proposal.pending().id.0.to_string(),
                    &request.decision,
                    body.as_deref(),
                )?;
                return Ok(response.status == 200);
            }
            ensure_proposal_window_opened_audit(coven_home, &proposal)?;
            let due = match &proposal.classification().approval_path {
                coven_threads_core::ApprovalPath::AutoRegression { veto: None } => true,
                coven_threads_core::ApprovalPath::AutoRegression { veto: Some(_) }
                | coven_threads_core::ApprovalPath::FamiliarCoherence { .. } => proposal
                    .veto_deadline()
                    .is_some_and(|deadline| now >= deadline),
                coven_threads_core::ApprovalPath::HumanApproval
                | coven_threads_core::ApprovalPath::HumanApprovalWithRationale => false,
            };
            if !due {
                return Ok(false);
            }
            let response = decide_threads_proposal_automatic(
                coven_home,
                &proposal.pending().id.0.to_string(),
                "approve",
                None,
            )?;
            Ok(response.status == 200)
        })();
        match result {
            Ok(true) => completed += 1,
            Ok(false) => {}
            Err(error) => crate::daemon::append_daemon_recovery_log(
                coven_home,
                &format!(
                    "threads scheduler: scheduled proposal failed for {}: {error:#}",
                    path.display()
                ),
            ),
        }
    }
    Ok(completed)
}

fn recover_proposal_claim(coven_home: &Path, claim_path: &Path) -> Result<bool> {
    let name = claim_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("proposal claim filename is not UTF-8")?;
    let (_, suffix) = name
        .rsplit_once(".json.")
        .context("proposal claim filename lacks decision suffix")?;
    let decision = suffix
        .strip_suffix(".deciding")
        .filter(|decision| matches!(*decision, "approve" | "reject"))
        .context("proposal claim has unknown decision")?;
    let raw = fs::read_to_string(claim_path)
        .with_context(|| format!("reading proposal claim {}", claim_path.display()))?;
    let value: Value = serde_json::from_str(&raw).context("parsing proposal recovery claim")?;
    let applying = proposal_applying_state(&value)?;
    let request = proposal_decision_request(&value)?;
    let mut authority = value;
    if let Some(object) = authority.as_object_mut() {
        object.remove("decisionState");
        object.remove("decisionRequest");
    }
    let proposal_id = if is_phase5_proposal_shape(&authority) {
        serde_json::from_value::<crate::proposal_scheduler::ScheduledProposal>(authority)?
            .pending()
            .id
            .0
    } else {
        serde_json::from_value::<coven_threads_core::PendingProposal>(authority)?
            .id
            .0
    };
    let body = applying
        .and_then(|state| state.rationale)
        .or_else(|| request.and_then(|request| request.rationale))
        .map(|note| json!({ "note": note }).to_string());
    let response = decide_threads_proposal_automatic(
        coven_home,
        &proposal_id.to_string(),
        decision,
        body.as_deref(),
    )?;
    Ok(response.status == 200)
}

fn ensure_proposal_window_opened_audit(
    coven_home: &Path,
    proposal: &crate::proposal_scheduler::ScheduledProposal,
) -> Result<()> {
    let (Some(deadline), Some(earliest_close)) =
        (proposal.veto_deadline(), proposal.earliest_close())
    else {
        return Ok(());
    };
    let pending = proposal.pending();
    let Some(familiar_id) = human_familiar_id_for_weave(coven_home, pending.familiar_id)? else {
        anyhow::bail!("scheduled proposal familiar is missing");
    };
    let workspace = crate::cockpit_sources::familiar_workspace(coven_home, &familiar_id);
    let config =
        ward::WardConfig::load(&workspace)?.context("scheduled proposal Ward is not configured")?;
    let targets: Vec<String> = pending
        .edits
        .iter()
        .map(|edit| edit.surface.as_str().to_string())
        .collect();
    let conn = store::open_store(&store_path(coven_home))?;
    let state = crate::threads_gate::build_weave_state_for_writer(
        &conn,
        &familiar_id,
        &workspace,
        &config,
        &targets,
        false,
        Some(&pending.writer),
    )?;
    let detail = coven_threads_core::ProposalWindowAuditDetail {
        approval_path_label: proposal
            .classification()
            .approval_path
            .display_label()
            .to_string(),
        deadline,
        earliest_close,
        evidence_replay_hash_hex: proposal
            .classification()
            .evidence_replay_hash
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        affected_regions: proposal
            .classification()
            .affected_regions
            .iter()
            .map(|region| region.as_str().to_string())
            .collect(),
    };
    let detail = serde_json::to_string(&detail)?;
    let files_touched = serde_json::to_string(&targets)?;
    let submitted_at = pending
        .staged_at
        .format(&time::format_description::well_known::Rfc3339)?;
    let decided_at =
        time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?;
    conn.execute(
        "INSERT INTO ward_audit (
            event_type, proposal_id, familiar_id, ward_hash, decision, approver,
            detail, files_touched, channel, submitted_at, decided_at
         )
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11
         WHERE NOT EXISTS (
            SELECT 1 FROM ward_audit WHERE proposal_id = ?2 AND event_type = ?1
         )",
        rusqlite::params![
            coven_threads_core::AuditEventType::ProposalWindowOpened.tag(),
            pending.id.0.to_string(),
            familiar_id,
            state.weave.weave_hash(),
            "window-opened",
            pending.writer.as_str(),
            detail,
            files_touched,
            format!("{:?}", pending.channel).to_lowercase(),
            submitted_at,
            decided_at,
        ],
    )
    .context("appending proposal_window_opened audit")?;
    Ok(())
}

struct PendingDecisionClaim {
    path: PathBuf,
    original_path: PathBuf,
    preserve: bool,
    recovery: bool,
}

#[derive(Debug)]
struct ProposalRevisionMismatch;

impl std::fmt::Display for ProposalRevisionMismatch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("proposal revision does not match inspected revision")
    }
}

impl std::error::Error for ProposalRevisionMismatch {}

impl PendingDecisionClaim {
    fn acquire(
        coven_home: &Path,
        proposal_id: Uuid,
        decision: &str,
        rationale: Option<&str>,
        expected_revision: Option<&str>,
        revision_required: bool,
    ) -> Result<Option<Self>> {
        if let Some((path, claimed_decision)) =
            find_any_pending_decision_claim(coven_home, &proposal_id.to_string())
        {
            if claimed_decision != decision {
                anyhow::bail!("proposal is already claimed for {claimed_decision}, not {decision}");
            }
            let suffix = format!(".{decision}.deciding");
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .context("proposal decision claim has a non-utf8 filename")?;
            let original_name = file_name
                .strip_suffix(&suffix)
                .context("proposal decision claim has an invalid suffix")?;
            return Ok(Some(Self {
                original_path: path.with_file_name(original_name),
                path,
                preserve: false,
                recovery: true,
            }));
        }

        let Some(original_path) = find_pending_proposal(coven_home, proposal_id)? else {
            return Ok(None);
        };
        let file_name = original_path
            .file_name()
            .and_then(|name| name.to_str())
            .context("pending proposal has a non-utf8 filename")?;
        let path = original_path.with_file_name(format!("{file_name}.{decision}.deciding"));
        let raw = fs::read_to_string(&original_path)
            .with_context(|| format!("reading pending proposal {}", original_path.display()))?;
        let mut value: Value =
            serde_json::from_str(&raw).context("parsing pending proposal before decision claim")?;
        if proposal_decision_request(&value)?.is_none() {
            let mut authority_value = value.clone();
            if let Some(object) = authority_value.as_object_mut() {
                object.remove("decisionState");
                object.remove("decisionRequest");
            }
            if let Some(expected_revision) = expected_revision {
                if proposal_revision(&authority_value)? != expected_revision {
                    return Err(ProposalRevisionMismatch.into());
                }
            }
            value
                .as_object_mut()
                .context("pending proposal must be a JSON object")?
                .insert(
                    "decisionRequest".to_string(),
                    serde_json::to_value(ProposalDecisionRequest {
                        decision: decision.to_string(),
                        rationale: rationale.map(str::to_string),
                        claimed_at: time::OffsetDateTime::now_utc(),
                        expected_revision: expected_revision.map(str::to_string),
                        revision_required,
                    })?,
                );
        }
        persist_proposal_claim_value(&original_path, &value)?;
        fs::rename(&original_path, &path).with_context(|| {
            format!(
                "claiming pending proposal {} as {}",
                original_path.display(),
                path.display()
            )
        })?;
        Ok(Some(Self {
            path,
            original_path,
            preserve: false,
            recovery: false,
        }))
    }

    fn preserve(&mut self) {
        self.preserve = true;
    }

    fn consume(&mut self) -> Result<()> {
        fs::remove_file(&self.path)
            .with_context(|| format!("removing proposal decision claim {}", self.path.display()))?;
        self.preserve = true;
        Ok(())
    }

    fn restore_pending(&mut self, raw_value: &mut Value) -> Result<()> {
        raw_value
            .as_object_mut()
            .context("pending proposal claim must be a JSON object")?
            .remove("decisionState");
        raw_value
            .as_object_mut()
            .context("pending proposal claim must be a JSON object")?
            .remove("decisionRequest");
        persist_proposal_claim_value(&self.path, raw_value)?;
        fs::rename(&self.path, &self.original_path).with_context(|| {
            format!(
                "restoring proposal decision claim {} to {}",
                self.path.display(),
                self.original_path.display()
            )
        })?;
        self.preserve = true;
        Ok(())
    }
}

impl Drop for PendingDecisionClaim {
    fn drop(&mut self) {
        if !self.preserve && self.path.exists() {
            let _ = fs::rename(&self.path, &self.original_path);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProposalDecisionRequest {
    decision: String,
    rationale: Option<String>,
    claimed_at: time::OffsetDateTime,
    #[serde(default)]
    expected_revision: Option<String>,
    #[serde(default)]
    revision_required: bool,
}

fn proposal_decision_request(raw_value: &Value) -> Result<Option<ProposalDecisionRequest>> {
    raw_value
        .get("decisionRequest")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .context("proposal decision request is corrupt")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProposalApplyingState {
    decision: String,
    recovery_commitment: Vec<u8>,
    weave_hash: Vec<u8>,
    before_images: Vec<ProposalBeforeImage>,
    rationale: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProposalBeforeImage {
    target: String,
    contents: coven_threads_core::StagedContents,
}

fn proposal_recovery_commitment(
    conn: &rusqlite::Connection,
    config: &ward::WardConfig,
    authority_value: &Value,
    familiar_id: &str,
    targets: &[String],
) -> Result<Vec<u8>> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"coven:proposal-decision-recovery:v2");
    let config_bytes = serde_json::to_vec(config).context("serializing Ward config")?;
    hasher.update(&(config_bytes.len() as u64).to_be_bytes());
    hasher.update(&config_bytes);
    let authority_bytes =
        serde_json::to_vec(authority_value).context("serializing proposal authority envelope")?;
    hasher.update(&(authority_bytes.len() as u64).to_be_bytes());
    hasher.update(&authority_bytes);
    let mut targets = targets.to_vec();
    targets.sort();
    for target in targets {
        hasher.update(&(target.len() as u64).to_be_bytes());
        hasher.update(target.as_bytes());
        let baseline = crate::threads_gate::load_baseline(conn, familiar_id, &target)?;
        match baseline {
            Some(bytes) => {
                hasher.update(&[1]);
                hasher.update(&(bytes.len() as u64).to_be_bytes());
                hasher.update(&bytes);
            }
            None => {
                hasher.update(&[0]);
            }
        };
    }
    Ok(hasher.finalize().as_bytes().to_vec())
}

fn proposal_before_images(
    workspace: &Path,
    targets: &[String],
    scheduled: Option<&crate::proposal_scheduler::ScheduledProposal>,
) -> Result<Vec<ProposalBeforeImage>> {
    targets
        .iter()
        .map(|target| {
            let contents = if let Some(scheduled) = scheduled {
                scheduled
                    .materialized_diff()
                    .for_surface(&coven_threads_core::SurfaceId::new(target))
                    .with_context(|| {
                        format!("scheduled materialized diff is missing target {target}")
                    })?
                    .before
                    .clone()
                    .with_context(|| {
                        format!("scheduled target {target} has no approved before-image")
                    })?
            } else {
                crate::threads_gate::read_surface(workspace, target)?
            };
            Ok(ProposalBeforeImage {
                target: target.clone(),
                contents: coven_threads_core::StagedContents::from_bytes(&contents),
            })
        })
        .collect()
}

fn proposal_expected_before(applying: &ProposalApplyingState) -> Result<BTreeMap<String, Vec<u8>>> {
    applying
        .before_images
        .iter()
        .map(|before| {
            Ok((
                before.target.clone(),
                before.contents.to_bytes().map_err(anyhow::Error::msg)?,
            ))
        })
        .collect()
}

fn proposal_expected_after(edits: &[ward::FileEdit]) -> BTreeMap<String, Vec<u8>> {
    edits
        .iter()
        .map(|edit| (edit.target.clone(), edit.new_contents.clone()))
        .collect()
}

fn approved_bytes_by_resolved(
    report: &ward::ApplyReport,
    edits: &[ward::FileEdit],
) -> Result<BTreeMap<String, Vec<u8>>> {
    if report.changes.len() != edits.len() {
        anyhow::bail!("Ward apply report length does not match approved edits");
    }
    report
        .changes
        .iter()
        .zip(edits)
        .map(|(change, edit)| Ok((change.decision.resolved.clone(), edit.new_contents.clone())))
        .collect()
}

fn proposal_rollback_edits(applying: &ProposalApplyingState) -> Result<Vec<ward::FileEdit>> {
    applying
        .before_images
        .iter()
        .map(|before| {
            Ok(ward::FileEdit::new(
                before.target.clone(),
                before.contents.to_bytes().map_err(anyhow::Error::msg)?,
            ))
        })
        .collect()
}

fn persist_proposal_applying_state(
    claim_path: &Path,
    raw_value: &mut Value,
    state: &ProposalApplyingState,
) -> Result<()> {
    raw_value
        .as_object_mut()
        .context("pending proposal claim must be a JSON object")?
        .insert(
            "decisionState".to_string(),
            serde_json::to_value(state).context("serializing proposal applying state")?,
        );
    persist_proposal_claim_value(claim_path, raw_value)
}

fn persist_proposal_claim_value(claim_path: &Path, raw_value: &Value) -> Result<()> {
    let body = serde_json::to_vec_pretty(raw_value).context("serializing proposal claim")?;
    let file_name = claim_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("proposal decision claim has a non-utf8 filename")?;
    let staged = claim_path.with_file_name(format!(".{file_name}.{}.staged", Uuid::new_v4()));
    let write_result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&staged)
            .with_context(|| format!("creating proposal claim stage {}", staged.display()))?;
        file.write_all(&body)
            .with_context(|| format!("writing proposal claim stage {}", staged.display()))?;
        file.sync_all()
            .with_context(|| format!("syncing proposal claim stage {}", staged.display()))?;
        fs::rename(&staged, claim_path).with_context(|| {
            format!(
                "committing proposal applying state {}",
                claim_path.display()
            )
        })
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&staged);
    }
    write_result
}

fn proposal_applying_state(raw_value: &Value) -> Result<Option<ProposalApplyingState>> {
    raw_value
        .get("decisionState")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .context("invalid proposal applying state")
}

fn append_proposal_apply_intent(
    conn: &rusqlite::Connection,
    proposal_id: &str,
    familiar_id: &str,
    approver: &coven_threads_core::WriterId,
    files_touched: &[String],
    state: &ProposalApplyingState,
    channel: coven_threads_core::Channel,
) -> Result<()> {
    let detail = serde_json::to_string(state).context("serializing proposal apply intent")?;
    let files_touched = serde_json::to_string(files_touched)?;
    let now =
        time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?;
    conn.execute(
        "INSERT INTO ward_audit (
            event_type, proposal_id, familiar_id, ward_version, ward_hash,
            tier, decision, approver, diff_hash, detail, files_touched,
            channel, thread_id, submitted_at, decided_at
         ) VALUES (
            'validation_verdict', ?1, ?2, NULL, ?3, NULL,
            'proposal-apply-intent', ?4, NULL, ?5, ?6, ?7, NULL, ?8, ?8
         )",
        rusqlite::params![
            proposal_id,
            familiar_id,
            state.weave_hash,
            approver.as_str(),
            detail,
            files_touched,
            format!("{channel:?}").to_lowercase(),
            now,
        ],
    )
    .context("appending proposal apply intent")?;
    Ok(())
}

fn load_proposal_apply_intent(
    conn: &rusqlite::Connection,
    proposal_id: &str,
) -> Result<Option<ProposalApplyingState>> {
    use rusqlite::OptionalExtension;

    let detail: Option<String> = conn
        .query_row(
            "SELECT detail
             FROM ward_audit
             WHERE proposal_id = ?1
               AND event_type = 'validation_verdict'
               AND decision = 'proposal-apply-intent'
             ORDER BY id DESC
             LIMIT 1",
            [proposal_id],
            |row| row.get(0),
        )
        .optional()
        .context("loading proposal apply intent")?;
    detail
        .map(|detail| serde_json::from_str(&detail).context("invalid proposal apply intent"))
        .transpose()
}

fn verify_recoverable_apply_state(
    workspace: &Path,
    pending: &coven_threads_core::PendingProposal,
    applying: &ProposalApplyingState,
) -> Result<()> {
    let staged: std::collections::BTreeMap<&str, Vec<u8>> = pending
        .edits
        .iter()
        .map(|edit| {
            Ok((
                edit.surface.as_str(),
                edit.contents.to_bytes().map_err(anyhow::Error::msg)?,
            ))
        })
        .collect::<Result<_>>()?;
    let mut before_images = std::collections::BTreeMap::new();
    for before in &applying.before_images {
        if before_images
            .insert(before.target.as_str(), &before.contents)
            .is_some()
        {
            anyhow::bail!("duplicate before image for {}", before.target);
        }
    }
    if before_images.keys().copied().collect::<Vec<_>>()
        != staged.keys().copied().collect::<Vec<_>>()
    {
        anyhow::bail!("proposal before images do not match staged targets");
    }
    for before in &applying.before_images {
        let current = crate::threads_gate::read_surface(workspace, &before.target)?;
        let before_bytes = before.contents.to_bytes().map_err(anyhow::Error::msg)?;
        let after_bytes = staged
            .get(before.target.as_str())
            .with_context(|| format!("missing staged contents for {}", before.target))?;
        if current != before_bytes && current != *after_bytes {
            anyhow::bail!(
                "surface {} diverged from both before and staged contents during recovery",
                before.target
            );
        }
    }
    Ok(())
}

struct ProposalTerminalAudit {
    event_type: String,
    files_touched: Vec<String>,
}

fn terminal_decision_label(event_type: &str) -> &'static str {
    match event_type {
        "proposal_approved" => "approved",
        "proposal_vetoed" => "vetoed",
        _ => "rejected",
    }
}

fn proposal_terminal_event(
    conn: &rusqlite::Connection,
    proposal_id: &str,
) -> Result<Option<ProposalTerminalAudit>> {
    use rusqlite::OptionalExtension;

    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT event_type, files_touched
         FROM ward_audit
         WHERE proposal_id = ?1
           AND event_type IN ('proposal_approved', 'proposal_rejected', 'proposal_vetoed')
         ORDER BY id DESC
         LIMIT 1",
            [proposal_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .context("loading proposal terminal audit event")?;
    row.map(|(event_type, files_touched)| {
        Ok(ProposalTerminalAudit {
            event_type,
            files_touched: serde_json::from_str(&files_touched)
                .context("terminal audit files_touched is invalid")?,
        })
    })
    .transpose()
}

fn find_any_pending_decision_claim(
    coven_home: &Path,
    proposal_id: &str,
) -> Option<(PathBuf, String)> {
    let pending_dir = coven_home.join("pending");
    let entries = fs::read_dir(pending_dir).ok()?;
    let marker = format!("-{proposal_id}.json.");
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some((_, suffix)) = name.split_once(&marker) else {
            continue;
        };
        let Some(decision) = suffix.strip_suffix(".deciding").map(str::to_string) else {
            continue;
        };
        if matches!(decision.as_str(), "approve" | "reject") {
            return Some((path, decision));
        }
    }
    None
}

#[cfg(test)]
fn find_pending_decision_claim(
    coven_home: &Path,
    proposal_id: &str,
    decision: &str,
) -> Option<PathBuf> {
    find_any_pending_decision_claim(coven_home, proposal_id)
        .filter(|(_, claimed_decision)| claimed_decision == decision)
        .map(|(path, _)| path)
}

fn cleanup_terminal_proposal_artifacts(coven_home: &Path, proposal_id: Uuid) -> Result<()> {
    if let Some((claim, _)) = find_any_pending_decision_claim(coven_home, &proposal_id.to_string())
    {
        fs::remove_file(&claim)
            .with_context(|| format!("removing terminal proposal claim {}", claim.display()))?;
    }
    if let Some(pending) = find_pending_proposal(coven_home, proposal_id)? {
        fs::remove_file(&pending).with_context(|| {
            format!(
                "removing terminal proposal pending file {}",
                pending.display()
            )
        })?;
    }
    Ok(())
}

fn human_familiar_id_for_weave(
    coven_home: &Path,
    familiar_uuid: coven_threads_core::FamiliarId,
) -> Result<Option<String>> {
    for familiar in crate::cockpit_sources::read_familiars(coven_home)? {
        if crate::threads_gate::familiar_weave_id(&familiar.id) == familiar_uuid {
            return Ok(Some(familiar.id));
        }
    }
    Ok(None)
}

fn authorization_from_writer(writer: &coven_threads_core::WriterId) -> ward::Authorization {
    writer
        .as_str()
        .strip_prefix("principal:")
        .map(|fp| ward::Authorization::signed_by(fp.to_string()))
        .unwrap_or_else(ward::Authorization::unsigned)
}

fn ward_config_is_unchanged(workspace: &Path, expected: &ward::WardConfig) -> Result<bool> {
    Ok(ward::WardConfig::load(workspace)?.as_ref() == Some(expected))
}

fn staged_edits_to_ward_edits(
    pending: &coven_threads_core::PendingProposal,
) -> Result<Vec<ward::FileEdit>> {
    pending
        .edits
        .iter()
        .map(|edit| {
            let bytes = edit
                .contents
                .to_bytes()
                .map_err(|err| anyhow::anyhow!("decoding staged contents: {err}"))?;
            let contents = String::from_utf8(bytes)
                .context("staged proposal contents are not utf8; Ward FileEdit is text-only")?;
            Ok(ward::FileEdit::new(edit.surface.as_str(), contents))
        })
        .collect()
}

fn append_proposal_refusal_audit(
    conn: &rusqlite::Connection,
    proposal_id: &str,
    familiar_id: &str,
    weave_hash: &[u8],
    approver: &coven_threads_core::WriterId,
    files_touched: &[String],
    channel: coven_threads_core::Channel,
) -> Result<()> {
    append_proposal_decision_audit(
        conn,
        ProposalDecisionAudit {
            event_type: coven_threads_core::AuditEventType::ValidationVerdict,
            proposal_id,
            familiar_id,
            weave_hash,
            approver: Some(approver),
            files_touched,
            decision: "proposal-revalidation-failed",
            approval_rationale: None,
            approval_path_label: "human_review",
            window_close: None,
            channel,
        },
    )
}

struct ProposalDecisionAudit<'a> {
    event_type: coven_threads_core::AuditEventType,
    proposal_id: &'a str,
    familiar_id: &'a str,
    weave_hash: &'a [u8],
    approver: Option<&'a coven_threads_core::WriterId>,
    files_touched: &'a [String],
    decision: &'a str,
    approval_rationale: Option<&'a str>,
    approval_path_label: &'a str,
    window_close: Option<&'a coven_threads_core::ProposalWindowCloseAuditDetail>,
    channel: coven_threads_core::Channel,
}

struct ApprovedProposalFinalization<'a> {
    proposal_id: &'a str,
    familiar_id: &'a str,
    workspace: &'a Path,
    weave_hash: &'a [u8],
    approver: &'a coven_threads_core::WriterId,
    gated_targets: &'a [String],
    approved_bytes: &'a BTreeMap<String, Vec<u8>>,
    files_touched: &'a [String],
    rationale: Option<&'a str>,
    approval_path_label: &'a str,
    window_close: Option<&'a coven_threads_core::ProposalWindowCloseAuditDetail>,
    channel: coven_threads_core::Channel,
}

fn finalize_approved_proposal(
    conn: &rusqlite::Connection,
    finalization: ApprovedProposalFinalization<'_>,
) -> Result<()> {
    conn.execute_batch("BEGIN IMMEDIATE")
        .context("starting proposal approval transaction")?;
    let result = (|| -> Result<()> {
        for target in finalization.gated_targets {
            let expected_bytes = finalization
                .approved_bytes
                .get(target)
                .with_context(|| format!("approved target {target} is missing staged contents"))?;
            crate::threads_gate::advance_surface_baseline_from_bytes(
                conn,
                finalization.familiar_id,
                finalization.workspace,
                target,
                expected_bytes,
            )?;
        }
        append_proposal_decision_audit(
            conn,
            ProposalDecisionAudit {
                event_type: coven_threads_core::AuditEventType::ProposalApproved,
                proposal_id: finalization.proposal_id,
                familiar_id: finalization.familiar_id,
                weave_hash: finalization.weave_hash,
                approver: Some(finalization.approver),
                files_touched: finalization.files_touched,
                decision: "approved",
                approval_rationale: finalization.rationale,
                approval_path_label: finalization.approval_path_label,
                window_close: finalization.window_close,
                channel: finalization.channel,
            },
        )?;
        conn.execute_batch("COMMIT")
            .context("committing proposal approval transaction")
    })();
    if result.is_err() {
        let _ = conn.execute_batch("ROLLBACK");
    }
    result
}

fn append_proposal_decision_audit(
    conn: &rusqlite::Connection,
    audit: ProposalDecisionAudit<'_>,
) -> Result<()> {
    let files_touched = serde_json::to_string(audit.files_touched)?;
    let detail = match audit.event_type {
        coven_threads_core::AuditEventType::ProposalApproved => Some(serde_json::to_string(
            &coven_threads_core::ProposalApprovalAuditDetail {
                approval_path_label: audit.approval_path_label.to_string(),
                rationale: audit.approval_rationale.map(str::to_string),
                window_close: audit.window_close.cloned(),
            },
        )?),
        coven_threads_core::AuditEventType::ProposalVetoed => {
            audit.window_close.map(serde_json::to_string).transpose()?
        }
        _ => None,
    };
    let now =
        time::OffsetDateTime::now_utc().format(&time::format_description::well_known::Rfc3339)?;
    conn.execute(
        "INSERT INTO ward_audit (
            event_type, proposal_id, familiar_id, ward_version, ward_hash,
            tier, decision, approver, diff_hash, detail, files_touched,
            channel, thread_id, submitted_at, decided_at
        ) VALUES (?1, ?2, ?3, NULL, ?4, NULL, ?5, ?6, NULL, ?7, ?8, ?9, NULL, ?10, ?10)",
        rusqlite::params![
            audit.event_type.tag(),
            audit.proposal_id,
            audit.familiar_id,
            audit.weave_hash,
            audit.decision,
            audit.approver.map(|w| w.as_str().to_string()),
            detail,
            files_touched,
            format!("{:?}", audit.channel).to_lowercase(),
            now,
        ],
    )
    .context("appending proposal decision to ward_audit")?;
    Ok(())
}

/// Serialize one Ward per-edit outcome for the `/familiars/{id}/edits` response.
fn ward_change_json(change: &crate::ward::AppliedChange) -> Value {
    use crate::ward::{Disposition, Verdict};

    let disposition = match change.disposition {
        Disposition::Applied => "applied",
        Disposition::HeldForCoherence => "held",
        Disposition::Refused => "refused",
    };
    let verdict = match &change.decision.verdict {
        Verdict::Allow => json!({ "kind": "allow" }),
        Verdict::AllowWithLog => json!({ "kind": "allowWithLog" }),
        Verdict::RequiresCoherenceReview => json!({ "kind": "requiresCoherenceReview" }),
        Verdict::AuthorizedProtectedChange => json!({ "kind": "authorizedProtectedChange" }),
        Verdict::Blocked { reason } => {
            json!({ "kind": "blocked", "reason": reason.to_string() })
        }
    };
    let mut value = json!({
        "target": change.decision.target,
        "resolved": change.decision.resolved,
        "tier": u8::from(change.decision.tier),
        "verdict": verdict,
        "disposition": disposition,
    });
    if let Some(audit) = &change.audit {
        value["audit"] = serde_json::to_value(audit).unwrap_or(Value::Null);
    }
    value
}

pub(crate) fn parse_body(body: Option<&str>) -> Result<Value> {
    match body.filter(|body| !body.trim().is_empty()) {
        Some(body) => serde_json::from_str(body).context("failed to parse request body"),
        None => Ok(json!({})),
    }
}

fn split_path_query(path: &str) -> (&str, Option<&str>) {
    match path.split_once('?') {
        Some((route, query)) => (route, Some(query)),
        None => (path, None),
    }
}

pub(crate) fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|part| {
        let (candidate, value) = part.split_once('=')?;
        (candidate == key).then_some(value)
    })
}

fn session_action_id<'a>(path: &'a str, suffix: &str) -> &'a str {
    path.trim_start_matches("/sessions/")
        .strip_suffix(suffix)
        .unwrap_or_default()
}

pub(crate) fn current_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Nanos, true)
}

/// The daemon has no periodic maintenance loop, so the sessions list — the
/// endpoint Cave polls constantly — doubles as the reap tick for rows a dead
/// `coven run` stranded in `created` (#342). Throttled so back-to-back polls
/// don't each pay a write, and best-effort: a failed repair never fails the
/// read it piggybacks on. Startup recovery covers daemons nobody lists.
const STALE_CREATED_REAP_INTERVAL_SECS: u64 = 60;

fn reap_stale_created_sessions_throttled(conn: &rusqlite::Connection) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_REAP_EPOCH_SECS: AtomicU64 = AtomicU64::new(0);

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let last = LAST_REAP_EPOCH_SECS.load(Ordering::Relaxed);
    if now_secs.saturating_sub(last) < STALE_CREATED_REAP_INTERVAL_SECS {
        return;
    }
    if LAST_REAP_EPOCH_SECS
        .compare_exchange(last, now_secs, Ordering::Relaxed, Ordering::Relaxed)
        .is_err()
    {
        // Another connection claimed this tick.
        return;
    }
    let cutoff = (Utc::now() - Duration::seconds(crate::daemon::STALE_CREATED_TTL_SECS))
        .to_rfc3339_opts(SecondsFormat::Nanos, true);
    let _ = store::mark_stale_created_sessions_failed(conn, &cutoff, &current_timestamp());
}

pub(crate) fn api_error(
    status: u16,
    code: &str,
    message: &str,
    details: Option<Value>,
) -> Result<ApiResponse> {
    let mut error = json!({
        "code": code,
        "message": message,
    });
    if let Some(d) = details {
        error["details"] = d;
    }
    json_response(status, &json!({ "error": error }))
}

pub(crate) fn json_response<T: Serialize>(status: u16, body: &T) -> Result<ApiResponse> {
    Ok(ApiResponse {
        status,
        content_type: "application/json",
        body: serde_json::to_string(body).context("failed to serialize API response")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_health_response() {
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
        assert_eq!(response.daemon, None);
        assert_eq!(response.hub, None);
    }

    #[test]
    fn routes_health_request_to_json() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let daemon = DaemonStatus {
            pid: 12345,
            started_at: "2026-04-27T10:00:00Z".to_string(),
            socket: temp_dir
                .path()
                .join("coven.sock")
                .to_string_lossy()
                .into_owned(),
        };

        let response = handle_request("GET", "/health", temp_dir.path(), Some(daemon))?;

        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");
        assert!(response.body.contains(r#""ok":true"#));
        assert!(response.body.contains(r#""apiVersion":"coven.daemon.v1""#));
        assert!(response.body.contains(r#""pid":12345"#));
        assert!(response.body.contains(r#""sessions":true"#));
        assert!(response.body.contains(r#""travel":true"#));
        assert!(response.body.contains(r#""scheduler":true"#));
        assert!(response.body.contains(r#""hub":true"#));
        assert!(response.body.contains(r#""executorDispatch":true"#));
        assert!(response.body.contains(r#""role":"hub""#));
        assert!(response.body.contains(r#""structuredErrors":true"#));
        Ok(())
    }

    #[test]
    fn routes_versioned_health_request_to_named_api_contract() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let response = handle_request("GET", "/api/v1/health", temp_dir.path(), None)?;

        assert_eq!(response.status, 200);
        assert!(response.body.contains(r#""apiVersion":"coven.daemon.v1""#));
        assert!(response.body.contains(r#""covenVersion""#));
        assert!(response.body.contains(r#""capabilities""#));
        assert!(response.body.contains(r#""eventCursor":"sequence""#));
        assert!(response.body.contains(r#""ok":true"#));
        Ok(())
    }

    #[test]
    fn travel_profile_generation_returns_compressed_read_only_profile_metadata(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let response = handle_request_with_body(
            "POST",
            "/api/v1/travel/profiles",
            temp_dir.path(),
            None,
            Some(
                r#"{
                    "familiarId":"sage",
                    "workspaceId":"workspace-1",
                    "expiresInSeconds":604800,
                    "staleAfterSeconds":172800,
                    "includeContext":["memory","workspace","policy"]
                }"#,
            ),
        )?;

        assert_eq!(response.status, 201);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["version"], "0.1");
        assert!(body["profileId"].as_str().unwrap().starts_with("travel_"));
        assert_eq!(body["scope"]["familiarId"], "sage");
        assert_eq!(body["scope"]["workspaceId"], "workspace-1");
        assert_eq!(body["permissions"]["mode"], "travel-read-only");
        assert_eq!(body["permissions"]["allowMemoryOverwrite"], false);
        assert_eq!(body["permissions"]["allowHeavyweightLocalWork"], false);
        assert_eq!(body["encoding"], "gzip+base64");
        assert!(body["profileBlob"].as_str().unwrap().len() > 16);
        assert!(body["contentHash"].as_str().unwrap().starts_with("sha256:"));
        assert!(body["generatedAt"].as_str().unwrap().contains('T'));
        assert!(body["sourceHub"]["hubId"]
            .as_str()
            .unwrap()
            .starts_with("hub_"));
        assert!(body["expiresAt"].as_str().unwrap() > body["generatedAt"].as_str().unwrap());
        assert!(body["staleAfter"].as_str().unwrap() > body["generatedAt"].as_str().unwrap());
        Ok(())
    }

    #[test]
    fn travel_profile_generation_reuses_stable_source_hub_identity() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let first = handle_request_with_body(
            "POST",
            "/api/v1/travel/profiles",
            temp_dir.path(),
            None,
            Some(r#"{"familiarId":"sage","workspaceId":"workspace-1"}"#),
        )?;
        let second = handle_request_with_body(
            "POST",
            "/api/v1/travel/profiles",
            temp_dir.path(),
            None,
            Some(r#"{"familiarId":"sage","workspaceId":"workspace-1"}"#),
        )?;

        let first: serde_json::Value = serde_json::from_str(&first.body)?;
        let second: serde_json::Value = serde_json::from_str(&second.body)?;
        assert_eq!(first["sourceHub"]["hubId"], second["sourceHub"]["hubId"]);
        assert_ne!(first["profileId"], second["profileId"]);
        Ok(())
    }

    #[test]
    fn travel_profile_generation_embeds_familiar_memory_context_and_readonly_artifact(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let memory_dir = temp_dir.path().join("memory").join("sage");
        std::fs::create_dir_all(&memory_dir)?;
        std::fs::write(
            memory_dir.join("field-notes.md"),
            "# Field notes\n\nSage remembers the travel-mode acceptance criteria.",
        )?;

        let response = handle_request_with_body(
            "POST",
            "/api/v1/travel/profiles",
            temp_dir.path(),
            None,
            Some(r#"{"familiarId":"sage","workspaceId":"workspace-1"}"#),
        )?;

        assert_eq!(response.status, 201);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        let profile_id = body["profileId"].as_str().unwrap();
        let blob = body["profileBlob"].as_str().unwrap();
        let compressed = base64::engine::general_purpose::STANDARD.decode(blob)?;
        let mut decoder = flate2::read::GzDecoder::new(&compressed[..]);
        let mut decoded = String::new();
        std::io::Read::read_to_string(&mut decoder, &mut decoded)?;
        let profile: serde_json::Value = serde_json::from_str(&decoded)?;
        assert_eq!(
            profile["payload"]["memoryContext"][0]["path"],
            "sage/field-notes.md"
        );
        assert_eq!(
            profile["payload"]["memoryContext"][0]["excerpt"],
            "Sage remembers the travel-mode acceptance criteria."
        );

        let artifact = temp_dir
            .path()
            .join("travel")
            .join("profiles")
            .join(format!("{profile_id}.json.gz"));
        assert!(artifact.exists(), "missing {}", artifact.display());
        assert!(
            artifact.metadata()?.permissions().readonly(),
            "travel profile artifact should be read-only"
        );
        Ok(())
    }

    #[test]
    fn travel_delta_upload_appends_results_without_overwriting_canonical_memory(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let profile_response = handle_request_with_body(
            "POST",
            "/api/v1/travel/profiles",
            temp_dir.path(),
            None,
            Some(r#"{"familiarId":"sage","workspaceId":"workspace-1"}"#),
        )?;
        let profile: serde_json::Value = serde_json::from_str(&profile_response.body)?;
        let profile_id = profile["profileId"].as_str().unwrap();
        let source_hub_id = profile["sourceHub"]["hubId"].as_str().unwrap();
        let memory_revision = profile["sourceRevision"]["memoryRevision"]
            .as_str()
            .unwrap();
        let loop_revision = profile["sourceRevision"]["loopRevision"].as_str().unwrap();
        let delta_body = serde_json::json!({
            "profileId": profile_id,
            "sourceHubId": source_hub_id,
            "sourceRevision": {
                "memoryRevision": memory_revision,
                "loopRevision": loop_revision
            },
            "clientId": "laptop-1",
            "events": [{"id":"event-1","kind":"assistant","text":"offline result"}],
            "artifacts": [{"id":"artifact-1","kind":"summary"}],
            "proposedMemoryAdditions": [{"path":"MEMORY.md","text":"append this"}],
            "canonicalMemoryOverwrite": {"path":"MEMORY.md","text":"replace everything"}
        });

        let response = handle_request_with_body(
            "POST",
            "/api/v1/travel/deltas",
            temp_dir.path(),
            None,
            Some(&delta_body.to_string()),
        )?;

        assert_eq!(response.status, 202);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert!(body["deltaId"].as_str().unwrap().starts_with("delta_"));
        assert_eq!(body["state"], "hub_resumed");
        assert_eq!(body["acceptedEvents"], 1);
        assert_eq!(body["acceptedArtifacts"], 1);
        assert_eq!(body["memoryReviewState"], "queued");
        assert_eq!(body["canonicalMemoryOverwriteApplied"], false);
        assert!(body["hubRevision"]["memoryRevision"]
            .as_str()
            .unwrap()
            .starts_with("mem_"));
        Ok(())
    }

    #[test]
    fn travel_delta_upload_rejects_expired_profiles() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let profile_response = handle_request_with_body(
            "POST",
            "/api/v1/travel/profiles",
            temp_dir.path(),
            None,
            Some(r#"{"familiarId":"sage","workspaceId":"workspace-1"}"#),
        )?;
        let profile: serde_json::Value = serde_json::from_str(&profile_response.body)?;
        let profile_id = profile["profileId"].as_str().unwrap();
        let conn = store::open_store(&store_path(temp_dir.path()))?;
        conn.execute(
            "UPDATE travel_profiles SET expires_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
            [profile_id],
        )?;
        drop(conn);

        let response = handle_request_with_body(
            "POST",
            "/api/v1/travel/deltas",
            temp_dir.path(),
            None,
            Some(
                &serde_json::json!({
                    "profileId": profile_id,
                    "sourceHubId": profile["sourceHub"]["hubId"],
                    "clientId": "laptop-1",
                    "events": [{"id":"event-1","kind":"assistant","text":"offline result"}]
                })
                .to_string(),
            ),
        )?;

        assert_eq!(response.status, 409);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "travel_profile_expired");
        Ok(())
    }

    #[test]
    fn travel_state_reports_stale_profile_before_handoff() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let profile_response = handle_request_with_body(
            "POST",
            "/api/v1/travel/profiles",
            temp_dir.path(),
            None,
            Some(r#"{"familiarId":"sage","workspaceId":"workspace-1"}"#),
        )?;
        let profile: serde_json::Value = serde_json::from_str(&profile_response.body)?;
        let profile_id = profile["profileId"].as_str().unwrap();
        let conn = store::open_store(&store_path(temp_dir.path()))?;
        conn.execute(
            "UPDATE travel_profiles SET stale_after = '2020-01-01T00:00:00Z' WHERE id = ?1",
            [profile_id],
        )?;
        drop(conn);

        let response = handle_request(
            "GET",
            &format!("/api/v1/travel/state?clientId=laptop-1&profileId={profile_id}"),
            temp_dir.path(),
            None,
        )?;

        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["state"], "travel_stale");
        assert_eq!(body["profileId"], profile_id);
        assert_eq!(body["profileFreshness"], "stale");
        assert_eq!(body["hubReachable"], false);
        Ok(())
    }

    #[test]
    fn travel_state_refuses_local_execution_for_expired_profile() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let profile_response = handle_request_with_body(
            "POST",
            "/api/v1/travel/profiles",
            temp_dir.path(),
            None,
            Some(r#"{"familiarId":"sage","workspaceId":"workspace-1"}"#),
        )?;
        let profile: serde_json::Value = serde_json::from_str(&profile_response.body)?;
        let profile_id = profile["profileId"].as_str().unwrap();
        let conn = store::open_store(&store_path(temp_dir.path()))?;
        conn.execute(
            "UPDATE travel_profiles SET expires_at = '2020-01-01T00:00:00Z' WHERE id = ?1",
            [profile_id],
        )?;
        drop(conn);

        let response = handle_request(
            "GET",
            &format!("/api/v1/travel/state?clientId=laptop-1&profileId={profile_id}"),
            temp_dir.path(),
            None,
        )?;

        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["state"], "travel_stale");
        assert_eq!(body["profileFreshness"], "expired");
        assert_eq!(body["travelExecutionAllowed"], false);
        Ok(())
    }

    #[test]
    fn travel_delta_upload_appends_offline_events_to_canonical_event_log() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let profile_response = handle_request_with_body(
            "POST",
            "/api/v1/travel/profiles",
            temp_dir.path(),
            None,
            Some(r#"{"familiarId":"sage","workspaceId":"workspace-1"}"#),
        )?;
        let profile: serde_json::Value = serde_json::from_str(&profile_response.body)?;
        let response = handle_request_with_body(
            "POST",
            "/api/v1/travel/deltas",
            temp_dir.path(),
            None,
            Some(
                &serde_json::json!({
                    "profileId": profile["profileId"],
                    "sourceHubId": profile["sourceHub"]["hubId"],
                    "clientId": "laptop-1",
                    "events": [
                        {"id":"local-event-1","kind":"assistant","text":"offline result"}
                    ]
                })
                .to_string(),
            ),
        )?;

        assert_eq!(response.status, 202);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        let session_id = body["reconciliationSessionId"].as_str().unwrap();
        let events = handle_request(
            "GET",
            &format!("/api/v1/events?sessionId={session_id}"),
            temp_dir.path(),
            None,
        )?;
        assert_eq!(events.status, 200);
        let events_body: serde_json::Value = serde_json::from_str(&events.body)?;
        assert_eq!(events_body["events"][0]["kind"], "travel.offline_event");
        assert!(events_body["events"][0]["payload_json"]
            .as_str()
            .unwrap()
            .contains("offline result"));
        Ok(())
    }

    #[test]
    fn travel_state_exposes_handoff_states_for_clients() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let initial = handle_request(
            "GET",
            "/api/v1/travel/state?clientId=laptop-1",
            temp_dir.path(),
            None,
        )?;

        assert_eq!(initial.status, 200);
        let body: serde_json::Value = serde_json::from_str(&initial.body)?;
        assert_eq!(body["state"], "hub_active");
        assert_eq!(body["hubReachable"], true);
        assert_eq!(body["pendingDeltaBytes"], 0);

        let profile_response = handle_request_with_body(
            "POST",
            "/api/v1/travel/profiles",
            temp_dir.path(),
            None,
            Some(r#"{"familiarId":"sage","workspaceId":"workspace-1"}"#),
        )?;
        let profile: serde_json::Value = serde_json::from_str(&profile_response.body)?;
        let pending_delta = serde_json::json!({
            "profileId": profile["profileId"],
            "sourceHubId": profile["sourceHub"]["hubId"],
            "sourceRevision": profile["sourceRevision"],
            "clientId": "laptop-1",
            "events": [{"id":"event-1","kind":"assistant","text":"offline result"}]
        });
        let _ = handle_request_with_body(
            "POST",
            "/api/v1/travel/deltas?defer=1",
            temp_dir.path(),
            None,
            Some(&pending_delta.to_string()),
        )?;

        let pending = handle_request(
            "GET",
            "/api/v1/travel/state?clientId=laptop-1",
            temp_dir.path(),
            None,
        )?;

        assert_eq!(pending.status, 200);
        let body: serde_json::Value = serde_json::from_str(&pending.body)?;
        assert_eq!(body["state"], "handoff_pending");
        assert_eq!(body["profileId"], profile["profileId"]);
        assert!(body["pendingDeltaBytes"].as_i64().unwrap() > 0);
        assert_eq!(body["profileFreshness"], "fresh");
        Ok(())
    }

    #[test]
    fn travel_failure_simulation_reconnect_walks_handoff_sync_and_resume_states(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let profile_response = handle_request_with_body(
            "POST",
            "/api/v1/travel/profiles",
            temp_dir.path(),
            None,
            Some(r#"{"familiarId":"sage","workspaceId":"workspace-1"}"#),
        )?;
        let profile: serde_json::Value = serde_json::from_str(&profile_response.body)?;
        let profile_id = profile["profileId"].as_str().unwrap();

        let local = handle_request(
            "GET",
            &format!("/api/v1/travel/state?clientId=laptop-1&profileId={profile_id}"),
            temp_dir.path(),
            None,
        )?;
        let local_body: serde_json::Value = serde_json::from_str(&local.body)?;
        assert_eq!(local_body["state"], "travel_local");
        assert_eq!(local_body["hubReachable"], false);

        let delta = serde_json::json!({
            "profileId": profile["profileId"],
            "sourceHubId": profile["sourceHub"]["hubId"],
            "sourceRevision": profile["sourceRevision"],
            "clientId": "laptop-1",
            "events": [{"id":"event-1","kind":"assistant","text":"offline result"}]
        });
        let handoff = handle_request_with_body(
            "POST",
            "/api/v1/travel/deltas?state=handoff_pending",
            temp_dir.path(),
            None,
            Some(&delta.to_string()),
        )?;
        assert_eq!(handoff.status, 202);
        let handoff_body: serde_json::Value = serde_json::from_str(&handoff.body)?;
        assert_eq!(handoff_body["state"], "handoff_pending");

        let syncing = handle_request_with_body(
            "POST",
            "/api/v1/travel/deltas?state=syncing_delta",
            temp_dir.path(),
            None,
            Some(&delta.to_string()),
        )?;
        assert_eq!(syncing.status, 202);
        let syncing_body: serde_json::Value = serde_json::from_str(&syncing.body)?;
        assert_eq!(syncing_body["state"], "syncing_delta");

        let syncing_state = handle_request(
            "GET",
            "/api/v1/travel/state?clientId=laptop-1",
            temp_dir.path(),
            None,
        )?;
        let syncing_state_body: serde_json::Value = serde_json::from_str(&syncing_state.body)?;
        assert_eq!(syncing_state_body["state"], "syncing_delta");
        assert_eq!(syncing_state_body["hubReachable"], true);

        let resumed = handle_request_with_body(
            "POST",
            "/api/v1/travel/deltas",
            temp_dir.path(),
            None,
            Some(&delta.to_string()),
        )?;
        assert_eq!(resumed.status, 202);
        let resumed_body: serde_json::Value = serde_json::from_str(&resumed.body)?;
        assert_eq!(resumed_body["state"], "hub_resumed");
        Ok(())
    }

    #[test]
    fn scheduler_decision_selects_available_executor_by_capability_and_queue_pressure(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let request = serde_json::json!({
            "jobId": "job-gpu-loop",
            "requiredCapabilities": ["gpu", "long-running-loop"],
            "taskWeight": "heavyweight",
            "travelState": "hub_active",
            "nodes": [
                {
                    "nodeId": "node-stationary",
                    "role": "stationary_executor",
                    "available": true,
                    "capabilities": ["shell"],
                    "queuePressure": 0
                },
                {
                    "nodeId": "node-compute-busy",
                    "role": "compute_executor",
                    "available": true,
                    "capabilities": ["gpu", "long-running-loop"],
                    "queuePressure": 8
                },
                {
                    "nodeId": "node-compute-idle",
                    "role": "compute_executor",
                    "available": true,
                    "capabilities": ["gpu", "long-running-loop"],
                    "queuePressure": 1
                }
            ]
        });

        let response = handle_request_with_body(
            "POST",
            "/api/v1/scheduler/decisions",
            temp_dir.path(),
            None,
            Some(&request.to_string()),
        )?;

        assert_eq!(response.status, 201);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert!(body["decisionId"].as_str().unwrap().starts_with("sched_"));
        assert_eq!(body["jobId"], "job-gpu-loop");
        assert_eq!(body["target"]["role"], "compute_executor");
        assert_eq!(body["target"]["nodeId"], "node-compute-idle");
        assert!(body["reason"]
            .as_str()
            .unwrap()
            .contains("required capability"));
        assert_eq!(
            body["inputs"]["requiredCapabilities"],
            serde_json::json!(["gpu", "long-running-loop"])
        );
        assert_eq!(body["inputs"]["queuePressure"], "low");
        assert_eq!(body["inputs"]["travelState"], "hub_active");

        let persisted = handle_request(
            "GET",
            &format!(
                "/api/v1/scheduler/decisions/{}",
                body["decisionId"].as_str().unwrap()
            ),
            temp_dir.path(),
            None,
        )?;
        assert_eq!(persisted.status, 200);
        assert_eq!(persisted.body, response.body);
        Ok(())
    }

    #[test]
    fn scheduler_decision_rejects_laptop_local_work_when_travel_battery_is_low(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let request = serde_json::json!({
            "jobId": "job-local-notes",
            "requiredCapabilities": ["shell"],
            "taskWeight": "lightweight",
            "travelState": "travel_local",
            "nodes": [
                {
                    "nodeId": "laptop-travel",
                    "role": "laptop_local",
                    "available": true,
                    "capabilities": ["shell"],
                    "queuePressure": 0,
                    "batteryPercent": 9,
                    "powerSource": "battery"
                }
            ]
        });

        let response = handle_request_with_body(
            "POST",
            "/api/v1/scheduler/decisions",
            temp_dir.path(),
            None,
            Some(&request.to_string()),
        )?;

        assert_eq!(response.status, 409);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "no_scheduler_target");
        assert_eq!(body["error"]["details"]["travelState"], "travel_local");
        assert_eq!(body["error"]["details"]["batteryAware"], true);
        Ok(())
    }

    #[test]
    fn scheduler_failure_simulation_redispatches_when_compute_executor_goes_offline_mid_loop(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let request = serde_json::json!({
            "loopId": "loop-gpu",
            "jobId": "job-gpu-loop",
            "currentNodeId": "compute-primary",
            "requiredCapabilities": ["gpu", "long-running-loop"],
            "loopResumable": true,
            "nodes": [
                {
                    "nodeId": "compute-primary",
                    "role": "compute_executor",
                    "available": false,
                    "capabilities": ["gpu", "long-running-loop"],
                    "queuePressure": 3,
                    "queuedJobIds": ["job-gpu-loop"]
                },
                {
                    "nodeId": "compute-fallback",
                    "role": "compute_executor",
                    "available": true,
                    "capabilities": ["gpu", "long-running-loop"],
                    "queuePressure": 1,
                    "queuedJobIds": []
                }
            ]
        });

        let response = handle_request_with_body(
            "POST",
            "/api/v1/scheduler/redispatch",
            temp_dir.path(),
            None,
            Some(&request.to_string()),
        )?;

        assert_eq!(response.status, 202);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["state"], "redispatched");
        assert_eq!(body["loopId"], "loop-gpu");
        assert_eq!(body["target"]["nodeId"], "compute-fallback");
        assert_eq!(body["preservedSubqueue"]["nodeId"], "compute-primary");
        assert_eq!(
            body["preservedSubqueue"]["jobIds"],
            serde_json::json!(["job-gpu-loop"])
        );
        assert!(body["reason"].as_str().unwrap().contains("offline"));
        assert!(body["decisionId"].as_str().unwrap().starts_with("sched_"));
        Ok(())
    }

    #[test]
    fn scheduler_failure_simulation_pauses_when_stationary_executor_goes_offline_without_alternate(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let request = serde_json::json!({
            "loopId": "loop-shell",
            "jobId": "job-shell-loop",
            "currentNodeId": "stationary-primary",
            "requiredCapabilities": ["shell"],
            "loopResumable": false,
            "nodes": [
                {
                    "nodeId": "stationary-primary",
                    "role": "stationary_executor",
                    "available": false,
                    "capabilities": ["shell"],
                    "queuePressure": 2,
                    "queuedJobIds": ["job-shell-loop"]
                }
            ]
        });

        let response = handle_request_with_body(
            "POST",
            "/api/v1/scheduler/redispatch",
            temp_dir.path(),
            None,
            Some(&request.to_string()),
        )?;

        assert_eq!(response.status, 202);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["state"], "paused");
        assert_eq!(body["loopId"], "loop-shell");
        assert_eq!(body["target"]["role"], "paused");
        assert_eq!(body["nodeAvailability"][0]["nodeId"], "stationary-primary");
        assert_eq!(body["nodeAvailability"][0]["available"], false);
        assert_eq!(body["preservedSubqueue"]["nodeId"], "stationary-primary");
        assert_eq!(
            body["preservedSubqueue"]["jobIds"],
            serde_json::json!(["job-shell-loop"])
        );
        Ok(())
    }

    #[test]
    fn scheduler_failure_simulation_persists_loop_state_for_restart_recovery() -> anyhow::Result<()>
    {
        let temp_dir = tempfile::tempdir()?;
        let request = serde_json::json!({
            "loopId": "loop-persistent",
            "jobId": "job-persistent-loop",
            "currentNodeId": "compute-primary",
            "requiredCapabilities": ["gpu"],
            "loopResumable": true,
            "nodes": [
                {
                    "nodeId": "compute-primary",
                    "role": "compute_executor",
                    "available": false,
                    "capabilities": ["gpu"],
                    "queuePressure": 4,
                    "queuedJobIds": ["job-persistent-loop", "job-followup"]
                },
                {
                    "nodeId": "compute-fallback",
                    "role": "compute_executor",
                    "available": true,
                    "capabilities": ["gpu"],
                    "queuePressure": 0,
                    "queuedJobIds": []
                }
            ]
        });
        let redispatch = handle_request_with_body(
            "POST",
            "/api/v1/scheduler/redispatch",
            temp_dir.path(),
            None,
            Some(&request.to_string()),
        )?;
        assert_eq!(redispatch.status, 202);

        let recovered = handle_request(
            "GET",
            "/api/v1/scheduler/loops/loop-persistent",
            temp_dir.path(),
            None,
        )?;

        assert_eq!(recovered.status, 200);
        let body: serde_json::Value = serde_json::from_str(&recovered.body)?;
        assert_eq!(body["loopId"], "loop-persistent");
        assert_eq!(body["state"], "redispatched");
        assert_eq!(body["jobId"], "job-persistent-loop");
        assert!(body["decisionId"].as_str().unwrap().starts_with("sched_"));
        assert_eq!(body["target"]["nodeId"], "compute-fallback");
        assert_eq!(body["preservedSubqueue"]["nodeId"], "compute-primary");
        assert_eq!(
            body["preservedSubqueue"]["jobIds"],
            serde_json::json!(["job-persistent-loop", "job-followup"])
        );
        assert_eq!(body["nodeAvailability"][0]["available"], false);
        Ok(())
    }

    #[test]
    fn routes_store_vacuum_request_to_repair_response() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let home = temp_dir.path();
        let conn = store::open_store(&store_path(home))?;
        store::insert_session(
            &conn,
            &store::SessionRecord {
                id: "session-1".into(),
                project_root: "/repo".into(),
                harness: "codex".into(),
                title: "demo".into(),
                status: "completed".into(),
                exit_code: Some(0),
                archived_at: Some("2026-01-01T00:00:00Z".into()),
                created_at: "2026-01-01T00:00:00Z".into(),
                updated_at: "2026-01-01T00:00:00Z".into(),
                conversation_id: None,
                familiar_id: None,
                labels: Vec::new(),
                visibility: "private".to_string(),
                external: false,
                transcript_path: None,
            },
        )?;
        store::insert_json_event(
            &conn,
            "session-1",
            "output",
            &json!({"text": "phoenix rises"}),
            "2026-01-01T00:00:01Z",
        )?;
        conn.execute(
            "INSERT INTO events_fts(events_fts) VALUES('delete-all')",
            [],
        )?;
        assert!(store::search_events(&conn, "phoenix")?.is_empty());
        drop(conn);

        let response = handle_request("POST", "/api/v1/store/vacuum", home, None)?;

        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["ok"], true);
        assert_eq!(body["eventIndexRebuilt"], true);
        let conn = store::open_store(&store_path(home))?;
        assert_eq!(store::search_events(&conn, "phoenix")?.len(), 1);
        Ok(())
    }

    #[test]
    fn rejects_unknown_api_version_prefixes() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let response = handle_request("GET", "/api/v2/health", temp_dir.path(), None)?;

        assert_eq!(response.status, 404);
        assert!(response.body.contains(r#""code":"invalid_request""#));
        Ok(())
    }

    #[test]
    fn routes_control_capabilities_discovery_to_json() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let response = handle_request("GET", "/api/v1/capabilities", temp_dir.path(), None)?;

        assert_eq!(response.status, 200);
        assert!(response.body.contains(r#""id":"coven.sessions""#));
        assert!(response.body.contains(r#""id":"coven.travel""#));
        assert!(response.body.contains(r#""id":"coven.scheduler""#));
        assert!(response.body.contains(r#""id":"coven.control.actions""#));
        assert!(response.body.contains(r#""id":"desktop.automation""#));
        assert!(response.body.contains(r#""policy":"requiresApproval""#));
        Ok(())
    }

    #[test]
    fn control_action_routes_safe_capability_refresh() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let body = json!({
            "action": "coven.capabilities.refresh",
            "origin": "external-client",
            "intentId": "intent-1"
        })
        .to_string();

        let response = handle_request_with_body(
            "POST",
            "/api/v1/actions",
            temp_dir.path(),
            None,
            Some(&body),
        )?;

        assert_eq!(response.status, 200);
        assert!(response.body.contains(r#""accepted":true"#));
        assert!(response
            .body
            .contains(r#""action":"coven.capabilities.refresh""#));
        assert!(response.body.contains(r#""kind":"capabilities.refreshed""#));
        assert!(response.body.contains(r#""origin":"external-client""#));
        assert!(response.body.contains(r#""intentId":"intent-1""#));
        Ok(())
    }

    #[test]
    fn capabilities_only_advertise_routable_control_actions() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let response = handle_request("GET", "/api/v1/capabilities", temp_dir.path(), None)?;

        assert_eq!(response.status, 200);
        assert!(response
            .body
            .contains(r#""actions":["coven.capabilities.refresh"]"#));
        assert!(!response.body.contains("coven.sessions.launch"));
        assert!(!response.body.contains("desktop.window.focus"));
        Ok(())
    }

    #[test]
    fn routes_harness_capability_aggregate_to_json() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let response = handle_request(
            "GET",
            "/api/v1/capabilities/harnesses",
            temp_dir.path(),
            None,
        )?;

        assert_eq!(response.status, 200);
        assert!(response.body.contains(r#""harness_capabilities""#));
        assert!(response.body.contains(r#""coven_skills""#));
        assert!(response.body.contains(r#""scanned_at""#));
        for harness in ["codex", "claude", "copilot"] {
            assert!(
                response
                    .body
                    .contains(&format!(r#""harness_id":"{harness}""#)),
                "aggregate missing manifest for `{harness}`: {}",
                response.body
            );
        }
        // The bare path stays the control-plane catalog: no harness manifests.
        let catalog = handle_request("GET", "/api/v1/capabilities", temp_dir.path(), None)?;
        assert!(!catalog.body.contains(r#""harness_capabilities""#));
        Ok(())
    }

    #[test]
    fn harness_capability_aggregate_accepts_refresh_query() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let response = handle_request(
            "GET",
            "/api/v1/capabilities/harnesses?refresh=1",
            temp_dir.path(),
            None,
        )?;

        assert_eq!(response.status, 200);
        assert!(response.body.contains(r#""harness_capabilities""#));
        Ok(())
    }

    #[test]
    fn routes_single_harness_capability_manifest() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let response = handle_request("GET", "/api/v1/capabilities/codex", temp_dir.path(), None)?;

        assert_eq!(response.status, 200);
        assert!(response.body.contains(r#""harness_id":"codex""#));
        assert!(response.body.contains(r#""global_instructions""#));
        Ok(())
    }

    #[test]
    fn unknown_harness_capability_manifest_fails_closed_with_structured_error() -> anyhow::Result<()>
    {
        let temp_dir = tempfile::tempdir()?;

        let response =
            handle_request("GET", "/api/v1/capabilities/warlock", temp_dir.path(), None)?;

        assert_eq!(response.status, 404);
        assert!(response.body.contains(r#""code":"harness_not_found""#));
        assert!(response.body.contains(r#""harnessId":"warlock""#));
        Ok(())
    }

    #[test]
    fn malformed_control_actions_fail_closed_with_structured_json() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let malformed = handle_request_with_body(
            "POST",
            "/api/v1/actions",
            temp_dir.path(),
            None,
            Some("{not-json"),
        )?;
        let missing = handle_request_with_body(
            "POST",
            "/api/v1/actions",
            temp_dir.path(),
            None,
            Some(r#"{"origin":"external-client"}"#),
        )?;
        let empty = handle_request_with_body(
            "POST",
            "/api/v1/actions",
            temp_dir.path(),
            None,
            Some(r#"{"action":"   "}"#),
        )?;
        let non_object = handle_request_with_body(
            "POST",
            "/api/v1/actions",
            temp_dir.path(),
            None,
            Some(r#"["not","an","object"]"#),
        )?;

        assert_eq!(malformed.status, 400);
        assert_eq!(missing.status, 400);
        assert_eq!(empty.status, 400);
        assert_eq!(non_object.status, 400);
        assert!(malformed.body.contains(r#""accepted":false"#));
        assert!(malformed.body.contains(r#""action":"(unknown)""#));
        assert!(missing.body.contains("request body requires string field"));
        assert!(empty.body.contains("request body requires string field"));
        assert!(non_object
            .body
            .contains("request body must be a JSON object"));
        Ok(())
    }

    #[test]
    fn control_action_blocks_unknown_actions_before_adapters_run() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let body = json!({
            "action": "desktop.deleteEverything",
            "origin": "external-client"
        })
        .to_string();

        let response = handle_request_with_body(
            "POST",
            "/api/v1/actions",
            temp_dir.path(),
            None,
            Some(&body),
        )?;

        assert_eq!(response.status, 400);
        assert!(response.body.contains(r#""accepted":false"#));
        assert!(response.body.contains("unknown action"));
        Ok(())
    }

    #[test]
    fn routes_sessions_list_and_detail_requests_to_json() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let conn = crate::store::open_store(&temp_dir.path().join("coven.sqlite3"))?;
        let session = crate::store::SessionRecord {
            id: "session-1".to_string(),
            project_root: "/repo".to_string(),
            harness: "codex".to_string(),
            title: "hello from coven".to_string(),
            status: "created".to_string(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-04-27T10:00:00Z".to_string(),
            updated_at: "2026-04-27T10:00:00Z".to_string(),
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        };
        crate::store::insert_session(&conn, &session)?;

        let list = handle_request("GET", "/sessions", temp_dir.path(), None)?;
        let detail = handle_request("GET", "/sessions/session-1", temp_dir.path(), None)?;

        assert_eq!(list.status, 200);
        assert!(list.body.contains(r#""id":"session-1""#));
        assert_eq!(detail.status, 200);
        assert!(detail.body.contains(r#""title":"hello from coven""#));
        Ok(())
    }

    #[test]
    fn returns_not_found_for_unknown_session() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let response = handle_request("GET", "/sessions/missing", temp_dir.path(), None)?;

        assert_eq!(response.status, 404);
        assert!(response.body.contains(r#""code":"session_not_found""#));
        Ok(())
    }

    #[test]
    fn launch_request_invokes_runtime_and_persists_running_session() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("repo");
        let cwd = project_root.join("app");
        std::fs::create_dir_all(&cwd)?;
        let runtime = RecordingRuntime::default();
        let body = json!({
            "projectRoot": project_root,
            "cwd": cwd,
            "harness": "codex",
            "model": "openai/gpt-5.6-sol",
            "prompt": "hello coven",
            "title": "Demo"
        })
        .to_string();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;
        let list = handle_request("GET", "/sessions", temp_dir.path(), None)?;

        assert_eq!(response.status, 201);
        assert!(response.body.contains(r#""status":"running""#));
        assert_eq!(runtime.launches.borrow().len(), 1);
        assert_eq!(runtime.launches.borrow()[0].harness, "codex");
        assert_eq!(
            runtime.launches.borrow()[0].model.as_deref(),
            Some("openai/gpt-5.6-sol")
        );
        assert_eq!(
            runtime.launches.borrow()[0].launch_mode,
            HarnessLaunchMode::Interactive
        );
        assert_eq!(runtime.launches.borrow()[0].prompt, "hello coven");
        assert_eq!(
            runtime.launches.borrow()[0].project_root,
            project::canonical_project_root(&project_root)?.to_string_lossy()
        );
        assert_eq!(
            runtime.launches.borrow()[0].cwd,
            project::resolve_inside_root(
                &project::canonical_project_root(&project_root)?,
                Some(&cwd)
            )?
            .to_string_lossy()
        );
        assert!(list.body.contains(r#""title":"Demo""#));
        Ok(())
    }

    #[test]
    fn launch_request_accepts_non_interactive_mode_for_plain_chat() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&project_root)?;
        let runtime = RecordingRuntime::default();
        let body = json!({
            "projectRoot": project_root,
            "harness": "codex",
            "launchMode": "nonInteractive",
            "prompt": "hello coven"
        })
        .to_string();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;

        assert_eq!(response.status, 201);
        assert_eq!(
            runtime.launches.borrow()[0].launch_mode,
            HarnessLaunchMode::NonInteractive
        );
        assert!(runtime.launches.borrow()[0].model.is_none());
        Ok(())
    }

    #[test]
    fn launch_request_threads_conversation_hint_through_to_runtime() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&project_root)?;
        let runtime = RecordingRuntime::default();
        let body = json!({
            "projectRoot": project_root,
            "harness": "claude",
            "launchMode": "nonInteractive",
            "prompt": "hello",
            "conversation": {"mode": "init", "id": "abc-123"}
        })
        .to_string();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;

        assert_eq!(response.status, 201);
        assert_eq!(
            runtime.launches.borrow()[0].conversation,
            Some(ConversationHint::Init {
                id: "abc-123".to_string()
            })
        );
        Ok(())
    }

    #[test]
    fn launch_request_accepts_resume_conversation_hint() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&project_root)?;
        let runtime = RecordingRuntime::default();
        let body = json!({
            "projectRoot": project_root,
            "harness": "claude",
            "launchMode": "nonInteractive",
            "prompt": "follow up",
            "conversation": {"mode": "resume", "id": "abc-123"}
        })
        .to_string();

        handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;

        assert_eq!(
            runtime.launches.borrow()[0].conversation,
            Some(ConversationHint::Resume {
                id: "abc-123".to_string()
            })
        );
        Ok(())
    }

    #[test]
    fn launch_request_persists_conversation_id_on_the_session_row() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&project_root)?;
        let runtime = RecordingRuntime::default();
        let body = json!({
            "projectRoot": project_root,
            "harness": "claude",
            "launchMode": "nonInteractive",
            "prompt": "hi",
            "conversation": {"mode": "init", "id": "abc-123"},
            "conversationId": "abc-123"
        })
        .to_string();

        handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;

        assert_eq!(
            runtime.launches.borrow()[0].conversation_id.as_deref(),
            Some("abc-123")
        );

        // And it round-trips through the session list payload too.
        let list = handle_request("GET", "/sessions", temp_dir.path(), None)?;
        assert!(
            list.body.contains(r#""conversation_id":"abc-123""#),
            "list response should expose conversation_id, got: {}",
            list.body
        );
        Ok(())
    }

    #[test]
    fn launch_request_treats_missing_conversation_id_as_null() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&project_root)?;
        let runtime = RecordingRuntime::default();
        let body = json!({
            "projectRoot": project_root,
            "harness": "claude",
            "launchMode": "nonInteractive",
            "prompt": "hi"
        })
        .to_string();

        handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;

        assert!(runtime.launches.borrow()[0].conversation_id.is_none());
        Ok(())
    }

    #[test]
    fn launch_request_persists_familiar_id_on_the_session_row() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        seed_familiars_toml(temp_dir.path())?;
        let project_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&project_root)?;
        let runtime = RecordingRuntime::default();
        let body = json!({
            "projectRoot": project_root,
            "harness": "claude",
            "launchMode": "nonInteractive",
            "prompt": "hi",
            "familiarId": "sage"
        })
        .to_string();

        handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;

        assert_eq!(
            runtime.launches.borrow()[0].familiar_id.as_deref(),
            Some("sage")
        );

        // And it round-trips through the session list payload too.
        let list = handle_request("GET", "/sessions", temp_dir.path(), None)?;
        assert!(
            list.body.contains(r#""familiar_id":"sage""#),
            "list response should expose familiar_id, got: {}",
            list.body
        );
        Ok(())
    }

    #[test]
    fn launch_request_rejects_unknown_familiar_id_without_inserting_session() -> anyhow::Result<()>
    {
        let temp_dir = tempfile::tempdir()?;
        seed_familiars_toml(temp_dir.path())?;
        let project_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&project_root)?;
        let runtime = RecordingRuntime::default();
        let body = json!({
            "projectRoot": project_root,
            "harness": "claude",
            "launchMode": "nonInteractive",
            "prompt": "hi",
            "familiarId": "missing"
        })
        .to_string();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;

        assert_eq!(response.status, 400);
        assert!(
            response.body.contains("unknown_familiar"),
            "expected unknown familiar error, got: {}",
            response.body
        );
        assert!(
            runtime.launches.borrow().is_empty(),
            "unknown familiar must not launch a runtime"
        );
        let list = handle_request("GET", "/sessions", temp_dir.path(), None)?;
        assert!(
            list.body == "[]",
            "unknown familiar must not insert a session row, got: {}",
            list.body
        );
        Ok(())
    }

    #[test]
    fn launch_request_reports_familiar_config_errors_without_launching() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        std::fs::write(temp_dir.path().join("familiars.toml"), "[[familiar]\n")?;
        let project_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&project_root)?;
        let runtime = RecordingRuntime::default();
        let body = json!({
            "projectRoot": project_root,
            "harness": "claude",
            "launchMode": "nonInteractive",
            "prompt": "hi",
            "familiarId": "sage"
        })
        .to_string();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;

        assert_eq!(response.status, 500);
        assert!(
            response.body.contains("familiar_lookup_failed"),
            "expected familiar config error, got: {}",
            response.body
        );
        assert!(
            runtime.launches.borrow().is_empty(),
            "malformed familiar config must not launch a runtime"
        );
        Ok(())
    }

    #[test]
    fn conversation_id_rejects_shell_metacharacters() {
        // Ids carrying whitespace or shell metacharacters must be rejected so they
        // can never reach the harness CLI's `--session-id`/`--resume`/`resume` argv,
        // where on Windows a `.cmd` shim would re-parse cmd.exe metacharacters.
        for bad in [
            "not-a-uuid & calc.exe",
            "a|b",
            "a b",
            "$(whoami)",
            "a\"b",
            "a^b",
        ] {
            let payload = json!({"conversation": {"mode": "resume", "id": bad}});
            assert!(
                conversation_from_payload(&payload).is_err(),
                "expected {bad:?} to be rejected"
            );
        }
        // UUIDs and opaque slugs are accepted.
        for good in [
            "11111111-2222-3333-4444-555555555555",
            "abc-123",
            "sess_1.2",
        ] {
            let payload = json!({"conversation": {"mode": "init", "id": good}});
            assert!(
                conversation_from_payload(&payload)
                    .expect("shell-safe id should parse")
                    .is_some(),
                "expected {good:?} to be accepted"
            );
        }
    }

    #[test]
    fn launch_request_with_malformed_conversation_mode_returns_400_not_daemon_crash(
    ) -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&project_root)?;
        let runtime = RecordingRuntime::default();
        let body = json!({
            "projectRoot": project_root,
            "harness": "claude",
            "launchMode": "nonInteractive",
            "prompt": "hi",
            "conversation": {"mode": "forge", "id": "abc"}
        })
        .to_string();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;

        // Must be a structured 400 — bubbling the error up to the
        // daemon's accept loop would take the daemon down.
        assert_eq!(response.status, 400);
        assert!(
            response.body.contains("conversation.mode"),
            "expected body to mention conversation.mode, got: {}",
            response.body
        );
        assert!(
            response.body.contains("invalid_request"),
            "expected structured `invalid_request` code, got: {}",
            response.body
        );
        assert!(runtime.launches.borrow().is_empty());
        Ok(())
    }

    #[test]
    fn launch_request_runtime_failure_returns_500_and_marks_session_failed() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&project_root)?;
        let runtime = FailingLaunchRuntime;
        let body = json!({
            "projectRoot": project_root,
            "harness": "codex",
            "prompt": "hello"
        })
        .to_string();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;
        let sessions = handle_request("GET", "/sessions", temp_dir.path(), None)?;

        // Must be a structured response — propagating Err would crash
        // the daemon's accept loop.
        assert_eq!(response.status, 500);
        assert!(
            response.body.contains("launch_failed"),
            "expected structured `launch_failed` code, got: {}",
            response.body
        );
        assert!(
            response.body.contains("launch failed"),
            "expected runtime error message in the body, got: {}",
            response.body
        );
        assert!(sessions.body.contains(r#""status":"failed""#));
        Ok(())
    }

    #[test]
    fn launch_request_rejects_cwd_outside_project_root() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("repo");
        let outside = temp_dir.path().join("outside");
        std::fs::create_dir_all(&project_root)?;
        std::fs::create_dir_all(&outside)?;
        let runtime = RecordingRuntime::default();
        let body = json!({
            "projectRoot": project_root,
            "cwd": outside,
            "harness": "codex",
            "prompt": "hello"
        })
        .to_string();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;

        assert_eq!(response.status, 400);
        assert!(
            response.body.contains("outside the Coven project root"),
            "unexpected body: {}",
            response.body
        );
        assert!(runtime.launches.borrow().is_empty());
        Ok(())
    }

    #[test]
    fn launch_request_rejects_missing_required_fields() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let runtime = RecordingRuntime::default();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(r#"{"harness":"codex"}"#),
            &runtime,
        )?;

        assert_eq!(response.status, 400);
        assert!(response.body.contains("projectRoot"));
        assert!(response.body.contains("invalid_request"));
        assert!(runtime.launches.borrow().is_empty());
        Ok(())
    }

    #[test]
    fn input_request_records_session_event() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        let response = handle_request_with_body(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"hello coven"}"#),
        )?;
        let events = handle_request("GET", "/events?sessionId=session-1", temp_dir.path(), None)?;

        assert_eq!(response.status, 202);
        assert!(response.body.contains(r#""accepted":true"#));
        assert_eq!(events.status, 200);
        assert!(events.body.contains(r#""kind":"input""#));
        assert!(events.body.contains("hello coven"));
        Ok(())
    }

    #[test]
    fn input_request_invokes_live_runtime_hook() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;
        let runtime = RecordingRuntime::default();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"hello coven"}"#),
            &runtime,
        )?;

        assert_eq!(response.status, 202);
        assert_eq!(
            runtime.inputs.borrow().as_slice(),
            &["session-1:hello coven"]
        );
        Ok(())
    }

    #[test]
    fn launch_request_with_unknown_harness_returns_400_upfront_no_session_row() -> anyhow::Result<()>
    {
        let temp_dir = tempfile::tempdir()?;
        let project_root = temp_dir.path().join("repo");
        std::fs::create_dir_all(&project_root)?;
        let runtime = RecordingRuntime::default();
        let body = json!({
            "projectRoot": project_root,
            "harness": "hermes",
            "launchMode": "nonInteractive",
            "prompt": "hello"
        })
        .to_string();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions",
            temp_dir.path(),
            None,
            Some(&body),
            &runtime,
        )?;

        assert_eq!(response.status, 400);
        assert!(
            response.body.contains("unsupported harness `hermes`"),
            "expected unsupported-harness validation message, got: {}",
            response.body
        );
        assert!(
            response
                .body
                .contains(crate::harness::EXTERNAL_ADAPTER_MANIFEST_ENV),
            "expected external adapter manifest guidance, got: {}",
            response.body
        );
        assert!(
            runtime.launches.borrow().is_empty(),
            "runtime must not be invoked for an unsupported harness"
        );
        // And no session row should have been inserted.
        let sessions = handle_request("GET", "/sessions", temp_dir.path(), None)?;
        assert!(
            sessions.body.contains("[]"),
            "no session row should exist after a 400-rejected launch, got: {}",
            sessions.body
        );
        Ok(())
    }

    #[test]
    fn input_request_with_missing_data_field_returns_400_not_500() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        let response = handle_request_with_body(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"foo":"bar"}"#),
        )?;

        assert_eq!(response.status, 400);
        assert!(
            response.body.contains("invalid_request"),
            "missing `data` is a client error, expected 400 invalid_request, got: {}",
            response.body
        );
        assert!(
            response.body.contains("`data`"),
            "expected the message to name the missing field, got: {}",
            response.body
        );
        Ok(())
    }

    #[test]
    fn input_request_with_non_string_data_field_returns_400_not_500() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        let response = handle_request_with_body(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"data": 42}"#),
        )?;

        assert_eq!(response.status, 400);
        assert!(
            response.body.contains("invalid_request"),
            "non-string `data` is a client error, expected 400 invalid_request, got: {}",
            response.body
        );
        Ok(())
    }

    #[test]
    fn input_request_with_malformed_body_returns_400_not_daemon_crash() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        let response = handle_request_with_body(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some("{ not json"),
        )?;

        assert_eq!(response.status, 400);
        assert!(
            response.body.contains("invalid_request"),
            "expected structured `invalid_request` code, got: {}",
            response.body
        );
        Ok(())
    }

    #[test]
    fn input_request_not_live_runtime_error_routes_to_409_via_typed_downcast() -> anyhow::Result<()>
    {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        struct NotLiveRuntime;
        impl SessionRuntime for NotLiveRuntime {
            fn launch_session(&self, _: &SessionLaunch) -> Result<()> {
                Ok(())
            }
            fn send_input(&self, _: &str, _: &Value) -> Result<()> {
                Err(anyhow::Error::new(crate::daemon::NotLiveError {
                    session_id: "session-1".to_string(),
                }))
            }
            fn kill_session(&self, _: &str) -> Result<()> {
                Ok(())
            }
        }
        let runtime = NotLiveRuntime;

        let response = handle_request_with_runtime(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"hi"}"#),
            &runtime,
        )?;

        assert_eq!(response.status, 409);
        assert!(
            response.body.contains("session_not_live"),
            "typed NotLiveError must route to 409 session_not_live, got: {}",
            response.body
        );
        Ok(())
    }

    #[test]
    fn input_request_runtime_failure_returns_500_not_daemon_crash() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        // Runtime that fails send_input with a non-"not live" message
        // — we want the daemon to surface it as a structured 500
        // instead of bubbling it to serve_next_connection's `?`.
        struct FailingInput;
        impl SessionRuntime for FailingInput {
            fn launch_session(&self, _: &SessionLaunch) -> Result<()> {
                Ok(())
            }
            fn send_input(&self, _: &str, _: &Value) -> Result<()> {
                Err(anyhow::anyhow!("simulated send_input failure"))
            }
            fn kill_session(&self, _: &str) -> Result<()> {
                Ok(())
            }
        }
        let runtime = FailingInput;

        let response = handle_request_with_runtime(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"hello"}"#),
            &runtime,
        )?;

        assert_eq!(response.status, 500);
        assert!(
            response.body.contains("send_input_failed"),
            "expected structured `send_input_failed` code, got: {}",
            response.body
        );
        Ok(())
    }

    #[test]
    fn kill_request_runtime_failure_returns_500_not_daemon_crash() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        struct FailingKill;
        impl SessionRuntime for FailingKill {
            fn launch_session(&self, _: &SessionLaunch) -> Result<()> {
                Ok(())
            }
            fn send_input(&self, _: &str, _: &Value) -> Result<()> {
                Ok(())
            }
            fn kill_session(&self, _: &str) -> Result<()> {
                Err(anyhow::anyhow!("simulated kill_session failure"))
            }
        }
        let runtime = FailingKill;

        let response = handle_request_with_runtime(
            "POST",
            "/sessions/session-1/kill",
            temp_dir.path(),
            None,
            Some("{}"),
            &runtime,
        )?;

        assert_eq!(response.status, 500);
        assert!(
            response.body.contains("kill_failed"),
            "expected structured `kill_failed` code, got: {}",
            response.body
        );
        Ok(())
    }

    #[test]
    fn kill_request_marks_session_killed_and_records_event() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        let response = handle_request("POST", "/sessions/session-1/kill", temp_dir.path(), None)?;
        let detail = handle_request("GET", "/sessions/session-1", temp_dir.path(), None)?;
        let events = handle_request("GET", "/events?sessionId=session-1", temp_dir.path(), None)?;

        assert_eq!(response.status, 202);
        assert!(detail.body.contains(r#""status":"killed""#));
        assert!(events.body.contains(r#""kind":"kill""#));
        Ok(())
    }

    #[test]
    fn kill_request_invokes_live_runtime_hook() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;
        let runtime = RecordingRuntime::default();

        let response = handle_request_with_runtime(
            "POST",
            "/sessions/session-1/kill",
            temp_dir.path(),
            None,
            None,
            &runtime,
        )?;

        assert_eq!(response.status, 202);
        assert_eq!(runtime.kills.borrow().as_slice(), &["session-1"]);
        Ok(())
    }

    #[test]
    fn input_and_kill_reject_completed_sessions_as_not_live() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session_with_status(temp_dir.path(), "session-1", "completed")?;

        let input = handle_request_with_body(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"hello"}"#),
        )?;
        let kill = handle_request("POST", "/sessions/session-1/kill", temp_dir.path(), None)?;

        assert_eq!(input.status, 409);
        assert_eq!(kill.status, 409);
        assert!(input.body.contains(r#""code":"session_not_live""#));
        assert!(kill.body.contains(r#""code":"session_not_live""#));
        Ok(())
    }

    #[test]
    fn input_and_kill_reject_orphaned_sessions_as_not_live() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session_with_status(temp_dir.path(), "session-1", "orphaned")?;

        let input = handle_request_with_body(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"hello"}"#),
        )?;
        let kill = handle_request("POST", "/sessions/session-1/kill", temp_dir.path(), None)?;

        assert_eq!(input.status, 409);
        assert_eq!(kill.status, 409);
        assert!(input.body.contains(r#""code":"session_not_live""#));
        assert!(kill.body.contains(r#""code":"session_not_live""#));
        Ok(())
    }

    #[test]
    fn runtime_not_live_errors_become_conflict_responses() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;
        let runtime = NotLiveRuntime;

        let input = handle_request_with_runtime(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"hello"}"#),
            &runtime,
        )?;
        let kill = handle_request_with_runtime(
            "POST",
            "/sessions/session-1/kill",
            temp_dir.path(),
            None,
            None,
            &runtime,
        )?;

        assert_eq!(input.status, 409);
        assert_eq!(kill.status, 409);
        assert!(input.body.contains(r#""code":"session_not_live""#));
        assert!(kill.body.contains(r#""code":"session_not_live""#));
        Ok(())
    }

    #[test]
    fn input_and_kill_reject_unknown_sessions() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let input = handle_request_with_body(
            "POST",
            "/sessions/missing/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"hello"}"#),
        )?;
        let kill = handle_request("POST", "/sessions/missing/kill", temp_dir.path(), None)?;

        assert_eq!(input.status, 404);
        assert_eq!(kill.status, 404);
        assert!(input.body.contains(r#""code":"session_not_found""#));
        assert!(kill.body.contains(r#""code":"session_not_found""#));
        Ok(())
    }

    #[derive(Default)]
    struct RecordingRuntime {
        launches: std::cell::RefCell<Vec<SessionLaunch>>,
        inputs: std::cell::RefCell<Vec<String>>,
        kills: std::cell::RefCell<Vec<String>>,
    }

    impl SessionRuntime for RecordingRuntime {
        fn launch_session(&self, launch: &SessionLaunch) -> Result<()> {
            self.launches.borrow_mut().push(launch.clone());
            Ok(())
        }

        fn send_input(&self, session_id: &str, payload: &Value) -> Result<()> {
            let data = payload
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default();
            self.inputs
                .borrow_mut()
                .push(format!("{session_id}:{data}"));
            Ok(())
        }

        fn kill_session(&self, session_id: &str) -> Result<()> {
            self.kills.borrow_mut().push(session_id.to_string());
            Ok(())
        }
    }

    struct FailingLaunchRuntime;

    impl SessionRuntime for FailingLaunchRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> Result<()> {
            anyhow::bail!("launch failed")
        }

        fn send_input(&self, _session_id: &str, _payload: &Value) -> Result<()> {
            Ok(())
        }

        fn kill_session(&self, _session_id: &str) -> Result<()> {
            Ok(())
        }
    }

    struct NotLiveRuntime;

    impl SessionRuntime for NotLiveRuntime {
        fn launch_session(&self, _launch: &SessionLaunch) -> Result<()> {
            Ok(())
        }

        fn send_input(&self, session_id: &str, _payload: &Value) -> Result<()> {
            Err(anyhow::Error::new(crate::daemon::NotLiveError {
                session_id: session_id.to_string(),
            }))
        }

        fn kill_session(&self, session_id: &str) -> Result<()> {
            Err(anyhow::Error::new(crate::daemon::NotLiveError {
                session_id: session_id.to_string(),
            }))
        }
    }

    fn insert_test_session(coven_home: &std::path::Path, id: &str) -> anyhow::Result<()> {
        insert_test_session_with_status(coven_home, id, "running")
    }

    fn insert_test_session_with_status(
        coven_home: &std::path::Path,
        id: &str,
        status: &str,
    ) -> anyhow::Result<()> {
        let conn = crate::store::open_store(&coven_home.join("coven.sqlite3"))?;
        let session = crate::store::SessionRecord {
            id: id.to_string(),
            project_root: "/repo".to_string(),
            harness: "codex".to_string(),
            title: "hello from coven".to_string(),
            status: status.to_string(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-04-27T10:00:00Z".to_string(),
            updated_at: "2026-04-27T10:00:00Z".to_string(),
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        };
        crate::store::insert_session(&conn, &session)?;
        Ok(())
    }

    #[test]
    fn events_response_has_paginated_envelope_with_next_cursor() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        handle_request_with_body(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"first"}"#),
        )?;
        handle_request_with_body(
            "POST",
            "/sessions/session-1/input",
            temp_dir.path(),
            None,
            Some(r#"{"data":"second"}"#),
        )?;

        let events = handle_request("GET", "/events?sessionId=session-1", temp_dir.path(), None)?;

        assert_eq!(events.status, 200);
        let body: serde_json::Value = serde_json::from_str(&events.body)?;
        assert!(body["events"].is_array());
        assert_eq!(body["events"].as_array().unwrap().len(), 2);
        assert!(body["events"][0]["seq"].as_i64().unwrap() > 0);
        assert!(
            body["events"][1]["seq"].as_i64().unwrap() > body["events"][0]["seq"].as_i64().unwrap()
        );
        assert!(body["nextCursor"]["afterSeq"].as_i64().is_some());
        assert_eq!(body["hasMore"], false);
        Ok(())
    }

    #[test]
    fn events_endpoint_supports_after_seq_cursor() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        for data in &["a", "b", "c"] {
            handle_request_with_body(
                "POST",
                "/sessions/session-1/input",
                temp_dir.path(),
                None,
                Some(&format!(r#"{{"data":"{data}"}}"#)),
            )?;
        }

        let all = handle_request("GET", "/events?sessionId=session-1", temp_dir.path(), None)?;
        let all_body: serde_json::Value = serde_json::from_str(&all.body)?;
        let first_seq = all_body["events"][0]["seq"].as_i64().unwrap();

        let after = handle_request(
            "GET",
            &format!("/events?sessionId=session-1&afterSeq={first_seq}"),
            temp_dir.path(),
            None,
        )?;
        let after_body: serde_json::Value = serde_json::from_str(&after.body)?;
        assert_eq!(after.status, 200);
        assert_eq!(after_body["events"].as_array().unwrap().len(), 2);
        Ok(())
    }

    #[test]
    fn events_endpoint_supports_limit_param() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        for data in &["a", "b", "c", "d"] {
            handle_request_with_body(
                "POST",
                "/sessions/session-1/input",
                temp_dir.path(),
                None,
                Some(&format!(r#"{{"data":"{data}"}}"#)),
            )?;
        }

        let limited = handle_request(
            "GET",
            "/events?sessionId=session-1&limit=2",
            temp_dir.path(),
            None,
        )?;
        let body: serde_json::Value = serde_json::from_str(&limited.body)?;
        assert_eq!(limited.status, 200);
        assert_eq!(body["events"].as_array().unwrap().len(), 2);
        assert_eq!(body["hasMore"], true);
        Ok(())
    }

    #[test]
    fn events_endpoint_combines_after_seq_cursor_with_limit() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        for data in &["a", "b", "c"] {
            handle_request_with_body(
                "POST",
                "/sessions/session-1/input",
                temp_dir.path(),
                None,
                Some(&format!(r#"{{"data":"{data}"}}"#)),
            )?;
        }

        let all = handle_request("GET", "/events?sessionId=session-1", temp_dir.path(), None)?;
        let all_body: serde_json::Value = serde_json::from_str(&all.body)?;
        let first_seq = all_body["events"][0]["seq"].as_i64().unwrap();

        let page = handle_request(
            "GET",
            &format!("/events?sessionId=session-1&afterSeq={first_seq}&limit=1"),
            temp_dir.path(),
            None,
        )?;

        let body: serde_json::Value = serde_json::from_str(&page.body)?;
        assert_eq!(page.status, 200);
        assert_eq!(body["events"].as_array().unwrap().len(), 1);
        assert!(body["events"][0]["seq"].as_i64().unwrap() > first_seq);
        assert_eq!(body["nextCursor"]["afterSeq"], body["events"][0]["seq"]);
        assert_eq!(body["hasMore"], true);
        Ok(())
    }

    #[test]
    fn events_endpoint_supports_after_event_id_cursor() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        for data in &["a", "b", "c"] {
            handle_request_with_body(
                "POST",
                "/sessions/session-1/input",
                temp_dir.path(),
                None,
                Some(&format!(r#"{{"data":"{data}"}}"#)),
            )?;
        }

        let all = handle_request("GET", "/events?sessionId=session-1", temp_dir.path(), None)?;
        let all_body: serde_json::Value = serde_json::from_str(&all.body)?;
        let first_event_id = all_body["events"][0]["id"].as_str().unwrap();
        let second_event_id = all_body["events"][1]["id"].as_str().unwrap();
        let third_event_id = all_body["events"][2]["id"].as_str().unwrap();

        let after = handle_request(
            "GET",
            &format!("/events?sessionId=session-1&afterEventId={first_event_id}"),
            temp_dir.path(),
            None,
        )?;
        let after_body: serde_json::Value = serde_json::from_str(&after.body)?;
        assert_eq!(after.status, 200);
        assert_eq!(after_body["events"].as_array().unwrap().len(), 2);
        assert_eq!(after_body["events"][0]["id"], second_event_id);
        assert_eq!(after_body["events"][1]["id"], third_event_id);
        Ok(())
    }

    #[test]
    fn events_endpoint_clamps_zero_limit_to_one_event() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        for data in &["a", "b"] {
            handle_request_with_body(
                "POST",
                "/sessions/session-1/input",
                temp_dir.path(),
                None,
                Some(&format!(r#"{{"data":"{data}"}}"#)),
            )?;
        }

        let response = handle_request(
            "GET",
            "/events?sessionId=session-1&limit=0",
            temp_dir.path(),
            None,
        )?;

        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(response.status, 200);
        assert_eq!(body["events"].as_array().unwrap().len(), 1);
        assert_eq!(body["hasMore"], true);
        Ok(())
    }

    #[test]
    fn events_endpoint_returns_structured_error_for_missing_session_id() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let response = handle_request("GET", "/events", temp_dir.path(), None)?;

        assert_eq!(response.status, 400);
        assert!(response.body.contains(r#""code":"invalid_request""#));
        Ok(())
    }

    #[test]
    fn events_endpoint_returns_structured_error_for_non_integer_limit() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        let response = handle_request(
            "GET",
            "/events?sessionId=session-1&limit=foo",
            temp_dir.path(),
            None,
        )?;

        assert_eq!(response.status, 400);
        assert!(response.body.contains(r#""code":"invalid_request""#));
        assert!(response.body.contains(r#""limit":"foo""#));
        Ok(())
    }

    #[test]
    fn events_endpoint_returns_structured_error_for_non_integer_after_seq() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        insert_test_session(temp_dir.path(), "session-1")?;

        let response = handle_request(
            "GET",
            "/events?sessionId=session-1&afterSeq=foo",
            temp_dir.path(),
            None,
        )?;

        assert_eq!(response.status, 400);
        assert!(response.body.contains(r#""code":"invalid_request""#));
        assert!(response.body.contains(r#""afterSeq":"foo""#));
        Ok(())
    }

    #[test]
    fn events_endpoint_validates_limit_before_session_lookup() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let response = handle_request(
            "GET",
            "/events?sessionId=ghost&limit=foo",
            temp_dir.path(),
            None,
        )?;

        assert_eq!(response.status, 400);
        assert!(response.body.contains(r#""code":"invalid_request""#));
        assert!(response.body.contains(r#""limit":"foo""#));
        Ok(())
    }

    #[test]
    fn events_endpoint_validates_after_seq_before_session_lookup() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let response = handle_request(
            "GET",
            "/events?sessionId=ghost&afterSeq=foo",
            temp_dir.path(),
            None,
        )?;

        assert_eq!(response.status, 400);
        assert!(response.body.contains(r#""code":"invalid_request""#));
        assert!(response.body.contains(r#""afterSeq":"foo""#));
        Ok(())
    }

    #[test]
    fn events_endpoint_returns_structured_error_for_unknown_session() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let response = handle_request("GET", "/events?sessionId=ghost", temp_dir.path(), None)?;

        assert_eq!(response.status, 404);
        assert!(response.body.contains(r#""code":"session_not_found""#));
        Ok(())
    }

    #[test]
    fn get_session_log_returns_log_lines_from_events() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let conn = store::open_store(&store_path(home))?;
        let session = store::SessionRecord {
            id: "sess-log".into(),
            project_root: "/tmp/proj".into(),
            harness: "claude".into(),
            title: "demo".into(),
            status: "running".into(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        };
        store::insert_session(&conn, &session)?;
        insert_event(&conn, home, "sess-log", "input", json!({"text": "hello"}))?;
        insert_event(&conn, home, "sess-log", "output", json!({"text": "world"}))?;
        insert_event(&conn, home, "sess-log", "error", json!({"message": "boom"}))?;
        drop(conn);

        let response = handle_request("GET", "/api/v1/sessions/sess-log/log", home, None)?;
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        let lines = body.as_array().expect("array body");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0]["level"], "info");
        assert!(lines[0]["message"].as_str().unwrap().contains("hello"));
        assert_eq!(lines[2]["level"], "error");
        assert!(lines[2]["message"].as_str().unwrap().contains("boom"));
        Ok(())
    }

    #[test]
    fn logs_and_events_return_redacted_payloads_by_default() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let conn = store::open_store(&store_path(home))?;
        let session = store::SessionRecord {
            id: "sess-secret".into(),
            project_root: "/tmp/proj".into(),
            harness: "codex".into(),
            title: "demo".into(),
            status: "running".into(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        };
        store::insert_session(&conn, &session)?;
        let fake = fake_openai_key();
        insert_event(
            &conn,
            home,
            "sess-secret",
            "input",
            json!({"data": format!("Authorization: Bearer {fake}")}),
        )?;
        drop(conn);

        let log = handle_request("GET", "/api/v1/sessions/sess-secret/log", home, None)?;
        let events = handle_request("GET", "/api/v1/events?sessionId=sess-secret", home, None)?;
        let alias = handle_request("GET", "/api/v1/sessions/sess-secret/events", home, None)?;

        for response in [log, events, alias] {
            assert_eq!(response.status, 200);
            assert!(!response.body.contains(&fake));
            assert!(response.body.contains("[REDACTED]"));
        }
        Ok(())
    }

    #[test]
    fn raw_artifact_endpoint_is_disabled_by_default() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let conn = store::open_store(&store_path(home))?;
        let session = store::SessionRecord {
            id: "sess-secret".into(),
            project_root: "/tmp/proj".into(),
            harness: "codex".into(),
            title: "demo".into(),
            status: "running".into(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        };
        store::insert_session(&conn, &session)?;
        drop(conn);

        let response = handle_request(
            "GET",
            "/api/v1/sessions/sess-secret/artifacts/event-1?raw=1",
            home,
            None,
        )?;

        assert_eq!(response.status, 403);
        assert!(response.body.contains(r#""code":"raw_artifacts_disabled""#));
        assert!(!response.body.contains("payload"));
        Ok(())
    }

    #[test]
    fn raw_artifact_endpoint_returns_404_for_expired_artifact() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        std::fs::write(home.join("privacy.toml"), "persist_raw_artifacts = true\n")?;
        let conn = store::open_store(&store_path(home))?;
        let session = store::SessionRecord {
            id: "sess-secret".into(),
            project_root: "/tmp/proj".into(),
            harness: "codex".into(),
            title: "demo".into(),
            status: "running".into(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        };
        store::insert_session(&conn, &session)?;
        insert_event(
            &conn,
            home,
            "sess-secret",
            "input",
            json!({"data": "secret"}),
        )?;
        let event_id = store::list_events(&conn, "sess-secret")?
            .pop()
            .expect("event")
            .id;
        store::insert_sensitive_artifact(
            &conn,
            &store::SensitiveArtifactRecord {
                id: "artifact-1".into(),
                session_id: "sess-secret".into(),
                event_id,
                kind: "input".into(),
                nonce: vec![0; 24],
                ciphertext: vec![1, 2, 3],
                created_at: "2026-01-01T00:00:00Z".into(),
                expires_at: "2026-01-02T00:00:00Z".into(),
            },
        )?;
        drop(conn);

        let response = handle_request(
            "GET",
            "/api/v1/sessions/sess-secret/artifacts/artifact-1?raw=1",
            home,
            None,
        )?;

        assert_eq!(response.status, 404);
        assert!(response.body.contains(r#""code":"artifact_expired""#));
        assert!(!response.body.contains("payload"));
        Ok(())
    }

    #[test]
    fn get_session_log_returns_404_for_unknown_session() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let response = handle_request("GET", "/api/v1/sessions/missing/log", temp.path(), None)?;
        assert_eq!(response.status, 404);
        assert!(
            response.body.contains(r#""sessionId":"missing""#),
            "expected sessionId 'missing' (not 'missing/log'); got: {}",
            response.body
        );
        Ok(())
    }

    #[test]
    fn post_cast_records_event_and_returns_result() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let body = json!({
            "code": "/status",
            "target": null,
        });
        let response = handle_request_with_body(
            "POST",
            "/api/v1/cast",
            temp.path(),
            None,
            Some(&body.to_string()),
        )?;
        assert_eq!(response.status, 202);
        let result: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(result["accepted"], true);
        assert!(result["cast_id"]
            .as_str()
            .expect("cast_id")
            .starts_with("cast-"));
        assert_eq!(result["echo"], "/status");
        Ok(())
    }

    #[test]
    fn post_cast_rejects_missing_code() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let body = json!({ "target": "sess-1" });
        let response = handle_request_with_body(
            "POST",
            "/api/v1/cast",
            temp.path(),
            None,
            Some(&body.to_string()),
        )?;
        assert_eq!(response.status, 400);
        Ok(())
    }

    #[test]
    fn post_cast_with_target_logs_event_to_session() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();

        let conn = store::open_store(&store_path(home))?;
        let session = store::SessionRecord {
            id: "sess-target".into(),
            project_root: "/tmp/proj".into(),
            harness: "claude".into(),
            title: "demo".into(),
            status: "running".into(),
            exit_code: None,
            archived_at: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            conversation_id: None,
            familiar_id: None,
            labels: Vec::new(),
            visibility: "private".to_string(),
            external: false,
            transcript_path: None,
        };
        store::insert_session(&conn, &session)?;
        drop(conn);

        let body = json!({ "code": "/handoff", "target": "sess-target" });
        let response =
            handle_request_with_body("POST", "/api/v1/cast", home, None, Some(&body.to_string()))?;
        assert_eq!(response.status, 202);
        let result: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(result["accepted"], true);
        assert_eq!(result["echo"], "/handoff → sess-target");

        // Verify the cast landed as an event on the target session, not __cockpit__.
        let log_response = handle_request("GET", "/api/v1/sessions/sess-target/log", home, None)?;
        assert_eq!(log_response.status, 200);
        let lines: serde_json::Value = serde_json::from_str(&log_response.body)?;
        let arr = lines.as_array().expect("array body");
        assert_eq!(arr.len(), 1);
        assert!(
            arr[0]["message"].as_str().unwrap().contains("/handoff"),
            "expected log message to contain code; got: {}",
            arr[0]["message"]
        );
        Ok(())
    }

    #[test]
    fn post_cast_with_unknown_target_returns_404() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let body = json!({ "code": "/status", "target": "no-such-session" });
        let response = handle_request_with_body(
            "POST",
            "/api/v1/cast",
            temp.path(),
            None,
            Some(&body.to_string()),
        )?;
        assert_eq!(response.status, 404);
        assert!(
            response.body.contains(r#""code":"session_not_found""#),
            "expected session_not_found body; got: {}",
            response.body
        );
        assert!(
            response.body.contains(r#""sessionId":"no-such-session""#),
            "expected sessionId in error details; got: {}",
            response.body
        );
        Ok(())
    }

    #[test]
    fn post_cast_without_target_idempotently_uses_cockpit_session() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let body = json!({ "code": "/status" });
        for _ in 0..3 {
            let response = handle_request_with_body(
                "POST",
                "/api/v1/cast",
                home,
                None,
                Some(&body.to_string()),
            )?;
            assert_eq!(response.status, 202);
        }
        // Only one __cockpit__ row should exist; all three casts land as events on it.
        let conn = store::open_store(&store_path(home))?;
        let sessions = store::list_sessions(&conn)?;
        let cockpit_count = sessions.iter().filter(|s| s.id == "__cockpit__").count();
        assert_eq!(
            cockpit_count, 1,
            "expected exactly one __cockpit__ session row"
        );
        Ok(())
    }

    #[test]
    fn get_overview_returns_session_count_and_zeroed_unknowns() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let conn = store::open_store(&store_path(home))?;
        for id in ["s1", "s2", "s3"] {
            let now = "2026-01-01T00:00:00Z";
            let status = if id == "s3" { "ended" } else { "running" };
            store::insert_session(
                &conn,
                &store::SessionRecord {
                    id: id.into(),
                    project_root: "/tmp".into(),
                    harness: "claude".into(),
                    title: "t".into(),
                    status: status.into(),
                    exit_code: None,
                    archived_at: None,
                    created_at: now.into(),
                    updated_at: now.into(),
                    conversation_id: None,
                    familiar_id: None,
                    labels: Vec::new(),
                    visibility: "private".to_string(),
                    external: false,
                    transcript_path: None,
                },
            )?;
        }
        drop(conn);

        let response = handle_request("GET", "/api/v1/overview", home, None)?;
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["open_sessions"], 2);
        assert_eq!(body["active_familiars"], 0);
        assert_eq!(body["skills_count"], 0);
        Ok(())
    }

    #[test]
    fn get_overview_counts_familiars_skills_and_research_from_local_sources() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();

        std::fs::write(
            home.join("familiars.toml"),
            r#"
[[familiar]]
id = "charm"
display_name = "Charm"
role = "steward"
description = "keeps the hearth"

[[familiar]]
id = "sage"
display_name = "Sage"
role = "researcher"
description = "digs deep"
"#,
        )?;

        for skill in ["eval-loop", "stream-scribe"] {
            let dir = home.join("skills").join(skill);
            std::fs::create_dir_all(&dir)?;
            std::fs::write(
                dir.join("metadata.json"),
                format!(r#"{{"name":"{skill}","description":"a skill","version":"1.0.0"}}"#),
            )?;
        }

        let research_dir = home.join("research");
        std::fs::create_dir_all(&research_dir)?;
        std::fs::write(
            research_dir.join("results.tsv"),
            "1\tharness capabilities\t7.5\t1.5\tcontinue\tnotes.md\n\
             2\tstream continuity\t9.0\t2.0\tadopt\tnotes.md\n",
        )?;

        let conn = store::open_store(&store_path(home))?;
        let now = "2026-01-01T00:00:00Z";
        for (id, status, familiar) in [
            ("s1", "running", Some("charm")),
            ("s2", "running", Some("charm")),
            ("s3", "running", Some("ghost-not-in-roster")),
            ("s4", "ended", Some("sage")),
        ] {
            store::insert_session(
                &conn,
                &store::SessionRecord {
                    id: id.into(),
                    project_root: "/tmp".into(),
                    harness: "claude".into(),
                    title: "t".into(),
                    status: status.into(),
                    exit_code: None,
                    archived_at: None,
                    created_at: now.into(),
                    updated_at: now.into(),
                    conversation_id: None,
                    familiar_id: familiar.map(str::to_string),
                    labels: Vec::new(),
                    visibility: "private".to_string(),
                    external: false,
                    transcript_path: None,
                },
            )?;
        }
        drop(conn);

        let response = handle_request("GET", "/api/v1/overview", home, None)?;
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["open_sessions"], 3);
        assert_eq!(body["total_familiars"], 2);
        // Only roster familiars with an open session count as active: charm
        // (running twice, deduped); sage's session ended; the ghost id is not
        // in the roster.
        assert_eq!(body["active_familiars"], 1);
        assert_eq!(body["skills_count"], 2);
        // Skill scores are stubbed at 0.0 until scoring lands.
        assert_eq!(body["average_skill_score"], 0);
        assert_eq!(body["research_iterations"], 2);
        assert_eq!(body["last_research_delta"], 2);
        Ok(())
    }

    #[test]
    fn empty_array_stubs_return_200_with_empty_json_array() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        for route in [
            "/api/v1/familiars",
            "/api/v1/skills",
            "/api/v1/memory",
            "/api/v1/research",
        ] {
            let response = handle_request("GET", route, home, None)?;
            assert_eq!(response.status, 200, "route {route}");
            assert_eq!(response.content_type, "application/json", "route {route}");
            assert_eq!(response.body, "[]", "route {route}");
        }
        Ok(())
    }

    #[test]
    fn get_cast_codes_returns_grammar_templates() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let response = handle_request("GET", "/api/v1/cast-codes", temp.path(), None)?;
        assert_eq!(response.status, 200);
        assert_eq!(response.content_type, "application/json");
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        let codes = body.as_array().expect("cast-codes returns an array");
        let literals: Vec<&str> = codes
            .iter()
            .map(|c| c["code"].as_str().expect("code is a string"))
            .collect();
        assert!(literals.contains(&"~?"));
        assert!(literals.contains(&"~>{familiar}"));
        assert!(literals.contains(&"~delegate:{familiar}"));
        assert!(literals.contains(&"~broadcast *"));
        // No per-familiar literals — those are cockpit-side concerns once the
        // daemon learns about specific familiars.
        for code in &literals {
            assert!(
                !code.contains("sage") && !code.contains("cody"),
                "unexpected per-familiar literal in /cast-codes: {code}"
            );
        }
        let first = &codes[0];
        assert_eq!(first["type"], "status");
        Ok(())
    }

    #[test]
    fn delete_eval_loop_run_lock_clears_with_force() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let eval_dir = home.join("familiars").join("sage").join("eval-loop");
        std::fs::create_dir_all(&eval_dir)?;
        std::fs::write(eval_dir.join("run.lock"), "run-123")?;

        let response = handle_request_with_body(
            "DELETE",
            "/api/v1/skills/eval-loop/sage/run-lock",
            home,
            None,
            Some(r#"{"force":true}"#),
        )?;

        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["ok"], true);
        assert_eq!(body["cleared"], true);
        assert!(!eval_dir.join("run.lock").exists());
        Ok(())
    }

    #[test]
    fn delete_eval_loop_run_lock_rejects_fresh_lock_without_force() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let eval_dir = home.join("familiars").join("sage").join("eval-loop");
        std::fs::create_dir_all(&eval_dir)?;
        std::fs::write(eval_dir.join("run.json"), r#"{"runId":"run-123"}"#)?;
        std::fs::write(eval_dir.join("run.lock"), "run-123")?;

        let response = handle_request_with_body(
            "DELETE",
            "/api/v1/skills/eval-loop/sage/run-lock",
            home,
            None,
            Some(r#"{"force":false}"#),
        )?;

        assert_eq!(response.status, 409);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "lock_not_stale");
        assert!(eval_dir.join("run.lock").exists());
        Ok(())
    }

    fn fake_openai_key() -> String {
        format!("sk-{}", "c".repeat(40))
    }

    // ---- PUT /api/v1/familiars/{id}/icon ---------------------------------

    fn seed_familiars_toml(home: &Path) -> Result<()> {
        std::fs::write(
            home.join("familiars.toml"),
            r#"[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "Builds and debugs."

[[familiar]]
id = "sage"
display_name = "Sage"
role = "Research"
description = "Reads and synthesizes."
icon = "ph:leaf-fill"
"#,
        )?;
        Ok(())
    }

    #[test]
    fn familiar_ward_route_reports_declared_surface() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_familiars_toml(home)?;
        let workspace = home.join("familiars").join("sage");
        std::fs::create_dir_all(&workspace)?;
        std::fs::write(
            workspace.join("ward.toml"),
            r#"principal_key_fingerprint = "SHA256:principal-key"
protected_surface = ["SOUL.md"]

[[surface]]
path = "SOUL.md"
tier = 0

[[surface]]
path = "memory/"
tier = 2
"#,
        )?;

        let response = handle_request("GET", "/api/v1/familiars/sage/ward", home, None)?;
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["ok"], true);
        assert_eq!(body["familiarId"], "sage");
        assert_eq!(
            body["ward"]["principalKeyFingerprint"],
            "SHA256:principal-key"
        );
        assert_eq!(body["ward"]["defaultTier"], 2);
        assert_eq!(body["ward"]["surface"][0]["path"], "SOUL.md");
        assert_eq!(body["ward"]["surface"][0]["tier"], 0);
        assert_eq!(body["ward"]["surface"][1]["path"], "memory/");
        assert_eq!(body["ward"]["surface"][1]["tier"], 2);
        assert_eq!(body["ward"]["protectedSurface"][0], "SOUL.md");
        Ok(())
    }

    #[test]
    fn familiar_ward_route_fails_closed_on_unknown_and_unconfigured() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_familiars_toml(home)?;

        // Unknown familiar id.
        let response = handle_request("GET", "/api/v1/familiars/ghost/ward", home, None)?;
        assert_eq!(response.status, 404);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "familiar_not_found");

        // Known familiar without a ward.toml.
        let response = handle_request("GET", "/api/v1/familiars/sage/ward", home, None)?;
        assert_eq!(response.status, 404);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "ward_not_configured");

        // Malformed ids are a 400, matching the /icon and /edits contract.
        for path in ["/api/v1/familiars//ward", "/api/v1/familiars/a/b/ward"] {
            let response = handle_request("GET", path, home, None)?;
            assert_eq!(response.status, 400, "path {path}");
            let body: serde_json::Value = serde_json::from_str(&response.body)?;
            assert_eq!(body["error"]["code"], "invalid_request");
        }
        Ok(())
    }

    #[test]
    fn threads_proposals_lists_staged_coherence_proposal() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_warded_familiar(home)?;

        // Empty state first: missing pending/ is an empty list, not an error.
        let response = handle_request("GET", "/api/v1/threads/proposals", home, None)?;
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["proposals"], serde_json::json!([]));

        // Stage a Tier-1 coherence proposal through the write path.
        let staged = post_edits(
            home,
            r#"{"edits":[{"target":"reviewed/skill.md","contents":"tweak"}]}"#,
        )?;
        let staged_body: serde_json::Value = serde_json::from_str(&staged.body)?;
        let proposal_id = staged_body["proposalId"].as_str().expect("id").to_string();

        // The list surfaces it with lane, familiar, and targets.
        let response = handle_request("GET", "/api/v1/threads/proposals", home, None)?;
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        let listed = &body["proposals"][0];
        assert_eq!(listed["proposalId"], proposal_id.as_str());
        assert_eq!(listed["familiarId"], "sage");
        assert_eq!(listed["reviewKind"], "coherence");
        assert_eq!(listed["targets"][0], "reviewed/skill.md");

        // Detail returns the same record; unknown and malformed ids fail
        // closed with the structured shapes.
        let response = handle_request(
            "GET",
            &format!("/api/v1/threads/proposals/{proposal_id}"),
            home,
            None,
        )?;
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["proposal"]["reviewKind"], "coherence");

        let response = handle_request(
            "GET",
            "/api/v1/threads/proposals/00000000-0000-0000-0000-000000000000",
            home,
            None,
        )?;
        assert_eq!(response.status, 404);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "proposal_not_found");

        let response = handle_request("GET", "/api/v1/threads/proposals/not-a-uuid", home, None)?;
        assert_eq!(response.status, 400);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "invalid_request");
        Ok(())
    }

    #[test]
    fn threads_proposals_renders_validated_phase5_scheduler_state() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_warded_familiar(home)?;
        let proposal_id = coven_threads_core::ProposalId::new();
        let familiar_id = crate::threads_gate::familiar_weave_id("sage");
        let surface = coven_threads_core::SurfaceId::new("TOOLS.md");
        let staged_at = time::OffsetDateTime::from_unix_timestamp(1_700_000_000)?;
        let pending = coven_threads_core::PendingProposal {
            id: proposal_id,
            familiar_id,
            writer: coven_threads_core::WriterId::new("principal:fpr-val"),
            channel: coven_threads_core::Channel::Mutation,
            thread_id: coven_threads_core::ThreadId::new(),
            fray: coven_threads_core::FrayOrSnap::Frayed {
                strand: None,
                channel: coven_threads_core::Channel::Mutation,
                reason: coven_threads_core::FrayReason::Other("phase-5 fixture".to_string()),
            },
            edits: vec![coven_threads_core::StagedEdit {
                surface: surface.clone(),
                contents: coven_threads_core::StagedContents::from_bytes(b"tweak"),
            }],
            staged_at,
        };
        let diff =
            coven_threads_core::MaterializedDiff::try_new(vec![coven_threads_core::SurfaceDiff {
                surface: surface.clone(),
                before: None,
                after: Some(b"tweak".to_vec()),
            }])
            .map_err(anyhow::Error::msg)?;
        let evidence =
            coven_threads_core::SurfaceRegionRegistry::default_registry().classify_all(&diff);
        let classification = coven_threads_core::ProposalClassification {
            proposal_id,
            familiar_id,
            channel: coven_threads_core::Channel::Mutation,
            affected_surfaces: vec![surface],
            affected_regions: evidence.iter().map(|item| item.region_id.clone()).collect(),
            path_tier_floor: 1,
            approval_path: coven_threads_core::ApprovalPath::FamiliarCoherence {
                veto: coven_threads_core::VetoWindow::new(
                    std::time::Duration::from_secs(300),
                    std::time::Duration::from_secs(60),
                ),
            },
            evidence_replay_hash: coven_threads_core::evidence_replay_hash(&diff, &evidence),
            classified_at: staged_at,
        };
        let scheduled =
            crate::proposal_scheduler::ScheduledProposal::try_new(pending, classification, diff)?;
        let pending_dir = home.join("pending");
        std::fs::create_dir_all(&pending_dir)?;
        std::fs::write(
            pending_dir.join(format!("{familiar_id}-{proposal_id}.json")),
            serde_json::to_vec_pretty(&scheduled)?,
        )?;

        let response = handle_request("GET", "/api/v1/threads/proposals", home, None)?;

        assert_eq!(response.status, 200, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        let listed = &body["proposals"][0];
        assert_eq!(listed["proposalId"], proposal_id.to_string());
        assert_eq!(listed["familiarId"], "sage");
        assert_eq!(listed["familiarUuid"], familiar_id.0.to_string());
        assert_eq!(listed["proposalRevision"].as_str().map(str::len), Some(64));
        assert_eq!(listed["approvalPath"]["variant"], "familiar_coherence");
        assert_eq!(listed["approvalPath"]["label"], "familiar_review");
        assert_eq!(
            listed["approvalPath"]["veto_deadline"],
            "2023-11-14T22:18:20Z"
        );
        assert_eq!(listed["lifecycle"], "veto_window_open");
        assert_eq!(listed["earliestClose"], "2023-11-14T22:14:20Z");
        assert_eq!(listed["affectedRegions"][0], "tool_defaults");
        assert!(listed.get("reviewKind").is_none());
        Ok(())
    }

    #[test]
    fn threads_proposals_does_not_fallback_malformed_phase5_to_legacy() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending_path, _) = stage_pending_protected_edit(home)?;
        let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(&pending_path)?)?;
        value["classification"] = serde_json::json!({});
        std::fs::write(&pending_path, serde_json::to_vec_pretty(&value)?)?;

        let response = handle_request("GET", "/api/v1/threads/proposals", home, None)?;

        assert_eq!(response.status, 200, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(
            body["proposals"][0]["degraded"]["reason"],
            "proposal-unparseable"
        );
        assert!(body["proposals"][0].get("proposalId").is_none());
        Ok(())
    }

    #[test]
    fn put_familiar_icon_updates_existing_value() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_familiars_toml(home)?;
        let response = handle_request_with_body(
            "PUT",
            "/api/v1/familiars/sage/icon",
            home,
            None,
            Some(r#"{"icon":"🌿"}"#),
        )?;
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["ok"], true);
        assert_eq!(body["action"], "updated");
        assert_eq!(body["id"], "sage");
        let raw = std::fs::read_to_string(home.join("familiars.toml"))?;
        assert!(raw.contains("icon = \"🌿\""), "got {raw}");
        Ok(())
    }

    #[test]
    fn put_familiar_icon_inserts_when_absent() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_familiars_toml(home)?;
        let response = handle_request_with_body(
            "PUT",
            "/api/v1/familiars/cody/icon",
            home,
            None,
            Some(r#"{"icon":"ph:lightning-fill"}"#),
        )?;
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["action"], "updated");
        let raw = std::fs::read_to_string(home.join("familiars.toml"))?;
        assert!(raw.contains("icon = \"ph:lightning-fill\""), "got {raw}");
        // Other familiar's icon must be untouched.
        assert!(raw.contains("icon = \"ph:leaf-fill\""));
        Ok(())
    }

    #[test]
    fn put_familiar_icon_clears_when_null() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_familiars_toml(home)?;
        let response = handle_request_with_body(
            "PUT",
            "/api/v1/familiars/sage/icon",
            home,
            None,
            Some(r#"{"icon":null}"#),
        )?;
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["action"], "cleared");
        let raw = std::fs::read_to_string(home.join("familiars.toml"))?;
        assert!(!raw.contains("ph:leaf-fill"));
        Ok(())
    }

    #[test]
    fn put_familiar_icon_returns_404_for_unknown_id() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_familiars_toml(home)?;
        let response = handle_request_with_body(
            "PUT",
            "/api/v1/familiars/ghost/icon",
            home,
            None,
            Some(r#"{"icon":"ph:ghost-fill"}"#),
        )?;
        assert_eq!(response.status, 404);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "familiar_not_found");
        Ok(())
    }

    #[test]
    fn put_familiar_icon_rejects_non_string_icon() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_familiars_toml(home)?;
        let response = handle_request_with_body(
            "PUT",
            "/api/v1/familiars/sage/icon",
            home,
            None,
            Some(r#"{"icon":[1,2,3]}"#),
        )?;
        assert_eq!(response.status, 400);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "invalid_request");
        // File must be untouched.
        let raw = std::fs::read_to_string(home.join("familiars.toml"))?;
        assert!(raw.contains("ph:leaf-fill"));
        Ok(())
    }

    #[test]
    fn put_familiar_icon_with_empty_body_clears() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_familiars_toml(home)?;
        let response =
            handle_request_with_body("PUT", "/api/v1/familiars/sage/icon", home, None, None)?;
        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["action"], "cleared");
        Ok(())
    }

    // ---- POST /api/v1/familiars/{id}/edits (Ward write path) -------------

    /// Seed a warded sage: familiars.toml plus a workspace carrying a
    /// ward.toml with SOUL.md protected (tier 0), reviewed/ tier 1, and the
    /// default tier 2 everywhere else. Returns the workspace path.
    fn seed_warded_familiar(home: &Path) -> Result<std::path::PathBuf> {
        seed_familiars_toml(home)?;
        let workspace = home.join("familiars").join("sage");
        std::fs::create_dir_all(&workspace)?;
        std::fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
        std::fs::write(
            workspace.join("ward.toml"),
            r#"principal_key_fingerprint = "fpr-val"
protected_surface = ["SOUL.md"]

[[surface]]
path = "SOUL.md"
tier = 0

[[surface]]
path = "reviewed/"
tier = 1
"#,
        )?;
        Ok(workspace)
    }

    fn post_edits(home: &Path, body: &str) -> Result<ApiResponse> {
        handle_request_with_body(
            "POST",
            "/api/v1/familiars/sage/edits",
            home,
            None,
            Some(body),
        )
    }

    #[test]
    fn post_familiar_edits_applies_and_audits_tier2_write() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let workspace = seed_warded_familiar(home)?;

        let response = post_edits(
            home,
            r#"{"edits":[{"target":"notes/today.md","contents":"hello ward"}]}"#,
        )?;

        assert_eq!(response.status, 200, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["ok"], true);
        assert_eq!(body["disposition"], "applied");
        assert_eq!(body["changes"][0]["tier"], 2);
        assert_eq!(body["changes"][0]["disposition"], "applied");
        // Gate 4: a tier-2 write must carry a tamper-evident audit record.
        assert!(
            body["changes"][0]["audit"]["nextSha256"].is_string(),
            "expected audit record, got {}",
            body["changes"][0]
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("notes/today.md"))?,
            "hello ward"
        );
        Ok(())
    }

    #[test]
    fn post_familiar_edits_refuses_traversal_and_writes_nothing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let workspace = seed_warded_familiar(home)?;

        // All-or-nothing: a clean tier-2 edit bundled with a traversal escape
        // must not be written either.
        let response = post_edits(
            home,
            r#"{"edits":[
                {"target":"notes/ok.md","contents":"fine"},
                {"target":"../escape.md","contents":"nope"}
            ]}"#,
        )?;

        assert_eq!(response.status, 403, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "ward_refused");
        assert!(!workspace.join("notes/ok.md").exists());
        assert!(!home.join("familiars/escape.md").exists());
        Ok(())
    }

    #[test]
    fn post_familiar_edits_refuses_unsigned_protected_write() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let workspace = seed_warded_familiar(home)?;

        let response = post_edits(
            home,
            r#"{"edits":[{"target":"SOUL.md","contents":"new identity"}]}"#,
        )?;

        assert_eq!(response.status, 403, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "ward_refused");
        assert_eq!(
            body["error"]["details"]["changes"][0]["verdict"]["kind"],
            "blocked"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("SOUL.md"))?,
            "# Sage\n"
        );
        Ok(())
    }

    #[test]
    fn post_familiar_edits_holds_authorized_protected_write() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let workspace = seed_warded_familiar(home)?;

        // Gate 1 passes, but Gate 3 (coherence) is unimplemented: held, not
        // written — fail-closed.
        let response = post_edits(
            home,
            r#"{"edits":[{"target":"SOUL.md","contents":"new identity"}],
                "principalKeyFingerprint":"fpr-val"}"#,
        )?;

        assert_eq!(response.status, 202, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["disposition"], "held");
        // An authorized-protected hold is the authority lane's business —
        // it must NOT be staged into the Gate-3 coherence lane.
        assert!(body.get("reviewKind").is_none());
        assert_eq!(
            std::fs::read_to_string(workspace.join("SOUL.md"))?,
            "# Sage\n"
        );
        // The coven-threads gate ran first and permitted: the verdict is in
        // the payload and in the append-only ward_audit ledger.
        assert_eq!(
            body["threadsGate"]["outcome"]["kind"], "permitted",
            "got {}",
            response.body
        );
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let decision: String = conn.query_row(
            "SELECT decision FROM ward_audit WHERE familiar_id='sage' ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(decision, "permit");
        Ok(())
    }

    #[test]
    fn post_familiar_edits_stages_to_pending_after_out_of_band_drift() -> Result<()> {
        // §5 of the coven-threads design (DegradeToProposal), end to end:
        // baseline the surface, drift it outside the daemon, then propose —
        // the write is staged at ~/.coven/pending/, the surface is untouched,
        // and `staged` is the one additive disposition the §6 compatibility
        // contract allows.
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let workspace = seed_warded_familiar(home)?;

        // First signed request bootstraps the content baseline (held).
        let first = post_edits(
            home,
            r#"{"edits":[{"target":"SOUL.md","contents":"new identity"}],
                "principalKeyFingerprint":"fpr-val"}"#,
        )?;
        assert_eq!(first.status, 202, "got {}", first.body);

        // Out-of-band drift: something edits SOUL.md around the daemon.
        std::fs::write(workspace.join("SOUL.md"), "# Mallory\n")?;

        let response = post_edits(
            home,
            r#"{"edits":[{"target":"SOUL.md","contents":"new identity v2"}],
                "principalKeyFingerprint":"fpr-val"}"#,
        )?;
        assert_eq!(response.status, 202, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["disposition"], "staged");
        assert_eq!(body["threadsGate"]["outcome"]["kind"], "staged");

        let pending = body["threadsGate"]["outcome"]["pendingPath"]
            .as_str()
            .expect("staged outcome carries pendingPath");
        assert!(
            std::path::Path::new(pending).exists(),
            "pending proposal file must exist"
        );
        // The staged proposal carries the full desired contents.
        let staged: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(pending)?)?;
        assert_eq!(staged["edits"][0]["surface"], "SOUL.md");
        // Nothing wrote the protected surface.
        assert_eq!(
            std::fs::read_to_string(workspace.join("SOUL.md"))?,
            "# Mallory\n"
        );
        // The degrade decision is in the append-only ledger.
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let decision: String = conn.query_row(
            "SELECT decision FROM ward_audit WHERE familiar_id='sage' ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(decision, "degrade_to_proposal");
        Ok(())
    }

    #[test]
    fn post_familiar_edits_blocked_target_refuses_even_with_drifted_surface() -> Result<()> {
        // Review finding: a mixed proposal (drifted Tier-0 edit + traversal
        // escape) must be refused as a unit BEFORE the threads gate can stage
        // it — a blocked target must never ride into ~/.coven/pending/.
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let workspace = seed_warded_familiar(home)?;

        // Baseline SOUL.md, then drift it so the gate would want to stage.
        let first = post_edits(
            home,
            r#"{"edits":[{"target":"SOUL.md","contents":"new identity"}],
                "principalKeyFingerprint":"fpr-val"}"#,
        )?;
        assert_eq!(first.status, 202, "got {}", first.body);
        std::fs::write(workspace.join("SOUL.md"), "# Mallory\n")?;

        let response = post_edits(
            home,
            r#"{"edits":[
                {"target":"SOUL.md","contents":"new identity v2"},
                {"target":"../escape.md","contents":"nope"}
            ], "principalKeyFingerprint":"fpr-val"}"#,
        )?;
        assert_eq!(response.status, 403, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "ward_refused");
        // Nothing was staged and nothing escaped.
        let pending = home.join("pending");
        let staged_count = std::fs::read_dir(&pending)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(staged_count, 0, "blocked proposal must not stage");
        assert!(!home.join("familiars/escape.md").exists());
        Ok(())
    }

    #[test]
    fn post_familiar_edits_editable_tier_bypasses_the_weave() -> Result<()> {
        // Editable-tier writes are the Ward tiers' lane: the weave reports no
        // verdicts and appends nothing to ward_audit.
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_warded_familiar(home)?;
        let response = post_edits(
            home,
            r#"{"edits":[{"target":"notes/today.md","contents":"hello ward"}]}"#,
        )?;
        assert_eq!(response.status, 200, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(
            body["threadsGate"]["verdicts"].as_array().map(Vec::len),
            Some(0)
        );
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM ward_audit", [], |row| row.get(0))?;
        assert_eq!(count, 0);
        Ok(())
    }

    #[test]
    fn threads_weaves_returns_cave_normalizable_weave_entries() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_warded_familiar(home)?;

        let baseline = post_edits(
            home,
            r#"{"edits":[{"target":"SOUL.md","contents":"new identity"}],
                "principalKeyFingerprint":"fpr-val"}"#,
        )?;
        assert_eq!(baseline.status, 202, "got {}", baseline.body);

        let response = handle_request("GET", "/api/v1/threads/weaves", home, None)?;
        assert_eq!(response.status, 200, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        let entries = body.as_array().expect("daemon returns a raw entry array");
        assert_eq!(entries.len(), 1);
        let entry = &entries[0];
        assert_eq!(entry["coherence"], "Coherent");
        assert_eq!(entry["weave"]["familiar_id"], "sage");
        assert!(entry["weave"]["id"].is_string());
        assert!(entry["weave"]["weave_hash"]
            .as_array()
            .is_some_and(|v| !v.is_empty()));
        assert_eq!(entry["weave"]["threads"][0]["surface"], "SOUL.md");
        assert_eq!(entry["weave"]["threads"][0]["writer"], "principal:fpr-val");
        assert_eq!(entry["weave"]["threads"][0]["tension"], "Holds");
        assert!(entry["weave"]["threads"][0]["created_at"].is_array());
        assert!(entry["weave"]["threads"][0]["strands"]
            .as_array()
            .is_some_and(|v| !v.is_empty()));
        assert!(entry["weave"]["pattern_descriptor"]["name"].is_string());
        Ok(())
    }

    #[test]
    fn threads_weaves_skips_malformed_ward_without_aborting_fleet_read() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_warded_familiar(home)?;

        let cody_workspace = home.join("familiars").join("cody");
        std::fs::create_dir_all(&cody_workspace)?;
        std::fs::write(cody_workspace.join("SOUL.md"), "# Cody\n")?;
        std::fs::write(
            cody_workspace.join("ward.toml"),
            r#"protected_surface = ["SOUL.md"]

[[surface]]
path = "SOUL.md"
tier = 0
"#,
        )?;

        let response = handle_request("GET", "/api/v1/threads/weaves", home, None)?;

        assert_eq!(response.status, 200, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        let entries = body.as_array().expect("weaves response is an array");
        assert_eq!(
            entries.len(),
            2,
            "malformed cody ward must not abort healthy weave or vanish"
        );
        let healthy = entries
            .iter()
            .find(|entry| entry.get("weave").is_some())
            .expect("healthy weave is still listed");
        assert_eq!(healthy["weave"]["familiar_id"], "sage");
        let degraded = entries
            .iter()
            .find_map(|entry| entry.get("degraded"))
            .expect("malformed ward appears as a degraded familiar");
        assert_eq!(degraded["familiarId"], "cody");
        assert_eq!(degraded["reason"], "ward-config-unparseable");
        let error = degraded["error"].as_str().expect("error string");
        assert!(
            !error.contains('\n'),
            "error must be single-line: {error:?}"
        );
        assert!(
            error.contains("ward.toml"),
            "error names ward.toml: {error}"
        );
        assert!(
            !error.contains(&home.display().to_string()),
            "error must not leak absolute home path: {error}"
        );
        assert!(
            !error.contains("/familiars/cody/ward.toml"),
            "error must not leak rooted ward path: {error}"
        );
        Ok(())
    }

    #[test]
    fn threads_weaves_omits_familiars_without_ward_toml() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_warded_familiar(home)?;
        let cody_workspace = home.join("familiars").join("cody");
        std::fs::create_dir_all(&cody_workspace)?;
        std::fs::write(cody_workspace.join("SOUL.md"), "# Cody\n")?;

        let response = handle_request("GET", "/api/v1/threads/weaves", home, None)?;

        assert_eq!(response.status, 200, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        let entries = body.as_array().expect("weaves response is an array");
        assert_eq!(entries.len(), 1, "no-ward familiar should be skipped");
        assert_eq!(entries[0]["weave"]["familiar_id"], "sage");
        assert!(
            entries.iter().all(|entry| entry.get("degraded").is_none()),
            "no-ward familiar is not degraded: {entries:?}"
        );
        Ok(())
    }

    fn stage_pending_protected_edit(home: &Path) -> Result<(std::path::PathBuf, String)> {
        let workspace = seed_warded_familiar(home)?;
        let baseline = post_edits(
            home,
            r#"{"edits":[{"target":"SOUL.md","contents":"new identity"}],
                "principalKeyFingerprint":"fpr-val"}"#,
        )?;
        assert_eq!(baseline.status, 202, "got {}", baseline.body);
        std::fs::write(workspace.join("SOUL.md"), "# Mallory\n")?;
        let staged = post_edits(
            home,
            r#"{"edits":[{"target":"SOUL.md","contents":"approved identity"}],
                "principalKeyFingerprint":"fpr-val"}"#,
        )?;
        assert_eq!(staged.status, 202, "got {}", staged.body);
        let body: serde_json::Value = serde_json::from_str(&staged.body)?;
        let pending = std::path::PathBuf::from(
            body["threadsGate"]["outcome"]["pendingPath"]
                .as_str()
                .expect("staged response carries pendingPath"),
        );
        let proposal_id = body["threadsGate"]["outcome"]["proposalId"]
            .as_str()
            .expect("staged response carries proposalId")
            .to_string();
        Ok((pending, proposal_id))
    }

    fn stage_scheduled_reviewed_edit(
        home: &Path,
        approval_path: coven_threads_core::ApprovalPath,
        staged_at: time::OffsetDateTime,
    ) -> Result<(std::path::PathBuf, String)> {
        stage_scheduled_reviewed_edit_on_channel(
            home,
            approval_path,
            staged_at,
            coven_threads_core::Channel::Mutation,
        )
    }

    fn stage_scheduled_reviewed_edit_on_channel(
        home: &Path,
        approval_path: coven_threads_core::ApprovalPath,
        staged_at: time::OffsetDateTime,
        channel: coven_threads_core::Channel,
    ) -> Result<(std::path::PathBuf, String)> {
        stage_scheduled_edit(
            home,
            "reviewed/skill.md",
            1,
            approval_path,
            staged_at,
            channel,
        )
    }

    fn stage_scheduled_edit(
        home: &Path,
        target: &str,
        path_tier_floor: u8,
        approval_path: coven_threads_core::ApprovalPath,
        staged_at: time::OffsetDateTime,
        channel: coven_threads_core::Channel,
    ) -> Result<(std::path::PathBuf, String)> {
        let workspace = seed_warded_familiar(home)?;
        if let Some(parent) = workspace.join(target).parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(workspace.join(target), b"before")?;
        let proposal_id = coven_threads_core::ProposalId::new();
        let familiar_id = crate::threads_gate::familiar_weave_id("sage");
        let surface = coven_threads_core::SurfaceId::new(target);
        let pending = coven_threads_core::PendingProposal {
            id: proposal_id,
            familiar_id,
            writer: coven_threads_core::WriterId::new("principal:fpr-val"),
            channel,
            thread_id: coven_threads_core::ThreadId::new(),
            fray: coven_threads_core::FrayOrSnap::Frayed {
                strand: None,
                channel,
                reason: coven_threads_core::FrayReason::Other("phase-5 decision".to_string()),
            },
            edits: vec![coven_threads_core::StagedEdit {
                surface: surface.clone(),
                contents: coven_threads_core::StagedContents::from_bytes(b"after"),
            }],
            staged_at,
        };
        let diff =
            coven_threads_core::MaterializedDiff::try_new(vec![coven_threads_core::SurfaceDiff {
                surface: surface.clone(),
                before: Some(b"before".to_vec()),
                after: Some(b"after".to_vec()),
            }])
            .map_err(anyhow::Error::msg)?;
        let evidence =
            coven_threads_core::SurfaceRegionRegistry::default_registry().classify_all(&diff);
        let classification = coven_threads_core::ProposalClassification {
            proposal_id,
            familiar_id,
            channel,
            affected_surfaces: vec![surface],
            affected_regions: evidence.iter().map(|item| item.region_id.clone()).collect(),
            path_tier_floor,
            approval_path,
            evidence_replay_hash: coven_threads_core::evidence_replay_hash(&diff, &evidence),
            classified_at: staged_at,
        };
        let scheduled =
            crate::proposal_scheduler::ScheduledProposal::try_new(pending, classification, diff)?;
        let pending_dir = home.join("pending");
        std::fs::create_dir_all(&pending_dir)?;
        let path = pending_dir.join(format!("{familiar_id}-{proposal_id}.json"));
        std::fs::write(&path, serde_json::to_vec_pretty(&scheduled)?)?;
        Ok((path, proposal_id.to_string()))
    }

    fn scheduled_decision_body(
        home: &Path,
        proposal_id: &str,
        note: Option<&str>,
    ) -> Result<String> {
        let listed = handle_request("GET", "/api/v1/threads/proposals", home, None)?;
        let body: Value = serde_json::from_str(&listed.body)?;
        let proposal = body["proposals"]
            .as_array()
            .and_then(|proposals| {
                proposals
                    .iter()
                    .find(|proposal| proposal["proposalId"] == proposal_id)
            })
            .context("scheduled proposal is listed")?;
        let revision = proposal["proposalRevision"]
            .as_str()
            .context("scheduled proposal carries a revision")?;
        Ok(match note {
            Some(note) => json!({ "note": note, "expectedRevision": revision }).to_string(),
            None => json!({ "expectedRevision": revision }).to_string(),
        })
    }

    #[test]
    fn threads_scheduled_human_required_enforces_rationale_and_applies() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, proposal_id) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::HumanApprovalWithRationale,
            time::OffsetDateTime::now_utc(),
        )?;
        let missing_rationale_body = scheduled_decision_body(home, &proposal_id, None)?;

        let missing_rationale = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(&missing_rationale_body),
        )?;
        assert_eq!(
            missing_rationale.status, 409,
            "got {}",
            missing_rationale.body
        );
        let body: Value = serde_json::from_str(&missing_rationale.body)?;
        assert_eq!(body["why"], "proposal-rationale-required");
        assert!(pending.exists());

        let approved_body =
            scheduled_decision_body(home, &proposal_id, Some("reviewed semantic change"))?;
        let approved = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(&approved_body),
        )?;
        assert_eq!(approved.status, 200, "got {}", approved.body);
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/reviewed/skill.md"))?,
            "after"
        );
        assert!(!pending.exists());
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let detail: String = conn.query_row(
            "SELECT detail FROM ward_audit
             WHERE proposal_id = ?1 AND event_type = 'proposal_approved'",
            [&proposal_id],
            |row| row.get(0),
        )?;
        let detail: coven_threads_core::ProposalApprovalAuditDetail =
            serde_json::from_str(&detail)?;
        assert_eq!(detail.approval_path_label, "human_required");
        assert_eq!(
            detail.rationale.as_deref(),
            Some("reviewed semantic change")
        );
        Ok(())
    }

    #[test]
    fn threads_scheduled_manual_decision_requires_matching_revision() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, proposal_id) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::HumanApproval,
            time::OffsetDateTime::now_utc(),
        )?;

        let missing = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;
        assert_eq!(missing.status, 409);
        assert!(missing.body.contains("proposal-revision-required"));
        assert!(pending.exists());

        let mismatch = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(&json!({ "expectedRevision": "0".repeat(64) }).to_string()),
        )?;
        assert_eq!(mismatch.status, 409);
        assert!(mismatch.body.contains("proposal-revision-mismatch"));
        assert!(pending.exists());

        let listed = handle_request("GET", "/api/v1/threads/proposals", home, None)?;
        let body: Value = serde_json::from_str(&listed.body)?;
        let revision = body["proposals"][0]["proposalRevision"]
            .as_str()
            .context("proposal list carries revision")?;
        let approved = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(&json!({ "expectedRevision": revision }).to_string()),
        )?;
        assert_eq!(approved.status, 200, "got {}", approved.body);
        Ok(())
    }

    #[test]
    fn threads_scheduled_audits_preserve_committed_channel() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_, proposal_id) = stage_scheduled_reviewed_edit_on_channel(
            home,
            coven_threads_core::ApprovalPath::HumanApproval,
            time::OffsetDateTime::now_utc(),
            coven_threads_core::Channel::Serialization,
        )?;
        let decision_body = scheduled_decision_body(home, &proposal_id, None)?;

        let approved = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(&decision_body),
        )?;
        assert_eq!(approved.status, 200, "got {}", approved.body);
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let mismatched: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE proposal_id = ?1 AND channel != 'serialization'",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(mismatched, 0);
        Ok(())
    }

    #[test]
    fn scheduled_apply_intent_uses_committed_before_image() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, _) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::HumanApproval,
            time::OffsetDateTime::now_utc(),
        )?;
        let scheduled: crate::proposal_scheduler::ScheduledProposal =
            serde_json::from_slice(&std::fs::read(pending)?)?;
        let workspace = home.join("familiars/sage");
        std::fs::write(workspace.join("reviewed/skill.md"), b"concurrent")?;

        let before_images = proposal_before_images(
            &workspace,
            &["reviewed/skill.md".to_string()],
            Some(&scheduled),
        )?;

        assert_eq!(
            before_images[0]
                .contents
                .to_bytes()
                .map_err(anyhow::Error::msg)?,
            b"before"
        );
        Ok(())
    }

    #[test]
    fn threads_scheduled_veto_window_delays_apply_and_records_veto() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let veto = coven_threads_core::VetoWindow::new(
            std::time::Duration::from_secs(300),
            std::time::Duration::from_secs(60),
        );
        let (pending, proposal_id) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::FamiliarCoherence { veto },
            time::OffsetDateTime::now_utc(),
        )?;
        let premature_body = scheduled_decision_body(home, &proposal_id, None)?;

        let premature = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(&premature_body),
        )?;
        assert_eq!(premature.status, 409, "got {}", premature.body);
        let body: Value = serde_json::from_str(&premature.body)?;
        assert_eq!(body["why"], "proposal-minimum-visibility-open");
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/reviewed/skill.md"))?,
            "before"
        );

        let veto_body = scheduled_decision_body(home, &proposal_id, Some("familiar objected"))?;
        let vetoed = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/reject"),
            home,
            None,
            Some(&veto_body),
        )?;
        assert_eq!(vetoed.status, 200, "got {}", vetoed.body);
        let body: Value = serde_json::from_str(&vetoed.body)?;
        assert_eq!(body["decision"], "vetoed");
        assert!(!pending.exists());
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let (event, detail): (String, String) = conn.query_row(
            "SELECT event_type, detail FROM ward_audit WHERE proposal_id = ?1",
            [&proposal_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(event, "proposal_vetoed");
        let close: coven_threads_core::ProposalWindowCloseAuditDetail =
            serde_json::from_str(&detail)?;
        assert_eq!(close.reason, coven_threads_core::WindowCloseReason::Vetoed);
        assert_eq!(close.replay_hash_matched, None);
        assert_eq!(close.rationale.as_deref(), Some("familiar objected"));
        Ok(())
    }

    #[test]
    fn threads_scheduled_applies_only_after_veto_deadline() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let veto = coven_threads_core::VetoWindow::new(
            std::time::Duration::from_secs(300),
            std::time::Duration::from_secs(60),
        );
        let (_, proposal_id) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::FamiliarCoherence { veto },
            time::OffsetDateTime::now_utc() - time::Duration::minutes(10),
        )?;
        let decision_body = scheduled_decision_body(home, &proposal_id, None)?;

        let approved = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(&decision_body),
        )?;

        assert_eq!(approved.status, 200, "got {}", approved.body);
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/reviewed/skill.md"))?,
            "after"
        );
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let detail: String = conn.query_row(
            "SELECT detail FROM ward_audit
             WHERE proposal_id = ?1 AND event_type = 'proposal_approved'",
            [&proposal_id],
            |row| row.get(0),
        )?;
        let detail: coven_threads_core::ProposalApprovalAuditDetail =
            serde_json::from_str(&detail)?;
        assert_eq!(detail.approval_path_label, "familiar_review");
        let close = detail
            .window_close
            .expect("delayed apply records window close");
        assert_eq!(close.reason, coven_threads_core::WindowCloseReason::Applied);
        assert_eq!(close.replay_hash_matched, Some(true));
        Ok(())
    }

    #[test]
    fn threads_scheduled_deadline_replay_refuses_diverged_before_image() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, proposal_id) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::HumanApproval,
            time::OffsetDateTime::now_utc(),
        )?;
        let target = home.join("familiars/sage/reviewed/skill.md");
        std::fs::write(&target, "concurrent")?;
        let decision_body = scheduled_decision_body(home, &proposal_id, None)?;

        let response = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(&decision_body),
        )?;

        assert_eq!(response.status, 409, "got {}", response.body);
        let body: Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["why"], "proposal-evidence-diverged");
        assert_eq!(std::fs::read_to_string(target)?, "concurrent");
        assert!(!pending.exists(), "failed deadline replay is terminal");
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let event: String = conn.query_row(
            "SELECT event_type FROM ward_audit
             WHERE proposal_id = ?1 AND event_type = 'proposal_rejected'",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(event, "proposal_rejected");
        Ok(())
    }

    #[test]
    fn threads_scheduled_rejects_live_promotion_to_protected_tier() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, proposal_id) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::HumanApproval,
            time::OffsetDateTime::now_utc(),
        )?;
        let ward_path = home.join("familiars/sage/ward.toml");
        let ward = std::fs::read_to_string(&ward_path)?
            .replace(
                "protected_surface = [\"SOUL.md\"]",
                "protected_surface = [\"SOUL.md\", \"reviewed/skill.md\"]",
            )
            .replace(
                "path = \"reviewed/\"\ntier = 1",
                "path = \"reviewed/skill.md\"\ntier = 0",
            );
        std::fs::write(&ward_path, ward)?;
        let decision_body = scheduled_decision_body(home, &proposal_id, None)?;

        let response = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(&decision_body),
        )?;

        assert_eq!(response.status, 409, "got {}", response.body);
        let body: Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["why"], "proposal-live-tier-escalated");
        assert!(!pending.exists());
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/reviewed/skill.md"))?,
            "before"
        );
        Ok(())
    }

    #[test]
    fn threads_scheduled_recovery_reparses_authority_envelope() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_, proposal_id) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::HumanApprovalWithRationale,
            time::OffsetDateTime::now_utc(),
        )?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ApplyBeforeAudit,
            proposal_id.clone(),
        )));
        let decision_body = scheduled_decision_body(home, &proposal_id, Some("durable rationale"))?;
        let interrupted = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(&decision_body),
        );
        assert!(interrupted.is_err());

        let retry = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            None,
        )?;

        assert_eq!(retry.status, 200, "got {}", retry.body);
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/reviewed/skill.md"))?,
            "after"
        );
        Ok(())
    }

    #[test]
    fn threads_scheduler_opens_window_once_and_waits_until_deadline() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let veto = coven_threads_core::VetoWindow::new(
            std::time::Duration::from_secs(300),
            std::time::Duration::from_secs(60),
        );
        let (_, proposal_id) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::FamiliarCoherence { veto },
            time::OffsetDateTime::now_utc(),
        )?;

        assert_eq!(process_due_threads_proposals(home)?, 0);
        assert_eq!(process_due_threads_proposals(home)?, 0);
        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/reviewed/skill.md"))?,
            "before"
        );
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE proposal_id = ?1 AND event_type = 'proposal_window_opened'",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(
            count,
            1,
            "scheduler log: {}",
            std::fs::read_to_string(crate::daemon::daemon_recovery_log_path(home))
                .unwrap_or_default()
        );
        Ok(())
    }

    #[test]
    fn threads_scheduler_applies_due_delayed_proposal() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let veto = coven_threads_core::VetoWindow::new(
            std::time::Duration::from_secs(300),
            std::time::Duration::from_secs(60),
        );
        let (_, proposal_id) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::FamiliarCoherence { veto },
            time::OffsetDateTime::now_utc() - time::Duration::minutes(10),
        )?;

        let completed = process_due_threads_proposals(home)?;
        assert_eq!(
            completed,
            1,
            "scheduler log: {}",
            std::fs::read_to_string(crate::daemon::daemon_recovery_log_path(home))
                .unwrap_or_default()
        );

        assert_eq!(
            std::fs::read_to_string(home.join("familiars/sage/reviewed/skill.md"))?,
            "after"
        );
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let events: Vec<String> = {
            let mut statement = conn.prepare(
                "SELECT event_type FROM ward_audit
                 WHERE proposal_id = ?1 ORDER BY id",
            )?;
            let rows = statement
                .query_map([&proposal_id], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            rows
        };
        assert_eq!(
            events.first().map(String::as_str),
            Some("proposal_window_opened")
        );
        assert_eq!(events.last().map(String::as_str), Some("proposal_approved"));
        assert_eq!(
            events
                .iter()
                .filter(|event| event.as_str() == "proposal_window_opened")
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn threads_scheduler_recovers_durable_approval_claim() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_, proposal_id) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::HumanApprovalWithRationale,
            time::OffsetDateTime::now_utc(),
        )?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ApplyBeforeAudit,
            proposal_id.clone(),
        )));
        let decision_body =
            scheduled_decision_body(home, &proposal_id, Some("scheduler recovery"))?;
        let interrupted = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(&decision_body),
        );
        assert!(interrupted.is_err());

        assert_eq!(process_due_threads_proposals(home)?, 1);

        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let detail: String = conn.query_row(
            "SELECT detail FROM ward_audit
             WHERE proposal_id = ?1 AND event_type = 'proposal_approved'",
            [&proposal_id],
            |row| row.get(0),
        )?;
        let detail: coven_threads_core::ProposalApprovalAuditDetail =
            serde_json::from_str(&detail)?;
        assert_eq!(detail.rationale.as_deref(), Some("scheduler recovery"));
        Ok(())
    }

    #[test]
    fn threads_scheduler_recovers_rationale_persisted_at_claim_time() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_, proposal_id) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::HumanApprovalWithRationale,
            time::OffsetDateTime::now_utc(),
        )?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ClaimBeforeValidation,
            proposal_id.clone(),
        )));
        let decision_body =
            scheduled_decision_body(home, &proposal_id, Some("claim-time rationale"))?;
        let interrupted = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(&decision_body),
        );
        assert!(interrupted.is_err());

        assert_eq!(process_due_threads_proposals(home)?, 1);

        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let detail: String = conn.query_row(
            "SELECT detail FROM ward_audit
             WHERE proposal_id = ?1 AND event_type = 'proposal_approved'",
            [&proposal_id],
            |row| row.get(0),
        )?;
        let detail: coven_threads_core::ProposalApprovalAuditDetail =
            serde_json::from_str(&detail)?;
        assert_eq!(detail.rationale.as_deref(), Some("claim-time rationale"));
        Ok(())
    }

    #[test]
    fn threads_scheduler_recovers_veto_claimed_before_deadline() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let veto = coven_threads_core::VetoWindow::new(
            std::time::Duration::from_secs(300),
            std::time::Duration::ZERO,
        );
        // Stage in the past so the veto deadline (staged_at + 300s) has
        // already elapsed by the time recovery runs — no wall-clock sleep.
        let staged_at = time::OffsetDateTime::now_utc() - time::Duration::minutes(10);
        let (pending_path, proposal_id) = stage_scheduled_reviewed_edit(
            home,
            coven_threads_core::ApprovalPath::FamiliarCoherence { veto },
            staged_at,
        )?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ClaimBeforeValidation,
            proposal_id.clone(),
        )));
        let decision_body = scheduled_decision_body(home, &proposal_id, Some("timely veto"))?;
        let interrupted = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/reject"),
            home,
            None,
            Some(&decision_body),
        );
        assert!(interrupted.is_err());

        // The interrupted claim durably recorded claimed_at = wall-clock now,
        // which is after the backdated deadline. Recovery judges veto
        // timeliness by the durable claimed_at alone, so pin it inside the
        // window to model a claim that landed before the deadline. This keeps
        // the scenario — timely claim, post-deadline replay — deterministic
        // instead of racing a real clock (flaky on slow CI runners, #455).
        let raw = std::fs::read_to_string(&pending_path)?;
        let mut value: Value = serde_json::from_str(&raw)?;
        let mut request = proposal_decision_request(&value)?
            .context("interrupted reject left a durable decision request")?;
        request.claimed_at = staged_at + time::Duration::seconds(1);
        value
            .as_object_mut()
            .context("pending proposal is a JSON object")?
            .insert(
                "decisionRequest".to_string(),
                serde_json::to_value(&request)?,
            );
        std::fs::write(&pending_path, serde_json::to_vec_pretty(&value)?)?;

        let recovered = process_due_threads_proposals(home)?;
        assert_eq!(recovered, 1);
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let event: String = conn.query_row(
            "SELECT event_type FROM ward_audit
             WHERE proposal_id = ?1 AND event_type = 'proposal_vetoed'",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(event, "proposal_vetoed");
        Ok(())
    }

    #[test]
    fn invalid_manual_decision_does_not_block_automatic_apply() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_, proposal_id) = stage_scheduled_edit(
            home,
            "logged/skill.md",
            2,
            coven_threads_core::ApprovalPath::AutoRegression { veto: None },
            time::OffsetDateTime::now_utc(),
            coven_threads_core::Channel::Mutation,
        )?;
        let decision_body = scheduled_decision_body(home, &proposal_id, None)?;

        let rejected = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/reject"),
            home,
            None,
            Some(&decision_body),
        )?;
        assert_eq!(rejected.status, 409);
        assert!(rejected.body.contains("proposal-not-human-decidable"));

        let processed = process_due_threads_proposals(home)?;
        assert_eq!(processed, 1);
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let event: String = conn.query_row(
            "SELECT event_type FROM ward_audit
             WHERE proposal_id = ?1 AND event_type = 'proposal_approved'",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(event, "proposal_approved");
        Ok(())
    }

    #[test]
    fn threads_approve_revalidates_applies_audits_and_removes_pending() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, proposal_id) = stage_pending_protected_edit(home)?;
        let workspace = home.join("familiars").join("sage");
        std::fs::write(workspace.join("SOUL.md"), "# Sage\n")?;

        let response = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(r#"{"note":"principal reviewed"}"#),
        )?;

        assert_eq!(response.status, 200, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["decision"], "approved");
        assert_eq!(body["proposalId"], proposal_id);
        assert_eq!(
            std::fs::read_to_string(workspace.join("SOUL.md"))?,
            "approved identity"
        );
        assert!(!pending.exists(), "approved proposal must be removed");
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let (event_type, detail): (String, String) = conn.query_row(
            "SELECT event_type, detail
             FROM ward_audit
             WHERE proposal_id = ?1
             ORDER BY id DESC
             LIMIT 1",
            [&proposal_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(event_type, "proposal_approved");
        let detail: coven_threads_core::ProposalApprovalAuditDetail =
            serde_json::from_str(&detail)?;
        assert_eq!(detail.approval_path_label, "human_review");
        assert_eq!(detail.rationale.as_deref(), Some("principal reviewed"));
        assert_eq!(detail.window_close, None);
        Ok(())
    }

    #[test]
    fn threads_approve_recovers_after_apply_before_audit() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, proposal_id) = stage_pending_protected_edit(home)?;
        let workspace = home.join("familiars").join("sage");
        std::fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ApplyBeforeAudit,
            proposal_id.clone(),
        )));

        let first = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(r#"{"note":"principal reviewed"}"#),
        );

        assert!(first.is_err(), "failpoint must interrupt the decision");
        assert_eq!(
            std::fs::read_to_string(workspace.join("SOUL.md"))?,
            "approved identity"
        );
        assert!(
            !pending.exists(),
            "the original file must be atomically claimed"
        );
        let claim = find_pending_decision_claim(home, &proposal_id, "approve")
            .expect("interrupted approval leaves a durable claim");
        assert!(claim.exists());
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let terminal_count: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM ward_audit
             WHERE proposal_id = ?1
               AND event_type IN ('proposal_approved', 'proposal_rejected', 'proposal_vetoed')",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(terminal_count, 0);
        drop(conn);

        let retry = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some(r#"{"note":"principal reviewed"}"#),
        )?;

        assert_eq!(retry.status, 200, "got {}", retry.body);
        assert!(!claim.exists(), "successful recovery consumes the claim");
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let approved_count: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM ward_audit
             WHERE proposal_id = ?1 AND event_type = 'proposal_approved'",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(approved_count, 1);
        Ok(())
    }

    #[test]
    fn threads_approve_recovery_ward_refusal_restores_retryable_pending() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, proposal_id) = stage_pending_protected_edit(home)?;
        let workspace = home.join("familiars").join("sage");
        std::fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ApplyBeforeAudit,
            proposal_id.clone(),
        )));
        let interrupted = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        );
        assert!(interrupted.is_err());
        force_recovery_ward_refusal(proposal_id.clone());

        let refused = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;

        assert_eq!(refused.status, 409, "got {}", refused.body);
        let body: serde_json::Value = serde_json::from_str(&refused.body)?;
        assert_eq!(body["why"], "proposal-recovery-revalidation-failed");
        assert!(
            pending.exists(),
            "refused recovery must restore pending JSON"
        );
        let restored: serde_json::Value = serde_json::from_slice(&std::fs::read(&pending)?)?;
        assert!(
            restored.get("decisionState").is_none(),
            "restored proposal must not retain recovery-only decision state"
        );
        assert!(
            find_pending_decision_claim(home, &proposal_id, "approve").is_none(),
            "refused recovery must consume the claimed filename"
        );

        let retry = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;
        assert_eq!(retry.status, 200, "got {}", retry.body);
        Ok(())
    }

    #[test]
    fn threads_approve_recovery_preserves_claim_if_ward_config_diverged() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, proposal_id) = stage_pending_protected_edit(home)?;
        let workspace = home.join("familiars").join("sage");
        std::fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ApplyBeforeAudit,
            proposal_id.clone(),
        )));
        let interrupted = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        );
        assert!(interrupted.is_err());
        let claim = find_pending_decision_claim(home, &proposal_id, "approve")
            .expect("interrupted approval leaves a recovery claim");
        let ward_path = workspace.join("ward.toml");
        let changed_ward = std::fs::read_to_string(&ward_path)?.replace("fpr-val", "fpr-other");
        std::fs::write(&ward_path, changed_ward)?;

        let retry = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;

        assert_eq!(retry.status, 409, "got {}", retry.body);
        let body: serde_json::Value = serde_json::from_str(&retry.body)?;
        assert_eq!(body["why"], "proposal-recovery-evidence-diverged");
        assert!(claim.exists(), "diverged recovery must retain its claim");
        assert!(
            !pending.exists(),
            "claim must remain the sole proposal file"
        );
        assert_eq!(
            std::fs::read_to_string(workspace.join("SOUL.md"))?,
            "approved identity"
        );
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let terminal_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit
             WHERE proposal_id = ?1
               AND event_type IN ('proposal_approved', 'proposal_rejected', 'proposal_vetoed')",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(terminal_count, 0);
        Ok(())
    }

    #[test]
    fn threads_approve_recovery_preserves_claim_if_ward_disappears() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_, proposal_id) = stage_pending_protected_edit(home)?;
        let workspace = home.join("familiars").join("sage");
        std::fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ApplyBeforeAudit,
            proposal_id.clone(),
        )));
        let interrupted = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        );
        assert!(interrupted.is_err());
        let claim = find_pending_decision_claim(home, &proposal_id, "approve")
            .expect("interrupted approval leaves a recovery claim");
        std::fs::remove_file(workspace.join("ward.toml"))?;

        let retry = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;

        assert_eq!(retry.status, 409, "got {}", retry.body);
        let body: Value = serde_json::from_str(&retry.body)?;
        assert_eq!(body["why"], "ward-not-configured");
        assert!(
            claim.exists(),
            "recovery claim must not downgrade to pending"
        );
        Ok(())
    }

    #[test]
    fn pending_claim_search_skips_unrelated_directory_entries() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let pending = home.join("pending");
        std::fs::create_dir_all(&pending)?;
        std::fs::write(pending.join("000-unrelated"), "junk")?;
        let proposal_id = Uuid::new_v4().to_string();
        let claim = pending.join(format!(
            "{}-{proposal_id}.json.approve.deciding",
            Uuid::new_v4()
        ));
        std::fs::write(&claim, "{}")?;

        let found = find_any_pending_decision_claim(home, &proposal_id);

        assert_eq!(found, Some((claim, "approve".to_string())));
        Ok(())
    }

    #[test]
    fn threads_approve_recovery_refuses_concurrent_surface_bytes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_, proposal_id) = stage_pending_protected_edit(home)?;
        let workspace = home.join("familiars").join("sage");
        std::fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::ApplyBeforeAudit,
            proposal_id.clone(),
        )));
        let first = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        );
        assert!(first.is_err());
        std::fs::write(workspace.join("SOUL.md"), "concurrent bytes")?;

        let retry = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;

        assert_eq!(retry.status, 409, "got {}", retry.body);
        let body: serde_json::Value = serde_json::from_str(&retry.body)?;
        assert_eq!(body["why"], "proposal-recovery-surface-diverged");
        let baseline = ward_manifest_entry_hash(home, "sage", "SOUL.md")?;
        assert_eq!(
            baseline,
            coven_threads_core::manifest_entry_hash(
                &coven_threads_core::SurfaceId::new("SOUL.md"),
                b"# Sage\n",
            )
            .to_vec(),
            "unapproved concurrent bytes must not become the baseline"
        );
        Ok(())
    }

    #[test]
    fn threads_approve_retry_after_audit_is_idempotent() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_, proposal_id) = stage_pending_protected_edit(home)?;
        let workspace = home.join("familiars").join("sage");
        std::fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::AuditBeforeCleanup,
            proposal_id.clone(),
        )));

        let first = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        );

        assert!(first.is_err(), "failpoint must interrupt pending cleanup");
        let claim = find_pending_decision_claim(home, &proposal_id, "approve")
            .expect("committed approval leaves its claim until recovery");
        assert!(claim.exists());

        let retry = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;

        assert_eq!(retry.status, 200, "got {}", retry.body);
        assert!(!claim.exists());
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let approved_count: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM ward_audit
             WHERE proposal_id = ?1 AND event_type = 'proposal_approved'",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(approved_count, 1, "retry must not duplicate terminal audit");
        Ok(())
    }

    #[test]
    fn threads_completed_decision_uses_terminal_audit_without_pending_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_, proposal_id) = stage_pending_protected_edit(home)?;
        let workspace = home.join("familiars").join("sage");
        std::fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
        let approved = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;
        assert_eq!(approved.status, 200, "got {}", approved.body);

        let retry = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;
        assert_eq!(retry.status, 200, "got {}", retry.body);
        let retry_body: serde_json::Value = serde_json::from_str(&retry.body)?;
        assert_eq!(retry_body["idempotent"], true);

        let opposite = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/reject"),
            home,
            None,
            Some("{}"),
        )?;
        assert_eq!(opposite.status, 409, "got {}", opposite.body);
        let opposite_body: serde_json::Value = serde_json::from_str(&opposite.body)?;
        assert_eq!(opposite_body["why"], "proposal-already-decided");
        Ok(())
    }

    fn ward_manifest_entry_hash(home: &Path, familiar_id: &str, surface: &str) -> Result<Vec<u8>> {
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        conn.query_row(
            "SELECT entry_hash FROM ward_manifest WHERE familiar_id = ?1 AND surface = ?2",
            [familiar_id, surface],
            |row| row.get(0),
        )
        .map_err(Into::into)
    }

    #[test]
    fn threads_approve_advances_baseline_after_apply() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, proposal_id) = stage_pending_protected_edit(home)?;
        let before = ward_manifest_entry_hash(home, "sage", "SOUL.md")?;
        let workspace = home.join("familiars").join("sage");
        std::fs::write(workspace.join("SOUL.md"), "# Sage\n")?;

        let response = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;

        assert_eq!(response.status, 200, "got {}", response.body);
        assert!(!pending.exists(), "approved proposal must be removed");
        let after = ward_manifest_entry_hash(home, "sage", "SOUL.md")?;
        assert_ne!(after, before, "baseline must advance to the approved bytes");
        assert_eq!(
            after,
            coven_threads_core::manifest_entry_hash(
                &coven_threads_core::SurfaceId::new("SOUL.md"),
                b"approved identity"
            )
            .to_vec()
        );
        Ok(())
    }

    #[test]
    fn threads_approve_second_cycle_succeeds_after_baseline_advance() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (first_pending, first_proposal_id) = stage_pending_protected_edit(home)?;
        let workspace = home.join("familiars").join("sage");
        std::fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
        let first = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{first_proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;
        assert_eq!(first.status, 200, "got {}", first.body);
        assert!(!first_pending.exists());

        std::fs::write(workspace.join("SOUL.md"), "# Eve\n")?;
        let staged = post_edits(
            home,
            r#"{"edits":[{"target":"SOUL.md","contents":"second identity"}],
                "principalKeyFingerprint":"fpr-val"}"#,
        )?;
        assert_eq!(staged.status, 202, "got {}", staged.body);
        let body: serde_json::Value = serde_json::from_str(&staged.body)?;
        let second_pending = std::path::PathBuf::from(
            body["threadsGate"]["outcome"]["pendingPath"]
                .as_str()
                .expect("staged response carries pendingPath"),
        );
        let second_proposal_id = body["threadsGate"]["outcome"]["proposalId"]
            .as_str()
            .expect("staged response carries proposalId");

        std::fs::write(workspace.join("SOUL.md"), "approved identity")?;
        let second = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{second_proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;

        assert_eq!(second.status, 200, "got {}", second.body);
        assert_eq!(
            std::fs::read_to_string(workspace.join("SOUL.md"))?,
            "second identity"
        );
        assert!(
            !second_pending.exists(),
            "approved second proposal is consumed"
        );
        Ok(())
    }

    #[test]
    fn threads_approve_refusal_keeps_pending_audits_and_retry_can_succeed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, proposal_id) = stage_pending_protected_edit(home)?;
        let workspace = home.join("familiars").join("sage");

        let response = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;

        assert_eq!(response.status, 409, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["blocked"], true);
        assert_eq!(body["why"], "proposal-revalidation-failed");
        assert_eq!(
            std::fs::read_to_string(workspace.join("SOUL.md"))?,
            "# Mallory\n"
        );
        assert!(
            pending.exists(),
            "refused approve must leave the pending proposal retryable"
        );
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let proposal_audit_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit WHERE proposal_id = ?1",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert!(
            proposal_audit_count > 0,
            "refused approve must leave proposal-scoped audit evidence"
        );
        let approved_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit WHERE event_type='proposal_approved' AND proposal_id = ?1",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(approved_count, 0);
        let terminal_count: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM ward_audit
             WHERE proposal_id = ?1
               AND event_type IN ('proposal_approved', 'proposal_rejected', 'proposal_vetoed')",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(
            terminal_count, 0,
            "a retryable refusal must not close the proposal lifecycle"
        );

        std::fs::write(workspace.join("SOUL.md"), "# Sage\n")?;
        let retry = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;
        assert_eq!(retry.status, 200, "got {}", retry.body);
        assert_eq!(
            std::fs::read_to_string(workspace.join("SOUL.md"))?,
            "approved identity"
        );
        assert!(!pending.exists(), "approved retry consumes proposal");
        Ok(())
    }

    #[test]
    fn threads_reject_audits_removes_pending_and_does_not_apply() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (pending, proposal_id) = stage_pending_protected_edit(home)?;
        let workspace = home.join("familiars").join("sage");

        let response = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/reject"),
            home,
            None,
            Some(r#"{"note":"not this change"}"#),
        )?;

        assert_eq!(response.status, 200, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["decision"], "rejected");
        assert_eq!(
            std::fs::read_to_string(workspace.join("SOUL.md"))?,
            "# Mallory\n"
        );
        assert!(!pending.exists(), "rejected proposal must be removed");
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let event_type: String = conn.query_row(
            "SELECT event_type FROM ward_audit WHERE proposal_id = ?1 ORDER BY id DESC LIMIT 1",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(event_type, "proposal_rejected");
        Ok(())
    }

    #[test]
    fn threads_reject_retry_after_audit_is_idempotent() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_, proposal_id) = stage_pending_protected_edit(home)?;
        set_proposal_decision_failpoint(Some((
            ProposalDecisionFailpoint::AuditBeforeCleanup,
            proposal_id.clone(),
        )));

        let first = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/reject"),
            home,
            None,
            Some("{}"),
        );

        assert!(first.is_err(), "failpoint must interrupt pending cleanup");
        let claim = find_pending_decision_claim(home, &proposal_id, "reject")
            .expect("committed rejection leaves its claim until recovery");

        let retry = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/reject"),
            home,
            None,
            Some("{}"),
        )?;

        assert_eq!(retry.status, 200, "got {}", retry.body);
        assert!(!claim.exists());
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let rejected_count: i64 = conn.query_row(
            "SELECT COUNT(*)
             FROM ward_audit
             WHERE proposal_id = ?1 AND event_type = 'proposal_rejected'",
            [&proposal_id],
            |row| row.get(0),
        )?;
        assert_eq!(rejected_count, 1);
        Ok(())
    }

    #[test]
    fn threads_decision_preserves_ward_audit_append_only() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let (_pending, proposal_id) = stage_pending_protected_edit(home)?;

        let response = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/reject"),
            home,
            None,
            Some("{}"),
        )?;
        assert_eq!(response.status, 200, "got {}", response.body);

        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let update = conn.execute(
            "UPDATE ward_audit SET decision = 'tampered' WHERE proposal_id = ?1",
            [&proposal_id],
        );
        assert!(update.is_err(), "UPDATE must abort on ward_audit");
        let delete = conn.execute(
            "DELETE FROM ward_audit WHERE proposal_id = ?1",
            [&proposal_id],
        );
        assert!(delete.is_err(), "DELETE must abort on ward_audit");
        Ok(())
    }

    #[test]
    fn threads_decision_corrupt_pending_blocks_and_keeps_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let pending_dir = home.join("pending");
        std::fs::create_dir_all(&pending_dir)?;
        let proposal_id = uuid::Uuid::new_v4().to_string();
        let file = pending_dir.join(format!("{}-{proposal_id}.json", uuid::Uuid::new_v4()));
        std::fs::write(&file, "{not json")?;

        let response = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;

        assert_eq!(response.status, 409, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["blocked"], true);
        assert_eq!(body["why"], "proposal-corrupt");
        assert!(file.exists(), "corrupt proposal must remain for inspection");
        Ok(())
    }

    #[test]
    fn threads_decision_unknown_proposal_fails_closed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let response = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{}/reject", uuid::Uuid::new_v4()),
            home,
            None,
            Some("{}"),
        )?;

        assert_eq!(response.status, 404, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["blocked"], true);
        assert_eq!(body["why"], "proposal-not-found");
        Ok(())
    }

    #[test]
    fn post_familiar_edits_holds_tier1_write() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        let workspace = seed_warded_familiar(home)?;

        let response = post_edits(
            home,
            r#"{"edits":[{"target":"reviewed/skill.md","contents":"tweak"}]}"#,
        )?;

        // Gate 3 G3.1: a pure Tier-1 hold is staged for coherence review
        // instead of dead-ending as a bare hold. Nothing is written.
        assert_eq!(response.status, 202, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["disposition"], "staged");
        assert_eq!(body["reviewKind"], "coherence");
        assert_eq!(
            body["changes"][0]["verdict"]["kind"],
            "requiresCoherenceReview"
        );
        assert!(!workspace.join("reviewed/skill.md").exists());

        // The pending file exists, carries the sidecar marker, and still
        // parses as the core PendingProposal type (marker is additive).
        let pending_path =
            std::path::PathBuf::from(body["pendingPath"].as_str().expect("pendingPath present"));
        let raw = std::fs::read_to_string(&pending_path)?;
        let staged: serde_json::Value = serde_json::from_str(&raw)?;
        assert_eq!(staged["reviewKind"], "coherence");
        let parsed: coven_threads_core::PendingProposal = serde_json::from_str(&raw)?;
        assert_eq!(parsed.id.0.to_string(), body["proposalId"]);
        assert_eq!(parsed.edits.len(), 1);

        // One proposal_submitted row landed in the append-only ledger.
        let conn = store::open_store(&home.join("coven.sqlite3"))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM ward_audit WHERE event_type = 'proposal_submitted' \
             AND decision = 'staged:coherence'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(count, 1);

        // Fail-closed until Gate-3 resolution (PR 4) lands: approving a
        // coherence proposal 409s; nothing is written.
        let proposal_id = body["proposalId"].as_str().expect("proposalId");
        let approve = handle_request_with_body(
            "POST",
            &format!("/api/v1/threads/proposals/{proposal_id}/approve"),
            home,
            None,
            Some("{}"),
        )?;
        assert_eq!(approve.status, 409, "got {}", approve.body);
        assert!(!workspace.join("reviewed/skill.md").exists());
        Ok(())
    }

    #[test]
    fn post_familiar_edits_fails_closed_without_ward_toml() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_familiars_toml(home)?;
        let workspace = home.join("familiars").join("sage");
        std::fs::create_dir_all(&workspace)?;

        let response = post_edits(
            home,
            r#"{"edits":[{"target":"notes/today.md","contents":"hello"}]}"#,
        )?;

        assert_eq!(response.status, 409, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "ward_not_configured");
        assert!(!workspace.join("notes/today.md").exists());
        Ok(())
    }

    #[test]
    fn post_familiar_edits_returns_404_for_unknown_familiar() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_familiars_toml(home)?;

        let response = handle_request_with_body(
            "POST",
            "/api/v1/familiars/ghost/edits",
            home,
            None,
            Some(r#"{"edits":[{"target":"x.md","contents":"y"}]}"#),
        )?;

        assert_eq!(response.status, 404, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "familiar_not_found");
        Ok(())
    }

    #[test]
    fn post_familiar_edits_rejects_missing_edits_field() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let home = temp.path();
        seed_warded_familiar(home)?;

        let response = post_edits(home, r#"{"nope":true}"#)?;

        assert_eq!(response.status, 400, "got {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "invalid_request");
        Ok(())
    }

    #[test]
    fn get_coven_calls_api_route_returns_empty_array_when_no_file() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let response = handle_request("GET", "/api/v1/coven-calls", temp_dir.path(), None)?;

        assert_eq!(response.status, 200);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["ok"], true);
        assert_eq!(body["calls"], serde_json::json!([]));
        Ok(())
    }

    #[test]
    fn get_coven_calls_by_id_returns_404_when_missing() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;

        let response = handle_request(
            "GET",
            "/api/v1/coven-calls/no-such-id",
            temp_dir.path(),
            None,
        )?;

        assert_eq!(response.status, 404);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "call_not_found");
        Ok(())
    }

    #[test]
    fn register_external_session_returns_201_with_external_flag() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let body = json!({
            "id": "engine-sess-abc",
            "projectRoot": temp.path().to_string_lossy(),
            "harness": "engine",
            "title": "Engine TUI session",
            "transcriptPath": "/tmp/engine-sess-abc.jsonl"
        })
        .to_string();

        let response = handle_request_with_body(
            "POST",
            "/api/v1/sessions/external",
            temp.path(),
            None,
            Some(&body),
        )?;

        assert_eq!(response.status, 201, "unexpected body: {}", response.body);
        let record: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(record["id"], "engine-sess-abc");
        assert_eq!(record["status"], "running");
        assert_eq!(record["external"], true);
        assert_eq!(record["transcript_path"], "/tmp/engine-sess-abc.jsonl");

        // Verify idempotency: a second POST with the same id returns 200, not 201.
        let response2 = handle_request_with_body(
            "POST",
            "/api/v1/sessions/external",
            temp.path(),
            None,
            Some(&body),
        )?;
        assert_eq!(
            response2.status, 200,
            "idempotent re-register should return 200"
        );

        Ok(())
    }

    #[test]
    fn register_external_session_empty_title_defaults_to_external_session() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;

        // Empty string title should fall back to "External session".
        let body_empty = json!({
            "id": "engine-sess-empty-title",
            "projectRoot": temp.path().to_string_lossy(),
            "harness": "engine",
            "title": ""
        })
        .to_string();
        let response = handle_request_with_body(
            "POST",
            "/api/v1/sessions/external",
            temp.path(),
            None,
            Some(&body_empty),
        )?;
        assert_eq!(response.status, 201, "unexpected body: {}", response.body);
        let record: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(
            record["title"], "External session",
            "empty title should default to \"External session\""
        );

        // Whitespace-only title should also fall back to "External session".
        let body_ws = json!({
            "id": "engine-sess-ws-title",
            "projectRoot": temp.path().to_string_lossy(),
            "harness": "engine",
            "title": "   "
        })
        .to_string();
        let response_ws = handle_request_with_body(
            "POST",
            "/api/v1/sessions/external",
            temp.path(),
            None,
            Some(&body_ws),
        )?;
        assert_eq!(
            response_ws.status, 201,
            "unexpected body: {}",
            response_ws.body
        );
        let record_ws: serde_json::Value = serde_json::from_str(&response_ws.body)?;
        assert_eq!(
            record_ws["title"], "External session",
            "whitespace-only title should default to \"External session\""
        );

        Ok(())
    }

    #[test]
    fn complete_external_session_with_exit_0_marks_completed() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;

        // Register the session first.
        let reg_body = json!({
            "id": "engine-sess-complete",
            "projectRoot": temp.path().to_string_lossy(),
            "harness": "engine",
            "title": "will complete"
        })
        .to_string();
        handle_request_with_body(
            "POST",
            "/api/v1/sessions/external",
            temp.path(),
            None,
            Some(&reg_body),
        )?;

        // Complete with exitCode 0.
        let complete_body = json!({ "exitCode": 0 }).to_string();
        let response = handle_request_with_body(
            "POST",
            "/api/v1/sessions/engine-sess-complete/complete",
            temp.path(),
            None,
            Some(&complete_body),
        )?;

        assert_eq!(response.status, 200, "body: {}", response.body);
        let record: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(record["status"], "completed");
        Ok(())
    }

    #[test]
    fn complete_external_session_with_nonzero_exit_marks_failed() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;

        let reg_body = json!({
            "id": "engine-sess-fail",
            "projectRoot": temp.path().to_string_lossy(),
            "harness": "engine",
            "title": "will fail"
        })
        .to_string();
        handle_request_with_body(
            "POST",
            "/api/v1/sessions/external",
            temp.path(),
            None,
            Some(&reg_body),
        )?;

        let complete_body = json!({ "exitCode": 1 }).to_string();
        let response = handle_request_with_body(
            "POST",
            "/api/v1/sessions/engine-sess-fail/complete",
            temp.path(),
            None,
            Some(&complete_body),
        )?;

        assert_eq!(response.status, 200, "body: {}", response.body);
        let record: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(record["status"], "failed");
        Ok(())
    }

    #[test]
    fn complete_unknown_session_returns_404() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;

        let response = handle_request_with_body(
            "POST",
            "/api/v1/sessions/no-such-session-id/complete",
            temp.path(),
            None,
            Some(r#"{"exitCode": 0}"#),
        )?;

        assert_eq!(response.status, 404);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "session_not_found");
        Ok(())
    }

    #[test]
    fn kill_external_session_returns_422() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;

        // Register an external session so it's in the store as running+external.
        let reg_body = json!({
            "id": "ext-kill-guard",
            "projectRoot": temp.path().to_string_lossy(),
            "harness": "engine",
            "title": "external session"
        })
        .to_string();
        handle_request_with_body(
            "POST",
            "/api/v1/sessions/external",
            temp.path(),
            None,
            Some(&reg_body),
        )?;

        let response = handle_request_with_runtime(
            "POST",
            "/api/v1/sessions/ext-kill-guard/kill",
            temp.path(),
            None,
            None,
            &NoopSessionRuntime,
        )?;

        assert_eq!(response.status, 422, "body: {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "external_session_not_killable");
        assert_eq!(body["error"]["details"]["sessionId"], "ext-kill-guard");
        Ok(())
    }

    #[test]
    fn complete_non_external_session_returns_422() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;

        // Insert a daemon-managed (non-external) running session.
        insert_test_session(temp.path(), "daemon-sess")?;

        let response = handle_request_with_body(
            "POST",
            "/api/v1/sessions/daemon-sess/complete",
            temp.path(),
            None,
            Some(r#"{"exitCode": 0}"#),
        )?;

        assert_eq!(response.status, 422, "body: {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "not_external_session");
        assert_eq!(body["error"]["details"]["sessionId"], "daemon-sess");
        Ok(())
    }

    #[test]
    fn register_external_session_conflicts_with_daemon_session_returns_409() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;

        // Insert a daemon-managed session with the id we're about to try to register.
        insert_test_session(temp.path(), "shared-id")?;

        let reg_body = json!({
            "id": "shared-id",
            "projectRoot": temp.path().to_string_lossy(),
            "harness": "engine",
            "title": "should conflict"
        })
        .to_string();

        let response = handle_request_with_body(
            "POST",
            "/api/v1/sessions/external",
            temp.path(),
            None,
            Some(&reg_body),
        )?;

        assert_eq!(response.status, 409, "body: {}", response.body);
        let body: serde_json::Value = serde_json::from_str(&response.body)?;
        assert_eq!(body["error"]["code"], "session_id_conflict");
        assert_eq!(body["error"]["details"]["sessionId"], "shared-id");
        Ok(())
    }
}
