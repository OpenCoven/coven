//! Owner-local operator retry of a run held after a failure, through the
//! daemon's `attempt.retry.v1` control action. Never opens the store or starts
//! a daemon, and never prints daemon error text.
use anyhow::{anyhow, bail, Result};
use coven_client::{DaemonClient, DaemonEndpoint};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::attempt_retry::{NOTE_MAX_CHARS, RETRY_ACTION};
use super::contract::types::RunId;
use super::lease_cli::valid_id;

const SCHEMA: &str = "coven.automations.retry.v1";
/// Said wherever the daemon may have opened the attempt without the command
/// learning it. Running the command again stays safe: the attempt it names is
/// then no longer the latest, or the new one is still live.
const OUTCOME_UNKNOWN: &str =
    "its outcome is unknown, so check `coven automations attempts <run-id>` before running it again";

/// How the run's latest attempt settled. The daemon opens the next attempt only
/// when the run's latest attempt matches it, so a retry never lands on an
/// attempt the operator did not mean. Ambiguous work is recovered, not retried.
#[derive(Clone, Copy, Debug, Eq, PartialEq, clap::ValueEnum)]
pub(crate) enum PriorDisposition {
    #[value(name = "failed")]
    Failed,
    #[value(name = "timed_out")]
    TimedOut,
    #[value(name = "cancelled")]
    Cancelled,
}

impl PriorDisposition {
    fn as_str(self) -> &'static str {
        match self {
            Self::Failed => "failed",
            Self::TimedOut => "timed_out",
            Self::Cancelled => "cancelled",
        }
    }
}

/// The retry the daemon opened: ids, numbers and times only.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Retried {
    run_id: String,
    occurrence_id: String,
    attempt_id: String,
    attempt_number: i64,
    prior_disposition: String,
    status: String,
    retried_at: String,
}

pub(crate) fn run(
    run_id: &str,
    prior_attempt: u16,
    prior_disposition: PriorDisposition,
    note: Option<&str>,
    as_json: bool,
) -> Result<()> {
    let request = request(
        run_id,
        prior_attempt,
        prior_disposition,
        note,
        &adoption_key(),
    )?;
    let home = crate::paths::coven_home_dir()
        .map_err(|_| anyhow!("cannot resolve Coven home for automation retry"))?;
    let endpoint = DaemonEndpoint::discover(home).map_err(|_| {
        anyhow!("owner-local Coven daemon unavailable; check `coven daemon status`")
    })?;
    if !endpoint.is_owner_local() {
        bail!("automation retry requires owner-local daemon IPC");
    }
    let response = DaemonClient::new(endpoint)
        .request_json("POST", "/api/v1/actions", Some(&request))
        .map_err(|_| {
            anyhow!(
                "automation retry got no answer from the owner-local daemon (check `coven daemon \
                 status`); {OUTCOME_UNKNOWN}"
            )
        })?;
    // Never propagate daemon error text: it can include database paths or logs.
    if response.status != 200 {
        if let Some(message) = known_refusal(response.status, &response.body) {
            bail!(message);
        }
        bail!(
            "automation retry was refused by the daemon (HTTP {})",
            response.status
        );
    }
    let response: Value = serde_json::from_slice(&response.body)
        .map_err(|_| anyhow!("automation retry response is invalid; {OUTCOME_UNKNOWN}"))?;
    let output = project(&response, run_id, prior_attempt, prior_disposition)?;
    print!("{}", render(&output, as_json)?);
    Ok(())
}

/// Serializes only the checked DTO; never daemon reasons or arbitrary fields.
fn render(output: &Value, as_json: bool) -> Result<String> {
    let heading = if as_json {
        ""
    } else {
        "Automations retry (owner-local)\n"
    };
    Ok(format!(
        "{heading}{}\n",
        serde_json::to_string_pretty(output)?
    ))
}

/// A fresh key for each invocation. The daemon stores a refusal under its key
/// and replays it, so a reused key would repeat a refusal whose cause has
/// cleared. Naming the prior attempt already keeps a repeated command from
/// opening a second attempt: once one opens, the named attempt is no longer
/// the latest.
fn adoption_key() -> String {
    format!("cli-retry-{}", uuid::Uuid::new_v4().simple())
}

