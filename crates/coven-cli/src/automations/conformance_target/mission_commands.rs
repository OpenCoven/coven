//! Mission commands under M0 (coven#1313). Each case sends steps, in order,
//! to one store that holds one registered familiar, whose first revision has
//! been superseded by a second. The clock advances one minute per step. Only
//! production code commits, replays, refuses, launches, stops or settles;
//! this suite compares what it answers and what a refusal leaves behind.
//!
//! M-17 and M-18 cover a submit's `scopes` (#1597): accepted, returned by the
//! view and bound by the adoption key, or refused before anything is recorded.
//!
//! A case runs on one of two runtimes. `unavailable`, the default, takes no
//! authority projections, so a mission settles at its preflight (M-06). On
//! `launching`, a mission launches through the trusted adapter (#857): the
//! runtime preflights only that the envelope runs the scoped read service on
//! its own harness and establishes ownership without starting a process. A
//! `session.end` step then seeds what the harness would leave, its stream
//! and its channel's close, or nothing, and ends the session through the
//! store and the terminal observer. `daemon.tick` runs the daemon's mission
//! stop, run timeouts and settlement, and `familiar.revoke` revokes the
//! current revision. A stop the launching runtime is asked for reads the
//! mission's view on its own connection while it is in flight, as a polling
//! client would, before it lands.
//! M-08 is held by unit tests with a mission kept queued, and M-16 is a
//! transport rule, held by the transport authority tests.
//!
//! Values that exist only once the store does are written as whole-string
//! tokens:
//! - `{{rootId}}`, `{{revisionId}}` and `{{staleRevisionId}}` name the familiar;
//! - `{{project}}`, `{{insideHome}}` and `{{missingProject}}` name directories;
//! - `{{start:10m}}` and `{{start:-1m}}` are instants relative to the case's
//!   first step, so a repeated request is the same request;
//! - `{{bytes:N}}` is N bytes of text.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

use super::command_envelope_outcomes::durable_state;
use super::{scheduler_conformance_tempdir, valid_case_id, MAX_CASES};
use crate::api::{NoopSessionRuntime, SessionLaunch, SessionRuntime};
use crate::automations::authority_projection::AutomationAuthorityConsumerProjection;
use crate::automations::contract::error::ErrorCode;
use crate::automations::contract::types::{AdoptionKey, TerminalOutcome};
use crate::automations::missions::{
    execute_mission_cancel, execute_mission_get, execute_mission_submit_launching_at,
    mission_id_for, mission_view, stop_withdrawn_missions, MissionExecution, CANCEL_ACTION,
    GET_ACTION, SUBMIT_ACTION,
};
use crate::automations::owner_grants::CommandAuthority;
use crate::automations::runtime_envelope::RuntimeEnvelope;

const SCHEMA: &str = "coven.automations.mission-commands-vectors.v1";
const FIRST_AT: &str = "2026-10-18T09:00:00.000Z";
const MAX_STEPS: usize = 16;
const MAX_TOKEN_BYTES: usize = 65_536;
const FAMILIAR: &str = "scout";
const WARD: &str = "principal_key_fingerprint = \"SHA256:abc\"\nprotected_surface = [\"SOUL.md\"]\n\n[[surface]]\npath = \"SOUL.md\"\ntier = 0\n";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VectorSet {
    schema_version: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Case {
    case_id: String,
    #[serde(default)]
    runtime: Runtime,
    steps: Vec<Step>,
}

/// The runtime a case's daemon has.
#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Runtime {
    /// Takes no authority projections: a mission is refused at preflight.
    #[default]
    Unavailable,
    /// Launches the scoped read envelope ([`LaunchingRuntime`]).
    Launching,
}

#[derive(Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Step {
    command: Command,
    #[serde(default)]
    caller: Caller,
    #[serde(default)]
    adoption_key: Option<String>,
    /// The adoption key of the submit whose mission a get or cancel names.
    #[serde(default)]
    mission_of: Option<String>,
    /// A submit's payload, or a cancel's optional `reason`.
    #[serde(default)]
    payload: Option<Value>,
    /// How many minutes after a submit its launch begins: a busy dispatcher.
    #[serde(default)]
    launch_after_minutes: Option<u8>,
    /// What a `session.end` leaves behind.
    #[serde(default)]
    stream: Option<Stream>,
    expect: Expect,
}

