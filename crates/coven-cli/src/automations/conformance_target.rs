use std::collections::BTreeSet;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::capability_negotiation::{negotiate_definition, DefinitionNegotiation};
use super::contract::events::EventReducer;
use super::contract::types::{AutomationId, OccurrenceId};
use super::contract::{canonicalize, sha256_hex, AutomationDefinition, EventEnvelope};
use super::runs::{
    record_run_finish, record_run_start, RunFinish, RunStart, AUTOMATION_ATTEMPTS_SCHEMA_SQL,
};

const TARGET_CAPABILITY_SCHEMA_VERSION: &str = "coven.automations.conformance-target-capability.v1";
const SUITE_REQUEST_SCHEMA_VERSION: &str = "coven.automations.conformance-suite-request.v1";
const SUITE_RESULT_SCHEMA_VERSION: &str = "coven.automations.conformance-suite-result.v1";
const CAPABILITY_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.capability-negotiation-vectors.v1";
const ATTEMPT_TERMINAL_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.attempt-terminal-immutability-vectors.v1";
const DEFINITION_VALIDATION_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.definition-validation-vectors.v1";
const EVENT_REDUCER_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.event-reducer-determinism-vectors.v1";
const OCCURRENCE_FENCE_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.occurrence-fence-uniqueness-vectors.v1";
const RUN_TERMINAL_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.run-terminal-monotonicity-vectors.v1";
const STRUCTURAL_PROFILE: &str = "structural";
const MAX_CASES: usize = 128;

pub const CAPABILITY_NEGOTIATION_SUITE: &str = "capability-negotiation";
pub const ATTEMPT_TERMINAL_IMMUTABILITY_SUITE: &str = "attempt-terminal-immutability";
pub const DEFINITION_VALIDATION_SUITE: &str = "definition-validation";
pub const EVENT_REDUCER_DETERMINISM_SUITE: &str = "event-reducer-determinism";
pub const OCCURRENCE_FENCE_UNIQUENESS_SUITE: &str = "occurrence-fence-uniqueness";
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
struct AttemptTerminalVectorSet {
    schema_version: String,
    cases: Vec<AttemptTerminalVectorCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AttemptTerminalVectorCase {
    case_id: String,
    first_state: AttemptLedgerState,
    attempted_state: AttemptLedgerState,
    expected: ExpectedAttemptTerminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum AttemptLedgerState {
    Adopted,
    Dispatching,
    Started,
    Observing,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Ambiguous,
}

impl AttemptLedgerState {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Adopted => "adopted",
            Self::Dispatching => "dispatching",
            Self::Started => "started",
            Self::Observing => "observing",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::TimedOut => "timed_out",
            Self::Ambiguous => "ambiguous",
        }
    }

