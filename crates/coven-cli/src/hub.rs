//! Hub control-plane state for multi-host mode (#266).
//!
//! The hub owns the durable node registry, routing table, global job queue,
//! and per-executor subqueues described in `specs/coven-multi-host-daemon`.
//! All state persists in the Coven SQLite store, so a daemon restart reloads
//! the registry and queues without losing loop/job assignments. Executor
//! nodes that go unavailable keep their subqueue held on the hub until they
//! recover or the scheduler explicitly redispatches their work.

use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    api::{api_error, current_timestamp, json_response, parse_body, ApiResponse},
    store,
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
    json!({
        "nodeId": node.node_id,
        "role": node.role,
        "transport": node.transport,
        "capabilities": parse_capabilities(&node.capabilities_json),
        "available": node.available,
        "queuePressure": node.queue_pressure,
        "lastHealthAt": node.last_health_at,
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
    let record = store::NodeRecord {
        node_id: request.node_id.clone(),
        role: request.role,
        transport: request
            .transport
            .filter(|transport| !transport.trim().is_empty())
            .unwrap_or_else(|| "ssh".to_string()),
        capabilities_json,
        available: request.available,
        queue_pressure: existing
            .as_ref()
            .map(|node| node.queue_pressure)
            .unwrap_or(0),
        last_health_at: now.clone(),
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

    let (from_state, to_state) = if request.available {
        (JOB_STATE_HELD, JOB_STATE_ASSIGNED)
    } else {
        (JOB_STATE_ASSIGNED, JOB_STATE_HELD)
    };
    let mut transitioned = Vec::new();
    for job in store::list_hub_jobs_for_node(&conn, node_id)? {
        if job.state == from_state {
            store::update_hub_job_state(&conn, &job.job_id, to_state, Some(node_id), &now)?;
            transitioned.push(job.job_id);
        }
    }
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
}
