//! The trusted Runtime Authority adapter (coven#857, slice 6).
//!
//! It composes slices 1–5 into the `AutomationDispatchAuthority` the runner
//! consults for a routine that declares authority. For one attempt, inside the
//! runner's launch transaction, it:
//!
//! 1. checks the owner grant: the owner activation of the current revision
//!    (slice 2), with a fresh nonce and validity window;
//! 2. picks the runtime envelope that enforces the declared grant, refusing a
//!    grant no envelope enforces;
//! 3. issues a familiar embodiment binding from the ledger (slice 3);
//! 4. decides the attempt's Threads request under a policy snapshot whose
//!    recurring grant is the owner activation, then verifies the dispatch
//!    bundle against a consumption snapshot and consumes the decision
//!    (slice 4);
//! 5. composes the `AutomationExecutionBinding`, pinning the envelope's
//!    descriptor digest for the terminal observer (slice 5), and signs it
//!    with the `dispatch-authority` key.
//!
//! The producers only load keys, because the key store needs transactions
//! of its own to create them: dispatch calls
//! [`authority_keys::ensure_role_keys`] before the launch transaction opens.
//! Only a `permit` dispatches. Approvals are slice 7.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, DurationRound, SecondsFormat, TimeDelta, Utc};
use coven_threads_core::automation_authority::{canonical_digest, verify_dispatch, Domain};
use rusqlite::Connection;
use serde_json::{json, Value};

use super::authority_keys::{self, AuthorityKeyRole, PRODUCER_COMPONENT, PRODUCER_INSTANCE_ID};
use super::contract::authority::{
    AuthorityEvidenceVerifier, AuthorityProfileError, AuthorityProfileErrorCode,
    AuthorityValidationPhase, AutomationAuthorityExtension, AUTHORITY_EXTENSION_KEY,
    AUTHORITY_PROFILE, BINDING_DOMAIN, RECEIPT_DOMAIN,
};
use super::contract::canonical_json::{canonicalize, sha256_hex};
use super::contract::types::ExtensionBag;
use super::definition::{RoutineAuthority, RoutineDefinition, RoutineRiskClass};
use super::ed25519_trust::authenticate_authority_extension;
use super::owner_grants::{self, OwnerAuthorization, OwnerAuthorizationRefusal};
use super::runner::{
    AutomationAuthorityRequest, AutomationDispatchAuthority, AutomationTerminalAuthorityRequest,
};
use super::runtime_envelope::{self, RuntimeEnvelope};
use super::threads_decisions::{self, Ed25519, ThreadsDecision};
use crate::familiar_issuer::{self, BindingPurpose, BindingRequest, IssueRefusal, TargetType};

/// How long an attempt's authorization stays valid.
const VALIDITY: TimeDelta = TimeDelta::minutes(5);
const POLICY_ID: &str = "coven-runtime-authority-policy:v1";
const MANIFEST_ID: &str = "coven-protected-surfaces:v1";
const PROFILE_VERSION: &str = "1.0.0";

/// The policy the daemon decides under: the #857 defaults, as decided on
/// 2026-10-04. Its digest is what decisions and bindings name.
fn policy_descriptor() -> Value {
    json!({
        "profile": "coven.runtime-authority.policy.v1",
        "unattendedRiskClasses": ["R0", "R1"],
        "recurringGrant": "owner-activation-of-the-current-revision",
        "importedRoutines": "per-run-approval",
        "launches": "only-what-a-runtime-envelope-enforces",
    })
}

/// The protected surfaces a v1 envelope may write: none. Every v1 envelope is
/// read-only.
fn manifest_descriptor() -> Value {
    json!({ "profile": "coven.protected-surfaces.v1", "writableSurfaces": [] })
}

fn descriptor_digest(descriptor: &Value) -> String {
    sha256_hex(&canonicalize(descriptor).expect("a constant descriptor is I-JSON"))
}

/// The adapter for one dispatch, deciding at `now` with `conn`, the
/// connection whose launch transaction is open.
pub(crate) struct TrustedAuthority<'a> {
    conn: &'a Connection,
    coven_home: &'a Path,
    now: DateTime<Utc>,
}

impl<'a> TrustedAuthority<'a> {
    pub(crate) fn new(conn: &'a Connection, coven_home: &'a Path, now: DateTime<Utc>) -> Self {
        let now = now
            .duration_trunc(TimeDelta::milliseconds(1))
            .unwrap_or(now);
        Self {
            conn,
            coven_home,
            now,
        }
    }