/// What the harness of a `session.end` printed.
#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Stream {
    /// A clean scoped read run that read nothing, and its channel's close.
    Complete,
    /// The same run, whose result event reports an error, as a harness whose
    /// model refused its key does before it exits 0.
    Errored,
    /// Nothing: the harness's last writes were never observed.
    None,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
enum Command {
    #[serde(rename = "mission.submit.v1")]
    Submit,
    #[serde(rename = "mission.get.v1")]
    Get,
    #[serde(rename = "mission.cancel.v1")]
    Cancel,
    #[serde(rename = "session.end")]
    SessionEnd,
    #[serde(rename = "daemon.tick")]
    Tick,
    #[serde(rename = "familiar.revoke")]
    RevokeFamiliar,
}

/// Who sends the step: the owner over the owner socket, or any other caller.
#[derive(Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Caller {
    #[default]
    Owner,
    Other,
}

#[derive(Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expect {
    outcome: Outcome,
    #[serde(default)]
    error_code: Option<ErrorCode>,
    #[serde(default)]
    conflict_outcome: Option<String>,
    /// Members the error's `details` must carry, after tokens resolve.
    #[serde(default)]
    details: Option<Value>,
    /// The mission view's state and reason: the answer's `mission`, a get's
    /// view, or a refusal's `details.mission`.
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    /// The scopes that view must return, compared whole.
    #[serde(default)]
    scopes: Option<Value>,
    /// The outcome of the cancellation that view reports.
    #[serde(default)]
    cancellation: Option<String>,
    /// How many attempts that view lists.
    #[serde(default)]
    attempts: Option<usize>,
    /// How many artifacts that view's result lists.
    #[serde(default)]
    artifacts: Option<usize>,
    /// How many sessions the store holds after the step.
    #[serde(default)]
    sessions: Option<i64>,
    /// The state a reader sees while the step's one stop is in flight.
    #[serde(default)]
    while_stopping: Option<String>,
    /// The earlier read whose view this one repeats exactly, revision and all.
    #[serde(default)]
    same_as: Option<usize>,
    /// The earlier, identical step whose answer this one repeats.
    #[serde(default)]
    replays: Option<usize>,
    /// What a refusal may leave behind.
    #[serde(default)]
    writes: Option<Writes>,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Writes {
    Nothing,
    Adoption,
    /// The refusal settled what it raced, so the domain may change too.
    Settlement,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Committed,
    Replayed,
    Rejected,
}

pub(super) fn evaluate(vector: &Value) -> Result<bool, &'static str> {
    let vectors: VectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != SCHEMA
        || vectors.cases.is_empty()
        || vectors.cases.len() > MAX_CASES
    {
        return Err("conformance vector is invalid");
    }
    let mut case_ids = BTreeSet::new();
    for case in &vectors.cases {
        if !valid_case_id(&case.case_id) || !case_ids.insert(&case.case_id) || !valid_steps(case) {
            return Err("conformance vector is invalid");
        }
    }

    let mut all_passed = true;
    for case in &vectors.cases {
        all_passed &= case_matches(case)?;
    }
    Ok(all_passed)
}

