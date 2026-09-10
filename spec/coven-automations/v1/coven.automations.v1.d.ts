/**
 * coven.automations.v1 — pinned TypeScript contract types.
 *
 * SPEC ARTIFACT, NOT A PACKAGE MODULE. This file is the portable,
 * hand-pinned type projection of the JSON Schemas in this directory
 * (draft 2020-12). SDK and Cave canaries consume it as a pinned artifact
 * — never as a source-relative import of Coven internals — and must pair
 * it with test-vectors.json for behavioral conformance.
 *
 * Regeneration: any change here requires a contract-profile decision per
 * compatibility-matrix.json. Field shapes mirror the schemas exactly;
 * descriptions are omitted here and live in the schemas.
 *
 * Consumers must treat unknown variants (trigger, action, condition,
 * policy values, event kinds) as failures per the negative negotiation
 * rules in capabilities.json — TS unions here are closed.
 */

export type SchemaVersion = "coven.automations.v1";

/** RFC 3339 UTC, millisecond precision (matches the #816 store encoding). */
export type Timestamp = string;

export interface Digest {
  algorithm: "sha256";
  canonicalization: "jcs-rfc8785";
  /** Lowercase hex SHA-256 over the RFC 8785 canonical serialization of the covered object. */
  value: string;
}

export type AdoptionKey = string;
export type CorrelationId = string;
/** Exact IANA TZID validated by the Rust authority before persistence. */
export type IanaTimezoneId = string & { readonly __ianaTimezoneId: unique symbol };

export interface PrincipalRef {
  principalId: string;
  displayName?: string;
}

export interface FamiliarRef {
  familiarId: string;
}

export interface RuntimeDescriptor {
  runtimeId: string;
  capabilities: string[];
  model?: string;
}

export interface ApprovalRef {
  approvalPolicyRef: string;
  approvalRecordRef?: string;
}

/** Keys are `x-prefixed` or reverse-DNS namespaced; values opaque; preserve, never interpret. */
export type ExtensionBag = Record<string, unknown>;

export type PrivacyClassification = "public" | "operational" | "sensitive" | "restricted";

export interface RetentionClass {
  classification: "ephemeral" | "standard" | "extended";
  deleteAfter?: Timestamp;
}

export interface ProducerIdentity {
  component: string;
  instanceId: string;
  implementationVersion?: string;
}

export interface Provenance {
  createdBy: PrincipalRef;
  createdAt?: Timestamp;
  updatedBy?: PrincipalRef;
  updatedAt?: Timestamp;
  importedFrom?: string;
}

// ---------------------------------------------------------------------------
// AutomationDefinition
// ---------------------------------------------------------------------------

export type DefinitionLifecycleState = "draft" | "paused" | "active" | "disabled" | "invalid";

export interface AutomationDefinition {
  schemaVersion: SchemaVersion;
  automationId: string;
  /** Monotonic per automation; incremented by exactly one per accepted mutation. */
  revision: number;
  integrity: Digest;
  lifecycleState: DefinitionLifecycleState;
  deletion?: {
    tombstoned: true;
    requestedAt: Timestamp;
    requestedBy?: PrincipalRef;
    reason?: string;
  };
  display: {
    name: string;
    description?: string;
    tags?: string[];
  };
  trigger: ScheduleTrigger; // v1 union: exactly this variant; future variants extend the union in later profiles
  conditions?: Condition[]; // v1 defines zero condition variants; any value fails validation
  action: FamiliarInvocationAction; // v1 union: exactly this variant
  binding: {
    familiarBindingPolicy: "exact";
    familiarId: string;
    authority: ApprovalRef;
  };
  runtimeRequirements?: RuntimeDescriptor; // required for active/paused/disabled
  policies: {
    timeout: { perRunMinutes: number }; // 1..=44640
    retry: {
      maxAttempts: number; // 1..=10
      backoffPolicy: "none" | "fixed" | "exponential";
      backoffSeconds?: number; // required when backoffPolicy is "fixed"
      retryableClasses?: Array<"transient_dispatch" | "lease_expired" | "runtime_unavailable">;
    };
    concurrency: { overlap: "forbid" };
    misfire: { disposition: "latest" };
    // Reserved wire shape; capabilities.json currently refuses outputTarget.atomic.
    delivery?: {
      outputTarget: string;
      mode: "atomic";
    };
    retention: {
      occurrenceHistory: RetentionClass;
      runLogs?: RetentionClass;
      receipts?: RetentionClass;
    };
  };
  provenance?: Provenance;
  activation?: {
    effectiveFrom?: Timestamp;
    effectiveUntil?: Timestamp;
  };
  extensions?: ExtensionBag;
}