    fn compose(
        &self,
        request: &AutomationAuthorityRequest,
    ) -> Result<std::result::Result<Value, AuthorityProfileError>> {
        let refuse = |code, message: String| Ok(Err(AuthorityProfileError::new(code, message)));
        threads_decisions::ensure_threads_decisions_schema(self.conn)?;
        let Some(definition) = current_definition(self.conn, &request.automation_id)? else {
            return refuse(
                AuthorityProfileErrorCode::DefinitionMismatch,
                format!("no routine `{}`", request.automation_id),
            );
        };
        let Some(authority) = definition.authority.as_ref() else {
            return refuse(
                AuthorityProfileErrorCode::DefinitionMismatch,
                format!("routine `{}` declares no authority", definition.id),
            );
        };
        let Some(familiar_id) = definition.familiar_id.as_deref() else {
            return refuse(
                AuthorityProfileErrorCode::FamiliarMismatch,
                format!("routine `{}` names no familiar", definition.id),
            );
        };
        let Some(cwd) = definition.cwd.as_deref() else {
            return refuse(
                AuthorityProfileErrorCode::DefinitionMismatch,
                format!("routine `{}` has no working directory", definition.id),
            );
        };

        // 1. The owner grant, fresh for this attempt.
        let owner = match owner_grants::authorize_scheduled_dispatch(
            self.conn,
            &request.automation_id,
            request.automation_revision,
            self.now,
            VALIDITY,
        )? {
            Ok(owner) => owner,
            Err(OwnerAuthorizationRefusal::NoGrant { .. }) => {
                return refuse(
                    AuthorityProfileErrorCode::ApprovalRequired,
                    "no owner command activated this revision".to_owned(),
                )
            }
            Err(refusal) => {
                return refuse(
                    AuthorityProfileErrorCode::DefinitionMismatch,
                    format!("the revision is not current and active: {refusal:?}"),
                )
            }
        };

        // 2. The envelope that enforces the declared grant.
        let capabilities: Vec<&str> = authority.capabilities.iter().map(String::as_str).collect();
        let Some(envelope) = runtime_envelope::for_grant(&definition.runtime, &capabilities) else {
            return refuse(
                AuthorityProfileErrorCode::RuntimeDowngrade,
                format!(
                    "no runtime envelope enforces {capabilities:?} on `{}`",
                    definition.runtime
                ),
            );
        };

        // 3. The familiar embodiment binding. The contract's opaque ids carry a
        // namespace, which Coven's attempt ids do not.
        let target_id = format!("coven-attempt:{}", request.attempt_id);
        let issued = match familiar_issuer::issue_in(
            self.conn,
            self.coven_home,
            &BindingRequest {
                familiar_id,
                purpose: BindingPurpose::Dispatch,
                target_type: TargetType::AutomationAttempt,
                target_id: &target_id,
                principal_id: &owner.principal_id,
            },
            self.now,
        )? {
            Ok(issued) => issued,
            Err(IssueRefusal::NotRegistered) => {
                return refuse(
                    AuthorityProfileErrorCode::FamiliarMismatch,
                    format!("familiar `{familiar_id}` is not registered"),
                )
            }
            Err(IssueRefusal::NotActive { .. }) => {
                return refuse(
                    AuthorityProfileErrorCode::FamiliarStatusInvalid,
                    format!("familiar `{familiar_id}` has no active revision"),
                )
            }
            Err(IssueRefusal::DeclarationsChanged { .. }) => {
                return refuse(
                    AuthorityProfileErrorCode::FamiliarStale,
                    format!("familiar `{familiar_id}` has unadopted declaration changes"),
                )
            }
        };

        // 4. The Threads decision, its dispatch verification and consumption.
        let inputs = RequestInputs {
            request,
            authority,
            owner: &owner,
            envelope,
            familiar_root_id: issued.authority.familiar_root_id.as_str(),
            embodiment_digest: &format!("sha256:{}", issued.binding_digest),
            definition_digest: &format!("sha256:{}", request.definition_digest),
            project_id: &opaque_id("project", cwd),
            workspace_id: &opaque_id("workspace", cwd),
        };
        let draft = inputs.request_draft(&self.timestamp(owner.issued_at));
        let policy = inputs.policy_snapshot(self.conn, &self.timestamp(self.now))?;
        let decided = match threads_decisions::decide_in(
            self.conn,
            self.coven_home,
            &draft,
            &policy,
            self.now,
        )? {
            Ok(decided) => decided,
            Err(threads_decisions::DecisionRefusal::Replayed) => {
                return refuse(
                    AuthorityProfileErrorCode::Replayed,
                    "the attempt's request was already decided".to_owned(),
                )
            }
            Err(refusal) => {
                return refuse(
                    AuthorityProfileErrorCode::PolicyStale,
                    format!("the automation-authority profile refused the request: {refusal:?}"),
                )
            }
        };
        let Some(decision) = decided.binding()? else {
            return refuse(
                AuthorityProfileErrorCode::CapabilityEscalation,
                format!("the decision is `{}`", decided.outcome),
            );
        };
        if decided.outcome != "permit" {
            return refuse(
                AuthorityProfileErrorCode::ApprovalRequired,
                "the decision requires approval, which slice 7 brings".to_owned(),
            );
        }
        let snapshot = threads_decisions::consumption_snapshot_in(
            self.conn,
            self.coven_home,
            &decided,
            self.now,
        )?;
        let bundle = json!({
            "request": decided.request,
            "decision": decided.decision,
            "approval": null,
            "approval_authorization_request": null,
            "approval_authorization_decision": null,
            "lifecycle_events": [],
            "consumption_snapshot": snapshot,
            "snapshot": inputs.dispatch_snapshot(&decided, &snapshot),
        });
        let keyring = threads_decisions::keyring_at(self.conn, &owner.principal_id, self.now)?;
        if let Err(error) = verify_dispatch(&bundle, &keyring, &Ed25519) {
            return refuse(
                AuthorityProfileErrorCode::DispatchConsumptionMismatch,
                format!("the dispatch bundle failed verification: {error}"),
            );
        }
        if threads_decisions::consume_in(self.conn, &decided, self.now)?.is_err() {
            return refuse(
                AuthorityProfileErrorCode::Replayed,
                "the decision was already consumed".to_owned(),
            );
        }

        // 5. The execution binding.
        let snapshot_digest = canonical_digest(&snapshot, Domain::ConsumptionSnapshot.as_str())
            .context("a consumption snapshot has a digest")?;
        let mut binding = json!({
            "profile": AUTHORITY_PROFILE,
            "kind": "AutomationExecutionBinding",
            "bindingId": format!("binding:{}", request.attempt_id),
            "base": {
                "automationId": request.automation_id,
                "automationRevision": request.automation_revision,
                "definitionDigest": digest_value(&request.definition_digest),
                "occurrenceId": request.occurrence_id,
                "occurrenceKey": request.occurrence_key,
                "occurrenceFenceGeneration": request.occurrence_fence_generation,
                "runId": request.run_id,
                "attemptId": request.attempt_id,
                "attemptNumber": request.attempt_number,
                "adoptionKey": request.adoption_key,
            },
            "principal": {
                "principalId": owner.principal_id,
                "authorizationProofRef": inputs.proof_ref(),
                "authenticationState": "authenticated",
            },
            "authorization": {
                "operation": owner.operation,
                "requestId": decided.request["request_id"],
                "requestDigest": digest_value(unprefixed(&decided.request_digest)?),
                "decisionId": decided.decision["decision_id"],
                "decisionDigest": digest_value(unprefixed(&decided.decision_digest)?),
                "nonce": owner.nonce,
                "issuedAt": self.timestamp(owner.issued_at),
                "validFrom": self.timestamp(owner.issued_at),
                "validUntil": self.timestamp(owner.valid_until),
                "replayState": "fresh",
                "consumptionSnapshotId": snapshot["snapshot_id"],
                "consumptionSnapshotDigest": digest_value(&snapshot_digest),
                "consumptionStoreRevision": snapshot["store_revision"],
                "outcome": "permit",
            },
            "familiar": issued.authority,
            "contextProjection": {
                "projectId": inputs.project_id,
                "workspaceId": inputs.workspace_id,
                "contextProjectionIds": [],
                "memoryProjectionIds": [],
            },
            "threads": decision.threads,
            "capabilities": decision.capabilities,
            "approval": {
                "requirement": "not_required",
                "evidence": null,
                "scopeDigest": null,
                "expiresAt": null,
                "use": null,
                "consumption": { "state": "not_required" },
            },
            "risk": {
                "riskClass": decision.risk_class,
                "sideEffectClass": envelope_side_effect(envelope),
            },
            "runtime": {
                "runtimeId": definition.runtime,
                "descriptorVersion": envelope.descriptor_version,
                "descriptorDigest": digest_value(&envelope.descriptor_digest()),
                "capabilities": envelope.capabilities,
                "selectionRationale": "exact_requirement_match",
            },
            "versions": {
                "baseProfile": "coven.automations.v1",
                "authorityProfile": AUTHORITY_PROFILE,
                "familiarProfile": "familiar.embodiment_binding.v1",
                "threadsProfile": "automation-authority/1.0.0",
                "policyVersion": POLICY_ID,
                "policyDigest": digest_value(&descriptor_digest(&policy_descriptor())),
            },
            "decisionTimestamp": self.timestamp(self.now),
            "producer": producer(),
            "privacy": {
                "classification": "operational",
                "retention": "authority_evidence_90d",
                "redactionStatus": "not_required",
                "sensitiveMaterialIncluded": false,
            },
        });
        self.seal(&mut binding, BINDING_DOMAIN)?;
        Ok(Ok(binding))
    }

