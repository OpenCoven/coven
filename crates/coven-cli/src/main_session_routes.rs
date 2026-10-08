//! `/api/v1/main-session` routes (coven#1177, epic #1183).
//!
//! Home — the single persistent chat on mobile and desktop — talks to the
//! daemon through these four routes and never carries a conversation id
//! itself. The daemon owns the init-versus-resume decision:
//!
//! - `GET /main-session` reads the pointer and the row currently hosting it.
//! - `POST /main-session/turn` delivers a prompt. If the bound session is
//!   live it is piped in as input; otherwise a session is launched with
//!   `conversation: resume` when the conversation has history and `init`
//!   when it has none, and the pointer is bound to the new row.
//! - `POST /main-session/reset` is the user's clean slate: rotate the
//!   conversation id, kill and archive the old row, count it, and apply any
//!   replacement harness, familiar, or project root it carries.
//! - `POST /main-session/rollover` is recovery from a conversation id the
//!   harness no longer recognises: same rotation, archive, not counted, and
//!   only if the pointer still holds the id that failed (compare-and-swap),
//!   so a late duplicate never rotates away a fresh conversation.
//!
//! Turn, reset, and rollover each hold a per-scope [`ScopeGate`] for their
//! whole read → launch/rotate → bind sequence, so two requests for the same
//! Home never interleave: no duplicate launch of one conversation, and no
//! reset slipping between a launch and its bind. If the pointer still moves
//! under a launch (a writer outside the gate), the turn stops the session it
//! launched rather than leave it running with nothing pointing at it.
//!
//! The handlers compose the existing `launch_session` and `record_input`
//! handlers rather than re-implementing a launch, so every gate those routes
//! enforce (maintenance writer, harness validation, authority, context
//! admission) applies to Home unchanged.
//!
//! Startup observation is installed before spawn. The route binds the
//! harness-native identity, or rotates a rejected resume and retries the
//! same prompt once as init. Startup parsing keeps stdout and stderr separate
//! so assistant text cannot impersonate a Codex status banner.

use std::{fs, path::Path};

use anyhow::{Context, Result};
use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    api::{current_timestamp, store_path, SessionRuntime},
    api_response::{api_error, json_response, ApiResponse},
    harness::harness_supports_stream_mode,
    main_session::{
        self, MainSessionRecord, MainSessionSettings, RotatedMainSession, StaleIdRotation,
        DEFAULT_MAIN_SESSION_FAMILIAR_ID, DEFAULT_MAIN_SESSION_HARNESS, INSTANCE_MAIN_SCOPE_KEY,
    },
    request_authority::RequestAuthority,
    store,
};

const EVENT_KIND_RESET: &str = "main_session.reset";
const EVENT_KIND_ROLLOVER: &str = "main_session.rollover";
const DEFAULT_ROLLOVER_REASON: &str = "stale-conversation";
const HOME_TITLE: &str = "Home";
const SCOPE_LOCK_DIR: &str = "main-session-locks";

/// Exclusive per-scope gate for every main-session mutation. It is an OS
/// file lock, so it excludes other daemon worker threads (each acquisition
/// opens its own descriptor) and any other process on the same COVEN_HOME.
/// Lock files stay on disk and drop only unlocks, for the reason
/// `AdoptionGate` gives: unlinking would split waiters across inodes. The
/// descriptor is close-on-exec, so a harness launched while the gate is held
/// never inherits it.
pub(crate) struct ScopeGate {
    file: fs::File,
}

impl Drop for ScopeGate {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl ScopeGate {
    pub(crate) fn acquire(coven_home: &Path, scope_key: &str) -> Result<Self> {
        crate::daemon::ensure_windows_supervised_or_private_coven_home(coven_home)?;
        let directory = coven_home.join(SCOPE_LOCK_DIR);
        fs::create_dir_all(&directory).with_context(|| {
            format!(
                "failed to create main-session lock directory {}",
                directory.display()
            )
        })?;
        let digest = Sha256::digest(scope_key.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let path = directory.join(format!("{digest}.lock"));
        let file = crate::state_lock::open_lock_file(&path)?;
        file.lock_exclusive()
            .with_context(|| format!("failed to acquire main-session lock {}", path.display()))?;
        Ok(Self { file })
    }
}

fn parse_optional_body(body: Option<&str>) -> Result<Value, anyhow::Error> {
    match body.map(str::trim).filter(|body| !body.is_empty()) {
        None => Ok(json!({})),
        Some(raw) => {
            let value: Value = serde_json::from_str(raw).context("request body must be JSON")?;
            anyhow::ensure!(value.is_object(), "request body must be a JSON object");
            Ok(value)
        }
    }
}

fn optional_string<'a>(payload: &'a Value, key: &str) -> Option<&'a str> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

/// Like `optional_string`, but a present value that is not a non-empty
/// string is an error rather than silently ignored: a reset that was asked
/// to change a setting must not quietly keep the old one.
fn optional_setting<'a>(payload: &'a Value, key: &str) -> Result<Option<&'a str>, anyhow::Error> {
    match payload.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => optional_string(payload, key)
            .map(Some)
            .with_context(|| format!("`{key}` must be a non-empty string")),
    }
}

fn unsupported_main_harness(harness: &str) -> Result<ApiResponse> {
    api_error(
        400,
        "unsupported_main_session_harness",
        "Main-session continuity currently supports Claude and Codex.",
        Some(json!({"harness":harness})),
    )
}

fn canonical_root(value: Option<&str>) -> Result<Option<String>> {
    value
        .map(|root| {
            let path = crate::project::canonical_project_root(Path::new(root))
                .context("projectRoot must name an existing project directory")?;
            anyhow::ensure!(
                path.is_dir(),
                "projectRoot must name an existing project directory"
            );
            Ok(path.to_string_lossy().into_owned())
        })
        .transpose()
}

/// File references only: clients stage uploads inside the project before a turn.
/// Canonicalization rejects traversal and symlinks escaping the project root.
fn prompt_with_attachments(prompt: &str, payload: &Value, root: Option<&str>) -> Result<String> {
    let Some(value) = payload.get("attachments").filter(|value| !value.is_null()) else {
        return Ok(prompt.to_owned());
    };
    let attachments = value
        .as_array()
        .context("attachments must be an array of file paths")?;
    anyhow::ensure!(
        attachments.len() <= 16,
        "attachments accepts at most 16 file paths"
    );
    if attachments.is_empty() {
        return Ok(prompt.to_owned());
    }
    let root = Path::new(root.context("attachments require a projectRoot")?)
        .canonicalize()
        .context("attachment projectRoot is unavailable")?;
    let mut paths = Vec::new();
    for value in attachments {
        let path = value
            .as_str()
            .filter(|path| !path.trim().is_empty())
            .context("each attachment must be a non-empty file path")?;
        let path = root
            .join(path)
            .canonicalize()
            .context("attachment file is unavailable")?;
        anyhow::ensure!(
            path.starts_with(&root) && path.is_file(),
            "each attachment must be a file inside projectRoot"
        );
        let path = path
            .to_str()
            .context("attachment path must be valid UTF-8")?;
        paths.push(path.to_owned());
    }
    Ok(format!(
        "{prompt}\n\nAttached project files (JSON paths; read these files as context):\n{}",
        serde_json::to_string(&paths)?
    ))
}

fn scope_from(payload: &Value, query: &str) -> Result<String, anyhow::Error> {
    let scope = optional_string(payload, "scope")
        .map(ToOwned::to_owned)
        .or_else(|| crate::api::query_param(query, "scope").map(|value| value.to_string()))
        .unwrap_or_else(|| INSTANCE_MAIN_SCOPE_KEY.to_string());
    main_session::validate_scope_key(&scope)?;
    Ok(scope)
}

