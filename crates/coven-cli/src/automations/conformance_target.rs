use std::collections::BTreeSet;

use chrono::{DateTime, Duration, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::capability_negotiation::{negotiate_definition, DefinitionNegotiation};
use super::runs::{record_run_finish, record_run_start, RunFinish, RunStart};

const TARGET_CAPABILITY_SCHEMA_VERSION: &str = "coven.automations.conformance-target-capability.v1";
const SUITE_REQUEST_SCHEMA_VERSION: &str = "coven.automations.conformance-suite-request.v1";
const SUITE_RESULT_SCHEMA_VERSION: &str = "coven.automations.conformance-suite-result.v1";
const CAPABILITY_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.capability-negotiation-vectors.v1";
const RUN_TERMINAL_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.run-terminal-monotonicity-vectors.v1";
const STRUCTURAL_PROFILE: &str = "structural";
const MAX_CASES: usize = 128;

pub const CAPABILITY_NEGOTIATION_SUITE: &str = "capability-negotiation";
pub const RUN_TERMINAL_MONOTONICITY_SUITE: &str = "run-terminal-monotonicity";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetCapability {
    schema_version: &'static str,
    profiles: Vec<TargetProfileCapability>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetProfileCapability {
    profile: &'static str,
    suites: Vec<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSuiteResult {
    schema_version: &'static str,
    suite_id: String,
    pub status: TargetSuiteStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetSuiteStatus {
    Passed,
    Failed,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetSuiteRequest {
    schema_version: String,
    profile: String,
    suite_id: String,
    protocol_artifact: Value,
    subject_artifact: Value,
    vector: Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilityVectorSet {
    schema_version: String,
    cases: Vec<CapabilityVectorCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CapabilityVectorCase {
    case_id: String,
    definition: Value,
    expected: ExpectedNegotiation,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
enum ExpectedNegotiation {
    Supported,
    Unsupported { variant: String },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunTerminalVectorSet {
    schema_version: String,
    cases: Vec<RunTerminalVectorCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RunTerminalVectorCase {
    case_id: String,
    first_status: TerminalRunStatus,
    replay_status: TerminalRunStatus,
    expected: ExpectedRunTerminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TerminalRunStatus {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
}

impl TerminalRunStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedRunTerminal {
    first_committed: bool,
    replay_committed: bool,
    final_status: TerminalRunStatus,
}

#[must_use]
pub fn capability() -> TargetCapability {
    TargetCapability {
        schema_version: TARGET_CAPABILITY_SCHEMA_VERSION,
        profiles: vec![TargetProfileCapability {
            profile: STRUCTURAL_PROFILE,
            suites: vec![
                CAPABILITY_NEGOTIATION_SUITE,
                RUN_TERMINAL_MONOTONICITY_SUITE,
            ],
        }],
    }
}

pub fn evaluate(request: &Value) -> Result<TargetSuiteResult, &'static str> {
    let request: TargetSuiteRequest =
        serde_json::from_value(request.clone()).map_err(|_| "conformance request is invalid")?;
    if request.schema_version != SUITE_REQUEST_SCHEMA_VERSION
        || request.profile != STRUCTURAL_PROFILE
    {
        return Err("conformance suite is unsupported");
    }
    if !request.protocol_artifact.is_object() || !request.subject_artifact.is_object() {
        return Err("conformance request is invalid");
    }

    let all_passed = match request.suite_id.as_str() {
        CAPABILITY_NEGOTIATION_SUITE => evaluate_capability_negotiation(&request.vector)?,
        RUN_TERMINAL_MONOTONICITY_SUITE => evaluate_run_terminal_monotonicity(&request.vector)?,
        _ => return Err("conformance suite is unsupported"),
    };
    result_for(&request.suite_id, &request.vector, all_passed)
}

fn evaluate_capability_negotiation(vector: &Value) -> Result<bool, &'static str> {
    let vectors: CapabilityVectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != CAPABILITY_VECTOR_SCHEMA_VERSION
        || vectors.cases.is_empty()
        || vectors.cases.len() > MAX_CASES
    {
        return Err("conformance vector is invalid");
    }

    let mut case_ids = BTreeSet::new();
    for case in &vectors.cases {
        if !valid_case_id(&case.case_id)
            || !case_ids.insert(&case.case_id)
            || !case.definition.is_object()
        {
            return Err("conformance vector is invalid");
        }
    }

    Ok(vectors.cases.iter().all(capability_case_matches))
}

fn evaluate_run_terminal_monotonicity(vector: &Value) -> Result<bool, &'static str> {
    let vectors: RunTerminalVectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != RUN_TERMINAL_VECTOR_SCHEMA_VERSION
        || vectors.cases.is_empty()
        || vectors.cases.len() > MAX_CASES
    {
        return Err("conformance vector is invalid");
    }

    let mut case_ids = BTreeSet::new();
    for case in &vectors.cases {
        if !valid_case_id(&case.case_id) || !case_ids.insert(&case.case_id) {
            return Err("conformance vector is invalid");
        }
    }

    let conn = Connection::open_in_memory().map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch(super::store::AUTOMATION_DEFINITIONS_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch(super::occurrences::AUTOMATION_OCCURRENCES_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch(super::runs::AUTOMATION_RUNS_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;
    let now = DateTime::<Utc>::from_timestamp(0, 0).ok_or("conformance suite execution failed")?;

    for (index, case) in vectors.cases.iter().enumerate() {
        if !run_terminal_case_matches(&conn, case, index, now)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn run_terminal_case_matches(
    conn: &Connection,
    case: &RunTerminalVectorCase,
    index: usize,
    now: DateTime<Utc>,
) -> Result<bool, &'static str> {
    let run_id = format!("conformance-run-{index}");
    record_run_start(
        conn,
        &run_id,
        RunStart {
            automation_id: "conformance-automation",
            occurrence_id: None,
            authority_profile: None,
            session_id: None,
            familiar_id: None,
            runtime: "coven-code",
            timeout_at: now + Duration::minutes(30),
        },
        now,
    )
    .map_err(|_| "conformance suite execution failed")?;
    let first_committed = record_run_finish(
        conn,
        &run_id,
        RunFinish {
            status: case.first_status.as_str(),
            exit_code: None,
            session_id: None,
            log_json: None,
            output_commit: None,
        },
        now,
    )
    .map_err(|_| "conformance suite execution failed")?;
    let replay_committed = record_run_finish(
        conn,
        &run_id,
        RunFinish {
            status: case.replay_status.as_str(),
            exit_code: None,
            session_id: None,
            log_json: None,
            output_commit: None,
        },
        now + Duration::seconds(1),
    )
    .map_err(|_| "conformance suite execution failed")?;
    let final_status = conn
        .query_row(
            "SELECT status FROM automation_runs WHERE id = ?1",
            [&run_id],
            |row| row.get::<_, String>(0),
        )
        .map_err(|_| "conformance suite execution failed")?;

    Ok(first_committed == case.expected.first_committed
        && replay_committed == case.expected.replay_committed
        && final_status == case.expected.final_status.as_str())
}

fn result_for(
    suite_id: &str,
    vector: &Value,
    all_passed: bool,
) -> Result<TargetSuiteResult, &'static str> {
    let evidence = if all_passed {
        let canonical = serde_jcs::to_vec(vector).map_err(|_| "conformance vector is invalid")?;
        let vector_digest: String = Sha256::digest(canonical)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let executed_cases = vector["cases"]
            .as_array()
            .ok_or("conformance vector is invalid")?
            .len();
        Some(json!({
            "executedCases": executed_cases,
            "passedCases": executed_cases,
            "vectorDigest": format!("sha256:{vector_digest}")
        }))
    } else {
        None
    };

    Ok(TargetSuiteResult {
        schema_version: SUITE_RESULT_SCHEMA_VERSION,
        suite_id: suite_id.to_owned(),
        status: if all_passed {
            TargetSuiteStatus::Passed
        } else {
            TargetSuiteStatus::Failed
        },
        evidence,
    })
}

fn capability_case_matches(case: &CapabilityVectorCase) -> bool {
    match (negotiate_definition(&case.definition), &case.expected) {
        (Ok(DefinitionNegotiation::Supported(_)), ExpectedNegotiation::Supported) => true,
        (
            Ok(DefinitionNegotiation::Unsupported(observed)),
            ExpectedNegotiation::Unsupported { variant },
        ) => observed.variant == *variant,
        _ => false,
    }
}

fn valid_case_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_alphanumeric()
                || (index > 0 && matches!(byte, b'.' | b'_' | b':' | b'@' | b'-'))
        })
}