    /// Signs `value` in place with the `dispatch-authority` key under
    /// `domain`, as the contract's integrity rule reads it.
    fn seal(&self, value: &mut Value, domain: &[u8]) -> Result<()> {
        let key = authority_keys::existing_signing_key(
            self.conn,
            self.coven_home,
            AuthorityKeyRole::DispatchAuthority,
        )?
        .context("there is no dispatch-authority key; dispatch provisions it first")?;
        let object = value
            .as_object_mut()
            .context("an authority value is an object")?;
        object.remove("integrity");
        object.remove("authentication");
        let mut preimage = domain.to_vec();
        preimage.push(0);
        preimage.extend_from_slice(&canonicalize(&*value).context("authority value is I-JSON")?);
        let digest = sha256_hex(&preimage);
        let mut bytes = [0_u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&digest[index * 2..index * 2 + 2], 16)
                .context("a SHA-256 digest is hex")?;
        }
        value["integrity"] = digest_value(&digest);
        value["authentication"] = json!({
            "method": "ed25519",
            "keyId": key.record().key_id,
            "proofRef": key.record().proof_ref,
            "signedDigest": digest,
            "signature": key.sign_digest(&bytes),
        });
        Ok(())
    }

    fn timestamp(&self, value: DateTime<Utc>) -> String {
        value.to_rfc3339_opts(SecondsFormat::Millis, true)
    }

    fn extension_bag(binding: Value, receipt_evidence: Value) -> Result<ExtensionBag> {
        ExtensionBag::new(BTreeMap::from([(
            AUTHORITY_EXTENSION_KEY.to_owned(),
            json!({
                "profile": AUTHORITY_PROFILE,
                "kind": "AutomationAuthorityExtension",
                "executionBinding": binding,
                "receiptEvidence": receipt_evidence,
            }),
        )]))
        .map_err(|error| anyhow::anyhow!("authority extension is invalid: {error:?}"))
    }
}

