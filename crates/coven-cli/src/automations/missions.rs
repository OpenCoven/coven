//! Missions (#1313): one owner-submitted, one-shot invocation of an exact
//! familiar revision on one project, as a thin layer over one manual
//! occurrence. The contract is `docs/architecture/coven-automations-missions.md`.
//!
//! A submit commits, in one transaction, the mission's internal definition
//! (stored disabled, and never planned or listed because it belongs to a
//! mission), its claimed occurrence, the mission row and the adoption. The
//! mission row is also the one-run owner grant: it binds the owner-local
//! principal to this adoption key and request digest. The `automation_missions`
//! table is created with the definitions table
//! (`store::AUTOMATION_DEFINITIONS_SCHEMA_SQL`), so every store that can hold a
//! definition can tell a mission's from a routine.
//!
//! A submit may name `scopes`: profile filesystem read scopes on the project
//! root (#1597). They are validated by the scoped read service's own rules,
//! stored canonically on the mission row and returned by its view. Nothing
//! reads them yet.
//!
//! M0 launches nothing. The only dispatch path launches with ambient
//! authority, and Runtime Authority, which would hold the run to the R0 read
//! envelope, is not constructed yet (#857). So once the submit has committed,
//! the mission's occurrence settles failed with
//! [`AUTHORITY_UNAVAILABLE_REASON`], which the mission view reports as
//! `unavailable`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde_json::{json, Value};

use super::cancel_commands::{
    execute_occurrence_cancel, CancelExecution, OCCURRENCE_CANCEL_ACTION,
};
use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::error::{AdoptionConflictOutcome, ErrorAdoption, ErrorCode, ErrorEnvelope};
use super::contract::types::{AdoptionKey, MissionCancelPayload, MissionId, MissionSubmitPayload};
use super::occurrences::{insert_claimed_occurrence, settle_occurrence};
use super::owner_grants::CommandAuthority;
use super::runtime_envelope::CLAUDE_R0_READ;
use super::scoped_read::scope::Grants;
use super::scoped_read::R0_READ_POLICY;
use crate::api::SessionRuntime;

pub const SUBMIT_ACTION: &str = "coven.automations.mission.submit.v1";
pub const GET_ACTION: &str = "coven.automations.mission.get.v1";
pub const CANCEL_ACTION: &str = "coven.automations.mission.cancel.v1";
const SUBMIT_COMMAND: &str = "mission.submit.v1";
const CANCEL_COMMAND: &str = "mission.cancel.v1";
/// Prefix of the `occurrence.cancel.v1` key a mission cancel derives. The view
/// reads the mission's cancellation back from that adoption.
const CANCELLATION_KEY_PREFIX: &str = "mc:";
/// Dispatch must begin before `validUntil`, at most this long after the request.
const MAX_VALIDITY_MINUTES: i64 = 15;
/// The failure reason that marks a mission refused for want of Runtime
/// Authority. The mission view projects it to `unavailable`.
pub(crate) const AUTHORITY_UNAVAILABLE_REASON: &str =
    "mission not launched: runtime authority is not available on this daemon";
/// The scheduler's reason for a claim whose lease ran out before dispatch.
const LEASE_EXPIRED_REASON: &str = "lease expired";
/// The most filesystem scopes one mission may name (the schema's `maxItems`).
pub(crate) const MAX_MISSION_SCOPES: usize = 32;

/// The answer to one mission command.
#[derive(Debug, Clone, PartialEq)]
pub enum MissionExecution {
    Success { payload: Value, replayed: bool },
    Rejected(ErrorEnvelope),
}

/// `mission-` and the first 32 hex digits of sha256(adoptionKey): a client
/// can derive it before the submit's response arrives.
#[must_use]
pub fn mission_id_for(adoption_key: &str) -> String {
    format!("mission-{}", &sha256_hex(adoption_key.as_bytes())[..32])
}

/// Executes one `mission.submit.v1` request at `now`. The body is the flat
/// action: `action`, `adoptionKey` and the payload's fields.
pub fn execute_mission_submit(
    conn: &Connection,
    authority: CommandAuthority,
    body: Value,
    now: DateTime<Utc>,
) -> Result<MissionExecution, String> {
    let (adoption_key, payload, digest) = match owner_request(authority, &body, SUBMIT_COMMAND)? {
        Ok(request) => request,
        Err(refusal) => return Ok(refusal),
    };
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|error| format!("failed to begin mission.submit: {error}"))?;
    if let Some(answer) = adopted(&transaction, SUBMIT_COMMAND, &adoption_key, &digest)? {
        transaction
            .commit()
            .map_err(|error| format!("failed to close mission.submit replay: {error}"))?;
        return Ok(answer);
    }
    let decided = match serde_json::from_value::<MissionSubmitPayload>(payload) {
        Ok(request) => claim_in(&transaction, &adoption_key, &digest, &request, now)?,
        Err(error) => Err(typed_error(
            ErrorCode::ValidationFailed,
            format!("invalid mission.submit.v1 payload: {error}"),
        )),
    };
    adopt(
        &transaction,
        SUBMIT_COMMAND,
        &adoption_key,
        &mission_id_for(&adoption_key),
        &digest,
        &decided,
        now,
    )?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit mission.submit: {error}"))?;
    let result = match decided {
        Ok(result) => result,
        Err(error) => return Ok(MissionExecution::Rejected(error)),
    };

    // M0: nothing launches without Runtime Authority (see the module docs).
    // The answer is the state at commit; mission.get reports the settlement.
    let occurrence_id = result["occurrenceId"]
        .as_str()
        .ok_or("committed mission has no occurrence")?;
    settle_occurrence(
        conn,
        occurrence_id,
        "failed",
        Some(AUTHORITY_UNAVAILABLE_REASON),
        now,
    )?;
    Ok(MissionExecution::Success {
        payload: result,
        replayed: false,
    })
}

/// Executes one `mission.cancel.v1` request at `now`. Before dispatch it is
/// `occurrence.cancel.v1` on the mission's occurrence; after dispatch that
/// command hands it to `run.cancel.v1` and its stop fence. The inner command
/// runs under a key derived from this one, so a replay after a crash between
/// the two adoptions repeats the same cancellation instead of starting another.
pub fn execute_mission_cancel(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    authority: CommandAuthority,
    body: Value,
    now: DateTime<Utc>,
) -> Result<MissionExecution, String> {
    let (adoption_key, payload, digest) = match owner_request(authority, &body, CANCEL_COMMAND)? {
        Ok(request) => request,
        Err(refusal) => return Ok(refusal),
    };
    if let Some(answer) = adopted(conn, CANCEL_COMMAND, &adoption_key, &digest)? {
        return Ok(answer);
    }
    let (mission_id, decided) = match serde_json::from_value::<MissionCancelPayload>(payload) {
        Ok(request) => (
            request.mission_id.as_str().to_owned(),
            cancel(conn, runtime, &adoption_key, &request, now)?,
        ),
        Err(error) => (
            String::new(),
            Err(typed_error(
                ErrorCode::ValidationFailed,
                format!("invalid mission.cancel.v1 payload: {error}"),
            )),
        ),
    };
    let transaction = rusqlite::Transaction::new_unchecked(conn, TransactionBehavior::Immediate)
        .map_err(|error| format!("failed to begin mission.cancel: {error}"))?;
    // The same key may have been adopted while the cancellation ran.
    if let Some(answer) = adopted(&transaction, CANCEL_COMMAND, &adoption_key, &digest)? {
        transaction
            .commit()
            .map_err(|error| format!("failed to close mission.cancel replay: {error}"))?;
        return Ok(answer);
    }
    adopt(
        &transaction,
        CANCEL_COMMAND,
        &adoption_key,
        &mission_id,
        &digest,
        &decided,
        now,
    )?;
    transaction
        .commit()
        .map_err(|error| format!("failed to commit mission.cancel: {error}"))?;
    Ok(match decided {
        Ok(payload) => MissionExecution::Success {
            payload,
            replayed: false,
        },
        Err(error) => MissionExecution::Rejected(error),
    })
}