    const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::TimedOut | Self::Ambiguous
        )
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedAttemptTerminal {
    update_committed: bool,
    delete_committed: bool,
    final_state: AttemptLedgerState,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DefinitionValidationVectorSet {
    schema_version: String,
    cases: Vec<DefinitionValidationVectorCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DefinitionValidationVectorCase {
    case_id: String,
    definition: Value,
    expected: ExpectedDefinitionValidation,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
enum ExpectedDefinitionValidation {
    Accepted {
        #[serde(rename = "normalizedDigest")]
        normalized_digest: String,
    },
    Rejected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventReducerVectorSet {
    schema_version: String,
    cases: Vec<EventReducerVectorCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EventReducerVectorCase {
    case_id: String,
    events: Vec<EventEnvelope>,
    duplicate_index: usize,
    expected_state_digest: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OccurrenceFenceVectorSet {
    schema_version: String,
    cases: Vec<OccurrenceFenceVectorCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OccurrenceFenceVectorCase {
    case_id: String,
    scenario: OccurrenceFenceScenario,
    first: OccurrenceFenceInput,
    second: OccurrenceFenceInput,
    expected: ExpectedOccurrenceFence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OccurrenceFenceScenario {
    DuplicateSlot,
    DifferentAutomation,
    DifferentSlot,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OccurrenceFenceInput {
    occurrence_id: String,
    automation_id: String,
    scheduled_for: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedOccurrenceFence {
    first_preserved: bool,
    second_committed: bool,
    row_count: usize,
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
                ATTEMPT_TERMINAL_IMMUTABILITY_SUITE,
                CAPABILITY_NEGOTIATION_SUITE,
                DEFINITION_VALIDATION_SUITE,
                EVENT_REDUCER_DETERMINISM_SUITE,
                OCCURRENCE_FENCE_UNIQUENESS_SUITE,
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
        ATTEMPT_TERMINAL_IMMUTABILITY_SUITE => {
            evaluate_attempt_terminal_immutability(&request.vector)?
        }
        CAPABILITY_NEGOTIATION_SUITE => evaluate_capability_negotiation(&request.vector)?,
        DEFINITION_VALIDATION_SUITE => evaluate_definition_validation(&request.vector)?,
        EVENT_REDUCER_DETERMINISM_SUITE => evaluate_event_reducer_determinism(&request.vector)?,
        OCCURRENCE_FENCE_UNIQUENESS_SUITE => evaluate_occurrence_fence_uniqueness(&request.vector)?,
        RUN_TERMINAL_MONOTONICITY_SUITE => evaluate_run_terminal_monotonicity(&request.vector)?,
        _ => return Err("conformance suite is unsupported"),
    };
    result_for(&request.suite_id, &request.vector, all_passed)
}

fn evaluate_attempt_terminal_immutability(vector: &Value) -> Result<bool, &'static str> {
    let vectors: AttemptTerminalVectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != ATTEMPT_TERMINAL_VECTOR_SCHEMA_VERSION
        || vectors.cases.is_empty()
        || vectors.cases.len() > MAX_CASES
    {
        return Err("conformance vector is invalid");
    }

    let mut case_ids = BTreeSet::new();
    let mut terminal_states = BTreeSet::new();
    for case in &vectors.cases {
        if !valid_case_id(&case.case_id)
            || !case_ids.insert(&case.case_id)
            || !case.first_state.is_terminal()
            || case.first_state == case.attempted_state
        {
            return Err("conformance vector is invalid");
        }
        terminal_states.insert(case.first_state);
    }
    if terminal_states.len() != 5 {
        return Err("conformance vector is invalid");
    }

    let conn = Connection::open_in_memory().map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch(super::store::AUTOMATION_DEFINITIONS_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch(super::occurrences::AUTOMATION_OCCURRENCES_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch(super::runs::AUTOMATION_RUNS_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch("CREATE TABLE sessions (id TEXT PRIMARY KEY NOT NULL);")
        .map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch(AUTOMATION_ATTEMPTS_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;

    let mut all_passed = true;
    for (index, case) in vectors.cases.iter().enumerate() {
        all_passed &= attempt_terminal_case_matches(&conn, case, index)?;
    }
    Ok(all_passed)
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

fn evaluate_definition_validation(vector: &Value) -> Result<bool, &'static str> {
    let vectors: DefinitionValidationVectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != DEFINITION_VALIDATION_VECTOR_SCHEMA_VERSION
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
            || matches!(
                &case.expected,
                ExpectedDefinitionValidation::Accepted { normalized_digest }
                    if !valid_sha256_digest(normalized_digest)
            )
        {
            return Err("conformance vector is invalid");
        }
    }

    Ok(vectors.cases.iter().all(definition_case_matches))
}

fn evaluate_event_reducer_determinism(vector: &Value) -> Result<bool, &'static str> {
    let vectors: EventReducerVectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != EVENT_REDUCER_VECTOR_SCHEMA_VERSION
        || vectors.cases.is_empty()
        || vectors.cases.len() > MAX_CASES
    {
        return Err("conformance vector is invalid");
    }

    let mut case_ids = BTreeSet::new();
    for case in &vectors.cases {
        let mut event_ids = BTreeSet::new();
        if !valid_case_id(&case.case_id)
            || !case_ids.insert(&case.case_id)
            || case.events.is_empty()
            || case.events.len() > MAX_CASES
            || case
                .events
                .iter()
                .any(|event| !event_ids.insert(event.event_id.as_str()))
            || case.duplicate_index >= case.events.len()
            || !valid_sha256_digest(&case.expected_state_digest)
        {
            return Err("conformance vector is invalid");
        }
    }

    let mut all_passed = true;
    for case in &vectors.cases {
        all_passed &= event_reducer_case_matches(case)?;
    }
    Ok(all_passed)
}

fn event_reducer_case_matches(case: &EventReducerVectorCase) -> Result<bool, &'static str> {
    let mut canonical = EventReducer::default();
    for event in &case.events {
        if canonical.apply(event).is_err() {
            return Ok(false);
        }
    }

    let mut duplicated = EventReducer::default();
    for (index, event) in case.events.iter().enumerate() {
        if duplicated.apply(event).is_err()
            || (index == case.duplicate_index && duplicated.apply(event).is_err())
        {
            return Ok(false);
        }
    }

    let canonical_state =
        canonicalize(canonical.state()).map_err(|_| "conformance suite execution failed")?;
    let observed_digest = format!("sha256:{}", sha256_hex(&canonical_state));
    Ok(canonical.state() == duplicated.state() && observed_digest == case.expected_state_digest)
}

fn evaluate_occurrence_fence_uniqueness(vector: &Value) -> Result<bool, &'static str> {
    let vectors: OccurrenceFenceVectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != OCCURRENCE_FENCE_VECTOR_SCHEMA_VERSION
        || vectors.cases.is_empty()
        || vectors.cases.len() > MAX_CASES
    {
        return Err("conformance vector is invalid");
    }

    let mut case_ids = BTreeSet::new();
    let mut occurrence_ids = BTreeSet::new();
    let mut scenarios = BTreeSet::new();
    for case in &vectors.cases {
        if !valid_case_id(&case.case_id)
            || !case_ids.insert(&case.case_id)
            || !scenarios.insert(case.scenario)
            || !valid_occurrence_fence_input(&case.first)
            || !valid_occurrence_fence_input(&case.second)
            || !occurrence_ids.insert(&case.first.occurrence_id)
            || !occurrence_ids.insert(&case.second.occurrence_id)
            || !occurrence_fence_scenario_matches(case)
            || !(1..=2).contains(&case.expected.row_count)
        {
            return Err("conformance vector is invalid");
        }
    }
    if scenarios.len() != 3 {
        return Err("conformance vector is invalid");
    }

    let mut all_passed = true;
    for case in &vectors.cases {
        all_passed &= occurrence_fence_case_matches(case)?;
    }
    Ok(all_passed)
}

fn valid_occurrence_fence_input(input: &OccurrenceFenceInput) -> bool {
    OccurrenceId::new(input.occurrence_id.clone()).is_ok()
        && AutomationId::new(input.automation_id.clone()).is_ok()
        && DateTime::parse_from_rfc3339(&input.scheduled_for).is_ok_and(|timestamp| {
            timestamp
                .with_timezone(&Utc)
                .to_rfc3339_opts(SecondsFormat::Millis, true)
                == input.scheduled_for
        })
}

fn occurrence_fence_scenario_matches(case: &OccurrenceFenceVectorCase) -> bool {
    let same_automation = case.first.automation_id == case.second.automation_id;
    let same_slot = case.first.scheduled_for == case.second.scheduled_for;
    match case.scenario {
        OccurrenceFenceScenario::DuplicateSlot => same_automation && same_slot,
        OccurrenceFenceScenario::DifferentAutomation => !same_automation && same_slot,
        OccurrenceFenceScenario::DifferentSlot => same_automation && !same_slot,
    }
}

fn occurrence_fence_case_matches(case: &OccurrenceFenceVectorCase) -> Result<bool, &'static str> {
    let conn = Connection::open_in_memory().map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch(super::store::AUTOMATION_DEFINITIONS_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch(super::occurrences::AUTOMATION_OCCURRENCES_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;
    let recorded_at = "1970-01-01T00:00:00.000Z";
    let insert = |input: &OccurrenceFenceInput| {
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, scheduled_for, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
            params![
                input.occurrence_id,
                input.automation_id,
                input.scheduled_for,
                recorded_at
            ],
        )
    };

    insert(&case.first).map_err(|_| "conformance suite execution failed")?;
    let second_committed = insert(&case.second).is_ok();
    let row_count = conn
        .query_row("SELECT COUNT(*) FROM automation_occurrences", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(|_| "conformance suite execution failed")
        .and_then(|count| {
            usize::try_from(count).map_err(|_| "conformance suite execution failed")
        })?;
    let first_preserved: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM automation_occurrences WHERE id = ?1)",
            [&case.first.occurrence_id],
            |row| row.get(0),
        )
        .map_err(|_| "conformance suite execution failed")?;

    Ok(first_preserved == case.expected.first_preserved
        && second_committed == case.expected.second_committed
        && row_count == case.expected.row_count)
}

fn evaluate_run_terminal_monotonicity(vector: &Value) -> Result<bool, &'static str> {
    let (executed_cases, passed_cases) = evaluate_run_terminal_monotonicity_case_counts(vector)?;
    Ok(passed_cases == executed_cases)
}

pub(super) fn evaluate_run_terminal_monotonicity_case_counts(
    vector: &Value,
) -> Result<(usize, usize), &'static str> {
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
        if !valid_case_id(&case.case_id)
            || !case_ids.insert(&case.case_id)
            || case.first_status == case.replay_status
        {
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

    let mut passed_cases = 0;
    for (index, case) in vectors.cases.iter().enumerate() {
        passed_cases += usize::from(run_terminal_case_matches(&conn, case, index, now)?);
    }
    Ok((vectors.cases.len(), passed_cases))
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

fn attempt_terminal_case_matches(
    conn: &Connection,
    case: &AttemptTerminalVectorCase,
    index: usize,
) -> Result<bool, &'static str> {
    let automation_id = format!("conformance-attempt-automation-{index}");
    let occurrence_id = format!("conformance-attempt-occurrence-{index}");
    let run_id = format!("conformance-attempt-run-{index}");
    let attempt_id = format!("conformance-attempt-{index}");
    let adoption_key = format!("{run_id}:1");
    let opened_at = "1970-01-01T00:00:00.000Z";
    let settled_at = "1970-01-01T00:00:01.000Z";

    conn.execute(
        "INSERT INTO automation_occurrences
            (id, automation_id, scheduled_for, state, attempt, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'failed', 1, ?3, ?4)",
        params![occurrence_id, automation_id, opened_at, settled_at],
    )
    .map_err(|_| "conformance suite execution failed")?;
    conn.execute(
        "INSERT INTO automation_runs
            (id, automation_id, occurrence_id, runtime, status, started_at, finished_at)
         VALUES (?1, ?2, ?3, 'coven-code', 'failed', ?4, ?5)",
        params![run_id, automation_id, occurrence_id, opened_at, settled_at],
    )
    .map_err(|_| "conformance suite execution failed")?;
    conn.execute(
        "INSERT INTO automation_attempts (
            id, run_id, occurrence_id, attempt_number, adoption_key,
            occurrence_fence_generation, dispatch_generation, state,
            retry_classification, not_before, opened_at, settled_at
         ) VALUES (?1, ?2, ?3, 1, ?4, 1, 1, ?5, 'initial', ?6, ?6, ?7)",
        params![
            attempt_id,
            run_id,
            occurrence_id,
            adoption_key,
            case.first_state.as_str(),
            opened_at,
            settled_at
        ],
    )
    .map_err(|_| "conformance suite execution failed")?;

    let update_committed = conn
        .execute(
            "UPDATE automation_attempts
             SET state = ?2, state_reason = 'conformance mutation', settled_at = ?3
             WHERE id = ?1",
            params![attempt_id, case.attempted_state.as_str(), opened_at],
        )
        .is_ok();
    let delete_committed = conn
        .execute(
            "DELETE FROM automation_attempts WHERE id = ?1",
            [&attempt_id],
        )
        .is_ok();
    let final_state = conn
        .query_row(
            "SELECT state FROM automation_attempts WHERE id = ?1",
            [&attempt_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|_| "conformance suite execution failed")?;

    Ok(update_committed == case.expected.update_committed
        && delete_committed == case.expected.delete_committed
        && final_state.as_deref() == Some(case.expected.final_state.as_str()))
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

fn definition_case_matches(case: &DefinitionValidationVectorCase) -> bool {
    let parsed = serde_json::from_value::<AutomationDefinition>(case.definition.clone());
    match (&case.expected, parsed) {
        (ExpectedDefinitionValidation::Accepted { normalized_digest }, Ok(definition)) => {
            serde_json::to_value(definition)
                .ok()
                .and_then(|value| canonicalize(&value).ok())
                .map(|canonical| format!("sha256:{}", sha256_hex(&canonical)))
                .is_some_and(|observed| observed == *normalized_digest)
        }
        (ExpectedDefinitionValidation::Rejected, Err(_)) => true,
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

fn valid_sha256_digest(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}
