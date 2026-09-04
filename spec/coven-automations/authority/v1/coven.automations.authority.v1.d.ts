/**
 * Pinned TypeScript projection for coven.automations.authority.v1.
 * Rust remains authoritative; this file is a portable contract artifact only.
 */

export type AuthorityProfile = "coven.automations.authority.v1";
export type Timestamp = string;
export type OpaqueIdentifier = string;
export type Capability = string;
export type RiskClass = "R0" | "R1" | "R2" | "R3" | "R4";
export type SideEffectClass =
  | "none"
  | "local_read"
  | "local_write"
  | "external_read"
  | "external_mutation"
  | "irreversible_external_mutation";

export interface Digest {
  algorithm: "sha256";
  canonicalization: "jcs-rfc8785";
  value: string;
}

export interface AuthorityProducer {
  component: OpaqueIdentifier;
  instanceId: OpaqueIdentifier;
  implementationVersion: string;
}

export interface AuthorityAuthentication {
  method: "ed25519";
  keyId: OpaqueIdentifier;
  proofRef: OpaqueIdentifier;
  signedDigest: string;
  signature: string;
}

export interface DeniedCapability {
  capability: Capability;
  reasonCode: OpaqueIdentifier;
}

export type ApprovalBinding =
  | {
      requirement: "not_required";
      evidence: null;
      scopeDigest: null;
      expiresAt: null;
      use: null;
      consumption: { state: "not_required" };
    }
  | {
      requirement: "human_per_run" | "protected_owner_per_run";
      evidence: {
        approvalId: OpaqueIdentifier;
        approvalDigest: Digest;
        state: "approved";
      };
      scopeDigest: Digest;
      expiresAt: Timestamp;
      use: { kind: "single_use" };
      consumption: {
        state: "consumed_for_dispatch";
        eventId: OpaqueIdentifier;
        eventDigest: Digest;
        requestDigest: Digest;
        decisionDigest: Digest;
        occurrenceId: OpaqueIdentifier;
        runId: OpaqueIdentifier;
        attemptNumber: number;
        fenceGeneration: number;
      };
    }
  | {
      requirement: "bounded_recurring";
      evidence: {
        approvalId: OpaqueIdentifier;
        approvalDigest: Digest;
        state: "approved";
      };
      scopeDigest: Digest;
      expiresAt: Timestamp;
      use: {
        kind: "recurring";
        grantId: OpaqueIdentifier;
        maxUses: number;
        occurrencePrefix: string;
        priorUses: number;
      };
      consumption: {
        state: "consumed_for_dispatch";
        eventId: OpaqueIdentifier;
        eventDigest: Digest;
        requestDigest: Digest;
        decisionDigest: Digest;
        occurrenceId: OpaqueIdentifier;
        runId: OpaqueIdentifier;
        attemptNumber: number;
        fenceGeneration: number;
        usageNumber: number;
      };
    };

export interface BindingAuthorization {
  operation: OpaqueIdentifier;
  requestId: OpaqueIdentifier;
  requestDigest: Digest;
  decisionId: OpaqueIdentifier;
  decisionDigest: Digest;
  nonce: OpaqueIdentifier;
  issuedAt: Timestamp;
  validFrom: Timestamp;
  validUntil: Timestamp;
  replayState: "fresh";
  consumptionSnapshotId: OpaqueIdentifier;
  consumptionSnapshotDigest: Digest;
  consumptionStoreRevision: number;
}

