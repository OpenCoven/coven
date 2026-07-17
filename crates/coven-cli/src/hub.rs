//! Hub control-plane state for multi-host mode (#266).
//!
//! The hub owns the durable node registry, routing table, global job queue,
//! and per-executor subqueues described in `specs/coven-multi-host-daemon`.
//! All state persists in the Coven SQLite store, so a daemon restart reloads
//! the registry and queues without losing loop/job assignments. Executor
//! nodes that go unavailable keep their subqueue held on the hub until they
//! recover or the scheduler explicitly redispatches their work.

use std::path::Path;

use anyhow::{bail, Context, Result};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    api::{api_error, current_timestamp, json_response, parse_body, ApiResponse},
    executor_node, store,
};

/// Store-meta key shared with travel profile generation so the hub identity
/// reported by `/hub/status` matches the `sourceHub.hubId` embedded in
/// generated travel profiles.
pub const HUB_ID_META_KEY: &str = "travel_source_hub_id";

pub const HUB_ROLE: &str = "hub";

const JOB_STATE_QUEUED: &str = "queued";
const JOB_STATE_ASSIGNED: &str = "assigned";
const JOB_STATE_HELD: &str = "held";
const TERMINAL_JOB_STATES: [&str; 3] = ["completed", "failed", "cancelled"];

fn store_path(coven_home: &Path) -> std::path::PathBuf {
    coven_home.join("coven.sqlite3")
}

fn hub_id(conn: &Connection) -> Result<String> {
    store::get_or_insert_store_meta(conn, HUB_ID_META_KEY, &format!("hub_{}", Uuid::new_v4()))
}

fn parse_capabilities(json_text: &str) -> Vec<String> {
    serde_json::from_str(json_text).unwrap_or_default()
}

fn node_response(node: &store::NodeRecord) -> Value {
    let transport_config: Value = node
        .transport_config_json
        .as_deref()
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or(Value::Null);
    json!({
        "nodeId": node.node_id,
        "role": node.role,
        "transport": node.transport,
        "transportConfig": transport_config,
        "capabilities": parse_capabilities(&node.capabilities_json),
        "available": node.available,
        "queuePressure": node.queue_pressure,
        "lastHealthAt": node.last_health_at,
        "lastError": node.last_error,
        "registeredAt": node.registered_at,
        "updatedAt": node.updated_at,
    })
}

fn job_response(job: &store::HubJobRecord) -> Value {
    json!({
        "jobId": job.job_id,
        "state": job.state,
        "priority": job.priority,
        "requiredCapabilities": parse_capabilities(&job.required_capabilities_json),
        "assignedNodeId": job.assigned_node_id,
        "loopId": job.loop_id,
        "payload": serde_json::from_str::<Value>(&job.payload_json).unwrap_or(Value::Null),
        "createdAt": job.created_at,
        "updatedAt": job.updated_at,
    })
}

fn route_response(route: &store::RouteRecord) -> Value {
    json!({
        "jobId": route.job_id,
        "nodeId": route.node_id,
        "decisionId": route.decision_id,
        "reason": route.reason,
        "createdAt": route.created_at,
        "updatedAt": route.updated_at,
    })
}

/// Rebuild the persistent subqueue for a node from its assigned/held jobs and
/// refresh the node's stored queue pressure. Held jobs stay in the subqueue so
/// an unavailable executor never loses assigned work.
fn sync_executor_queue(conn: &Connection, node_id: &str, now: &str) -> Result<Vec<String>> {
    let job_ids: Vec<String> = store::list_hub_jobs_for_node(conn, node_id)?
        .into_iter()
        .filter(|job| job.state == JOB_STATE_ASSIGNED || job.state == JOB_STATE_HELD)
        .map(|job| job.job_id)
        .collect();
    let job_ids_json =
        serde_json::to_string(&job_ids).context("failed to serialize executor subqueue")?;
    store::upsert_executor_queue(
        conn,
        &store::ExecutorQueueRecord {
            node_id: node_id.to_string(),
            job_ids_json,
            updated_at: now.to_string(),
        },
    )?;
    if let Some(mut node) = store::get_node(conn, node_id)? {
        node.queue_pressure = job_ids.len() as i64;
        node.updated_at = now.to_string();
        store::upsert_node(conn, &node)?;
    }
    Ok(job_ids)
}

/// Bring the hub's persistent job, routing-table, and subqueue state in line
/// with a scheduler redispatch outcome. Returns `false` when the job is not
/// tracked in the hub's global queue (snapshot-only simulation flows), in
/// which case nothing is touched.
pub(crate) fn apply_redispatch_outcome(
    conn: &Connection,
    job_id: &str,
    target_node_id: Option<&str>,
    decision_id: &str,
    reason: &str,
    now: &str,
) -> Result<bool> {
    let Some(job) = store::get_hub_job(conn, job_id)? else {
        return Ok(false);
    };
    if TERMINAL_JOB_STATES.contains(&job.state.as_str()) {
        return Ok(false);
    }
    let previous_node_id = job.assigned_node_id.clone();
    match target_node_id {
        Some(target) => {
            store::update_hub_job_state(conn, job_id, JOB_STATE_ASSIGNED, Some(target), now)?;
            store::upsert_route(
                conn,
                &store::RouteRecord {
                    job_id: job_id.to_string(),
                    node_id: target.to_string(),
                    decision_id: Some(decision_id.to_string()),
                    reason: reason.to_string(),
                    created_at: now.to_string(),
                    updated_at: now.to_string(),
                },
            )?;
            sync_executor_queue(conn, target, now)?;
            if let Some(previous) = previous_node_id.filter(|previous| previous != target) {
                sync_executor_queue(conn, &previous, now)?;
            }
        }
        None => {
            // Paused: hold the job on its current node without losing state.
            store::update_hub_job_state(
                conn,
                job_id,
                JOB_STATE_HELD,
                previous_node_id.as_deref(),
                now,
            )?;
            if let Some(previous) = previous_node_id {
                sync_executor_queue(conn, &previous, now)?;
            }
        }
    }
    Ok(true)
}

pub fn hub_health_summary(coven_home: &Path) -> Result<Value> {
    let conn = store::open_store(&store_path(coven_home))?;
    let nodes = store::list_nodes(&conn)?;
    let available = nodes.iter().filter(|node| node.available).count();
    Ok(json!({
        "role": HUB_ROLE,
        "hubId": hub_id(&conn)?,
        "nodesTotal": nodes.len(),
        "nodesAvailable": available,
    }))
}

