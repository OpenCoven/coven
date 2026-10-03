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
    automation_command_result, automation_error, route_action_at, typed_rejection,
    validation_rejection, ActionStatus, ControlActionResponse,
};

pub const ACTION: &str = "coven.automations.command.v1";

const SCHEMA_VERSION: &str = "coven.automations.v1";

/// Mutations run through command adoption, so their adoption key is honoured.
/// Queries are read afresh on every request: their key is echoed, not stored.
fn adopted(command: CommandName) -> bool {
    matches!(
        command,
        CommandName::DefinitionCreate
            | CommandName::DefinitionRevise
            | CommandName::DefinitionActivate
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

    let (status, inner) = match command {
        // Rich definition bodies have no flat action: they go straight to
        // command adoption, which projects them onto the executable routine.
        CommandName::DefinitionCreate | CommandName::DefinitionRevise => {
            let definition = envelope["payload"]["definition"].clone();
            let rich = if command == CommandName::DefinitionCreate {
                super::command_adoption::DefinitionCommand::RichCreate { definition }
            } else {
                super::command_adoption::DefinitionCommand::RichRevise {
                    definition,
                    expected_revision: envelope["expectedRevision"].as_u64(),
                }
            };
            automation_command_result(
                &format!("coven.automations.{command_name}"),
                envelope["origin"]["channel"]
                    .as_str()
                    .map(ToOwned::to_owned),
                envelope["origin"]["correlationId"]
                    .as_str()
                    .map(ToOwned::to_owned),
                super::command_adoption::execute_definition_command(
                    conn,
                    adoption_key.as_str().unwrap_or_default(),
                    rich,
                    recorded_at,
                ),
            )
        }
        _ => match flat_request(&command_name, envelope, &request) {
            Ok(flat) => route_action_at(flat, conn, runtime, recorded_at),
            Err(message) => return refuse(ErrorCode::CapabilityUnsupported, message),
        },
    };
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

/// A rejected spec response around the adapter's error. The spec requires a
/// typed error on every rejection, so an untyped inner failure (a legacy-style
/// read reporting only a reason) becomes `INTERNAL`.
fn rejected_response(
    command: &str,
    adoption_key: &Value,
    (status, inner): (u16, ControlActionResponse),
) -> (u16, ControlActionResponse) {
    if inner.error.is_none() {
        let reason = inner
            .reason
            .clone()
            .unwrap_or_else(|| format!("{command} failed without a typed error"));
        return rejected_response(
            command,
            adoption_key,
            typed_rejection(ACTION, automation_error(ErrorCode::Internal, reason)),
        );
    }
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
                "error": error.clone(),
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
            return Err(format!("`{command}` has no flat translation."));
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
    fn event_subscriptions_answer_pages_and_typed_cursor_rejections() {
        let validator = response_validator();
        let (_temp, conn) = store();
        let stream = json!({ "kind": "automation", "id": "envelope-wire" });
        let (status, response) = send(
            &conn,
            envelope(
                "events.subscribe.v1",
                "adopt:envelope:subscribe",
                None,
                json!({ "stream": stream }),
            ),
        );
        assert_eq!(status, 200, "{response:?}");
        let page = response.result.unwrap();
        assert!(validator.is_valid(&page), "{page}");
        assert_eq!(page["outcome"], "committed");
        let checkpoint = page["result"]["checkpoint"].as_str().unwrap().to_owned();

        let (status, resumed) = send(
            &conn,
            envelope(
                "events.subscribe.v1",
                "adopt:envelope:subscribe-resume",
                None,
                json!({ "stream": stream, "checkpoint": checkpoint }),
            ),
        );
        assert_eq!(status, 200, "{resumed:?}");
        assert!(validator.is_valid(resumed.result.as_ref().unwrap()));

        let (status, unknown) = send(
            &conn,
            envelope(
                "events.subscribe.v1",
                "adopt:envelope:subscribe-unknown",
                None,
                json!({ "stream": stream, "checkpoint": format!("ecp{}", "0".repeat(32)) }),
            ),
        );
        let unknown = unknown.result.unwrap();
        assert!(validator.is_valid(&unknown), "{unknown}");
        assert_eq!(unknown["outcome"], "rejected");
        assert_ne!(status, 200);
        assert!(unknown["error"]["code"].is_string(), "{unknown}");
    }

    #[test]
    fn untyped_adapter_failures_become_typed_internal_rejections() {
        let validator = response_validator();
        let (_temp, conn) = store();
        conn.execute(
            "UPDATE automation_definitions SET definition_json = '{' WHERE id = 'envelope-wire'",
            [],
        )
        .unwrap();
        for (command, payload) in [
            (
                "definition.get.v1",
                json!({ "automationId": "envelope-wire" }),
            ),
            ("definition.list.v1", json!({})),
        ] {
            let (status, response) = send(
                &conn,
                envelope(command, "adopt:envelope:corrupt", None, payload),
            );
            let body = response.result.unwrap();
            assert!(validator.is_valid(&body), "{command}: {body}");
            assert_eq!(body["outcome"], "rejected", "{command}");
            assert_eq!(body["error"]["code"], "INTERNAL", "{command}: {body}");
            assert_eq!(status, 500, "{command}");
        }
    }

    /// The spec's golden definition reduced to the executable subset, at the
    /// given revision and lifecycle state, with its integrity recomputed.
    fn rich(revision: u64, lifecycle_state: &str, change: impl FnOnce(&mut Value)) -> Value {
        use crate::automations::contract::canonical_json::{
            canonicalize_without_integrity, sha256_hex,
        };
        let vectors: Value = serde_json::from_str(
            &std::fs::read_to_string(format!("{SPEC_DIR}/test-vectors.json")).unwrap(),
        )
        .unwrap();
        let mut definition = vectors["fixtures"]["definition.golden"].clone();
        let object = definition.as_object_mut().unwrap();
        object["policies"]
            .as_object_mut()
            .unwrap()
            .remove("delivery");
        object.remove("activation");
        // The capability profile advertises only standard retention.
        definition["policies"]["retention"]["receipts"] = json!({ "classification": "standard" });
        definition["automationId"] = json!("rich-notes");
        definition["revision"] = json!(revision);
        definition["lifecycleState"] = json!(lifecycle_state);
        change(&mut definition);
        let digest = sha256_hex(&canonicalize_without_integrity(&definition).unwrap());
        definition["integrity"]["value"] = json!(digest);
        definition
    }

    fn rich_command(command: &str, key: &str, expected: Option<u64>, definition: Value) -> Value {
        envelope(command, key, expected, json!({ "definition": definition }))
    }

    fn assert_verifies(definition: &Value) {
        let typed: crate::automations::contract::types::AutomationDefinition =
            serde_json::from_value(definition.clone()).unwrap();
        typed.verify_integrity().unwrap();
    }

    #[test]
    fn rich_definitions_create_revise_and_regenerate_through_the_lifecycle() {
        let validator = response_validator();
        let (_temp, conn) = store();
        let create = rich_command(
            "definition.create.v1",
            "adopt:rich:create",
            None,
            rich(1, "draft", |_| {}),
        );

        let (status, response) = send(&conn, create.clone());
        assert_eq!(status, 200, "{response:?}");
        let body = response.result.unwrap();
        assert!(validator.is_valid(&body), "{body}");
        assert_eq!(
            (body["outcome"].clone(), body["revision"].clone()),
            (json!("committed"), json!(1))
        );
        assert_eq!(body["result"]["definition"]["lifecycleState"], "draft");
        assert_verifies(&body["result"]["definition"]);

        // The executor runs the projected routine, paused as a draft.
        let record = crate::automations::store::get_definition(&conn, "rich-notes")
            .unwrap()
            .unwrap();
        assert_eq!(
            (record.status.as_str(), record.lifecycle_state.as_str()),
            ("PAUSED", "draft")
        );
        let routine: Value = serde_json::from_str(&record.definition_json).unwrap();
        for (key, expected) in [
            ("prompt", json!("Write the daily reflection.")),
            ("cwd", json!("~/projects/notes")),
            ("familiarId", json!("charm")),
            ("rrule", json!("FREQ=DAILY;BYHOUR=9")),
            ("timezone", json!("utc")),
            ("timeoutMinutes", json!(30)),
            ("runtime", json!("coven-code")),
            ("tags", json!(["notes", "daily"])),
        ] {
            assert_eq!(routine[key], expected, "{key}");
        }
        assert_eq!(routine["retry"]["maxAttempts"], 3);

        let (_, replay) = send(&conn, create);
        assert_eq!(replay.result.unwrap()["outcome"], "replayed");
        // The same key cannot also name a routine-bodied create.
        let (_, flat) = crate::control_plane::route_action(
            json!({
                "action": "coven.automations.definition.create.v1",
                "adoptionKey": "adopt:rich:create",
                "definition": routine,
            }),
            &conn,
            &NoopSessionRuntime,
        );
        assert_eq!(flat.error.unwrap()["code"], "ADOPTION_REPLAY_MISMATCH");

        // A draft revises only to paused, at exactly the next revision.
        let (_, to_active) = send(
            &conn,
            rich_command(
                "definition.revise.v1",
                "adopt:rich:revise-active",
                Some(1),
                rich(2, "active", |_| {}),
            ),
        );
        assert_eq!(
            to_active.result.unwrap()["error"]["code"],
            "ILLEGAL_TRANSITION"
        );
        let (_, skipped) = send(
            &conn,
            rich_command(
                "definition.revise.v1",
                "adopt:rich:revise-skip",
                Some(1),
                rich(3, "paused", |_| {}),
            ),
        );
        assert_eq!(
            skipped.result.unwrap()["error"]["code"],
            "VALIDATION_FAILED"
        );
        let (status, revised) = send(
            &conn,
            rich_command(
                "definition.revise.v1",
                "adopt:rich:revise",
                Some(1),
                rich(2, "paused", |definition| {
                    definition["action"]["prompt"] = json!("Reflect briefly.")
                }),
            ),
        );
        assert_eq!(status, 200, "{revised:?}");
        let revised = revised.result.unwrap();
        assert!(validator.is_valid(&revised), "{revised}");
        assert_eq!(revised["revision"], 2);

        // Activation is its own command; the rich view follows it.
        let (_, activated) = send(
            &conn,
            envelope(
                "definition.activate.v1",
                "adopt:rich:activate",
                Some(2),
                json!({ "automationId": "rich-notes" }),
            ),
        );
        assert_eq!(activated.result.unwrap()["revision"], 3);
        let (_, read) = send(
            &conn,
            envelope(
                "definition.get.v1",
                "adopt:rich:get",
                None,
                json!({ "automationId": "rich-notes" }),
            ),
        );
        let view = read.result.unwrap()["result"]["definition"].clone();
        assert_eq!(
            (view["revision"].clone(), view["lifecycleState"].clone()),
            (json!(3), json!("active"))
        );
        assert_eq!(view["action"]["prompt"], "Reflect briefly.");
        assert_verifies(&view);

        // A routine-bodied revise replaces the rich body.
        let mut routine: Value = serde_json::from_str(
            &crate::automations::store::get_definition(&conn, "rich-notes")
                .unwrap()
                .unwrap()
                .definition_json,
        )
        .unwrap();
        routine["prompt"] = json!("Plain routine now.");
        let (status, flat) = crate::control_plane::route_action(
            json!({
                "action": "coven.automations.definition.revise.v1",
                "adoptionKey": "adopt:rich:flat-revise",
                "expectedRevision": 3,
                "definition": routine,
            }),
            &conn,
            &NoopSessionRuntime,
        );
        assert_eq!(status, 200, "{flat:?}");
        let (_, read) = send(
            &conn,
            envelope(
                "definition.get.v1",
                "adopt:rich:get-flat",
                None,
                json!({ "automationId": "rich-notes" }),
            ),
        );
        assert!(read.result.unwrap()["result"].get("definition").is_none());

        // Every rich revision stays recoverable after later ones replace it.
        let history: Vec<(i64, String)> = conn
            .prepare(
                "SELECT revision, rich_definition_json FROM automation_rich_definition_revisions
                 WHERE automation_id = 'rich-notes' ORDER BY revision",
            )
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            history
                .iter()
                .map(|(revision, _)| *revision)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        let prompts = history
            .iter()
            .map(|(_, json)| {
                let snapshot: Value = serde_json::from_str(json).unwrap();
                assert_verifies(&snapshot);
                (
                    snapshot["lifecycleState"].clone(),
                    snapshot["action"]["prompt"].clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            prompts,
            vec![
                (json!("draft"), json!("Write the daily reflection.")),
                (json!("paused"), json!("Reflect briefly.")),
                (json!("active"), json!("Reflect briefly.")),
            ]
        );
    }

    /// What `rich_revision_history` committed.
    struct RichHistory {
        created: Value,
        revised: Value,
        /// The stored routine digest at revision 4, the scheduler's fence.
        snapshot_at_4: String,
    }

    /// Revisions 1-4 of `rich-notes` (create, revise, activate, pause) and a
    /// routine-bodied revision 5, which has no rich document.
    fn rich_revision_history(conn: &rusqlite::Connection) -> RichHistory {
        let (_, created) = send(
            conn,
            rich_command(
                "definition.create.v1",
                "adopt:digest:create",
                None,
                rich(1, "draft", |_| {}),
            ),
        );
        let (_, revised) = send(
            conn,
            rich_command(
                "definition.revise.v1",
                "adopt:digest:revise",
                Some(1),
                rich(2, "paused", |definition| {
                    definition["action"]["prompt"] = json!("Reflect briefly.")
                }),
            ),
        );
        for (command, key, expected) in [
            ("definition.activate.v1", "adopt:digest:activate", 2),
            ("definition.pause.v1", "adopt:digest:pause", 3),
        ] {
            let (status, response) = send(
                conn,
                envelope(
                    command,
                    key,
                    Some(expected),
                    json!({ "automationId": "rich-notes" }),
                ),
            );
            assert_eq!(status, 200, "{command}: {response:?}");
        }
        let record = crate::automations::store::get_definition(conn, "rich-notes")
            .unwrap()
            .unwrap();
        assert_eq!(record.revision, 4);
        let snapshot_at_4 = record.definition_digest.unwrap();
        let mut routine: Value = serde_json::from_str(&record.definition_json).unwrap();
        routine["prompt"] = json!("Plain routine now.");
        let (status, flat) = crate::control_plane::route_action(
            json!({
                "action": "coven.automations.definition.revise.v1",
                "adoptionKey": "adopt:digest:flat-revise",
                "expectedRevision": 4,
                "definition": routine,
            }),
            conn,
            &NoopSessionRuntime,
        );
        assert_eq!(status, 200, "{flat:?}");
        RichHistory {
            created: created.result.unwrap(),
            revised: revised.result.unwrap(),
            snapshot_at_4,
        }
    }

    fn recorded_integrity(conn: &rusqlite::Connection, revision: i64) -> String {
        conn.query_row(
            "SELECT integrity FROM automation_rich_definition_revisions
             WHERE automation_id = 'rich-notes' AND revision = ?1",
            [revision],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn rich_revisions_publish_their_document_digest_on_lifecycle_events() {
        let (_temp, conn) = store();
        let history = rich_revision_history(&conn);
        let published: Vec<(i64, String)> = conn
            .prepare(
                "SELECT event_json FROM automation_events
                 WHERE stream_kind = 'automation' AND stream_id = 'rich-notes'
                 ORDER BY sequence",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .map(|json| {
                let event: Value = serde_json::from_str(&json.unwrap()).unwrap();
                (
                    event["payload"]["revision"].as_i64().unwrap(),
                    event["payload"]["definitionDigest"]["value"]
                        .as_str()
                        .unwrap()
                        .to_owned(),
                )
            })
            .collect();
        assert_eq!(
            published
                .iter()
                .map(|(revision, _)| *revision)
                .collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5]
        );

        // Each richly authored revision publishes its recorded document's
        // integrity, which is what create and revise returned.
        for (revision, digest) in &published[..4] {
            assert_eq!(
                digest,
                &recorded_integrity(&conn, *revision),
                "revision {revision}"
            );
        }
        assert_eq!(
            published[0].1,
            history.created["result"]["definition"]["integrity"]["value"]
        );
        assert_eq!(
            published[1].1,
            history.revised["result"]["definition"]["integrity"]["value"]
        );
        // Revisions 2 and 4 are both paused with the same body, so their
        // routine projections match; their documents name their revision.
        assert_ne!(published[1].1, published[3].1);

        // Revision 5 has no rich document, so it publishes the routine digest,
        // and the stored fence is still the routine digest.
        let record = crate::automations::store::get_definition(&conn, "rich-notes")
            .unwrap()
            .unwrap();
        let snapshot =
            crate::automations::contract::migration::definition_digest(&record.definition_json)
                .unwrap();
        assert_eq!(record.definition_digest.as_deref(), Some(snapshot.as_str()));
        assert_eq!(published[4], (5, snapshot));
    }

    #[test]
    fn rich_revision_digest_reaches_occurrence_and_run_projections() {
        let (_temp, conn) = store();
        let history = rich_revision_history(&conn);
        let snapshot_at_5 = crate::automations::store::get_definition(&conn, "rich-notes")
            .unwrap()
            .unwrap()
            .definition_digest
            .unwrap();
        // Occurrences and runs store the snapshot digest the scheduler fenced
        // on: one at revision 4 (rich) and one at revision 5 (routine-bodied).
        let fences = [
            (4, history.snapshot_at_4.clone()),
            (5, snapshot_at_5.clone()),
        ];
        for (revision, digest) in &fences {
            conn.execute(
                "INSERT INTO automation_occurrences
                    (id, automation_id, automation_revision, definition_digest, scheduled_for,
                     kind, state, attempt, created_at, updated_at)
                 VALUES (?1, 'rich-notes', ?2, ?3, ?4, 'scheduled', 'succeeded', 1, ?4, ?4)",
                rusqlite::params![
                    format!("occurrence-{revision}"),
                    revision,
                    digest,
                    format!("2026-10-0{revision}T09:00:00.000Z"),
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO automation_runs
                    (id, automation_id, automation_revision, definition_digest, occurrence_id,
                     runtime, status, started_at)
                 VALUES (?1, 'rich-notes', ?2, ?3, ?4, 'coven-code', 'succeeded', ?5)",
                rusqlite::params![
                    format!("run-{revision}"),
                    revision,
                    digest,
                    format!("occurrence-{revision}"),
                    format!("2026-10-0{revision}T09:00:01.000Z"),
                ],
            )
            .unwrap();
        }
        // Migrated history that cannot be attributed to a revision keeps no
        // digest, even at a revision that has a rich document.
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for,
                 kind, state, attempt, created_at, updated_at)
             VALUES ('occurrence-unverifiable', 'rich-notes', 4, NULL,
                     '2026-09-01T09:00:00.000Z', 'scheduled', 'succeeded', 1,
                     '2026-09-01T09:00:00.000Z', '2026-09-01T09:00:00.000Z')",
            [],
        )
        .unwrap();
        let read = |request: Value| {
            let (status, response) =
                crate::control_plane::route_action(request, &conn, &NoopSessionRuntime);
            assert_eq!(status, 200, "{response:?}");
            response.event.unwrap().payload
        };
        let published = [(4, recorded_integrity(&conn, 4)), (5, snapshot_at_5)];
        assert_ne!(published[0].1, history.snapshot_at_4);
        for (revision, digest) in &published {
            let occurrence = read(json!({
                "action": "coven.automations.occurrence.get.v1",
                "id": format!("occurrence-{revision}"),
            }))["occurrence"]
                .clone();
            assert_eq!(
                occurrence["definitionDigest"],
                json!(digest),
                "revision {revision}"
            );
            assert_eq!(
                occurrence["runs"][0]["definitionDigest"],
                json!(digest),
                "revision {revision}"
            );
            let run = read(json!({
                "action": "coven.automations.run.get.v1",
                "id": format!("run-{revision}"),
            }))["run"]
                .clone();
            assert_eq!(
                run["definitionDigest"],
                json!(digest),
                "revision {revision}"
            );
        }
        let page = read(json!({
            "action": "coven.automations.occurrence.history.v1",
            "automationId": "rich-notes",
        }));
        assert_eq!(
            page["occurrences"]
                .as_array()
                .unwrap()
                .iter()
                .map(|occurrence| occurrence["definitionDigest"].clone())
                .collect::<Vec<_>>(),
            vec![json!(published[1].1), json!(published[0].1), Value::Null]
        );
        assert_eq!(
            read(json!({
                "action": "coven.automations.occurrence.get.v1",
                "id": "occurrence-unverifiable",
            }))["occurrence"]["definitionDigest"],
            Value::Null
        );
        // The rows themselves still carry the snapshot fence.
        let stored: Vec<Option<String>> = conn
            .prepare("SELECT definition_digest FROM automation_occurrences ORDER BY id")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(
            stored,
            vec![Some(fences[0].1.clone()), Some(fences[1].1.clone()), None]
        );
    }

    #[test]
    fn rich_creates_refuse_invalid_unsupported_and_unlawful_bodies_without_writes() {
        let validator = response_validator();
        let (_temp, conn) = store();
        let mut tampered = rich(1, "draft", |_| {});
        tampered["action"]["prompt"] = json!("Changed after signing.");
        // The typed envelope already verifies integrity, so a tampered body
        // never reaches adoption: a plain validation refusal, nothing stored.
        let (status, response) = send(
            &conn,
            rich_command(
                "definition.create.v1",
                "adopt:rich:tampered",
                None,
                tampered,
            ),
        );
        assert_eq!(status, 400);
        assert_eq!(response.error.unwrap()["code"], "VALIDATION_FAILED");
        let cases = [
            (
                "a revision other than 1",
                rich(2, "draft", |_| {}),
                "VALIDATION_FAILED",
            ),
            (
                "a non-draft state",
                rich(1, "paused", |_| {}),
                "ILLEGAL_TRANSITION",
            ),
            (
                "an activation window",
                rich(1, "draft", |definition| {
                    definition["activation"] =
                        json!({ "effectiveFrom": "2026-08-30T09:00:00.000Z" });
                }),
                "CAPABILITY_UNSUPPORTED",
            ),
            (
                "a delivery policy",
                rich(1, "draft", |definition| {
                    definition["policies"]["delivery"] =
                        json!({ "outputTarget": "~/notes/today.md", "mode": "atomic" });
                }),
                "CAPABILITY_UNSUPPORTED",
            ),
            (
                "an extended retention class",
                rich(1, "draft", |definition| {
                    definition["policies"]["retention"]["receipts"] =
                        json!({ "classification": "extended" });
                }),
                "CAPABILITY_UNSUPPORTED",
            ),
        ];
        for (index, (label, definition, code)) in cases.into_iter().enumerate() {
            let (_, response) = send(
                &conn,
                rich_command(
                    "definition.create.v1",
                    &format!("adopt:rich:refuse:{index}"),
                    None,
                    definition,
                ),
            );
            let body = response
                .result
                .clone()
                .unwrap_or_else(|| panic!("{label}: {:?}", response.reason));
            assert!(validator.is_valid(&body), "{label}: {body}");
            assert_eq!(body["error"]["code"], code, "{label}: {body}");
        }
        assert!(
            crate::automations::store::get_definition(&conn, "rich-notes")
                .unwrap()
                .is_none()
        );
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