fn request(
    run_id: &str,
    prior_attempt: u16,
    prior_disposition: PriorDisposition,
    note: Option<&str>,
    adoption_key: &str,
) -> Result<Value> {
    if RunId::new(run_id.to_owned()).is_err()
        || prior_attempt == 0
        || note.is_some_and(|note| {
            note.trim().is_empty()
                || note.chars().count() > NOTE_MAX_CHARS
                || note.chars().any(char::is_control)
        })
    {
        bail!("automation retry arguments are invalid");
    }
    let mut request = json!({
        "action": RETRY_ACTION,
        "adoptionKey": adoption_key,
        "runId": run_id,
        "priorAttemptNumber": prior_attempt,
        "priorDisposition": prior_disposition.as_str(),
    });
    if let Some(note) = note {
        request["note"] = json!(note);
    }
    Ok(request)
}

/// A typed refusal an operator can act on, recognized by status, code and
/// action together. Its message is still never shown: an illegal transition
/// is matched against the daemon's known reasons only to pick which fixed
/// sentence names the condition that refused it (coven#1793).
fn known_refusal(status: u16, body: &[u8]) -> Option<&'static str> {
    let body: Value = serde_json::from_slice(body).ok()?;
    if body["action"] != RETRY_ACTION {
        return None;
    }
    match (status, body["error"]["code"].as_str()?) {
        (404, "NOT_FOUND") => Some("automation run was not found"),
        (403, "AUTHORITY_REQUIRED") => {
            Some("a Runtime Authority run cannot be retried by this command")
        }
        (422, "RETRY_DISPOSITION_INVALID") => Some(
            "the run's latest attempt is not the attempt and disposition named; check `coven automations attempts <run-id>`",
        ),
        (422, "AMBIGUOUS_RETRY_FORBIDDEN") => {
            Some("the run's latest attempt is ambiguous; recover its occurrence instead")
        }
        (409, "CANCEL_PENDING") => {
            Some("the run has an unresolved stop, so it cannot be retried")
        }
        (504, "DEADLINE_EXCEEDED") => Some("the run's deadline has passed; it cannot be retried"),
        (422, "ILLEGAL_TRANSITION") => Some(illegal_transition(
            body["error"]["message"].as_str().unwrap_or_default(),
        )),
        _ => None,
    }
}

/// Said when an illegal transition's reason is not one this command knows.
const NOT_HELD: &str = "the run is not held for a retry: it needs to be running, with a latest \
    attempt that failed, timed out or was cancelled, an open deadline, an attempt left, and an \
    active routine that is not quarantined";

/// The fixed sentence for each reason `attempt.retry.v1` refuses an illegal
/// transition with. Ids, states and counts in the daemon's reason are matched
/// by shape and never copied.
fn illegal_transition(reason: &str) -> &'static str {
    let shaped = |prefix: &str, suffix: &str| {
        reason.len() > prefix.len() + suffix.len()
            && reason.starts_with(prefix)
            && reason.ends_with(suffix)
    };
    if shaped(
        "the run is `",
        "`; only a run held after a failure can be retried before its deadline",
    ) {
        "the run is not held for a retry: it is not running"
    } else if reason == "the run has no occurrence to plan" {
        "the run is not held for a retry: it has no occurrence to plan"
    } else if reason == "the run has no attempt" {
        "the run is not held for a retry: it has no attempt"
    } else if shaped("attempt #", "`") && reason.contains(" is still `") {
        "the run is not held for a retry: its latest attempt has not failed, timed out or been \
         cancelled"
    } else if shaped("the run has used all ", " attempts") {
        "the run is not held for a retry: it has used all its attempts"
    } else if reason
        == "the run has no original deadline, so it cannot be held for an operator retry"
    {
        "the run is not held for a retry: it has no original deadline"
    } else if shaped("routine `", "` is not active, so it cannot be retried") {
        "the run's routine is not active, so the run cannot be retried"
    } else if reason == "the routine is quarantined; explicitly unquarantine it before retrying" {
        "the run's routine is quarantined; explicitly unquarantine it before retrying"
    } else if reason == "the run's occurrence is not held for a retry" {
        "the run's occurrence is not held for a retry"
    } else {
        NOT_HELD
    }
}

