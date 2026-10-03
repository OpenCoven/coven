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
//!   harness no longer recognises: same rotation, archive, not counted.
//!
//! The handlers compose the existing `launch_session` and `record_input`
//! handlers rather than re-implementing a launch, so every gate those routes
//! enforce (maintenance writer, harness validation, authority, context
//! admission) applies to Home unchanged.
//!
//! Stale-id detection is not yet daemon-side: claude and codex exit 0 on a
//! stale id and only print a phrase (`docs/chat-persistence.md`), and the
//! only matcher today lives in the legacy TUI. Until the daemon watches its
//! own output stream for that phrase, the client that observes it calls
//! `/rollover`; the daemon-side detector will call the same store path.

use std::path::Path;

use anyhow::{Context, Result};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::{
    api::{current_timestamp, store_path, SessionRuntime},
    api_response::{api_error, json_response, ApiResponse},
    harness::harness_supports_stream_mode,
    main_session::{
        self, MainSessionRecord, MainSessionSettings, RotatedMainSession,
        DEFAULT_MAIN_SESSION_FAMILIAR_ID, DEFAULT_MAIN_SESSION_HARNESS, INSTANCE_MAIN_SCOPE_KEY,
    },
    request_authority::RequestAuthority,
    store,
};

const EVENT_KIND_RESET: &str = "main_session.reset";
const EVENT_KIND_ROLLOVER: &str = "main_session.rollover";
const DEFAULT_ROLLOVER_REASON: &str = "stale-conversation";
const HOME_TITLE: &str = "Home";

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
    let requested_harness = optional_string(&payload, "harness");
    let requested_familiar = optional_string(&payload, "familiarId");
    let requested_root = optional_string(&payload, "projectRoot");
    let model = optional_string(&payload, "model");

    let mut conn = store::open_store(&store_path(coven_home))?;
    let existing = main_session::get_main_session(&conn, &scope)?;
    // `created` is the store's verdict from the IMMEDIATE transaction inside
    // `resolve_main_session`, not `existing.is_none()`: two first turns can
    // both read `None`, but only one of them creates the pointer, and only
    // that one may unwind it if its launch is refused.
    let (record, created) = match existing {
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

    // Live bound session: pipe the prompt in. `record_input` answers 409
    // `session_not_live` for anything that is not running, which is exactly
    // the signal to launch instead; every other response is returned as-is.
    if let Some(session) = current_session(&conn, &record)? {
        if session.status == "running" {
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
    let conversation_mode = if has_history(&conn, &record)? {
        "resume"
    } else {
        "init"
    };
    let launch_mode = if harness_supports_stream_mode(&record.harness) {
        "stream"
    } else {
        "nonInteractive"
    };
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
    // Hold no transaction across the launch: it opens its own connection and
    // must be free to write the session row.
    drop(conn);
    let response =
        crate::api::launch_session(coven_home, Some(&launch.to_string()), runtime, authority)?;
    if response.status != 201 {
        if created {
            // The pointer was born for this launch and the launch was
            // refused (unknown familiar, maintenance lock, missing harness
            // binary, ...). Unwind it so the next turn starts from nothing
            // instead of tripping over settings that never ran.
            let conn = store::open_store(&store_path(coven_home))?;
            main_session::delete_main_session(&conn, &scope)?;
        }
        return Ok(response);
    }
    let session: store::SessionRecord = serde_json::from_str(&response.body)
        .context("launch_session returned 201 with a non-session body")?;
    let mut conn = store::open_store(&store_path(coven_home))?;
    let record =
        main_session::bind_main_session(&mut conn, &scope, &session.id, &current_timestamp())?;
    json_response(
        201,
        &json!({
            "ok": true,
            "mode": "launched",
            "conversation": conversation_mode,
            "session": session,
            "mainSession": record,
        }),
    )
}

/// Shared tail of reset and rollover: archive the previous row, record the
/// rotation on it so the event stream names both conversation ids, and
/// optionally kill its process.
fn finish_rotation(
    coven_home: &Path,
    conn: &rusqlite::Connection,
    rotated: &RotatedMainSession,
    kind: &str,
    reason: &str,
    runtime: Option<&dyn SessionRuntime>,
) -> Result<bool> {
    let now = current_timestamp();
    let Some(previous_id) = rotated.previous_session_id.as_deref() else {
        return Ok(false);
    };
    let Some(previous) = store::get_session(conn, previous_id)? else {
        return Ok(false);
    };
    if let Some(runtime) = runtime {
        if previous.status == "running" {
            // Best effort: a process that already exited answers not-live,
            // which is the state we want anyway.
            let _ = runtime.kill_session(previous_id);
        }
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
    let settings = match (|| {
        Ok::<_, anyhow::Error>(MainSessionSettings {
            familiar_id: optional_setting(&payload, "familiarId")?,
            harness: optional_setting(&payload, "harness")?,
            project_root: optional_setting(&payload, "projectRoot")?,
        })
    })() {
        Ok(settings) => settings,
        Err(error) => return invalid_request(error),
    };
    let mut conn = store::open_store(&store_path(coven_home))?;
    if main_session::get_main_session(&conn, &scope)?.is_none() {
        return not_found(&scope);
    }
    let rotated =
        main_session::reset_main_session(&mut conn, &scope, &settings, &current_timestamp())?;
    let archived = finish_rotation(
        coven_home,
        &conn,
        &rotated,
        EVENT_KIND_RESET,
        reason,
        Some(runtime),
    )?;
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
    let mut conn = store::open_store(&store_path(coven_home))?;
    if main_session::get_main_session(&conn, &scope)?.is_none() {
        return not_found(&scope);
    }
    let rotated =
        main_session::rotate_main_session_conversation(&mut conn, &scope, &current_timestamp())?;
    let archived = finish_rotation(
        coven_home,
        &conn,
        &rotated,
        EVENT_KIND_ROLLOVER,
        reason,
        None,
    )?;
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
        let other = other_dir
            .canonicalize()
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

        let (status, body) = call(&h, "POST", "/api/v1/main-session/rollover", None);
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
    fn reset_and_rollover_before_any_pointer_are_not_found() {
        let h = home();
        for route in [
            "/api/v1/main-session/reset",
            "/api/v1/main-session/rollover",
        ] {
            let (status, body) = call(&h, "POST", route, None);
            assert_eq!(status, 404, "{route}: {body}");
            assert_eq!(body["error"]["code"], "main_session_not_found");
        }
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
