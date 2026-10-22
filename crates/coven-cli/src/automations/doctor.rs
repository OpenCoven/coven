//! Minimized persisted observations. Missing evidence is never a healthy result.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[path = "doctor_capacity.rs"]
mod capacity;
#[path = "doctor_failure_history.rs"]
mod failure_history;
#[path = "doctor_planning.rs"]
mod planning;
#[path = "doctor_read.rs"]
mod read;
#[path = "doctor_wal.rs"]
mod wal;
pub(crate) use read::ReadBudgetExceeded;
#[path = "doctor_receipt_commitments.rs"]
mod receipt_commitments;
#[path = "doctor_receipt_links.rs"]
mod receipt_links;
#[path = "doctor_schema.rs"]
mod schema;
#[path = "doctor_sequences.rs"]
mod sequences;
#[path = "doctor_stop_fences.rs"]
mod stop_fences;

pub const ACTION: &str = "coven.automations.doctor.v1";
pub const SCHEMA: &str = "coven.automations.doctor.v1";
const MAX_SAFE: u64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Leadership,
    LastSuccessfulPass,
    Store,
    Queue,
    LeasesFences,
    PlanningStartLag,
    RepeatedFailures,
    AuthorityRefusals,
    DeliveryReceipts,
    EventChangefeed,
    DiskRetention,
}

