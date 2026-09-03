//! Serde projections of every object published in Coven Automations v1.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use super::error::ErrorEnvelope;

pub type Timestamp = String;
pub type AutomationId = String;
pub type OccurrenceId = String;
pub type RunId = String;
pub type AttemptId = String;
pub type ReceiptId = String;
pub type AdoptionKey = String;
pub type CorrelationId = String;
pub type ExtensionBag = BTreeMap<String, Value>;
pub type JsonObject = BTreeMap<String, Value>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaVersion {
    #[serde(rename = "coven.automations.v1")]
    V1,
}

/// A wire-number that accepts and emits exactly `1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VersionOne;

impl Serialize for VersionOne {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(1)
    }
}

impl<'de> Deserialize<'de> for VersionOne {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match u8::deserialize(deserializer)? {
            1 => Ok(Self),
            value => Err(serde::de::Error::custom(format!(
                "expected version 1, got {value}"
            ))),
        }
    }
}

/// The v1 conditions union has no variants, so only an empty array is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EmptyConditions;

impl Serialize for EmptyConditions {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Vec::<Value>::new().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for EmptyConditions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = Vec::<Value>::deserialize(deserializer)?;
        if values.is_empty() {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(
                "coven.automations.v1 defines no condition variants",
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DigestValue {
    pub algorithm: DigestAlgorithm,
    pub canonicalization: Canonicalization,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DigestAlgorithm {
    #[serde(rename = "sha256")]
    Sha256,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Canonicalization {
    #[serde(rename = "jcs-rfc8785")]
    JcsRfc8785,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrincipalRef {
    pub principal_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FamiliarRef {
    pub familiar_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeDescriptor {
    pub runtime_id: String,
    pub capabilities: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalRef {
    pub approval_policy_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_record_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyClassification {
    Public,
    Operational,
    Sensitive,
    Restricted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetentionClass {
    pub classification: RetentionClassification,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete_after: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClassification {
    Ephemeral,
    Standard,
    Extended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProducerIdentity {
    pub component: String,
    pub instance_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub implementation_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Provenance {
    pub created_by: PrincipalRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<PrincipalRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActivationWindow {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_from: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effective_until: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefinitionLifecycleState {
    Draft,
    Paused,
    Active,
    Disabled,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationDefinition {
    pub schema_version: SchemaVersion,
    pub automation_id: AutomationId,
    pub revision: u64,
    pub integrity: DigestValue,
    pub lifecycle_state: DefinitionLifecycleState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion: Option<DefinitionDeletion>,
    pub display: DefinitionDisplay,
    pub trigger: ScheduleTrigger,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<EmptyConditions>,
    pub action: FamiliarInvocationAction,
    pub binding: DefinitionBinding,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_requirements: Option<RuntimeDescriptor>,
    pub policies: DefinitionPolicies,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provenance: Option<Provenance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activation: Option<ActivationWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionBag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionDeletion {
    pub tombstoned: True,
    pub requested_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<PrincipalRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct True;

impl Serialize for True {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for True {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom("expected true"))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionDisplay {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleTrigger {
    pub variant: ScheduleVariant,
    pub version: VersionOne,
    pub schedule: Schedule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleVariant {
    #[serde(rename = "schedule")]
    Schedule,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Schedule {
    pub rrule: String,
    pub timezone: Timezone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Timezone {
    Local,
    Utc,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FamiliarInvocationAction {
    pub variant: FamiliarInvocationVariant,
    pub version: VersionOne,
    pub prompt: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FamiliarInvocationVariant {
    #[serde(rename = "familiarInvocation")]
    FamiliarInvocation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionBinding {
    pub familiar_binding_policy: FamiliarBindingPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub familiar_id: Option<String>,
    pub authority: ApprovalRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FamiliarBindingPolicy {
    Exact,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionPolicies {
    pub timeout: TimeoutPolicy,
    pub retry: RetryPolicy,
    pub concurrency: ConcurrencyPolicy,
    pub misfire: MisfirePolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<DeliveryPolicy>,
    pub retention: RetentionPolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimeoutPolicy {
    pub per_run_minutes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryPolicy {
    pub max_attempts: u64,
    pub backoff_policy: BackoffPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backoff_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retryable_classes: Option<Vec<RetryableClass>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackoffPolicy {
    None,
    Fixed,
    Exponential,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryableClass {
    TransientDispatch,
    LeaseExpired,
    RuntimeUnavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConcurrencyPolicy {
    pub overlap: OverlapPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OverlapPolicy {
    Forbid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MisfirePolicy {
    pub disposition: MisfirePolicyDisposition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MisfirePolicyDisposition {
    Latest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeliveryPolicy {
    pub output_target: String,
    pub mode: DeliveryMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    Atomic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetentionPolicy {
    pub occurrence_history: RetentionClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_logs: Option<RetentionClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipts: Option<RetentionClass>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OccurrenceState {
    Planned,
    Eligible,
    Claimed,
    Dispatching,
    Running,
    Recovering,
    RecoveryRequired,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Skipped,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MisfireDisposition {
    None,
    CollapsedToLatest,
    SkippedOverlap,
    SkippedPaused,
    SkippedInvalid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationOccurrence {
    pub schema_version: SchemaVersion,
    pub occurrence_id: OccurrenceId,
    pub automation_id: AutomationId,
    pub automation_revision: u64,
    pub trigger_identity: TriggerIdentity,
    pub occurrence_key: String,
    pub scheduled_for: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eligible_at: Option<Timestamp>,
    pub state: OccurrenceState,
    pub state_reason: String,
    pub fence: OccurrenceFence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub misfire_disposition: Option<MisfireDisposition>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim_metadata: Option<ClaimMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_run_ref: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancellation: Option<Cancellation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery: Option<Recovery>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_window: Option<EventWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionBag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TriggerIdentity {
    pub kind: TriggerIdentityKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rrule_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<PrincipalRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TriggerIdentityKind {
    #[serde(rename = "schedule.slot")]
    ScheduleSlot,
    #[serde(rename = "manual.request")]
    ManualRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OccurrenceFence {
    pub generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ClaimMetadata {
    pub claimed_at: Timestamp,
    pub lease_minutes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Cancellation {
    pub requested_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_by: Option<PrincipalRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciled_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Recovery {
    pub entered_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence: Option<RecoveryEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_disposition: Option<RecoveryDisposition>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryEvidence {
    LeaseExpired,
    DispatchUnconfirmed,
    RuntimeLost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryDisposition {
    FailedDeterministic,
    FailedAmbiguous,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventWindow {
    pub first_sequence: u64,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    Accepted,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Ambiguous,
}

pub type RunOutcome = RunState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationRun {
    pub schema_version: SchemaVersion,
    pub run_id: RunId,
    pub occurrence_id: OccurrenceId,
    pub automation_id: AutomationId,
    pub automation_revision: u64,
    pub binding: RunBinding,
    pub state: RunState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<String>,
    pub attempt_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_attempt_id: Option<AttemptId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_disposition: Option<TerminalDisposition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<RunDelivery>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_digest: Option<DigestValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_ref: Option<ReceiptId>,
    pub started_at: Timestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionBag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunBinding {
    pub familiar: FamiliarRef,
    pub authority: AuthorityBinding,
    pub runtime: RuntimeDescriptor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityBinding {
    pub principal: PrincipalRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication_class: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TerminalDisposition {
    pub outcome: TerminalOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<TerminalFailureClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalOutcome {
    Succeeded,
    Failed,
    Cancelled,
    TimedOut,
    Ambiguous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalFailureClass {
    LaunchRefused,
    RuntimeError,
    Timeout,
    CancelledByRequest,
    LeaseExpired,
    AmbiguousEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RunDelivery {
    pub status: RunDeliveryStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_refs: Option<Vec<ArtifactRef>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunDeliveryStatus {
    None,
    Pending,
    Committed,
    Refused,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactRef {
    pub r#ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub digest: Option<DigestValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationAttempt {
    pub schema_version: SchemaVersion,
    pub attempt_id: AttemptId,
    pub run_id: RunId,
    pub occurrence_id: OccurrenceId,
    pub attempt_number: u64,
    pub adoption_key: AdoptionKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prior_disposition: Option<PriorDisposition>,
    pub dispatch_fence: DispatchFence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worker_correlation: Option<WorkerCorrelation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_classification: Option<RetryClassification>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lease_observations: Option<Vec<LeaseObservation>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_cursors: Option<OutputCursors>,
    pub state: AttemptState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opened_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settled_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<ExtensionBag>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PriorDisposition {
    pub attempt_number: u64,
    pub outcome: PriorAttemptOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriorAttemptOutcome {
    Failed,
    TimedOut,
    Ambiguous,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DispatchFence {
    pub occurrence_fence_generation: u64,
    pub dispatch_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkerCorrelation {
    pub worker_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adopted_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RetryClassification {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification: Option<RetryClassificationKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eligible_classes: Option<Vec<RetryableClass>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryClassificationKind {
    Initial,
    AutomaticRetry,
    OperatorRetry,
    OperatorRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LeaseObservation {
    pub observed_at: Timestamp,
    pub heartbeat_ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputCursors {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_cursor: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_cursor: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SideEffectClass {
    None,
    LocalRead,
    LocalWrite,
    ExternalRead,
    ExternalMutation,
    IrreversibleExternalMutation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationReceipt {
    pub schema_version: SchemaVersion,
    pub receipt_id: ReceiptId,
    pub automation_id: AutomationId,
    pub automation_revision: u64,
    pub definition_digest: DigestValue,
    pub occurrence_id: OccurrenceId,
    pub occurrence_fence_generation: u64,
    pub run_id: RunId,
    pub attempt_id: AttemptId,
    pub attempt_number: u64,
    pub identity: FamiliarRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<ReceiptAuthority>,
    pub runtime: RuntimeDescriptor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_digest: Option<DigestValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_digest: Option<DigestValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exercised_capabilities: Option<Vec<String>>,
    pub side_effect_class: SideEffectClass,
    pub outcome: ReceiptOutcome,
    pub produced_at: Timestamp,
    pub producer: ProducerIdentity,
    pub integrity: ReceiptIntegrity,
    pub privacy: ReceiptPrivacy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptAuthority {
    pub principal: PrincipalRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval: Option<ApprovalRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptOutcome {
    pub disposition: TerminalOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partial_failures: Option<Vec<PartialFailure>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_disposition: Option<ReceiptRecoveryDisposition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PartialFailure {
    pub step: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovered: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptRecoveryDisposition {
    NotRequired,
    RecoveredInline,
    DeferredToOperator,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptIntegrity {
    pub algorithm: DigestAlgorithm,
    pub canonicalization: Canonicalization,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication: Option<ReceiptAuthentication>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReceiptAuthentication {
    None,
    ProducerHmac,
    Cosign,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptPrivacy {
    pub classification: PrivacyClassification,
    pub retention: RetentionClass,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandName {
    #[serde(rename = "definition.create.v1")]
    DefinitionCreate,
    #[serde(rename = "definition.revise.v1")]
    DefinitionRevise,
    #[serde(rename = "definition.activate.v1")]
    DefinitionActivate,
    #[serde(rename = "definition.pause.v1")]
    DefinitionPause,
    #[serde(rename = "definition.disable.v1")]
    DefinitionDisable,
    #[serde(rename = "definition.tombstone.v1")]
    DefinitionTombstone,
    #[serde(rename = "occurrence.runNow.v1")]
    OccurrenceRunNow,
    #[serde(rename = "occurrence.cancel.v1")]
    OccurrenceCancel,
    #[serde(rename = "run.cancel.v1")]
    RunCancel,
    #[serde(rename = "attempt.cancel.v1")]
    AttemptCancel,
    #[serde(rename = "attempt.retry.v1")]
    AttemptRetry,
    #[serde(rename = "occurrence.recover.v1")]
    OccurrenceRecover,
    #[serde(rename = "definition.list.v1")]
    DefinitionList,
    #[serde(rename = "definition.get.v1")]
    DefinitionGet,
    #[serde(rename = "run.history.v1")]
    RunHistory,
    #[serde(rename = "definition.health.v1")]
    DefinitionHealth,
    #[serde(rename = "events.read.v1")]
    EventsRead,
    #[serde(rename = "events.subscribe.v1")]
    EventsSubscribe,
    #[serde(rename = "legacy.import.v1")]
    LegacyImport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandRequest {
    pub schema_version: SchemaVersion,
    pub command: CommandName,
    pub adoption_key: AdoptionKey,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_revision: Option<u64>,
    pub origin: CommandOrigin,
    pub intent: CommandIntent,
    /// Per-command payloads are a schema-defined object bag.
    pub payload: JsonObject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandOrigin {
    pub principal: PrincipalRef,
    pub channel: CommandChannel,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authentication_class: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_at: Option<Timestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandChannel {
    #[serde(rename = "daemon-ipc")]
    DaemonIpc,
    #[serde(rename = "http")]
    Http,
    #[serde(rename = "control-action")]
    ControlAction,
    #[serde(rename = "cli")]
    Cli,
    #[serde(rename = "sdk")]
    Sdk,
    #[serde(rename = "cave")]
    Cave,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandIntent {
    pub statement: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandOutcome {
    Committed,
    Replayed,
    Rejected,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CommandResponse {
    pub schema_version: SchemaVersion,
    pub command: CommandName,
    pub adoption_key: AdoptionKey,
    pub outcome: CommandOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replay: Option<ReplayMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    /// Committed result bodies are explicitly open in the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<JsonObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_ref: Option<ReceiptId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_ref: Option<EventRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplayMetadata {
    pub first_committed_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventRef {
    pub stream: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventKind {
    #[serde(rename = "definition.created")]
    DefinitionCreated,
    #[serde(rename = "definition.revised")]
    DefinitionRevised,
    #[serde(rename = "definition.activated")]
    DefinitionActivated,
    #[serde(rename = "definition.paused")]
    DefinitionPaused,
    #[serde(rename = "definition.disabled")]
    DefinitionDisabled,
    #[serde(rename = "definition.invalidated")]
    DefinitionInvalidated,
    #[serde(rename = "definition.tombstoned")]
    DefinitionTombstoned,
    #[serde(rename = "definition.imported")]
    DefinitionImported,
    #[serde(rename = "occurrence.transitioned")]
    OccurrenceTransitioned,
    #[serde(rename = "occurrence.misfire_recorded")]
    OccurrenceMisfireRecorded,
    #[serde(rename = "run.transitioned")]
    RunTransitioned,
    #[serde(rename = "attempt.transitioned")]
    AttemptTransitioned,
    #[serde(rename = "receipt.recorded")]
    ReceiptRecorded,
    #[serde(rename = "feed.snapshot")]
    FeedSnapshot,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventEnvelope {
    pub schema_version: SchemaVersion,
    pub event_id: String,
    pub stream: StreamRef,
    pub sequence: u64,
    pub recorded_at: Timestamp,
    pub observed_at: Timestamp,
    pub producer: ProducerIdentity,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub causation: Option<EventCausation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<AutomationId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub occurrence_id: Option<OccurrenceId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<RunId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<AttemptId>,
    pub kind: EventKind,
    pub summary: String,
    pub payload: EventPayload,
    pub privacy: EventPrivacy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub integrity: Option<DigestValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StreamRef {
    pub kind: StreamKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamKind {
    Automation,
    Occurrence,
    Run,
    Feed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventCausation {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adoption_key: Option<AdoptionKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause_event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EventPayload {
    DefinitionLifecycle(DefinitionLifecyclePayload),
    Transition(TransitionPayload),
    Misfire(MisfirePayload),
    Receipt(ReceiptPayload),
    Snapshot(SnapshotPayload),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DefinitionLifecyclePayload {
    pub revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition_digest: Option<DigestValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle_state: Option<EventLifecycleState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventLifecycleState {
    Draft,
    Paused,
    Active,
    Disabled,
    Invalid,
    Tombstoned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TransitionPayload {
    pub entity: TransitionEntity,
    pub from: String,
    pub to: String,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fence_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt_number: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_adoption_key: Option<AdoptionKey>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransitionEntity {
    Occurrence,
    Run,
    Attempt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MisfirePayload {
    pub disposition: MisfireDisposition,
    pub collapsed_slots: Vec<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReceiptPayload {
    pub receipt_ref: ReceiptId,
    pub outcome: TerminalOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side_effect_class: Option<SideEffectClass>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SnapshotPayload {
    pub through_sequence: u64,
    /// Compacted state is explicitly an open, schema-defined JSON object.
    pub state: JsonObject,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<SnapshotReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotReason {
    RetentionCompaction,
    ManualSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EventPrivacy {
    pub classification: PrivacyClassification,
    pub retention: RetentionClass,
}

impl fmt::Display for SchemaVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("coven.automations.v1")
    }
}