impl AuthorityEvidenceVerifier for TrustedAuthority<'_> {
    /// The binding and any receipt evidence must be signed by a
    /// `dispatch-authority` key the daemon trusts at their decision time.
    fn verify(
        &self,
        extension: &AutomationAuthorityExtension,
        phase: AuthorityValidationPhase,
    ) -> std::result::Result<(), AuthorityProfileError> {
        let keys = authority_keys::trusted_keys(self.conn, AuthorityKeyRole::DispatchAuthority)
            .map_err(|error| {
                AuthorityProfileError::new(
                    AuthorityProfileErrorCode::TrustedStateUnavailable,
                    format!("the dispatch-authority keys are unreadable: {error:#}"),
                )
            })?;
        authenticate_authority_extension(&keys, extension, phase)
    }
}

impl AutomationDispatchAuthority for TrustedAuthority<'_> {
    fn resolve(
        &self,
        request: &AutomationAuthorityRequest,
    ) -> std::result::Result<ExtensionBag, AuthorityProfileError> {
        let binding = self.compose(request).map_err(unavailable)??;
        Self::extension_bag(binding, Value::Null).map_err(unavailable)
    }

    /// The receipt authority evidence for a terminal receipt: the binding's
    /// projection, correlated with the receipt and carrying the capabilities
    /// the receipt says were exercised.
    fn produce_terminal_evidence(
        &self,
        request: &AutomationTerminalAuthorityRequest<'_>,
    ) -> std::result::Result<ExtensionBag, AuthorityProfileError> {
        let binding = request.execution_binding;
        let receipt = request.receipt;
        // The receipt states what was exercised: what the terminal observer
        // saw for a launched run, and nothing for one that never launched.
        let exercised = receipt.exercised_capabilities.as_ref().ok_or_else(|| {
            AuthorityProfileError::new(
                AuthorityProfileErrorCode::ReceiptEvidenceRequired,
                "an authorized receipt states its exercised capabilities",
            )
        })?;
        let mut evidence = json!({
            "profile": AUTHORITY_PROFILE,
            "kind": "AutomationReceiptAuthorityEvidence",
            "receiptId": receipt.receipt_id,
            "automationId": binding.base.automation_id,
            "automationRevision": binding.base.automation_revision,
            "definitionDigest": binding.base.definition_digest,
            "occurrenceId": binding.base.occurrence_id,
            "occurrenceFenceGeneration": binding.base.occurrence_fence_generation,
            "runId": binding.base.run_id,
            "attemptId": binding.base.attempt_id,
            "attemptNumber": binding.base.attempt_number,
            // The receipt's digest, without the authentication it also carries.
            "baseReceiptDigest": {
                "algorithm": receipt.integrity.algorithm,
                "canonicalization": receipt.integrity.canonicalization,
                "value": receipt.integrity.value,
            },
            "bindingId": binding.binding_id,
            "bindingDigest": binding.integrity,
            "principalId": binding.principal.principal_id,
            "familiar": {
                "familiarRootId": binding.familiar.familiar_root_id,
                "identityRevisionId": binding.familiar.identity_revision_id,
                "declarationDigest": binding.familiar.declaration_digest,
                "statusAtDecision": binding.familiar.status_at_decision,
                "verifiedAt": binding.familiar.verified_at,
                "freshnessPolicyVersion": binding.familiar.freshness_policy_version,
                "freshnessBoundSeconds": binding.familiar.freshness_bound_seconds,
                "validTime": binding.familiar.valid_time,
                "revocation": binding.familiar.revocation,
                "retirement": binding.familiar.retirement,
            },
            "authorization": {
                "operation": binding.authorization.operation,
                "requestId": binding.authorization.request_id,
                "requestDigest": binding.authorization.request_digest,
                "decisionId": binding.authorization.decision_id,
                "decisionDigest": binding.authorization.decision_digest,
                "consumptionSnapshotDigest": binding.authorization.consumption_snapshot_digest,
                "outcome": binding.authorization.outcome,
            },
            "capabilities": {
                "requested": binding.capabilities.requested,
                "granted": binding.capabilities.granted,
                "denied": binding.capabilities.denied,
                "degraded": binding.capabilities.degraded,
                "exercised": exercised,
            },
            "approval": binding.approval,
            "risk": binding.risk,
            "runtime": {
                "runtimeId": binding.runtime.runtime_id,
                "descriptorVersion": binding.runtime.descriptor_version,
                "descriptorDigest": binding.runtime.descriptor_digest,
                "capabilities": binding.runtime.capabilities,
            },
            "decisionTimestamp": binding.decision_timestamp,
            "producer": binding.producer,
            "privacy": binding.privacy,
        });
        self.seal(&mut evidence, RECEIPT_DOMAIN)
            .map_err(unavailable)?;
        let binding = serde_json::to_value(binding).map_err(|error| unavailable(error.into()))?;
        Self::extension_bag(binding, evidence).map_err(unavailable)
    }
}