pub const CATEGORIES: [Category; 11] = [
    Category::Leadership,
    Category::LastSuccessfulPass,
    Category::Store,
    Category::Queue,
    Category::LeasesFences,
    Category::PlanningStartLag,
    Category::RepeatedFailures,
    Category::AuthorityRefusals,
    Category::DeliveryReceipts,
    Category::EventChangefeed,
    Category::DiskRetention,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fact {
    LeaderAssigned,
    SchedulerGeneration,
    LatestPassSucceeded,
    LastSuccessfulPassFinishedAtMillis,
    LatestPassDurationMs,
    LatestPassStartLagMs,
    ScheduledOccurrencesPlanningChecked,
    MaximumScheduledOccurrencePlanningDelayMs,
    PresentAutomationTables,
    RequiredSchemaObjectPresenceChecks,
    MissingRequiredSchemaObjects,
    UnrecognizedRequiredSchemaObjectForms,
    IncompleteRequiredSchemaObjectChecks,
    UnexpectedAutomationSchemaObjects,
    RecordedContractMigrationPresent,
    ContractMigrationDefinitionsMigrated,
    ContractMigrationOccurrencesMigrated,
    ContractMigrationRunsMigrated,
    ContractMigrationUnverifiableDefinitions,
    ContractMigrationUnverifiableOccurrences,
    ContractMigrationUnverifiableRuns,
    ContractMigrationRecordedAtMillis,

    PlannedOccurrences,
    DuePlannedOccurrences,
    EligibleOccurrences,
    ClaimedOccurrences,
    RunningOccurrences,
    RecoveryRequiredOccurrences,
    LeaseBearingOccurrences,
    ActiveLeaseOccurrences,
    ExpiredLeaseOccurrences,
    MissingLeaseExpiryOccurrences,
    InvalidLeaseExpiryOccurrences,
    LeaseOutsideActiveStateOccurrences,
    RecordedSchedulerFencePresent,
    MismatchedUnlaunchedSchedulerClaims,
    StopFences,
    ActiveStopFences,
    ExpiredStopFences,
    InvalidStopFenceExpiries,
    QuarantinedRoutines,
    FailedAttempts,
    RetryExhaustionHistories,
    MaximumConsecutiveRetryExhaustions,
    RetainedOccurrenceFailureHistoriesChecked,
    RoutinesWithRepeatedRetainedFailuresSinceSuccess,
    MaximumRetainedFailuresSinceSuccess,
    LaunchRefusedAttempts,
    UnsupportedAuthorityAttempts,
    FailedDeliveries,
    PendingDeliveries,
    StoredReceipts,
    ReceiptCommitmentsChecked,
    InvalidReceiptCommitments,
    EventStreams,
    StoredEvents,
    StoredEventCheckpoints,
    UnexpiredEventCheckpoints,
    ExpiredEventCheckpoints,
    InvalidEventCheckpointExpiries,
    InvalidCheckpointCursorMetadata,
    CheckpointsWithoutRetentionBoundary,
    CheckpointsWithInvalidRetentionBoundary,
    CheckpointRetentionComparisons,
    CheckpointsBeforeRetention,
    EventSequenceStreamsChecked,
    EventSequenceStreamMismatches,
    EventSequenceRowsWithoutHead,
    EventSequenceRowsBeforeRetention,
    EventSequenceRowsBeyondHead,
    DatabasePages,
    PageSizeBytes,
    DatabaseFreelistPages,
    DatabaseLogicalBytes,
    DatabaseNonFreelistBytes,
    FilesystemAvailableBytesEstimate,
    FilesystemCapacitySampledAtMillis,
    DatabaseWalSidecarPresent,
    DatabaseWalSidecarBytesEstimate,
    DatabaseWalSidecarSampledAtMillis,
}

impl Fact {
    fn category(self) -> Category {
        match self {
            Self::LeaderAssigned | Self::SchedulerGeneration => Category::Leadership,
            Self::LatestPassSucceeded | Self::LastSuccessfulPassFinishedAtMillis => {
                Category::LastSuccessfulPass
            }
            Self::LatestPassDurationMs
            | Self::LatestPassStartLagMs
            | Self::ScheduledOccurrencesPlanningChecked
            | Self::MaximumScheduledOccurrencePlanningDelayMs => Category::PlanningStartLag,
            Self::PresentAutomationTables
            | Self::RequiredSchemaObjectPresenceChecks
            | Self::MissingRequiredSchemaObjects
            | Self::UnrecognizedRequiredSchemaObjectForms
            | Self::IncompleteRequiredSchemaObjectChecks
            | Self::UnexpectedAutomationSchemaObjects
            | Self::RecordedContractMigrationPresent
            | Self::ContractMigrationDefinitionsMigrated
            | Self::ContractMigrationOccurrencesMigrated
            | Self::ContractMigrationRunsMigrated
            | Self::ContractMigrationUnverifiableDefinitions
            | Self::ContractMigrationUnverifiableOccurrences
            | Self::ContractMigrationUnverifiableRuns
            | Self::ContractMigrationRecordedAtMillis => Category::Store,
            Self::PlannedOccurrences
            | Self::DuePlannedOccurrences
            | Self::EligibleOccurrences
            | Self::ClaimedOccurrences
            | Self::RunningOccurrences
            | Self::RecoveryRequiredOccurrences => Category::Queue,
            Self::LeaseBearingOccurrences
            | Self::ActiveLeaseOccurrences
            | Self::ExpiredLeaseOccurrences
            | Self::MissingLeaseExpiryOccurrences
            | Self::InvalidLeaseExpiryOccurrences
            | Self::LeaseOutsideActiveStateOccurrences
            | Self::RecordedSchedulerFencePresent
            | Self::MismatchedUnlaunchedSchedulerClaims
            | Self::StopFences
            | Self::ActiveStopFences
            | Self::ExpiredStopFences
            | Self::InvalidStopFenceExpiries => Category::LeasesFences,
            Self::QuarantinedRoutines
            | Self::FailedAttempts
            | Self::RetryExhaustionHistories
            | Self::MaximumConsecutiveRetryExhaustions
            | Self::RetainedOccurrenceFailureHistoriesChecked
            | Self::RoutinesWithRepeatedRetainedFailuresSinceSuccess
            | Self::MaximumRetainedFailuresSinceSuccess => Category::RepeatedFailures,
            Self::LaunchRefusedAttempts | Self::UnsupportedAuthorityAttempts => {
                Category::AuthorityRefusals
            }
            Self::FailedDeliveries
            | Self::PendingDeliveries
            | Self::StoredReceipts
            | Self::ReceiptCommitmentsChecked
            | Self::InvalidReceiptCommitments => Category::DeliveryReceipts,
            Self::EventStreams
            | Self::StoredEvents
            | Self::StoredEventCheckpoints
            | Self::UnexpiredEventCheckpoints
            | Self::ExpiredEventCheckpoints
            | Self::InvalidEventCheckpointExpiries
            | Self::InvalidCheckpointCursorMetadata
            | Self::CheckpointsWithoutRetentionBoundary
            | Self::CheckpointsWithInvalidRetentionBoundary
            | Self::CheckpointRetentionComparisons
            | Self::CheckpointsBeforeRetention
            | Self::EventSequenceStreamsChecked
            | Self::EventSequenceStreamMismatches
            | Self::EventSequenceRowsWithoutHead
            | Self::EventSequenceRowsBeforeRetention
            | Self::EventSequenceRowsBeyondHead => Category::EventChangefeed,
            Self::DatabasePages
            | Self::PageSizeBytes
            | Self::DatabaseFreelistPages
            | Self::DatabaseLogicalBytes
            | Self::DatabaseNonFreelistBytes
            | Self::FilesystemAvailableBytesEstimate
            | Self::FilesystemCapacitySampledAtMillis
            | Self::DatabaseWalSidecarPresent
            | Self::DatabaseWalSidecarBytesEstimate
            | Self::DatabaseWalSidecarSampledAtMillis => Category::DiskRetention,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gap {
    InvalidPersistedMetadata,
    InvalidLatestPassStatus,
    InvalidLatestPassDuration,
    LatestPassStatusUnavailable,
    LatestPassDurationUnavailable,
    LatestPassLagUnavailable,
    ProcessLivenessNotProven,
    PriorSuccessfulPassNotRecorded,
    SuccessfulPassStoreUnavailable,
    ContractMigrationStoreUnavailable,
    IntegrityAndMigrationsNotVerified,
    RuntimeAdmissionNotProven,
    EligibilityStoreUnavailable,
    EligibilityReadBudgetExceeded,
    DueQueueUnavailable,
    QueueStateUnavailable,
    InvalidOccurrenceState,
    FenceConsistencyNotMeasured,
    RecordedAuthorityNotObserved,
    RecordedAuthorityUnavailable,
    RecordedClaimStoreUnavailable,
    LeaseStoreUnavailable,
    LeaseStateUnavailable,
    StopFenceStoreUnavailable,
    LagHistoryNotMeasured,
    OccurrencePlanningStoreUnavailable,
    OccurrencePlanningMetadataInvalid,
    OccurrencePlanningReadBudgetExceeded,
    FailureHistoryNotSummarized,
    OccurrenceFailureHistoryStoreUnavailable,
    OccurrenceFailureHistoryMetadataInvalid,
    OccurrenceFailureHistoryReadBudgetExceeded,
    RetryExhaustionHistoryUnavailable,
    QuarantineStoreUnavailable,
    AttemptStateUnavailable,
    InvalidAttemptState,
    IdentityAndApprovalRefusalsNotRecorded,
    AttemptFailureClassUnavailable,
    InvalidAttemptFailureClass,
    ReceiptIntegrityNotVerified,
    ReceiptAuthorityNotVerified,
    ReceiptCommitmentsUnavailable,
    ReceiptCommitmentsMetadataInvalid,
    ReceiptCommitmentsReadBudgetExceeded,
    ReceiptCommitmentsInconsistent,
    ReceiptStoreUnavailable,
    DeliveryStateUnavailable,
    InvalidDeliveryState,
    ReceiptLinkStoreUnavailable,
    ReceiptLinkMetadataInvalid,
    ReceiptLinkReadBudgetExceeded,
    ReceiptLinksInconsistent,
    SubscriberReplayNotObserved,
    EventStoreUnavailable,
    CheckpointStoreUnavailable,
    CheckpointExpiryUnavailable,
    CheckpointCursorMetadataUnavailable,
    CheckpointRetentionStoreUnavailable,
    EventSequenceStoreUnavailable,
    EventSequenceMetadataInvalid,
    EventSequenceReadBudgetExceeded,
    RetentionPressureNotObserved,
    FilesystemCapacityUnavailable,
    FilesystemCapacityMetadataInvalid,
    WalSidecarUnavailable,
    WalSidecarMetadataInvalid,
}

#[derive(Default)]
struct StorageGapScope {
    capacity_incomplete: bool,
    wal_incomplete: bool,
}
impl StorageGapScope {
    fn from_gaps(gaps: &[Gap]) -> Option<Self> {
        let mut scope = Self::default();
        let mut remaining = gaps;
        if matches!(
            remaining.first(),
            Some(Gap::FilesystemCapacityUnavailable | Gap::FilesystemCapacityMetadataInvalid)
        ) {
            scope.capacity_incomplete = true;
            remaining = &remaining[1..];
        }
        if matches!(
            remaining.first(),
            Some(Gap::WalSidecarUnavailable | Gap::WalSidecarMetadataInvalid)
        ) {
            scope.wal_incomplete = true;
            remaining = &remaining[1..];
        }
        (remaining == [Gap::RetentionPressureNotObserved]).then_some(scope)
    }
}

// Independent failure-query gaps have one canonical order. Parse this closed
// grammar once so combinations cannot silently widen the required fact set.
#[derive(Default)]
struct FailureGapScope {
    attempts_unavailable: bool,
    quarantine_unavailable: bool,
    history_unavailable: bool,
    history_invalid: bool,
}

impl FailureGapScope {
    fn from_gaps(gaps: &[Gap]) -> Option<Self> {
        if gaps == [Gap::FailureHistoryNotSummarized] {
            return Some(Self::default());
        }
        let mut scope = Self::default();
        let mut remaining = gaps;
        if let Some(rest) = remaining.strip_prefix(&[Gap::AttemptStateUnavailable]) {
            scope.attempts_unavailable = true;
            remaining = rest;
        }
        if let Some(rest) = remaining.strip_prefix(&[Gap::InvalidAttemptState]) {
            if scope.attempts_unavailable {
                return None;
            }
            scope.attempts_unavailable = true;
            remaining = rest;
        }
        if let Some(rest) = remaining.strip_prefix(&[Gap::QuarantineStoreUnavailable]) {
            scope.quarantine_unavailable = true;
            remaining = rest;
        }
        match remaining {
            [] => {}
            [Gap::RetryExhaustionHistoryUnavailable] => scope.history_unavailable = true,
            [Gap::InvalidPersistedMetadata] => scope.history_invalid = true,
            _ => return None,
        }
        (scope.attempts_unavailable
            || scope.quarantine_unavailable
            || scope.history_unavailable
            || scope.history_invalid)
            .then_some(scope)
    }
}

#[derive(Default)]
struct DeliveryGapScope {
    state_unavailable: bool,
    receipts_unavailable: bool,
    commitments_checked: bool,
    inconsistent_commitments: bool,
}
impl DeliveryGapScope {
    fn from_gaps(gaps: &[Gap]) -> Option<Self> {
        if gaps == [Gap::ReceiptIntegrityNotVerified] {
            return Some(Self::default());
        }
        let mut scope = Self::default();
        let has_commitment_scope = gaps.last() == Some(&Gap::ReceiptAuthorityNotVerified);
        let mut remaining = gaps;
        if has_commitment_scope {
            remaining = &remaining[..remaining.len() - 1];
            match remaining.last() {
                Some(Gap::ReceiptCommitmentsInconsistent) => {
                    scope.commitments_checked = true;
                    scope.inconsistent_commitments = true;
                    remaining = &remaining[..remaining.len() - 1];
                }
                Some(
                    Gap::ReceiptCommitmentsUnavailable
                    | Gap::ReceiptCommitmentsMetadataInvalid
                    | Gap::ReceiptCommitmentsReadBudgetExceeded,
                ) => {
                    remaining = &remaining[..remaining.len() - 1];
                }
                _ => {
                    scope.commitments_checked = true;
                }
            }
        }
        if matches!(
            remaining.first(),
            Some(Gap::DeliveryStateUnavailable | Gap::InvalidDeliveryState)
        ) {
            scope.state_unavailable = true;
            remaining = &remaining[1..];
        }
        if let Some(rest) = remaining.strip_prefix(&[Gap::ReceiptStoreUnavailable]) {
            if has_commitment_scope {
                return None;
            }
            scope.receipts_unavailable = true;
            remaining = rest;
        }
        if scope.receipts_unavailable && remaining != [Gap::ReceiptLinkStoreUnavailable] {
            return None;
        }
        let links_incomplete = match remaining {
            [] => false,
            [Gap::ReceiptLinkStoreUnavailable
            | Gap::ReceiptLinkMetadataInvalid
            | Gap::ReceiptLinkReadBudgetExceeded
            | Gap::ReceiptLinksInconsistent] => true,
            _ => return None,
        };
        (has_commitment_scope
            || scope.state_unavailable
            || scope.receipts_unavailable
            || links_incomplete)
            .then_some(scope)
    }
}

#[derive(Default)]
struct EventGapScope {
    events_unavailable: bool,
    checkpoints_unavailable: bool,
    expiry_unavailable: bool,
    cursor_unavailable: bool,
    retention_unavailable: bool,
    sequence_incomplete: bool,
}

impl EventGapScope {
    fn from_gaps(gaps: &[Gap]) -> Option<Self> {
        if gaps == [Gap::SubscriberReplayNotObserved] {
            return Some(Self::default());
        }
        let mut scope = Self::default();
        let mut remaining = gaps;
        if let Some(rest) = remaining.strip_prefix(&[Gap::EventStoreUnavailable]) {
            scope.events_unavailable = true;
            remaining = rest;
        }
        if let Some(rest) = remaining.strip_prefix(&[Gap::CheckpointStoreUnavailable]) {
            scope.checkpoints_unavailable = true;
            remaining = rest;
        } else {
            if let Some(rest) = remaining.strip_prefix(&[Gap::CheckpointExpiryUnavailable]) {
                scope.expiry_unavailable = true;
                remaining = rest;
            }
            if let Some(rest) = remaining.strip_prefix(&[Gap::CheckpointCursorMetadataUnavailable])
            {
                scope.cursor_unavailable = true;
                remaining = rest;
            } else if let Some(rest) =
                remaining.strip_prefix(&[Gap::CheckpointRetentionStoreUnavailable])
            {
                scope.retention_unavailable = true;
                remaining = rest;
            }
        }
        match remaining {
            [] => {}
            [Gap::EventSequenceStoreUnavailable
            | Gap::EventSequenceMetadataInvalid
            | Gap::EventSequenceReadBudgetExceeded] => scope.sequence_incomplete = true,
            _ => return None,
        }
        (scope.events_unavailable
            || scope.checkpoints_unavailable
            || scope.expiry_unavailable
            || scope.cursor_unavailable
            || scope.retention_unavailable
            || scope.sequence_incomplete)
            .then_some(scope)
    }
}

// Cohort loss subsumes unavailable state classification. Authority outcomes
// remain independent, but exactly one authority suffix is permitted.
#[derive(Default)]
struct LeaseGapScope {
    cohort_unavailable: bool,
    state_unavailable: bool,
    authority_unavailable: bool,
    authority_invalid: bool,
    claims_unavailable: bool,
}

impl LeaseGapScope {
    fn from_gaps(gaps: &[Gap]) -> Option<Self> {
        if gaps == [Gap::FenceConsistencyNotMeasured] {
            return Some(Self::default());
        }
        let mut scope = Self::default();
        let mut remaining = gaps;
        if let Some(rest) = remaining.strip_prefix(&[Gap::LeaseStoreUnavailable]) {
            scope.cohort_unavailable = true;
            remaining = rest;
        } else if let Some(rest) = remaining.strip_prefix(&[Gap::LeaseStateUnavailable]) {
            scope.state_unavailable = true;
            remaining = rest;
        }
        match remaining {
            [] => {}
            [Gap::RecordedAuthorityUnavailable] => scope.authority_unavailable = true,
            [Gap::InvalidPersistedMetadata] => scope.authority_invalid = true,
            [Gap::RecordedClaimStoreUnavailable] => scope.claims_unavailable = true,
            _ => return None,
        }
        (scope.cohort_unavailable
            || scope.state_unavailable
            || scope.authority_unavailable
            || scope.authority_invalid
            || scope.claims_unavailable)
            .then_some(scope)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextStep {
    InspectSchedulerStatus,
    InspectDaemonStatus,
    InspectLeaseMetadata,
    InspectRunAndAttempts,
    ReviewAuthorityGate,
    ReviewChangefeedEvidence,
    ReviewStorageMaintenance,
}

impl Category {
    fn contract(self) -> (&'static [Fact], Gap, NextStep) {
        use Fact::*;
        use Gap::*;
        use NextStep::*;
        match self {
            Self::Leadership => (
                &[LeaderAssigned, SchedulerGeneration],
                ProcessLivenessNotProven,
                InspectSchedulerStatus,
            ),
            Self::LastSuccessfulPass => (
                &[LatestPassSucceeded],
                PriorSuccessfulPassNotRecorded,
                InspectSchedulerStatus,
            ),
            Self::Store => (
                &[
                    PresentAutomationTables,
                    RequiredSchemaObjectPresenceChecks,
                    MissingRequiredSchemaObjects,
                    UnrecognizedRequiredSchemaObjectForms,
                    IncompleteRequiredSchemaObjectChecks,
                    UnexpectedAutomationSchemaObjects,
                    RecordedContractMigrationPresent,
                    ContractMigrationDefinitionsMigrated,
                    ContractMigrationOccurrencesMigrated,
                    ContractMigrationRunsMigrated,
                    ContractMigrationUnverifiableDefinitions,
                    ContractMigrationUnverifiableOccurrences,
                    ContractMigrationUnverifiableRuns,
                    ContractMigrationRecordedAtMillis,
                ],
                IntegrityAndMigrationsNotVerified,
                InspectDaemonStatus,
            ),
            Self::Queue => (
                &[
                    PlannedOccurrences,
                    DuePlannedOccurrences,
                    EligibleOccurrences,
                    ClaimedOccurrences,
                    RunningOccurrences,
                    RecoveryRequiredOccurrences,
                ],
                RuntimeAdmissionNotProven,
                InspectSchedulerStatus,
            ),
            Self::LeasesFences => (
                &[
                    LeaseBearingOccurrences,
                    ActiveLeaseOccurrences,
                    ExpiredLeaseOccurrences,
                    MissingLeaseExpiryOccurrences,
                    InvalidLeaseExpiryOccurrences,
                    LeaseOutsideActiveStateOccurrences,
                    RecordedSchedulerFencePresent,
                    MismatchedUnlaunchedSchedulerClaims,
                ],
                FenceConsistencyNotMeasured,
                InspectLeaseMetadata,
            ),
            Self::PlanningStartLag => (
                &[LatestPassDurationMs, LatestPassStartLagMs],
                LagHistoryNotMeasured,
                InspectSchedulerStatus,
            ),
            Self::RepeatedFailures => (
                &[
                    QuarantinedRoutines,
                    FailedAttempts,
                    RetryExhaustionHistories,
                    MaximumConsecutiveRetryExhaustions,
                ],
                FailureHistoryNotSummarized,
                InspectRunAndAttempts,
            ),
            Self::AuthorityRefusals => (
                &[LaunchRefusedAttempts, UnsupportedAuthorityAttempts],
                IdentityAndApprovalRefusalsNotRecorded,
                ReviewAuthorityGate,
            ),
            Self::DeliveryReceipts => (
                &[FailedDeliveries, PendingDeliveries, StoredReceipts],
                ReceiptIntegrityNotVerified,
                InspectRunAndAttempts,
            ),
            Self::EventChangefeed => (
                &[
                    EventStreams,
                    StoredEvents,
                    StoredEventCheckpoints,
                    UnexpiredEventCheckpoints,
                    ExpiredEventCheckpoints,
                    InvalidEventCheckpointExpiries,
                    InvalidCheckpointCursorMetadata,
                    CheckpointsWithoutRetentionBoundary,
                    CheckpointsWithInvalidRetentionBoundary,
                    CheckpointRetentionComparisons,
                    CheckpointsBeforeRetention,
                    EventSequenceStreamsChecked,
                    EventSequenceStreamMismatches,
                    EventSequenceRowsWithoutHead,
                    EventSequenceRowsBeforeRetention,
                    EventSequenceRowsBeyondHead,
                ],
                SubscriberReplayNotObserved,
                ReviewChangefeedEvidence,
            ),
            Self::DiskRetention => (
                &[
                    DatabasePages,
                    PageSizeBytes,
                    DatabaseFreelistPages,
                    DatabaseLogicalBytes,
                    DatabaseNonFreelistBytes,
                    FilesystemAvailableBytesEstimate,
                    FilesystemCapacitySampledAtMillis,
                    DatabaseWalSidecarPresent,
                    DatabaseWalSidecarBytesEstimate,
                    DatabaseWalSidecarSampledAtMillis,
                ],
                RetentionPressureNotObserved,
                ReviewStorageMaintenance,
            ),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Observation {
    pub category: Category,
    pub facts: BTreeMap<Fact, u64>,
    pub gaps: Vec<Gap>,
    pub next_step: NextStep,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Report {
    pub schema_version: String,
    pub observed_at: String,
    pub observations: Vec<Observation>,
}

impl Report {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == SCHEMA && self.observed_at.len() <= 64,
            "invalid doctor report"
        );
        DateTime::parse_from_rfc3339(&self.observed_at)?;
        ensure!(
            self.observations.len() == CATEGORIES.len(),
            "invalid doctor categories"
        );
        let categories = self
            .observations
            .iter()
            .map(|value| value.category)
            .collect::<BTreeSet<_>>();
        ensure!(
            categories == CATEGORIES.into_iter().collect(),
            "invalid doctor categories"
        );
        for observation in &self.observations {
            let mut core = observation.clone();
            if core.category == Category::PlanningStartLag {
                planning::validate_and_strip(&mut core)?;
            } else if core.category == Category::RepeatedFailures {
                failure_history::validate_and_strip(&mut core)?;
            } else if core.category == Category::LeasesFences {
                stop_fences::validate_and_strip(&mut core)?;
            }
            let observation = &core;
            ensure!(
                observation
                    .facts
                    .iter()
                    .all(|(fact, count)| fact.category() == observation.category
                        && *count <= MAX_SAFE),
                "invalid doctor facts"
            );
            let (facts, gap, next_step) = observation.category.contract();
            let has_success = observation.category == Category::LastSuccessfulPass
                && observation
                    .facts
                    .contains_key(&Fact::LastSuccessfulPassFinishedAtMillis);
            let unavailable_status = observation.category == Category::LastSuccessfulPass
                && (observation.gaps == [Gap::LatestPassStatusUnavailable]
                    || observation.gaps
                        == [
                            Gap::LatestPassStatusUnavailable,
                            Gap::SuccessfulPassStoreUnavailable,
                        ]
                    || observation.gaps
                        == [
                            Gap::LatestPassStatusUnavailable,
                            Gap::InvalidPersistedMetadata,
                        ]);
            let missing_success_store = observation.category == Category::LastSuccessfulPass
                && (observation.gaps == [Gap::SuccessfulPassStoreUnavailable]
                    || observation.gaps
                        == [
                            Gap::LatestPassStatusUnavailable,
                            Gap::SuccessfulPassStoreUnavailable,
                        ]);
            let invalid_success = observation.category == Category::LastSuccessfulPass
                && observation.gaps == [Gap::InvalidPersistedMetadata];
            let failure_scope = if observation.category == Category::RepeatedFailures {
                FailureGapScope::from_gaps(&observation.gaps)
            } else {
                None
            };
            let unavailable_attempts = failure_scope
                .as_ref()
                .is_some_and(|scope| scope.attempts_unavailable);
            let unavailable_quarantine = failure_scope
                .as_ref()
                .is_some_and(|scope| scope.quarantine_unavailable);
            let unavailable_retry_counters = failure_scope
                .as_ref()
                .is_some_and(|scope| scope.history_unavailable);
            let invalid_retry_counters = failure_scope
                .as_ref()
                .is_some_and(|scope| scope.history_invalid);
            let invalid_occurrence_state = observation.category == Category::Queue
                && observation.gaps == [Gap::InvalidOccurrenceState];
            let unavailable_queue_states = observation.category == Category::Queue
                && observation.gaps
                    == [
                        Gap::QueueStateUnavailable,
                        Gap::DueQueueUnavailable,
                        Gap::EligibilityStoreUnavailable,
                    ];
            let unavailable_due = observation.category == Category::Queue
                && (observation.gaps
                    == [Gap::DueQueueUnavailable, Gap::EligibilityStoreUnavailable]
                    || unavailable_queue_states);
            let unavailable_eligibility = observation.category == Category::Queue
                && (observation.gaps == [Gap::EligibilityStoreUnavailable]
                    || observation.gaps == [Gap::EligibilityReadBudgetExceeded]
                    || unavailable_due);
            let invalid_queue = observation.category == Category::Queue
                && observation.gaps == [Gap::InvalidPersistedMetadata];
            let unavailable_duration = observation.category == Category::PlanningStartLag
                && (observation.gaps == [Gap::LatestPassDurationUnavailable]
                    || observation.gaps
                        == [
                            Gap::LatestPassDurationUnavailable,
                            Gap::InvalidPersistedMetadata,
                        ]
                    || observation.gaps
                        == [
                            Gap::LatestPassDurationUnavailable,
                            Gap::LatestPassLagUnavailable,
                        ]);
            let unavailable_lag = observation.category == Category::PlanningStartLag
                && (observation.gaps == [Gap::LatestPassLagUnavailable]
                    || observation.gaps
                        == [
                            Gap::LatestPassDurationUnavailable,
                            Gap::LatestPassLagUnavailable,
                        ]
                    || observation.gaps
                        == [
                            Gap::InvalidLatestPassDuration,
                            Gap::LatestPassLagUnavailable,
                        ]);
            let invalid_lag = observation.category == Category::PlanningStartLag
                && (observation.gaps == [Gap::InvalidPersistedMetadata]
                    || observation.gaps
                        == [
                            Gap::LatestPassDurationUnavailable,
                            Gap::InvalidPersistedMetadata,
                        ]);
            let lease_scope = if observation.category == Category::LeasesFences {
                LeaseGapScope::from_gaps(&observation.gaps)
            } else {
                None
            };
            let unavailable_lease_cohort = lease_scope
                .as_ref()
                .is_some_and(|scope| scope.cohort_unavailable);
            let unavailable_lease_state = lease_scope
                .as_ref()
                .is_some_and(|scope| scope.state_unavailable);
            let invalid_authority = (observation.category == Category::Leadership
                && observation.gaps == [Gap::InvalidPersistedMetadata])
                || lease_scope
                    .as_ref()
                    .is_some_and(|scope| scope.authority_invalid);
            let invalid_pass_status = observation.category == Category::LastSuccessfulPass
                && observation.gaps == [Gap::InvalidLatestPassStatus];
            let invalid_pass_duration = observation.category == Category::PlanningStartLag
                && (observation.gaps == [Gap::InvalidLatestPassDuration]
                    || observation.gaps
                        == [
                            Gap::InvalidLatestPassDuration,
                            Gap::LatestPassLagUnavailable,
                        ]);
            let unavailable_migration = observation.category == Category::Store
                && observation.gaps == [Gap::ContractMigrationStoreUnavailable];
            let delivery_scope = if observation.category == Category::DeliveryReceipts {
                DeliveryGapScope::from_gaps(&observation.gaps)
            } else {
                None
            };
            let unavailable_receipts = delivery_scope
                .as_ref()
                .is_some_and(|scope| scope.receipts_unavailable);
            let unavailable_delivery_state = delivery_scope
                .as_ref()
                .is_some_and(|scope| scope.state_unavailable);
            let storage_scope = if observation.category == Category::DiskRetention {
                StorageGapScope::from_gaps(&observation.gaps)
            } else {
                None
            };
            let capacity_incomplete = storage_scope
                .as_ref()
                .is_some_and(|s| s.capacity_incomplete);
            let wal_incomplete = storage_scope.as_ref().is_some_and(|s| s.wal_incomplete);
            let unavailable_refusals = observation.category == Category::AuthorityRefusals
                && matches!(
                    observation.gaps.as_slice(),
                    [Gap::AttemptFailureClassUnavailable | Gap::InvalidAttemptFailureClass]
                );
            let event_scope = if observation.category == Category::EventChangefeed {
                EventGapScope::from_gaps(&observation.gaps)
            } else {
                None
            };
            let unavailable_events = event_scope
                .as_ref()
                .is_some_and(|scope| scope.events_unavailable);
            let invalid_migration = observation.category == Category::Store
                && observation.gaps == [Gap::InvalidPersistedMetadata];
            let unavailable_authority = (observation.category == Category::Leadership
                && observation.gaps == [Gap::RecordedAuthorityUnavailable])
                || lease_scope
                    .as_ref()
                    .is_some_and(|scope| scope.authority_unavailable);
            let unavailable_claims = lease_scope
                .as_ref()
                .is_some_and(|scope| scope.claims_unavailable);
            let absent_authority = observation.category == Category::Leadership
                && observation.gaps == [Gap::RecordedAuthorityNotObserved];
            ensure!(
                (if has_success {
                    observation.gaps.is_empty()
                        || observation.gaps == [Gap::LatestPassStatusUnavailable]
                } else {
                    invalid_pass_status
                        || unavailable_status
                        || invalid_pass_duration
                        || invalid_migration
                        || event_scope.is_some()
                        || storage_scope.is_some()
                        || unavailable_refusals
                        || delivery_scope.is_some()
                        || unavailable_migration
                        || lease_scope.is_some()
                        || invalid_authority
                        || unavailable_authority
                        || unavailable_claims
                        || absent_authority
                        || invalid_success
                        || missing_success_store
                        || failure_scope.is_some()
                        || invalid_lag
                        || unavailable_duration
                        || unavailable_lag
                        || invalid_queue
                        || invalid_occurrence_state
                        || unavailable_eligibility
                        || observation.gaps == [gap]
                }) && observation.next_step == next_step,
                "invalid doctor evidence scope"
            );
            let mut required_facts = facts.iter().copied().collect::<BTreeSet<_>>();
            if unavailable_status {
                required_facts.remove(&Fact::LatestPassSucceeded);
            }
            if unavailable_duration {
                required_facts.remove(&Fact::LatestPassDurationMs);
            }
            if unavailable_lag {
                required_facts.remove(&Fact::LatestPassStartLagMs);
            }

            if unavailable_refusals {
                required_facts.clear();
            }
            if unavailable_receipts {
                required_facts.remove(&Fact::StoredReceipts);
            }
            if unavailable_delivery_state {
                required_facts.remove(&Fact::FailedDeliveries);
                required_facts.remove(&Fact::PendingDeliveries);
            }

            if let Some(scope) = &delivery_scope {
                if scope.commitments_checked {
                    required_facts.insert(Fact::ReceiptCommitmentsChecked);
                    required_facts.insert(Fact::InvalidReceiptCommitments);
                    let checked = observation.facts.get(&Fact::ReceiptCommitmentsChecked);
                    let invalid = observation.facts.get(&Fact::InvalidReceiptCommitments);
                    ensure!(
                        checked == observation.facts.get(&Fact::StoredReceipts)
                            && checked.is_some_and(|n| *n <= receipt_commitments::MAX_ROWS as u64)
                            && invalid.zip(checked).is_some_and(|(bad, total)| bad <= total
                                && (*bad > 0) == scope.inconsistent_commitments),
                        "invalid receipt commitment counts or finding"
                    );
                }
            }

            if delivery_scope.is_some()
                && !observation.gaps.iter().any(|gap| {
                    matches!(
                        gap,
                        Gap::ReceiptLinkStoreUnavailable
                            | Gap::ReceiptLinkMetadataInvalid
                            | Gap::ReceiptLinkReadBudgetExceeded
                    )
                })
            {
                ensure!(
                    observation
                        .facts
                        .get(&Fact::StoredReceipts)
                        .is_some_and(|count| *count <= receipt_links::MAX_ROWS as u64),
                    "observed receipt links exceed scan row cap"
                );
                if let (Some(failed), Some(pending)) = (
                    observation.facts.get(&Fact::FailedDeliveries),
                    observation.facts.get(&Fact::PendingDeliveries),
                ) {
                    ensure!(
                        failed + pending <= receipt_links::MAX_ROWS as u64,
                        "observed receipt links exceed run scan row cap"
                    );
                }
            }

            if capacity_incomplete {
                required_facts.remove(&Fact::FilesystemAvailableBytesEstimate);
                required_facts.remove(&Fact::FilesystemCapacitySampledAtMillis);
            }
            if wal_incomplete {
                required_facts.remove(&Fact::DatabaseWalSidecarPresent);
                required_facts.remove(&Fact::DatabaseWalSidecarBytesEstimate);
                required_facts.remove(&Fact::DatabaseWalSidecarSampledAtMillis);
            }
            if unavailable_events {
                let sources = [Fact::EventStreams, Fact::StoredEvents];
                ensure!(
                    sources
                        .iter()
                        .any(|fact| !observation.facts.contains_key(fact)),
                    "unavailable event store has complete facts"
                );
                for fact in sources {
                    if !observation.facts.contains_key(&fact) {
                        required_facts.remove(&fact);
                    }
                }
            }
            if let Some(scope) = &event_scope {
                if scope.sequence_incomplete {
                    for fact in [
                        Fact::EventSequenceStreamsChecked,
                        Fact::EventSequenceStreamMismatches,
                        Fact::EventSequenceRowsWithoutHead,
                        Fact::EventSequenceRowsBeforeRetention,
                        Fact::EventSequenceRowsBeyondHead,
                    ] {
                        required_facts.remove(&fact);
                    }
                }
                if scope.checkpoints_unavailable {
                    required_facts.remove(&Fact::StoredEventCheckpoints);
                }
                if scope.checkpoints_unavailable || scope.cursor_unavailable {
                    required_facts.remove(&Fact::InvalidCheckpointCursorMetadata);
                }
                if scope.checkpoints_unavailable
                    || scope.cursor_unavailable
                    || scope.retention_unavailable
                {
                    for fact in [
                        Fact::CheckpointsWithoutRetentionBoundary,
                        Fact::CheckpointsWithInvalidRetentionBoundary,
                        Fact::CheckpointRetentionComparisons,
                        Fact::CheckpointsBeforeRetention,
                    ] {
                        required_facts.remove(&fact);
                    }
                }
                if scope.checkpoints_unavailable || scope.expiry_unavailable {
                    for fact in [
                        Fact::UnexpiredEventCheckpoints,
                        Fact::ExpiredEventCheckpoints,
                        Fact::InvalidEventCheckpointExpiries,
                    ] {
                        required_facts.remove(&fact);
                    }
                }
            }

            if observation.category == Category::Leadership
                && (invalid_authority || unavailable_authority || absent_authority)
            {
                required_facts.clear();
            }
            if observation.category == Category::LeasesFences {
                if unavailable_lease_state
                    && observation.facts.get(&Fact::RecordedSchedulerFencePresent) == Some(&1)
                {
                    ensure!(
                        unavailable_claims,
                        "unavailable lease state cannot have measured scheduler claims"
                    );
                }
                if unavailable_lease_cohort {
                    for fact in [
                        Fact::LeaseBearingOccurrences,
                        Fact::ActiveLeaseOccurrences,
                        Fact::ExpiredLeaseOccurrences,
                        Fact::MissingLeaseExpiryOccurrences,
                        Fact::InvalidLeaseExpiryOccurrences,
                    ] {
                        required_facts.remove(&fact);
                    }
                }
                if unavailable_lease_cohort || unavailable_lease_state {
                    required_facts.remove(&Fact::LeaseOutsideActiveStateOccurrences);
                }
                if invalid_authority || unavailable_authority {
                    required_facts.remove(&Fact::RecordedSchedulerFencePresent);
                    required_facts.remove(&Fact::MismatchedUnlaunchedSchedulerClaims);
                } else if unavailable_claims {
                    ensure!(
                        observation.facts.get(&Fact::RecordedSchedulerFencePresent) == Some(&1),
                        "unavailable claim count requires recorded authority"
                    );
                    required_facts.remove(&Fact::MismatchedUnlaunchedSchedulerClaims);
                } else if observation.facts.get(&Fact::RecordedSchedulerFencePresent) == Some(&0) {
                    required_facts.remove(&Fact::MismatchedUnlaunchedSchedulerClaims);
                }
            }
            if observation.category == Category::Store {
                if invalid_migration || unavailable_migration {
                    required_facts.retain(|fact| {
                        matches!(
                            fact,
                            Fact::PresentAutomationTables
                                | Fact::RequiredSchemaObjectPresenceChecks
                                | Fact::MissingRequiredSchemaObjects
                                | Fact::UnrecognizedRequiredSchemaObjectForms
                                | Fact::IncompleteRequiredSchemaObjectChecks
                                | Fact::UnexpectedAutomationSchemaObjects
                        )
                    });
                } else if observation
                    .facts
                    .get(&Fact::RecordedContractMigrationPresent)
                    == Some(&0)
                {
                    required_facts.retain(|fact| {
                        matches!(
                            fact,
                            Fact::PresentAutomationTables
                                | Fact::RecordedContractMigrationPresent
                                | Fact::RequiredSchemaObjectPresenceChecks
                                | Fact::MissingRequiredSchemaObjects
                                | Fact::UnrecognizedRequiredSchemaObjectForms
                                | Fact::IncompleteRequiredSchemaObjectChecks
                                | Fact::UnexpectedAutomationSchemaObjects
                        )
                    });
                }
            }
            if invalid_pass_status || invalid_pass_duration || invalid_occurrence_state {
                required_facts.clear();
            }
            if unavailable_queue_states {
                for fact in [
                    Fact::PlannedOccurrences,
                    Fact::ClaimedOccurrences,
                    Fact::RunningOccurrences,
                    Fact::RecoveryRequiredOccurrences,
                ] {
                    required_facts.remove(&fact);
                }
            }
            if invalid_queue || unavailable_eligibility {
                required_facts.remove(&Fact::EligibleOccurrences);
            }
            if unavailable_due {
                required_facts.remove(&Fact::DuePlannedOccurrences);
            }
            if invalid_lag {
                required_facts.remove(&Fact::LatestPassStartLagMs);
            }
            if unavailable_attempts {
                required_facts.remove(&Fact::FailedAttempts);
            }
            if unavailable_quarantine {
                required_facts.remove(&Fact::QuarantinedRoutines);
            }
            if invalid_retry_counters || unavailable_retry_counters {
                required_facts.remove(&Fact::RetryExhaustionHistories);
                required_facts.remove(&Fact::MaximumConsecutiveRetryExhaustions);
            }
            if has_success {
                required_facts.insert(Fact::LastSuccessfulPassFinishedAtMillis);
            }
            // An invalid lag retains an independently observed duration. Only
            // a normal missing pass permits an empty planning observation.
            let optional_pass = observation.category == Category::LastSuccessfulPass
                || (observation.category == Category::PlanningStartLag && !invalid_lag);
            ensure!(
                (optional_pass && observation.facts.is_empty())
                    || observation.facts.keys().copied().collect::<BTreeSet<_>>() == required_facts,
                "missing doctor facts"
            );
            if observation.category == Category::EventChangefeed
                && observation
                    .facts
                    .contains_key(&Fact::UnexpiredEventCheckpoints)
            {
                let total = [
                    Fact::UnexpiredEventCheckpoints,
                    Fact::ExpiredEventCheckpoints,
                    Fact::InvalidEventCheckpointExpiries,
                ]
                .iter()
                .map(|fact| observation.facts[fact])
                .sum::<u64>();
                ensure!(
                    total == observation.facts[&Fact::StoredEventCheckpoints],
                    "invalid checkpoint expiry partition"
                );
            }
            if let Some(invalid) = observation
                .facts
                .get(&Fact::InvalidCheckpointCursorMetadata)
            {
                let total = observation.facts[&Fact::StoredEventCheckpoints];
                ensure!(*invalid <= total, "invalid checkpoint cursor count");
                if let Some(comparisons) =
                    observation.facts.get(&Fact::CheckpointRetentionComparisons)
                {
                    ensure!(
                        *invalid
                            + observation.facts[&Fact::CheckpointsWithoutRetentionBoundary]
                            + observation.facts[&Fact::CheckpointsWithInvalidRetentionBoundary]
                            + comparisons
                            == total,
                        "invalid checkpoint retention partition"
                    );
                    ensure!(
                        observation.facts[&Fact::CheckpointsBeforeRetention] <= *comparisons,
                        "invalid checkpoint retention subset"
                    );
                }
            }
            if observation.category == Category::DiskRetention {
                if let Some(present) = observation.facts.get(&Fact::DatabaseWalSidecarPresent) {
                    ensure!(*present <= 1, "invalid sidecar presence flag");
                    ensure!(
                        *present != 0
                            || observation.facts[&Fact::DatabaseWalSidecarBytesEstimate] == 0,
                        "absent sidecar cannot have bytes"
                    );
                }
                let (logical, non_freelist) = database_geometry(
                    observation.facts[&Fact::DatabasePages],
                    observation.facts[&Fact::PageSizeBytes],
                    observation.facts[&Fact::DatabaseFreelistPages],
                )?;
                ensure!(
                    observation.facts[&Fact::DatabaseLogicalBytes] == logical
                        && observation.facts[&Fact::DatabaseNonFreelistBytes] == non_freelist,
                    "inconsistent database byte geometry"
                );
            }
            if let Some(checked) = observation.facts.get(&Fact::EventSequenceStreamsChecked) {
                ensure!(
                    observation.facts.get(&Fact::EventStreams) == Some(checked),
                    "incomplete event sequence head coverage"
                );
                ensure!(
                    observation.facts[&Fact::EventSequenceStreamMismatches] <= *checked,
                    "invalid event sequence mismatch subset"
                );
                ensure!(
                    observation.facts[&Fact::EventSequenceRowsBeyondHead] == 0
                        || observation.facts[&Fact::EventSequenceStreamMismatches] > 0,
                    "future event rows require a mismatched stream"
                );
                ensure!(
                    *checked <= sequences::MAX_HEADS as u64
                        && observation
                            .facts
                            .get(&Fact::StoredEvents)
                            .is_some_and(|total| *total <= sequences::MAX_EVENTS as u64),
                    "complete event sequence observation exceeds scan caps"
                );
                let rows = [
                    Fact::EventSequenceRowsWithoutHead,
                    Fact::EventSequenceRowsBeforeRetention,
                    Fact::EventSequenceRowsBeyondHead,
                ]
                .iter()
                .map(|fact| observation.facts[fact])
                .sum::<u64>();
                ensure!(
                    observation
                        .facts
                        .get(&Fact::StoredEvents)
                        .is_some_and(|total| rows <= *total),
                    "invalid event sequence row subsets"
                );
            }
            if observation.category == Category::Queue {
                if let Some(due) = observation.facts.get(&Fact::DuePlannedOccurrences) {
                    ensure!(
                        observation
                            .facts
                            .get(&Fact::PlannedOccurrences)
                            .is_some_and(|planned| due <= planned),
                        "invalid doctor due queue count"
                    );
                }
                if let Some(eligible) = observation.facts.get(&Fact::EligibleOccurrences) {
                    ensure!(
                        observation
                            .facts
                            .get(&Fact::DuePlannedOccurrences)
                            .is_some_and(|due| eligible <= due),
                        "invalid doctor eligible queue count"
                    );
                }
            }
            if observation.category == Category::Store {
                let checks = observation.facts[&Fact::RequiredSchemaObjectPresenceChecks];
                ensure!(
                    checks == REQUIRED_SCHEMA_OBJECTS.len() as u64
                        && [
                            Fact::MissingRequiredSchemaObjects,
                            Fact::UnrecognizedRequiredSchemaObjectForms,
                            Fact::IncompleteRequiredSchemaObjectChecks
                        ]
                        .into_iter()
                        .map(|fact| observation.facts[&fact])
                        .sum::<u64>()
                            <= checks,
                    "invalid doctor schema presence counts"
                );
            }
            if observation.category == Category::LeasesFences {
                if let Some(&total) = observation.facts.get(&Fact::LeaseBearingOccurrences) {
                    let partition: u64 = [
                        Fact::ActiveLeaseOccurrences,
                        Fact::ExpiredLeaseOccurrences,
                        Fact::MissingLeaseExpiryOccurrences,
                        Fact::InvalidLeaseExpiryOccurrences,
                    ]
                    .iter()
                    .map(|fact| observation.facts[fact])
                    .sum();
                    ensure!(
                        partition == total
                            && observation
                                .facts
                                .get(&Fact::LeaseOutsideActiveStateOccurrences)
                                .is_none_or(|count| *count <= total)
                            && observation
                                .facts
                                .get(&Fact::MismatchedUnlaunchedSchedulerClaims)
                                .is_none_or(|count| *count <= total),
                        "invalid doctor lease partition"
                    );
                }
            }
            for fact in [
                Fact::LeaderAssigned,
                Fact::LatestPassSucceeded,
                Fact::RecordedSchedulerFencePresent,
                Fact::RecordedContractMigrationPresent,
            ] {
                ensure!(
                    observation.facts.get(&fact).is_none_or(|value| *value <= 1),
                    "invalid doctor boolean"
                );
            }
        }
        planning::validate_cohort(&self.observations)?;
        Ok(())
    }
}

fn bounded_timestamp(value: &[u8]) -> Result<DateTime<Utc>> {
    ensure!(value.len() <= 64, "invalid doctor timestamp");
    Ok(DateTime::parse_from_rfc3339(std::str::from_utf8(value)?)?.with_timezone(&Utc))
}

// Presence-only contract for explicit objects created by the Automations
// initialization path. Autoindexes and object definitions need separate semantic
// compatibility checks; zero missing objects is not an integrity/health claim.
const REQUIRED_SCHEMA_OBJECTS: &[(&str, &str)] = &[
    ("index", "automation_cancellations_active_run"),
    ("index", "idx_automation_attempts_dispatch"),
    ("index", "idx_automation_command_adoptions_automation"),
    ("index", "idx_automation_definitions_planning"),
    ("index", "idx_automation_definitions_updated_at"),
    ("index", "idx_automation_event_checkpoints_expiry"),
    ("index", "idx_automation_events_recorded"),
    ("index", "idx_automation_missions_scope"),
    ("index", "idx_automation_occurrences_history"),
    ("index", "idx_automation_occurrences_leases"),
    ("index", "idx_automation_occurrences_leases_by_automation"),
    ("index", "idx_automation_occurrences_scheduled"),
    ("index", "idx_automation_occurrences_state"),
    ("index", "idx_automation_runs_automation_started"),
    ("index", "idx_automation_runs_delivery_pending"),
    ("index", "idx_automation_runs_history"),
    ("index", "idx_automation_runtime_terminal_evidence_binding"),
    ("index", "idx_automation_runtime_terminal_evidence_run"),
    ("table", "automation_attempts"),
    ("table", "automation_cancellations"),
    ("table", "automation_command_adoptions"),
    ("table", "automation_command_reservations"),
    ("table", "automation_contract_migrations"),
    ("table", "automation_definitions"),
    ("table", "automation_event_checkpoints"),
    ("table", "automation_event_id_scratch"),
    ("table", "automation_event_migrations"),
    ("table", "automation_event_stream_heads"),
    ("table", "automation_events"),
    ("table", "automation_missions"),
    ("table", "automation_occurrences"),
    ("table", "automation_receipt_authority_extensions"),
    ("table", "automation_receipts"),
    ("table", "automation_retry_state"),
    ("table", "automation_rich_definition_revisions"),
    ("table", "automation_runs"),
    ("table", "automation_runtime_terminal_evidence"),
    ("table", "automation_scheduler_authority"),
    ("table", "automation_scheduler_last_pass"),
    ("table", "automation_scheduler_last_successful_pass"),
    ("table", "automation_scheduler_planning_cursor"),
    ("table", "automation_stop_fences"),
    ("table", "automation_timezone_migrations"),
    ("trigger", "automation_attempt_adoption_key_global_insert"),
    ("trigger", "automation_attempt_authority_delete_refused"),
    ("trigger", "automation_attempt_authority_immutable"),
    ("trigger", "automation_attempt_transition_changed"),
    ("trigger", "automation_attempt_transition_created"),
    ("trigger", "automation_attempts_delete_terminal_refused"),
    ("trigger", "automation_attempts_terminal_immutable"),
    ("trigger", "automation_command_adoption_key_global_insert"),
    ("trigger", "automation_command_adoptions_immutable_columns"),
    ("trigger", "automation_command_adoptions_immutable_rowid"),
    ("trigger", "automation_command_adoptions_no_delete"),
    (
        "trigger",
        "automation_command_adoptions_response_update_guard",
    ),
    (
        "trigger",
        "automation_command_reservation_key_global_insert",
    ),
    ("trigger", "automation_events_no_delete"),
    ("trigger", "automation_events_no_update"),
    ("trigger", "automation_missions_immutable"),
    ("trigger", "automation_missions_retained"),
    ("trigger", "automation_occurrence_transition_changed"),
    ("trigger", "automation_occurrence_transition_created"),
    (
        "trigger",
        "automation_receipt_authority_extensions_no_delete",
    ),
    (
        "trigger",
        "automation_receipt_authority_extensions_no_update",
    ),
    ("trigger", "automation_receipts_no_delete"),
    ("trigger", "automation_receipts_no_update"),
    ("trigger", "automation_run_authority_profile_immutable"),
    ("trigger", "automation_run_delivery_settled_immutable"),
    ("trigger", "automation_run_receipt_once"),
    ("trigger", "automation_run_transition_changed"),
    ("trigger", "automation_run_transition_created"),
    ("trigger", "automation_runtime_terminal_evidence_no_delete"),
    ("trigger", "automation_runtime_terminal_evidence_no_update"),
];

#[cfg(test)]
fn missing_required_schema_objects(conn: &Connection) -> Result<u64> {
    let mut query =
        conn.prepare("SELECT EXISTS(SELECT 1 FROM sqlite_schema WHERE type=?1 AND name=?2)")?;
    let mut missing = 0;
    for (kind, name) in REQUIRED_SCHEMA_OBJECTS {
        let present: bool = query.query_row(rusqlite::params![kind, name], |row| row.get(0))?;
        if !present {
            missing += 1;
        }
    }
    Ok(missing)
}

// Callers provide only fixed production table and column names. These checks
// test query prerequisites, not type/constraint correctness or integrity.
fn projection_available(conn: &Connection, table: &str, columns: &[&str]) -> Result<bool> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM main.sqlite_schema WHERE type='table' AND name=?1)",
        [table],
        |row| row.get(0),
    )?;
    if !exists {
        return Ok(false);
    }
    ensure!(columns.len() <= 16, "invalid doctor projection contract");
    // COUNT(*) has no projected application columns. Main-table existence is
    // sufficient; do not issue a PRAGMA or inspect row contents for this case.
    if columns.is_empty() {
        return Ok(true);
    }
    let placeholders = vec!["?"; columns.len()].join(",");
    let count: i64 = conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM pragma_table_xinfo(?,'main') WHERE name IN ({placeholders})"
        ),
        rusqlite::params_from_iter(std::iter::once(table).chain(columns.iter().copied())),
        |row| row.get(0),
    )?;
    Ok(count == columns.len() as i64)
}

#[derive(Debug)]
enum RecordedContractMigration {
    Unavailable,
    Absent,
    Invalid,
    Observed {
        counters: [u64; 6],
        recorded_at: u64,
    },
}

fn recorded_contract_migration(conn: &Connection) -> Result<RecordedContractMigration> {
    let available = projection_available(
        conn,
        "automation_contract_migrations",
        &[
            "profile",
            "definitions_migrated",
            "occurrences_migrated",
            "runs_migrated",
            "unverifiable_definitions",
            "unverifiable_occurrences",
            "unverifiable_runs",
            "migrated_at",
        ],
    )?;
    if !available {
        return Ok(RecordedContractMigration::Unavailable);
    }

    // This is a historical ledger observation, not proof of the current schema
    // or every migration. Never invoke the migration or read definition bodies.
    let columns = [
        "definitions_migrated",
        "occurrences_migrated",
        "runs_migrated",
        "unverifiable_definitions",
        "unverifiable_occurrences",
        "unverifiable_runs",
    ];
    let projections = columns.iter().map(|column| format!(
        "CASE WHEN typeof({column})='integer' AND {column} BETWEEN 0 AND 9007199254740991 THEN {column} END"
    )).collect::<Vec<_>>().join(", ");
    let raw = conn.query_row(
        &format!("SELECT {projections}, substr(CAST(migrated_at AS BLOB), 1, 65) FROM main.automation_contract_migrations WHERE profile=?1"),
        [super::contract::migration::PROFILE],
        |row| Ok(((0..6).map(|index| row.get::<_, Option<i64>>(index)).collect::<rusqlite::Result<Vec<_>>>()?, row.get::<_, Vec<u8>>(6)?)),
    ).optional()?;
    let Some((counters, timestamp)) = raw else {
        return Ok(RecordedContractMigration::Absent);
    };
    let Some(counters) = counters.into_iter().collect::<Option<Vec<_>>>() else {
        return Ok(RecordedContractMigration::Invalid);
    };
    let recorded_at = bounded_timestamp(&timestamp)
        .ok()
        .and_then(|value| u64::try_from(value.timestamp_millis()).ok())
        .filter(|value| *value <= MAX_SAFE);
    let Some(recorded_at) = recorded_at else {
        return Ok(RecordedContractMigration::Invalid);
    };
    Ok(RecordedContractMigration::Observed {
        counters: std::array::from_fn(|index| counters[index] as u64),
        recorded_at,
    })
}

// IDs are compared inside SQLite and transiently decoded from bounded bytes
// for canonical UTF-8/length validation. No IDs/tokens enter reports or errors;
// checkpoint tokens are never selected.
fn checkpoint_retention_counts(conn: &Connection, retention_available: bool) -> Result<[u64; 5]> {
    let matching_head = "FROM main.automation_event_stream_heads h
        WHERE h.stream_kind=c.stream_kind COLLATE BINARY AND h.stream_id=c.stream_id COLLATE BINARY";
    let head = if retention_available {
        // One output row per checkpoint even if a damaged head table lost its
        // unique key. Two matches are already ambiguous; do not enumerate them
        // or pick a winning boundary, even when their values happen to agree.
        format!("(SELECT COUNT(*) FROM (SELECT 1 {matching_head} LIMIT 2)),
         (SELECT CASE WHEN typeof(h.earliest_sequence)='integer' AND h.earliest_sequence BETWEEN 0 AND 9007199254740991 THEN h.earliest_sequence END {matching_head} LIMIT 1),
         (SELECT CASE WHEN typeof(h.next_sequence)='integer' AND h.next_sequence BETWEEN 0 AND 9007199254740991 THEN h.next_sequence END {matching_head} LIMIT 1)")
    } else {
        "0,NULL,NULL".to_owned()
    };
    let mut statement = conn.prepare(&format!(
        "SELECT CASE WHEN typeof(c.after_sequence)='integer' AND c.after_sequence BETWEEN -1 AND 9007199254740991 THEN c.after_sequence END,
         CASE WHEN typeof(c.stream_kind)='text' THEN substr(CAST(c.stream_kind AS BLOB),1,17) END,
         CASE WHEN typeof(c.stream_id)='text' AND (c.stream_kind COLLATE BINARY != 'feed' OR c.stream_id COLLATE BINARY='all') THEN 1 ELSE 0 END,
         {head}, CASE WHEN typeof(c.stream_id)='text' THEN substr(CAST(c.stream_id AS BLOB),1,1281) END
         FROM main.automation_event_checkpoints c"
    ))?;
    let mut rows = statement.query([])?;
    let mut counts = [0_u64; 5];
    while let Some(row) = rows.next()? {
        let after = row.get::<_, Option<i64>>(0)?;
        let kind = row.get::<_, Option<Vec<u8>>>(1)?;
        let kind_valid = kind
            .as_deref()
            .and_then(|bytes| std::str::from_utf8(bytes).ok())
            .is_some_and(|kind| {
                serde_json::from_value::<super::contract::types::StreamKind>(
                    serde_json::Value::String(kind.to_owned()),
                )
                .is_ok()
            });
        // EventStreamId permits at most 320 Unicode characters (up to 1280
        // UTF-8 bytes). The extra byte makes truncation fail closed.
        let identity = row.get::<_, Option<Vec<u8>>>(6)?;
        let identity_valid = row.get::<_, bool>(2)?
            && identity
                .as_deref()
                .filter(|bytes| bytes.len() <= 1280)
                .and_then(|bytes| std::str::from_utf8(bytes).ok())
                .is_some_and(|text| {
                    super::contract::types::EventStreamId::new(text.to_owned()).is_ok()
                });
        let mut before = false;
        let index = if let Some(after) = after.filter(|_| kind_valid && identity_valid) {
            let matching_heads = row.get::<_, i64>(3)?;
            if matching_heads == 0 {
                1
            } else if matching_heads != 1 {
                2
            } else if let (Some(earliest), Some(next)) =
                (row.get::<_, Option<i64>>(4)?, row.get::<_, Option<i64>>(5)?)
            {
                if earliest > next {
                    2
                } else {
                    // after is bounded well below i64::MAX, including sentinel -1.
                    before = after + 1 < earliest;
                    3
                }
            } else {
                2
            }
        } else {
            0
        };
        counts[index] = counts[index]
            .checked_add(1)
            .filter(|value| *value <= MAX_SAFE)
            .ok_or_else(|| anyhow::anyhow!("checkpoint count exceeds supported range"))?;
        if before {
            counts[4] += 1;
        }
    }
    Ok(counts)
}

// Expiry observations do not read checkpoint tokens, stream IDs or event payloads.
// They do not invoke event paging/resume, which creates and prunes checkpoints.
fn checkpoint_expiry_counts(conn: &Connection, now: DateTime<Utc>) -> Result<[u64; 3]> {
    let mut statement = conn.prepare(
        "SELECT CASE WHEN typeof(expires_at)='text' THEN substr(CAST(expires_at AS BLOB),1,65) END
         FROM main.automation_event_checkpoints",
    )?;
    let mut rows = statement.query([])?;
    let mut counts = [0_u64; 3];
    while let Some(row) = rows.next()? {
        let value = row.get::<_, Option<Vec<u8>>>(0)?;
        let expiry = value
            .as_deref()
            .and_then(|value| bounded_timestamp(value).ok());
        let index = match expiry {
            Some(expiry) if now < expiry => 0,
            Some(_) => 1,
            None => 2,
        };
        counts[index] = counts[index]
            .checked_add(1)
            .filter(|value| *value <= MAX_SAFE)
            .ok_or_else(|| anyhow::anyhow!("checkpoint count exceeds supported range"))?;
    }
    Ok(counts)
}

// Decode bounded state bytes with the contract enum, rather than duplicating
// its vocabulary in SQL. Non-text/NULL values are invalid, not empty cohorts.
fn occurrence_states_valid(conn: &Connection) -> Result<bool> {
    persisted_states_valid::<super::contract::types::OccurrenceState>(
        conn,
        "automation_occurrences",
    )
}

fn attempt_states_valid(conn: &Connection) -> Result<bool> {
    persisted_states_valid::<super::contract::types::AttemptState>(conn, "automation_attempts")
}

fn persisted_states_valid<T: serde::de::DeserializeOwned>(
    conn: &Connection,
    table: &str,
) -> Result<bool> {
    let mut statement = conn.prepare(&format!(
        "SELECT CASE WHEN typeof(state)='text' THEN substr(CAST(state AS BLOB),1,65) END FROM main.{table}"
    ))?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        let Some(bytes) = row.get::<_, Option<Vec<u8>>>(0)? else {
            return Ok(false);
        };
        if bytes.len() > 64 {
            return Ok(false);
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            return Ok(false);
        };
        if serde_json::from_value::<T>(serde_json::Value::String(text.to_owned())).is_err() {
            return Ok(false);
        }
    }
    Ok(true)
}

// Bound what the canonical definition reader materializes before calling it.
// These are diagnostic input limits, not scheduler capacity or SQL-time bounds.
const ELIGIBILITY_INPUT_ROWS: usize = 256;
const ELIGIBILITY_INPUT_BYTES: u64 = 1_048_576;

fn eligibility_inputs_within_budget(conn: &Connection) -> Result<bool> {
    // All text fields loaded by store::list_definitions, including incidental
    // fields on inactive rows, count toward the budget. Revision is a scalar.
    let columns = [
        "id",
        "name",
        "status",
        "definition_json",
        "definition_digest",
        "lifecycle_state",
        "tombstoned_at",
        "updated_at",
    ];
    // Saturate each field at the overflow sentinel before adding lengths, so
    // neither a huge field nor a combination can overflow SQLite's integer sum.
    let sizes = columns
        .iter()
        .map(|column| {
            format!(
                "min(coalesce(length(CAST({column} AS BLOB)),0),{})",
                ELIGIBILITY_INPUT_BYTES + 1
            )
        })
        .collect::<Vec<_>>()
        .join(" + ");
    let queries = [
        format!(
            "SELECT {sizes} FROM main.automation_definitions AS definition
         WHERE tombstoned_at IS NULL AND NOT EXISTS (
             SELECT 1 FROM main.automation_missions WHERE automation_id=definition.id
         ) LIMIT {}",
            ELIGIBILITY_INPUT_ROWS + 1
        ),
        format!(
            "SELECT min(coalesce(length(CAST(automation_id AS BLOB)),0),{})
            FROM main.automation_retry_state WHERE quarantined_at IS NOT NULL LIMIT {}",
            ELIGIBILITY_INPUT_BYTES + 1,
            ELIGIBILITY_INPUT_ROWS + 1
        ),
    ];
    let mut remaining = ELIGIBILITY_INPUT_BYTES;
    for query in queries {
        let mut statement = conn.prepare(&query)?;
        let mut rows = statement.query([])?;
        let mut count = 0;
        while let Some(row) = rows.next()? {
            count += 1;
            if count > ELIGIBILITY_INPUT_ROWS {
                return Ok(false);
            }
            let bytes = u64::try_from(row.get::<_, i64>(0)?)?;
            let Some(next) = remaining.checked_sub(bytes) else {
                return Ok(false);
            };
            remaining = next;
        }
    }
    Ok(true)
}

// Snapshot page geometry is not physical allocation or filesystem capacity.
fn database_geometry(pages: u64, page_size: u64, freelist: u64) -> Result<(u64, u64)> {
    ensure!(
        (512..=65_536).contains(&page_size) && page_size.is_power_of_two(),
        "invalid database page size"
    );
    // Page one contains the database header and cannot be on the freelist.
    ensure!(
        freelist <= pages && (pages == 0 || freelist < pages),
        "invalid database freelist geometry"
    );
    let logical = pages
        .checked_mul(page_size)
        .filter(|bytes| *bytes <= MAX_SAFE)
        .ok_or_else(|| anyhow::anyhow!("database byte geometry exceeds supported range"))?;
    Ok((logical, (pages - freelist) * page_size))
}

/// A bounded-output read transaction. These aggregates describe persisted rows,
/// not complete operational health; the explicit gaps remain acceptance work.
/// No initialization, migration, integrity scan, event paging or repair is invoked.
pub fn inspect_owner_store(conn: &Connection, now: DateTime<Utc>) -> Result<Report> {
    read::inspect(conn, now)
}

/// Observe an existing store without invoking writable initialization helpers.
pub(crate) fn inspect_owner_store_path(
    path: &std::path::Path,
    now: DateTime<Utc>,
) -> Result<Report> {
    read::inspect_path(path, now)
}

pub(super) fn inspect(conn: &Connection, now: DateTime<Utc>) -> Result<Report> {
    let transaction = conn.unchecked_transaction()?;
    let count = |sql: &str| -> Result<u64> {
        let value: i64 = transaction.query_row(sql, [], |row| row.get(0))?;
        let value = u64::try_from(value)?;
        ensure!(value <= MAX_SAFE, "doctor count exceeds supported range");
        Ok(value)
    };
    // This is the scheduler's persisted schedule cutoff, not eligibility:
    // older latest-only history, inactive definitions and blocked retries can
    // all remain due. Keep that distinction visible in the queue gap.
    let state_available = projection_available(&transaction, "automation_occurrences", &["state"])?;
    let invalid_occurrence_state = state_available && !occurrence_states_valid(&transaction)?;
    let due_available = projection_available(
        &transaction,
        "automation_occurrences",
        &["state", "scheduled_for"],
    )?;
    let due_planned = if due_available && !invalid_occurrence_state {
        let value: i64 = transaction.query_row(
            "SELECT COUNT(*) FROM main.automation_occurrences WHERE state = 'planned' AND scheduled_for <= ?1",
            [now.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)], |row| row.get(0),
        )?;
        let value = u64::try_from(value)?;
        ensure!(value <= MAX_SAFE, "doctor count exceeds supported range");
        Some(value)
    } else {
        None
    };
    // An absent row is not a zero-duration pass or a failed pass. Read no raw
    // error text, and reject states outside the closed persisted vocabulary.
    let status_available = projection_available(
        &transaction,
        "automation_scheduler_last_pass",
        &["id", "status"],
    )?;
    let pass_status: Option<Option<i64>> = if status_available {
        transaction
            .query_row(
                "SELECT CASE WHEN typeof(status)='text' THEN CASE status COLLATE BINARY WHEN 'succeeded' THEN 1 WHEN 'failed' THEN 0 END END
         FROM main.automation_scheduler_last_pass WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()?
    } else {
        None
    };
    let duration_available = projection_available(
        &transaction,
        "automation_scheduler_last_pass",
        &["id", "duration_ms"],
    )?;
    let pass_duration: Option<Option<i64>> = if duration_available {
        transaction.query_row(
            "SELECT CASE WHEN typeof(duration_ms)='integer' AND duration_ms BETWEEN 0 AND 9007199254740991 THEN duration_ms END
             FROM main.automation_scheduler_last_pass WHERE id=1",
            [], |row| row.get(0),
        ).optional()?
    } else {
        None
    };
    let invalid_pass_status = pass_status.is_some_and(|status| status.is_none());
    let invalid_pass_duration = pass_duration.is_some_and(|duration| duration.is_none());
    let lag_available = projection_available(
        &transaction,
        "automation_scheduler_last_pass",
        &["id", "scheduled_at", "started_at"],
    )?;
    type PassLagCells = (Option<Vec<u8>>, Option<Vec<u8>>);
    let lag: Option<PassLagCells> = if lag_available {
        transaction
            .query_row(
                "SELECT CASE WHEN typeof(scheduled_at)='text' THEN substr(CAST(scheduled_at AS BLOB), 1, 65) END,
                    CASE WHEN typeof(started_at)='text' THEN substr(CAST(started_at AS BLOB), 1, 65) END
             FROM main.automation_scheduler_last_pass WHERE id=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
    } else {
        None
    };
    let lag_result = lag
        .map(|(scheduled, started)| -> Result<u64> {
            let scheduled = bounded_timestamp(
                scheduled
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("invalid scheduler timestamp type"))?,
            )?;
            let started = bounded_timestamp(
                started
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("invalid scheduler timestamp type"))?,
            )?;
            ensure!(started >= scheduled, "invalid scheduler clock order");
            let millis =
                u64::try_from(started.signed_duration_since(scheduled).num_milliseconds())?;
            ensure!(millis <= MAX_SAFE, "scheduler lag exceeds supported range");
            Ok(millis)
        })
        .transpose();
    let invalid_lag = lag_result.is_err();
    let start_lag = lag_result.ok().flatten();
    let success_store_available = projection_available(
        &transaction,
        "automation_scheduler_last_successful_pass",
        &["id", "finished_at"],
    )?;
    let last_success: Option<Option<Vec<u8>>> = if success_store_available {
        transaction
            .query_row(
                "SELECT CASE WHEN typeof(finished_at)='text' THEN substr(CAST(finished_at AS BLOB), 1, 65) END
             FROM main.automation_scheduler_last_successful_pass WHERE id=1",
                [],
                |row| row.get(0),
            )
            .optional()?
    } else {
        None
    };
    let last_success_result = last_success
        .map(|value| -> Result<u64> {
            let value = value
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("invalid successful pass timestamp type"))?;
            let millis = u64::try_from(bounded_timestamp(value)?.timestamp_millis())?;
            ensure!(
                millis <= MAX_SAFE,
                "successful pass timestamp exceeds supported range"
            );
            Ok(millis)
        })
        .transpose();
    // Invalid history must not conceal independent queue/store observations.
    // Retain only a closed finding code, never the raw timestamp or parse error.
    let invalid_success = invalid_pass_status
        || last_success_result.is_err()
        || (status_available
            && pass_status.is_none()
            && last_success_result
                .as_ref()
                .is_ok_and(|value| value.is_some()));
    let last_success = if invalid_success {
        None
    } else {
        last_success_result?
    };
    let attempt_state_available =
        projection_available(&transaction, "automation_attempts", &["state"])?;
    let invalid_attempt_state = attempt_state_available && !attempt_states_valid(&transaction)?;
    let failed_attempts = if attempt_state_available && !invalid_attempt_state {
        Some(count(
            "SELECT COUNT(*) FROM main.automation_attempts WHERE state COLLATE BINARY = 'failed'",
        )?)
    } else {
        None
    };
    // MAX alone could conceal a malformed smaller row. Validate the whole
    // retained counter set, without selecting private reason/class text.
    let retry_history_available = projection_available(
        &transaction,
        "automation_retry_state",
        &["consecutive_exhaustions"],
    )?;
    let quarantine =
        if projection_available(&transaction, "automation_retry_state", &["quarantined_at"])? {
            Some(count(
                "SELECT COUNT(*) FROM main.automation_retry_state WHERE quarantined_at IS NOT NULL",
            )?)
        } else {
            None
        };
    let invalid_retry_counters = retry_history_available && count("SELECT COUNT(*) FROM main.automation_retry_state WHERE typeof(consecutive_exhaustions) != 'integer' OR consecutive_exhaustions < 0 OR consecutive_exhaustions > 9007199254740991")? != 0;
    let mut eligibility_available = true;
    for &(table, columns) in super::occurrences::ELIGIBILITY_QUERY_PREREQUISITES {
        if !projection_available(&transaction, table, columns)? {
            eligibility_available = false;
            break;
        }
    }
    let eligibility_budget_exceeded = eligibility_available
        && !invalid_occurrence_state
        && !eligibility_inputs_within_budget(&transaction)?;
    let eligible =
        if eligibility_available && !invalid_occurrence_state && !eligibility_budget_exceeded {
            match super::occurrences::eligible_occurrence_count(&transaction, now) {
                Ok(count) => Some(count),
                Err(error)
                    if error.is::<super::occurrences::InvalidEligibleTimestamp>()
                        || error.is::<super::occurrences::InvalidEligibleDefinition>() =>
                {
                    None
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
    let invalid_queue = eligibility_available
        && !invalid_occurrence_state
        && !eligibility_budget_exceeded
        && eligible.is_none();
    let lease_cohort_available = projection_available(
        &transaction,
        "automation_occurrences",
        &["lease_owner", "lease_expires_at"],
    )?;
    let leases = if lease_cohort_available {
        Some(
            super::leases::summarize_expiry(&transaction, now)
                .map_err(|_| anyhow::anyhow!("cannot inspect lease metadata"))?,
        )
    } else {
        None
    };
    let outside_active_state = if lease_cohort_available && state_available {
        Some(
            super::leases::outside_active_state_count(&transaction)
                .map_err(|_| anyhow::anyhow!("cannot inspect lease state metadata"))?,
        )
    } else {
        None
    };
    use super::occurrences::RecordedSchedulerAuthority;
    let recorded_fence = if projection_available(
        &transaction,
        "automation_scheduler_authority",
        &["id", "generation", "owner_id"],
    )? {
        Some(super::occurrences::recorded_scheduler_authority(
            &transaction,
        )?)
    } else {
        None
    };
    let unavailable_authority = recorded_fence.is_none();
    let invalid_authority = matches!(
        &recorded_fence,
        Some(RecordedSchedulerAuthority::InvalidMetadata)
    );
    let mut unavailable_claims = false;
    let mismatched_claims =
        if let Some(RecordedSchedulerAuthority::Observed(authority)) = &recorded_fence {
            if projection_available(
                &transaction,
                "automation_occurrences",
                &["id", "state", "lease_owner", "scheduler_generation"],
            )? && projection_available(&transaction, "automation_runs", &["occurrence_id"])?
            {
                Some(authority.mismatched_claim_count(&transaction)?)
            } else {
                unavailable_claims = true;
                None
            }
        } else {
            None
        };
    let migration = recorded_contract_migration(&transaction)?;
    let invalid_migration = matches!(migration, RecordedContractMigration::Invalid);
    let unavailable_migration = matches!(migration, RecordedContractMigration::Unavailable);
    let receipts = if projection_available(&transaction, "automation_receipts", &[])? {
        Some(count("SELECT COUNT(*) FROM main.automation_receipts")?)
    } else {
        None
    };
    let delivery_state_available =
        projection_available(&transaction, "automation_runs", &["delivery_state"])?;
    // Validate without materializing persisted state text. NULL is the run-store's
    // valid no-delivery value; binary comparison ignores damaged column collation.
    let invalid_delivery_state = delivery_state_available
        && transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.automation_runs
         WHERE delivery_state IS NOT NULL AND (typeof(delivery_state) != 'text'
            OR delivery_state COLLATE BINARY NOT IN ('pending','delivered','failed')))",
            [],
            |row| row.get::<_, bool>(0),
        )?;
    let mut receipt_links_available = true;
    for &(table, columns) in receipt_links::PREREQUISITES {
        if !projection_available(&transaction, table, columns)? {
            receipt_links_available = false;
            break;
        }
    }
    let receipt_link_observation = if receipt_links_available {
        Some(receipt_links::inspect(&transaction)?)
    } else {
        None
    };
    let mut receipt_commitments_available = receipts.is_some();
    for &(table, columns) in receipt_commitments::PREREQUISITES {
        if !projection_available(&transaction, table, columns)? {
            receipt_commitments_available = false;
            break;
        }
    }
    let receipt_commitment_observation = if receipt_commitments_available {
        Some(receipt_commitments::inspect(&transaction)?)
    } else {
        None
    };
    let mut delivery_facts = Vec::with_capacity(5);
    if delivery_state_available && !invalid_delivery_state {
        delivery_facts.push((
            Fact::FailedDeliveries,
            count("SELECT COUNT(*) FROM main.automation_runs WHERE delivery_state='failed'")?,
        ));
        delivery_facts.push((
            Fact::PendingDeliveries,
            count("SELECT COUNT(*) FROM main.automation_runs WHERE delivery_state='pending'")?,
        ));
    }
    if let Some(receipts) = receipts {
        delivery_facts.push((Fact::StoredReceipts, receipts));
    }
    let failure_class_available =
        projection_available(&transaction, "automation_attempts", &["failure_class"])?;
    let mut refusal_facts = Vec::with_capacity(2);
    let invalid_failure_class = failure_class_available
        && transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM main.automation_attempts
         WHERE failure_class IS NOT NULL AND (typeof(failure_class) != 'text'
         OR failure_class COLLATE BINARY NOT IN ('transient_dispatch','lease_expired',
         'runtime_unavailable','launch_refused','runtime_error','timeout','cancelled',
         'ambiguous_evidence','runtime_authority_unsupported')))",
            [],
            |row| row.get::<_, bool>(0),
        )?;
    if failure_class_available && !invalid_failure_class {
        refusal_facts.push((Fact::LaunchRefusedAttempts, count("SELECT COUNT(*) FROM main.automation_attempts WHERE failure_class COLLATE BINARY = 'launch_refused'")?));
        refusal_facts.push((Fact::UnsupportedAuthorityAttempts, count("SELECT COUNT(*) FROM main.automation_attempts WHERE failure_class COLLATE BINARY = 'runtime_authority_unsupported'")?));
    }
    let mut event_facts = Vec::with_capacity(2);
    for (fact, table, sql) in [
        (
            Fact::EventStreams,
            "automation_event_stream_heads",
            "SELECT COUNT(*) FROM main.automation_event_stream_heads",
        ),
        (
            Fact::StoredEvents,
            "automation_events",
            "SELECT COUNT(*) FROM main.automation_events",
        ),
    ] {
        if projection_available(&transaction, table, &[])? {
            event_facts.push((fact, count(sql)?));
        }
    }
    let unavailable_events = event_facts.len() != 2;
    let checkpoint_total =
        if projection_available(&transaction, "automation_event_checkpoints", &[])? {
            Some(count(
                "SELECT COUNT(*) FROM main.automation_event_checkpoints",
            )?)
        } else {
            None
        };
    let checkpoint_expiry_available = checkpoint_total.is_some()
        && projection_available(
            &transaction,
            "automation_event_checkpoints",
            &["expires_at"],
        )?;
    if let Some(total) = checkpoint_total {
        event_facts.push((Fact::StoredEventCheckpoints, total));
    }
    if checkpoint_expiry_available {
        let partitions = checkpoint_expiry_counts(&transaction, now)?;
        event_facts.extend(
            [
                Fact::UnexpiredEventCheckpoints,
                Fact::ExpiredEventCheckpoints,
                Fact::InvalidEventCheckpointExpiries,
            ]
            .into_iter()
            .zip(partitions),
        );
    }
    match &receipt_commitment_observation {
        Some(receipt_commitments::Observation::Verified(checked)) => delivery_facts.extend([
            (Fact::ReceiptCommitmentsChecked, *checked),
            (Fact::InvalidReceiptCommitments, 0),
        ]),
        Some(receipt_commitments::Observation::Inconsistent(invalid)) => {
            if let Some(total) = receipts {
                delivery_facts.extend([
                    (Fact::ReceiptCommitmentsChecked, total),
                    (Fact::InvalidReceiptCommitments, *invalid),
                ]);
            }
        }
        _ => {}
    }
    let checkpoint_cursor_available = checkpoint_total.is_some()
        && projection_available(
            &transaction,
            "automation_event_checkpoints",
            &["stream_kind", "stream_id", "after_sequence"],
        )?;
    let checkpoint_retention_available = checkpoint_cursor_available
        && projection_available(
            &transaction,
            "automation_event_stream_heads",
            &[
                "stream_kind",
                "stream_id",
                "earliest_sequence",
                "next_sequence",
            ],
        )?;
    if checkpoint_cursor_available {
        let counts = checkpoint_retention_counts(&transaction, checkpoint_retention_available)?;
        event_facts.push((Fact::InvalidCheckpointCursorMetadata, counts[0]));
        if checkpoint_retention_available {
            event_facts.extend(
                [
                    Fact::CheckpointsWithoutRetentionBoundary,
                    Fact::CheckpointsWithInvalidRetentionBoundary,
                    Fact::CheckpointRetentionComparisons,
                    Fact::CheckpointsBeforeRetention,
                ]
                .into_iter()
                .zip(counts.into_iter().skip(1)),
            );
        }
    }
    let mut sequence_available = true;
    for &(table, columns) in sequences::PREREQUISITES {
        if !projection_available(&transaction, table, columns)? {
            sequence_available = false;
            break;
        }
    }
    let sequence_observation = if sequence_available {
        Some(sequences::inspect(&transaction)?)
    } else {
        None
    };
    if let Some(sequences::Observation::Complete(stats)) = sequence_observation {
        event_facts.extend([
            (Fact::EventSequenceStreamsChecked, stats.streams),
            (Fact::EventSequenceStreamMismatches, stats.mismatches),
            (Fact::EventSequenceRowsWithoutHead, stats.orphans),
            (
                Fact::EventSequenceRowsBeforeRetention,
                stats.before_retention,
            ),
            (Fact::EventSequenceRowsBeyondHead, stats.beyond_head),
        ]);
    }
    let capacity_observation = capacity::inspect(&transaction);
    let wal_observation = wal::inspect(&transaction);
    let mut observations = Vec::with_capacity(11);
    let mut add = |category, facts: Vec<(Fact, u64)>, gap, next_step| {
        observations.push(Observation {
            category,
            facts: if (category == Category::LastSuccessfulPass && invalid_pass_status)
                || (category == Category::PlanningStartLag && invalid_pass_duration)
            {
                BTreeMap::new()
            } else {
                facts.into_iter().collect()
            },
            gaps: if category == Category::LeasesFences {
                let mut gaps = Vec::new();
                if leases.is_none() {
                    gaps.push(Gap::LeaseStoreUnavailable);
                } else if outside_active_state.is_none() {
                    gaps.push(Gap::LeaseStateUnavailable);
                }
                if unavailable_authority {
                    gaps.push(Gap::RecordedAuthorityUnavailable);
                } else if invalid_authority {
                    gaps.push(Gap::InvalidPersistedMetadata);
                } else if unavailable_claims {
                    gaps.push(Gap::RecordedClaimStoreUnavailable);
                }
                if gaps.is_empty() {
                    gaps.push(gap);
                }
                gaps
            } else if category == Category::Leadership && unavailable_authority {
                vec![Gap::RecordedAuthorityUnavailable]
            } else if category == Category::LastSuccessfulPass && !status_available {
                let mut gaps = vec![Gap::LatestPassStatusUnavailable];
                if !success_store_available {
                    gaps.push(Gap::SuccessfulPassStoreUnavailable);
                } else if invalid_success {
                    gaps.push(Gap::InvalidPersistedMetadata);
                }
                gaps
            } else if category == Category::LastSuccessfulPass && invalid_pass_status {
                vec![Gap::InvalidLatestPassStatus]
            } else if category == Category::LastSuccessfulPass && !success_store_available {
                vec![Gap::SuccessfulPassStoreUnavailable]
            } else if category == Category::Queue && invalid_occurrence_state {
                vec![Gap::InvalidOccurrenceState]
            } else if category == Category::Queue && !state_available {
                vec![
                    Gap::QueueStateUnavailable,
                    Gap::DueQueueUnavailable,
                    Gap::EligibilityStoreUnavailable,
                ]
            } else if category == Category::Queue && !due_available {
                vec![Gap::DueQueueUnavailable, Gap::EligibilityStoreUnavailable]
            } else if category == Category::Queue && !eligibility_available {
                vec![Gap::EligibilityStoreUnavailable]
            } else if category == Category::Queue && eligibility_budget_exceeded {
                vec![Gap::EligibilityReadBudgetExceeded]
            } else if category == Category::RepeatedFailures {
                let mut gaps = Vec::new();
                if !attempt_state_available {
                    gaps.push(Gap::AttemptStateUnavailable);
                } else if invalid_attempt_state {
                    gaps.push(Gap::InvalidAttemptState);
                }
                if quarantine.is_none() {
                    gaps.push(Gap::QuarantineStoreUnavailable);
                }
                if !retry_history_available {
                    gaps.push(Gap::RetryExhaustionHistoryUnavailable);
                } else if invalid_retry_counters {
                    gaps.push(Gap::InvalidPersistedMetadata);
                }
                if gaps.is_empty() {
                    gaps.push(gap);
                }
                gaps
            } else if category == Category::AuthorityRefusals && !failure_class_available {
                vec![Gap::AttemptFailureClassUnavailable]
            } else if category == Category::AuthorityRefusals && invalid_failure_class {
                vec![Gap::InvalidAttemptFailureClass]
            } else if category == Category::DeliveryReceipts {
                let mut gaps = Vec::new();
                if !delivery_state_available || invalid_delivery_state {
                    gaps.push(if invalid_delivery_state {
                        Gap::InvalidDeliveryState
                    } else {
                        Gap::DeliveryStateUnavailable
                    });
                }
                if receipts.is_none() {
                    gaps.push(Gap::ReceiptStoreUnavailable);
                }
                match receipt_link_observation {
                    None => gaps.push(Gap::ReceiptLinkStoreUnavailable),
                    Some(receipt_links::Observation::InvalidMetadata) => {
                        gaps.push(Gap::ReceiptLinkMetadataInvalid)
                    }
                    Some(receipt_links::Observation::BudgetExceeded) => {
                        gaps.push(Gap::ReceiptLinkReadBudgetExceeded)
                    }
                    Some(receipt_links::Observation::Inconsistent) => {
                        gaps.push(Gap::ReceiptLinksInconsistent)
                    }
                    Some(receipt_links::Observation::Consistent) => {}
                }
                if receipts.is_some() {
                    match &receipt_commitment_observation {
                        None | Some(receipt_commitments::Observation::Unavailable) => {
                            gaps.push(Gap::ReceiptCommitmentsUnavailable)
                        }
                        Some(receipt_commitments::Observation::InvalidMetadata) => {
                            gaps.push(Gap::ReceiptCommitmentsMetadataInvalid)
                        }
                        Some(receipt_commitments::Observation::BudgetExceeded) => {
                            gaps.push(Gap::ReceiptCommitmentsReadBudgetExceeded)
                        }
                        Some(receipt_commitments::Observation::Inconsistent(_)) => {
                            gaps.push(Gap::ReceiptCommitmentsInconsistent)
                        }
                        Some(receipt_commitments::Observation::Verified(_)) => {}
                    }
                    gaps.push(Gap::ReceiptAuthorityNotVerified);
                }
                if gaps.is_empty() {
                    gaps.push(gap);
                }
                gaps
            } else if category == Category::DiskRetention {
                let mut gaps = match capacity_observation {
                    capacity::Observation::Observed { .. } => Vec::new(),
                    capacity::Observation::Unavailable => vec![Gap::FilesystemCapacityUnavailable],
                    capacity::Observation::InvalidMetadata => {
                        vec![Gap::FilesystemCapacityMetadataInvalid]
                    }
                };
                match wal_observation {
                    wal::Observation::Observed { .. } => {}
                    wal::Observation::Unavailable => gaps.push(Gap::WalSidecarUnavailable),
                    wal::Observation::InvalidMetadata => gaps.push(Gap::WalSidecarMetadataInvalid),
                }
                gaps.push(Gap::RetentionPressureNotObserved);
                gaps
            } else if category == Category::EventChangefeed {
                let mut gaps = Vec::new();
                if unavailable_events {
                    gaps.push(Gap::EventStoreUnavailable);
                }
                if checkpoint_total.is_none() {
                    gaps.push(Gap::CheckpointStoreUnavailable);
                } else {
                    if !checkpoint_expiry_available {
                        gaps.push(Gap::CheckpointExpiryUnavailable);
                    }
                    if !checkpoint_cursor_available {
                        gaps.push(Gap::CheckpointCursorMetadataUnavailable);
                    } else if !checkpoint_retention_available {
                        gaps.push(Gap::CheckpointRetentionStoreUnavailable);
                    }
                }
                match sequence_observation {
                    None => gaps.push(Gap::EventSequenceStoreUnavailable),
                    Some(sequences::Observation::InvalidMetadata) => {
                        gaps.push(Gap::EventSequenceMetadataInvalid)
                    }
                    Some(sequences::Observation::BudgetExceeded) => {
                        gaps.push(Gap::EventSequenceReadBudgetExceeded)
                    }
                    Some(sequences::Observation::Complete(_)) => {}
                }
                if gaps.is_empty() {
                    gaps.push(gap);
                }
                gaps
            } else if category == Category::Store && unavailable_migration {
                vec![Gap::ContractMigrationStoreUnavailable]
            } else if category == Category::PlanningStartLag && !lag_available {
                let mut gaps = Vec::new();
                if !duration_available {
                    gaps.push(Gap::LatestPassDurationUnavailable);
                } else if invalid_pass_duration {
                    gaps.push(Gap::InvalidLatestPassDuration);
                }
                gaps.push(Gap::LatestPassLagUnavailable);
                gaps
            } else if category == Category::PlanningStartLag && !duration_available {
                let mut gaps = vec![Gap::LatestPassDurationUnavailable];
                if invalid_lag {
                    gaps.push(Gap::InvalidPersistedMetadata);
                }
                gaps
            } else if category == Category::PlanningStartLag && invalid_pass_duration {
                vec![Gap::InvalidLatestPassDuration]
            } else if (category == Category::LastSuccessfulPass && invalid_success)
                || (category == Category::PlanningStartLag && invalid_lag)
                || (category == Category::Queue && invalid_queue)
                || (category == Category::Store && invalid_migration)
                || (matches!(category, Category::Leadership | Category::LeasesFences)
                    && invalid_authority)
            {
                vec![Gap::InvalidPersistedMetadata]
            } else if category == Category::Leadership
                && matches!(
                    &recorded_fence,
                    Some(RecordedSchedulerAuthority::Unobserved { generation: None })
                )
            {
                vec![Gap::RecordedAuthorityNotObserved]
            } else if category == Category::LastSuccessfulPass && last_success.is_some() {
                vec![]
            } else {
                vec![gap]
            },
            next_step,
        });
    };
    let leadership = match &recorded_fence {
        Some(RecordedSchedulerAuthority::Observed(authority)) => vec![
            (Fact::LeaderAssigned, 1),
            (Fact::SchedulerGeneration, authority.generation()),
        ],
        Some(RecordedSchedulerAuthority::Unobserved {
            generation: Some(generation),
        }) => vec![
            (Fact::LeaderAssigned, 0),
            (Fact::SchedulerGeneration, *generation),
        ],
        _ => vec![],
    };
    add(
        Category::Leadership,
        leadership,
        Gap::ProcessLivenessNotProven,
        NextStep::InspectSchedulerStatus,
    );
    add(
        Category::LastSuccessfulPass,
        {
            let mut facts = pass_status
                .and_then(|succeeded| {
                    succeeded.map(|value| vec![(Fact::LatestPassSucceeded, value as u64)])
                })
                .unwrap_or_default();
            if let Some(millis) = last_success {
                facts.push((Fact::LastSuccessfulPassFinishedAtMillis, millis));
            }
            facts
        },
        Gap::PriorSuccessfulPassNotRecorded,
        NextStep::InspectSchedulerStatus,
    );
    let mut store_facts = vec![(
        Fact::PresentAutomationTables,
        count(
            "SELECT COUNT(*) FROM sqlite_schema WHERE type = 'table' AND name GLOB 'automation_*'",
        )?,
    )];
    let schema = schema::inspect(&transaction, REQUIRED_SCHEMA_OBJECTS)?;
    store_facts.extend([
        (
            Fact::RequiredSchemaObjectPresenceChecks,
            REQUIRED_SCHEMA_OBJECTS.len() as u64,
        ),
        (Fact::MissingRequiredSchemaObjects, schema.missing),
        (
            Fact::UnrecognizedRequiredSchemaObjectForms,
            schema.unrecognized,
        ),
        (
            Fact::IncompleteRequiredSchemaObjectChecks,
            schema.incomplete,
        ),
    ]);
    store_facts.push((Fact::UnexpectedAutomationSchemaObjects, schema.unexpected));
    match migration {
        RecordedContractMigration::Absent => {
            store_facts.push((Fact::RecordedContractMigrationPresent, 0))
        }
        RecordedContractMigration::Unavailable | RecordedContractMigration::Invalid => {}
        RecordedContractMigration::Observed {
            counters,
            recorded_at,
        } => {
            store_facts.push((Fact::RecordedContractMigrationPresent, 1));
            store_facts.push((Fact::ContractMigrationRecordedAtMillis, recorded_at));
            for (fact, value) in [
                Fact::ContractMigrationDefinitionsMigrated,
                Fact::ContractMigrationOccurrencesMigrated,
                Fact::ContractMigrationRunsMigrated,
                Fact::ContractMigrationUnverifiableDefinitions,
                Fact::ContractMigrationUnverifiableOccurrences,
                Fact::ContractMigrationUnverifiableRuns,
            ]
            .into_iter()
            .zip(counters)
            {
                store_facts.push((fact, value));
            }
        }
    }
    add(
        Category::Store,
        store_facts,
        Gap::IntegrityAndMigrationsNotVerified,
        NextStep::InspectDaemonStatus,
    );
    add(
        Category::Queue,
        {
            let mut facts = if state_available && !invalid_occurrence_state {
                vec![
            (
                Fact::PlannedOccurrences,
                count("SELECT COUNT(*) FROM main.automation_occurrences WHERE state = 'planned'")?,
            ),
            (
                Fact::ClaimedOccurrences,
                count("SELECT COUNT(*) FROM main.automation_occurrences WHERE state = 'claimed'")?,
            ),
            (
                Fact::RunningOccurrences,
                count("SELECT COUNT(*) FROM main.automation_occurrences WHERE state = 'running'")?,
            ),
            (
                Fact::RecoveryRequiredOccurrences,
                count(
                    "SELECT COUNT(*) FROM main.automation_occurrences WHERE state = 'recovery_required'",
                )?,
            ),
            ]
            } else {
                Vec::new()
            };
            if let Some(count) = due_planned {
                facts.push((Fact::DuePlannedOccurrences, count));
            }
            if let Some(count) = eligible {
                facts.push((Fact::EligibleOccurrences, count));
            }
            facts
        },
        Gap::RuntimeAdmissionNotProven,
        NextStep::InspectSchedulerStatus,
    );
    add(
        Category::LeasesFences,
        {
            let mut facts = Vec::new();
            if let Some(leases) = &leases {
                facts.extend([
                    (Fact::LeaseBearingOccurrences, leases.total),
                    (Fact::ActiveLeaseOccurrences, leases.active),
                    (Fact::ExpiredLeaseOccurrences, leases.expired),
                    (Fact::MissingLeaseExpiryOccurrences, leases.missing_expiry),
                    (Fact::InvalidLeaseExpiryOccurrences, leases.invalid_expiry),
                ]);
            }
            if let Some(count) = outside_active_state {
                facts.push((Fact::LeaseOutsideActiveStateOccurrences, count));
            }
            match &recorded_fence {
                Some(RecordedSchedulerAuthority::Observed(_)) => {
                    facts.push((Fact::RecordedSchedulerFencePresent, 1));
                    if let Some(count) = mismatched_claims {
                        facts.push((Fact::MismatchedUnlaunchedSchedulerClaims, count));
                    }
                }
                Some(RecordedSchedulerAuthority::Unobserved { .. }) => {
                    facts.push((Fact::RecordedSchedulerFencePresent, 0));
                }
                Some(RecordedSchedulerAuthority::InvalidMetadata) | None => {}
            }
            facts
        },
        Gap::FenceConsistencyNotMeasured,
        NextStep::InspectLeaseMetadata,
    );
    add(
        Category::PlanningStartLag,
        {
            let mut facts = pass_duration
                .and_then(|duration| {
                    duration.map(|value| vec![(Fact::LatestPassDurationMs, value as u64)])
                })
                .unwrap_or_default();
            if let Some(millis) = start_lag {
                facts.push((Fact::LatestPassStartLagMs, millis));
            }
            facts
        },
        Gap::LagHistoryNotMeasured,
        NextStep::InspectSchedulerStatus,
    );
    add(
        Category::RepeatedFailures,
        {
            let mut facts = Vec::new();
            if let Some(count) = failed_attempts {
                facts.push((Fact::FailedAttempts, count));
            }
            if let Some(count) = quarantine {
                facts.push((Fact::QuarantinedRoutines, count));
            }
            if retry_history_available && !invalid_retry_counters {
                facts.extend([
            (
                Fact::RetryExhaustionHistories,
                count(
                    "SELECT COUNT(*) FROM main.automation_retry_state WHERE consecutive_exhaustions > 0",
                )?,
            ),
            (
                Fact::MaximumConsecutiveRetryExhaustions,
                count(
                    "SELECT COALESCE(MAX(consecutive_exhaustions), 0) FROM main.automation_retry_state",
                )?,
            ),
            ]);
            }
            facts
        },
        Gap::FailureHistoryNotSummarized,
        NextStep::InspectRunAndAttempts,
    );
    add(
        Category::AuthorityRefusals,
        refusal_facts,
        Gap::IdentityAndApprovalRefusalsNotRecorded,
        NextStep::ReviewAuthorityGate,
    );
    add(
        Category::DeliveryReceipts,
        delivery_facts,
        Gap::ReceiptIntegrityNotVerified,
        NextStep::InspectRunAndAttempts,
    );
    add(
        Category::EventChangefeed,
        event_facts,
        Gap::SubscriberReplayNotObserved,
        NextStep::ReviewChangefeedEvidence,
    );
    let pages = count("PRAGMA main.page_count")?;
    let page_size = count("PRAGMA main.page_size")?;
    let freelist = count("PRAGMA main.freelist_count")?;
    let (logical, non_freelist) = database_geometry(pages, page_size, freelist)?;
    let mut storage_facts = vec![
        (Fact::DatabasePages, pages),
        (Fact::PageSizeBytes, page_size),
        (Fact::DatabaseFreelistPages, freelist),
        (Fact::DatabaseLogicalBytes, logical),
        (Fact::DatabaseNonFreelistBytes, non_freelist),
    ];
    if let capacity::Observation::Observed { bytes, sampled_at } = capacity_observation {
        storage_facts.extend([
            (Fact::FilesystemAvailableBytesEstimate, bytes),
            (Fact::FilesystemCapacitySampledAtMillis, sampled_at),
        ]);
    }
    if let wal::Observation::Observed {
        present,
        bytes,
        sampled_at,
    } = wal_observation
    {
        storage_facts.extend([
            (Fact::DatabaseWalSidecarPresent, u64::from(present)),
            (Fact::DatabaseWalSidecarBytesEstimate, bytes),
            (Fact::DatabaseWalSidecarSampledAtMillis, sampled_at),
        ]);
    }
    add(
        Category::DiskRetention,
        storage_facts,
        Gap::RetentionPressureNotObserved,
        NextStep::ReviewStorageMaintenance,
    );
    let planning_measurement = planning::inspect(&transaction)?;
    planning::append(
        observations
            .iter_mut()
            .find(|value| value.category == Category::PlanningStartLag)
            .expect("planning category is constructed above"),
        planning_measurement,
    );
    let failure_history_measurement = failure_history::inspect(&transaction)?;
    failure_history::append(
        observations
            .iter_mut()
            .find(|value| value.category == Category::RepeatedFailures)
            .expect("failure category is constructed above"),
        failure_history_measurement,
    );
    let stop_fence_measurement = stop_fences::inspect(&transaction, now)?;
    stop_fences::append(
        observations
            .iter_mut()
            .find(|value| value.category == Category::LeasesFences)
            .expect("lease category is constructed above"),
        stop_fence_measurement,
    );
    transaction.commit()?;
    let report = Report {
        schema_version: SCHEMA.to_owned(),
        observed_at: now.to_rfc3339(),
        observations,
    };
    report.validate()?;
    Ok(report)
}

#[cfg(test)]
#[path = "doctor_tests.rs"]
mod tests;