/// Each command carries exactly the fields it takes, a refusal names its code
/// and what it writes, and a repeat names an earlier identical step that
/// answered the same way: a commit for `replayed`, a refusal for `rejected`.
fn valid_steps(case: &Case) -> bool {
    if case.steps.is_empty() || case.steps.len() > MAX_STEPS {
        return false;
    }
    case.steps.iter().enumerate().all(|(index, step)| {
        let key_valid = step
            .adoption_key
            .as_ref()
            .is_some_and(|key| AdoptionKey::new(key.clone()).is_ok());
        let mission_valid = step
            .mission_of
            .as_ref()
            .is_some_and(|key| AdoptionKey::new(key.clone()).is_ok());
        let shape = match step.command {
            Command::Submit => {
                key_valid
                    && step.mission_of.is_none()
                    && step.payload.as_ref().is_some_and(Value::is_object)
            }
            Command::Get | Command::SessionEnd | Command::Tick => {
                step.adoption_key.is_none() && mission_valid && step.payload.is_none()
            }
            Command::RevokeFamiliar => key_valid && mission_valid && step.payload.is_none(),
            Command::Cancel => {
                key_valid
                    && mission_valid
                    && step.payload.as_ref().is_none_or(|payload| {
                        payload
                            .as_object()
                            .is_some_and(|fields| fields.keys().all(|key| key == "reason"))
                    })
            }
        };
        let expect = &step.expect;
        let shape = shape
            && (step.launch_after_minutes.is_none() || step.command == Command::Submit)
            && (expect.while_stopping.is_none()
                || (case.runtime == Runtime::Launching
                    && matches!(step.command, Command::Tick | Command::Cancel)))
            && (step.stream.is_some() == (step.command == Command::SessionEnd))
            && expect.same_as.is_none_or(|earlier| {
                earlier < index
                    && step.command == Command::Get
                    && case.steps[earlier].command == Command::Get
                    && case.steps[earlier].mission_of == step.mission_of
            });
        let rejected = expect.outcome == Outcome::Rejected;
        let repeat_names_its_original = expect.replays.is_none_or(|earlier| {
            earlier < index
                && matches!(expect.outcome, Outcome::Replayed | Outcome::Rejected)
                && same_request(&case.steps[earlier], step)
                && case.steps[earlier].expect.outcome
                    == match expect.outcome {
                        Outcome::Replayed => Outcome::Committed,
                        other => other,
                    }
        }) && (expect.outcome != Outcome::Replayed
            || expect.replays.is_some());
        shape
            && repeat_names_its_original
            && rejected == expect.error_code.is_some()
            && rejected == expect.writes.is_some()
            && (expect.conflict_outcome.is_none() || rejected)
            && (expect.details.is_none() || rejected)
    })
}

fn same_request(first: &Step, second: &Step) -> bool {
    first.command == second.command
        && first.caller == second.caller
        && first.adoption_key == second.adoption_key
        && first.mission_of == second.mission_of
        && first.payload == second.payload
        && first.launch_after_minutes == second.launch_after_minutes
}

/// Any failure of the suite's own setup or execution, as the target reports it.
fn failed<E>(_: E) -> &'static str {
    "conformance suite execution failed"
}

/// The case's store, familiar and directories.
struct Fixture {
    home: std::path::PathBuf,
    conn: Connection,
    root_id: String,
    revision_id: String,
    stale_revision_id: String,
    project: String,
    inside_home: String,
    missing_project: String,
}