fn unavailable(error: anyhow::Error) -> AuthorityProfileError {
    AuthorityProfileError::new(
        AuthorityProfileErrorCode::TrustedStateUnavailable,
        format!("Runtime Authority could not be composed: {error:#}"),
    )
}

/// Everything one attempt's Threads request is built from.
struct RequestInputs<'a> {
    request: &'a AutomationAuthorityRequest,
    authority: &'a RoutineAuthority,
    owner: &'a OwnerAuthorization,
    envelope: &'static RuntimeEnvelope,
    familiar_root_id: &'a str,
    embodiment_digest: &'a str,
    definition_digest: &'a str,
    project_id: &'a str,
    workspace_id: &'a str,
}

impl RequestInputs<'_> {
    /// The owner grant that authorizes the request, as an opaque reference.
    fn proof_ref(&self) -> String {
        format!("owner-grant:{}", self.owner.grant.request_digest)
    }

    fn envelope_digest(&self) -> String {
        format!("sha256:{}", self.envelope.descriptor_digest())
    }

    /// The unsigned request: the daemon stamps the owner principal and signs
    /// it when it decides.
    fn request_draft(&self, issued_at: &str) -> Value {
        let request = self.request;
        json!({
            "schema_version": "opencoven.automation-authorization-request/v1",
            "request_id": format!("request:{}", request.attempt_id),
            "principal": {
                "id": self.owner.principal_id,
                "authorization_proof_ref": self.proof_ref(),
            },
            "replay": {
                "nonce": self.owner.nonce,
                "adoption_key": request.adoption_key,
                "issued_at": issued_at,
                "expires_at": self.owner.valid_until.to_rfc3339_opts(SecondsFormat::Millis, true),
            },
            "familiar": { "id": self.familiar_root_id, "embodiment_digest": self.embodiment_digest },
            "automation": {
                "id": request.automation_id,
                "definition_revision": request.automation_revision,
                "definition_digest": self.definition_digest,
            },
            "execution": {
                "occurrence_id": request.occurrence_id,
                "run_id": request.run_id,
                "attempt": request.attempt_number,
                "fence_generation": request.occurrence_fence_generation,
            },
            // The definition is the action: its digest covers the prompt.
            "action": {
                "type": self.authority.action_type,
                "digest": self.definition_digest,
                "risk_class": self.authority.risk_class,
                "proposal_safe": self.authority.proposal_safe,
            },
            "requested_capabilities": self.authority.capabilities,
            "scopes": self.authority.scopes,
            "context": {
                "project_id": self.project_id,
                "workspace_id": self.workspace_id,
                "runtime": {
                    "id": self.request.runtime_id,
                    "descriptor_digest": self.envelope_digest(),
                    "capabilities": self.envelope.capabilities,
                },
            },
            "versions": {
                "profile": PROFILE_VERSION,
                "policy": POLICY_ID,
                "policy_digest": format!("sha256:{}", descriptor_digest(&policy_descriptor())),
                "manifest": MANIFEST_ID,
                "manifest_digest": format!("sha256:{}", descriptor_digest(&manifest_descriptor())),
            },
            "previous_approval_digest": null,
            // Imported routines never declare authority, so none is imported.
            "conditions": [],
            "data": { "sensitivity": "internal", "retention": "authority_evidence_90d" },
        })
    }

    /// The policy snapshot. An R0 or R1 routine's recurring grant is the
    /// owner activation of this revision. It lasts as long as the revision is
    /// current, so for this attempt it expires with the attempt's window and
    /// covers exactly this use.
    fn policy_snapshot(&self, conn: &Connection, now: &str) -> Result<Value> {
        let mut grants = Vec::new();
        if matches!(
            self.authority.risk_class,
            RoutineRiskClass::R0 | RoutineRiskClass::R1
        ) {
            let uses: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM automation_threads_store_events AS event
                     JOIN automation_threads_decisions AS decision
                       ON decision.decision_id = event.decision_id
                     WHERE event.kind = 'consumption'
                       AND json_extract(decision.request_json, '$.automation.id') = ?1
                       AND json_extract(decision.request_json, '$.automation.definition_revision') = ?2",
                    rusqlite::params![
                        self.request.automation_id,
                        i64::try_from(self.request.automation_revision)
                            .context("revision exceeds SQLite range")?
                    ],
                    |row| row.get(0),
                )
                .context("failed to count the grant's uses")?;
            grants.push(json!({
                "grant_id": self.proof_ref(),
                "principal_id": self.owner.principal_id,
                "familiar_id": self.familiar_root_id,
                "familiar_embodiment_digest": self.embodiment_digest,
                "automation_id": self.request.automation_id,
                "definition_revision": self.request.automation_revision,
                "definition_digest": self.definition_digest,
                "action_type": self.authority.action_type,
                "action_digest": self.definition_digest,
                "project_id": self.project_id,
                "workspace_id": self.workspace_id,
                "runtime_id": self.request.runtime_id,
                "runtime_descriptor_digest": self.envelope_digest(),
                "runtime_capabilities": self.envelope.capabilities,
                "risk_classes": [self.authority.risk_class],
                "capabilities": self.authority.capabilities,
                "scopes": self.authority.scopes,
                "expires_at": self.owner.valid_until.to_rfc3339_opts(SecondsFormat::Millis, true),
                "max_uses": uses + 1,
                "uses": uses,
            }));
        }
        Ok(json!({
            "now": now,
            "policy": POLICY_ID,
            "policy_digest": format!("sha256:{}", descriptor_digest(&policy_descriptor())),
            "manifest": MANIFEST_ID,
            "manifest_digest": format!("sha256:{}", descriptor_digest(&manifest_descriptor())),
            "recurring_grants": grants,
            "protected_owner_approval": false,
            "recurring_approval_allowed": false,
        }))
    }

    /// The trusted dispatch-time state the bundle is checked against.
    fn dispatch_snapshot(&self, decided: &ThreadsDecision, consumption: &Value) -> Value {
        let request = self.request;
        json!({
            "now": decided.policy["now"],
            "principal_id": self.owner.principal_id,
            "familiar_id": self.familiar_root_id,
            "familiar_embodiment_digest": self.embodiment_digest,
            "automation_id": request.automation_id,
            "definition_revision": request.automation_revision,
            "definition_digest": self.definition_digest,
            "occurrence_id": request.occurrence_id,
            "run_id": request.run_id,
            "attempt": request.attempt_number,
            "fence_generation": request.occurrence_fence_generation,
            "action_digest": self.definition_digest,
            "runtime_id": request.runtime_id,
            "runtime_descriptor_digest": self.envelope_digest(),
            "runtime_capabilities": self.envelope.capabilities,
            "project_id": self.project_id,
            "workspace_id": self.workspace_id,
            "policy": POLICY_ID,
            "policy_digest": decided.policy["policy_digest"],
            "manifest": MANIFEST_ID,
            "manifest_digest": decided.policy["manifest_digest"],
            "consumption_revision": consumption["store_revision"],
            "policy_snapshot": decided.policy,
            "approval_authorization_policy_snapshot": null,
        })
    }
}

