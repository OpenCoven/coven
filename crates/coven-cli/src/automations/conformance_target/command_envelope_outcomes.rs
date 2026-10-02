//! Portable `coven.automations.v1` command-envelope outcomes (coven#1054).
//! Each case sends spec command envelopes, in order, to one durable store
//! through the producer's `coven.automations.command.v1` router, and closes
//! and reopens that store where the case restarts. Only the router commits,
//! replays or refuses; this suite compares what it answers.

use std::collections::BTreeSet;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::Connection;
use serde::Deserialize;
use serde_json::{json, Value};

use super::{scheduler_conformance_tempdir, valid_case_id, MAX_CASES};
use crate::automations::command_envelope;
use crate::automations::contract::error::ErrorCode;
use crate::automations::contract::types::{AdoptionKey, CommandRequest};

const SCHEMA: &str = "coven.automations.command-envelope-outcomes-vectors.v1";
const FIRST_RECORDED_AT: &str = "2026-08-30T09:00:00.000Z";
const MAX_STEPS: usize = 32;

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

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Step {
    /// Send one envelope and compare the spec response.
    Send { envelope: Value, expect: Expect },
    /// Close the store and open it again.
    Restart,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Expect {
    outcome: Outcome,
    #[serde(default)]
    revision: Option<u64>,
    #[serde(default)]
    error_code: Option<ErrorCode>,
    /// The earlier step whose `result` and `eventRef` this replay returns.
    #[serde(default)]
    replays: Option<usize>,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Outcome {
    Committed,
    Replayed,
    Rejected,
}

impl Outcome {
    fn wire(self) -> &'static str {
        match self {
            Self::Committed => "committed",
            Self::Replayed => "replayed",
            Self::Rejected => "rejected",
        }
    }
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

/// Every envelope is a valid spec command, a case opens by sending one, a
/// rejection names its code, and a replay names an earlier committed send of
/// the same envelope.
fn valid_steps(case: &Case) -> bool {
    if case.steps.is_empty()
        || case.steps.len() > MAX_STEPS
        || !matches!(case.steps[0], Step::Send { .. })
    {
        return false;
    }
    case.steps.iter().enumerate().all(|(index, step)| {
        let Step::Send { envelope, expect } = step else {
            return true;
        };
        let valid_envelope = serde_json::from_value::<CommandRequest>(envelope.clone()).is_ok()
            && envelope["adoptionKey"]
                .as_str()
                .is_some_and(|key| AdoptionKey::new(key.to_owned()).is_ok());
        let replay_names_its_original = match (expect.outcome, expect.replays) {
            (Outcome::Replayed, Some(earlier)) => {
                earlier < index
                    && matches!(
                        &case.steps[earlier],
                        Step::Send { envelope: original, expect: first }
                            if original == envelope && first.outcome == Outcome::Committed
                    )
            }
            (Outcome::Replayed, None) => false,
            (_, replays) => replays.is_none(),
        };
        valid_envelope
            && replay_names_its_original
            && (expect.outcome == Outcome::Rejected) == expect.error_code.is_some()
    })
}

fn case_matches(case: &Case) -> Result<bool, &'static str> {
    let home = scheduler_conformance_tempdir()?;
    crate::daemon::ensure_private_coven_home(home.path())
        .map_err(|_| "conformance suite execution failed")?;
    let store = home.path().join("coven.sqlite3");
    let open =
        || crate::store::open_store(&store).map_err(|_| "conformance suite execution failed");
    let first_recorded_at = DateTime::parse_from_rfc3339(FIRST_RECORDED_AT)
        .map_err(|_| "conformance suite execution failed")?
        .with_timezone(&Utc);

    let mut conn: Connection = open()?;
    let mut responses: Vec<Option<Value>> = Vec::with_capacity(case.steps.len());
    let mut passed = true;
    for (index, step) in case.steps.iter().enumerate() {
        let Step::Send { envelope, expect } = step else {
            drop(conn);
            conn = open()?;
            responses.push(None);
            continue;
        };
        let minutes = i64::try_from(index).map_err(|_| "conformance vector is invalid")?;
        let recorded_at = (first_recorded_at + Duration::minutes(minutes))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let (_, response) = command_envelope::route(
            &json!({ "action": command_envelope::ACTION, "envelope": envelope }),
            &conn,
            &crate::api::NoopSessionRuntime,
            &recorded_at,
        );
        let body = response.result.unwrap_or(Value::Null);
        let replay_matches = expect.replays.is_none_or(|earlier| {
            responses[earlier].as_ref().is_some_and(|original| {
                original["result"] == body["result"] && original["eventRef"] == body["eventRef"]
            })
        });
        let error_code = serde_json::from_value::<ErrorCode>(body["error"]["code"].clone()).ok();
        passed &= body["outcome"] == expect.outcome.wire()
            && error_code == expect.error_code
            && expect
                .revision
                .is_none_or(|revision| body["revision"] == json!(revision))
            && replay_matches;
        responses.push(Some(body));
    }
    Ok(passed)
}
