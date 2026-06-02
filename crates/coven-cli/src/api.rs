use std::{borrow::Cow, path::Path};

use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    control_plane,
    daemon::DaemonStatus,
    encrypted_artifacts::SensitiveArtifactStore,
    harness::{ConversationHint, HarnessLaunchMode},
    privacy, project, store,
};

const MAX_EVENTS_LIMIT: i64 = 1_000;
pub const COVEN_API_VERSION: &str = "v1";
pub const COVEN_API_NAMED_VERSION: &str = "coven.daemon.v1";
pub const COVEN_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SUPPORTED_API_VERSIONS: [&str; 1] = [COVEN_API_VERSION];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthCapabilities {
    pub sessions: bool,
    pub events: bool,
    pub event_cursor: String,
    pub structured_errors: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub ok: bool,
    pub api_version: String,
    pub coven_version: String,
    pub capabilities: HealthCapabilities,
    pub daemon: Option<DaemonStatus>,
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
            event_cursor: "sequence".to_string(),
            structured_errors: true,
        },
        daemon,
    }
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
        ("GET", "/health") => json_response(200, &health_response(daemon)),
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
        ("GET", "/skills") => json_response(200, &crate::cockpit_sources::scan_skills(coven_home)?),
        ("GET", p) if p == "/capabilities" || p.starts_with("/capabilities?") => {
            let refresh = query.map(|q| q.contains("refresh=1")).unwrap_or(false);
            let resp = crate::capabilities::get_all(coven_home, refresh);
            json_response(200, &resp)
        }
        ("GET", p) if p.starts_with("/capabilities/") => {
            let harness_id = p.trim_start_matches("/capabilities/");
            let refresh = query.map(|q| q.contains("refresh=1")).unwrap_or(false);
            match crate::capabilities::get_one(coven_home, harness_id, refresh) {
                Some(m) => json_response(200, &m),
                None => json_response(404, &serde_json::json!({"error": "unknown harness"})),
            }
        }
        ("GET", "/memory") => json_response(200, &crate::cockpit_sources::scan_memory(coven_home)?),
        ("GET", "/research") => {
            json_response(200, &crate::cockpit_sources::read_research(coven_home)?)
        }
        ("GET", "/sessions") => {
            let conn = store::open_store(&store_path(coven_home))?;
            let sessions = store::list_sessions(&conn)?;
            json_response(200, &sessions)
        }
        ("POST", "/sessions") => launch_session(coven_home, body, runtime),
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
    let launch = match session_launch_from_payload(payload) {
        Ok(launch) => launch,
        Err(error) => {
            return api_error(400, "invalid_request", &error.to_string(), None);
        }
    };
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
        labels: Vec::new(),
        visibility: "private".to_string(),
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
    json_response(201, &record)
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
    let supported: Vec<&'static str> = crate::harness::built_in_harness_specs()
        .into_iter()
        .map(|spec| spec.id)
        .collect();
    if !supported.iter().any(|id| *id == harness) {
        anyhow::bail!(
            "harness `{harness}` is not a supported harness; expected one of {supported:?}"
        );
    }
    let launch_mode = launch_mode_from_payload(&payload)?;
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

    Ok(SessionLaunch {
        id: Uuid::new_v4().to_string(),
        project_root: canonical_project_root.to_string_lossy().into_owned(),
        cwd: canonical_cwd.to_string_lossy().into_owned(),
        harness,
        launch_mode,
        prompt,
        title,
        conversation,
        conversation_id,
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
    let open_sessions = sessions
        .iter()
        .filter(|s| s.status == "running" || s.status == "active")
        .count() as u32;
    json_response(
        200,
        &OverviewDto {
            active_familiars: 0,
            total_familiars: 0,
            open_sessions,
            skills_count: 0,
            average_skill_score: 0,
            research_iterations: 0,
            last_research_delta: 0,
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
        labels: Vec::new(),
        visibility: "private".to_string(),
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

fn parse_body(body: Option<&str>) -> Result<Value> {
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

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
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

fn api_error(
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

fn json_response<T: Serialize>(status: u16, body: &T) -> Result<ApiResponse> {
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
        assert_eq!(response.capabilities.event_cursor, "sequence");
        assert!(response.capabilities.structured_errors);
        assert_eq!(response.daemon, None);
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
            "origin": "open-meow",
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
        assert!(response.body.contains(r#""origin":"open-meow""#));
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
            Some(r#"{"origin":"open-meow"}"#),
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
            "origin": "open-meow"
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
            labels: Vec::new(),
            visibility: "private".to_string(),
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
            runtime.launches.borrow()[0].launch_mode,
            HarnessLaunchMode::Interactive
        );
        assert_eq!(runtime.launches.borrow()[0].prompt, "hello coven");
        assert_eq!(
            runtime.launches.borrow()[0].project_root,
            project_root.canonicalize()?.to_string_lossy()
        );
        assert_eq!(
            runtime.launches.borrow()[0].cwd,
            cwd.canonicalize()?.to_string_lossy()
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
            response.body.contains("not a supported harness"),
            "expected supported-harness validation message, got: {}",
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
            labels: Vec::new(),
            visibility: "private".to_string(),
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
            labels: Vec::new(),
            visibility: "private".to_string(),
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
            labels: Vec::new(),
            visibility: "private".to_string(),
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
            labels: Vec::new(),
            visibility: "private".to_string(),
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
            labels: Vec::new(),
            visibility: "private".to_string(),
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
            labels: Vec::new(),
            visibility: "private".to_string(),
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
                    labels: Vec::new(),
                    visibility: "private".to_string(),
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
}