fn current_definition(conn: &Connection, automation_id: &str) -> Result<Option<RoutineDefinition>> {
    use rusqlite::OptionalExtension;
    let json: Option<String> = conn
        .query_row(
            "SELECT definition_json FROM automation_definitions
             WHERE id = ?1 AND tombstoned_at IS NULL",
            [automation_id],
            |row| row.get(0),
        )
        .optional()
        .context("failed to read the routine")?;
    json.map(|json| serde_json::from_str(&json).context("a stored routine is valid"))
        .transpose()
}

/// A stable opaque identifier for a working directory.
fn opaque_id(kind: &str, cwd: &str) -> String {
    format!("{kind}:{}", &sha256_hex(cwd.as_bytes())[..32])
}

fn envelope_side_effect(envelope: &RuntimeEnvelope) -> Value {
    envelope
        .tools
        .iter()
        .map(|tool| json!(tool.side_effect))
        .max_by_key(|class| side_effect_rank(class.as_str().unwrap_or_default()))
        .unwrap_or_else(|| json!("none"))
}

fn side_effect_rank(class: &str) -> u8 {
    match class {
        "local_read" => 1,
        "local_write" => 2,
        "external_read" => 3,
        "external_mutation" => 4,
        "irreversible_external_mutation" => 5,
        _ => 0,
    }
}