fn invalid_request(error: anyhow::Error) -> Result<ApiResponse> {
    api_error(400, "invalid_request", &format!("{error:#}"), None)
}

fn not_found(scope: &str) -> Result<ApiResponse> {
    api_error(
        404,
        "main_session_not_found",
        "No main session exists for this scope.",
        Some(json!({ "scopeKey": scope })),
    )
}

fn current_session(
    conn: &rusqlite::Connection,
    record: &MainSessionRecord,
) -> Result<Option<store::SessionRecord>> {
    match record.current_session_id.as_deref() {
        Some(id) => store::get_session(conn, id),
        None => Ok(None),
    }
}

fn has_history(conn: &rusqlite::Connection, record: &MainSessionRecord) -> Result<bool> {
    Ok(store::get_latest_session_by_conversation_id(conn, &record.conversation_id)?.is_some())
}

pub(crate) fn get_main_session(coven_home: &Path, query: Option<&str>) -> Result<ApiResponse> {
    let scope = match scope_from(&json!({}), query.unwrap_or("")) {
        Ok(scope) => scope,
        Err(error) => return invalid_request(error),
    };
    let conn = store::open_store(&store_path(coven_home))?;
    let Some(record) = main_session::get_main_session(&conn, &scope)? else {
        return not_found(&scope);
    };
    let session = current_session(&conn, &record)?;
    let history = has_history(&conn, &record)?;
    json_response(
        200,
        &json!({
            "mainSession": record,
            "currentSession": session,
            "hasHistory": history,
        }),
    )
}

pub(crate) fn turn(
    coven_home: &Path,
    body: Option<&str>,
    runtime: &dyn SessionRuntime,
    authority: RequestAuthority,
) -> Result<ApiResponse> {
    let payload = match parse_optional_body(body) {
        Ok(payload) => payload,
        Err(error) => return invalid_request(error),
    };
    let scope = match scope_from(&payload, "") {
        Ok(scope) => scope,
        Err(error) => return invalid_request(error),
    };
    let Some(prompt) = optional_string(&payload, "prompt") else {
        return api_error(
            400,
            "invalid_request",
            "turn requires a non-empty string field `prompt`",
            None,
        );
    };
    let requested_harness = match optional_setting(&payload, "harness") {
        Ok(harness) => harness,
        Err(error) => return invalid_request(error),
    };
    if let Some(harness) = requested_harness {
        if !crate::main_session_start::supports_harness(harness) {
            return unsupported_main_harness(harness);
        }
    }
    let requested_familiar = optional_string(&payload, "familiarId");
    let requested_root = match optional_setting(&payload, "projectRoot").and_then(canonical_root) {
        Ok(root) => root,
        Err(error) => return invalid_request(error),
    };
    let requested_root = requested_root.as_deref();
    let model = optional_string(&payload, "model");

    let _scope_gate = ScopeGate::acquire(coven_home, &scope)?;
    let mut conn = store::open_store(&store_path(coven_home))?;
    let existing = main_session::get_main_session(&conn, &scope)?;
    // Validate before pointer creation or input delivery. Existing settings own
    // the scope; a request cannot redirect attachment resolution to another root.
    let attachment_root = existing
        .as_ref()
        .and_then(|record| record.project_root.as_deref())
        .or(requested_root);
    let prompt = match prompt_with_attachments(prompt, &payload, attachment_root) {
        Ok(prompt) => prompt,
        Err(error) => return invalid_request(error),
    };
    // `created` is the store's verdict from the IMMEDIATE transaction inside
    // `resolve_main_session`, not `existing.is_none()`: two first turns can
    // both read `None`, but only one of them creates the pointer, and only
    // that one may unwind it if its launch is refused.
    let (mut record, created) = match existing {
        Some(record) => {
            // An existing pointer is authoritative. A request that names a
            // different harness, familiar, or root is asking for a different
            // conversation; that is what `/reset` is for.
            let mut mismatched = Vec::new();
            if requested_harness.is_some_and(|value| value != record.harness) {
                mismatched.push("harness");
            }
            if requested_familiar.is_some_and(|value| Some(value) != record.familiar_id.as_deref())
            {
                mismatched.push("familiarId");
            }
            if requested_root.is_some_and(|value| Some(value) != record.project_root.as_deref()) {
                mismatched.push("projectRoot");
            }
            if !mismatched.is_empty() {
                return api_error(
                    409,
                    "main_session_mismatch",
                    "The main session already exists with different settings; reset it with the new settings to change them.",
                    Some(json!({
                        "scopeKey": scope,
                        "fields": mismatched,
                        "mainSession": record,
                    })),
                );
            }
            (record, false)
        }
        None => {
            let Some(project_root) = requested_root else {
                return api_error(
                    400,
                    "invalid_request",
                    "the first turn of a main session requires `projectRoot`",
                    Some(json!({ "scopeKey": scope })),
                );
            };
            let resolved = main_session::resolve_main_session(
                &mut conn,
                &scope,
                Some(requested_familiar.unwrap_or(DEFAULT_MAIN_SESSION_FAMILIAR_ID)),
                requested_harness.unwrap_or(DEFAULT_MAIN_SESSION_HARNESS),
                project_root,
                &current_timestamp(),
            )?;
            (resolved.record, resolved.created)
        }
    };

    if !crate::main_session_start::supports_harness(&record.harness) {
        return unsupported_main_harness(&record.harness);
    }

    // Live bound session: pipe the prompt in. `record_input` answers 409
    // `session_not_live` for anything that is not running, which is exactly
    // the signal to launch instead; every other response is returned as-is.
    if let Some(session) = current_session(&conn, &record)? {
        let accepts_turn = if session.status == "running" {
            runtime.live_session_accepts_turn(&session.id)?
        } else {
            None
        };
        if let Some(accepts_turn) = accepts_turn {
            if !accepts_turn {
                return api_error(
                    409,
                    "main_session_busy",
                    "The main session is still processing a turn; retry after it finishes.",
                    Some(json!({ "sessionId": session.id, "scopeKey": scope })),
                );
            }
            let input_body = json!({ "data": prompt }).to_string();
            let response =
                crate::api::record_input(coven_home, &session.id, Some(&input_body), runtime)?;
            if response.status == 202 {
                return json_response(
                    202,
                    &json!({
                        "ok": true,
                        "mode": "input",
                        "session": session,
                        "mainSession": record,
                    }),
                );
            }
            if !(response.status == 409 && response.body.contains("session_not_live")) {
                return Ok(response);
            }
        }
    }

    let Some(project_root) = record.project_root.clone() else {
        return api_error(
            409,
            "main_session_project_root_missing",
            "This main session predates project roots; reset it with `projectRoot` to continue.",
            Some(json!({ "scopeKey": scope, "mainSession": record })),
        );
    };
    let mut conversation_mode = if has_history(&conn, &record)? {
        "resume"
    } else {
        "init"
    };
    let launch_mode = if harness_supports_stream_mode(&record.harness) {
        "stream"
    } else {
        "nonInteractive"
    };
    // The scope gate spans both attempts, but no database transaction spans a launch.
    drop(conn);
    loop {
        let mut launch = json!({
            "projectRoot": project_root,
            "harness": record.harness,
            "prompt": prompt,
            "title": HOME_TITLE,
            "launchMode": launch_mode,
            "conversation": { "mode": conversation_mode, "id": record.conversation_id },
            "conversationId": record.conversation_id,
        });
        if let Some(familiar_id) = record.familiar_id.as_deref() {
            launch["familiarId"] = json!(familiar_id);
        }
        if let Some(model) = model {
            launch["model"] = json!(model);
        }
        let mut startup = None;
        let mut retained_session = None;
        let response = crate::api::launch_session_with_executor(
            coven_home,
            Some(&launch.to_string()),
            authority,
            &mut |launch, writer| {
                match runtime.launch_main_session(launch, writer) {
                    Ok(outcome) => startup = Some(outcome),
                    Err(error) => {
                        if error
                            .downcast_ref::<crate::daemon::RuntimeOwnershipRetainedError>()
                            .is_some()
                        {
                            retained_session = Some(launch.id.clone());
                        }
                        return Err(error);
                    }
                }
                Ok(())
            },
        )?;
        if response.status != 201 {
            if let Some(session_id) = retained_session {
                let mut conn = store::open_store(&store_path(coven_home))?;
                // Preserve the pending process as a killable, busy main session.
                // A runtime retaining ownership must refuse another turn until
                // that process is settled; do not mint a duplicate conversation.
                if main_session::get_main_session(&conn, &scope)?.as_ref() == Some(&record) {
                    main_session::bind_main_session_with_native_id(
                        &mut conn,
                        &record,
                        &session_id,
                        &record.conversation_id,
                        &current_timestamp(),
                    )?;
                }
                return Ok(response);
            }
            if created {
                // The pointer was born for this launch and the launch was
                // refused (unknown familiar, maintenance lock, missing harness
                // binary, ...). Unwind it so the next turn starts from nothing
                // instead of tripping over settings that never ran.
                let conn = store::open_store(&store_path(coven_home))?;
                if main_session::get_main_session(&conn, &scope)?.as_ref() == Some(&record) {
                    main_session::delete_main_session(&conn, &scope)?;
                }
            }
            return Ok(response);
        }
        let session: store::SessionRecord = serde_json::from_str(&response.body)
            .context("launch_session returned 201 with a non-session body")?;
        let mut conn = store::open_store(&store_path(coven_home))?;
        let startup = startup.context("main-session launch omitted startup outcome")?;
        let native_id = match &startup {
            crate::main_session_start::StartupOutcome::Ready(id) => id.as_str(),
            crate::main_session_start::StartupOutcome::Stale => record.conversation_id.as_str(),
        };
        record = match main_session::bind_main_session_with_native_id(
            &mut conn,
            &record,
            &session.id,
            native_id,
            &current_timestamp(),
        ) {
            Ok(record) => record,
            Err(error) => {
                // Never leave a launched harness running with nothing
                // pointing at it. Under the scope gate this only happens when
                // a writer outside the gate moved the pointer mid-launch.
                let _ = runtime.kill_session(&session.id);
                let current = main_session::get_main_session(&conn, &scope)?;
                let moved = match &current {
                    None => true,
                    Some(current) => current != &record,
                };
                if !moved {
                    return Err(error);
                }
                return api_error(
                    409,
                    "main_session_changed",
                    "The main session changed while this turn was launching; the launched session was stopped. Retry the turn.",
                    Some(json!({
                        "scopeKey": scope,
                        "stoppedSessionId": session.id,
                        "mainSession": current,
                    })),
                );
            }
        };
        if startup == crate::main_session_start::StartupOutcome::Stale {
            store::update_session_status_if_current(
                &conn,
                &session.id,
                "running",
                "failed",
                None,
                &current_timestamp(),
            )?;
            if conversation_mode != "resume" {
                return api_error(502, "main_session_start_failed",
                "The harness rejected a fresh main-session conversation; no further retry was attempted.",
                Some(json!({"sessionId":session.id, "mainSession":record})),
            );
            }
            let transaction = main_session::begin_rotation(&mut conn)?;
            let rotated = match main_session::rotate_main_session_generation_in(
                &transaction,
                &record,
                &current_timestamp(),
            )? {
                StaleIdRotation::Rotated(rotated) => rotated,
                StaleIdRotation::AlreadyRotated(current) => {
                    return api_error(
                        409,
                        "main_session_changed",
                        "The main session changed during stale recovery.",
                        Some(json!({"mainSession":current})),
                    );
                }
            };
            finish_rotation(
                coven_home,
                &transaction,
                &rotated,
                EVENT_KIND_ROLLOVER,
                DEFAULT_ROLLOVER_REASON,
            )?;
            transaction.commit()?;
            record = rotated.record;
            conversation_mode = "init";
            continue;
        }
        let session = store::get_session(&conn, &session.id)?
            .context("bound main-session row disappeared")?;
        return json_response(
            201,
            &json!({
                "ok": true,
                "mode": "launched",
                "conversation": conversation_mode,
                "session": session,
                "mainSession": record,
            }),
        );
    }
}