export interface ScheduleTrigger {
  variant: "schedule";
  version: 1;
  schedule: {
    /** Scoped RRULE: FREQ=DAILY|WEEKLY, optional BYHOUR list, optional BYDAY list for weekly. */
    rrule: string;
    timezone: "utc" | IanaTimezoneId;
  };
}

/** v1 has zero condition variants; this type is intentionally uninhabited. */
export type Condition = never;

export interface FamiliarInvocationAction {
  variant: "familiarInvocation";
  version: 1;
  prompt: string;
  cwd?: string;
}

// ---------------------------------------------------------------------------
// AutomationOccurrence / AutomationRun / AutomationAttempt / AutomationReceipt
// ---------------------------------------------------------------------------

export type OccurrenceState =
  | "planned"
  | "eligible"
  | "claimed"
  | "dispatching"
  | "running"
  | "recovering"
  | "recovery_required"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "timed_out"
  | "skipped"
  | "superseded";

export type MisfireDisposition =
  | "none"
  | "collapsed_to_latest"
  | "skipped_overlap"
  | "skipped_paused"
  | "skipped_invalid";

export interface AutomationOccurrence {
  schemaVersion: SchemaVersion;
  occurrenceId: string;
  automationId: string;
  /** Exact definition revision executed against; never rewritten. */
  automationRevision: number;
  triggerIdentity:
    | { kind: "schedule.slot"; rruleRef: string }
    | { kind: "manual.request"; requestedBy?: PrincipalRef };
  /** `automationId@scheduledFor` or `automationId@manual-<adoptionKey>`. */
  occurrenceKey: string;
  scheduledFor: Timestamp;
  observedAt?: Timestamp;
  eligibleAt?: Timestamp;
  state: OccurrenceState;
  stateReason: string;
  fence: {
    generation: number; // monotonic, >= 1
    claimedBy?: string;
    leaseExpiresAt?: Timestamp;
  };
  misfireDisposition?: MisfireDisposition;
  claimMetadata?: {
    claimedAt: Timestamp;
    leaseMinutes: number; // 1..=1440
  };
  activeRunRef?: string;
  cancellation?: {
    requestedAt: Timestamp;
    requestedBy?: PrincipalRef;
    acknowledgedAt?: Timestamp;
    reconciledAt?: Timestamp;
  };
  recovery?: {
    enteredAt: Timestamp;
    evidence?: "lease_expired" | "dispatch_unconfirmed" | "runtime_lost";
    resolvedDisposition?: "failed_deterministic" | "failed_ambiguous";
  };
  createdAt?: Timestamp;
  updatedAt?: Timestamp;
  eventWindow?: {
    firstSequence: number;
    lastSequence: number;
  };
  extensions?: ExtensionBag;
}

export type RunState =
  | "accepted"
  | "running"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "timed_out"
  | "ambiguous";

export type RunOutcome =
  | "succeeded"
  | "failed"
  | "cancelled"
  | "timed_out"
  | "ambiguous";

export interface AutomationRun {
  schemaVersion: SchemaVersion;
  runId: string;
  occurrenceId: string;
  automationId: string;
  automationRevision: number;
  binding: {
    familiar: FamiliarRef;
    authority: {
      principal: PrincipalRef;
      approval?: ApprovalRef;
      authenticationClass?: string;
    };
    runtime: RuntimeDescriptor;
  };
  state: RunState;
  stateReason?: string;
  attemptCount: number;
  currentAttemptId?: string;
  terminalDisposition?: {
    outcome: RunOutcome;
    failureClass?:
      | "launch_refused"
      | "runtime_error"
      | "timeout"
      | "cancelled_by_request"
      | "lease_expired"
      | "ambiguous_evidence";
    detail?: string;
  };
  delivery?: {
    status: "none" | "pending" | "committed" | "refused" | "rolled_back";
    target?: string;
    artifactRefs?: Array<{ ref: string; digest?: Digest }>;
  };
  resultDigest?: Digest;
  receiptRef?: string;
  startedAt: Timestamp;
  finishedAt?: Timestamp; // required when state is terminal
  extensions?: ExtensionBag;
}