/// Cancels the mission's occurrence, or says why it cannot be.
fn cancel(
    conn: &Connection,
    runtime: &dyn SessionRuntime,
    adoption_key: &str,
    request: &MissionCancelPayload,
    now: DateTime<Utc>,
) -> Result<Result<Value, ErrorEnvelope>, String> {
    let mission_id = request.mission_id.as_str();
    let Some(view) = mission_view(conn, mission_id)? else {
        return Ok(Err(typed_error(
            ErrorCode::NotFound,
            format!("no mission `{mission_id}`"),
        )));
    };
    if view["terminal"] == json!(true) {
        return Ok(Err(nothing_to_cancel(view)));
    }
    let cancellation_id = format!(
        "{CANCELLATION_KEY_PREFIX}{}",
        &sha256_hex(adoption_key.as_bytes())[..40]
    );
    let mut inner = json!({
        "action": OCCURRENCE_CANCEL_ACTION,
        "adoptionKey": cancellation_id,
        "occurrenceId": view["occurrenceId"],
    });
    if let Some(reason) = &request.reason {
        inner["reason"] = json!(reason.as_str());
    }
    if let CancelExecution::Rejected(error) = execute_occurrence_cancel(conn, runtime, inner, now)?
    {
        // The mission may have settled since it was read.
        return Ok(Err(
            match mission_view(conn, mission_id)?.filter(|view| view["terminal"] == json!(true)) {
                Some(view) => nothing_to_cancel(view),
                None => error,
            },
        ));
    }
    let view = mission_view(conn, mission_id)?.ok_or("the cancelled mission is not readable")?;
    Ok(Ok(json!({
        "cancellation": view["cancellation"].clone(),
        "mission": view,
    })))
}

/// `ILLEGAL_TRANSITION` for a mission that has already settled, with the view
/// it settled in.
fn nothing_to_cancel(view: Value) -> ErrorEnvelope {
    typed_error(
        ErrorCode::IllegalTransition,
        format!(
            "the mission is `{}`, with nothing left to cancel",
            view["state"].as_str().unwrap_or("settled")
        ),
    )
    .with_details(BTreeMap::from([("mission".to_owned(), view)]))
}

/// Executes one `mission.get.v1` read: the mission's current view.
pub fn execute_mission_get(
    conn: &Connection,
    authority: CommandAuthority,
    body: &Value,
) -> Result<MissionExecution, String> {
    let not_found = |mission_id: &str| {
        Ok(MissionExecution::Rejected(typed_error(
            ErrorCode::NotFound,
            format!("no mission `{mission_id}`"),
        )))
    };
    if let Some(extra) = body.as_object().and_then(|object| {
        object
            .keys()
            .find(|key| key.as_str() != "missionId" && !ROUTING_KEYS.contains(&key.as_str()))
    }) {
        return Ok(MissionExecution::Rejected(typed_error(
            ErrorCode::ValidationFailed,
            format!("mission.get.v1 does not accept `{extra}`"),
        )));
    }
    let Some(mission_id) = body
        .get("missionId")
        .and_then(Value::as_str)
        .filter(|id| MissionId::new((*id).to_owned()).is_ok())
    else {
        return Ok(MissionExecution::Rejected(typed_error(
            ErrorCode::ValidationFailed,
            "missionId must be `mission-` followed by 32 lowercase hex digits",
        )));
    };
    // A mission belongs to the owner. Any other caller learns nothing, not
    // even that the mission exists.
    if authority != CommandAuthority::OwnerLocal {
        return not_found(mission_id);
    }
    match mission_view(conn, mission_id)? {
        Some(view) => Ok(MissionExecution::Success {
            payload: view,
            replayed: false,
        }),
        None => not_found(mission_id),
    }
}

/// The owner's request: its adoption key, its payload and the digest that
/// adoption compares. Any other caller, or a missing key, is refused before
/// anything is read or adopted.
fn owner_request(
    authority: CommandAuthority,
    body: &Value,
    command: &str,
) -> Result<Result<(String, Value, String), MissionExecution>, String> {
    if authority != CommandAuthority::OwnerLocal {
        return Ok(Err(MissionExecution::Rejected(typed_error(
            ErrorCode::AuthorityRequired,
            "missions are owner-local: send mission commands over the daemon's owner socket",
        ))));
    }
    let Some(adoption_key) = body
        .get("adoptionKey")
        .and_then(Value::as_str)
        .filter(|key| AdoptionKey::new((*key).to_owned()).is_ok())
        .map(ToOwned::to_owned)
    else {
        return Ok(Err(MissionExecution::Rejected(typed_error(
            ErrorCode::ValidationFailed,
            "adoptionKey is required and must be a valid adoption key",
        ))));
    };
    let payload = flat_payload(body);
    // The digest covers the command and its payload, not the wire form, so a
    // flat request and an envelope with the same payload are the same request.
    let digest = sha256_hex(
        &canonicalize(&json!({ "command": command, "payload": payload }))
            .map_err(|error| format!("failed to canonicalize {command}: {error:#}"))?,
    );
    Ok(Ok((adoption_key, payload, digest)))
}

/// Records the command's answer under its adoption key.
fn adopt(
    conn: &Connection,
    command: &str,
    adoption_key: &str,
    mission_id: &str,
    digest: &str,
    decided: &Result<Value, ErrorEnvelope>,
    now: DateTime<Utc>,
) -> Result<(), String> {
    let stored = match decided {
        Ok(result) => json!({ "outcome": "committed", "result": result }),
        Err(error) => json!({ "outcome": "rejected", "error": error }),
    };
    conn.execute(
        "INSERT INTO automation_command_adoptions (
            adoption_key, command, automation_id, request_digest, outcome,
            revision, response_json, adopted_at
         ) VALUES (?1, ?2, NULLIF(?3, ''), ?4, ?5, NULL, ?6, ?7)",
        params![
            adoption_key,
            command,
            mission_id,
            digest,
            stored["outcome"].as_str(),
            stored.to_string(),
            iso(now)
        ],
    )
    .map_err(|error| format!("failed to adopt {command}: {error}"))?;
    Ok(())
}

/// The keys a flat request carries for routing and correlation, not as
/// payload.
const ROUTING_KEYS: [&str; 4] = ["action", "adoptionKey", "origin", "intentId"];

/// The payload fields of a flat request: everything but the routing keys.
fn flat_payload(body: &Value) -> Value {
    let mut payload = body.as_object().cloned().unwrap_or_default();
    for key in ROUTING_KEYS {
        payload.remove(key);
    }
    Value::Object(payload)
}

