use std::collections::BTreeSet;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::capability_negotiation::{negotiate_definition, DefinitionNegotiation};
use super::command_adoption::{
    ensure_global_adoption_key_guards, execute_definition_command, DefinitionCommand,
    DefinitionCommandOutcome, AUTOMATION_COMMAND_ADOPTIONS_SCHEMA_SQL,
};
use super::contract::error::ErrorCode;
use super::contract::events::EventReducer;
use super::contract::types::{AdoptionKey, AutomationId, OccurrenceId, Sha256Digest};
use super::contract::{
    canonicalize, canonicalize_without_integrity, sha256_hex, AutomationDefinition,
    AutomationReceipt, EventEnvelope,
};
use super::rrule::{parse_rrule, RruleFrequency};
use super::runs::{
    record_run_finish, record_run_start, RunFinish, RunStart, AUTOMATION_ATTEMPTS_SCHEMA_SQL,
};

const TARGET_CAPABILITY_SCHEMA_VERSION: &str = "coven.automations.conformance-target-capability.v1";
const SUITE_REQUEST_SCHEMA_VERSION: &str = "coven.automations.conformance-suite-request.v1";
const SUITE_RESULT_SCHEMA_VERSION: &str = "coven.automations.conformance-suite-result.v1";
const CAPABILITY_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.capability-negotiation-vectors.v1";
const COMMAND_ADOPTION_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.command-adoption-idempotency-vectors.v1";
const DEFINITION_LIFECYCLE_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.definition-lifecycle-transitions-vectors.v1";
const ATTEMPT_TERMINAL_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.attempt-terminal-immutability-vectors.v1";
const DEFINITION_VALIDATION_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.definition-validation-vectors.v1";
const EVENT_REDUCER_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.event-reducer-determinism-vectors.v1";
const MISFIRE_LATEST_PLANNING_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.misfire-latest-planning-vectors.v1";
const OCCURRENCE_FENCE_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.occurrence-fence-uniqueness-vectors.v1";
const RECEIPT_INTEGRITY_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.receipt-integrity-validation-vectors.v1";
const RRULE_VOCABULARY_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.rrule-vocabulary-vectors.v1";
const RUN_TERMINAL_VECTOR_SCHEMA_VERSION: &str =
    "coven.automations.run-terminal-monotonicity-vectors.v1";
const STRUCTURAL_PROFILE: &str = "structural";
const SCHEDULER_RELIABILITY_PROFILE: &str = "scheduler_reliability";
const MAX_CASES: usize = 128;