export type AttemptState =
  | "adopted"
  | "dispatching"
  | "started"
  | "observing"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "timed_out"
  | "ambiguous";

export interface AutomationAttempt {
  schemaVersion: SchemaVersion;
  attemptId: string;
  runId: string;
  occurrenceId: string;
  /** Monotonic within the run, starting at 1; never reused. */
  attemptNumber: number;
  adoptionKey: AdoptionKey;
  priorDisposition?: {
    attemptNumber: number;
    outcome: "failed" | "timed_out" | "ambiguous" | "cancelled";
  };
  dispatchFence: {
    occurrenceFenceGeneration: number;
    dispatchGeneration: number;
  };
  workerCorrelation?: {
    workerId: string;
    sessionId?: string; // at most one session binds to one attempt
    adoptedAt?: Timestamp;
  };
  retryClassification?: {
    classification?: "initial" | "automatic_retry" | "operator_retry" | "operator_recovery";
    eligibleClasses?: Array<"transient_dispatch" | "lease_expired" | "runtime_unavailable">;
  };
  leaseObservations?: Array<{
    observedAt: Timestamp;
    heartbeatOk: boolean;
    note?: string;
  }>;
  outputCursors?: {
    eventCursor?: number;
    logCursor?: number;
  };
  state: AttemptState;
  stateReason?: string;
  openedAt?: Timestamp;
  settledAt?: Timestamp;
  extensions?: ExtensionBag;
}

export type SideEffectClass =
  | "none"
  | "local_read"
  | "local_write"
  | "external_read"
  | "external_mutation"
  | "irreversible_external_mutation";