fn project(
    response: &Value,
    run_id: &str,
    prior_attempt: u16,
    prior_disposition: PriorDisposition,
) -> Result<Value> {
    let invalid = || anyhow!("automation retry response is invalid; {OUTCOME_UNKNOWN}");
    if response["ok"] != true
        || response["accepted"] != true
        || response["status"] != "completed"
        || response["action"] != RETRY_ACTION
        || response["event"]["action"] != RETRY_ACTION
        || response["event"]["payload"] != response["result"]
    {
        return Err(invalid());
    }
    let retried: Retried =
        serde_json::from_value(response["result"].clone()).map_err(|_| invalid())?;
    if retried.run_id != run_id
        || !valid_id(&retried.occurrence_id)
        || !valid_id(&retried.attempt_id)
        || retried.attempt_number != i64::from(prior_attempt) + 1
        || retried.prior_disposition != prior_disposition.as_str()
        || retried.status != "planned"
        || retried.retried_at.len() > 64
        || chrono::DateTime::parse_from_rfc3339(&retried.retried_at).is_err()
    {
        return Err(invalid());
    }
    Ok(json!({"schemaVersion": SCHEMA, "retry": retried}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A store with an active routine, `daily`, whose run `r-hold` is held
    /// after its first attempt failed, with its deadline far ahead.
    fn held_store() -> (tempfile::TempDir, rusqlite::Connection) {
        store("failed")
    }

    /// The same store with the run's only attempt settled `attempt_state`.
    fn store(attempt_state: &str) -> (tempfile::TempDir, rusqlite::Connection) {
        let temp = tempfile::tempdir().unwrap();
        let conn = crate::store::open_store(&temp.path().join("store.sqlite")).unwrap();
        let definition = crate::automations::definition::RoutineDefinition::from_json(&json!({
            "schemaVersion": 1, "id": "daily", "name": "daily", "status": "ACTIVE",
            "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc", "misfire": "latest",
            "overlap": "forbid", "timeoutMinutes": 30, "runtime": "coven-code",
            "cwd": "/work/project", "prompt": "private sentinel"
        }))
        .unwrap();
        crate::automations::store::insert_definition(&conn, &definition).unwrap();
        conn.execute_batch(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, kind, state, attempt, lease_owner,
                 lease_expires_at, created_at, updated_at)
             VALUES ('o-hold', 'daily', '2026-10-17T09:00:00.000Z', 'manual', 'claimed', 1,
                     'operator', '2099-01-01T00:00:00.000Z', '2026-10-17T09:00:00.000Z',
                     '2026-10-17T09:00:00.000Z');
             INSERT INTO automation_runs
                (id, automation_id, occurrence_id, runtime, status, started_at, timeout_at)
             VALUES ('r-hold', 'daily', 'o-hold', 'coven-code', 'running',
                     '2026-10-17T09:00:00.000Z', '2099-01-01T03:00:00.000Z');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO automation_attempts
                (id, run_id, occurrence_id, attempt_number, adoption_key,
                 occurrence_fence_generation, state, retry_classification, not_before,
                 state_reason, opened_at)
             VALUES ('a-hold', 'r-hold', 'o-hold', 1, 'r-hold-key', 1, ?1, 'initial',
                     '2026-10-17T09:00:00.000Z', 'private sentinel',
                     '2026-10-17T09:00:00.000Z')",
            [attempt_state],
        )
        .unwrap();
        (temp, conn)
    }

    /// Routes the command's own request through the daemon's control plane.
    fn route(
        conn: &rusqlite::Connection,
        run_id: &str,
        prior_attempt: u16,
        prior_disposition: PriorDisposition,
        note: Option<&str>,
    ) -> (u16, Value) {
        let request = request(
            run_id,
            prior_attempt,
            prior_disposition,
            note,
            &adoption_key(),
        )
        .unwrap();
        let (status, response) =
            crate::control_plane::route_action(request, conn, &crate::api::NoopSessionRuntime);
        (status, serde_json::to_value(response).unwrap())
    }

    #[test]
    fn retry_opens_the_next_attempt_and_prints_only_its_ids() {
        let (_temp, conn) = held_store();
        let (status, response) = route(
            &conn,
            "r-hold",
            1,
            PriorDisposition::Failed,
            Some("private sentinel"),
        );
        assert_eq!(status, 200, "{response}");
        let output = project(&response, "r-hold", 1, PriorDisposition::Failed).unwrap();
        let retried_at = output["retry"]["retriedAt"].clone();
        assert!(retried_at.as_str().is_some());
        assert_eq!(
            output,
            json!({"schemaVersion": "coven.automations.retry.v1", "retry": {
                "runId": "r-hold", "occurrenceId": "o-hold", "attemptId": "attempt-r-hold-2",
                "attemptNumber": 2, "priorDisposition": "failed", "status": "planned",
                "retriedAt": retried_at
            }})
        );
        assert!(!output.to_string().contains("private sentinel"));
        let printed = render(&output, true).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&printed).unwrap(), output);
        assert_eq!(
            render(&output, false).unwrap(),
            format!("Automations retry (owner-local)\n{printed}")
        );
        let (occurrence, attempt): (String, String) = conn
            .query_row(
                "SELECT o.state, a.state FROM automation_occurrences o
                 JOIN automation_attempts a ON a.occurrence_id = o.id
                 WHERE o.id = 'o-hold' AND a.attempt_number = 2",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (occurrence.as_str(), attempt.as_str()),
            ("planned", "adopted")
        );
        // Running the same command again finds the new attempt live, so it
        // opens nothing.
        let (status, response) = route(&conn, "r-hold", 1, PriorDisposition::Failed, None);
        assert_eq!(
            known_refusal(status, &serde_json::to_vec(&response).unwrap()),
            Some(
                "the run is not held for a retry: its latest attempt has not failed, timed out or \
                 been cancelled"
            )
        );
        let attempts: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM automation_attempts WHERE run_id = 'r-hold'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(attempts, 2);
    }

    #[test]
    fn each_typed_refusal_from_the_real_handler_gets_a_fixed_message() {
        let cases: [(&str, &str, &str, PriorDisposition, &str); 9] = [
            (
                "r-missing",
                "failed",
                "",
                PriorDisposition::Failed,
                "automation run was not found",
            ),
            (
                "r-authority",
                "failed",
                "INSERT INTO automation_occurrences
                    (id, automation_id, scheduled_for, kind, state, attempt, lease_owner,
                     lease_expires_at, created_at, updated_at)
                 VALUES ('o-authority', 'daily', '2026-10-16T09:00:00.000Z', 'manual',
                         'claimed', 1, 'operator', '2099-01-01T00:00:00.000Z',
                         '2026-10-16T09:00:00.000Z', '2026-10-16T09:00:00.000Z');
                 INSERT INTO automation_runs
                    (id, automation_id, occurrence_id, runtime, status, started_at, timeout_at,
                     authority_profile)
                 VALUES ('r-authority', 'daily', 'o-authority', 'coven-code', 'running',
                         '2026-10-16T09:00:00.000Z', '2099-01-01T03:00:00.000Z',
                         'coven.automations.authority.v1');",
                PriorDisposition::Failed,
                "a Runtime Authority run cannot be retried by this command",
            ),
            (
                "r-hold",
                "failed",
                "",
                PriorDisposition::TimedOut,
                "the run's latest attempt is not the attempt and disposition named; check `coven automations attempts <run-id>`",
            ),
            (
                "r-hold",
                "ambiguous",
                "",
                PriorDisposition::Failed,
                "the run's latest attempt is ambiguous; recover its occurrence instead",
            ),
            (
                "r-hold",
                "failed",
                "INSERT INTO automation_stop_fences
                    (run_id, session_id, owner, acquired_at, execution_expires_at)
                 VALUES ('r-hold', 's-hold', 'timeout', '2026-10-17T09:00:00.000Z',
                         '2099-01-01T00:00:00.000Z');",
                PriorDisposition::Failed,
                "the run has an unresolved stop, so it cannot be retried",
            ),
            (
                "r-hold",
                "failed",
                "UPDATE automation_runs SET timeout_at = '2026-10-17T09:30:00.000Z';",
                PriorDisposition::Failed,
                "the run's deadline has passed; it cannot be retried",
            ),
            (
                "r-hold",
                "failed",
                "UPDATE automation_definitions SET lifecycle_state = 'paused';",
                PriorDisposition::Failed,
                "the run's routine is not active, so the run cannot be retried",
            ),
            (
                "r-hold",
                "failed",
                "INSERT INTO automation_retry_state
                    (automation_id, consecutive_exhaustions, quarantined_at, failure_class,
                     reason, updated_at)
                 VALUES ('daily', 1, '2026-10-17T09:10:00.000Z', 'runtime_unavailable',
                         'retry exhausted', '2026-10-17T09:10:00.000Z');",
                PriorDisposition::Failed,
                "the run's routine is quarantined; explicitly unquarantine it before retrying",
            ),
            (
                "r-hold",
                "failed",
                "UPDATE automation_runs SET status = 'failed';",
                PriorDisposition::Failed,
                "the run is not held for a retry: it is not running",
            ),
        ];
        for (run_id, attempt_state, change, prior_disposition, message) in cases {
            let (_temp, conn) = store(attempt_state);
            conn.execute_batch(change).unwrap();
            let (status, response) = route(&conn, run_id, 1, prior_disposition, None);
            assert_eq!(
                known_refusal(status, &serde_json::to_vec(&response).unwrap()),
                Some(message),
                "{change}: {response}"
            );
        }
    }

    #[test]
    fn each_illegal_transition_reason_gets_its_own_fixed_sentence() {
        let max = super::super::runner::MAX_ATTEMPT_NUMBER;
        for (reason, sentence) in [
            (
                "the run is `succeeded`; only a run held after a failure can be retried before \
                 its deadline"
                    .to_owned(),
                "the run is not held for a retry: it is not running",
            ),
            (
                "the run has no occurrence to plan".to_owned(),
                "the run is not held for a retry: it has no occurrence to plan",
            ),
            (
                "the run has no attempt".to_owned(),
                "the run is not held for a retry: it has no attempt",
            ),
            (
                "attempt #2 is still `dispatching`".to_owned(),
                "the run is not held for a retry: its latest attempt has not failed, timed out \
                 or been cancelled",
            ),
            (
                format!("the run has used all {max} attempts"),
                "the run is not held for a retry: it has used all its attempts",
            ),
            (
                "the run has no original deadline, so it cannot be held for an operator retry"
                    .to_owned(),
                "the run is not held for a retry: it has no original deadline",
            ),
            (
                "routine `private sentinel` is not active, so it cannot be retried".to_owned(),
                "the run's routine is not active, so the run cannot be retried",
            ),
            (
                "the routine is quarantined; explicitly unquarantine it before retrying".to_owned(),
                "the run's routine is quarantined; explicitly unquarantine it before retrying",
            ),
            (
                "the run's occurrence is not held for a retry".to_owned(),
                "the run's occurrence is not held for a retry",
            ),
            // Unknown or malformed reasons keep the general sentence.
            ("private sentinel".to_owned(), NOT_HELD),
            (
                "routine `` is not active, so it cannot be retried".to_owned(),
                NOT_HELD,
            ),
            ("".to_owned(), NOT_HELD),
        ] {
            let body = serde_json::to_vec(&json!({
                "action": RETRY_ACTION,
                "error": {"code": "ILLEGAL_TRANSITION", "message": reason}
            }))
            .unwrap();
            let refusal = known_refusal(422, &body);
            assert_eq!(refusal, Some(sentence), "{reason}");
            assert!(!refusal.unwrap().contains("private sentinel"));
        }
        let no_message = serde_json::to_vec(&json!({
            "action": RETRY_ACTION, "error": {"code": "ILLEGAL_TRANSITION"}
        }))
        .unwrap();
        assert_eq!(known_refusal(422, &no_message), Some(NOT_HELD));
    }

    #[test]
    fn only_typed_retry_refusals_get_their_own_message() {
        let body = |action: &str, code: &str| {
            serde_json::to_vec(&json!({
                "action": action,
                "error": {"code": code, "message": "private sentinel"}
            }))
            .unwrap()
        };
        assert_eq!(
            known_refusal(404, &body(RETRY_ACTION, "NOT_FOUND")),
            Some("automation run was not found")
        );
        for (status, refusal) in [
            (404, body("coven.automations.run.get.v1", "NOT_FOUND")),
            (410, body(RETRY_ACTION, "NOT_FOUND")),
            (409, body(RETRY_ACTION, "ADOPTION_REPLAY_MISMATCH")),
            (400, body(RETRY_ACTION, "VALIDATION_FAILED")),
            (500, body(RETRY_ACTION, "INTERNAL")),
            (404, b"private sentinel".to_vec()),
        ] {
            assert_eq!(known_refusal(status, &refusal), None, "{status}");
        }
    }

    #[test]
    fn projection_accepts_only_the_retry_it_asked_for() {
        let (_temp, conn) = held_store();
        let (_, response) = route(&conn, "r-hold", 1, PriorDisposition::Failed, None);
        assert!(project(&response, "r-hold", 1, PriorDisposition::Failed).is_ok());
        assert!(project(&response, "r-other", 1, PriorDisposition::Failed).is_err());
        assert!(project(&response, "r-hold", 2, PriorDisposition::Failed).is_err());
        assert!(project(&response, "r-hold", 1, PriorDisposition::Cancelled).is_err());
        let forged = |pointer: &str, value: Value| {
            let mut forged = response.clone();
            *forged.pointer_mut(pointer).unwrap() = value;
            forged
        };
        for forged in [
            forged("/ok", json!(false)),
            forged("/accepted", json!(false)),
            forged("/status", json!("rejected")),
            forged("/action", json!("coven.automations.run.get.v1")),
            forged("/event/action", json!("coven.automations.run.get.v1")),
            forged("/event/payload/attemptId", json!("attempt-r-hold-3")),
            forged("/result/status", json!("running")),
            forged("/result/occurrenceId", json!("")),
            forged("/result/attemptId", json!("private\nsentinel")),
            forged("/result/retriedAt", json!("soon")),
            forged("/result", json!({"note": "private sentinel"})),
        ] {
            assert!(
                project(&forged, "r-hold", 1, PriorDisposition::Failed).is_err(),
                "{forged}"
            );
        }
        let mut extra = response.clone();
        extra["result"]["note"] = json!("private sentinel");
        assert!(project(&extra, "r-hold", 1, PriorDisposition::Failed).is_err());
    }

    #[test]
    fn requests_are_checked_before_they_reach_the_daemon() {
        assert_eq!(
            request(
                "r-hold",
                3,
                PriorDisposition::TimedOut,
                Some("after the outage"),
                "cli-retry-key"
            )
            .unwrap(),
            json!({
                "action": RETRY_ACTION, "adoptionKey": "cli-retry-key", "runId": "r-hold",
                "priorAttemptNumber": 3, "priorDisposition": "timed_out",
                "note": "after the outage"
            })
        );
        let long = "n".repeat(NOTE_MAX_CHARS + 1);
        for (run_id, prior_attempt, note) in [
            ("", 1, None),
            (" r-hold", 1, None),
            ("r\u{7}hold", 1, None),
            ("r hold", 1, None),
            ("r/hold", 1, None),
            ("r-hold", 1, Some("")),
            ("r-hold", 1, Some("  ")),
            ("r-hold", 0, None),
            ("r-hold", 1, Some(long.as_str())),
            ("r-hold", 1, Some("two\nlines")),
        ] {
            assert!(
                request(run_id, prior_attempt, PriorDisposition::Failed, note, "key").is_err(),
                "{run_id:?} {prior_attempt} {note:?}"
            );
        }
        let (first, second) = (adoption_key(), adoption_key());
        assert_ne!(first, second);
        for key in [first, second] {
            assert!(super::super::contract::types::AdoptionKey::new(key).is_ok());
        }
    }
}