export interface AutomationExecutionBindingCommon {
  profile: AuthorityProfile;
  kind: "AutomationExecutionBinding";
  bindingId: OpaqueIdentifier;
  base: {
    automationId: OpaqueIdentifier;
    automationRevision: number;
    definitionDigest: Digest;
    occurrenceId: OpaqueIdentifier;
    occurrenceKey: OpaqueIdentifier;
    occurrenceFenceGeneration: number;
    runId: OpaqueIdentifier;
    attemptId: OpaqueIdentifier;
    attemptNumber: number;
    adoptionKey: OpaqueIdentifier;
  };
  principal: {
    principalId: OpaqueIdentifier;
    authorizationProofRef: OpaqueIdentifier;
    authenticationState: "authenticated";
  };
  familiar: {
    familiarRootId: OpaqueIdentifier;
    identityRevisionId: OpaqueIdentifier;
    declarationDigest: Digest;
    embodimentBindingId: OpaqueIdentifier;
    embodimentDigest: Digest;
    statusAtDecision: "active";
    verifiedAt: Timestamp;
    freshnessPolicyVersion: OpaqueIdentifier;
    freshnessBoundSeconds: number;
  };
  contextProjection: {
    projectId: OpaqueIdentifier;
    workspaceId: OpaqueIdentifier;
    contextProjectionIds: OpaqueIdentifier[];
    memoryProjectionIds: OpaqueIdentifier[];
  };
  threads: {
    decisionId: OpaqueIdentifier;
    decisionDigest: Digest;
    protectedSurfaceManifestId: OpaqueIdentifier;
    protectedSurfaceManifestDigest: Digest;
  };
  capabilities: {
    requested: Capability[];
    granted: Capability[];
    denied: DeniedCapability[];
    degraded: Capability[];
  };
  risk: {
    riskClass: RiskClass;
    sideEffectClass: SideEffectClass;
  };
  runtime: {
    runtimeId: OpaqueIdentifier;
    descriptorVersion: string;
    descriptorDigest: Digest;
    capabilities: Capability[];
    selectionRationale:
      | "exact_requirement_match"
      | "policy_preferred"
      | "only_conformant_runtime"
      | "operator_pinned";
  };
  versions: {
    baseProfile: "coven.automations.v1";
    authorityProfile: AuthorityProfile;
    familiarProfile: "familiar.embodiment_binding.v1";
    threadsProfile: "automation-authority/1.0.0";
    policyVersion: OpaqueIdentifier;
    policyDigest: Digest;
  };
  decisionTimestamp: Timestamp;
  producer: AuthorityProducer;
  integrity: Digest;
  authentication: AuthorityAuthentication;
}

export type AutomationExecutionBinding = AutomationExecutionBindingCommon &
  (
    | {
        authorization: BindingAuthorization & { outcome: "permit" };
        approval: Extract<ApprovalBinding, { requirement: "not_required" }>;
      }
    | {
        authorization: BindingAuthorization & { outcome: "requires_approval" };
        approval: Exclude<ApprovalBinding, { requirement: "not_required" }>;
      }
  );

export interface ReceiptAuthorization {
  operation: OpaqueIdentifier;
  requestId: OpaqueIdentifier;
  requestDigest: Digest;
  decisionId: OpaqueIdentifier;
  decisionDigest: Digest;
  consumptionSnapshotDigest: Digest;
}

export interface AutomationReceiptAuthorityEvidenceCommon {
  profile: AuthorityProfile;
  kind: "AutomationReceiptAuthorityEvidence";
  receiptId: OpaqueIdentifier;
  automationId: OpaqueIdentifier;
  automationRevision: number;
  definitionDigest: Digest;
  occurrenceId: OpaqueIdentifier;
  occurrenceFenceGeneration: number;
  runId: OpaqueIdentifier;
  attemptId: OpaqueIdentifier;
  attemptNumber: number;
  baseReceiptDigest: Digest;
  bindingId: OpaqueIdentifier;
  bindingDigest: Digest;
  principalId: OpaqueIdentifier;
  familiar: {
    familiarRootId: OpaqueIdentifier;
    identityRevisionId: OpaqueIdentifier;
    declarationDigest: Digest;
    statusAtDecision: "active";
    verifiedAt: Timestamp;
    freshnessPolicyVersion: OpaqueIdentifier;
    freshnessBoundSeconds: number;
  };
  capabilities: {
    requested: Capability[];
    granted: Capability[];
    denied: DeniedCapability[];
    degraded: Capability[];
    exercised: Capability[];
  };
  risk: {
    riskClass: RiskClass;
    sideEffectClass: SideEffectClass;
  };
  runtime: {
    runtimeId: OpaqueIdentifier;
    descriptorVersion: string;
    descriptorDigest: Digest;
    capabilities: Capability[];
  };
  decisionTimestamp: Timestamp;
  producer: AuthorityProducer;
  privacy: {
    classification: "operational" | "sensitive" | "restricted";
    retention: "ephemeral_24h" | "authority_evidence_90d" | "authority_evidence_1y";
    redactionStatus: "not_required" | "redacted" | "tombstoned";
    sensitiveMaterialIncluded: false;
  };
  integrity: Digest;
  authentication: AuthorityAuthentication;
}

export type AutomationReceiptAuthorityEvidence = AutomationReceiptAuthorityEvidenceCommon &
  (
    | {
        authorization: ReceiptAuthorization & { outcome: "permit" };
        approval: Extract<ApprovalBinding, { requirement: "not_required" }>;
      }
    | {
        authorization: ReceiptAuthorization & { outcome: "requires_approval" };
        approval: Exclude<ApprovalBinding, { requirement: "not_required" }>;
      }
  );

export interface AutomationAuthorityExtension {
  profile: AuthorityProfile;
  kind: "AutomationAuthorityExtension";
  executionBinding: AutomationExecutionBinding;
  receiptEvidence: AutomationReceiptAuthorityEvidence | null;
}