export interface AutomationReceipt {
  schemaVersion: SchemaVersion;
  receiptId: string;
  automationId: string;
  automationRevision: number;
  definitionDigest: Digest;
  occurrenceId: string;
  occurrenceFenceGeneration: number;
  runId: string;
  attemptId: string;
  attemptNumber: number;
  identity: FamiliarRef;
  authority?: {
    principal: PrincipalRef;
    approval?: ApprovalRef;
  };
  runtime: RuntimeDescriptor;
  deliveryDigest?: Digest;
  resultDigest?: Digest;
  exercisedCapabilities?: string[];
  sideEffectClass: SideEffectClass;
  outcome: {
    disposition: RunOutcome;
    failureClass?: string;
    detail?: string;
    partialFailures?: Array<{
      step: string;
      reason: string;
      recovered?: boolean;
    }>;
    recoveryDisposition?: "not_required" | "recovered_inline" | "deferred_to_operator";
  };
  producedAt: Timestamp;
  producer: ProducerIdentity;
  integrity: Digest & {
    authentication: "none" | "producer-hmac" | "cosign";
  };
  privacy: {
    classification: PrivacyClassification;
    retention: RetentionClass;
    notes?: string;
  };
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

export type CommandName =
  | "definition.create.v1"
  | "definition.revise.v1"
  | "definition.activate.v1"
  | "definition.pause.v1"
  | "definition.disable.v1"
  | "definition.tombstone.v1"
  | "occurrence.runNow.v1"
  | "occurrence.cancel.v1"
  | "run.cancel.v1"
  | "attempt.cancel.v1"
  | "attempt.retry.v1"
  | "occurrence.recover.v1"
  | "definition.list.v1"
  | "definition.get.v1"
  | "run.history.v1"
  | "definition.health.v1"
  | "events.read.v1"
  | "events.subscribe.v1"
  | "legacy.import.v1";

export interface CommandOrigin {
  principal: PrincipalRef;
  channel: "daemon-ipc" | "http" | "control-action" | "cli" | "sdk" | "cave";
  authenticationClass?: string;
  requestedAt?: Timestamp;
  correlationId?: CorrelationId;
}

/** Payload shapes per command; see command-envelope.schema.json for the normative pin. */
export interface CommandPayloadByCommand {
  "definition.create.v1": { definition: AutomationDefinition };
  "definition.revise.v1": { definition: AutomationDefinition };
  "definition.activate.v1": { automationId: string; reason?: string };
  "definition.pause.v1": { automationId: string; reason?: string };
  "definition.disable.v1": { automationId: string; reason?: string };
  "definition.tombstone.v1": { automationId: string; reason?: string };
  "occurrence.runNow.v1": { automationId: string; note?: string; bypassEligibility?: boolean };
  "occurrence.cancel.v1": { occurrenceId: string; reason?: string };
  "run.cancel.v1": { runId: string; reason?: string };
  "attempt.cancel.v1": { attemptId: string; reason?: string };
  "attempt.retry.v1": {
    runId: string;
    priorAttemptNumber: number;
    priorDisposition: "failed" | "timed_out" | "cancelled";
    note?: string;
  };
  "occurrence.recover.v1": {
    occurrenceId: string;
    evidenceDetermination: "failed_deterministic" | "retry_with_new_attempt";
    statement: string;
  };
  "definition.list.v1": {
    lifecycleState?: "draft" | "paused" | "active" | "disabled" | "invalid" | "tombstoned" | "all";
    limit?: number;
    cursor?: string;
  };
  "definition.get.v1": { automationId: string; revision?: number };
  "run.history.v1": {
    automationId: string;
    occurrenceId?: string;
    limit?: number;
    cursor?: string;
  };
  "definition.health.v1": { automationId: string };
  "events.read.v1": {
    stream: StreamRef;
    after?: number;
    limit?: number;
    from?: Timestamp;
  };
  "events.subscribe.v1": {
    stream: StreamRef;
    after?: number;
    checkpoint?: string;
  };
  "legacy.import.v1": { source: "codex-automation-toml"; dryRun?: boolean };
}

export interface StreamRef {
  kind: "automation" | "occurrence" | "run" | "feed";
  id: string;
}

export interface CommandEnvelope<C extends CommandName = CommandName> {
  schemaVersion: SchemaVersion;
  command: C;
  adoptionKey: AdoptionKey;
  /** Required for definition.revise/activate/pause/disable/tombstone; forbidden otherwise. */
  expectedRevision?: number;
  origin: CommandOrigin;
  intent: { statement: string };
  payload: CommandPayloadByCommand[C];
}

export type ErrorCode =
  | "SCHEMA_VERSION_UNSUPPORTED"
  | "VALIDATION_FAILED"
  | "ADOPTION_REPLAY_MISMATCH"
  | "REVISION_CONFLICT"
  | "NOT_FOUND"
  | "GONE_TOMBSTONED"
  | "CAPABILITY_UNSUPPORTED"
  | "ILLEGAL_TRANSITION"
  | "AUTHORITY_REQUIRED"
  | "APPROVAL_REQUIRED"
  | "CANCEL_PENDING"
  | "OVERLAP_FORBIDDEN"
  | "RETRY_DISPOSITION_INVALID"
  | "AMBIGUOUS_RETRY_FORBIDDEN"
  | "CURSOR_EXPIRED"
  | "STREAM_OUT_OF_ORDER"
  | "PAYLOAD_TOO_LARGE"
  | "DEADLINE_EXCEEDED"
  | "CONCURRENCY_LIMIT"
  | "INTERNAL";

export interface ErrorEnvelope {
  code: ErrorCode;
  httpStatus: number;
  message: string;
  retryable: boolean;
  details?: Record<string, unknown>;
  adoption?: {
    key: AdoptionKey;
    conflictOutcome?: "committed" | "rejected";
  };
  currentRevision?: number;
}

export interface EventRef {
  stream: string;
  sequence: number;
}

export type CommandResultByCommand<C extends CommandName> =
  C extends "events.read.v1" | "events.subscribe.v1" ? EventPage : Record<string, unknown>;

export interface CommandResponse<C extends CommandName = CommandName> {
  schemaVersion: SchemaVersion;
  command: C;
  adoptionKey: AdoptionKey;
  outcome: "committed" | "replayed" | "rejected";
  replay?: { firstCommittedAt: Timestamp };
  revision?: number;
  result?: CommandResultByCommand<C>;
  error?: ErrorEnvelope;
  receiptRef?: string;
  eventRef?: EventRef;
}

// ---------------------------------------------------------------------------
// Events / changefeed
// ---------------------------------------------------------------------------

export type EventKind =
  | "definition.created"
  | "definition.revised"
  | "definition.activated"
  | "definition.paused"
  | "definition.disabled"
  | "definition.invalidated"
  | "definition.tombstoned"
  | "definition.imported"
  | "occurrence.transitioned"
  | "occurrence.misfire_recorded"
  | "run.transitioned"
  | "attempt.transitioned"
  | "receipt.recorded"
  | "feed.snapshot";

export interface EventPage {
  stream: StreamRef;
  /** Concrete exclusive cursor used for this page; null means the stream beginning. */
  after: number | null;
  events: EventEnvelope[];
  /** Last delivered sequence, or the concrete exclusive cursor when the page is empty. */
  nextAfter: number | null;
  checkpoint: string;
  checkpointExpiresAt: Timestamp;
}

export interface DefinitionLifecycleEventPayload {
  revision: number;
  definitionDigest?: Digest;
  lifecycleState?: DefinitionLifecycleState | "tombstoned";
  importedFrom?: string;
}

export interface TransitionEventPayload {
  entity: "occurrence" | "run" | "attempt";
  from: string;
  to: string;
  reason: string;
  fenceGeneration?: number;
  attemptNumber?: number;
  commandAdoptionKey?: AdoptionKey;
}

export interface MisfireEventPayload {
  disposition: MisfireDisposition;
  collapsedSlots: Timestamp[];
}

export interface ReceiptEventPayload {
  receiptRef: string;
  outcome: RunOutcome;
  sideEffectClass?: SideEffectClass;
}

export interface SnapshotEventPayload {
  throughSequence: number;
  state: Record<string, unknown>;
  reason?: "retention_compaction" | "manual_snapshot";
}

export type EventPayload =
  | DefinitionLifecycleEventPayload
  | TransitionEventPayload
  | MisfireEventPayload
  | ReceiptEventPayload
  | SnapshotEventPayload;

export interface EventEnvelopeBase {
  schemaVersion: SchemaVersion;
  /** Globally unique; duplicates of a delivered eventId are redeliveries: ignore, never re-apply. */
  eventId: string;
  stream: { kind: "automation" | "occurrence" | "run" | "feed"; id: string };
  /** Monotonically increasing, gapless within `stream`. */
  sequence: number;
  recordedAt: Timestamp;
  observedAt: Timestamp;
  producer: ProducerIdentity;
  causation?: {
    adoptionKey?: AdoptionKey;
    causeEventId?: string;
    correlationId?: CorrelationId;
  };
  automationId?: string;
  occurrenceId?: string;
  runId?: string;
  attemptId?: string;
  summary: string;
  privacy: {
    classification: PrivacyClassification;
    retention: RetentionClass;
  };
  integrity?: Digest;
}

export type EventEnvelope = EventEnvelopeBase &
  (
    | {
        kind:
          | "definition.created"
          | "definition.revised"
          | "definition.activated"
          | "definition.paused"
          | "definition.disabled"
          | "definition.invalidated"
          | "definition.tombstoned"
          | "definition.imported";
        payload: DefinitionLifecycleEventPayload;
      }
    | {
        kind: "occurrence.transitioned";
        payload: TransitionEventPayload & { entity: "occurrence" };
      }
    | {
        kind: "run.transitioned";
        payload: TransitionEventPayload & { entity: "run" };
      }
    | {
        kind: "attempt.transitioned";
        payload: TransitionEventPayload & { entity: "attempt" };
      }
    | {
        kind: "occurrence.misfire_recorded";
        payload: MisfireEventPayload;
      }
    | {
        kind: "receipt.recorded";
        payload: ReceiptEventPayload;
      }
    | {
        kind: "feed.snapshot";
        payload: SnapshotEventPayload;
      }
  );