/// Shared tail of reset and rollover: archive the previous row, record the
/// rotation on it so the event stream names both conversation ids. The caller
/// owns the transaction and commits before terminating the previous process.
fn finish_rotation(
    coven_home: &Path,
    conn: &rusqlite::Connection,
    rotated: &RotatedMainSession,
    kind: &str,
    reason: &str,
) -> Result<bool> {
    let now = current_timestamp();
    let Some(previous_id) = rotated.previous_session_id.as_deref() else {
        return Ok(false);
    };
    if store::get_session(conn, previous_id)?.is_none() {
        return Ok(false);
    }
    // `reason` is client-supplied, so the event goes through the instance's
    // configured privacy policy like every other persisted event.
    store::insert_event_with_privacy(
        conn,
        coven_home,
        &store::EventRecord {
            seq: 0,
            id: Uuid::new_v4().to_string(),
            session_id: previous_id.to_string(),
            kind: kind.to_string(),
            payload_json: json!({
                "scopeKey": rotated.record.scope_key,
                "previousConversationId": rotated.previous_conversation_id,
                "conversationId": rotated.record.conversation_id,
                "reason": reason,
            })
            .to_string(),
            created_at: now.clone(),
        },
    )?;
    store::archive_session(conn, previous_id, &now)?;
    Ok(true)
}

fn rotation_response(rotated: &RotatedMainSession, archived: bool) -> Result<ApiResponse> {
    json_response(
        200,
        &json!({
            "ok": true,
            "mainSession": rotated.record,
            "previousConversationId": rotated.previous_conversation_id,
            "previousSessionId": rotated.previous_session_id,
            "archived": archived,
        }),
    )
}

pub(crate) fn reset(
    coven_home: &Path,
    body: Option<&str>,
    runtime: &dyn SessionRuntime,
) -> Result<ApiResponse> {
    let payload = match parse_optional_body(body) {
        Ok(payload) => payload,
        Err(error) => return invalid_request(error),
    };
    let scope = match scope_from(&payload, "") {
        Ok(scope) => scope,
        Err(error) => return invalid_request(error),
    };
    let reason = optional_string(&payload, "reason").unwrap_or("user");
    let root = match optional_setting(&payload, "projectRoot").and_then(canonical_root) {
        Ok(root) => root,
        Err(error) => return invalid_request(error),
    };
    let settings = match (|| {
        Ok::<_, anyhow::Error>(MainSessionSettings {
            familiar_id: optional_setting(&payload, "familiarId")?,
            harness: optional_setting(&payload, "harness")?,
            project_root: root.as_deref(),
        })
    })() {
        Ok(settings) => settings,
        Err(error) => return invalid_request(error),
    };
    if let Some(harness) = settings.harness {
        if !crate::main_session_start::supports_harness(harness) {
            return unsupported_main_harness(harness);
        }
    }
    let _scope_gate = ScopeGate::acquire(coven_home, &scope)?;
    let mut conn = store::open_store(&store_path(coven_home))?;
    if main_session::get_main_session(&conn, &scope)?.is_none() {
        return not_found(&scope);
    }
    let transaction = main_session::begin_rotation(&mut conn)?;
    let rotated =
        main_session::reset_main_session_in(&transaction, &scope, &settings, &current_timestamp())?;
    let archived = finish_rotation(coven_home, &transaction, &rotated, EVENT_KIND_RESET, reason)?;
    let session_to_stop = match rotated.previous_session_id.as_deref() {
        Some(previous_id)
            if store::get_session(&transaction, previous_id)?
                .is_some_and(|session| session.status == "running") =>
        {
            Some(previous_id)
        }
        _ => None,
    };
    transaction.commit()?;
    if let Some(previous_id) = session_to_stop {
        // Persistence succeeded. Process termination is best effort after commit.
        let _ = runtime.kill_session(previous_id);
    }
    rotation_response(&rotated, archived)
}