fn case_matches(case: &Case) -> Result<bool, &'static str> {
    let home = scheduler_conformance_tempdir()?;
    let projects = scheduler_conformance_tempdir()?;
    let first_at = DateTime::parse_from_rfc3339(FIRST_AT)
        .map_err(failed)?
        .with_timezone(&Utc);
    let fixture = fixture(home.path(), projects.path(), first_at)?;
    let launching = LaunchingRuntime::new(fixture.home.join("coven.sqlite3"));
    let runtime: &dyn SessionRuntime = match case.runtime {
        Runtime::Unavailable => &NoopSessionRuntime,
        Runtime::Launching => &launching,
    };

    let mut answers: Vec<(Outcome, Value)> = Vec::with_capacity(case.steps.len());
    let mut passed = true;
    for (index, step) in case.steps.iter().enumerate() {
        let minutes = i64::try_from(index).map_err(|_| "conformance vector is invalid")?;
        let now = first_at + Duration::minutes(minutes);
        let before = durable_state(&fixture.conn)?;
        let (outcome, answer) = send(&fixture, runtime, step, first_at, now)?;
        let after = durable_state(&fixture.conn)?;
        let seen_while_stopping = launching.seen_while_stopping.take();
        let sessions: i64 = fixture
            .conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .map_err(failed)?;
        let expect = &step.expect;

        let writes_match = expect.writes.is_none_or(|writes| match writes {
            Writes::Nothing => before.domain == after.domain && before.adoptions == after.adoptions,
            Writes::Adoption => {
                before.domain == after.domain && before.adoptions != after.adoptions
            }
            Writes::Settlement => before.adoptions != after.adoptions,
        });
        let repeat_matches = expect.replays.is_none_or(|earlier| {
            answers
                .get(earlier)
                .is_some_and(|(_, first)| first == &answer)
        });
        let error_code = serde_json::from_value::<ErrorCode>(answer["code"].clone()).ok();
        let details_match = match &expect.details {
            None => true,
            Some(details) => {
                let details = resolve(details, &fixture, first_at)?;
                details.as_object().is_some_and(|members| {
                    members
                        .iter()
                        .all(|(key, value)| answer["details"].get(key) == Some(value))
                })
            }
        };
        let view = match (outcome, step.command) {
            (Outcome::Rejected, _) => &answer["details"]["mission"],
            (_, Command::Submit | Command::Cancel) => &answer["mission"],
            _ => &answer,
        };
        let same_matches = expect.same_as.is_none_or(|earlier| {
            answers
                .get(earlier)
                .is_some_and(|(_, first)| first == &answer)
        });
        passed &= outcome == expect.outcome
            && error_code == expect.error_code
            && expect.conflict_outcome.as_deref().is_none_or(|conflict| {
                answer["adoption"]["conflictOutcome"].as_str() == Some(conflict)
            })
            && details_match
            && expect
                .state
                .as_deref()
                .is_none_or(|state| view["state"].as_str() == Some(state))
            && expect
                .reason
                .as_deref()
                .is_none_or(|reason| view["reason"].as_str() == Some(reason))
            && expect
                .scopes
                .as_ref()
                .is_none_or(|scopes| view.get("scopes") == Some(scopes))
            && expect.cancellation.as_deref().is_none_or(|cancellation| {
                view["cancellation"]["outcome"].as_str() == Some(cancellation)
            })
            && expect
                .attempts
                .is_none_or(|attempts| view["attempts"].as_array().map(Vec::len) == Some(attempts))
            && expect.sessions.is_none_or(|expected| sessions == expected)
            && expect.artifacts.is_none_or(|artifacts| {
                view["result"]["artifacts"].as_array().map(Vec::len) == Some(artifacts)
            })
            && expect
                .while_stopping
                .as_ref()
                .is_none_or(|state| seen_while_stopping == [state.clone()])
            && same_matches
            && repeat_matches
            && writes_match;
        answers.push((outcome, answer));
    }
    Ok(passed)
}

/// Sends one step and returns its outcome with the result, or the error.
fn send(
    fixture: &Fixture,
    runtime: &dyn SessionRuntime,
    step: &Step,
    start: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(Outcome, Value), &'static str> {
    let authority = match step.caller {
        Caller::Owner => CommandAuthority::OwnerLocal,
        Caller::Other => CommandAuthority::Unauthenticated,
    };
    let mission_id = step.mission_of.as_deref().map(mission_id_for);
    if let Some(answer) = daemon_step(fixture, runtime, step, mission_id.as_deref(), now)? {
        return Ok((Outcome::Committed, answer));
    }
    let mut body = match &step.payload {
        Some(payload) => resolve(payload, fixture, start)?,
        None => json!({}),
    };
    body["action"] = json!(match step.command {
        Command::Submit => SUBMIT_ACTION,
        Command::Get => GET_ACTION,
        _ => CANCEL_ACTION,
    });
    if let Some(key) = &step.adoption_key {
        body["adoptionKey"] = json!(key);
    }
    if let Some(mission_id) = mission_id {
        body["missionId"] = json!(mission_id);
    }
    let conn = &fixture.conn;
    let executed = match step.command {
        Command::Submit => {
            let launch_at =
                now + Duration::minutes(i64::from(step.launch_after_minutes.unwrap_or(0)));
            execute_mission_submit_launching_at(conn, runtime, authority, body, now, launch_at)
        }
        Command::Get => execute_mission_get(conn, authority, &body),
        _ => execute_mission_cancel(conn, runtime, authority, body, now),
    }
    .map_err(|_| "conformance suite execution failed")?;
    Ok(match executed {
        MissionExecution::Success { payload, replayed } => (
            if replayed {
                Outcome::Replayed
            } else {
                Outcome::Committed
            },
            payload,
        ),
        MissionExecution::Rejected(error) => (
            Outcome::Rejected,
            serde_json::to_value(error).map_err(|_| "conformance suite execution failed")?,
        ),
    })
}