pub const CAPABILITY_NEGOTIATION_SUITE: &str = "capability-negotiation";
pub const ATTEMPT_TERMINAL_IMMUTABILITY_SUITE: &str = "attempt-terminal-immutability";
pub const COMMAND_ADOPTION_IDEMPOTENCY_SUITE: &str = "command-adoption-idempotency";
pub const DEFINITION_LIFECYCLE_TRANSITIONS_SUITE: &str = "definition-lifecycle-transitions";
pub const DEFINITION_VALIDATION_SUITE: &str = "definition-validation";
pub const EVENT_REDUCER_DETERMINISM_SUITE: &str = "event-reducer-determinism";
pub const MISFIRE_LATEST_PLANNING_SUITE: &str = "misfire-latest-planning";
pub const OCCURRENCE_FENCE_UNIQUENESS_SUITE: &str = "occurrence-fence-uniqueness";
pub const RECEIPT_INTEGRITY_VALIDATION_SUITE: &str = "receipt-integrity-validation";
pub const RRULE_VOCABULARY_SUITE: &str = "rrule-vocabulary";
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
struct CommandAdoptionVectorSet {
    schema_version: String,
    cases: Vec<CommandAdoptionVectorCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommandAdoptionVectorCase {
    case_id: String,
    adoption_key: String,
    definition: Value,
    conflicting_definition: Value,
    expected: ExpectedCommandAdoption,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CommandAdoptionOutcome {
    Committed,
    Replayed,
    Rejected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedCommandAdoption {
    first_outcome: CommandAdoptionOutcome,
    exact_replay_outcome: CommandAdoptionOutcome,
    conflicting_replay_outcome: CommandAdoptionOutcome,
    conflict_code: ErrorCode,
    exact_replay_result_preserved: bool,
    exact_replay_event_preserved: bool,
    first_commit_time_preserved: bool,
    original_definition_preserved: bool,
    definition_rows: usize,
    adoption_rows: usize,
    event_rows: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DefinitionLifecycleVectorSet {
    schema_version: String,
    cases: Vec<DefinitionLifecycleVectorCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DefinitionLifecycleVectorCase {
    case_id: String,
    scenario: DefinitionLifecycleScenario,
    initial_state: DefinitionLifecycleState,
    operation: DefinitionLifecycleOperation,
    expected: ExpectedDefinitionLifecycle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DefinitionLifecycleScenario {
    CreatePaused,
    CreateActive,
    CreateDisabledIsRefused,
    PausedToActive,
    ActiveToPaused,
    PausedToDisabled,
    ActiveToDisabled,
    PausedToTombstoned,
    ActiveToTombstoned,
    DisabledToTombstoned,
    DisabledToActiveIsRefused,
    DisabledToPausedIsRefused,
    DisabledToDisabledIsRefused,
    TombstonedToActiveIsRefused,
    TombstonedToPausedIsRefused,
    TombstonedToDisabledIsRefused,
    TombstonedToTombstonedIsRefused,
}

impl DefinitionLifecycleScenario {
    const COUNT: usize = 17;

    const fn input(self) -> (DefinitionLifecycleState, DefinitionLifecycleOperation) {
        match self {
            Self::CreatePaused => (
                DefinitionLifecycleState::Absent,
                DefinitionLifecycleOperation::CreatePaused,
            ),
            Self::CreateActive => (
                DefinitionLifecycleState::Absent,
                DefinitionLifecycleOperation::CreateActive,
            ),
            Self::CreateDisabledIsRefused => (
                DefinitionLifecycleState::Absent,
                DefinitionLifecycleOperation::CreateDisabled,
            ),
            Self::PausedToActive => (
                DefinitionLifecycleState::Paused,
                DefinitionLifecycleOperation::ReviseActive,
            ),
            Self::ActiveToPaused => (
                DefinitionLifecycleState::Active,
                DefinitionLifecycleOperation::RevisePaused,
            ),
            Self::PausedToDisabled => (
                DefinitionLifecycleState::Paused,
                DefinitionLifecycleOperation::Disable,
            ),
            Self::ActiveToDisabled => (
                DefinitionLifecycleState::Active,
                DefinitionLifecycleOperation::Disable,
            ),
            Self::PausedToTombstoned => (
                DefinitionLifecycleState::Paused,
                DefinitionLifecycleOperation::Tombstone,
            ),
            Self::ActiveToTombstoned => (
                DefinitionLifecycleState::Active,
                DefinitionLifecycleOperation::Tombstone,
            ),
            Self::DisabledToTombstoned => (
                DefinitionLifecycleState::Disabled,
                DefinitionLifecycleOperation::Tombstone,
            ),
            Self::DisabledToActiveIsRefused => (
                DefinitionLifecycleState::Disabled,
                DefinitionLifecycleOperation::ReviseActive,
            ),
            Self::DisabledToPausedIsRefused => (
                DefinitionLifecycleState::Disabled,
                DefinitionLifecycleOperation::RevisePaused,
            ),
            Self::DisabledToDisabledIsRefused => (
                DefinitionLifecycleState::Disabled,
                DefinitionLifecycleOperation::Disable,
            ),
            Self::TombstonedToActiveIsRefused => (
                DefinitionLifecycleState::Tombstoned,
                DefinitionLifecycleOperation::ReviseActive,
            ),
            Self::TombstonedToPausedIsRefused => (
                DefinitionLifecycleState::Tombstoned,
                DefinitionLifecycleOperation::RevisePaused,
            ),
            Self::TombstonedToDisabledIsRefused => (
                DefinitionLifecycleState::Tombstoned,
                DefinitionLifecycleOperation::Disable,
            ),
            Self::TombstonedToTombstonedIsRefused => (
                DefinitionLifecycleState::Tombstoned,
                DefinitionLifecycleOperation::Tombstone,
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DefinitionLifecycleState {
    Absent,
    Paused,
    Active,
    Disabled,
    Tombstoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum DefinitionLifecycleOperation {
    CreatePaused,
    CreateActive,
    CreateDisabled,
    RevisePaused,
    ReviseActive,
    Disable,
    Tombstone,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
enum ExpectedDefinitionLifecycle {
    Committed {
        #[serde(rename = "finalState")]
        final_state: DefinitionLifecycleState,
        revision: u64,
    },
    Rejected {
        code: ErrorCode,
        #[serde(rename = "finalState")]
        final_state: DefinitionLifecycleState,
        revision: Option<u64>,
    },
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
struct MisfireLatestPlanningVectorSet {
    schema_version: String,
    cases: Vec<MisfireLatestPlanningVectorCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MisfireLatestPlanningVectorCase {
    case_id: String,
    scenario: MisfireLatestPlanningScenario,
    definition: Value,
    created_at: String,
    observed_at: String,
    #[serde(default)]
    existing_scheduled_for: Option<String>,
    expected: ExpectedMisfireLatestPlanning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum MisfireLatestPlanningScenario {
    RestartCollapseLatest,
    ExistingFenceReplay,
    ClockRollbackNoOlderFence,
    PausedDefinition,
}

impl MisfireLatestPlanningScenario {
    const COUNT: usize = 4;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExpectedPlanOutcome {
    Planned,
    NotDue,
    AlreadyFenced,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExpectedMisfireLatestPlanning {
    first_outcome: ExpectedPlanOutcome,
    second_outcome: ExpectedPlanOutcome,
    scheduled_slots: Vec<String>,
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
struct ReceiptIntegrityVectorSet {
    schema_version: String,
    cases: Vec<ReceiptIntegrityVectorCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceiptIntegrityVectorCase {
    case_id: String,
    receipt: Value,
    expected: ExpectedReceiptIntegrity,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
enum ExpectedReceiptIntegrity {
    Accepted {
        #[serde(rename = "normalizedDigest")]
        normalized_digest: String,
    },
    Rejected,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RruleVocabularyVectorSet {
    schema_version: String,
    cases: Vec<RruleVocabularyVectorCase>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RruleVocabularyVectorCase {
    case_id: String,
    scenario: RruleVocabularyScenario,
    rrule: String,
    expected: ExpectedRruleVocabulary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RruleVocabularyScenario {
    DailyDefaults,
    DailyHourNormalization,
    WeeklyDefaults,
    WeeklyDayNormalization,
    UnsupportedFrequency,
    UnsupportedKey,
    DailyByDay,
    DuplicateFrequency,
    DuplicateByHour,
    DuplicateByDay,
    OutOfRangeHour,
    DuplicateHour,
    DuplicateWeekday,
    DuplicateWeekdayAlias,
    EmptyHourEntry,
    EmptyWeekdayEntry,
    UnknownWeekday,
    MissingFrequency,
    MalformedPart,
    TrailingSeparator,
    EmptySegment,
    EmptyValue,
}

impl RruleVocabularyScenario {
    const COUNT: usize = 22;

    const fn rule(self) -> &'static str {
        match self {
            Self::DailyDefaults => "FREQ=DAILY",
            Self::DailyHourNormalization => "freq=daily;byhour=17,9",
            Self::WeeklyDefaults => "FREQ=WEEKLY",
            Self::WeeklyDayNormalization => "freq=weekly;byday=wed,MO,fr;byhour=8",
            Self::UnsupportedFrequency => "FREQ=HOURLY",
            Self::UnsupportedKey => "FREQ=DAILY;COUNT=3",
            Self::DailyByDay => "FREQ=DAILY;BYDAY=MO",
            Self::DuplicateFrequency => "FREQ=DAILY;FREQ=WEEKLY",
            Self::DuplicateByHour => "FREQ=DAILY;BYHOUR=9;BYHOUR=17",
            Self::DuplicateByDay => "FREQ=WEEKLY;BYDAY=MO;BYDAY=TU",
            Self::OutOfRangeHour => "FREQ=DAILY;BYHOUR=24",
            Self::DuplicateHour => "FREQ=DAILY;BYHOUR=9,9",
            Self::DuplicateWeekday => "FREQ=WEEKLY;BYDAY=MO,MO",
            Self::DuplicateWeekdayAlias => "FREQ=WEEKLY;BYDAY=MO,MON",
            Self::EmptyHourEntry => "FREQ=DAILY;BYHOUR=9,,17",
            Self::EmptyWeekdayEntry => "FREQ=WEEKLY;BYDAY=MO,,TU",
            Self::UnknownWeekday => "FREQ=WEEKLY;BYDAY=XX",
            Self::MissingFrequency => "BYHOUR=9",
            Self::MalformedPart => "FREQ=DAILY;BYHOUR",
            Self::TrailingSeparator => "FREQ=DAILY;",
            Self::EmptySegment => "FREQ=DAILY;;BYHOUR=9",
            Self::EmptyValue => "FREQ=",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExpectedRruleFrequency {
    Daily,
    Weekly,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case", deny_unknown_fields)]
enum ExpectedRruleVocabulary {
    Accepted {
        frequency: ExpectedRruleFrequency,
        #[serde(rename = "byHour")]
        by_hour: Vec<u8>,
        #[serde(rename = "byDay")]
        by_day: Vec<String>,
    },
    Rejected,
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
        profiles: vec![
            TargetProfileCapability {
                profile: STRUCTURAL_PROFILE,
                suites: vec![
                    ATTEMPT_TERMINAL_IMMUTABILITY_SUITE,
                    CAPABILITY_NEGOTIATION_SUITE,
                    COMMAND_ADOPTION_IDEMPOTENCY_SUITE,
                    DEFINITION_LIFECYCLE_TRANSITIONS_SUITE,
                    DEFINITION_VALIDATION_SUITE,
                    EVENT_REDUCER_DETERMINISM_SUITE,
                    OCCURRENCE_FENCE_UNIQUENESS_SUITE,
                    RECEIPT_INTEGRITY_VALIDATION_SUITE,
                    RRULE_VOCABULARY_SUITE,
                    RUN_TERMINAL_MONOTONICITY_SUITE,
                ],
            },
            TargetProfileCapability {
                profile: SCHEDULER_RELIABILITY_PROFILE,
                suites: vec![MISFIRE_LATEST_PLANNING_SUITE],
            },
        ],
    }
}

pub fn evaluate(request: &Value) -> Result<TargetSuiteResult, &'static str> {
    let request: TargetSuiteRequest =
        serde_json::from_value(request.clone()).map_err(|_| "conformance request is invalid")?;
    if request.schema_version != SUITE_REQUEST_SCHEMA_VERSION {
        return Err("conformance suite is unsupported");
    }
    if !request.protocol_artifact.is_object() || !request.subject_artifact.is_object() {
        return Err("conformance request is invalid");
    }

    let all_passed = match (request.profile.as_str(), request.suite_id.as_str()) {
        (STRUCTURAL_PROFILE, ATTEMPT_TERMINAL_IMMUTABILITY_SUITE) => {
            evaluate_attempt_terminal_immutability(&request.vector)?
        }
        (STRUCTURAL_PROFILE, CAPABILITY_NEGOTIATION_SUITE) => {
            evaluate_capability_negotiation(&request.vector)?
        }
        (STRUCTURAL_PROFILE, COMMAND_ADOPTION_IDEMPOTENCY_SUITE) => {
            evaluate_command_adoption_idempotency(&request.vector)?
        }
        (STRUCTURAL_PROFILE, DEFINITION_LIFECYCLE_TRANSITIONS_SUITE) => {
            evaluate_definition_lifecycle_transitions(&request.vector)?
        }
        (STRUCTURAL_PROFILE, DEFINITION_VALIDATION_SUITE) => {
            evaluate_definition_validation(&request.vector)?
        }
        (STRUCTURAL_PROFILE, EVENT_REDUCER_DETERMINISM_SUITE) => {
            evaluate_event_reducer_determinism(&request.vector)?
        }
        (STRUCTURAL_PROFILE, OCCURRENCE_FENCE_UNIQUENESS_SUITE) => {
            evaluate_occurrence_fence_uniqueness(&request.vector)?
        }
        (STRUCTURAL_PROFILE, RECEIPT_INTEGRITY_VALIDATION_SUITE) => {
            evaluate_receipt_integrity_validation(&request.vector)?
        }
        (STRUCTURAL_PROFILE, RRULE_VOCABULARY_SUITE) => evaluate_rrule_vocabulary(&request.vector)?,
        (STRUCTURAL_PROFILE, RUN_TERMINAL_MONOTONICITY_SUITE) => {
            evaluate_run_terminal_monotonicity(&request.vector)?
        }
        (SCHEDULER_RELIABILITY_PROFILE, MISFIRE_LATEST_PLANNING_SUITE) => {
            evaluate_misfire_latest_planning(&request.vector)?
        }
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

fn evaluate_command_adoption_idempotency(vector: &Value) -> Result<bool, &'static str> {
    let vectors: CommandAdoptionVectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != COMMAND_ADOPTION_VECTOR_SCHEMA_VERSION
        || vectors.cases.is_empty()
        || vectors.cases.len() > MAX_CASES
    {
        return Err("conformance vector is invalid");
    }

    let mut case_ids = BTreeSet::new();
    let mut adoption_keys = BTreeSet::new();
    for case in &vectors.cases {
        let definition = super::definition::RoutineDefinition::from_json(&case.definition)
            .map_err(|_| "conformance vector is invalid")?;
        let conflicting =
            super::definition::RoutineDefinition::from_json(&case.conflicting_definition)
                .map_err(|_| "conformance vector is invalid")?;
        if !valid_case_id(&case.case_id)
            || !case_ids.insert(&case.case_id)
            || AdoptionKey::new(case.adoption_key.clone()).is_err()
            || !adoption_keys.insert(&case.adoption_key)
            || definition.id != conflicting.id
            || definition == conflicting
            || case.expected.first_outcome != CommandAdoptionOutcome::Committed
            || case.expected.exact_replay_outcome != CommandAdoptionOutcome::Replayed
            || case.expected.conflicting_replay_outcome != CommandAdoptionOutcome::Rejected
            || case.expected.conflict_code != ErrorCode::AdoptionReplayMismatch
            || !case.expected.exact_replay_result_preserved
            || !case.expected.exact_replay_event_preserved
            || !case.expected.first_commit_time_preserved
            || !case.expected.original_definition_preserved
            || case.expected.definition_rows != 1
            || case.expected.adoption_rows != 1
            || case.expected.event_rows != 1
        {
            return Err("conformance vector is invalid");
        }
    }

    let mut all_passed = true;
    for case in &vectors.cases {
        all_passed &= command_adoption_case_matches(case)?;
    }
    Ok(all_passed)
}

fn command_adoption_case_matches(case: &CommandAdoptionVectorCase) -> Result<bool, &'static str> {
    let conn = command_conformance_connection()?;
    let first = execute_definition_command(
        &conn,
        &case.adoption_key,
        DefinitionCommand::Create {
            definition: case.definition.clone(),
        },
        "2026-08-30T09:00:00.000Z",
    )
    .map_err(|_| "conformance suite execution failed")?;
    let exact_replay = execute_definition_command(
        &conn,
        &case.adoption_key,
        DefinitionCommand::Create {
            definition: case.definition.clone(),
        },
        "2026-08-30T09:01:00.000Z",
    )
    .map_err(|_| "conformance suite execution failed")?;
    let conflicting_replay = execute_definition_command(
        &conn,
        &case.adoption_key,
        DefinitionCommand::Create {
            definition: case.conflicting_definition.clone(),
        },
        "2026-08-30T09:02:00.000Z",
    )
    .map_err(|_| "conformance suite execution failed")?;

    let definition_rows = table_row_count(&conn, "SELECT COUNT(*) FROM automation_definitions")?;
    let adoption_rows =
        table_row_count(&conn, "SELECT COUNT(*) FROM automation_command_adoptions")?;
    let event_rows = table_row_count(&conn, "SELECT COUNT(*) FROM automation_events")?;
    let expected_definition = super::definition::RoutineDefinition::from_json(&case.definition)
        .map_err(|_| "conformance vector is invalid")?;
    let stored_definition = super::store::get_definition(&conn, &expected_definition.id)
        .map_err(|_| "conformance suite execution failed")?
        .and_then(|stored| {
            serde_json::from_str::<super::definition::RoutineDefinition>(&stored.definition_json)
                .ok()
        });
    let original_definition_preserved = stored_definition
        .as_ref()
        .is_some_and(|stored| stored == &expected_definition);
    Ok(
        command_outcome(first.outcome) == case.expected.first_outcome
            && command_outcome(exact_replay.outcome) == case.expected.exact_replay_outcome
            && command_outcome(conflicting_replay.outcome)
                == case.expected.conflicting_replay_outcome
            && conflicting_replay.error.as_ref().map(|error| error.code())
                == Some(case.expected.conflict_code)
            && (first.result == exact_replay.result) == case.expected.exact_replay_result_preserved
            && (first.event_ref == exact_replay.event_ref)
                == case.expected.exact_replay_event_preserved
            && (exact_replay.replay_first_committed_at.as_deref()
                == Some("2026-08-30T09:00:00.000Z"))
                == case.expected.first_commit_time_preserved
            && original_definition_preserved == case.expected.original_definition_preserved
            && definition_rows == case.expected.definition_rows
            && adoption_rows == case.expected.adoption_rows
            && event_rows == case.expected.event_rows,
    )
}

fn command_conformance_connection() -> Result<Connection, &'static str> {
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
    conn.execute_batch(AUTOMATION_COMMAND_ADOPTIONS_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch(super::contract::events::AUTOMATION_EVENTS_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;
    ensure_global_adoption_key_guards(&conn).map_err(|_| "conformance suite execution failed")?;
    Ok(conn)
}

fn evaluate_definition_lifecycle_transitions(vector: &Value) -> Result<bool, &'static str> {
    let vectors: DefinitionLifecycleVectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != DEFINITION_LIFECYCLE_VECTOR_SCHEMA_VERSION
        || vectors.cases.len() != DefinitionLifecycleScenario::COUNT
    {
        return Err("conformance vector is invalid");
    }

    let mut case_ids = BTreeSet::new();
    let mut scenarios = BTreeSet::new();
    for case in &vectors.cases {
        if !valid_case_id(&case.case_id)
            || !case_ids.insert(&case.case_id)
            || !scenarios.insert(case.scenario)
            || case.scenario.input() != (case.initial_state, case.operation)
        {
            return Err("conformance vector is invalid");
        }
    }
    if scenarios.len() != DefinitionLifecycleScenario::COUNT {
        return Err("conformance vector is invalid");
    }

    let mut all_passed = true;
    for case in &vectors.cases {
        all_passed &= definition_lifecycle_case_matches(case)?;
    }
    Ok(all_passed)
}

fn definition_lifecycle_case_matches(
    case: &DefinitionLifecycleVectorCase,
) -> Result<bool, &'static str> {
    let conn = command_conformance_connection()?;
    let automation_id = format!("lifecycle-{}", case.case_id);
    prepare_definition_lifecycle_state(&conn, &automation_id, case.initial_state)?;
    let current_revision = observed_definition_lifecycle(&conn, &automation_id)?.1;
    let response = execute_definition_command(
        &conn,
        &format!("adopt:lifecycle:{}:operation", case.case_id),
        lifecycle_operation_command(&automation_id, case.operation, current_revision),
        "2026-08-30T09:02:00.000Z",
    )
    .map_err(|_| "conformance suite execution failed")?;
    let (final_state, revision) = observed_definition_lifecycle(&conn, &automation_id)?;

    Ok(match &case.expected {
        ExpectedDefinitionLifecycle::Committed {
            final_state: expected_state,
            revision: expected_revision,
        } => {
            response.outcome == DefinitionCommandOutcome::Committed
                && response.error.is_none()
                && response.revision == Some(*expected_revision)
                && final_state == *expected_state
                && revision == Some(*expected_revision)
        }
        ExpectedDefinitionLifecycle::Rejected {
            code,
            final_state: expected_state,
            revision: expected_revision,
        } => {
            response.outcome == DefinitionCommandOutcome::Rejected
                && response.error.as_ref().map(|error| error.code()) == Some(*code)
                && response.revision == *expected_revision
                && final_state == *expected_state
                && revision == *expected_revision
        }
    })
}

fn prepare_definition_lifecycle_state(
    conn: &Connection,
    automation_id: &str,
    state: DefinitionLifecycleState,
) -> Result<(), &'static str> {
    if state == DefinitionLifecycleState::Absent {
        return Ok(());
    }
    let initial_status = if state == DefinitionLifecycleState::Active {
        DefinitionLifecycleState::Active
    } else {
        DefinitionLifecycleState::Paused
    };
    let created = execute_definition_command(
        conn,
        &format!("adopt:lifecycle:{automation_id}:create"),
        lifecycle_operation_command(
            automation_id,
            match initial_status {
                DefinitionLifecycleState::Active => DefinitionLifecycleOperation::CreateActive,
                DefinitionLifecycleState::Paused => DefinitionLifecycleOperation::CreatePaused,
                _ => return Err("conformance suite execution failed"),
            },
            None,
        ),
        "2026-08-30T09:00:00.000Z",
    )
    .map_err(|_| "conformance suite execution failed")?;
    if created.outcome != DefinitionCommandOutcome::Committed {
        return Err("conformance suite execution failed");
    }

    let setup_operation = match state {
        DefinitionLifecycleState::Disabled => Some(DefinitionLifecycleOperation::Disable),
        DefinitionLifecycleState::Tombstoned => Some(DefinitionLifecycleOperation::Tombstone),
        DefinitionLifecycleState::Paused | DefinitionLifecycleState::Active => None,
        DefinitionLifecycleState::Absent => unreachable!(),
    };
    if let Some(operation) = setup_operation {
        let setup = execute_definition_command(
            conn,
            &format!("adopt:lifecycle:{automation_id}:setup"),
            lifecycle_operation_command(automation_id, operation, Some(1)),
            "2026-08-30T09:01:00.000Z",
        )
        .map_err(|_| "conformance suite execution failed")?;
        if setup.outcome != DefinitionCommandOutcome::Committed {
            return Err("conformance suite execution failed");
        }
    }
    Ok(())
}

fn lifecycle_operation_command(
    automation_id: &str,
    operation: DefinitionLifecycleOperation,
    expected_revision: Option<u64>,
) -> DefinitionCommand {
    match operation {
        DefinitionLifecycleOperation::CreatePaused
        | DefinitionLifecycleOperation::CreateActive
        | DefinitionLifecycleOperation::CreateDisabled => DefinitionCommand::Create {
            definition: lifecycle_definition(
                automation_id,
                match operation {
                    DefinitionLifecycleOperation::CreatePaused => "PAUSED",
                    DefinitionLifecycleOperation::CreateActive => "ACTIVE",
                    DefinitionLifecycleOperation::CreateDisabled => "DISABLED",
                    _ => unreachable!(),
                },
            ),
        },
        DefinitionLifecycleOperation::RevisePaused | DefinitionLifecycleOperation::ReviseActive => {
            DefinitionCommand::Revise {
                definition: lifecycle_definition(
                    automation_id,
                    match operation {
                        DefinitionLifecycleOperation::RevisePaused => "PAUSED",
                        DefinitionLifecycleOperation::ReviseActive => "ACTIVE",
                        _ => unreachable!(),
                    },
                ),
                expected_revision,
            }
        }
        DefinitionLifecycleOperation::Disable => DefinitionCommand::Disable {
            automation_id: automation_id.to_owned(),
            expected_revision,
            reason: Some("native lifecycle conformance".to_string()),
        },
        DefinitionLifecycleOperation::Tombstone => DefinitionCommand::Delete {
            automation_id: automation_id.to_owned(),
            expected_revision,
        },
    }
}

fn lifecycle_definition(automation_id: &str, status: &str) -> Value {
    json!({
        "schemaVersion": 1,
        "id": automation_id,
        "name": "Lifecycle conformance",
        "status": status,
        "rrule": "FREQ=DAILY;BYHOUR=9",
        "timezone": "utc",
        "misfire": "latest",
        "overlap": "forbid",
        "timeoutMinutes": 30,
        "runtime": "coven-code",
        "prompt": "Run the native lifecycle conformance probe."
    })
}

fn observed_definition_lifecycle(
    conn: &Connection,
    automation_id: &str,
) -> Result<(DefinitionLifecycleState, Option<u64>), &'static str> {
    let row = conn
        .query_row(
            "SELECT lifecycle_state, revision, tombstoned_at IS NOT NULL
             FROM automation_definitions
             WHERE id = ?1",
            [automation_id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            },
        )
        .optional()
        .map_err(|_| "conformance suite execution failed")?;
    let Some((lifecycle_state, revision, tombstoned)) = row else {
        return Ok((DefinitionLifecycleState::Absent, None));
    };
    let revision = u64::try_from(revision).map_err(|_| "conformance suite execution failed")?;
    let state = if tombstoned {
        DefinitionLifecycleState::Tombstoned
    } else {
        match lifecycle_state.as_str() {
            "paused" => DefinitionLifecycleState::Paused,
            "active" => DefinitionLifecycleState::Active,
            "disabled" => DefinitionLifecycleState::Disabled,
            _ => return Err("conformance suite execution failed"),
        }
    };
    Ok((state, Some(revision)))
}

const fn command_outcome(outcome: DefinitionCommandOutcome) -> CommandAdoptionOutcome {
    match outcome {
        DefinitionCommandOutcome::Committed => CommandAdoptionOutcome::Committed,
        DefinitionCommandOutcome::Replayed => CommandAdoptionOutcome::Replayed,
        DefinitionCommandOutcome::Rejected => CommandAdoptionOutcome::Rejected,
    }
}

fn table_row_count(conn: &Connection, query: &str) -> Result<usize, &'static str> {
    conn.query_row(query, [], |row| row.get::<_, i64>(0))
        .map_err(|_| "conformance suite execution failed")
        .and_then(|count| usize::try_from(count).map_err(|_| "conformance suite execution failed"))
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

fn evaluate_misfire_latest_planning(vector: &Value) -> Result<bool, &'static str> {
    let vectors: MisfireLatestPlanningVectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != MISFIRE_LATEST_PLANNING_VECTOR_SCHEMA_VERSION
        || vectors.cases.is_empty()
        || vectors.cases.len() > MAX_CASES
    {
        return Err("conformance vector is invalid");
    }

    let mut case_ids = BTreeSet::new();
    let mut scenarios = BTreeSet::new();
    for case in &vectors.cases {
        if !valid_case_id(&case.case_id)
            || !case_ids.insert(&case.case_id)
            || !scenarios.insert(case.scenario)
            || !misfire_latest_planning_case_is_valid(case)
        {
            return Err("conformance vector is invalid");
        }
    }
    if scenarios.len() != MisfireLatestPlanningScenario::COUNT {
        return Err("conformance vector is invalid");
    }

    let mut all_passed = true;
    for case in &vectors.cases {
        all_passed &= misfire_latest_planning_case_matches(case)?;
    }
    Ok(all_passed)
}

fn misfire_latest_planning_case_is_valid(case: &MisfireLatestPlanningVectorCase) -> bool {
    let Ok(definition) = super::definition::RoutineDefinition::from_json(&case.definition) else {
        return false;
    };
    let Some(created_at) = canonical_timestamp(&case.created_at) else {
        return false;
    };
    let Some(observed_at) = canonical_timestamp(&case.observed_at) else {
        return false;
    };
    let existing = match case.existing_scheduled_for.as_deref() {
        Some(value) => {
            let Some(timestamp) = canonical_timestamp(value) else {
                return false;
            };
            Some(timestamp)
        }
        None => None,
    };
    let scheduled_slots = case
        .expected
        .scheduled_slots
        .iter()
        .map(|slot| canonical_timestamp(slot))
        .collect::<Option<Vec<_>>>();
    let Some(scheduled_slots) = scheduled_slots else {
        return false;
    };
    if scheduled_slots.windows(2).any(|slots| slots[0] >= slots[1])
        || definition.rrule != "FREQ=DAILY;BYHOUR=9"
        || definition.timezone != super::definition::RoutineTimezone::Utc
        || definition.misfire != super::definition::RoutineMisfire::Latest
        || definition.overlap != super::definition::RoutineOverlap::Forbid
    {
        return false;
    }

    let first_due = scheduled_after(&definition, created_at);
    let second_due = first_due.and_then(|slot| scheduled_after(&definition, slot));
    let latest_due = latest_scheduled_at_or_before(&definition, created_at, observed_at);
    let next_after_observation = scheduled_after(&definition, observed_at);

    match case.scenario {
        MisfireLatestPlanningScenario::RestartCollapseLatest => {
            definition.status == super::definition::RoutineStatus::Active
                && created_at < observed_at
                && second_due.is_some_and(|slot| slot <= observed_at)
                && existing.is_none()
                && case.expected.first_outcome == ExpectedPlanOutcome::Planned
                && case.expected.second_outcome == ExpectedPlanOutcome::AlreadyFenced
                && scheduled_slots.len() == 1
        }
        MisfireLatestPlanningScenario::ExistingFenceReplay => {
            definition.status == super::definition::RoutineStatus::Active
                && created_at < observed_at
                && existing.is_some_and(|slot| {
                    latest_due == Some(slot) && scheduled_slots.as_slice() == [slot]
                })
                && case.expected.first_outcome == ExpectedPlanOutcome::AlreadyFenced
                && case.expected.second_outcome == ExpectedPlanOutcome::AlreadyFenced
        }
        MisfireLatestPlanningScenario::ClockRollbackNoOlderFence => {
            definition.status == super::definition::RoutineStatus::Active
                && created_at < observed_at
                && first_due.is_some_and(|slot| slot <= observed_at)
                && existing.is_some_and(|slot| {
                    next_after_observation == Some(slot) && scheduled_slots.as_slice() == [slot]
                })
                && case.expected.first_outcome == ExpectedPlanOutcome::NotDue
                && case.expected.second_outcome == ExpectedPlanOutcome::NotDue
        }
        MisfireLatestPlanningScenario::PausedDefinition => {
            definition.status == super::definition::RoutineStatus::Paused
                && created_at < observed_at
                && first_due.is_some_and(|slot| slot <= observed_at)
                && existing.is_none()
                && case.expected.first_outcome == ExpectedPlanOutcome::NotDue
                && case.expected.second_outcome == ExpectedPlanOutcome::NotDue
                && scheduled_slots.is_empty()
        }
    }
}

fn scheduled_after(
    definition: &super::definition::RoutineDefinition,
    instant: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    super::schedule::next_due(&definition.rrule, definition.timezone, instant)
        .ok()
        .flatten()
}

fn latest_scheduled_at_or_before(
    definition: &super::definition::RoutineDefinition,
    created_at: DateTime<Utc>,
    observed_at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let bounded_start = observed_at.checked_sub_signed(Duration::days(15))?;
    let mut cursor = created_at.max(bounded_start);
    let mut latest = None;

    for _ in 0..=16 {
        let next = scheduled_after(definition, cursor)?;
        if next > observed_at {
            return latest;
        }
        latest = Some(next);
        cursor = next;
    }

    None
}

fn misfire_latest_planning_case_matches(
    case: &MisfireLatestPlanningVectorCase,
) -> Result<bool, &'static str> {
    let definition = super::definition::RoutineDefinition::from_json(&case.definition)
        .map_err(|_| "conformance vector is invalid")?;
    let created_at =
        canonical_timestamp(&case.created_at).ok_or("conformance vector is invalid")?;
    let observed_at =
        canonical_timestamp(&case.observed_at).ok_or("conformance vector is invalid")?;
    let conn = Connection::open_in_memory().map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch(super::store::AUTOMATION_DEFINITIONS_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;
    conn.execute_batch(super::occurrences::AUTOMATION_OCCURRENCES_SCHEMA_SQL)
        .map_err(|_| "conformance suite execution failed")?;
    let record = super::store::insert_definition(&conn, &definition)
        .map_err(|_| "conformance suite execution failed")?;
    conn.execute(
        "UPDATE automation_definitions
         SET created_at = ?2, updated_at = ?2
         WHERE id = ?1",
        params![definition.id, case.created_at],
    )
    .map_err(|_| "conformance suite execution failed")?;

    if let Some(existing) = case.existing_scheduled_for.as_deref() {
        let scheduled_at = canonical_timestamp(existing).ok_or("conformance vector is invalid")?;
        conn.execute(
            "INSERT INTO automation_occurrences
                (id, automation_id, automation_revision, definition_digest, scheduled_for, kind,
                 state, attempt, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'scheduled', 'planned', 0, ?6, ?6)",
            params![
                format!("{}-{}", definition.id, scheduled_at.timestamp_millis()),
                definition.id,
                i64::try_from(record.revision).map_err(|_| "conformance suite execution failed")?,
                record
                    .definition_digest
                    .as_deref()
                    .ok_or("conformance suite execution failed")?,
                existing,
                case.created_at,
            ],
        )
        .map_err(|_| "conformance suite execution failed")?;
    }

    let first =
        super::occurrences::plan_latest_due_occurrence(&conn, &definition, created_at, observed_at)
            .map_err(|_| "conformance suite execution failed")?;
    let second =
        super::occurrences::plan_latest_due_occurrence(&conn, &definition, created_at, observed_at)
            .map_err(|_| "conformance suite execution failed")?;
    let scheduled_slots = conn
        .prepare(
            "SELECT scheduled_for
             FROM automation_occurrences
             WHERE automation_id = ?1
             ORDER BY scheduled_for ASC",
        )
        .map_err(|_| "conformance suite execution failed")?
        .query_map([&definition.id], |row| row.get::<_, String>(0))
        .map_err(|_| "conformance suite execution failed")?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| "conformance suite execution failed")?;

    Ok(plan_outcome_matches(&first, case.expected.first_outcome)
        && plan_outcome_matches(&second, case.expected.second_outcome)
        && scheduled_slots == case.expected.scheduled_slots)
}

fn plan_outcome_matches(
    observed: &super::occurrences::PlanOutcome,
    expected: ExpectedPlanOutcome,
) -> bool {
    matches!(
        (observed, expected),
        (
            super::occurrences::PlanOutcome::Planned(_),
            ExpectedPlanOutcome::Planned
        ) | (
            super::occurrences::PlanOutcome::NotDue,
            ExpectedPlanOutcome::NotDue
        ) | (
            super::occurrences::PlanOutcome::AlreadyFenced,
            ExpectedPlanOutcome::AlreadyFenced
        )
    )
}

fn canonical_timestamp(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .filter(|timestamp| timestamp.to_rfc3339_opts(SecondsFormat::Millis, true) == value)
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

fn evaluate_receipt_integrity_validation(vector: &Value) -> Result<bool, &'static str> {
    let vectors: ReceiptIntegrityVectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != RECEIPT_INTEGRITY_VECTOR_SCHEMA_VERSION
        || vectors.cases.is_empty()
        || vectors.cases.len() > MAX_CASES
    {
        return Err("conformance vector is invalid");
    }

    let mut case_ids = BTreeSet::new();
    let mut has_accepted = false;
    let mut has_rejected = false;
    for case in &vectors.cases {
        match &case.expected {
            ExpectedReceiptIntegrity::Accepted { normalized_digest } => {
                has_accepted = true;
                if !valid_sha256_digest(normalized_digest) {
                    return Err("conformance vector is invalid");
                }
            }
            ExpectedReceiptIntegrity::Rejected => {
                has_rejected = true;
                if !structurally_valid_receipt(&case.receipt) {
                    return Err("conformance vector is invalid");
                }
            }
        }
        if !valid_case_id(&case.case_id)
            || !case_ids.insert(&case.case_id)
            || !case.receipt.is_object()
        {
            return Err("conformance vector is invalid");
        }
    }
    if !has_accepted || !has_rejected {
        return Err("conformance vector is invalid");
    }

    Ok(vectors.cases.iter().all(receipt_integrity_case_matches))
}

fn structurally_valid_receipt(value: &Value) -> bool {
    let mut candidate = value.clone();
    let Some(integrity_value) = candidate
        .get("integrity")
        .and_then(Value::as_object)
        .and_then(|integrity| integrity.get("value"))
        .and_then(Value::as_str)
    else {
        return false;
    };
    if Sha256Digest::new(integrity_value.to_owned()).is_err() {
        return false;
    }

    canonicalize_without_integrity(&candidate)
        .ok()
        .map(|canonical| {
            candidate["integrity"]["value"] = Value::String(sha256_hex(&canonical));
            candidate
        })
        .is_some_and(|candidate| serde_json::from_value::<AutomationReceipt>(candidate).is_ok())
}

fn receipt_integrity_case_matches(case: &ReceiptIntegrityVectorCase) -> bool {
    let parsed = serde_json::from_value::<AutomationReceipt>(case.receipt.clone());
    match (&case.expected, parsed) {
        (ExpectedReceiptIntegrity::Accepted { normalized_digest }, Ok(receipt)) => {
            serde_json::to_value(receipt)
                .ok()
                .and_then(|value| canonicalize(&value).ok())
                .map(|canonical| format!("sha256:{}", sha256_hex(&canonical)))
                .is_some_and(|observed| observed == *normalized_digest)
        }
        (ExpectedReceiptIntegrity::Rejected, Err(_)) => true,
        _ => false,
    }
}

fn evaluate_rrule_vocabulary(vector: &Value) -> Result<bool, &'static str> {
    let vectors: RruleVocabularyVectorSet =
        serde_json::from_value(vector.clone()).map_err(|_| "conformance vector is invalid")?;
    if vectors.schema_version != RRULE_VOCABULARY_VECTOR_SCHEMA_VERSION
        || vectors.cases.len() != RruleVocabularyScenario::COUNT
        || vectors.cases.len() > MAX_CASES
    {
        return Err("conformance vector is invalid");
    }

    let mut case_ids = BTreeSet::new();
    let mut scenarios = BTreeSet::new();
    let mut rules = BTreeSet::new();
    for case in &vectors.cases {
        if !valid_case_id(&case.case_id)
            || !case_ids.insert(&case.case_id)
            || !scenarios.insert(case.scenario)
            || case.rrule.is_empty()
            || case.rrule.len() > 1024
            || !rules.insert(&case.rrule)
            || case.rrule != case.scenario.rule()
            || !valid_rrule_expectation(&case.expected)
        {
            return Err("conformance vector is invalid");
        }
    }

    Ok(vectors.cases.iter().all(rrule_vocabulary_case_matches))
}

fn valid_rrule_expectation(expected: &ExpectedRruleVocabulary) -> bool {
    let ExpectedRruleVocabulary::Accepted {
        frequency,
        by_hour,
        by_day,
    } = expected
    else {
        return true;
    };
    let hours_valid = !by_hour.is_empty()
        && by_hour.iter().all(|hour| *hour <= 23)
        && by_hour.windows(2).all(|pair| pair[0] < pair[1]);
    let days_valid = by_day
        .iter()
        .all(|day| matches!(day.as_str(), "FR" | "MO" | "SA" | "SU" | "TH" | "TU" | "WE"))
        && by_day.windows(2).all(|pair| pair[0] < pair[1]);
    hours_valid
        && days_valid
        && match frequency {
            ExpectedRruleFrequency::Daily => by_day.is_empty(),
            ExpectedRruleFrequency::Weekly => !by_day.is_empty(),
        }
}

fn rrule_vocabulary_case_matches(case: &RruleVocabularyVectorCase) -> bool {
    match (&case.expected, parse_rrule(&case.rrule)) {
        (
            ExpectedRruleVocabulary::Accepted {
                frequency,
                by_hour,
                by_day,
            },
            Ok(parsed),
        ) => {
            let frequency_matches = matches!(
                (frequency, parsed.frequency),
                (ExpectedRruleFrequency::Daily, RruleFrequency::Daily)
                    | (ExpectedRruleFrequency::Weekly, RruleFrequency::Weekly)
            );
            frequency_matches
                && parsed.by_hour.as_slice() == by_hour.as_slice()
                && parsed.by_day.as_slice() == by_day.as_slice()
        }
        (ExpectedRruleVocabulary::Rejected, Err(_)) => true,
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
