//! `coven.automations.command.v1`: the spec command envelope over the
//! control-action wire (coven#1054).
//!
//! The request carries `{ "action": "coven.automations.command.v1",
//! "envelope": <commandEnvelope> }`. The envelope is validated as a typed
//! [`CommandRequest`], translated onto the producer's existing flat adapter for
//! that command, and answered with a spec `commandResponse` in `result`.
//! Nothing here executes on its own: every commit, replay, rejection and event
//! comes from the same adapter the flat action uses, so an adoption key
//! replays identically on either wire.
//!
//! Only the envelope's supported subset is translated. A command or field the
//! producer cannot honour is refused with `CAPABILITY_UNSUPPORTED` rather than
//! dropped: `definition.create.v1` and `.revise.v1` carry rich definition
//! bodies the executor cannot run yet, and `run.cancel.v1` lacks the attempt
//! and runtime correlation the producer's cancellation requires.

use serde_json::{json, Map, Value};

use super::command_matrix::{refusal_message, refused_command};
use super::contract::error::ErrorCode;
use super::contract::types::{CommandName, CommandRequest};
use crate::control_plane::{
    automation_error, route_action_at, typed_rejection, validation_rejection, ActionStatus,
    ControlActionResponse,
};

pub const ACTION: &str = "coven.automations.command.v1";

const SCHEMA_VERSION: &str = "coven.automations.v1";

/// Mutations run through command adoption, so their adoption key is honoured.
/// Queries are read afresh on every request: their key is echoed, not stored.
fn adopted(command: CommandName) -> bool {
    matches!(
        command,
        CommandName::DefinitionActivate
            | CommandName::DefinitionPause
            | CommandName::DefinitionDisable
            | CommandName::DefinitionTombstone
            | CommandName::LegacyImport
    )
}

pub(crate) fn route(
    payload: &Value,
    conn: &rusqlite::Connection,
    runtime: &dyn crate::api::SessionRuntime,
    recorded_at: &str,
) -> (u16, ControlActionResponse) {
    let Some(object) = payload.as_object() else {
        return validation_rejection(ACTION, "request body must be a JSON object".to_owned());
    };
    if let Some(extra) = object
        .keys()
        .find(|key| key.as_str() != "action" && key.as_str() != "envelope")
    {
        return validation_rejection(
            ACTION,
            format!("{ACTION} accepts only `action` and `envelope`, not `{extra}`"),
        );
    }
    let Some(envelope) = object.get("envelope") else {
        return validation_rejection(ACTION, format!("{ACTION} requires field `envelope`"));
    };
    let request: CommandRequest = match serde_json::from_value(envelope.clone()) {
        Ok(request) => request,
        Err(error) => {
            return validation_rejection(
                ACTION,
                format!("`envelope` is not a valid coven.automations.v1 command: {error}"),
            )
        }
    };
    // The typed parse validated every field; read them back from the wire form.
    let command = request.command();
    let command_name = serde_json::to_value(command)
        .ok()
        .and_then(|value| value.as_str().map(ToOwned::to_owned))
        .expect("command names serialize as strings");
    let adoption_key = envelope["adoptionKey"].clone();
    let refuse = |code: ErrorCode, message: String| {
        rejected_response(
            &command_name,
            &adoption_key,
            typed_rejection(ACTION, automation_error(code, message)),
        )
    };

    let flat = match flat_request(&command_name, envelope, &request) {
        Ok(flat) => flat,
        Err(message) => return refuse(ErrorCode::CapabilityUnsupported, message),
    };
    let (status, inner) = route_action_at(flat, conn, runtime, recorded_at);
    if !inner.ok || inner.error.is_some() {
        return rejected_response(&command_name, &adoption_key, (status, inner));
    }

    let mut response = json!({
        "schemaVersion": SCHEMA_VERSION,
        "command": command_name,
        "adoptionKey": adoption_key,
    });
    if adopted(command) {
        // `automation_command_result` shape: outcome, revision, result, and
        // eventRef or replay when present.
        let Some(committed) = inner.result.clone() else {
            return refuse(
                ErrorCode::Internal,
                format!("{command_name} committed without a result"),
            );
        };
        response["outcome"] = committed["outcome"].clone();
        if committed["revision"].is_u64() {
            response["revision"] = committed["revision"].clone();
        }
        response["result"] = committed["result"].clone();
        for key in ["eventRef", "replay"] {
            if let Some(value) = committed.get(key) {
                response[key] = value.clone();
            }
        }
    } else {
        // Definition and run reads answer in the event payload; event pages
        // answer in `result`.
        let Some(page) = inner
            .event
            .as_ref()
            .map(|event| event.payload.clone())
            .or_else(|| inner.result.clone())
        else {
            return refuse(
                ErrorCode::Internal,
                format!("{command_name} read returned no payload"),
            );
        };
        response["outcome"] = json!("committed");
        response["result"] = page;
    }
    (
        status,
        ControlActionResponse {
            ok: true,
            accepted: true,
            action: ACTION.to_owned(),
            status: ActionStatus::Completed,
            reason: None,
            error: None,
            result: Some(response),
            event: inner.event,
        },
    )
}