/// Runs a step that is the daemon's or the harness's rather than a mission
/// command, answering with the mission's view after it; `None` for a command.
fn daemon_step(
    fixture: &Fixture,
    runtime: &dyn SessionRuntime,
    step: &Step,
    mission_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<Option<Value>, &'static str> {
    let conn = &fixture.conn;
    let mission_id = match step.command {
        Command::Submit | Command::Get | Command::Cancel => return Ok(None),
        _ => mission_id.ok_or("conformance vector is invalid")?,
    };
    match step.command {
        Command::SessionEnd => end_session(
            conn,
            mission_id,
            step.stream.ok_or("conformance vector is invalid")?,
            now,
        )?,
        Command::Tick => {
            stop_withdrawn_missions(conn, runtime, now).map_err(failed)?;
            crate::automations::runner::enforce_run_timeouts(conn, runtime, now).map_err(failed)?;
            crate::automations::runner::settle_finished_runs(conn, now).map_err(failed)?;
        }
        _ => {
            let (status, _) = crate::familiar_ledger::route(
                &json!({
                    "action": "coven.familiars.ledger.revoke.v1",
                    "adoptionKey": step.adoption_key,
                    "revisionId": fixture.revision_id,
                    "reason": "the familiar's key was exposed",
                }),
                conn,
                &fixture.home,
                CommandAuthority::OwnerLocal,
                now,
            );
            if status != 200 {
                return Err("conformance suite execution failed");
            }
        }
    }
    mission_view(conn, mission_id)
        .map_err(failed)?
        .ok_or("conformance suite execution failed")
        .map(Some)
}

/// What claude 2.1.305 prints under `r0-read.2` when it reads nothing.
const QUIET_STREAM: [&str; 3] = [
    r#"{"type":"system","subtype":"init","tools":["mcp__coven__read_file","mcp__coven__list_directory","mcp__coven__search"],"mcp_servers":[{"name":"coven","status":"connected","source":"dynamic"}],"permissionMode":"default","claude_code_version":"2.1.305","session_id":"native-session"}"#,
    r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Nothing needed reading."}]}}"#,
    r#"{"type":"result","subtype":"success","is_error":false,"result":"Nothing needed reading."}"#,
];

/// The result event that ends an errored stream instead.
const ERRORED_RESULT: &str = r#"{"type":"result","subtype":"success","is_error":true,"result":"Failed to authenticate. API Error: 401","usage":{"input_tokens":7,"output_tokens":0}}"#;