/// A replay, a mismatch, or `None` when the key is new.
fn adopted(
    conn: &Connection,
    command_name: &str,
    adoption_key: &str,
    digest: &str,
) -> Result<Option<MissionExecution>, String> {
    let row: Option<(String, String, String, String)> = conn
        .query_row(
            "SELECT command, request_digest, outcome, response_json
             FROM automation_command_adoptions WHERE adoption_key = ?1",
            [adoption_key],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .optional()
        .map_err(|error| format!("failed to read the mission adoption: {error}"))?;
    let Some((command, stored_digest, outcome, response_json)) = row else {
        let used_elsewhere =
            super::command_adoption::attempt_adoption_key_exists(conn, adoption_key)
                .map_err(|error| format!("failed to inspect attempt adoption keys: {error:#}"))?
                || conn
                    .query_row(
                        "SELECT 1 FROM automation_command_reservations WHERE adoption_key = ?1",
                        [adoption_key],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(|error| format!("failed to inspect command reservations: {error}"))?
                    .is_some();
        return Ok(
            used_elsewhere.then(|| MissionExecution::Rejected(replay_mismatch(adoption_key, None)))
        );
    };
    if command != command_name || stored_digest != digest {
        let conflict = match outcome.as_str() {
            "committed" => Some(AdoptionConflictOutcome::Committed),
            "rejected" => Some(AdoptionConflictOutcome::Rejected),
            _ => None,
        };
        return Ok(Some(MissionExecution::Rejected(replay_mismatch(
            adoption_key,
            conflict,
        ))));
    }
    let response: Value = serde_json::from_str(&response_json)
        .map_err(|error| format!("stored mission response is invalid: {error}"))?;
    Ok(Some(match outcome.as_str() {
        // The first commit's result, unchanged: a replay never launches or
        // cancels again.
        "committed" => MissionExecution::Success {
            payload: response["result"].clone(),
            replayed: true,
        },
        "rejected" => MissionExecution::Rejected(
            serde_json::from_value(response["error"].clone())
                .map_err(|error| format!("stored mission error is invalid: {error}"))?,
        ),
        other => return Err(format!("stored mission outcome {other:?} is invalid")),
    }))
}

/// Validates the submit against the store and, when it may proceed, writes
/// the mission inside the caller's transaction. Returns the committed result
/// or the refusal; an `Err` is an internal failure.
fn claim_in(
    conn: &Connection,
    adoption_key: &str,
    digest: &str,
    request: &MissionSubmitPayload,
    now: DateTime<Utc>,
) -> Result<Result<Value, ErrorEnvelope>, String> {
    let refuse = |code, message: String| Ok(Err(typed_error(code, message)));

    let Ok(valid_until) = DateTime::parse_from_rfc3339(request.valid_until.as_str()) else {
        return refuse(
            ErrorCode::ValidationFailed,
            "validUntil is not an RFC 3339 instant".to_owned(),
        );
    };
    let valid_until = valid_until.with_timezone(&Utc);
    if valid_until <= now {
        return refuse(
            ErrorCode::ValidationFailed,
            "validUntil has already passed".to_owned(),
        );
    }
    if valid_until > now + Duration::minutes(MAX_VALIDITY_MINUTES) {
        return refuse(
            ErrorCode::ValidationFailed,
            format!("validUntil must be at most {MAX_VALIDITY_MINUTES} minutes after the request"),
        );
    }

    let supported_digest = CLAUDE_R0_READ.descriptor_digest();
    if request.envelope.id.as_str() != CLAUDE_R0_READ.descriptor_version
        || request.envelope.descriptor_digest.value.as_str() != supported_digest
    {
        return Ok(Err(unsupported_envelope(request.envelope.id.as_str())));
    }

    let scopes_json = match request.scopes.as_deref().map(validated_scopes).transpose() {
        Ok(scopes) => scopes,
        Err(message) => return refuse(ErrorCode::ValidationFailed, message),
    };

    let cwd = match canonical_directory(request.project.cwd.as_str()) {
        Ok(cwd) => cwd,
        Err(message) => return refuse(ErrorCode::ValidationFailed, message),
    };
    if store_home(conn).is_some_and(|home| cwd.starts_with(home)) {
        return refuse(
            ErrorCode::ValidationFailed,
            "project.cwd is inside COVEN_HOME; choose a project directory".to_owned(),
        );
    }
    let cwd_text = cwd.display().to_string();

    let root_id = request.familiar.root_id.as_str();
    let roster_id = match familiar_head(conn, root_id)? {
        None => return refuse(ErrorCode::NotFound, format!("no familiar root `{root_id}`")),
        Some(head) if head.retired || head.status != "active" => {
            return refuse(
                ErrorCode::RevisionConflict,
                format!("familiar root `{root_id}` has no active revision"),
            );
        }
        Some(head) if head.revision_id != request.familiar.revision_id.as_str() => {
            return Ok(Err(typed_error(
                ErrorCode::RevisionConflict,
                format!(
                    "familiar revision `{}` is not the current active revision of `{root_id}`",
                    request.familiar.revision_id.as_str()
                ),
            )
            .with_details(BTreeMap::from([(
                "currentRevisionId".to_owned(),
                json!(head.revision_id),
            )]))));
        }
        Some(head) => head.roster_id,
    };

    if active_mission_in_scope(conn, root_id, &cwd_text)? {
        return refuse(
            ErrorCode::ConcurrencyLimit,
            "this familiar already has an active mission in this project".to_owned(),
        );
    }

    let submitted_at = iso(now);
    let principal_id = super::owner_grants::owner_principal_id(conn, &submitted_at)
        .map_err(|error| format!("failed to read the owner principal: {error:#}"))?;
    let mission_id = mission_id_for(adoption_key);
    let occurrence_id = format!("occ-{mission_id}");
    let timeout_minutes = request.limits.timeout_minutes.get();
    let timeout_column = i64::try_from(timeout_minutes)
        .map_err(|_| "mission timeout does not fit the store".to_owned())?;
    let definition = super::definition::RoutineDefinition::from_json(&json!({
        "schemaVersion": 1,
        "id": mission_id,
        "name": mission_id,
        // Never ACTIVE, so the planner, activation grants and runNow all pass it by.
        "status": "DISABLED",
        "rrule": "FREQ=DAILY;BYHOUR=0",
        "timezone": "utc",
        "misfire": "latest",
        "overlap": "forbid",
        "timeoutMinutes": timeout_minutes,
        "runtime": CLAUDE_R0_READ.harness,
        "familiarId": roster_id,
        "cwd": cwd_text,
        "prompt": request.task.prompt.as_str(),
    }))
    .map_err(|error| format!("mission definition is invalid: {error}"))?;
    super::store::insert_definition_at(conn, &definition, &submitted_at)
        .map_err(|error| format!("failed to record the mission's definition: {error:#}"))?;
    let lease_minutes = ((valid_until - now).num_seconds() + 59) / 60;
    if !insert_claimed_occurrence(
        conn,
        &occurrence_id,
        &mission_id,
        "manual",
        lease_minutes.clamp(1, MAX_VALIDITY_MINUTES),
        now,
    )? {
        return Err(format!(
            "mission occurrence `{occurrence_id}` could not be claimed"
        ));
    }
    conn.execute(
        "INSERT INTO automation_missions (
            mission_id, automation_id, occurrence_id, adoption_key, request_digest,
            authority, principal_id, familiar_root_id, familiar_revision_id, envelope_id,
            envelope_descriptor_digest, project_cwd, statement, timeout_minutes,
            submitted_at, valid_until, scopes_json
         ) VALUES (?1, ?1, ?2, ?3, ?4, 'owner_local', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
                   ?15)",
        params![
            mission_id,
            occurrence_id,
            adoption_key,
            digest,
            principal_id,
            root_id,
            request.familiar.revision_id.as_str(),
            request.envelope.id.as_str(),
            supported_digest,
            cwd_text,
            request.task.statement.as_str(),
            timeout_column,
            submitted_at,
            iso(valid_until),
            scopes_json,
        ],
    )
    .map_err(|error| format!("failed to record the mission: {error}"))?;
    let view = mission_view(conn, &mission_id)?.ok_or("the recorded mission is not readable")?;
    Ok(Ok(json!({
        "missionId": mission_id,
        "occurrenceId": occurrence_id,
        "mission": view,
    })))
}

/// The canonical JSON of a mission's scopes, or why they are refused. Every
/// scope must be one the scoped read service would enforce as written
/// ([`Grants::from_scopes`] under the R0 read policy, the same rules the read
/// service applies), and also a read of the project root: a mission binds no
/// workspace root and is never granted a write. Absent scopes mean none, so a
/// present list must name at least one.
fn validated_scopes(scopes: &[Value]) -> Result<String, String> {
    if scopes.is_empty() {
        return Err("scopes must name at least one filesystem scope; omit it for none".to_owned());
    }
    if scopes.len() > MAX_MISSION_SCOPES {
        return Err(format!(
            "scopes may name at most {MAX_MISSION_SCOPES} filesystem scopes"
        ));
    }
    for (index, scope) in scopes.iter().enumerate() {
        if let Err(refusal) = Grants::from_scopes(std::slice::from_ref(scope), &R0_READ_POLICY) {
            return Err(format!(
                "scopes[{index}] is not a scope the read service can enforce ({}): it must be a \
                 closed filesystem scope whose path is relative, not root-wide, and free of `..`, \
                 globs and spellings the service does not interpret",
                refusal.code()
            ));
        }
        let field = |name: &str| scope.get(name).and_then(Value::as_str);
        if field("kind") != Some("filesystem") {
            return Err(format!("scopes[{index}] must be a `filesystem` scope"));
        }
        if field("root") != Some("project") {
            return Err(format!("scopes[{index}] must be rooted at `project`"));
        }
        if field("access") != Some("read") {
            return Err(format!("scopes[{index}] must grant `read` access"));
        }
    }
    let canonical = canonicalize(scopes)
        .map_err(|error| format!("scopes cannot be canonicalized: {error:#}"))?;
    String::from_utf8(canonical).map_err(|_| "scopes are not valid UTF-8".to_owned())
}

struct FamiliarHead {
    roster_id: String,
    retired: bool,
    revision_id: String,
    status: String,
}

/// The root's head revision: the one at its highest lineage position.
fn familiar_head(conn: &Connection, root_id: &str) -> Result<Option<FamiliarHead>, String> {
    crate::familiar_ledger::ensure_familiar_ledger_schema(conn)
        .map_err(|error| format!("failed to open the familiar ledger: {error:#}"))?;
    conn.query_row(
        "SELECT root.roster_id, root.retired_at IS NOT NULL, head.revision_id, head.status
         FROM familiar_ledger_roots AS root
         JOIN familiar_ledger_revisions AS head ON head.root_id = root.root_id
         WHERE root.root_id = ?1
         ORDER BY head.lineage_position DESC
         LIMIT 1",
        [root_id],
        |row| {
            Ok(FamiliarHead {
                roster_id: row.get(0)?,
                retired: row.get(1)?,
                revision_id: row.get(2)?,
                status: row.get(3)?,
            })
        },
    )
    .optional()
    .map_err(|error| format!("failed to read familiar root `{root_id}`: {error}"))
}

fn active_mission_in_scope(conn: &Connection, root_id: &str, cwd: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT 1 FROM automation_missions AS mission
         JOIN automation_occurrences AS occurrence ON occurrence.id = mission.occurrence_id
         WHERE mission.familiar_root_id = ?1 AND mission.project_cwd = ?2
           AND occurrence.state NOT IN ('succeeded', 'failed', 'cancelled', 'timed_out',
                                        'skipped', 'superseded')
         LIMIT 1",
        params![root_id, cwd],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .map_err(|error| format!("failed to read active missions: {error}"))
}

/// The mission as its records stand, or `None` when there is no such mission.
pub(crate) fn mission_view(conn: &Connection, mission_id: &str) -> Result<Option<Value>, String> {
    let row = conn
        .query_row(
            "SELECT mission.occurrence_id, mission.principal_id, mission.familiar_root_id,
                    mission.familiar_revision_id, mission.envelope_id,
                    mission.envelope_descriptor_digest, mission.project_cwd, mission.statement,
                    mission.submitted_at, mission.valid_until,
                    occurrence.state, occurrence.failure_reason, occurrence.updated_at,
                    mission.scopes_json
             FROM automation_missions AS mission
             JOIN automation_occurrences AS occurrence ON occurrence.id = mission.occurrence_id
             WHERE mission.mission_id = ?1",
            [mission_id],
            |row| {
                Ok((
                    [
                        row.get::<_, String>(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                    ],
                    row.get::<_, Option<String>>(11)?,
                    row.get::<_, String>(12)?,
                    row.get::<_, Option<String>>(13)?,
                ))
            },
        )
        .optional()
        .map_err(|error| format!("failed to read mission `{mission_id}`: {error}"))?;
    let Some((
        [occurrence_id, principal_id, root_id, revision_id, envelope_id, envelope_digest, cwd, statement, submitted_at, valid_until, occurrence_state],
        failure_reason,
        settled_at,
        scopes_json,
    )) = row
    else {
        return Ok(None);
    };
    let (state, reason) = project(&occurrence_state, failure_reason.as_deref());
    let cancellation = cancellation(conn, mission_id, &occurrence_state)?;
    // A stop the owner asked for that has not landed yet.
    let state = if state == "running"
        && cancellation
            .as_ref()
            .is_some_and(|cancellation| cancellation["outcome"] == "pending")
    {
        "cancelling"
    } else {
        state
    };
    let terminal = matches!(
        state,
        "completed" | "failed" | "cancelled" | "rejected" | "revoked" | "unavailable"
    );
    // Every state change appends an occurrence event, so the stream head is
    // the view's revision.
    let revision: i64 = conn
        .query_row(
            "SELECT next_sequence FROM automation_event_stream_heads
             WHERE stream_kind = 'occurrence' AND stream_id = ?1",
            [&occurrence_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("failed to read the mission's revision: {error}"))?
        .unwrap_or(1)
        .max(1);
    let revision = u64::try_from(revision).unwrap_or(1);
    let mut attempts = Vec::new();
    {
        let mut statement_rows = conn
            .prepare(
                "SELECT id, attempt_number, state, failure_class FROM automation_attempts
                 WHERE occurrence_id = ?1 ORDER BY attempt_number",
            )
            .map_err(|error| format!("failed to read the mission's attempts: {error}"))?;
        let rows = statement_rows
            .query_map([&occurrence_id], |row| {
                let mut attempt = json!({
                    "attemptId": row.get::<_, String>(0)?,
                    "attemptNumber": row.get::<_, i64>(1)?,
                    "state": row.get::<_, String>(2)?,
                });
                if let Some(class) = row.get::<_, Option<String>>(3)? {
                    attempt["failureClass"] = json!(class);
                }
                Ok(attempt)
            })
            .map_err(|error| format!("failed to read the mission's attempts: {error}"))?;
        for attempt in rows {
            attempts.push(attempt.map_err(|error| format!("failed to read an attempt: {error}"))?);
        }
    }
    let mut view = json!({
        "missionId": mission_id,
        "occurrenceId": occurrence_id,
        "revision": revision,
        "state": state,
        "terminal": terminal,
        "principal": { "principalId": principal_id },
        "familiar": { "rootId": root_id, "revisionId": revision_id },
        "envelope": {
            "id": envelope_id,
            "descriptorDigest": digest_value(&envelope_digest),
        },
        "project": { "cwd": cwd },
        "task": { "statement": statement },
        "submittedAt": submitted_at,
        "validUntil": valid_until,
        "attempts": attempts,
    });
    if let Some(reason) = reason {
        view["reason"] = json!(reason);
    }
    if let Some(scopes_json) = scopes_json {
        view["scopes"] = serde_json::from_str(&scopes_json).map_err(|error| {
            format!("stored scopes of mission `{mission_id}` are invalid: {error}")
        })?;
    }
    if let Some(cancellation) = cancellation {
        view["cancellation"] = cancellation;
    }
    if terminal {
        view["result"] = json!({
            "outcome": state,
            "settledAt": settled_at,
            "artifacts": [],
            "usage": null,
            // Nothing was launched, so nothing went unobserved.
            "observation": "complete",
        });
    }
    Ok(Some(view))
}

/// The cancellation `mission.cancel` asked for, if any: the first committed
/// `occurrence.cancel.v1` adoption under a key it derived. Its outcome follows
/// the occurrence as it stands now.
fn cancellation(
    conn: &Connection,
    mission_id: &str,
    occurrence_state: &str,
) -> Result<Option<Value>, String> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT adoption_key, adopted_at FROM automation_command_adoptions
             WHERE command = 'occurrence.cancel.v1' AND automation_id = ?1
               AND outcome = 'committed'
               AND substr(adoption_key, 1, length(?2)) = ?2
             ORDER BY adopted_at, adoption_key
             LIMIT 1",
            params![mission_id, CANCELLATION_KEY_PREFIX],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| format!("failed to read the mission's cancellation: {error}"))?;
    Ok(row.map(|(cancellation_id, requested_at)| {
        let outcome = match occurrence_state {
            "cancelled" => "cancelled",
            "recovery_required" => "recovery_required",
            "succeeded" | "failed" | "timed_out" | "skipped" | "superseded" => "lost_to_completion",
            _ => "pending",
        };
        json!({
            "cancellationId": cancellation_id,
            "requestedAt": requested_at,
            "outcome": outcome,
        })
    }))
}

/// The mission state for an occurrence state, by the contract's projection.
fn project(state: &str, failure_reason: Option<&str>) -> (&'static str, Option<&'static str>) {
    match state {
        "planned" | "eligible" | "claimed" => ("queued", None),
        "dispatching" | "running" => ("running", None),
        "succeeded" => ("completed", None),
        "cancelled" => ("cancelled", None),
        "timed_out" => ("failed", Some("timeout")),
        "failed" if failure_reason == Some(AUTHORITY_UNAVAILABLE_REASON) => {
            ("unavailable", Some("authority_unavailable"))
        }
        "failed" if failure_reason == Some(LEASE_EXPIRED_REASON) => {
            ("rejected", Some("expired_before_dispatch"))
        }
        "failed" => ("failed", Some("harness_failed")),
        // Recovery, and any state a mission should never reach: execution may
        // or may not have happened, so it is never reported as nothing.
        _ => ("reconciling", Some("ambiguous_execution")),
    }
}

/// The runtime envelopes a mission may name. The capability catalog
/// advertises this list as `missionEnvelopes`, and an unsupported-envelope
/// refusal repeats it, so a client learns the digest before it submits.
pub(crate) fn advertised_envelopes() -> Value {
    json!([{
        "id": CLAUDE_R0_READ.descriptor_version,
        "descriptorDigest": digest_value(&CLAUDE_R0_READ.descriptor_digest()),
    }])
}

fn unsupported_envelope(requested: &str) -> ErrorEnvelope {
    typed_error(
        ErrorCode::CapabilityUnsupported,
        format!(
            "envelope `{requested}` is not one this daemon advertises and enforces for missions"
        ),
    )
    .with_details(BTreeMap::from([(
        "supportedEnvelopes".to_owned(),
        advertised_envelopes(),
    )]))
}

fn digest_value(value: &str) -> Value {
    json!({ "algorithm": "sha256", "canonicalization": "jcs-rfc8785", "value": value })
}

fn canonical_directory(path: &str) -> Result<PathBuf, String> {
    let canonical = crate::project::canonical_project_root(Path::new(path))
        .map_err(|_| format!("project.cwd `{path}` does not exist or cannot be resolved"))?;
    if !canonical.is_dir() {
        return Err(format!("project.cwd `{path}` is not a directory"));
    }
    Ok(canonical)
}

/// The store's own home (its parent directory), canonical.
fn store_home(conn: &Connection) -> Option<PathBuf> {
    conn.path()
        .filter(|path| !path.is_empty())
        .and_then(|path| Path::new(path).parent())
        .and_then(|home| crate::project::canonical_project_root(home).ok())
}

fn replay_mismatch(
    adoption_key: &str,
    conflict_outcome: Option<AdoptionConflictOutcome>,
) -> ErrorEnvelope {
    let error = typed_error(
        ErrorCode::AdoptionReplayMismatch,
        "adoption key was already used for a different request",
    );
    match AdoptionKey::new(adoption_key.to_owned()) {
        Ok(key) => error.with_adoption(ErrorAdoption {
            key,
            conflict_outcome,
        }),
        Err(_) => error,
    }
}

fn typed_error(code: ErrorCode, message: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope::try_new(code, message, false)
        .expect("mission error messages satisfy protocol bounds")
}

fn iso(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::NoopSessionRuntime;
    use crate::automations::command_envelope::tests::{response_validator, spec_validator};
    use crate::familiar_ledger::tests::{at, Home};

    struct Fixture {
        home: Home,
        project: tempfile::TempDir,
        root_id: String,
        revision_id: String,
    }

    fn fixture() -> Fixture {
        let home = Home::new();
        home.register("adopt:ledger:mission-register");
        let head = home.head();
        Fixture {
            root_id: head.root.root_id,
            revision_id: head.head.revision_id,
            project: tempfile::tempdir().unwrap(),
            home,
        }
    }

    fn now() -> DateTime<Utc> {
        at(60)
    }

    fn payload(fixture: &Fixture, requested_at: DateTime<Utc>) -> Value {
        json!({
            "familiar": { "rootId": fixture.root_id, "revisionId": fixture.revision_id },
            // What the catalog advertises, so every commit below proves the
            // daemon accepts the envelope it advertises.
            "envelope": advertised_envelopes()[0].clone(),
            "project": { "cwd": fixture.project.path().display().to_string() },
            "task": {
                "statement": "Summarize the open issues.",
                "prompt": "Read the open issues and summarize them.",
            },
            "limits": { "timeoutMinutes": 10 },
            "validUntil": iso(requested_at + Duration::minutes(10)),
        })
    }

    fn flat(key: &str, payload: Value) -> Value {
        let mut body = payload;
        body["action"] = json!(SUBMIT_ACTION);
        body["adoptionKey"] = json!(key);
        body
    }

    fn submit(fixture: &Fixture, body: Value) -> MissionExecution {
        execute_mission_submit(
            &fixture.home.conn,
            CommandAuthority::OwnerLocal,
            body,
            now(),
        )
        .unwrap()
    }

    fn committed(answer: MissionExecution) -> (Value, bool) {
        match answer {
            MissionExecution::Success { payload, replayed } => (payload, replayed),
            MissionExecution::Rejected(error) => panic!("refused: {error:?}"),
        }
    }

    fn refused(answer: MissionExecution) -> Value {
        match answer {
            MissionExecution::Rejected(error) => serde_json::to_value(error).unwrap(),
            MissionExecution::Success { payload, .. } => panic!("committed: {payload}"),
        }
    }

    fn view(fixture: &Fixture, mission_id: &str) -> Value {
        committed(
            execute_mission_get(
                &fixture.home.conn,
                CommandAuthority::OwnerLocal,
                &json!({ "action": GET_ACTION, "missionId": mission_id }),
            )
            .unwrap(),
        )
        .0
    }

    fn view_of(fixture: &Fixture, mission_id: &str) -> Value {
        view(fixture, mission_id)
    }

    fn assert_mission_view(view: &Value) {
        let validator = spec_validator("missionView");
        let errors: Vec<String> = validator
            .iter_errors(view)
            .map(|error| error.to_string())
            .collect();
        assert!(errors.is_empty(), "{errors:?}\n{view:#}");
    }

    /// Records a mission and leaves it queued, as a daemon that can dispatch
    /// would. M0 settles every submitted mission at once, so this is the only
    /// way to reach a mission before dispatch. Returns its id.
    fn hold(fixture: &Fixture, key: &str) -> String {
        let request: MissionSubmitPayload =
            serde_json::from_value(payload(fixture, now())).unwrap();
        let transaction = rusqlite::Transaction::new_unchecked(
            &fixture.home.conn,
            TransactionBehavior::Immediate,
        )
        .unwrap();
        claim_in(&transaction, key, &"a".repeat(64), &request, now())
            .unwrap()
            .unwrap();
        transaction.commit().unwrap();
        mission_id_for(key)
    }

    fn cancel(fixture: &Fixture, authority: CommandAuthority, body: Value) -> MissionExecution {
        execute_mission_cancel(
            &fixture.home.conn,
            &NoopSessionRuntime,
            authority,
            body,
            now() + Duration::seconds(5),
        )
        .unwrap()
    }

    fn cancel_body(key: &str, mission_id: &str) -> Value {
        json!({ "action": CANCEL_ACTION, "adoptionKey": key, "missionId": mission_id })
    }

    fn count(fixture: &Fixture, table: &str) -> i64 {
        fixture.home.count(table)
    }

    #[test]
    fn submit_records_the_mission_and_settles_it_unavailable_without_a_launch() {
        let fixture = fixture();
        let key = "adopt:mission:first";
        let (answer, replayed) = committed(submit(&fixture, flat(key, payload(&fixture, now()))));
        assert!(!replayed);
        let mission_id = mission_id_for(key);
        assert_eq!(answer["missionId"], mission_id);
        assert_eq!(answer["occurrenceId"], format!("occ-{mission_id}"));
        // The answer is the state at commit.
        let at_commit = &answer["mission"];
        assert_mission_view(at_commit);
        assert_eq!(
            (at_commit["state"].as_str(), at_commit["terminal"].as_bool()),
            (Some("queued"), Some(false))
        );
        assert!(
            at_commit["task"].get("prompt").is_none(),
            "the prompt is never echoed"
        );
        assert!(at_commit["principal"]["principalId"]
            .as_str()
            .unwrap()
            .starts_with("principal:coven-local-owner:"));

        // M0 launches nothing: the mission settles unavailable.
        let settled = view(&fixture, &mission_id);
        assert_mission_view(&settled);
        assert_eq!(
            (
                settled["state"].as_str(),
                settled["reason"].as_str(),
                settled["terminal"].as_bool(),
                settled["result"]["outcome"].as_str(),
            ),
            (
                Some("unavailable"),
                Some("authority_unavailable"),
                Some(true),
                Some("unavailable")
            )
        );
        assert!(settled["revision"].as_u64() > at_commit["revision"].as_u64());
        for table in ["automation_runs", "automation_attempts", "sessions"] {
            assert_eq!(count(&fixture, table), 0, "{table}");
        }

        // Its internal definition is not a routine.
        let conn = &fixture.home.conn;
        assert!(super::super::store::get_definition(conn, &mission_id)
            .unwrap()
            .is_none());
        assert!(
            super::super::store::list_definitions_with_tombstones(conn, true)
                .unwrap()
                .iter()
                .all(|routine| routine.id != mission_id)
        );
    }

    #[test]
    fn a_replay_returns_the_first_commit_and_a_changed_body_is_a_mismatch() {
        let fixture = fixture();
        let key = "adopt:mission:replay";
        let body = flat(key, payload(&fixture, now()));
        let (first, _) = committed(submit(&fixture, body.clone()));

        // The same request, even with routing metadata added, is a replay.
        let mut again = body.clone();
        again["origin"] = json!("cli");
        let (replay, replayed) = committed(submit(&fixture, again));
        assert!(replayed);
        assert_eq!(replay, first);

        let mut changed = body;
        changed["task"]["statement"] = json!("Summarize the closed issues.");
        let error = refused(submit(&fixture, changed));
        assert_eq!(error["code"], "ADOPTION_REPLAY_MISMATCH");
        assert_eq!(error["adoption"]["conflictOutcome"], "committed");
        assert_eq!(count(&fixture, "automation_missions"), 1);
        assert_eq!(count(&fixture, "automation_occurrences"), 1);
    }

    #[test]
    fn submit_refuses_what_it_cannot_honour_and_records_nothing() {
        let fixture = fixture();
        let stale_revision = format!(
            "{}7",
            fixture
                .revision_id
                .trim_end_matches(|c: char| c.is_ascii_digit())
        );
        type Edit = Box<dyn Fn(&mut Value)>;
        let edits: Vec<(&str, Edit, &str)> = vec![
            (
                "past",
                Box::new(|body| body["validUntil"] = json!(iso(now() - Duration::minutes(1)))),
                "VALIDATION_FAILED",
            ),
            (
                "far",
                Box::new(|body| body["validUntil"] = json!(iso(now() + Duration::minutes(16)))),
                "VALIDATION_FAILED",
            ),
            (
                "envelope-id",
                Box::new(|body| body["envelope"]["id"] = json!("r1-write.1")),
                "CAPABILITY_UNSUPPORTED",
            ),
            (
                "envelope-digest",
                Box::new(|body| {
                    body["envelope"]["descriptorDigest"]["value"] = json!("0".repeat(64))
                }),
                "CAPABILITY_UNSUPPORTED",
            ),
            (
                "inside-home",
                Box::new({
                    let inside = fixture.home.path().join("familiars").display().to_string();
                    move |body| body["project"]["cwd"] = json!(inside)
                }),
                "VALIDATION_FAILED",
            ),
            (
                "missing-cwd",
                Box::new({
                    let missing = fixture.project.path().join("missing").display().to_string();
                    move |body| body["project"]["cwd"] = json!(missing)
                }),
                "VALIDATION_FAILED",
            ),
            (
                "unknown-root",
                Box::new(|body| {
                    body["familiar"]["rootId"] = json!(format!("familiar:{}", "0".repeat(32)))
                }),
                "NOT_FOUND",
            ),
            (
                "stale-revision",
                Box::new(move |body| body["familiar"]["revisionId"] = json!(stale_revision)),
                "REVISION_CONFLICT",
            ),
            (
                "long-prompt",
                Box::new(|body| body["task"]["prompt"] = json!("é".repeat(16_385))),
                "VALIDATION_FAILED",
            ),
            (
                "no-task",
                Box::new(|body| {
                    body.as_object_mut().unwrap().remove("task");
                }),
                "VALIDATION_FAILED",
            ),
        ];
        for (name, edit, code) in edits {
            let key = format!("adopt:mission:refused-{name}");
            let mut body = flat(&key, payload(&fixture, now()));
            edit(&mut body);
            let error = refused(submit(&fixture, body));
            assert_eq!(error["code"], code, "{name}: {error}");
            match name {
                "envelope-id" => assert_eq!(
                    error["details"]["supportedEnvelopes"],
                    advertised_envelopes(),
                    "{error}"
                ),
                "stale-revision" => assert_eq!(
                    error["details"]["currentRevisionId"], fixture.revision_id,
                    "{error}"
                ),
                _ => {}
            }
        }
        for table in [
            "automation_missions",
            "automation_occurrences",
            "automation_runs",
        ] {
            assert_eq!(count(&fixture, table), 0, "{table}");
        }
    }

    #[test]
    fn a_second_mission_waits_for_the_first_in_the_same_project() {
        let fixture = fixture();
        hold(&fixture, "adopt:mission:held");

        let key = "adopt:mission:second";
        let error = refused(submit(&fixture, flat(key, payload(&fixture, now()))));
        assert_eq!(error["code"], "CONCURRENCY_LIMIT");

        let elsewhere = tempfile::tempdir().unwrap();
        let mut body = flat("adopt:mission:elsewhere", payload(&fixture, now()));
        body["project"]["cwd"] = json!(elsewhere.path().display().to_string());
        committed(submit(&fixture, body));
    }

    #[test]
    fn missions_belong_to_the_owner() {
        let fixture = fixture();
        let conn = &fixture.home.conn;
        let key = "adopt:mission:owner";
        let error = refused(
            execute_mission_submit(
                conn,
                CommandAuthority::Unauthenticated,
                flat(key, payload(&fixture, now())),
                now(),
            )
            .unwrap(),
        );
        assert_eq!(error["code"], "AUTHORITY_REQUIRED");
        assert_eq!(count(&fixture, "automation_command_adoptions"), 0);

        committed(submit(&fixture, flat(key, payload(&fixture, now()))));
        let mission_id = mission_id_for(key);
        let get =
            |authority, body: Value| refused(execute_mission_get(conn, authority, &body).unwrap());
        assert_eq!(
            get(
                CommandAuthority::Unauthenticated,
                json!({ "action": GET_ACTION, "missionId": mission_id })
            )["code"],
            "NOT_FOUND",
            "another caller learns nothing"
        );
        assert_eq!(
            get(
                CommandAuthority::OwnerLocal,
                json!({ "action": GET_ACTION, "missionId": mission_id_for("adopt:mission:none") })
            )["code"],
            "NOT_FOUND"
        );
        assert_eq!(
            get(
                CommandAuthority::OwnerLocal,
                json!({ "action": GET_ACTION, "missionId": "mission:not-an-id" })
            )["code"],
            "VALIDATION_FAILED"
        );
        assert_eq!(
            get(
                CommandAuthority::OwnerLocal,
                json!({ "action": GET_ACTION, "missionId": mission_id, "prompt": true })
            )["code"],
            "VALIDATION_FAILED"
        );
    }

    #[test]
    fn routine_commands_do_not_reach_a_mission() {
        let fixture = fixture();
        let conn = &fixture.home.conn;
        let key = "adopt:mission:not-a-routine";
        committed(submit(&fixture, flat(key, payload(&fixture, now()))));
        let mission_id = mission_id_for(key);
        let route = |request: Value| {
            crate::control_plane::route_action_at(
                request,
                conn,
                &NoopSessionRuntime,
                &iso(now()),
                CommandAuthority::OwnerLocal,
            )
        };
        for request in [
            json!({
                "action": "coven.automations.definition.activate.v1",
                "adoptionKey": "adopt:activate:mission",
                "id": mission_id,
                "expectedRevision": 1,
            }),
            json!({
                "action": "coven.automations.occurrence.runNow.v1",
                "adoptionKey": "adopt:run-now:mission",
                "automationId": mission_id,
            }),
        ] {
            let (status, response) = route(request.clone());
            assert_eq!(
                (
                    status,
                    response.error.as_ref().map(|error| error["code"].clone())
                ),
                (404, Some(json!("NOT_FOUND"))),
                "{request}: {response:?}"
            );
        }
        // A read answers exactly as it does for an id that names nothing.
        let read = |id: &str| {
            let (status, response) =
                route(json!({ "action": "coven.automations.definition.get.v1", "id": id }));
            (
                status,
                response.result,
                response.event.map(|event| event.payload),
            )
        };
        assert_eq!(read(&mission_id), read("no-such-routine"));
        assert_eq!(count(&fixture, "automation_occurrences"), 1);
    }

    #[test]
    fn cancel_stops_a_queued_mission_once() {
        let fixture = fixture();
        let mission_id = hold(&fixture, "adopt:mission:to-cancel");
        let key = "adopt:mission-cancel:first";
        let mut body = cancel_body(key, &mission_id);
        body["reason"] = json!("Not needed after all.");
        let (answer, replayed) =
            committed(cancel(&fixture, CommandAuthority::OwnerLocal, body.clone()));
        assert!(!replayed);
        let view = &answer["mission"];
        assert_mission_view(view);
        assert_eq!(
            (
                view["state"].as_str(),
                view["terminal"].as_bool(),
                view["result"]["outcome"].as_str(),
                answer["cancellation"]["outcome"].as_str(),
            ),
            (
                Some("cancelled"),
                Some(true),
                Some("cancelled"),
                Some("cancelled")
            )
        );
        assert_eq!(answer["cancellation"], view["cancellation"]);
        let cancellation_id = answer["cancellation"]["cancellationId"].as_str().unwrap();
        assert!(cancellation_id.starts_with(CANCELLATION_KEY_PREFIX));
        // The view keeps the cancellation.
        assert_eq!(
            view_of(&fixture, &mission_id)["cancellation"],
            answer["cancellation"]
        );

        // The same key is the same cancellation; a new one finds nothing left.
        let (replay, replayed) = committed(cancel(&fixture, CommandAuthority::OwnerLocal, body));
        assert!(replayed);
        assert_eq!(replay, answer);
        let error = refused(cancel(
            &fixture,
            CommandAuthority::OwnerLocal,
            cancel_body("adopt:mission-cancel:second", &mission_id),
        ));
        assert_eq!(error["code"], "ILLEGAL_TRANSITION");
        assert_eq!(error["details"]["mission"]["state"], "cancelled");
        for table in ["automation_runs", "automation_attempts", "sessions"] {
            assert_eq!(count(&fixture, table), 0, "{table}");
        }
    }

    #[test]
    fn cancel_refuses_a_settled_mission_with_its_view() {
        let fixture = fixture();
        // M0: the submitted mission settles unavailable at once.
        let submitted = "adopt:mission:settled";
        committed(submit(&fixture, flat(submitted, payload(&fixture, now()))));
        let mission_id = mission_id_for(submitted);

        let key = "adopt:mission-cancel:late";
        let error = refused(cancel(
            &fixture,
            CommandAuthority::OwnerLocal,
            cancel_body(key, &mission_id),
        ));
        assert_eq!(error["code"], "ILLEGAL_TRANSITION");
        assert_mission_view(&error["details"]["mission"]);
        assert_eq!(error["details"]["mission"]["state"], "unavailable");
        // The refusal is adopted: the key answers the same way again, and
        // cannot be reused for another request.
        assert_eq!(
            refused(cancel(
                &fixture,
                CommandAuthority::OwnerLocal,
                cancel_body(key, &mission_id)
            )),
            error
        );
        let mut changed = cancel_body(key, &mission_id);
        changed["reason"] = json!("Changed my mind.");
        let mismatch = refused(cancel(&fixture, CommandAuthority::OwnerLocal, changed));
        assert_eq!(
            (
                mismatch["code"].as_str(),
                mismatch["adoption"]["conflictOutcome"].as_str()
            ),
            (Some("ADOPTION_REPLAY_MISMATCH"), Some("rejected"))
        );

        let cases = [
            (
                CommandAuthority::OwnerLocal,
                cancel_body(
                    "adopt:mission-cancel:unknown",
                    &mission_id_for("adopt:none"),
                ),
                "NOT_FOUND",
            ),
            (
                CommandAuthority::OwnerLocal,
                json!({
                    "action": CANCEL_ACTION,
                    "adoptionKey": "adopt:mission-cancel:long",
                    "missionId": mission_id,
                    "reason": "x".repeat(501),
                }),
                "VALIDATION_FAILED",
            ),
            (
                CommandAuthority::Unauthenticated,
                cancel_body("adopt:mission-cancel:remote", &mission_id),
                "AUTHORITY_REQUIRED",
            ),
        ];
        for (authority, body, code) in cases {
            assert_eq!(
                refused(cancel(&fixture, authority, body.clone()))["code"],
                code,
                "{body}"
            );
        }
        assert_eq!(count(&fixture, "automation_runs"), 0);
    }

    fn scope(path: &str, recursive: bool) -> Value {
        json!({
            "kind": "filesystem",
            "root": "project",
            "path": path,
            "access": "read",
            "recursive": recursive,
        })
    }

    fn with_scopes(fixture: &Fixture, key: &str, scopes: Value) -> Value {
        let mut body = flat(key, payload(fixture, now()));
        body["scopes"] = scopes;
        body
    }

    #[test]
    fn submit_records_project_read_scopes_and_get_returns_them() {
        let fixture = fixture();
        let key = "adopt:mission:scoped";
        // Members in a non-canonical order: the row stores the canonical form.
        let scopes = json!([
            scope("src/lib", true),
            { "recursive": false, "access": "read", "path": "README.md", "root": "project", "kind": "filesystem" },
        ]);
        let (answer, replayed) = committed(submit(&fixture, with_scopes(&fixture, key, scopes)));
        assert!(!replayed);
        let canonical = json!([scope("src/lib", true), scope("README.md", false)]);
        assert_mission_view(&answer["mission"]);
        assert_eq!(answer["mission"]["scopes"], canonical);

        let settled = view(&fixture, &mission_id_for(key));
        assert_mission_view(&settled);
        assert_eq!(settled["scopes"], canonical);
        // Scopes launch nothing: the mission still settles unavailable.
        assert_eq!(
            (settled["state"].as_str(), settled["reason"].as_str()),
            (Some("unavailable"), Some("authority_unavailable"))
        );
        for table in ["automation_runs", "automation_attempts", "sessions"] {
            assert_eq!(count(&fixture, table), 0, "{table}");
        }
        let stored: String = fixture
            .home
            .conn
            .query_row(
                "SELECT scopes_json FROM automation_missions WHERE mission_id = ?1",
                [mission_id_for(key)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            stored.as_bytes(),
            canonicalize(&canonical).unwrap().as_slice()
        );

        // Thirty-two scopes is the most a mission holds.
        let most: Vec<Value> = (0..32)
            .map(|index| scope(&format!("src/file-{index}.rs"), false))
            .collect();
        let mut body = with_scopes(&fixture, "adopt:mission:scoped-most", json!(most));
        let elsewhere = tempfile::tempdir().unwrap();
        body["project"]["cwd"] = json!(elsewhere.path().display().to_string());
        let (answer, _) = committed(submit(&fixture, body));
        assert_eq!(answer["mission"]["scopes"], json!(most));
    }

    #[test]
    fn a_mission_without_scopes_keeps_its_wire_form() {
        let fixture = fixture();
        let key = "adopt:mission:unscoped";
        let sent = payload(&fixture, now());
        let typed: MissionSubmitPayload = serde_json::from_value(sent.clone()).unwrap();
        assert_eq!(serde_json::to_value(&typed).unwrap(), sent);
        let (answer, _) = committed(submit(&fixture, flat(key, sent)));
        assert!(answer["mission"].get("scopes").is_none(), "{answer}");
        assert!(view(&fixture, &mission_id_for(key)).get("scopes").is_none());
        let stored: Option<String> = fixture
            .home
            .conn
            .query_row(
                "SELECT scopes_json FROM automation_missions WHERE mission_id = ?1",
                [mission_id_for(key)],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stored, None);
    }

    #[test]
    fn submit_refuses_scopes_it_cannot_honour_and_records_nothing() {
        let fixture = fixture();
        let field = |name: &str, value: Value| {
            let mut item = scope("src/lib", true);
            item[name] = value;
            item
        };
        let without = |name: &str| {
            let mut item = scope("src/lib", true);
            item.as_object_mut().unwrap().remove(name);
            item
        };
        let too_many: Vec<Value> = (0..33)
            .map(|index| scope(&format!("src/file-{index}.rs"), false))
            .collect();
        // Each refusal names the item and the rule it breaks. `None` is a list
        // that is not a list: the payload parser refuses it before validation.
        let unenforceable = "is not a scope the read service can enforce (grant_unsupported)";
        let cases: Vec<(&str, Value, Option<&str>)> = vec![
            ("empty", json!([]), Some("scopes must name at least one")),
            ("over-limit", json!(too_many), Some("at most 32")),
            ("null", Value::Null, None),
            ("not-a-list", scope("src/lib", true), None),
            (
                "root-wide-dot",
                json!([scope(".", false)]),
                Some(unenforceable),
            ),
            (
                "root-wide-dot-recursive",
                json!([scope(".", true)]),
                Some(unenforceable),
            ),
            (
                "root-wide-slash",
                json!([scope("/", true)]),
                Some(unenforceable),
            ),
            (
                "root-wide-top-level-subtree",
                json!([scope("src", true)]),
                Some(unenforceable),
            ),
            (
                "absolute",
                json!([scope("/etc/passwd", false)]),
                Some(unenforceable),
            ),
            (
                "absolute-drive",
                json!([scope("C:/project/notes.md", false)]),
                Some(unenforceable),
            ),
            ("parent", json!([scope("..", false)]), Some(unenforceable)),
            (
                "parent-inside",
                json!([scope("src/../secrets/key.pem", false)]),
                Some(unenforceable),
            ),
            (
                "glob",
                json!([scope("src/*.rs", false)]),
                Some(unenforceable),
            ),
            (
                "glob-subtree",
                json!([scope("src/**", true)]),
                Some(unenforceable),
            ),
            (
                "workspace-root",
                json!([field("root", json!("workspace"))]),
                Some("scopes[0] must be rooted at `project`"),
            ),
            (
                "unknown-root",
                json!([field("root", json!("home"))]),
                Some(unenforceable),
            ),
            (
                "write",
                json!([field("access", json!("write"))]),
                Some("scopes[0] must grant `read` access"),
            ),
            (
                "unknown-access",
                json!([field("access", json!("execute"))]),
                Some(unenforceable),
            ),
            (
                "network-kind",
                json!([field("kind", json!("network"))]),
                Some("scopes[0] must be a `filesystem` scope"),
            ),
            (
                "unknown-kind",
                json!([field("kind", json!("process"))]),
                Some(unenforceable),
            ),
            (
                "missing-recursive",
                json!([without("recursive")]),
                Some(unenforceable),
            ),
            (
                "missing-path",
                json!([without("path")]),
                Some(unenforceable),
            ),
            (
                "extra-field",
                json!([field("note", json!("extra"))]),
                Some(unenforceable),
            ),
            (
                "string-recursive",
                json!([field("recursive", json!("true"))]),
                Some(unenforceable),
            ),
            ("empty-path", json!([scope("", false)]), Some(unenforceable)),
            (
                "backslash",
                json!([scope("src\\lib\\mod.rs", false)]),
                Some(unenforceable),
            ),
            ("not-an-object", json!(["src/lib"]), Some(unenforceable)),
            (
                "one-bad-among-good",
                json!([scope("src/lib", true), scope("/etc", false)]),
                Some("scopes[1] is not a scope"),
            ),
        ];
        for (name, scopes, message) in cases {
            let key = format!("adopt:mission:scope-{name}");
            let error = refused(submit(&fixture, with_scopes(&fixture, &key, scopes)));
            assert_eq!(error["code"], "VALIDATION_FAILED", "{name}: {error}");
            let text = error["message"].as_str().unwrap();
            match message {
                Some(message) => assert!(text.contains(message), "{name}: {error}"),
                None => assert!(
                    text.starts_with("invalid mission.submit.v1 payload"),
                    "{name}: {error}"
                ),
            }
        }
        for table in [
            "automation_missions",
            "automation_occurrences",
            "automation_runs",
            "automation_definitions",
        ] {
            assert_eq!(count(&fixture, table), 0, "{table}");
        }
    }

    #[test]
    fn a_store_from_before_scopes_gains_the_column_in_the_fresh_form() {
        let fixture = fixture();
        let conn = &fixture.home.conn;
        let form = || -> String {
            conn.query_row(
                "SELECT sql FROM sqlite_schema WHERE name = 'automation_missions'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        let fresh = form();
        // A store from before the column, holding a mission.
        let mission_id = hold(&fixture, "adopt:mission:before-scopes");
        conn.execute(
            "ALTER TABLE automation_missions DROP COLUMN scopes_json",
            [],
        )
        .unwrap();
        assert_ne!(form(), fresh);
        super::super::store::ensure_mission_columns(conn).unwrap();
        super::super::store::ensure_mission_columns(conn).unwrap();
        assert_eq!(form(), fresh);
        let view = view(&fixture, &mission_id);
        assert_mission_view(&view);
        assert!(view.get("scopes").is_none());
    }

    #[test]
    fn scopes_are_part_of_the_request_an_adoption_key_names() {
        let fixture = fixture();
        let key = "adopt:mission:scoped-replay";
        let body = with_scopes(&fixture, key, json!([scope("src/lib", true)]));
        let (first, _) = committed(submit(&fixture, body.clone()));

        let (replay, replayed) = committed(submit(&fixture, body.clone()));
        assert!(replayed);
        assert_eq!(replay, first);

        for changed_scopes in [
            Some(json!([scope("src/bin", true)])),
            Some(json!([scope("src/lib", false)])),
            Some(json!([scope("src/lib", true), scope("README.md", false)])),
            None,
        ] {
            let mut changed = body.clone();
            match changed_scopes {
                Some(scopes) => changed["scopes"] = scopes,
                None => {
                    changed.as_object_mut().unwrap().remove("scopes");
                }
            }
            let error = refused(submit(&fixture, changed));
            assert_eq!(error["code"], "ADOPTION_REPLAY_MISMATCH", "{error}");
            assert_eq!(error["adoption"]["conflictOutcome"], "committed");
        }
        assert_eq!(count(&fixture, "automation_missions"), 1);
        assert_eq!(count(&fixture, "automation_occurrences"), 1);
    }

    #[test]
    fn both_wires_submit_replay_and_read_the_same_mission() {
        let fixture = fixture();
        let conn = &fixture.home.conn;
        let key = "adopt:mission:wire";
        let requested_at = Utc::now();
        let route = |request: Value| {
            let (status, response) = crate::control_plane::route_action_at(
                request,
                conn,
                &NoopSessionRuntime,
                &iso(requested_at),
                CommandAuthority::OwnerLocal,
            );
            assert_eq!(status, 200, "{response:?}");
            response.result.unwrap()
        };
        let envelope = |command: &str, payload: Value| {
            json!({
                "action": "coven.automations.command.v1",
                "envelope": {
                    "schemaVersion": "coven.automations.v1",
                    "command": command,
                    "adoptionKey": key,
                    "origin": {
                        "principal": { "principalId": "principal:owner" },
                        "channel": "sdk",
                        "correlationId": "corr-mission-wire"
                    },
                    "intent": { "statement": "Run a read-only mission." },
                    "payload": payload,
                }
            })
        };
        let validator = response_validator();

        let first = route(envelope(
            "mission.submit.v1",
            payload(&fixture, requested_at),
        ));
        assert!(validator.is_valid(&first), "{first:#}");
        assert_eq!(first["outcome"], "committed");
        assert_mission_view(&first["result"]["mission"]);

        let replay = route(envelope(
            "mission.submit.v1",
            payload(&fixture, requested_at),
        ));
        assert!(validator.is_valid(&replay), "{replay:#}");
        assert_eq!(replay["outcome"], "replayed");
        assert!(replay["replay"]["firstCommittedAt"].is_string());
        assert_eq!(replay["result"], first["result"]);

        // The flat action is the same request under the same key.
        let flat_replay = route(flat(key, payload(&fixture, requested_at)));
        assert_eq!(flat_replay, first["result"]);

        let mission_id = first["result"]["missionId"].clone();
        let read = route(envelope(
            "mission.get.v1",
            json!({ "missionId": mission_id }),
        ));
        assert!(validator.is_valid(&read), "{read:#}");
        assert_eq!(read["result"]["state"], "unavailable");
        assert_mission_view(&read["result"]);

        // A cancel over the envelope answers like the flat action: M0's
        // mission has settled, so there is nothing left to cancel.
        let mut cancel = envelope("mission.cancel.v1", json!({ "missionId": mission_id }));
        cancel["envelope"]["adoptionKey"] = json!("adopt:mission-cancel:wire");
        let (status, response) = crate::control_plane::route_action_at(
            cancel,
            conn,
            &NoopSessionRuntime,
            &iso(requested_at),
            CommandAuthority::OwnerLocal,
        );
        assert_eq!(status, 422, "{response:?}");
        let refused = response.result.unwrap();
        assert!(validator.is_valid(&refused), "{refused:#}");
        assert_eq!(
            (
                refused["outcome"].as_str(),
                refused["error"]["code"].as_str()
            ),
            (Some("rejected"), Some("ILLEGAL_TRANSITION"))
        );
    }
}