/// A rejected spec response around the typed error the adapter produced.
fn rejected_response(
    command: &str,
    adoption_key: &Value,
    (status, inner): (u16, ControlActionResponse),
) -> (u16, ControlActionResponse) {
    let error = inner.error.clone();
    (
        status,
        ControlActionResponse {
            ok: false,
            accepted: false,
            action: ACTION.to_owned(),
            status: ActionStatus::Rejected,
            reason: inner.reason,
            result: Some(json!({
                "schemaVersion": SCHEMA_VERSION,
                "command": command,
                "adoptionKey": adoption_key,
                "outcome": "rejected",
                "error": error.clone().unwrap_or(Value::Null),
            })),
            error,
            event: None,
        },
    )
}

/// The flat control-action request for a validated envelope, or why the
/// producer cannot honour it.
fn flat_request(
    command: &str,
    envelope: &Value,
    request: &CommandRequest,
) -> Result<Value, String> {
    let payload = envelope["payload"].as_object().cloned().unwrap_or_default();
    let unsupported = |field: &str| -> Result<Value, String> {
        Err(format!(
            "`{command}` field `payload.{field}` is not implemented by this producer."
        ))
    };
    let mut flat = Map::new();
    flat.insert(
        "action".into(),
        json!(format!("coven.automations.{command}")),
    );
    // Recorded on the committed event only; neither grants authority.
    flat.insert("origin".into(), envelope["origin"]["channel"].clone());
    if let Some(correlation) = envelope["origin"].get("correlationId") {
        flat.insert("intentId".into(), correlation.clone());
    }
    match request.command() {
        CommandName::DefinitionActivate
        | CommandName::DefinitionPause
        | CommandName::DefinitionDisable
        | CommandName::DefinitionTombstone => {
            if request.command() == CommandName::DefinitionTombstone
                && payload.contains_key("reason")
            {
                return unsupported("reason");
            }
            flat.insert("adoptionKey".into(), envelope["adoptionKey"].clone());
            flat.insert(
                "expectedRevision".into(),
                envelope["expectedRevision"].clone(),
            );
            flat.insert("id".into(), payload["automationId"].clone());
            if let Some(reason) = payload.get("reason") {
                flat.insert("reason".into(), reason.clone());
            }
        }
        CommandName::LegacyImport => {
            flat.insert("adoptionKey".into(), envelope["adoptionKey"].clone());
            flat.insert("source".into(), payload["source"].clone());
            if let Some(dry_run) = payload.get("dryRun") {
                flat.insert("dryRun".into(), dry_run.clone());
            }
        }
        CommandName::DefinitionList => {
            for field in ["limit", "cursor"] {
                if payload.contains_key(field) {
                    return unsupported(field);
                }
            }
            let include_tombstoned = match payload.get("lifecycleState") {
                None => false,
                Some(state) if state == "all" => true,
                Some(_) => return unsupported("lifecycleState"),
            };
            flat.insert("includeTombstoned".into(), json!(include_tombstoned));
        }
        CommandName::DefinitionGet => {
            if payload.contains_key("revision") {
                return unsupported("revision");
            }
            flat.insert("id".into(), payload["automationId"].clone());
        }
        CommandName::DefinitionHealth => {
            flat.insert("id".into(), payload["automationId"].clone());
        }
        CommandName::RunHistory | CommandName::EventsRead | CommandName::EventsSubscribe => {
            // Field names and bounds match the flat actions.
            flat.extend(payload);
        }
        CommandName::DefinitionCreate | CommandName::DefinitionRevise => {
            return Err(format!(
                "`{command}` carries a rich definition body, which this producer cannot yet \
                 execute (coven#1054). Use `coven.automations.{command}` with a routine \
                 definition."
            ));
        }
        CommandName::RunCancel => {
            return Err(format!(
                "`{command}` over the envelope lacks the attempt and runtime correlation this \
                 producer's cancellation requires. Use `coven.automations.run.cancel.v1`."
            ));
        }
        _ => {
            return Err(
                refused_command(&format!("coven.automations.{command}")).map_or_else(
                    || format!("`{command}` is not available over the command envelope yet."),
                    refusal_message,
                ),
            );
        }
    }
    Ok(Value::Object(flat))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::NoopSessionRuntime;

    const SPEC_DIR: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../spec/coven-automations/v1"
    );

    /// Validates against the published `commandResponse` definition, resolving
    /// every sibling v1 schema offline.
    fn response_validator() -> jsonschema::Validator {
        let mut registry = jsonschema::Registry::new();
        let mut command_schema_id = String::new();
        for entry in std::fs::read_dir(SPEC_DIR).unwrap() {
            let path = entry.unwrap().path();
            if !path.to_string_lossy().ends_with(".schema.json") {
                continue;
            }
            let schema: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            let id = schema["$id"].as_str().unwrap().to_owned();
            if path.ends_with("command-envelope.schema.json") {
                command_schema_id = id.clone();
            }
            registry = registry.add(id, schema).unwrap();
        }
        let registry = registry.prepare().unwrap();
        jsonschema::draft202012::options()
            .with_registry(&registry)
            .offline()
            .build(&json!({ "$ref": format!("{command_schema_id}#/$defs/commandResponse") }))
            .unwrap()
    }

    fn store() -> (tempfile::TempDir, rusqlite::Connection) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("store.sqlite");
        crate::store::initialize_store(&path).unwrap();
        let conn = crate::store::open_store(&path).unwrap();
        let (status, response) = crate::control_plane::route_action(
            json!({
                "action": "coven.automations.definition.create.v1",
                "adoptionKey": "adopt:create:envelope-wire",
                "definition": {
                    "schemaVersion": 1, "id": "envelope-wire", "name": "Envelope wire",
                    "status": "PAUSED", "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc",
                    "misfire": "latest", "overlap": "forbid", "timeoutMinutes": 30,
                    "runtime": "coven-code", "prompt": "Envelope wire fixture."
                }
            }),
            &conn,
            &NoopSessionRuntime,
        );
        assert_eq!(status, 200, "{response:?}");
        (temp, conn)
    }

    fn envelope(command: &str, key: &str, expected_revision: Option<u64>, payload: Value) -> Value {
        let mut envelope = json!({
            "schemaVersion": "coven.automations.v1",
            "command": command,
            "adoptionKey": key,
            "origin": {
                "principal": { "principalId": "principal:owner" },
                "channel": "sdk",
                "correlationId": "corr-envelope-wire"
            },
            "intent": { "statement": "Exercise the command envelope." },
            "payload": payload
        });
        if let Some(revision) = expected_revision {
            envelope["expectedRevision"] = json!(revision);
        }
        envelope
    }

    fn send(conn: &rusqlite::Connection, envelope: Value) -> (u16, ControlActionResponse) {
        crate::control_plane::route_action(
            json!({ "action": ACTION, "envelope": envelope }),
            conn,
            &NoopSessionRuntime,
        )
    }

    fn event_count(conn: &rusqlite::Connection) -> i64 {
        conn.query_row("SELECT COUNT(*) FROM automation_events", [], |row| {
            row.get(0)
        })
        .unwrap()
    }

    #[test]
    fn lifecycle_commands_commit_replay_and_share_adoption_with_the_flat_wire() {
        let validator = response_validator();
        let (_temp, conn) = store();
        let activate = envelope(
            "definition.activate.v1",
            "adopt:envelope:activate",
            Some(1),
            json!({ "automationId": "envelope-wire" }),
        );

        let (status, response) = send(&conn, activate.clone());
        assert_eq!(status, 200, "{response:?}");
        let body = response.result.unwrap();
        assert!(validator.is_valid(&body), "{body}");
        assert_eq!(body["outcome"], "committed");
        assert_eq!(body["revision"], 2);
        assert_eq!(body["adoptionKey"], "adopt:envelope:activate");
        assert!(body["eventRef"]["sequence"].is_u64(), "{body}");
        let events = event_count(&conn);

        let (_, replay) = send(&conn, activate);
        let replay = replay.result.unwrap();
        assert!(validator.is_valid(&replay), "{replay}");
        assert_eq!(replay["outcome"], "replayed");
        assert!(replay["replay"]["firstCommittedAt"].is_string(), "{replay}");
        assert_eq!(event_count(&conn), events);

        // The same key and fields on the flat wire replay the envelope's commit.
        let (_, flat) = crate::control_plane::route_action(
            json!({
                "action": "coven.automations.definition.activate.v1",
                "adoptionKey": "adopt:envelope:activate",
                "id": "envelope-wire",
                "expectedRevision": 1,
            }),
            &conn,
            &NoopSessionRuntime,
        );
        assert_eq!(flat.result.unwrap()["outcome"], "replayed");

        let (_, paused) = send(
            &conn,
            envelope(
                "definition.pause.v1",
                "adopt:envelope:pause",
                Some(2),
                json!({ "automationId": "envelope-wire", "reason": "Freeze." }),
            ),
        );
        let paused = paused.result.unwrap();
        assert!(validator.is_valid(&paused), "{paused}");
        assert_eq!(
            (paused["outcome"].clone(), paused["revision"].clone()),
            (json!("committed"), json!(3))
        );

        let (status, stale) = send(
            &conn,
            envelope(
                "definition.disable.v1",
                "adopt:envelope:stale",
                Some(1),
                json!({ "automationId": "envelope-wire" }),
            ),
        );
        let stale = stale.result.unwrap();
        assert!(validator.is_valid(&stale), "{stale}");
        assert_eq!(status, 409);
        assert_eq!(stale["outcome"], "rejected");
        assert_eq!(stale["error"]["code"], "REVISION_CONFLICT");
    }

    #[test]
    fn queries_answer_with_committed_results() {
        let validator = response_validator();
        let (_temp, conn) = store();
        let cases = [
            (
                "definition.get.v1",
                json!({ "automationId": "envelope-wire" }),
            ),
            ("definition.list.v1", json!({})),
            ("definition.list.v1", json!({ "lifecycleState": "all" })),
            (
                "definition.health.v1",
                json!({ "automationId": "envelope-wire" }),
            ),
            (
                "run.history.v1",
                json!({ "automationId": "envelope-wire", "limit": 5 }),
            ),
            (
                "events.read.v1",
                json!({ "stream": { "kind": "automation", "id": "envelope-wire" } }),
            ),
        ];
        for (command, payload) in cases {
            let (status, response) = send(
                &conn,
                envelope(command, "adopt:envelope:query", None, payload),
            );
            assert_eq!(status, 200, "{command}: {response:?}");
            let body = response.result.unwrap();
            assert!(validator.is_valid(&body), "{command}: {body}");
            assert_eq!(body["outcome"], "committed", "{command}");
            assert!(body["result"].is_object(), "{command}: {body}");
        }
        let (_, get) = send(
            &conn,
            envelope(
                "definition.get.v1",
                "adopt:envelope:get",
                None,
                json!({ "automationId": "envelope-wire" }),
            ),
        );
        assert_eq!(
            get.result.unwrap()["result"]["routine"]["id"],
            "envelope-wire"
        );
    }

    #[test]
    fn unsupported_commands_and_fields_are_refused_without_writes() {
        let validator = response_validator();
        let (_temp, conn) = store();
        let vectors: Value = serde_json::from_str(
            &std::fs::read_to_string(format!("{SPEC_DIR}/test-vectors.json")).unwrap(),
        )
        .unwrap();
        let golden_create = vectors["fixtures"]["command.create.golden"].clone();
        let mut golden_revise = golden_create.clone();
        golden_revise["command"] = json!("definition.revise.v1");
        golden_revise["adoptionKey"] = json!("adopt:revise-daily-notes-0001");
        golden_revise["expectedRevision"] = json!(1);
        let refused = [
            golden_create,
            golden_revise,
            envelope(
                "run.cancel.v1",
                "adopt:envelope:cancel",
                None,
                json!({ "runId": "run-1" }),
            ),
            envelope(
                "attempt.retry.v1",
                "adopt:envelope:retry",
                None,
                json!({ "runId": "run-1", "priorAttemptNumber": 1, "priorDisposition": "failed" }),
            ),
            envelope(
                "definition.list.v1",
                "adopt:envelope:list",
                None,
                json!({ "limit": 5 }),
            ),
            envelope(
                "definition.list.v1",
                "adopt:envelope:list-state",
                None,
                json!({ "lifecycleState": "paused" }),
            ),
            envelope(
                "definition.get.v1",
                "adopt:envelope:get-revision",
                None,
                json!({ "automationId": "envelope-wire", "revision": 1 }),
            ),
            envelope(
                "definition.tombstone.v1",
                "adopt:envelope:tombstone-reason",
                Some(1),
                json!({ "automationId": "envelope-wire", "reason": "Gone." }),
            ),
        ];
        let revision = || -> i64 {
            conn.query_row(
                "SELECT revision FROM automation_definitions WHERE id = 'envelope-wire'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        let (before_revision, before_events) = (revision(), event_count(&conn));
        for request in refused {
            let command = request["command"].as_str().unwrap().to_owned();
            let (status, response) = send(&conn, request);
            let body = response.result.unwrap();
            assert!(validator.is_valid(&body), "{command}: {body}");
            assert_eq!(body["outcome"], "rejected", "{command}");
            assert_eq!(
                body["error"]["code"], "CAPABILITY_UNSUPPORTED",
                "{command}: {body}"
            );
            assert_eq!(status, body["error"]["httpStatus"].as_u64().unwrap() as u16);
        }
        assert_eq!(
            (revision(), event_count(&conn)),
            (before_revision, before_events)
        );
        let adoptions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_command_adoptions WHERE adoption_key LIKE 'adopt:envelope:%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(adoptions, 0);
    }

    #[test]
    fn malformed_envelopes_are_validation_failures() {
        let (_temp, conn) = store();
        let mut unknown_field = envelope(
            "definition.get.v1",
            "adopt:envelope:bad",
            None,
            json!({ "automationId": "envelope-wire" }),
        );
        unknown_field["extra"] = json!(true);
        let mut missing_revision = envelope(
            "definition.activate.v1",
            "adopt:envelope:bad-rev",
            None,
            json!({ "automationId": "envelope-wire" }),
        );
        missing_revision
            .as_object_mut()
            .unwrap()
            .remove("expectedRevision");
        for request in [
            json!({ "action": ACTION }),
            json!({ "action": ACTION, "envelope": unknown_field }),
            json!({ "action": ACTION, "envelope": missing_revision }),
            json!({ "action": ACTION, "envelope": envelope("definition.get.v1", "short", None, json!({ "automationId": "envelope-wire" })) }),
            json!({
                "action": ACTION,
                "envelope": envelope("definition.get.v1", "adopt:envelope:x", None, json!({ "automationId": "envelope-wire" })),
                "origin": "caller"
            }),
        ] {
            let (status, response) =
                crate::control_plane::route_action(request.clone(), &conn, &NoopSessionRuntime);
            assert_eq!(status, 400, "{request}");
            assert_eq!(
                response.error.unwrap()["code"],
                "VALIDATION_FAILED",
                "{request}"
            );
        }
    }
}