pub fn hub_status(coven_home: &Path) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    let nodes = store::list_nodes(&conn)?;
    let jobs = store::list_hub_jobs(&conn, None)?;
    let queues = store::list_executor_queues(&conn)?;
    let count_state = |state: &str| jobs.iter().filter(|job| job.state == state).count();
    let node_views: Vec<Value> = nodes.iter().map(node_response).collect();
    let queue_views: Vec<Value> = queues
        .iter()
        .map(|queue| {
            let job_ids: Vec<String> = parse_capabilities(&queue.job_ids_json);
            json!({
                "nodeId": queue.node_id,
                "jobIds": job_ids,
                "updatedAt": queue.updated_at,
            })
        })
        .collect();
    json_response(
        200,
        &json!({
            "role": HUB_ROLE,
            "hubId": hub_id(&conn)?,
            "nodes": node_views,
            "nodesTotal": nodes.len(),
            "nodesAvailable": nodes.iter().filter(|node| node.available).count(),
            "globalQueue": {
                "queued": count_state(JOB_STATE_QUEUED),
                "assigned": count_state(JOB_STATE_ASSIGNED),
                "held": count_state(JOB_STATE_HELD),
                "total": jobs.len(),
            },
            "executorQueues": queue_views,
        }),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterNodeRequest {
    node_id: String,
    role: String,
    #[serde(default)]
    transport: Option<String>,
    /// Structured hub-outbound dispatch config. Required before the hub can
    /// poll or dispatch to this node over SSH/private network.
    #[serde(default)]
    transport_config: Option<executor_node::TransportConfig>,
    #[serde(default)]
    capabilities: Vec<String>,
    #[serde(default = "default_true")]
    available: bool,
}

fn default_true() -> bool {
    true
}

pub fn register_node(coven_home: &Path, body: Option<&str>) -> Result<ApiResponse> {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let request: RegisterNodeRequest = match serde_json::from_value(payload) {
        Ok(request) => request,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    if request.node_id.trim().is_empty() {
        return api_error(400, "invalid_request", "nodeId is required.", None);
    }
    if request.role.trim().is_empty() {
        return api_error(400, "invalid_request", "role is required.", None);
    }
    let conn = store::open_store(&store_path(coven_home))?;
    let now = current_timestamp();
    let existing = store::get_node(&conn, &request.node_id)?;
    let capabilities_json = serde_json::to_string(&request.capabilities)
        .context("failed to serialize node capabilities")?;
    let transport_config_json = match &request.transport_config {
        Some(config) => {
            if let Err(error) = validate_registered_transport_config(config) {
                return api_error(
                    400,
                    "invalid_request",
                    &format!("transportConfig is invalid: {error}"),
                    None,
                );
            }
            Some(serde_json::to_string(config).context("failed to serialize transport config")?)
        }
        // Preserve a previously registered dispatch config when the
        // re-registration omits it — but never re-activate a stored `local`
        // (or unparseable) config through the API: that would let a client
        // keep an internal-only daemon-side program dispatchable.
        None => match existing
            .as_ref()
            .and_then(|node| node.transport_config_json.clone())
        {
            Some(stored) => {
                if !stored_transport_config_is_ssh(&stored) {
                    return api_error(
                        400,
                        "invalid_request",
                        "this node's stored transport config cannot be preserved through the \
                         daemon API; supply a valid SSH transportConfig",
                        None,
                    );
                }
                Some(stored)
            }
            None => None,
        },
    };
    let record = store::NodeRecord {
        node_id: request.node_id.clone(),
        role: request.role,
        transport: request
            .transport
            .filter(|transport| !transport.trim().is_empty())
            .unwrap_or_else(|| "ssh".to_string()),
        transport_config_json,
        capabilities_json,
        available: request.available,
        queue_pressure: existing
            .as_ref()
            .map(|node| node.queue_pressure)
            .unwrap_or(0),
        last_health_at: now.clone(),
        last_error: None,
        registered_at: existing
            .as_ref()
            .map(|node| node.registered_at.clone())
            .unwrap_or_else(|| now.clone()),
        updated_at: now,
    };
    store::upsert_node(&conn, &record)?;
    json_response(
        if existing.is_some() { 200 } else { 201 },
        &node_response(&record),
    )
}

fn validate_registered_transport_config(config: &executor_node::TransportConfig) -> Result<()> {
    match config {
        executor_node::TransportConfig::Ssh { .. } => {
            executor_node::build_transport(config).map(drop)
        }
        executor_node::TransportConfig::Local { .. } => {
            bail!("local transport configs cannot be registered through the daemon API")
        }
    }
}

/// Whether a stored transport config JSON parses as an SSH transport — the
/// only kind an API re-registration may carry forward.
fn stored_transport_config_is_ssh(stored: &str) -> bool {
    matches!(
        serde_json::from_str::<executor_node::TransportConfig>(stored),
        Ok(executor_node::TransportConfig::Ssh { .. })
    )
}

pub fn list_nodes(coven_home: &Path) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    let nodes: Vec<Value> = store::list_nodes(&conn)?
        .iter()
        .map(node_response)
        .collect();
    json_response(200, &json!({ "nodes": nodes }))
}

pub fn get_node(coven_home: &Path, node_id: &str) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    match store::get_node(&conn, node_id)? {
        Some(node) => json_response(200, &node_response(&node)),
        None => api_error(
            404,
            "node_not_found",
            "Node was not found in the hub registry.",
            Some(json!({ "nodeId": node_id })),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NodeHealthRequest {
    available: bool,
    #[serde(default)]
    capabilities: Option<Vec<String>>,
}

/// Record an executor health report. Availability transitions move the node's
/// assigned jobs between `assigned` and `held` without ever dropping them from
/// the executor subqueue, so an offline node holds its work durably.
pub fn report_node_health(
    coven_home: &Path,
    node_id: &str,
    body: Option<&str>,
) -> Result<ApiResponse> {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let request: NodeHealthRequest = match serde_json::from_value(payload) {
        Ok(request) => request,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let conn = store::open_store(&store_path(coven_home))?;
    let Some(mut node) = store::get_node(&conn, node_id)? else {
        return api_error(
            404,
            "node_not_found",
            "Node was not found in the hub registry.",
            Some(json!({ "nodeId": node_id })),
        );
    };
    let now = current_timestamp();
    node.available = request.available;
    node.last_health_at = now.clone();
    node.updated_at = now.clone();
    if let Some(capabilities) = request.capabilities {
        node.capabilities_json = serde_json::to_string(&capabilities)
            .context("failed to serialize node capabilities")?;
    }
    store::upsert_node(&conn, &node)?;

    let (from_state, to_state, transitioned) =
        transition_jobs_for_availability(&conn, node_id, request.available, &now)?;
    let held_job_ids = sync_executor_queue(&conn, node_id, &now)?;
    let node =
        store::get_node(&conn, node_id)?.context("node disappeared while reporting health")?;
    json_response(
        200,
        &json!({
            "node": node_response(&node),
            "heldSubqueue": {
                "nodeId": node_id,
                "jobIds": held_job_ids,
            },
            "transitionedJobs": {
                "from": from_state,
                "to": to_state,
                "jobIds": transitioned,
            },
        }),
    )
}

/// Move a node's jobs between `assigned` and `held` when its availability
/// changes. Jobs are never dropped: an unavailable node's work is held on
/// the hub until the node recovers or the scheduler redispatches it.
fn transition_jobs_for_availability(
    conn: &rusqlite::Connection,
    node_id: &str,
    available: bool,
    now: &str,
) -> Result<(&'static str, &'static str, Vec<String>)> {
    let (from_state, to_state) = if available {
        (JOB_STATE_HELD, JOB_STATE_ASSIGNED)
    } else {
        (JOB_STATE_ASSIGNED, JOB_STATE_HELD)
    };
    let mut transitioned = Vec::new();
    for job in store::list_hub_jobs_for_node(conn, node_id)? {
        if job.state == from_state {
            store::update_hub_job_state(conn, &job.job_id, to_state, Some(node_id), now)?;
            transitioned.push(job.job_id);
        }
    }
    Ok((from_state, to_state, transitioned))
}

/// Hub-initiated availability poll (#267): connect outbound to the executor
/// over its registered transport, run `coven executor probe`, and record the
/// advertised role/capabilities plus last-known availability. Executors never
/// push health to the hub — this poll (and dispatch) is how the registry
/// learns about them. Poll failures are recorded, never fatal.
pub fn poll_node(coven_home: &Path, node_id: &str) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    let Some(mut node) = store::get_node(&conn, node_id)? else {
        return api_error(
            404,
            "node_not_found",
            "Node was not found in the hub registry.",
            Some(json!({ "nodeId": node_id })),
        );
    };
    let transport = match build_node_transport(&node) {
        Ok(transport) => transport,
        Err(response) => return response,
    };
    let now = current_timestamp();
    node.last_health_at = now.clone();
    node.updated_at = now.clone();

    let poll_result = executor_node::poll_executor(transport.as_ref()).and_then(|probe| {
        if probe.role != node.role {
            anyhow::bail!(
                "executor advertised role {} but is registered as {}",
                probe.role,
                node.role
            );
        }
        Ok(probe)
    });
    match poll_result {
        Ok(probe) => {
            node.capabilities_json = serde_json::to_string(&probe.capabilities)
                .context("failed to serialize probed capabilities")?;
            node.available = probe.available;
            node.last_error = None;
            store::upsert_node(&conn, &node)?;
            transition_jobs_for_availability(&conn, node_id, probe.available, &now)?;
            let held_job_ids = sync_executor_queue(&conn, node_id, &now)?;
            let node = store::get_node(&conn, node_id)?.context("node disappeared during poll")?;
            json_response(
                200,
                &json!({
                    "nodeId": node.node_id,
                    "ok": true,
                    "probe": probe,
                    "heldSubqueue": {
                        "nodeId": node_id,
                        "jobIds": held_job_ids,
                    },
                    "node": node_response(&node),
                }),
            )
        }
        Err(error) => {
            node.available = false;
            node.last_error = Some(format!("{error:#}"));
            store::upsert_node(&conn, &node)?;
            transition_jobs_for_availability(&conn, node_id, false, &now)?;
            let held_job_ids = sync_executor_queue(&conn, node_id, &now)?;
            let node = store::get_node(&conn, node_id)?.context("node disappeared during poll")?;
            json_response(
                200,
                &json!({
                    "nodeId": node.node_id,
                    "ok": false,
                    "error": format!("{error:#}"),
                    "heldSubqueue": {
                        "nodeId": node_id,
                        "jobIds": held_job_ids,
                    },
                    "node": node_response(&node),
                }),
            )
        }
    }
}

fn build_node_transport(
    node: &store::NodeRecord,
) -> std::result::Result<Box<dyn executor_node::ExecutorTransport>, Result<ApiResponse>> {
    let Some(raw) = node.transport_config_json.as_deref() else {
        return Err(api_error(
            409,
            "node_transport_not_configured",
            "Node has no dispatch transport configured; re-register it with a transportConfig.",
            Some(json!({ "nodeId": node.node_id })),
        ));
    };
    let config: executor_node::TransportConfig = match serde_json::from_str(raw) {
        Ok(config) => config,
        Err(error) => {
            return Err(api_error(
                500,
                "node_transport_invalid",
                &format!("Stored transport config could not be parsed: {error}"),
                Some(json!({ "nodeId": node.node_id })),
            ));
        }
    };
    match executor_node::build_transport(&config) {
        Ok(transport) => Ok(transport),
        Err(error) => Err(api_error(
            500,
            "node_transport_invalid",
            &format!("Stored transport config could not be used: {error}"),
            Some(json!({ "nodeId": node.node_id })),
        )),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DispatchJobRequest {
    #[serde(default)]
    job_id: Option<String>,
    command: Vec<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    stdin: Option<String>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
    #[serde(default)]
    required_capabilities: Vec<String>,
    #[serde(default)]
    context: Option<Value>,
}

/// Hub-outbound job dispatch (#267). The job spec sent to the executor
/// carries the full context (argv, cwd, env, stdin, opaque context blob) so
/// the stateless node needs no local durable authority; the executor's
/// normalized result envelope is persisted with the dispatch record. When
/// `jobId` names a queued hub job, its state is advanced from the envelope.
pub fn dispatch_to_node(
    coven_home: &Path,
    node_id: &str,
    body: Option<&str>,
) -> Result<ApiResponse> {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let request: DispatchJobRequest = match serde_json::from_value(payload) {
        Ok(request) => request,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    if request.command.is_empty() {
        return api_error(400, "invalid_request", "command must not be empty.", None);
    }
    let conn = store::open_store(&store_path(coven_home))?;
    let Some(mut node) = store::get_node(&conn, node_id)? else {
        return api_error(
            404,
            "node_not_found",
            "Node was not found in the hub registry.",
            Some(json!({ "nodeId": node_id })),
        );
    };
    let known_capabilities = parse_capabilities(&node.capabilities_json);
    let missing: Vec<&String> = request
        .required_capabilities
        .iter()
        .filter(|required| !known_capabilities.contains(required))
        .collect();
    if !missing.is_empty() {
        return api_error(
            409,
            "executor_capability_mismatch",
            "Executor node does not advertise the required capabilities.",
            Some(json!({
                "nodeId": node_id,
                "requiredCapabilities": request.required_capabilities,
                "knownCapabilities": known_capabilities,
                "missingCapabilities": missing,
            })),
        );
    }
    let transport = match build_node_transport(&node) {
        Ok(transport) => transport,
        Err(response) => return response,
    };
    let job_id = request
        .job_id
        .filter(|job_id| !job_id.trim().is_empty())
        .unwrap_or_else(|| format!("job_{}", Uuid::new_v4()));
    let job = executor_node::ExecutorJob {
        protocol_version: executor_node::EXECUTOR_PROTOCOL_VERSION.to_string(),
        job_id: job_id.clone(),
        hub_id: Some(hub_id(&conn)?),
        required_capabilities: request.required_capabilities,
        command: request.command,
        cwd: request.cwd,
        env: request.env,
        stdin: request.stdin,
        timeout_seconds: request.timeout_seconds,
        context: request.context,
    };
    let job_json = serde_json::to_string(&job).context("failed to serialize job spec")?;
    let now = current_timestamp();
    // Persist before dispatching so a hub crash mid-dispatch leaves evidence.
    store::upsert_executor_dispatch(
        &conn,
        &store::ExecutorDispatchRecord {
            job_id: job_id.clone(),
            node_id: node_id.to_string(),
            status: "dispatched".to_string(),
            job_json: job_json.clone(),
            envelope_json: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )?;

    let envelope = executor_node::dispatch_job(transport.as_ref(), &job);
    let envelope_json =
        serde_json::to_string(&envelope).context("failed to serialize result envelope")?;
    let finished = current_timestamp();
    store::upsert_executor_dispatch(
        &conn,
        &store::ExecutorDispatchRecord {
            job_id: job_id.clone(),
            node_id: node_id.to_string(),
            status: envelope.status.clone(),
            job_json,
            envelope_json: Some(envelope_json),
            created_at: now.clone(),
            updated_at: finished.clone(),
        },
    )?;

    // Advance a matching hub-queue job from the envelope. A transport error
    // leaves the job assigned/held so no work is lost.
    if store::get_hub_job(&conn, &job_id)?.is_some() {
        let hub_state = match envelope.status.as_str() {
            executor_node::RESULT_STATUS_COMPLETED => Some("completed"),
            executor_node::RESULT_STATUS_FAILED
            | executor_node::RESULT_STATUS_TIMEOUT
            | executor_node::RESULT_STATUS_REJECTED => Some("failed"),
            _ => None,
        };
        if let Some(state) = hub_state {
            store::update_hub_job_state(&conn, &job_id, state, Some(node_id), &finished)?;
        }
    }

    // A dispatch doubles as an availability observation.
    let unreachable = envelope.status == executor_node::RESULT_STATUS_TRANSPORT_ERROR;
    node.available = !unreachable;
    node.last_health_at = finished.clone();
    node.last_error = if unreachable {
        envelope.error.clone()
    } else {
        None
    };
    node.updated_at = finished.clone();
    store::upsert_node(&conn, &node)?;
    transition_jobs_for_availability(&conn, node_id, !unreachable, &finished)?;
    sync_executor_queue(&conn, node_id, &finished)?;

    if unreachable {
        return api_error(
            502,
            "executor_unreachable",
            "Executor node could not be reached or returned an invalid envelope.",
            Some(json!({
                "nodeId": node_id,
                "jobId": job_id,
                "envelope": envelope,
            })),
        );
    }
    json_response(
        200,
        &json!({
            "jobId": job_id,
            "nodeId": node_id,
            "createdAt": now,
            "envelope": envelope,
        }),
    )
}

pub fn get_dispatch(coven_home: &Path, job_id: &str) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    let Some(record) = store::get_executor_dispatch(&conn, job_id)? else {
        return api_error(
            404,
            "executor_job_not_found",
            "Executor job dispatch record was not found.",
            Some(json!({ "jobId": job_id })),
        );
    };
    let job: Value = serde_json::from_str(&record.job_json).context("failed to parse job spec")?;
    let envelope: Value = match &record.envelope_json {
        Some(envelope_json) => {
            serde_json::from_str(envelope_json).context("failed to parse result envelope")?
        }
        None => Value::Null,
    };
    json_response(
        200,
        &json!({
            "jobId": record.job_id,
            "nodeId": record.node_id,
            "status": record.status,
            "job": job,
            "envelope": envelope,
            "createdAt": record.created_at,
            "updatedAt": record.updated_at,
        }),
    )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnqueueJobRequest {
    #[serde(default)]
    job_id: Option<String>,
    #[serde(default)]
    required_capabilities: Vec<String>,
    #[serde(default)]
    priority: i64,
    #[serde(default)]
    loop_id: Option<String>,
    #[serde(default)]
    payload: Option<Value>,
}

pub fn enqueue_job(coven_home: &Path, body: Option<&str>) -> Result<ApiResponse> {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let request: EnqueueJobRequest = match serde_json::from_value(payload) {
        Ok(request) => request,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let job_id = request
        .job_id
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| format!("job_{}", Uuid::new_v4()));
    let conn = store::open_store(&store_path(coven_home))?;
    if store::get_hub_job(&conn, &job_id)?.is_some() {
        return api_error(
            409,
            "job_already_queued",
            "A job with this id already exists in the global queue.",
            Some(json!({ "jobId": job_id })),
        );
    }
    let now = current_timestamp();
    let record = store::HubJobRecord {
        job_id,
        state: JOB_STATE_QUEUED.to_string(),
        priority: request.priority,
        required_capabilities_json: serde_json::to_string(&request.required_capabilities)
            .context("failed to serialize required capabilities")?,
        assigned_node_id: None,
        loop_id: request.loop_id,
        payload_json: request.payload.unwrap_or(Value::Null).to_string(),
        created_at: now.clone(),
        updated_at: now,
    };
    store::upsert_hub_job(&conn, &record)?;
    json_response(201, &job_response(&record))
}

pub fn list_jobs(coven_home: &Path, query: &str) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    let state = crate::api::query_param(query, "state");
    let jobs: Vec<Value> = store::list_hub_jobs(&conn, state)?
        .iter()
        .map(job_response)
        .collect();
    json_response(200, &json!({ "jobs": jobs }))
}

pub fn get_job(coven_home: &Path, job_id: &str) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    match store::get_hub_job(&conn, job_id)? {
        Some(job) => {
            let route = store::get_route(&conn, job_id)?;
            let mut body = job_response(&job);
            body["route"] = route.as_ref().map(route_response).unwrap_or(Value::Null);
            json_response(200, &body)
        }
        None => api_error(
            404,
            "job_not_found",
            "Job was not found in the hub queue.",
            Some(json!({ "jobId": job_id })),
        ),
    }
}

fn hub_role_rank(role: &str) -> i32 {
    match role {
        "compute_executor" => 0,
        "stationary_executor" => 1,
        "hub" => 2,
        "laptop_local" => 3,
        _ => 4,
    }
}

fn node_supports(node: &store::NodeRecord, required: &[String]) -> bool {
    let capabilities = parse_capabilities(&node.capabilities_json);
    required
        .iter()
        .all(|capability| capabilities.iter().any(|owned| owned == capability))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssignJobRequest {
    #[serde(default)]
    node_id: Option<String>,
}

/// Assign a queued (or held) job to an executor from the persistent node
/// registry, recording the routing decision in the routing table and the
/// scheduler decision log.
pub fn assign_job(coven_home: &Path, job_id: &str, body: Option<&str>) -> Result<ApiResponse> {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let request: AssignJobRequest = match serde_json::from_value(payload) {
        Ok(request) => request,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let conn = store::open_store(&store_path(coven_home))?;
    let Some(job) = store::get_hub_job(&conn, job_id)? else {
        return api_error(
            404,
            "job_not_found",
            "Job was not found in the hub queue.",
            Some(json!({ "jobId": job_id })),
        );
    };
    if TERMINAL_JOB_STATES.contains(&job.state.as_str()) {
        return api_error(
            409,
            "job_not_assignable",
            "Job has already reached a terminal state.",
            Some(json!({ "jobId": job_id, "state": job.state })),
        );
    }
    let required = parse_capabilities(&job.required_capabilities_json);
    let nodes = store::list_nodes(&conn)?;
    let target = match request.node_id {
        Some(node_id) => {
            let Some(node) = nodes.iter().find(|node| node.node_id == node_id) else {
                return api_error(
                    404,
                    "node_not_found",
                    "Requested node was not found in the hub registry.",
                    Some(json!({ "nodeId": node_id })),
                );
            };
            if !node.available {
                return api_error(
                    409,
                    "node_unavailable",
                    "Requested node is not available.",
                    Some(json!({ "nodeId": node_id })),
                );
            }
            if !node_supports(node, &required) {
                return api_error(
                    409,
                    "node_missing_capabilities",
                    "Requested node does not satisfy the job's required capabilities.",
                    Some(json!({ "nodeId": node_id, "requiredCapabilities": required })),
                );
            }
            node.clone()
        }
        None => {
            let mut candidates: Vec<&store::NodeRecord> = nodes
                .iter()
                .filter(|node| node.available)
                .filter(|node| node_supports(node, &required))
                .collect();
            candidates.sort_by(|left, right| {
                left.queue_pressure
                    .cmp(&right.queue_pressure)
                    .then_with(|| hub_role_rank(&left.role).cmp(&hub_role_rank(&right.role)))
                    .then_with(|| left.node_id.cmp(&right.node_id))
            });
            match candidates.first() {
                Some(node) => (*node).clone(),
                None => {
                    return api_error(
                        409,
                        "no_available_node",
                        "No available registered node satisfies the job's required capabilities.",
                        Some(json!({
                            "jobId": job_id,
                            "requiredCapabilities": required,
                        })),
                    );
                }
            }
        }
    };

    let now = current_timestamp();
    let previous_node_id = job.assigned_node_id.clone();
    let decision_id = format!("sched_{}", Uuid::new_v4());
    let reason = format!(
        "{} selected from hub registry by capability match and queue pressure",
        target.node_id
    );
    store::insert_scheduler_decision(
        &conn,
        &store::SchedulerDecisionRecord {
            id: decision_id.clone(),
            job_id: job_id.to_string(),
            target_role: target.role.clone(),
            target_node_id: Some(target.node_id.clone()),
            target_json: json!({ "role": target.role, "nodeId": target.node_id }).to_string(),
            reason: reason.clone(),
            inputs_json: json!({
                "requiredCapabilities": required,
                "queuePressure": target.queue_pressure,
                "source": "hub_registry",
            })
            .to_string(),
            created_at: now.clone(),
        },
    )?;
    store::update_hub_job_state(
        &conn,
        job_id,
        JOB_STATE_ASSIGNED,
        Some(&target.node_id),
        &now,
    )?;
    store::upsert_route(
        &conn,
        &store::RouteRecord {
            job_id: job_id.to_string(),
            node_id: target.node_id.clone(),
            decision_id: Some(decision_id.clone()),
            reason: reason.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )?;
    sync_executor_queue(&conn, &target.node_id, &now)?;
    if let Some(previous) = previous_node_id.filter(|previous| previous != &target.node_id) {
        sync_executor_queue(&conn, &previous, &now)?;
    }
    let job = store::get_hub_job(&conn, job_id)?.context("job disappeared during assignment")?;
    let mut body = job_response(&job);
    body["decisionId"] = json!(decision_id);
    body["reason"] = json!(reason);
    json_response(200, &body)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompleteJobRequest {
    #[serde(default)]
    state: Option<String>,
}

pub fn complete_job(coven_home: &Path, job_id: &str, body: Option<&str>) -> Result<ApiResponse> {
    let payload = match parse_body(body) {
        Ok(payload) => payload,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let request: CompleteJobRequest = match serde_json::from_value(payload) {
        Ok(request) => request,
        Err(error) => return api_error(400, "invalid_request", &error.to_string(), None),
    };
    let state = request.state.unwrap_or_else(|| "completed".to_string());
    if !TERMINAL_JOB_STATES.contains(&state.as_str()) {
        return api_error(
            400,
            "invalid_request",
            "state must be `completed`, `failed`, or `cancelled`.",
            Some(json!({ "state": state })),
        );
    }
    let conn = store::open_store(&store_path(coven_home))?;
    let Some(job) = store::get_hub_job(&conn, job_id)? else {
        return api_error(
            404,
            "job_not_found",
            "Job was not found in the hub queue.",
            Some(json!({ "jobId": job_id })),
        );
    };
    let now = current_timestamp();
    let assigned_node_id = job.assigned_node_id.clone();
    store::update_hub_job_state(&conn, job_id, &state, assigned_node_id.as_deref(), &now)?;
    if let Some(node_id) = assigned_node_id {
        sync_executor_queue(&conn, &node_id, &now)?;
    }
    let job = store::get_hub_job(&conn, job_id)?.context("job disappeared during completion")?;
    json_response(200, &job_response(&job))
}

pub fn list_routing_table(coven_home: &Path) -> Result<ApiResponse> {
    let conn = store::open_store(&store_path(coven_home))?;
    let routes: Vec<Value> = store::list_routes(&conn)?
        .iter()
        .map(route_response)
        .collect();
    json_response(200, &json!({ "routes": routes }))
}

#[cfg(test)]
mod tests {
    use crate::api::{handle_request, handle_request_with_body};

    fn post(
        temp: &tempfile::TempDir,
        path: &str,
        body: &str,
    ) -> anyhow::Result<(u16, serde_json::Value)> {
        let response = handle_request_with_body("POST", path, temp.path(), None, Some(body))?;
        Ok((response.status, serde_json::from_str(&response.body)?))
    }

    fn get(temp: &tempfile::TempDir, path: &str) -> anyhow::Result<(u16, serde_json::Value)> {
        let response = handle_request("GET", path, temp.path(), None)?;
        Ok((response.status, serde_json::from_str(&response.body)?))
    }

    fn register_gpu_node(temp: &tempfile::TempDir, node_id: &str) -> anyhow::Result<()> {
        let (status, _) = post(
            temp,
            "/api/v1/hub/nodes",
            &format!(
                r#"{{"nodeId":"{node_id}","role":"compute_executor","transport":"ssh","capabilities":["gpu","long-running-loop"]}}"#
            ),
        )?;
        assert_eq!(status, 201);
        Ok(())
    }

    #[test]
    fn hub_status_exposes_role_and_node_availability() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        register_gpu_node(&temp, "node_a")?;
        register_gpu_node(&temp, "node_b")?;
        let (status, body) = post(
            &temp,
            "/api/v1/hub/nodes/node_b/health",
            r#"{"available":false}"#,
        )?;
        assert_eq!(status, 200);
        assert_eq!(body["node"]["available"], false);

        let (status, body) = get(&temp, "/api/v1/hub/status")?;
        assert_eq!(status, 200);
        assert_eq!(body["role"], "hub");
        assert!(body["hubId"].as_str().unwrap().starts_with("hub_"));
        assert_eq!(body["nodesTotal"], 2);
        assert_eq!(body["nodesAvailable"], 1);
        let nodes = body["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);

        // The generic health endpoint also exposes the hub role and
        // node-availability summary.
        let (status, health) = get(&temp, "/api/v1/health")?;
        assert_eq!(status, 200);
        assert_eq!(health["capabilities"]["hub"], true);
        assert_eq!(health["hub"]["role"], "hub");
        assert_eq!(health["hub"]["nodesTotal"], 2);
        assert_eq!(health["hub"]["nodesAvailable"], 1);
        Ok(())
    }

    #[test]
    fn global_queue_routes_job_to_capable_node_and_records_routing_table() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        register_gpu_node(&temp, "node_gpu")?;
        let (status, _) = post(
            &temp,
            "/api/v1/hub/nodes",
            r#"{"nodeId":"node_cpu","role":"stationary_executor","capabilities":["shell"]}"#,
        )?;
        assert_eq!(status, 201);

        let (status, job) = post(
            &temp,
            "/api/v1/hub/jobs",
            r#"{"jobId":"job_1","requiredCapabilities":["gpu"],"priority":5,"loopId":"loop_1"}"#,
        )?;
        assert_eq!(status, 201);
        assert_eq!(job["state"], "queued");

        let (status, assigned) = post(&temp, "/api/v1/hub/jobs/job_1/assign", "{}")?;
        assert_eq!(status, 200);
        assert_eq!(assigned["state"], "assigned");
        assert_eq!(assigned["assignedNodeId"], "node_gpu");
        assert!(assigned["decisionId"]
            .as_str()
            .unwrap()
            .starts_with("sched_"));

        let (status, routing) = get(&temp, "/api/v1/hub/routing")?;
        assert_eq!(status, 200);
        let routes = routing["routes"].as_array().unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0]["jobId"], "job_1");
        assert_eq!(routes[0]["nodeId"], "node_gpu");

        let (status, job) = get(&temp, "/api/v1/hub/jobs/job_1")?;
        assert_eq!(status, 200);
        assert_eq!(job["route"]["nodeId"], "node_gpu");

        // Node queue pressure reflects the persistent subqueue.
        let (_, node) = get(&temp, "/api/v1/hub/nodes/node_gpu")?;
        assert_eq!(node["queuePressure"], 1);
        Ok(())
    }

    #[test]
    fn assign_rejects_when_no_registered_node_matches_capabilities() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        register_gpu_node(&temp, "node_gpu")?;
        let (_, _) = post(
            &temp,
            "/api/v1/hub/jobs",
            r#"{"jobId":"job_arm","requiredCapabilities":["arm64"]}"#,
        )?;
        let (status, body) = post(&temp, "/api/v1/hub/jobs/job_arm/assign", "{}")?;
        assert_eq!(status, 409);
        assert_eq!(body["error"]["code"], "no_available_node");
        Ok(())
    }

    #[test]
    fn duplicate_job_ids_are_rejected() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let (status, _) = post(&temp, "/api/v1/hub/jobs", r#"{"jobId":"job_dup"}"#)?;
        assert_eq!(status, 201);
        let (status, body) = post(&temp, "/api/v1/hub/jobs", r#"{"jobId":"job_dup"}"#)?;
        assert_eq!(status, 409);
        assert_eq!(body["error"]["code"], "job_already_queued");
        Ok(())
    }

    #[test]
    fn unavailable_executor_holds_subqueue_without_losing_job_state() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        register_gpu_node(&temp, "node_gpu")?;
        post(
            &temp,
            "/api/v1/hub/jobs",
            r#"{"jobId":"job_loop","requiredCapabilities":["gpu"],"loopId":"loop_9"}"#,
        )?;
        let (status, _) = post(&temp, "/api/v1/hub/jobs/job_loop/assign", "{}")?;
        assert_eq!(status, 200);

        // Executor disappears: assigned work transitions to held, and the
        // per-executor subqueue keeps the job.
        let (status, body) = post(
            &temp,
            "/api/v1/hub/nodes/node_gpu/health",
            r#"{"available":false}"#,
        )?;
        assert_eq!(status, 200);
        assert_eq!(body["transitionedJobs"]["to"], "held");
        assert_eq!(body["heldSubqueue"]["jobIds"][0], "job_loop");

        let (_, job) = get(&temp, "/api/v1/hub/jobs/job_loop")?;
        assert_eq!(job["state"], "held");
        assert_eq!(job["assignedNodeId"], "node_gpu");
        assert_eq!(job["loopId"], "loop_9");

        let (_, hub) = get(&temp, "/api/v1/hub/status")?;
        assert_eq!(hub["globalQueue"]["held"], 1);
        assert_eq!(hub["executorQueues"][0]["jobIds"][0], "job_loop");

        // Executor recovers: held work resumes without loss.
        let (status, body) = post(
            &temp,
            "/api/v1/hub/nodes/node_gpu/health",
            r#"{"available":true}"#,
        )?;
        assert_eq!(status, 200);
        assert_eq!(body["transitionedJobs"]["to"], "assigned");
        let (_, job) = get(&temp, "/api/v1/hub/jobs/job_loop")?;
        assert_eq!(job["state"], "assigned");
        Ok(())
    }

    #[test]
    fn hub_state_persists_to_disk_and_reloads_after_restart() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        register_gpu_node(&temp, "node_gpu")?;
        post(
            &temp,
            "/api/v1/hub/jobs",
            r#"{"jobId":"job_persist","requiredCapabilities":["gpu"]}"#,
        )?;
        post(&temp, "/api/v1/hub/jobs/job_persist/assign", "{}")?;
        let (_, before) = get(&temp, "/api/v1/hub/status")?;

        // Simulate a daemon restart: reopen the store from disk on a fresh
        // connection and confirm the registry, queue, and routing reload.
        let conn = crate::store::open_store(&temp.path().join("coven.sqlite3"))?;
        let nodes = crate::store::list_nodes(&conn)?;
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id, "node_gpu");
        let jobs = crate::store::list_hub_jobs(&conn, Some("assigned"))?;
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].job_id, "job_persist");
        let routes = crate::store::list_routes(&conn)?;
        assert_eq!(routes.len(), 1);
        drop(conn);

        let (_, after) = get(&temp, "/api/v1/hub/status")?;
        assert_eq!(before["hubId"], after["hubId"]);
        assert_eq!(after["globalQueue"]["assigned"], 1);
        assert_eq!(after["executorQueues"][0]["jobIds"][0], "job_persist");
        Ok(())
    }

    #[test]
    fn completed_jobs_leave_the_executor_subqueue() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        register_gpu_node(&temp, "node_gpu")?;
        post(
            &temp,
            "/api/v1/hub/jobs",
            r#"{"jobId":"job_done","requiredCapabilities":["gpu"]}"#,
        )?;
        post(&temp, "/api/v1/hub/jobs/job_done/assign", "{}")?;
        let (status, job) = post(
            &temp,
            "/api/v1/hub/jobs/job_done/complete",
            r#"{"state":"completed"}"#,
        )?;
        assert_eq!(status, 200);
        assert_eq!(job["state"], "completed");
        let (_, node) = get(&temp, "/api/v1/hub/nodes/node_gpu")?;
        assert_eq!(node["queuePressure"], 0);
        let (status, body) = post(&temp, "/api/v1/hub/jobs/job_done/assign", "{}")?;
        assert_eq!(status, 409);
        assert_eq!(body["error"]["code"], "job_not_assignable");
        Ok(())
    }

    fn register_dispatchable_node(
        temp: &tempfile::TempDir,
        node_id: &str,
        role: &str,
        program: &str,
        capabilities: &str,
    ) -> anyhow::Result<(u16, serde_json::Value)> {
        let conn = crate::store::open_store(&super::store_path(temp.path()))?;
        let now = super::current_timestamp();
        let transport_config = serde_json::json!({
            "kind": "local",
            "program": program,
        });
        let record = crate::store::NodeRecord {
            node_id: node_id.to_string(),
            role: role.to_string(),
            transport: "local".to_string(),
            transport_config_json: Some(serde_json::to_string(&transport_config)?),
            capabilities_json: capabilities.to_string(),
            available: true,
            queue_pressure: 0,
            last_health_at: now.clone(),
            last_error: None,
            registered_at: now.clone(),
            updated_at: now,
        };
        crate::store::upsert_node(&conn, &record)?;
        // Release the write connection before the API read opens its own.
        drop(conn);
        get(temp, &format!("/api/v1/hub/nodes/{node_id}"))
    }

    #[test]
    fn node_registration_validates_and_exposes_the_dispatch_transport() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;

        let (status, node) = post(
            &temp,
            "/api/v1/hub/nodes",
            r#"{
                "nodeId":"node_stationary",
                "role":"stationary_executor",
                "transportConfig":{"kind":"ssh","host":"executor.internal"}
            }"#,
        )?;
        assert_eq!(status, 201);
        assert_eq!(node["transportConfig"]["kind"], "ssh");
        assert_eq!(node["lastError"], serde_json::Value::Null);

        // Local process transports are an internal/test seam only. Accepting
        // them over the daemon API would let API clients choose an arbitrary
        // daemon-side program for poll/dispatch to execute.
        let (status, body) = post(
            &temp,
            "/api/v1/hub/nodes",
            r#"{
                "nodeId":"node_local",
                "role":"stationary_executor",
                "transportConfig":{"kind":"local","program":"/bin/sh","args":["-c","id"]}
            }"#,
        )?;
        assert_eq!(status, 400);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("local transport configs cannot be registered"));

        // An SSH config shaped like an option injection must be rejected.
        let (status, body) = post(
            &temp,
            "/api/v1/hub/nodes",
            r#"{
                "nodeId":"node_evil",
                "role":"stationary_executor",
                "transportConfig":{"kind":"ssh","host":"-oProxyCommand=evil"}
            }"#,
        )?;
        assert_eq!(status, 400);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("transportConfig is invalid"));

        // Re-registering without a transportConfig preserves the stored one.
        let (status, node) = post(
            &temp,
            "/api/v1/hub/nodes",
            r#"{"nodeId":"node_stationary","role":"stationary_executor","capabilities":["shell","browser"]}"#,
        )?;
        assert_eq!(status, 200);
        assert_eq!(node["transportConfig"]["kind"], "ssh");
        Ok(())
    }

    #[test]
    fn reregistration_cannot_preserve_a_stored_local_transport() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        // Seed a node with an internal-only local transport (test seam /
        // legacy data path — not reachable through the API).
        let (status, node) = register_dispatchable_node(
            &temp,
            "node_seeded",
            "stationary_executor",
            "/usr/local/bin/coven",
            r#"["shell"]"#,
        )?;
        assert_eq!(status, 200);
        assert_eq!(node["transportConfig"]["kind"], "local");

        // An API re-registration without transportConfig must not carry the
        // stored local transport forward.
        let (status, body) = post(
            &temp,
            "/api/v1/hub/nodes",
            r#"{"nodeId":"node_seeded","role":"stationary_executor","capabilities":["shell"]}"#,
        )?;
        assert_eq!(status, 400);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("cannot be preserved through the daemon API"));

        // Supplying a valid SSH transportConfig replaces the local one.
        let (status, node) = post(
            &temp,
            "/api/v1/hub/nodes",
            r#"{
                "nodeId":"node_seeded",
                "role":"stationary_executor",
                "transportConfig":{"kind":"ssh","host":"executor.internal"}
            }"#,
        )?;
        assert_eq!(status, 200);
        assert_eq!(node["transportConfig"]["kind"], "ssh");
        Ok(())
    }

    #[test]
    fn poll_requires_a_configured_dispatch_transport() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        register_gpu_node(&temp, "node_gpu")?;

        let (status, body) = post(&temp, "/api/v1/hub/nodes/node_gpu/poll", "{}")?;

        assert_eq!(status, 409);
        assert_eq!(body["error"]["code"], "node_transport_not_configured");
        Ok(())
    }

    #[test]
    fn poll_records_last_known_unavailability_when_executor_is_unreachable() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        register_dispatchable_node(
            &temp,
            "node_gone",
            "stationary_executor",
            "/definitely/not/a/real/coven-executor-binary",
            r#"["shell"]"#,
        )?;

        let (status, body) = post(&temp, "/api/v1/hub/nodes/node_gone/poll", "{}")?;

        assert_eq!(status, 200);
        assert_eq!(body["ok"], false);
        assert_eq!(body["node"]["available"], false);
        assert!(!body["node"]["lastError"].as_str().unwrap().is_empty());
        assert!(body["node"]["lastHealthAt"].as_str().unwrap().contains('T'));
        Ok(())
    }

    #[test]
    fn poll_and_dispatch_return_404_for_unregistered_nodes() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;

        let (status, body) = post(&temp, "/api/v1/hub/nodes/node_missing/poll", "{}")?;
        assert_eq!(status, 404);
        assert_eq!(body["error"]["code"], "node_not_found");

        let (status, body) = post(
            &temp,
            "/api/v1/hub/nodes/node_missing/dispatch",
            r#"{"command":["echo","hi"]}"#,
        )?;
        assert_eq!(status, 404);
        assert_eq!(body["error"]["code"], "node_not_found");

        let (status, body) = get(&temp, "/api/v1/hub/dispatches/job_missing")?;
        assert_eq!(status, 404);
        assert_eq!(body["error"]["code"], "executor_job_not_found");
        Ok(())
    }

    #[test]
    fn dispatch_rejects_jobs_whose_capabilities_the_node_does_not_advertise() -> anyhow::Result<()>
    {
        let temp = tempfile::tempdir()?;
        register_dispatchable_node(
            &temp,
            "node_stationary",
            "stationary_executor",
            "/usr/local/bin/coven",
            r#"["shell"]"#,
        )?;

        let (status, body) = post(
            &temp,
            "/api/v1/hub/nodes/node_stationary/dispatch",
            r#"{"command":["train"],"requiredCapabilities":["gpu"]}"#,
        )?;

        assert_eq!(status, 409);
        assert_eq!(body["error"]["code"], "executor_capability_mismatch");
        assert_eq!(body["error"]["details"]["missingCapabilities"][0], "gpu");
        Ok(())
    }

    #[test]
    fn dispatch_marks_node_unreachable_on_transport_failure_and_keeps_the_record(
    ) -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        register_dispatchable_node(
            &temp,
            "node_gone",
            "compute_executor",
            "/definitely/not/a/real/coven-executor-binary",
            r#"["shell"]"#,
        )?;

        let (status, body) = post(
            &temp,
            "/api/v1/hub/nodes/node_gone/dispatch",
            r#"{"jobId":"job_lost","command":["echo","hi"]}"#,
        )?;
        assert_eq!(status, 502);
        assert_eq!(body["error"]["code"], "executor_unreachable");
        assert_eq!(
            body["error"]["details"]["envelope"]["status"],
            "transport_error"
        );

        // The dispatch record persists even when the transport failed.
        let (status, job) = get(&temp, "/api/v1/hub/dispatches/job_lost")?;
        assert_eq!(status, 200);
        assert_eq!(job["status"], "transport_error");
        assert_eq!(job["nodeId"], "node_gone");

        let (_, node) = get(&temp, "/api/v1/hub/nodes/node_gone")?;
        assert_eq!(node["available"], false);
        assert!(!node["lastError"].as_str().unwrap().is_empty());
        Ok(())
    }

    #[cfg(unix)]
    fn write_fake_executor_script(dir: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
        use std::os::unix::fs::PermissionsExt;

        let script = dir.join("fake-executor");
        std::fs::write(
            &script,
            r#"#!/bin/sh
if [ "$1" = "executor" ] && [ "$2" = "probe" ]; then
  printf '{"protocolVersion":"coven.executor.v1","role":"compute_executor","capabilities":["shell","gpu"],"available":true,"queuePressure":1,"covenVersion":"0.0.0","probedAt":"2026-07-06T00:00:00Z"}\n'
  exit 0
fi
if [ "$1" = "executor" ] && [ "$2" = "run-job" ]; then
  job=$(cat)
  id=$(printf '%s' "$job" | sed -n 's/.*"jobId":"\([^"]*\)".*/\1/p')
  printf '{"protocolVersion":"coven.executor.v1","jobId":"%s","status":"completed","exitCode":0,"stdout":"remote job output","stderr":"","startedAt":"2026-07-06T00:00:00Z","finishedAt":"2026-07-06T00:00:01Z","durationMs":1000,"error":null}\n' "$id"
  exit 0
fi
exit 2
"#,
        )?;
        let mut permissions = std::fs::metadata(&script)?.permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions)?;
        Ok(script)
    }

    #[cfg(unix)]
    #[test]
    fn hub_polls_and_dispatches_outbound_and_records_executor_state() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let script = write_fake_executor_script(temp.path())?;
        register_dispatchable_node(
            &temp,
            "node_compute",
            "compute_executor",
            &script.to_string_lossy(),
            r#"["shell"]"#,
        )?;

        // Hub-initiated poll refreshes capability metadata and availability.
        let (status, poll) = post(&temp, "/api/v1/hub/nodes/node_compute/poll", "{}")?;
        assert_eq!(status, 200);
        assert_eq!(poll["ok"], true);
        assert_eq!(poll["probe"]["role"], "compute_executor");
        assert_eq!(poll["node"]["available"], true);
        assert_eq!(
            poll["node"]["capabilities"],
            serde_json::json!(["shell", "gpu"])
        );
        assert_eq!(poll["node"]["lastError"], serde_json::Value::Null);

        // Outbound dispatch returns the executor's normalized envelope and
        // persists the dispatch record with the full-context job spec.
        let (status, dispatch) = post(
            &temp,
            "/api/v1/hub/nodes/node_compute/dispatch",
            r#"{
                "command":["echo","hello"],
                "requiredCapabilities":["gpu"],
                "context":{"workspaceId":"workspace-1"}
            }"#,
        )?;
        assert_eq!(status, 200);
        let job_id = dispatch["jobId"].as_str().unwrap().to_string();
        assert!(job_id.starts_with("job_"));
        assert_eq!(dispatch["envelope"]["status"], "completed");
        assert_eq!(dispatch["envelope"]["jobId"], job_id.as_str());
        assert_eq!(dispatch["envelope"]["exitCode"], 0);
        assert_eq!(dispatch["envelope"]["stdout"], "remote job output");

        let (status, job) = get(&temp, &format!("/api/v1/hub/dispatches/{job_id}"))?;
        assert_eq!(status, 200);
        assert_eq!(job["status"], "completed");
        assert_eq!(job["job"]["protocolVersion"], "coven.executor.v1");
        assert!(job["job"]["hubId"].as_str().unwrap().starts_with("hub_"));
        assert_eq!(job["job"]["context"]["workspaceId"], "workspace-1");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn dispatch_advances_a_matching_hub_queue_job_from_the_envelope() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let script = write_fake_executor_script(temp.path())?;
        register_dispatchable_node(
            &temp,
            "node_compute",
            "compute_executor",
            &script.to_string_lossy(),
            r#"["gpu","long-running-loop"]"#,
        )?;
        post(
            &temp,
            "/api/v1/hub/jobs",
            r#"{"jobId":"job_queue_1","requiredCapabilities":["gpu"]}"#,
        )?;
        post(&temp, "/api/v1/hub/jobs/job_queue_1/assign", "{}")?;

        let (status, dispatch) = post(
            &temp,
            "/api/v1/hub/nodes/node_compute/dispatch",
            r#"{"jobId":"job_queue_1","command":["echo","work"],"requiredCapabilities":["gpu"]}"#,
        )?;
        assert_eq!(status, 200);
        assert_eq!(dispatch["envelope"]["status"], "completed");

        // The hub-queue job advanced to completed and left the subqueue.
        let (_, job) = get(&temp, "/api/v1/hub/jobs/job_queue_1")?;
        assert_eq!(job["state"], "completed");
        let (_, node) = get(&temp, "/api/v1/hub/nodes/node_compute")?;
        assert_eq!(node["queuePressure"], 0);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn poll_fails_closed_when_executor_advertises_a_mismatched_role() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let script = write_fake_executor_script(temp.path())?;
        // Register the fake compute executor as stationary.
        register_dispatchable_node(
            &temp,
            "node_mislabeled",
            "stationary_executor",
            &script.to_string_lossy(),
            r#"["shell"]"#,
        )?;

        let (status, poll) = post(&temp, "/api/v1/hub/nodes/node_mislabeled/poll", "{}")?;

        assert_eq!(status, 200);
        assert_eq!(poll["ok"], false);
        assert_eq!(poll["node"]["available"], false);
        assert!(poll["node"]["lastError"]
            .as_str()
            .unwrap()
            .contains("advertised role"));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn poll_going_unavailable_holds_the_executor_subqueue() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let script = write_fake_executor_script(temp.path())?;
        register_dispatchable_node(
            &temp,
            "node_compute",
            "compute_executor",
            &script.to_string_lossy(),
            r#"["gpu","long-running-loop"]"#,
        )?;
        post(
            &temp,
            "/api/v1/hub/jobs",
            r#"{"jobId":"job_held_by_poll","requiredCapabilities":["gpu"]}"#,
        )?;
        post(&temp, "/api/v1/hub/jobs/job_held_by_poll/assign", "{}")?;

        // Break the transport, then poll: the node goes unavailable and its
        // assigned job is held on the hub without being dropped.
        std::fs::remove_file(&script)?;
        let (status, poll) = post(&temp, "/api/v1/hub/nodes/node_compute/poll", "{}")?;
        assert_eq!(status, 200);
        assert_eq!(poll["ok"], false);
        assert_eq!(poll["heldSubqueue"]["jobIds"][0], "job_held_by_poll");
        let (_, job) = get(&temp, "/api/v1/hub/jobs/job_held_by_poll")?;
        assert_eq!(job["state"], "held");
        Ok(())
    }
    #[test]
    fn scheduler_decision_falls_back_to_hub_registry_when_nodes_omitted() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;

        // Empty registry and no snapshot: fail closed.
        let (status, body) = post(
            &temp,
            "/api/v1/scheduler/decisions",
            r#"{"jobId":"job_reg","requiredCapabilities":["gpu"]}"#,
        )?;
        assert_eq!(status, 409);
        assert_eq!(body["error"]["code"], "no_scheduler_target");

        register_gpu_node(&temp, "node_gpu")?;
        let (status, body) = post(
            &temp,
            "/api/v1/scheduler/decisions",
            r#"{"jobId":"job_reg","requiredCapabilities":["gpu"]}"#,
        )?;
        assert_eq!(status, 201);
        assert_eq!(body["target"]["nodeId"], "node_gpu");
        assert_eq!(body["inputs"]["nodesSource"], "hub_registry");
        Ok(())
    }

    #[test]
    fn redispatch_uses_hub_registry_and_syncs_hub_job_state() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        register_gpu_node(&temp, "node_primary")?;
        post(
            &temp,
            "/api/v1/hub/jobs",
            r#"{"jobId":"job_move","requiredCapabilities":["gpu"],"loopId":"loop_move"}"#,
        )?;
        let (status, _) = post(&temp, "/api/v1/hub/jobs/job_move/assign", "{}")?;
        assert_eq!(status, 200);

        // Primary goes offline (job held), fallback joins the registry.
        post(
            &temp,
            "/api/v1/hub/nodes/node_primary/health",
            r#"{"available":false}"#,
        )?;
        register_gpu_node(&temp, "node_fallback")?;

        // No `nodes` snapshot: candidates come from the persistent registry.
        let (status, body) = post(
            &temp,
            "/api/v1/scheduler/redispatch",
            r#"{
                "loopId":"loop_move",
                "jobId":"job_move",
                "currentNodeId":"node_primary",
                "requiredCapabilities":["gpu"],
                "loopResumable":true
            }"#,
        )?;
        assert_eq!(status, 202);
        assert_eq!(body["state"], "redispatched");
        assert_eq!(body["target"]["nodeId"], "node_fallback");
        assert_eq!(body["hubJobSynced"], true);
        assert_eq!(body["inputs"].get("nodesSource"), None); // inputs not echoed here

        // The hub's persistent job, routing, and subqueue state follow the
        // redispatch decision.
        let (_, job) = get(&temp, "/api/v1/hub/jobs/job_move")?;
        assert_eq!(job["state"], "assigned");
        assert_eq!(job["assignedNodeId"], "node_fallback");
        assert_eq!(job["route"]["nodeId"], "node_fallback");
        let (_, hub) = get(&temp, "/api/v1/hub/status")?;
        let queues = hub["executorQueues"].as_array().unwrap();
        let queue_for = |node: &str| {
            queues
                .iter()
                .find(|queue| queue["nodeId"] == node)
                .map(|queue| queue["jobIds"].clone())
        };
        assert_eq!(queue_for("node_fallback").unwrap()[0], "job_move");
        assert_eq!(
            queue_for("node_primary").unwrap().as_array().unwrap().len(),
            0
        );
        Ok(())
    }

    #[test]
    fn redispatch_pause_holds_hub_job_without_losing_state() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        register_gpu_node(&temp, "node_primary")?;
        post(
            &temp,
            "/api/v1/hub/jobs",
            r#"{"jobId":"job_stuck","requiredCapabilities":["gpu"],"loopId":"loop_stuck"}"#,
        )?;
        post(&temp, "/api/v1/hub/jobs/job_stuck/assign", "{}")?;
        post(
            &temp,
            "/api/v1/hub/nodes/node_primary/health",
            r#"{"available":false}"#,
        )?;

        // No alternate registry node: the loop pauses, and the hub job is
        // held on its node instead of being dropped.
        let (status, body) = post(
            &temp,
            "/api/v1/scheduler/redispatch",
            r#"{
                "loopId":"loop_stuck",
                "jobId":"job_stuck",
                "currentNodeId":"node_primary",
                "requiredCapabilities":["gpu"],
                "loopResumable":true
            }"#,
        )?;
        assert_eq!(status, 202);
        assert_eq!(body["state"], "paused");
        assert_eq!(body["hubJobSynced"], true);
        assert_eq!(body["preservedSubqueue"]["jobIds"][0], "job_stuck");

        let (_, job) = get(&temp, "/api/v1/hub/jobs/job_stuck")?;
        assert_eq!(job["state"], "held");
        assert_eq!(job["assignedNodeId"], "node_primary");
        Ok(())
    }

    #[test]
    fn redispatch_with_snapshot_nodes_reports_untracked_jobs_as_unsynced() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        // Snapshot-only simulation flow (#270 fixtures): job is not in the
        // hub global queue, so no hub state is touched.
        let (status, body) = post(
            &temp,
            "/api/v1/scheduler/redispatch",
            r#"{
                "loopId":"loop_sim",
                "jobId":"job_sim",
                "currentNodeId":"node_primary",
                "requiredCapabilities":["gpu"],
                "loopResumable":true,
                "nodes":[
                    {"nodeId":"node_primary","role":"compute_executor","available":false,
                     "capabilities":["gpu"],"queuePressure":3,"queuedJobIds":["job_sim"]},
                    {"nodeId":"node_fallback","role":"compute_executor","available":true,
                     "capabilities":["gpu"],"queuePressure":1,"queuedJobIds":[]}
                ]
            }"#,
        )?;
        assert_eq!(status, 202);
        assert_eq!(body["state"], "redispatched");
        assert_eq!(body["target"]["nodeId"], "node_fallback");
        assert_eq!(body["hubJobSynced"], false);
        let (_, jobs) = get(&temp, "/api/v1/hub/jobs")?;
        assert_eq!(jobs["jobs"].as_array().unwrap().len(), 0);
        Ok(())
    }
}
