//! Mission commands under M0 (coven#1313). Each case sends mission commands,
//! in order, to one store that holds one registered familiar, whose first
//! revision has been superseded by a second. The clock advances one minute per
//! step. Only the mission executors commit, replay or refuse; this suite
//! compares what they answer and what a refusal leaves behind.
//!
//! M-17 and M-18 cover a submit's `scopes` (#1597): accepted, returned by the
//! view and bound by the adoption key, or refused before anything is recorded.
//!
//! M0 launches nothing, so the vectors that need a launched or still-queued
//! mission (M-07, M-08, M-10, M-12, M-13 and M-14) wait for Runtime Authority
//! (#857). M-16 is a transport rule, held by the transport authority tests.
//!
//! Values that exist only once the store does are written as whole-string
//! tokens:
//! - `{{rootId}}`, `{{revisionId}}` and `{{staleRevisionId}}` name the familiar;
//! - `{{project}}`, `{{insideHome}}` and `{{missingProject}}` name directories;
//! - `{{start:10m}}` and `{{start:-1m}}` are instants relative to the case's
//!   first step, so a repeated request is the same request;
//! - `{{bytes:N}}` is N bytes of text.

use std::collections::BTreeSet;
use std::path::Path;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

use super::command_envelope_outcomes::durable_state;
use super::{scheduler_conformance_tempdir, valid_case_id, MAX_CASES};
use crate::automations::contract::error::ErrorCode;
use crate::automations::contract::types::AdoptionKey;
use crate::automations::missions::{
    execute_mission_cancel, execute_mission_get, execute_mission_submit, mission_id_for,
    MissionExecution, CANCEL_ACTION, GET_ACTION, SUBMIT_ACTION,
};
use crate::automations::owner_grants::CommandAuthority;

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
    steps: Vec<Step>,
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
    expect: Expect,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
enum Command {
    #[serde(rename = "mission.submit.v1")]
    Submit,
    #[serde(rename = "mission.get.v1")]
    Get,
    #[serde(rename = "mission.cancel.v1")]
    Cancel,
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
            Command::Get => step.adoption_key.is_none() && mission_valid && step.payload.is_none(),
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
}

/// Any failure of the suite's own setup or execution, as the target reports it.
fn failed<E>(_: E) -> &'static str {
    "conformance suite execution failed"
}

/// The case's store, familiar and directories.
struct Fixture {
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

    let mut answers: Vec<(Outcome, Value)> = Vec::with_capacity(case.steps.len());
    let mut passed = true;
    for (index, step) in case.steps.iter().enumerate() {
        let minutes = i64::try_from(index).map_err(|_| "conformance vector is invalid")?;
        let now = first_at + Duration::minutes(minutes);
        let before = durable_state(&fixture.conn)?;
        let (outcome, answer) = send(&fixture, step, first_at, now)?;
        let after = durable_state(&fixture.conn)?;
        let expect = &step.expect;

        let writes_match = expect.writes.is_none_or(|writes| {
            before.domain == after.domain
                && match writes {
                    Writes::Nothing => before.adoptions == after.adoptions,
                    Writes::Adoption => before.adoptions != after.adoptions,
                }
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
            (_, Command::Get) => &answer,
            _ => &answer["mission"],
        };
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
            && repeat_matches
            && writes_match;
        answers.push((outcome, answer));
    }
    Ok(passed)
}

/// Sends one step and returns its outcome with the result, or the error.
fn send(
    fixture: &Fixture,
    step: &Step,
    start: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(Outcome, Value), &'static str> {
    let authority = match step.caller {
        Caller::Owner => CommandAuthority::OwnerLocal,
        Caller::Other => CommandAuthority::Unauthenticated,
    };
    let mission_id = step.mission_of.as_deref().map(mission_id_for);
    let mut body = match &step.payload {
        Some(payload) => resolve(payload, fixture, start)?,
        None => json!({}),
    };
    body["action"] = json!(match step.command {
        Command::Submit => SUBMIT_ACTION,
        Command::Get => GET_ACTION,
        Command::Cancel => CANCEL_ACTION,
    });
    if let Some(key) = &step.adoption_key {
        body["adoptionKey"] = json!(key);
    }
    if let Some(mission_id) = mission_id {
        body["missionId"] = json!(mission_id);
    }
    let conn = &fixture.conn;
    let executed = match step.command {
        Command::Submit => execute_mission_submit(conn, authority, body, now),
        Command::Get => execute_mission_get(conn, authority, &body),
        Command::Cancel => {
            execute_mission_cancel(conn, &crate::api::NoopSessionRuntime, authority, body, now)
        }
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
        root_id: head.root.root_id,
        revision_id: head.head.revision_id,
        stale_revision_id,
        project: path(&project),
        inside_home: path(&declarations),
        missing_project: path(&projects.join("missing")),
        conn,
    })
}