/// Ends the mission's session as a harness exiting cleanly would: what it
/// printed and its channel's close are seeded, then the store ends the
/// session and the terminal observer records it, in one transaction, as the
/// daemon's event writer does.
fn end_session(
    conn: &Connection,
    mission_id: &str,
    stream: Stream,
    now: DateTime<Utc>,
) -> Result<(), &'static str> {
    let (attempt_id, session_id): (String, String) = conn
        .query_row(
            "SELECT attempt.id, attempt.session_id FROM automation_attempts AS attempt
             JOIN automation_missions AS mission ON mission.occurrence_id = attempt.occurrence_id
             WHERE mission.mission_id = ?1 AND attempt.session_id IS NOT NULL",
            [mission_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(failed)?;
    let at = now.to_rfc3339_opts(SecondsFormat::Millis, true);
    if stream != Stream::None {
        let mut printed = QUIET_STREAM;
        if stream == Stream::Errored {
            printed[2] = ERRORED_RESULT;
        }
        for (index, line) in printed.iter().enumerate() {
            crate::store::insert_event(
                conn,
                &crate::store::EventRecord {
                    seq: 0,
                    id: format!("event:{session_id}:{index}"),
                    session_id: session_id.clone(),
                    kind: "output".to_owned(),
                    payload_json: json!({ "data": format!("{line}\n") }).to_string(),
                    created_at: at.clone(),
                },
            )
            .map_err(failed)?;
        }
        conn.execute_batch(crate::automations::scoped_read_settlement::SCHEMA)
            .map_err(failed)?;
        crate::automations::scoped_read_settlement::record(
            conn,
            &attempt_id,
            "channel:conformance",
            &[],
            now,
        )
        .map_err(failed)?;
    }
    let transaction =
        rusqlite::Transaction::new_unchecked(conn, rusqlite::TransactionBehavior::Immediate)
            .map_err(failed)?;
    if !crate::store::update_session_terminal_if_active(
        &transaction,
        &session_id,
        "completed",
        Some(0),
        &at,
    )
    .map_err(failed)?
    {
        return Err("conformance suite execution failed");
    }
    crate::automations::terminal_observer::record(
        &transaction,
        &session_id,
        TerminalOutcome::Succeeded,
        now,
    );
    transaction.commit().map_err(failed)
}

/// Stands in for a daemon that launches the scoped read envelope: it takes
/// authority projections, preflights only that the envelope runs the scoped
/// read service on its own harness, and establishes runtime ownership for
/// each authorized launch without starting a process. A stop it is asked for
/// always lands, once it has read, on its own connection to `store`, the
/// state of the mission whose session it stops.
struct LaunchingRuntime {
    store: PathBuf,
    seen_while_stopping: RefCell<Vec<String>>,
}

impl LaunchingRuntime {
    fn new(store: PathBuf) -> Self {
        Self {
            store,
            seen_while_stopping: RefCell::new(Vec::new()),
        }
    }

    /// The state of the mission running `session_id`, read as a client
    /// polling the daemon would.
    fn state_of(&self, session_id: &str) -> anyhow::Result<String> {
        let reader =
            Connection::open_with_flags(&self.store, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mission_id: String = reader.query_row(
            "SELECT mission.mission_id FROM automation_missions AS mission
             JOIN automation_attempts AS attempt ON attempt.occurrence_id = mission.occurrence_id
             WHERE attempt.session_id = ?1",
            [session_id],
            |row| row.get(0),
        )?;
        let view = mission_view(&reader, &mission_id).map_err(anyhow::Error::msg)?;
        Ok(view
            .and_then(|view| view["state"].as_str().map(ToOwned::to_owned))
            .unwrap_or_default())
    }
}

impl SessionRuntime for LaunchingRuntime {
    fn launch_session(&self, _launch: &SessionLaunch) -> anyhow::Result<()> {
        anyhow::bail!("a mission launches only under its authority projection")
    }

    fn accepts_automation_authority_projection(&self) -> bool {
        true
    }

    fn preflight_authority_launch(
        &self,
        harness: &str,
        envelope: &'static RuntimeEnvelope,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            harness == envelope.harness && envelope.service.is_some(),
            crate::api::RUNTIME_AUTHORITY_PROJECTION_UNSUPPORTED
        );
        Ok(())
    }

    fn launch_authorized_contained_adopted_session(
        &self,
        _launch: &SessionLaunch,
        authority: Option<&AutomationAuthorityConsumerProjection>,
        _writer: Option<crate::maintenance_gate::WriterLease>,
        ownership_established: &mut dyn FnMut() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            authority.is_some(),
            "a mission launches only under its authority projection"
        );
        ownership_established()
    }

    fn send_input(&self, _session_id: &str, _payload: &Value) -> anyhow::Result<()> {
        Ok(())
    }

    fn kill_session(&self, session_id: &str) -> anyhow::Result<()> {
        let state = self
            .state_of(session_id)
            .unwrap_or_else(|_| "unreadable".to_owned());
        self.seen_while_stopping.borrow_mut().push(state);
        Ok(())
    }
}

/// A copy of `value` with every whole-string token replaced.
fn resolve(value: &Value, fixture: &Fixture, start: DateTime<Utc>) -> Result<Value, &'static str> {
    Ok(match value {
        Value::String(text) => match text
            .strip_prefix("{{")
            .and_then(|rest| rest.strip_suffix("}}"))
        {
            Some(token) => json!(token_value(token, fixture, start)?),
            None => value.clone(),
        },
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| resolve(item, fixture, start))
                .collect::<Result<_, _>>()?,
        ),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(key, item)| Ok((key.clone(), resolve(item, fixture, start)?)))
                .collect::<Result<_, &'static str>>()?,
        ),
        _ => value.clone(),
    })
}