fn producer() -> Value {
    json!({
        "component": PRODUCER_COMPONENT,
        "instanceId": PRODUCER_INSTANCE_ID,
        "implementationVersion": env!("CARGO_PKG_VERSION"),
    })
}

fn digest_value(hex: &str) -> Value {
    json!({ "algorithm": "sha256", "canonicalization": "jcs-rfc8785", "value": hex })
}

fn unprefixed(digest: &str) -> Result<&str> {
    digest
        .strip_prefix("sha256:")
        .context("a profile digest is sha256-prefixed")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::NoopSessionRuntime;
    use crate::automations::contract::authority::{
        validate_authority_profile, AuthorityConsumerClass, AuthorityProfileDisposition,
        RUNTIME_AUTHORITY_CAPABILITY,
    };
    use crate::automations::owner_grants::CommandAuthority;
    use crate::control_plane::route_action_with_authority;
    use crate::familiar_ledger::tests::Home;

    fn r0() -> Value {
        json!({
            "actionType": "analysis.read", "riskClass": "R0",
            "capabilities": ["analysis.read"],
            "scopes": [{ "kind": "filesystem", "root": "workspace", "path": "notes/today.md",
                         "access": "read", "recursive": false }],
            "proposalSafe": true
        })
    }

    /// Creates routine `id` declaring `authority`, as the owner, with `status`.
    fn create(home: &Home, id: &str, authority: Value, status: &str) {
        let routine = json!({
            "schemaVersion": 1, "id": id, "name": id, "status": status,
            "rrule": "FREQ=DAILY;BYHOUR=9", "timezone": "utc", "misfire": "latest",
            "overlap": "forbid", "timeoutMinutes": 30, "runtime": "claude",
            "familiarId": "sage", "cwd": home.path().display().to_string(),
            "prompt": "Summarise the notes.", "authority": authority
        });
        let (status, response) = route_action_with_authority(
            json!({ "action": "coven.automations.definition.create.v1",
                    "adoptionKey": format!("adopt:{id}:create"), "definition": routine }),
            &home.conn,
            &NoopSessionRuntime,
            CommandAuthority::OwnerLocal,
        );
        assert!(status == 200 && response.accepted, "{response:?}");
    }

    /// A home with `sage` registered and an active R0 routine `notes` the
    /// owner created, so its creation is the grant.
    fn home() -> Home {
        let home = Home::new();
        home.register("adopt:ledger:register");
        create(&home, "notes", r0(), "ACTIVE");
        authority_keys::ensure_role_keys(&home.conn, home.path(), Utc::now()).unwrap();
        home
    }

    fn request(home: &Home, attempt: &str) -> AutomationAuthorityRequest {
        request_for(home, "notes", attempt)
    }

    fn request_for(home: &Home, id: &str, attempt: &str) -> AutomationAuthorityRequest {
        let (digest, revision): (String, i64) = home
            .conn
            .query_row(
                "SELECT definition_digest, revision FROM automation_definitions WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        AutomationAuthorityRequest {
            automation_id: id.to_owned(),
            automation_revision: u64::try_from(revision).unwrap(),
            definition_digest: digest,
            occurrence_id: format!("occurrence.notes-{attempt}"),
            occurrence_key: "notes@2026-10-04T09:00:00Z".to_owned(),
            occurrence_fence_generation: 1,
            run_id: format!("run.notes-{attempt}"),
            attempt_id: format!("attempt-run.notes-{attempt}-1"),
            attempt_number: 1,
            adoption_key: format!("automation:run.notes-{attempt}:1"),
            runtime_id: "claude".to_owned(),
        }
    }

    fn validated(
        adapter: &TrustedAuthority<'_>,
        extensions: &ExtensionBag,
    ) -> std::result::Result<Value, AuthorityProfileError> {
        let disposition = validate_authority_profile(
            extensions,
            AuthorityConsumerClass::RuntimeAuthorityV1,
            &["coven.automations.v1", AUTHORITY_PROFILE],
            &[RUNTIME_AUTHORITY_CAPABILITY],
            AuthorityValidationPhase::PreDispatch,
            Some(adapter),
        )?;
        let AuthorityProfileDisposition::Validated(extension) = disposition else {
            panic!("not validated");
        };
        Ok(serde_json::to_value(&extension.execution_binding).unwrap())
    }

    #[test]
    fn an_r0_routine_resolves_to_a_binding_the_contract_accepts() {
        let home = home();
        let transaction = home.conn.unchecked_transaction().unwrap();
        let adapter = TrustedAuthority::new(&transaction, home.path(), Utc::now());
        let extensions = adapter.resolve(&request(&home, "a")).unwrap();
        let binding = validated(&adapter, &extensions).unwrap();
        assert_eq!(binding["authorization"]["outcome"], json!("permit"));
        assert_eq!(binding["capabilities"]["granted"], json!(["analysis.read"]));
        assert_eq!(
            binding["risk"],
            json!({ "riskClass": "R0", "sideEffectClass": "local_read" })
        );
        assert_eq!(
            binding["runtime"]["descriptorDigest"]["value"],
            json!(runtime_envelope::CLAUDE_R0_READ.descriptor_digest())
        );
        assert_eq!(binding["approval"]["requirement"], json!("not_required"));
        transaction.commit().unwrap();
    }

    fn refusal(home: &Home, request: &AutomationAuthorityRequest) -> AuthorityProfileErrorCode {
        let transaction = home.conn.unchecked_transaction().unwrap();
        let adapter = TrustedAuthority::new(&transaction, home.path(), Utc::now());
        adapter.resolve(request).unwrap_err().code()
    }

    #[test]
    fn an_attempt_is_authorized_once() {
        let home = home();
        let transaction = home.conn.unchecked_transaction().unwrap();
        let adapter = TrustedAuthority::new(&transaction, home.path(), Utc::now());
        adapter.resolve(&request(&home, "once")).unwrap();
        assert_eq!(
            adapter.resolve(&request(&home, "once")).unwrap_err().code(),
            AuthorityProfileErrorCode::Replayed
        );
    }

    #[test]
    fn only_a_grant_an_envelope_enforces_and_the_profile_permits_dispatches() {
        let home = home();
        // R1 writes: no v1 envelope enforces them.
        let mut writes = r0();
        writes["riskClass"] = json!("R1");
        writes["actionType"] = json!("artifact.create");
        writes["capabilities"] = json!(["artifact.write"]);
        writes["scopes"][0]["access"] = json!("write");
        create(&home, "writes", writes, "ACTIVE");
        assert_eq!(
            refusal(&home, &request_for(&home, "writes", "w")),
            AuthorityProfileErrorCode::RuntimeDowngrade
        );
        // The envelope fits these reads, but the profile decides otherwise:
        // R2 needs safeguards a routine cannot yet declare; R3 needs a per-run
        // approval (slice 7); a proposal-safe R3 only degrades to a proposal.
        for (id, risk, proposal_safe, refused) in [
            ("r2", "R2", true, AuthorityProfileErrorCode::PolicyStale),
            (
                "r3",
                "R3",
                false,
                AuthorityProfileErrorCode::ApprovalRequired,
            ),
            (
                "r3-proposal",
                "R3",
                true,
                AuthorityProfileErrorCode::CapabilityEscalation,
            ),
        ] {
            let mut declared = r0();
            declared["riskClass"] = json!(risk);
            declared["proposalSafe"] = json!(proposal_safe);
            create(&home, id, declared, "ACTIVE");
            assert_eq!(refusal(&home, &request_for(&home, id, id)), refused, "{id}");
        }
        // A paused routine has no current activation.
        create(&home, "paused", r0(), "PAUSED");
        assert_eq!(
            refusal(&home, &request_for(&home, "paused", "p")),
            AuthorityProfileErrorCode::DefinitionMismatch
        );
    }

    #[test]
    fn a_familiar_with_unadopted_changes_does_not_dispatch() {
        let home = home();
        home.write("sage", "SOUL.md", "# SOUL\nChanged out of band.\n");
        assert_eq!(
            refusal(&home, &request(&home, "stale")),
            AuthorityProfileErrorCode::FamiliarStale
        );
    }
}