pub(crate) fn rollover(coven_home: &Path, body: Option<&str>) -> Result<ApiResponse> {
    let payload = match parse_optional_body(body) {
        Ok(payload) => payload,
        Err(error) => return invalid_request(error),
    };
    let scope = match scope_from(&payload, "") {
        Ok(scope) => scope,
        Err(error) => return invalid_request(error),
    };
    let reason = optional_string(&payload, "reason").unwrap_or(DEFAULT_ROLLOVER_REASON);
    // The id the caller saw fail. Required: rotating "whatever is current"
    // would let a late duplicate invalidate a conversation that works.
    let Some(failed_conversation_id) = optional_string(&payload, "conversationId") else {
        return api_error(
            400,
            "invalid_request",
            "rollover requires `conversationId`, the conversation id that failed",
            None,
        );
    };
    let _scope_gate = ScopeGate::acquire(coven_home, &scope)?;
    let mut conn = store::open_store(&store_path(coven_home))?;
    if main_session::get_main_session(&conn, &scope)?.is_none() {
        return not_found(&scope);
    }
    let transaction = main_session::begin_rotation(&mut conn)?;
    let rotated = match main_session::rotate_main_session_conversation_in(
        &transaction,
        &scope,
        failed_conversation_id,
        &current_timestamp(),
    )? {
        StaleIdRotation::Rotated(rotated) => rotated,
        StaleIdRotation::AlreadyRotated(current) => {
            return api_error(
                409,
                "main_session_conversation_changed",
                "The conversation that failed was already replaced; retry the turn on the current one.",
                Some(json!({
                    "scopeKey": scope,
                    "failedConversationId": failed_conversation_id,
                    "mainSession": current,
                })),
            );
        }
    };
    let archived = finish_rotation(
        coven_home,
        &transaction,
        &rotated,
        EVENT_KIND_ROLLOVER,
        reason,
    )?;
    transaction.commit()?;
    rotation_response(&rotated, archived)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{handle_request_with_runtime, SessionLaunch};
    use crate::harness::{ConversationHint, HarnessLaunchMode};
    use std::cell::RefCell;

    #[derive(Default)]
    struct RecordingRuntime {
        launches: RefCell<Vec<SessionLaunch>>,
        inputs: RefCell<Vec<(String, String)>>,
        kills: RefCell<Vec<String>>,
        retain_ownership: std::cell::Cell<bool>,
        startup_outcomes:
            RefCell<std::collections::VecDeque<crate::main_session_start::StartupOutcome>>,
        /// Runs inside `launch_session`, i.e. between a turn's launch and its
        /// bind. Lets a test move the pointer at exactly that moment.
        during_launch: RefCell<Option<Box<dyn Fn()>>>,
    }

    impl SessionRuntime for RecordingRuntime {
        fn launch_session(&self, launch: &SessionLaunch) -> Result<()> {
            self.launches.borrow_mut().push(launch.clone());
            if let Some(hook) = self.during_launch.borrow().as_ref() {
                hook();
            }
            Ok(())
        }

        fn launch_main_session(
            &self,
            launch: &SessionLaunch,
            _writer: Option<crate::maintenance_gate::WriterLease>,
        ) -> Result<crate::main_session_start::StartupOutcome> {
            self.launch_session(launch)?;
            if self.retain_ownership.get() {
                return Err(anyhow::Error::new(
                    crate::daemon::RuntimeOwnershipRetainedError,
                ));
            }
            Ok(self
                .startup_outcomes
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| {
                    crate::main_session_start::StartupOutcome::Ready(
                        launch.conversation_id.clone().unwrap(),
                    )
                }))
        }

        fn live_session_accepts_turn(&self, session_id: &str) -> Result<Option<bool>> {
            Ok(self
                .launches
                .borrow()
                .iter()
                .find(|launch| launch.id == session_id)
                .map(|launch| {
                    !self.retain_ownership.get() && launch.launch_mode == HarnessLaunchMode::Stream
                }))
        }

        fn send_input(&self, session_id: &str, payload: &Value) -> Result<()> {
            let data = payload
                .get("data")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            self.inputs
                .borrow_mut()
                .push((session_id.to_string(), data));
            Ok(())
        }

        fn kill_session(&self, session_id: &str) -> Result<()> {
            self.kills.borrow_mut().push(session_id.to_string());
            Ok(())
        }
    }

    struct Home {
        _temp: tempfile::TempDir,
        home: std::path::PathBuf,
        root: String,
        runtime: RecordingRuntime,
    }

    fn home() -> Home {
        let temp = tempfile::tempdir().unwrap();
        let root_dir = temp.path().join("project");
        std::fs::create_dir_all(&root_dir).unwrap();
        let root = root_dir
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let home = temp.path().join("coven-home");
        std::fs::create_dir_all(&home).unwrap();
        // `launch_session` resolves `familiarId` against familiars.toml and
        // refuses unknown familiars, so Home's default familiar must exist.
        std::fs::write(
            home.join("familiars.toml"),
            r#"[[familiar]]
id = "nova"
display_name = "Nova"
role = "Lead"
description = "Fronts Home."

[[familiar]]
id = "cody"
display_name = "Cody"
role = "Code"
description = "Builds and debugs."
"#,
        )
        .unwrap();
        Home {
            home,
            _temp: temp,
            root,
            runtime: RecordingRuntime::default(),
        }
    }

    fn call(h: &Home, method: &str, route: &str, body: Option<Value>) -> (u16, Value) {
        let body = body.map(|value| value.to_string());
        let response =
            handle_request_with_runtime(method, route, &h.home, None, body.as_deref(), &h.runtime)
                .unwrap();
        let value: Value = serde_json::from_str(&response.body).unwrap_or(Value::Null);
        (response.status, value)
    }

    fn first_turn(h: &Home) -> Value {
        let (status, body) = call(
            h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({ "prompt": "hello", "projectRoot": h.root })),
        );
        assert_eq!(status, 201, "{body}");
        body
    }

    fn set_status(h: &Home, session_id: &str, status: &str) {
        let conn = store::open_store(&store_path(&h.home)).unwrap();
        store::update_session_status(&conn, session_id, status, Some(0), &current_timestamp())
            .unwrap();
    }

    #[test]
    fn get_before_any_turn_is_not_found_and_bad_scope_is_rejected() {
        let h = home();
        let (status, body) = call(&h, "GET", "/api/v1/main-session", None);
        assert_eq!(status, 404);
        assert_eq!(body["error"]["code"], "main_session_not_found");
        assert_eq!(
            body["error"]["details"]["scopeKey"],
            INSTANCE_MAIN_SCOPE_KEY
        );

        let (status, body) = call(&h, "GET", "/api/v1/main-session?scope=Bad%20Key", None);
        assert_eq!(status, 400, "{body}");
    }

    #[test]
    fn main_session_attachments_reach_launch_and_live_input() {
        let h = home();
        std::fs::write(Path::new(&h.root).join("notes.txt"), "project notes").unwrap();
        let payload = json!({"prompt":"read these", "projectRoot":h.root,
            "attachments":["notes.txt"]});
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(payload.clone()),
        );
        assert_eq!(status, 201, "{body}");
        let prompt = h.runtime.launches.borrow()[0].prompt.clone();
        assert!(prompt.contains("notes.txt"), "attachment missing: {prompt}");
        let (status, body) = call(&h, "POST", "/api/v1/main-session/turn", Some(payload));
        assert_eq!(status, 202, "{body}");
        assert_eq!(h.runtime.inputs.borrow()[0].1, prompt);
    }

    #[test]
    fn main_session_invalid_attachments_do_not_create_or_deliver() {
        let h = home();
        std::fs::write(h._temp.path().join("outside.txt"), "outside").unwrap();
        for attachments in [
            json!("notes.txt"),
            json!([4]),
            json!([""]),
            json!(["missing.txt"]),
            json!(["."]),
            json!(["../outside.txt"]),
            json!(vec!["missing.txt"; 17]),
        ] {
            let (status, body) = call(
                &h,
                "POST",
                "/api/v1/main-session/turn",
                Some(json!({"prompt":"hello", "projectRoot":h.root,"attachments":attachments})),
            );
            assert_eq!(status, 400, "{body}");
            let conn = store::open_store(&store_path(&h.home)).unwrap();
            assert!(
                main_session::get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
                    .unwrap()
                    .is_none()
            );
        }
        first_turn(&h);
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt":"hello", "attachments":["../outside.txt"]})),
        );
        assert_eq!(status, 400, "{body}");
        assert!(h.runtime.inputs.borrow().is_empty());
        assert_eq!(h.runtime.launches.borrow().len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn main_session_attachment_symlink_cannot_escape_project() {
        let h = home();
        let outside = h._temp.path().join("outside.txt");
        std::fs::write(&outside, "outside").unwrap();
        std::os::unix::fs::symlink(&outside, Path::new(&h.root).join("linked.txt")).unwrap();
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt":"hello", "projectRoot":h.root,"attachments":["linked.txt"]})),
        );
        assert_eq!(status, 400, "{body}");
        assert!(h.runtime.launches.borrow().is_empty());
        let conn = store::open_store(&store_path(&h.home)).unwrap();
        assert!(
            main_session::get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn first_turn_requires_prompt_and_project_root() {
        let h = home();
        let (status, body) = call(&h, "POST", "/api/v1/main-session/turn", Some(json!({})));
        assert_eq!(status, 400);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("prompt"));

        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({ "prompt": "hello" })),
        );
        assert_eq!(status, 400);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("projectRoot"));
        assert!(h.runtime.launches.borrow().is_empty());

        let (status, _) = call(&h, "POST", "/api/v1/main-session/turn", None);
        assert_eq!(status, 400);
        let (status, _) = call(&h, "GET", "/api/v1/main-session", None);
        assert_eq!(status, 404, "a rejected turn must not create the pointer");
    }

    #[test]
    fn a_refused_first_launch_leaves_no_pointer_behind() {
        let h = home();
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({ "prompt": "hello", "projectRoot": h.root, "familiarId": "ghost" })),
        );
        assert_eq!(status, 400, "{body}");
        assert_eq!(body["error"]["code"], "unknown_familiar");
        assert!(h.runtime.launches.borrow().is_empty());
        let (status, _) = call(&h, "GET", "/api/v1/main-session", None);
        assert_eq!(status, 404, "the pointer must be unwound");

        // The next turn is a clean first turn again, not a mismatch.
        let body = first_turn(&h);
        assert_eq!(body["conversation"], "init");
        assert_eq!(body["mainSession"]["familiarId"], "nova");
    }

    #[test]
    fn turn_canonicalizes_project_root_before_persisting_and_comparing() {
        let h = home();
        let alias = Path::new(&h.root).join(".").to_string_lossy().into_owned();
        let (status, first) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt": "hello", "projectRoot": alias})),
        );
        assert_eq!(status, 201, "{first}");
        let canonical = crate::project::canonical_project_root(Path::new(&h.root)).unwrap();
        assert_eq!(
            first["mainSession"]["projectRoot"],
            canonical.to_string_lossy().as_ref()
        );
        let (status, second) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt": "again", "projectRoot": canonical})),
        );
        assert_eq!(status, 202, "{second}");
    }

    #[test]
    fn reset_canonicalizes_replacement_project_root() {
        let h = home();
        first_turn(&h);
        let replacement = h._temp.path().join("replacement");
        fs::create_dir(&replacement).unwrap();
        let (status, reset) = call(
            &h,
            "POST",
            "/api/v1/main-session/reset",
            Some(json!({"projectRoot": replacement.join(".")})),
        );
        assert_eq!(status, 200, "{reset}");
        let canonical = crate::project::canonical_project_root(&replacement).unwrap();
        assert_eq!(
            reset["mainSession"]["projectRoot"],
            canonical.to_string_lossy().as_ref()
        );
    }

    #[test]
    fn reset_rejects_file_project_root_without_changing_pointer() {
        let h = home();
        let first = first_turn(&h);
        let file = h._temp.path().join("not-a-directory");
        fs::write(&file, "data").unwrap();
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/reset",
            Some(json!({"projectRoot": file})),
        );
        assert_eq!(status, 400, "{body}");
        let (_, current) = call(&h, "GET", "/api/v1/main-session", None);
        assert_eq!(current["mainSession"], first["mainSession"]);
        assert!(h.runtime.kills.borrow().is_empty());
    }

    #[test]
    fn missing_runtime_handle_resumes_instead_of_writing_to_stale_running_row() {
        let h = home();
        let first = first_turn(&h);
        h.runtime.launches.borrow_mut().clear();
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt": "after restart"})),
        );
        assert_eq!(status, 201, "{body}");
        assert_eq!(body["conversation"], "resume");
        assert_eq!(
            body["mainSession"]["conversationId"],
            first["mainSession"]["conversationId"]
        );
        assert!(h.runtime.inputs.borrow().is_empty());
        assert_eq!(h.runtime.launches.borrow().len(), 1);
    }

    #[test]
    fn bound_claude_one_shot_is_busy_even_though_harness_supports_streaming() {
        let h = home();
        first_turn(&h);
        h.runtime.launches.borrow_mut()[0].launch_mode = HarnessLaunchMode::NonInteractive;
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt": "another turn"})),
        );
        assert_eq!(status, 409, "{body}");
        assert_eq!(body["error"]["code"], "main_session_busy");
        assert!(h.runtime.inputs.borrow().is_empty());
        assert_eq!(h.runtime.launches.borrow().len(), 1);
    }

    #[test]
    fn running_one_shot_turn_is_busy_without_sending_stdin() {
        let h = home();
        let (status, first) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt": "hello", "projectRoot": h.root, "harness": "codex"})),
        );
        assert_eq!(status, 201, "{first}");
        let (status, second) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt": "again"})),
        );
        assert_eq!(status, 409, "{second}");
        assert_eq!(second["error"]["code"], "main_session_busy");
        assert!(h.runtime.inputs.borrow().is_empty());
        assert_eq!(h.runtime.launches.borrow().len(), 1);
    }

    #[test]
    fn first_turn_creates_the_pointer_and_launches_init_in_stream_mode() {
        let h = home();
        let body = first_turn(&h);
        assert_eq!(body["mode"], "launched");
        assert_eq!(body["conversation"], "init");
        let conversation = body["mainSession"]["conversationId"].as_str().unwrap();
        let session_id = body["session"]["id"].as_str().unwrap();
        assert_eq!(body["mainSession"]["currentSessionId"], session_id);
        assert_eq!(body["mainSession"]["harness"], DEFAULT_MAIN_SESSION_HARNESS);
        assert_eq!(
            body["mainSession"]["familiarId"],
            DEFAULT_MAIN_SESSION_FAMILIAR_ID
        );
        assert_eq!(body["mainSession"]["projectRoot"], h.root);
        assert_eq!(body["session"]["conversation_id"], conversation);
        assert_eq!(body["session"]["title"], HOME_TITLE);

        let launches = h.runtime.launches.borrow();
        assert_eq!(launches.len(), 1);
        assert_eq!(launches[0].harness, "claude");
        assert_eq!(launches[0].launch_mode, HarnessLaunchMode::Stream);
        assert_eq!(launches[0].prompt, "hello");
        assert_eq!(
            launches[0].conversation,
            Some(ConversationHint::Init {
                id: conversation.to_string()
            })
        );
        assert_eq!(launches[0].conversation_id.as_deref(), Some(conversation));
        assert_eq!(launches[0].familiar_id.as_deref(), Some("nova"));
        assert!(h.runtime.inputs.borrow().is_empty());
        drop(launches);

        let (status, get) = call(&h, "GET", "/api/v1/main-session", None);
        assert_eq!(status, 200);
        assert_eq!(get["mainSession"]["conversationId"], conversation);
        assert_eq!(get["currentSession"]["id"], session_id);
        assert_eq!(get["currentSession"]["status"], "running");
        assert_eq!(get["hasHistory"], true);
    }

    #[test]
    fn second_turn_on_a_live_session_is_piped_as_input() {
        let h = home();
        let first = first_turn(&h);
        let session_id = first["session"]["id"].as_str().unwrap().to_string();

        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({ "prompt": "and again" })),
        );
        assert_eq!(status, 202, "{body}");
        assert_eq!(body["mode"], "input");
        assert_eq!(body["session"]["id"], session_id);
        assert_eq!(h.runtime.launches.borrow().len(), 1);
        assert_eq!(
            *h.runtime.inputs.borrow(),
            vec![(session_id, "and again".to_string())]
        );
    }

    #[test]
    fn unsupported_main_harness_is_rejected_before_creation_or_reset() {
        for harness in ["copilot", "unknown-harness"] {
            let h = home();
            let (status, body) = call(
                &h,
                "POST",
                "/api/v1/main-session/turn",
                Some(json!({"prompt":"hello", "harness":harness, "projectRoot":h.root})),
            );
            assert_eq!(status, 400, "{body}");
            assert_eq!(body["error"]["code"], "unsupported_main_session_harness");
            assert_eq!(call(&h, "GET", "/api/v1/main-session", None).0, 404);
            assert!(h.runtime.launches.borrow().is_empty());
            let first = first_turn(&h);
            let (status, body) = call(
                &h,
                "POST",
                "/api/v1/main-session/reset",
                Some(json!({"harness":harness})),
            );
            assert_eq!(status, 400, "{body}");
            assert_eq!(body["error"]["code"], "unsupported_main_session_harness");
            let (_, current) = call(&h, "GET", "/api/v1/main-session", None);
            assert_eq!(current["mainSession"], first["mainSession"]);
            assert!(h.runtime.kills.borrow().is_empty());
            assert!(current["currentSession"]["archived_at"].is_null());
        }
    }

    #[test]
    fn retained_startup_ownership_preserves_bound_running_row_and_blocks_retry() {
        let h = home();
        h.runtime.retain_ownership.set(true);
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt":"hello", "projectRoot":h.root})),
        );
        assert_eq!(status, 500, "{body}");
        let (status, current) = call(&h, "GET", "/api/v1/main-session", None);
        assert_eq!(status, 200, "{current}");
        assert_eq!(current["currentSession"]["status"], "running");
        assert!(current["mainSession"]["currentSessionId"].is_string());
        let (status, retry) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt":"again"})),
        );
        assert_eq!(status, 409, "{retry}");
        assert_eq!(h.runtime.launches.borrow().len(), 1);
    }

    #[test]
    fn native_startup_id_is_persisted_and_used_on_next_turn() {
        let h = home();
        let native = Uuid::new_v4().to_string();
        h.runtime.startup_outcomes.borrow_mut().push_back(
            crate::main_session_start::StartupOutcome::Ready(native.clone()),
        );
        let (status, first) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt":"first", "projectRoot":h.root, "harness":"codex"})),
        );
        assert_eq!(status, 201, "{first}");
        assert_eq!(first["mainSession"]["conversationId"], native);
        assert_eq!(first["session"]["conversation_id"], native);
        set_status(&h, first["session"]["id"].as_str().unwrap(), "completed");
        let (status, second) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt":"again"})),
        );
        assert_eq!(status, 201, "{second}");
        assert_eq!(
            h.runtime.launches.borrow()[1].conversation,
            Some(ConversationHint::Resume { id: native })
        );
    }

    #[test]
    fn stale_resume_rotates_and_retries_the_same_turn_once() {
        let h = home();
        std::fs::write(Path::new(&h.root).join("retry.txt"), "context").unwrap();
        let first = first_turn(&h);
        set_status(&h, first["session"]["id"].as_str().unwrap(), "completed");
        let fresh = Uuid::new_v4().to_string();
        h.runtime.startup_outcomes.borrow_mut().extend([
            crate::main_session_start::StartupOutcome::Stale,
            crate::main_session_start::StartupOutcome::Ready(fresh.clone()),
        ]);
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(
                json!({"prompt":"keep this prompt", "model":"test-model", "attachments":["retry.txt"]}),
            ),
        );
        assert_eq!(status, 201, "{body}");
        assert_eq!(body["mainSession"]["conversationId"], fresh);
        assert_eq!(body["mainSession"]["resetCount"], 0);
        let launches = h.runtime.launches.borrow();
        assert_eq!(launches.len(), 3);
        assert!(matches!(
            launches[1].conversation,
            Some(ConversationHint::Resume { .. })
        ));
        assert!(matches!(
            launches[2].conversation,
            Some(ConversationHint::Init { .. })
        ));
        assert!(launches[1].prompt.contains("retry.txt"));
        assert_eq!(launches[1].prompt, launches[2].prompt);
        assert_eq!(launches[1].model, launches[2].model);
        let conn = store::open_store(&store_path(&h.home)).unwrap();
        assert!(store::get_session(&conn, &launches[1].id)
            .unwrap()
            .unwrap()
            .archived_at
            .is_some());
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE kind = 'main_session.rollover'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn stale_fresh_retry_is_not_retried_again() {
        let h = home();
        let first = first_turn(&h);
        set_status(&h, first["session"]["id"].as_str().unwrap(), "completed");
        h.runtime.startup_outcomes.borrow_mut().extend([
            crate::main_session_start::StartupOutcome::Stale,
            crate::main_session_start::StartupOutcome::Stale,
        ]);
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({"prompt":"retry once"})),
        );
        assert_eq!(status, 502, "{body}");
        assert_eq!(body["error"]["code"], "main_session_start_failed");
        assert_eq!(h.runtime.launches.borrow().len(), 3);
        let conn = store::open_store(&store_path(&h.home)).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM events WHERE kind = 'main_session.rollover'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn turn_after_the_session_ended_relaunches_with_resume_and_rebinds() {
        let h = home();
        let first = first_turn(&h);
        let conversation = first["mainSession"]["conversationId"]
            .as_str()
            .unwrap()
            .to_string();
        let first_session = first["session"]["id"].as_str().unwrap().to_string();
        set_status(&h, &first_session, "exited");

        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({ "prompt": "back again" })),
        );
        assert_eq!(status, 201, "{body}");
        assert_eq!(body["mode"], "launched");
        assert_eq!(body["conversation"], "resume");
        let second_session = body["session"]["id"].as_str().unwrap();
        assert_ne!(second_session, first_session);
        assert_eq!(body["mainSession"]["conversationId"], conversation);
        assert_eq!(body["mainSession"]["currentSessionId"], second_session);

        let launches = h.runtime.launches.borrow();
        assert_eq!(launches.len(), 2);
        assert_eq!(
            launches[1].conversation,
            Some(ConversationHint::Resume {
                id: conversation.clone()
            })
        );
        assert_eq!(launches[1].prompt, "back again");
        assert!(h.runtime.inputs.borrow().is_empty());
    }

    #[test]
    fn turn_with_different_settings_is_a_conflict_not_a_silent_switch() {
        let h = home();
        first_turn(&h);
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({ "prompt": "x", "harness": "codex", "familiarId": "cody" })),
        );
        assert_eq!(status, 409, "{body}");
        assert_eq!(body["error"]["code"], "main_session_mismatch");
        assert_eq!(
            body["error"]["details"]["fields"],
            json!(["harness", "familiarId"])
        );
        assert_eq!(h.runtime.launches.borrow().len(), 1);
        assert!(h.runtime.inputs.borrow().is_empty());

        // Naming the same settings is fine.
        let (status, _) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(
                json!({ "prompt": "y", "harness": "claude", "familiarId": "nova", "projectRoot": h.root }),
            ),
        );
        assert_eq!(status, 202);
    }

    #[test]
    fn rotation_failure_rolls_back_pointer_event_and_archive_before_killing() {
        for route in ["reset", "rollover"] {
            for fail_archive in [false, true] {
                let h = home();
                let first = first_turn(&h);
                let id = first["session"]["id"].as_str().unwrap();
                let conn = store::open_store(&store_path(&h.home)).unwrap();
                let trigger = if fail_archive {
                    "CREATE TRIGGER fail_rotation BEFORE UPDATE OF archived_at ON sessions
                     WHEN NEW.archived_at IS NOT NULL BEGIN SELECT RAISE(ABORT, 'archive failed'); END;"
                } else {
                    "CREATE TRIGGER fail_rotation BEFORE INSERT ON events
                     WHEN NEW.kind LIKE 'main_session.%' BEGIN SELECT RAISE(ABORT, 'event failed'); END;"
                };
                conn.execute_batch(trigger).unwrap();
                let body =
                    json!({"conversationId": first["mainSession"]["conversationId"]}).to_string();
                let result = handle_request_with_runtime(
                    "POST",
                    &format!("/api/v1/main-session/{route}"),
                    &h.home,
                    None,
                    Some(&body),
                    &h.runtime,
                );
                assert!(result.is_err(), "{route}, archive={fail_archive}");
                let pointer = main_session::get_main_session(&conn, INSTANCE_MAIN_SCOPE_KEY)
                    .unwrap()
                    .unwrap();
                assert_eq!(
                    pointer.conversation_id,
                    first["mainSession"]["conversationId"].as_str().unwrap(),
                    "{route}, archive={fail_archive}"
                );
                assert_eq!(pointer.current_session_id.as_deref(), Some(id));
                assert_eq!(pointer.reset_count, 0);
                assert!(store::get_session(&conn, id)
                    .unwrap()
                    .unwrap()
                    .archived_at
                    .is_none());
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM events WHERE kind LIKE 'main_session.%'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap();
                assert_eq!(count, 0);
                assert!(
                    h.runtime.kills.borrow().is_empty(),
                    "must commit rotation before terminating its process"
                );
            }
        }
    }

    #[test]
    fn reset_rotates_kills_archives_records_and_the_next_turn_is_init() {
        let h = home();
        let first = first_turn(&h);
        let old_conversation = first["mainSession"]["conversationId"]
            .as_str()
            .unwrap()
            .to_string();
        let old_session = first["session"]["id"].as_str().unwrap().to_string();

        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/reset",
            Some(json!({ "reason": "user asked" })),
        );
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["previousConversationId"], old_conversation);
        assert_eq!(body["previousSessionId"], old_session);
        assert_eq!(body["archived"], true);
        assert_eq!(body["mainSession"]["resetCount"], 1);
        assert!(body["mainSession"]["currentSessionId"].is_null());
        let new_conversation = body["mainSession"]["conversationId"].as_str().unwrap();
        assert_ne!(new_conversation, old_conversation);
        assert_eq!(*h.runtime.kills.borrow(), vec![old_session.clone()]);

        let conn = store::open_store(&store_path(&h.home)).unwrap();
        let archived = store::get_session(&conn, &old_session).unwrap().unwrap();
        assert!(archived.archived_at.is_some());
        assert!(store::event_kind_exists(&conn, &old_session, EVENT_KIND_RESET).unwrap());
        drop(conn);

        let (status, get) = call(&h, "GET", "/api/v1/main-session", None);
        assert_eq!(status, 200);
        assert!(get["currentSession"].is_null());
        assert_eq!(get["hasHistory"], false);

        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({ "prompt": "fresh" })),
        );
        assert_eq!(status, 201, "{body}");
        assert_eq!(body["conversation"], "init");
        let launches = h.runtime.launches.borrow();
        assert_eq!(launches.len(), 2);
        assert_eq!(
            launches[1].conversation,
            Some(ConversationHint::Init {
                id: new_conversation.to_string()
            })
        );
    }

    #[test]
    fn reset_with_settings_unsticks_a_rootless_pointer_and_changes_settings() {
        let h = home();
        let first = first_turn(&h);
        // Not live, so the next turn has to launch rather than pipe input.
        set_status(&h, first["session"]["id"].as_str().unwrap(), "exited");
        let other_dir = h._temp.path().join("other-project");
        std::fs::create_dir_all(&other_dir).unwrap();
        // The daemon's own normalization, not `Path::canonicalize`: on
        // Windows the latter adds a `\\?\` prefix that `launch_session`
        // strips, so the two spellings would never compare equal.
        let other = crate::project::canonical_project_root(&other_dir)
            .unwrap()
            .to_string_lossy()
            .into_owned();

        // A pointer from before `project_root` existed cannot launch...
        let conn = store::open_store(&store_path(&h.home)).unwrap();
        conn.execute("UPDATE main_sessions SET project_root = NULL", [])
            .unwrap();
        drop(conn);
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({ "prompt": "stuck" })),
        );
        assert_eq!(status, 409, "{body}");
        assert_eq!(body["error"]["code"], "main_session_project_root_missing");

        // ...and the reset its error message asks for repairs it, along with
        // any other setting the reset carries.
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/reset",
            Some(json!({ "projectRoot": other, "familiarId": "cody" })),
        );
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["mainSession"]["projectRoot"], other);
        assert_eq!(body["mainSession"]["familiarId"], "cody");
        assert_eq!(body["mainSession"]["harness"], "claude");

        // A turn that names the new settings is no longer a mismatch, and
        // the launch uses them.
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({ "prompt": "unstuck", "familiarId": "cody", "projectRoot": other })),
        );
        assert_eq!(status, 201, "{body}");
        let launches = h.runtime.launches.borrow();
        let last = launches.last().unwrap();
        assert_eq!(last.project_root, other);
        assert_eq!(last.familiar_id.as_deref(), Some("cody"));
    }

    #[test]
    fn reset_rejects_malformed_settings_without_rotating() {
        let h = home();
        let first = first_turn(&h);
        for body in [
            json!({ "harness": 7 }),
            json!({ "projectRoot": "   " }),
            json!({ "familiarId": ["nova"] }),
        ] {
            let (status, response) =
                call(&h, "POST", "/api/v1/main-session/reset", Some(body.clone()));
            assert_eq!(status, 400, "{body}: {response}");
            assert_eq!(response["error"]["code"], "invalid_request");
        }
        let (_, get) = call(&h, "GET", "/api/v1/main-session", None);
        assert_eq!(
            get["mainSession"]["conversationId"],
            first["mainSession"]["conversationId"]
        );
        assert_eq!(get["mainSession"]["resetCount"], 0);
    }

    #[test]
    fn rotation_events_honour_the_configured_privacy_policy() {
        let h = home();
        std::fs::write(
            h.home.join("privacy.toml"),
            "extra_patterns = [\"custom-sensitive-[0-9]+\"]\n",
        )
        .unwrap();
        let first = first_turn(&h);
        let old_session = first["session"]["id"].as_str().unwrap().to_string();

        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/reset",
            Some(json!({ "reason": "custom-sensitive-1234" })),
        );
        assert_eq!(status, 200, "{body}");

        let conn = store::open_store(&store_path(&h.home)).unwrap();
        let events = store::list_events(&conn, &old_session).unwrap();
        let reset = events
            .iter()
            .find(|event| event.kind == EVENT_KIND_RESET)
            .expect("reset event recorded");
        assert!(
            !reset.payload_json.contains("custom-sensitive-1234"),
            "{}",
            reset.payload_json
        );
        assert!(
            reset.payload_json.contains("[REDACTED]"),
            "{}",
            reset.payload_json
        );
    }

    #[test]
    fn rollover_is_not_counted_and_does_not_kill() {
        let h = home();
        let first = first_turn(&h);
        let old_session = first["session"]["id"].as_str().unwrap().to_string();
        set_status(&h, &old_session, "exited");
        let failed = first["mainSession"]["conversationId"].clone();

        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/rollover",
            Some(json!({ "conversationId": failed })),
        );
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["mainSession"]["resetCount"], 0);
        assert_eq!(body["archived"], true);
        assert!(h.runtime.kills.borrow().is_empty());

        let conn = store::open_store(&store_path(&h.home)).unwrap();
        assert!(store::event_kind_exists(&conn, &old_session, EVENT_KIND_ROLLOVER).unwrap());
        drop(conn);

        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({ "prompt": "after rollover" })),
        );
        assert_eq!(status, 201, "{body}");
        assert_eq!(body["conversation"], "init");
    }

    #[test]
    fn a_late_rollover_for_a_replaced_conversation_changes_nothing() {
        let h = home();
        let first = first_turn(&h);
        set_status(&h, first["session"]["id"].as_str().unwrap(), "exited");
        let failed = first["mainSession"]["conversationId"].clone();

        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/rollover",
            Some(json!({ "conversationId": failed })),
        );
        assert_eq!(status, 200, "{body}");
        let fresh = body["mainSession"]["conversationId"].clone();

        // A duplicate report of the same failure arrives late.
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/rollover",
            Some(json!({ "conversationId": failed })),
        );
        assert_eq!(status, 409, "{body}");
        assert_eq!(body["error"]["code"], "main_session_conversation_changed");
        let (_, get) = call(&h, "GET", "/api/v1/main-session", None);
        assert_eq!(get["mainSession"]["conversationId"], fresh);

        // Rollover without the failed id is refused outright.
        let (status, body) = call(&h, "POST", "/api/v1/main-session/rollover", None);
        assert_eq!(status, 400, "{body}");
        assert_eq!(body["error"]["code"], "invalid_request");
    }

    #[test]
    fn reset_and_rollover_before_any_pointer_are_not_found() {
        let h = home();
        for (route, body) in [
            ("/api/v1/main-session/reset", None),
            (
                "/api/v1/main-session/rollover",
                Some(json!({ "conversationId": "c1" })),
            ),
        ] {
            let (status, body) = call(&h, "POST", route, body);
            assert_eq!(status, 404, "{route}: {body}");
            assert_eq!(body["error"]["code"], "main_session_not_found");
        }
    }

    #[test]
    fn scope_gate_serializes_one_scope_and_leaves_others_free() {
        use std::sync::mpsc;
        use std::time::Duration;

        let h = home();
        let held = ScopeGate::acquire(&h.home, INSTANCE_MAIN_SCOPE_KEY).unwrap();

        // Another scope is never blocked by this one.
        drop(ScopeGate::acquire(&h.home, "familiar:nova:main").unwrap());

        let (acquired_tx, acquired_rx) = mpsc::channel();
        let contender_home = h.home.clone();
        let contender = std::thread::spawn(move || {
            let gate = ScopeGate::acquire(&contender_home, INSTANCE_MAIN_SCOPE_KEY).unwrap();
            acquired_tx.send(()).unwrap();
            drop(gate);
        });
        assert!(
            acquired_rx
                .recv_timeout(Duration::from_millis(300))
                .is_err(),
            "a second holder of the same scope must wait"
        );
        drop(held);
        acquired_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("released gate must admit the waiter");
        contender.join().unwrap();
    }

    #[test]
    fn mutating_routes_wait_for_the_scope_gate() {
        use std::sync::mpsc;
        use std::time::Duration;

        let h = home();
        let first = first_turn(&h);
        set_status(&h, first["session"]["id"].as_str().unwrap(), "exited");

        for (route, body) in [
            ("/api/v1/main-session/turn", json!({ "prompt": "queued" })),
            ("/api/v1/main-session/reset", json!({ "reason": "queued" })),
        ] {
            let held = ScopeGate::acquire(&h.home, INSTANCE_MAIN_SCOPE_KEY).unwrap();
            let (done_tx, done_rx) = mpsc::channel();
            let request_home = h.home.clone();
            let request_body = body.to_string();
            let request = std::thread::spawn(move || {
                // Its own runtime: the recording runtime is not `Sync`.
                let runtime = RecordingRuntime::default();
                let response = handle_request_with_runtime(
                    "POST",
                    route,
                    &request_home,
                    None,
                    Some(&request_body),
                    &runtime,
                )
                .unwrap();
                done_tx.send(response.status).unwrap();
            });
            assert!(
                done_rx.recv_timeout(Duration::from_millis(300)).is_err(),
                "{route} must not proceed while another request holds the scope"
            );
            drop(held);
            let status = done_rx
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| panic!("{route} never finished"));
            assert!(matches!(status, 200 | 201), "{route}: {status}");
            request.join().unwrap();
        }
    }

    #[test]
    fn a_pointer_moved_mid_launch_stops_the_launched_session() {
        let h = home();
        let first = first_turn(&h);
        set_status(&h, first["session"]["id"].as_str().unwrap(), "exited");

        // A writer outside the gate resets Home while the next turn is
        // between its launch and its bind.
        let store_file = store_path(&h.home);
        *h.runtime.during_launch.borrow_mut() = Some(Box::new(move || {
            let mut conn = store::open_store(&store_file).unwrap();
            main_session::reset_main_session(
                &mut conn,
                INSTANCE_MAIN_SCOPE_KEY,
                &MainSessionSettings::default(),
                &current_timestamp(),
            )
            .unwrap();
        }));

        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(json!({ "prompt": "raced" })),
        );
        assert_eq!(status, 409, "{body}");
        assert_eq!(body["error"]["code"], "main_session_changed");
        let stopped = body["error"]["details"]["stoppedSessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("no stoppedSessionId in {body}"))
            .to_string();
        assert_eq!(*h.runtime.kills.borrow(), vec![stopped]);

        let (_, get) = call(&h, "GET", "/api/v1/main-session", None);
        assert!(get["mainSession"]["currentSessionId"].is_null());
    }

    #[test]
    fn scopes_are_independent_over_http() {
        let h = home();
        first_turn(&h);
        let (status, body) = call(
            &h,
            "POST",
            "/api/v1/main-session/turn",
            Some(
                json!({ "scope": "familiar:cody:main", "prompt": "hi", "projectRoot": h.root, "familiarId": "cody" }),
            ),
        );
        assert_eq!(status, 201, "{body}");
        assert_eq!(body["mainSession"]["scopeKey"], "familiar:cody:main");
        assert_eq!(body["mainSession"]["familiarId"], "cody");
        let (status, get) = call(
            &h,
            "GET",
            "/api/v1/main-session?scope=familiar:cody:main",
            None,
        );
        assert_eq!(status, 200);
        assert_eq!(get["mainSession"]["familiarId"], "cody");
        assert_eq!(h.runtime.launches.borrow().len(), 2);
    }
}