fn token_value(
    token: &str,
    fixture: &Fixture,
    start: DateTime<Utc>,
) -> Result<String, &'static str> {
    let invalid = "conformance vector is invalid";
    Ok(match token {
        "rootId" => fixture.root_id.clone(),
        "revisionId" => fixture.revision_id.clone(),
        "staleRevisionId" => fixture.stale_revision_id.clone(),
        "project" => fixture.project.clone(),
        "insideHome" => fixture.inside_home.clone(),
        "missingProject" => fixture.missing_project.clone(),
        _ => {
            if let Some(bytes) = token.strip_prefix("bytes:") {
                let bytes: usize = bytes.parse().map_err(|_| invalid)?;
                if bytes > MAX_TOKEN_BYTES {
                    return Err(invalid);
                }
                return Ok("x".repeat(bytes));
            }
            let minutes: i64 = token
                .strip_prefix("start:")
                .and_then(|rest| rest.strip_suffix('m'))
                .ok_or(invalid)?
                .parse()
                .map_err(|_| invalid)?;
            if !(-1_440..=1_440).contains(&minutes) {
                return Err(invalid);
            }
            (start + Duration::minutes(minutes)).to_rfc3339_opts(SecondsFormat::Millis, true)
        }
    })
}

/// A private Coven home with one familiar, registered and then revised, and
/// a project directory outside it.
fn fixture(home: &Path, projects: &Path, first_at: DateTime<Utc>) -> Result<Fixture, &'static str> {
    crate::daemon::ensure_private_coven_home(home).map_err(failed)?;
    let conn = crate::store::open_store(&home.join("coven.sqlite3")).map_err(failed)?;
    std::fs::write(
        home.join("familiars.toml"),
        format!(
            "[[familiar]]\nid = \"{FAMILIAR}\"\ndisplay_name = \"Scout\"\nrole = \"Research\"\ndescription = \"Finds things.\"\npronouns = \"they/them\"\n"
        ),
    )
    .map_err(failed)?;
    let declarations = home.join("familiars").join(FAMILIAR);
    std::fs::create_dir_all(&declarations).map_err(failed)?;
    for (file, text) in [
        ("IDENTITY.md", "# IDENTITY.md - Scout\n- **Name:** Scout\n"),
        ("SOUL.md", "# SOUL\nResearch familiar.\n"),
        ("ward.toml", WARD),
    ] {
        std::fs::write(declarations.join(file), text).map_err(failed)?;
    }
    let ledger = |request: Value, at: DateTime<Utc>| {
        let (status, response) =
            crate::familiar_ledger::route(&request, &conn, home, CommandAuthority::OwnerLocal, at);
        (status == 200)
            .then_some(response.result)
            .flatten()
            .ok_or("conformance suite execution failed")
    };
    let registered = ledger(
        json!({
            "action": crate::familiar_ledger::REGISTER,
            "adoptionKey": "adopt:conformance:register",
            "familiarId": FAMILIAR,
        }),
        first_at - Duration::minutes(10),
    )?;
    let stale_revision_id = registered["revision"]["revisionId"]
        .as_str()
        .ok_or("conformance suite execution failed")?
        .to_owned();
    // A changed soul, once adopted, supersedes the first revision.
    std::fs::write(
        declarations.join("SOUL.md"),
        "# SOUL\nResearch and teach.\n",
    )
    .map_err(failed)?;
    ledger(
        json!({
            "action": crate::familiar_ledger::ADOPT,
            "adoptionKey": "adopt:conformance:adopt",
            "familiarId": FAMILIAR,
            "expectedRevisionId": stale_revision_id,
        }),
        first_at - Duration::minutes(5),
    )?;
    let head = crate::familiar_ledger::live_head(&conn, FAMILIAR)
        .map_err(failed)?
        .ok_or("conformance suite execution failed")?;

    let project = projects.join("project");
    std::fs::create_dir_all(&project).map_err(failed)?;
    let path = |path: &Path| path.display().to_string();
    Ok(Fixture {
        home: home.to_path_buf(),
        root_id: head.root.root_id,
        revision_id: head.head.revision_id,
        stale_revision_id,
        project: path(&project),
        inside_home: path(&declarations),
        missing_project: path(&projects.join("missing")),
        conn,
    })
}
