//! Authenticated terminal observations from an authority-aware runtime.
//!
//! This contract is a receiving boundary only. No production runtime emits it
//! and no default verifier accepts it.

use std::collections::BTreeSet;
use std::fmt;

use serde::ser::Serializer;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::authority::{
    AuthorityCapability, AuthorityOpaqueIdentifier, AuthoritySideEffectClass,
    AutomationExecutionBinding,
};
use super::canonical_json::{canonicalize, sha256_hex};
use super::types::{
    AttemptId, DigestValue, ProducerIdentity, ReceiptPrivacy, RunId, SessionId, SideEffectClass,
    TerminalOutcome,
};

pub const RUNTIME_TERMINAL_EVIDENCE_PROFILE: &str =
    "coven.automations.runtime-terminal-evidence.v1";
pub const RUNTIME_TERMINAL_EVIDENCE_AUTHENTICATION_DOMAIN: &[u8] =
    b"opencoven:coven-automations-runtime-terminal-evidence:v1";

macro_rules! validated_text {
    ($name:ident, $validator:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: String) -> Result<Self, RuntimeTerminalEvidenceError> {
                if $validator(&value) {
                    Ok(Self(value))
                } else {
                    Err(RuntimeTerminalEvidenceError::new(
                        RuntimeTerminalEvidenceErrorCode::SchemaInvalid,
                    ))
                }
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

fn matches_opaque_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 256
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'@' | b'-')
        })
}

fn matches_reason_code(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 96
        && bytes[0].is_ascii_lowercase()
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn matches_signature(value: &str) -> bool {
    value.len() == 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

validated_text!(RuntimeTerminalEvidenceId, matches_opaque_identifier);
validated_text!(RuntimeTerminalEvidenceReasonCode, matches_reason_code);
validated_text!(RuntimeTerminalEvidenceKeyId, matches_opaque_identifier);
validated_text!(RuntimeTerminalEvidenceProofRef, matches_opaque_identifier);
validated_text!(RuntimeTerminalEvidenceSignature, matches_signature);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeTerminalEvidenceProfile {
    #[serde(rename = "coven.automations.runtime-terminal-evidence.v1")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeTerminalEvidenceBinding {
    pub binding_id: AuthorityOpaqueIdentifier,
    pub binding_digest: DigestValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeTerminalEvidenceRuntime {
    pub runtime_id: super::types::RuntimeId,
    pub descriptor_digest: DigestValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationCoverage {
    Complete,
    Partial,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeSideEffects {
    Observed {
        maximum_class: SideEffectClass,
        coverage: ObservationCoverage,
    },
    Unknown {
        reason_code: RuntimeTerminalEvidenceReasonCode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeEvidenceCapabilities(Vec<AuthorityCapability>);

impl RuntimeEvidenceCapabilities {
    #[must_use]
    pub fn as_slice(&self) -> &[AuthorityCapability] {
        &self.0
    }
}

impl Serialize for RuntimeEvidenceCapabilities {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for RuntimeEvidenceCapabilities {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = Vec::<AuthorityCapability>::deserialize(deserializer)?;
        let unique = values.iter().collect::<BTreeSet<_>>().len() == values.len();
        if values.len() <= 128 && unique {
            Ok(Self(values))
        } else {
            Err(serde::de::Error::custom(
                "runtime evidence capabilities must be unique and contain at most 128 entries",
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeExercisedCapabilities {
    Observed {
        values: RuntimeEvidenceCapabilities,
        coverage: ObservationCoverage,
    },
    Unknown {
        reason_code: RuntimeTerminalEvidenceReasonCode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeResultEvidence {
    Produced {
        #[serde(
            default,
            deserialize_with = "super::types::deserialize_non_null_option",
            skip_serializing_if = "Option::is_none"
        )]
        digest: Option<DigestValue>,
    },
    NotProduced,
    Unknown {
        reason_code: RuntimeTerminalEvidenceReasonCode,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum RuntimeDeliveryEvidence {
    NotAttempted,
    Committed {
        #[serde(
            default,
            deserialize_with = "super::types::deserialize_non_null_option",
            skip_serializing_if = "Option::is_none"
        )]
        digest: Option<DigestValue>,
    },
    Failed {
        #[serde(
            default,
            deserialize_with = "super::types::deserialize_non_null_option",
            skip_serializing_if = "Option::is_none"
        )]
        digest: Option<DigestValue>,
    },
    Partial {
        #[serde(
            default,
            deserialize_with = "super::types::deserialize_non_null_option",
            skip_serializing_if = "Option::is_none"
        )]
        digest: Option<DigestValue>,
    },
    Unknown {
        reason_code: RuntimeTerminalEvidenceReasonCode,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeTerminalEvidenceAuthenticationMethod {
    #[serde(rename = "ed25519")]
    Ed25519,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeTerminalEvidenceAuthentication {
    pub method: RuntimeTerminalEvidenceAuthenticationMethod,
    pub key_id: RuntimeTerminalEvidenceKeyId,
    pub proof_ref: RuntimeTerminalEvidenceProofRef,
    pub signed_digest: DigestValue,
    pub signature: RuntimeTerminalEvidenceSignature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeTerminalEvidence {
    pub profile: RuntimeTerminalEvidenceProfile,
    pub evidence_id: RuntimeTerminalEvidenceId,
    pub session_id: SessionId,
    pub run_id: RunId,
    pub attempt_id: AttemptId,
    pub binding: RuntimeTerminalEvidenceBinding,
    pub runtime: RuntimeTerminalEvidenceRuntime,
    pub produced_at: super::types::Timestamp,
    pub disposition: TerminalOutcome,
    pub side_effects: RuntimeSideEffects,
    pub exercised_capabilities: RuntimeExercisedCapabilities,
    pub result: RuntimeResultEvidence,
    pub delivery: RuntimeDeliveryEvidence,
    pub producer: ProducerIdentity,
    pub privacy: ReceiptPrivacy,
    pub integrity: DigestValue,
    pub authentication: RuntimeTerminalEvidenceAuthentication,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTerminalEvidenceClassification {
    ReceiptEligibleComplete,
    AuthenticatedPartialOrAmbiguous,
    AuthenticatedUnknown,
    PolicyViolating,
}

impl RuntimeTerminalEvidenceClassification {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReceiptEligibleComplete => "receipt_eligible_complete",
            Self::AuthenticatedPartialOrAmbiguous => "authenticated_partial_or_ambiguous",
            Self::AuthenticatedUnknown => "authenticated_unknown",
            Self::PolicyViolating => "policy_violating",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedRuntimeTerminalEvidence {
    pub evidence: RuntimeTerminalEvidence,
    pub classification: RuntimeTerminalEvidenceClassification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeTerminalEvidenceErrorCode {
    SchemaInvalid,
    IjsonInvalid,
    IntegrityInvalid,
    AuthenticationInvalid,
    AuthenticationStale,
    AuthenticationUnverifiable,
    CorrelationMismatch,
    AuthorityProfileRequired,
    Conflict,
    StoredEvidenceInvalid,
    StoreUnavailable,
}

impl RuntimeTerminalEvidenceErrorCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SchemaInvalid => "RUNTIME_TERMINAL_EVIDENCE_SCHEMA_INVALID",
            Self::IjsonInvalid => "RUNTIME_TERMINAL_EVIDENCE_IJSON_INVALID",
            Self::IntegrityInvalid => "RUNTIME_TERMINAL_EVIDENCE_INTEGRITY_INVALID",
            Self::AuthenticationInvalid => "RUNTIME_TERMINAL_EVIDENCE_AUTHENTICATION_INVALID",
            Self::AuthenticationStale => "RUNTIME_TERMINAL_EVIDENCE_AUTHENTICATION_STALE",
            Self::AuthenticationUnverifiable => {
                "RUNTIME_TERMINAL_EVIDENCE_AUTHENTICATION_UNVERIFIABLE"
            }
            Self::CorrelationMismatch => "RUNTIME_TERMINAL_EVIDENCE_CORRELATION_MISMATCH",
            Self::AuthorityProfileRequired => {
                "RUNTIME_TERMINAL_EVIDENCE_AUTHORITY_PROFILE_REQUIRED"
            }
            Self::Conflict => "RUNTIME_TERMINAL_EVIDENCE_CONFLICT",
            Self::StoredEvidenceInvalid => "RUNTIME_TERMINAL_EVIDENCE_STORED_INVALID",
            Self::StoreUnavailable => "RUNTIME_TERMINAL_EVIDENCE_STORE_UNAVAILABLE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTerminalEvidenceError {
    code: RuntimeTerminalEvidenceErrorCode,
}

impl RuntimeTerminalEvidenceError {
    #[must_use]
    pub const fn new(code: RuntimeTerminalEvidenceErrorCode) -> Self {
        Self { code }
    }

    #[must_use]
    pub const fn code(&self) -> RuntimeTerminalEvidenceErrorCode {
        self.code
    }
}

impl fmt::Display for RuntimeTerminalEvidenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.code.as_str())
    }
}

impl std::error::Error for RuntimeTerminalEvidenceError {}

pub trait RuntimeTerminalEvidenceVerifier {
    fn verify(
        &self,
        evidence: &RuntimeTerminalEvidence,
    ) -> Result<(), RuntimeTerminalEvidenceError>;
}

pub fn verify_runtime_terminal_evidence(
    evidence: RuntimeTerminalEvidence,
    binding: &AutomationExecutionBinding,
    verifier: &dyn RuntimeTerminalEvidenceVerifier,
) -> Result<VerifiedRuntimeTerminalEvidence, RuntimeTerminalEvidenceError> {
    validate_integrity(&evidence)?;
    validate_binding_correlation(&evidence, binding)?;
    verifier.verify(&evidence)?;
    let classification = classify_runtime_terminal_evidence(&evidence, binding);
    Ok(VerifiedRuntimeTerminalEvidence {
        evidence,
        classification,
    })
}

fn validate_integrity(
    evidence: &RuntimeTerminalEvidence,
) -> Result<(), RuntimeTerminalEvidenceError> {
    let mut body = serde_json::to_value(evidence).map_err(|_| {
        RuntimeTerminalEvidenceError::new(RuntimeTerminalEvidenceErrorCode::SchemaInvalid)
    })?;
    let Value::Object(object) = &mut body else {
        return Err(RuntimeTerminalEvidenceError::new(
            RuntimeTerminalEvidenceErrorCode::SchemaInvalid,
        ));
    };
    object.remove("integrity");
    object.remove("authentication");
    let canonical = canonicalize(&body).map_err(|_| {
        RuntimeTerminalEvidenceError::new(RuntimeTerminalEvidenceErrorCode::IjsonInvalid)
    })?;
    if evidence.integrity.value.as_str() != sha256_hex(&canonical) {
        return Err(RuntimeTerminalEvidenceError::new(
            RuntimeTerminalEvidenceErrorCode::IntegrityInvalid,
        ));
    }
    let mut authentication_preimage = Vec::with_capacity(
        RUNTIME_TERMINAL_EVIDENCE_AUTHENTICATION_DOMAIN.len() + canonical.len() + 1,
    );
    authentication_preimage.extend_from_slice(RUNTIME_TERMINAL_EVIDENCE_AUTHENTICATION_DOMAIN);
    authentication_preimage.push(0);
    authentication_preimage.extend_from_slice(&canonical);
    if evidence.authentication.signed_digest.value.as_str() != sha256_hex(&authentication_preimage)
    {
        return Err(RuntimeTerminalEvidenceError::new(
            RuntimeTerminalEvidenceErrorCode::AuthenticationInvalid,
        ));
    }
    Ok(())
}

fn validate_binding_correlation(
    evidence: &RuntimeTerminalEvidence,
    binding: &AutomationExecutionBinding,
) -> Result<(), RuntimeTerminalEvidenceError> {
    if evidence.run_id != binding.base.run_id
        || evidence.attempt_id != binding.base.attempt_id
        || evidence.binding.binding_id != binding.binding_id
        || evidence.binding.binding_digest != binding.integrity
        || evidence.runtime.runtime_id != binding.runtime.runtime_id
        || evidence.runtime.descriptor_digest != binding.runtime.descriptor_digest
    {
        return Err(RuntimeTerminalEvidenceError::new(
            RuntimeTerminalEvidenceErrorCode::CorrelationMismatch,
        ));
    }
    Ok(())
}

pub(in crate::automations) fn classify_runtime_terminal_evidence(
    evidence: &RuntimeTerminalEvidence,
    binding: &AutomationExecutionBinding,
) -> RuntimeTerminalEvidenceClassification {
    if policy_violating(evidence, binding) {
        return RuntimeTerminalEvidenceClassification::PolicyViolating;
    }
    if has_unknown_observation(evidence) {
        return RuntimeTerminalEvidenceClassification::AuthenticatedUnknown;
    }
    if complete(evidence) {
        RuntimeTerminalEvidenceClassification::ReceiptEligibleComplete
    } else {
        RuntimeTerminalEvidenceClassification::AuthenticatedPartialOrAmbiguous
    }
}

fn has_unknown_observation(evidence: &RuntimeTerminalEvidence) -> bool {
    matches!(evidence.side_effects, RuntimeSideEffects::Unknown { .. })
        || matches!(
            evidence.exercised_capabilities,
            RuntimeExercisedCapabilities::Unknown { .. }
        )
        || matches!(evidence.result, RuntimeResultEvidence::Unknown { .. })
        || matches!(evidence.delivery, RuntimeDeliveryEvidence::Unknown { .. })
}

fn policy_violating(
    evidence: &RuntimeTerminalEvidence,
    binding: &AutomationExecutionBinding,
) -> bool {
    let side_effect_escalation = match evidence.side_effects {
        RuntimeSideEffects::Observed { maximum_class, .. } => {
            side_effect_rank(maximum_class)
                > authority_side_effect_rank(binding.risk.side_effect_class)
        }
        RuntimeSideEffects::Unknown { .. } => false,
    };
    let capability_escalation = match &evidence.exercised_capabilities {
        RuntimeExercisedCapabilities::Observed { values, .. } => values
            .as_slice()
            .iter()
            .any(|capability| !binding.capabilities.granted.as_slice().contains(capability)),
        RuntimeExercisedCapabilities::Unknown { .. } => false,
    };
    side_effect_escalation || capability_escalation
}

fn complete(evidence: &RuntimeTerminalEvidence) -> bool {
    !matches!(evidence.disposition, TerminalOutcome::Ambiguous)
        && matches!(
            evidence.side_effects,
            RuntimeSideEffects::Observed {
                coverage: ObservationCoverage::Complete,
                ..
            }
        )
        && matches!(
            evidence.exercised_capabilities,
            RuntimeExercisedCapabilities::Observed {
                coverage: ObservationCoverage::Complete,
                ..
            }
        )
        && !matches!(evidence.result, RuntimeResultEvidence::Unknown { .. })
        && !matches!(
            evidence.delivery,
            RuntimeDeliveryEvidence::Partial { .. } | RuntimeDeliveryEvidence::Unknown { .. }
        )
}

const fn side_effect_rank(class: SideEffectClass) -> u8 {
    match class {
        SideEffectClass::None => 0,
        SideEffectClass::LocalRead => 1,
        SideEffectClass::LocalWrite => 2,
        SideEffectClass::ExternalRead => 3,
        SideEffectClass::ExternalMutation => 4,
        SideEffectClass::IrreversibleExternalMutation => 5,
    }
}

const fn authority_side_effect_rank(class: AuthoritySideEffectClass) -> u8 {
    match class {
        AuthoritySideEffectClass::None => 0,
        AuthoritySideEffectClass::LocalRead => 1,
        AuthoritySideEffectClass::LocalWrite => 2,
        AuthoritySideEffectClass::ExternalRead => 3,
        AuthoritySideEffectClass::ExternalMutation => 4,
        AuthoritySideEffectClass::IrreversibleExternalMutation => 5,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{
        verify_runtime_terminal_evidence, RuntimeTerminalEvidence,
        RuntimeTerminalEvidenceClassification, RuntimeTerminalEvidenceError,
        RuntimeTerminalEvidenceErrorCode, RuntimeTerminalEvidenceVerifier,
        RUNTIME_TERMINAL_EVIDENCE_AUTHENTICATION_DOMAIN, RUNTIME_TERMINAL_EVIDENCE_PROFILE,
    };
    use crate::automations::contract::authority::AutomationExecutionBinding;
    use crate::automations::contract::canonical_json::{canonicalize, sha256_hex};

    const AUTHORITY_VECTORS: &str =
        include_str!("../../../../../spec/coven-automations/authority/v1/test-vectors.json");

    struct FixtureVerifier {
        key_id: &'static str,
        stale: bool,
    }

    impl RuntimeTerminalEvidenceVerifier for FixtureVerifier {
        fn verify(
            &self,
            evidence: &RuntimeTerminalEvidence,
        ) -> Result<(), RuntimeTerminalEvidenceError> {
            if self.stale {
                return Err(RuntimeTerminalEvidenceError::new(
                    RuntimeTerminalEvidenceErrorCode::AuthenticationStale,
                ));
            }
            if evidence.authentication.key_id.as_str() != self.key_id
                || evidence.authentication.signature.as_str()
                    != format!(
                        "{}{}",
                        evidence.authentication.signed_digest.value.as_str(),
                        evidence.authentication.signed_digest.value.as_str()
                    )
            {
                return Err(RuntimeTerminalEvidenceError::new(
                    RuntimeTerminalEvidenceErrorCode::AuthenticationInvalid,
                ));
            }
            Ok(())
        }
    }

    fn binding() -> AutomationExecutionBinding {
        let vectors: Value = serde_json::from_str(AUTHORITY_VECTORS).unwrap();
        serde_json::from_value(vectors["fixtures"]["binding"].clone()).unwrap()
    }

    fn digest(value: &str) -> Value {
        json!({
            "algorithm": "sha256",
            "canonicalization": "jcs-rfc8785",
            "value": value
        })
    }

    fn evidence_value() -> Value {
        json!({
            "profile": RUNTIME_TERMINAL_EVIDENCE_PROFILE,
            "evidenceId": "evidence:daily-notes-1",
            "sessionId": "session-daily-notes-1",
            "runId": "run.daily-notes-1",
            "attemptId": "attempt.daily-notes-1-1",
            "binding": {
                "bindingId": "binding:daily-notes-1",
                "bindingDigest": digest(
                    "a952367c28a7f29ccc69904c72dcc0f8296dfdf61a3141fb78bcba43f5c4c5c7"
                )
            },
            "runtime": {
                "runtimeId": "runtime:coven-code",
                "descriptorDigest": digest(
                    "7777777777777777777777777777777777777777777777777777777777777777"
                )
            },
            "producedAt": "2026-09-03T12:30:00.000Z",
            "disposition": "succeeded",
            "sideEffects": {
                "state": "observed",
                "maximumClass": "local_write",
                "coverage": "complete"
            },
            "exercisedCapabilities": {
                "state": "observed",
                "values": ["analysis.read", "artifact.write"],
                "coverage": "complete"
            },
            "result": {
                "state": "produced",
                "digest": digest(
                    "8888888888888888888888888888888888888888888888888888888888888888"
                )
            },
            "delivery": {
                "state": "not_attempted"
            },
            "producer": {
                "component": "runtime-adapter",
                "instanceId": "runtime-instance-1",
                "implementationVersion": "1.0.0"
            },
            "privacy": {
                "classification": "operational",
                "retention": {
                    "classification": "standard"
                }
            },
            "integrity": digest(
                "0000000000000000000000000000000000000000000000000000000000000000"
            ),
            "authentication": {
                "method": "ed25519",
                "keyId": "key:runtime-instance-1",
                "proofRef": "proof:runtime-instance-1",
                "signedDigest": digest(
                    "0000000000000000000000000000000000000000000000000000000000000000"
                ),
                "signature": "0".repeat(128)
            }
        })
    }

    fn seal(mut value: Value) -> Value {
        let object = value.as_object_mut().unwrap();
        object.remove("integrity");
        object.remove("authentication");
        let canonical = canonicalize(&value).unwrap();
        let integrity = sha256_hex(&canonical);
        let mut authentication_preimage = Vec::new();
        authentication_preimage.extend_from_slice(RUNTIME_TERMINAL_EVIDENCE_AUTHENTICATION_DOMAIN);
        authentication_preimage.push(0);
        authentication_preimage.extend_from_slice(&canonical);
        let signed_digest = sha256_hex(&authentication_preimage);
        value["integrity"] = digest(&integrity);
        value["authentication"] = json!({
            "method": "ed25519",
            "keyId": "key:runtime-instance-1",
            "proofRef": "proof:runtime-instance-1",
            "signedDigest": digest(&signed_digest),
            "signature": format!("{signed_digest}{signed_digest}")
        });
        value
    }

    fn verified(value: Value) -> super::VerifiedRuntimeTerminalEvidence {
        let evidence: RuntimeTerminalEvidence = serde_json::from_value(seal(value)).unwrap();
        verify_runtime_terminal_evidence(
            evidence,
            &binding(),
            &FixtureVerifier {
                key_id: "key:runtime-instance-1",
                stale: false,
            },
        )
        .unwrap()
    }

    #[test]
    fn complete_authenticated_evidence_is_receipt_eligible() {
        let verified = verified(evidence_value());

        assert_eq!(
            verified.classification,
            RuntimeTerminalEvidenceClassification::ReceiptEligibleComplete
        );
        assert_ne!(
            verified.evidence.integrity.value,
            verified.evidence.authentication.signed_digest.value
        );
    }

    #[test]
    fn partial_and_ambiguous_observations_remain_authenticated_but_not_receipt_eligible() {
        let mut partial = evidence_value();
        partial["sideEffects"]["coverage"] = json!("partial");
        assert_eq!(
            verified(partial).classification,
            RuntimeTerminalEvidenceClassification::AuthenticatedPartialOrAmbiguous
        );

        let mut ambiguous = evidence_value();
        ambiguous["disposition"] = json!("ambiguous");
        assert_eq!(
            verified(ambiguous).classification,
            RuntimeTerminalEvidenceClassification::AuthenticatedPartialOrAmbiguous
        );
    }

    #[test]
    fn every_explicit_unknown_observation_is_classified_unknown() {
        for mutate in [
            (|value: &mut Value| {
                value["sideEffects"] = json!({
                    "state": "unknown",
                    "reasonCode": "runtime_observation_unavailable"
                });
            }) as fn(&mut Value),
            |value: &mut Value| {
                value["exercisedCapabilities"] = json!({
                    "state": "unknown",
                    "reasonCode": "runtime_observation_unavailable"
                });
            },
            |value: &mut Value| {
                value["result"] = json!({
                    "state": "unknown",
                    "reasonCode": "runtime_result_unavailable"
                });
            },
            |value: &mut Value| {
                value["delivery"] = json!({
                    "state": "unknown",
                    "reasonCode": "runtime_delivery_unavailable"
                });
            },
        ] {
            let mut unknown = evidence_value();
            mutate(&mut unknown);
            assert_eq!(
                verified(unknown).classification.as_str(),
                "authenticated_unknown"
            );
        }
    }

    #[test]
    fn unknown_precedes_partial_but_policy_violation_precedes_unknown() {
        let mut mixed = evidence_value();
        mixed["sideEffects"]["coverage"] = json!("partial");
        mixed["result"] = json!({
            "state": "unknown",
            "reasonCode": "runtime_result_unavailable"
        });
        assert_eq!(
            verified(mixed).classification.as_str(),
            "authenticated_unknown"
        );

        let mut violating_unknown = evidence_value();
        violating_unknown["sideEffects"] = json!({
            "state": "unknown",
            "reasonCode": "runtime_observation_unavailable"
        });
        violating_unknown["exercisedCapabilities"]["values"] =
            json!(["analysis.read", "artifact.write", "network.publish"]);
        assert_eq!(
            verified(violating_unknown).classification,
            RuntimeTerminalEvidenceClassification::PolicyViolating
        );
    }

    #[test]
    fn authenticated_capability_and_side_effect_escalation_is_classified_nonconformant() {
        let mut side_effect = evidence_value();
        side_effect["sideEffects"]["maximumClass"] = json!("external_mutation");
        assert_eq!(
            verified(side_effect).classification,
            RuntimeTerminalEvidenceClassification::PolicyViolating
        );

        let mut capability = evidence_value();
        capability["exercisedCapabilities"]["values"] =
            json!(["analysis.read", "artifact.write", "network.publish"]);
        assert_eq!(
            verified(capability).classification,
            RuntimeTerminalEvidenceClassification::PolicyViolating
        );
    }

    #[test]
    fn tagged_observation_unions_reject_unknown_fields_and_nullable_inference() {
        let mut unknown_field = evidence_value();
        unknown_field["sideEffects"]["unexpected"] = json!(true);
        assert!(serde_json::from_value::<RuntimeTerminalEvidence>(seal(unknown_field)).is_err());

        let mut null_observation = evidence_value();
        null_observation["sideEffects"] = Value::Null;
        assert!(serde_json::from_value::<RuntimeTerminalEvidence>(seal(null_observation)).is_err());

        let mut inferred_empty = evidence_value();
        inferred_empty["exercisedCapabilities"] = json!([]);
        assert!(serde_json::from_value::<RuntimeTerminalEvidence>(seal(inferred_empty)).is_err());
    }

    #[test]
    fn tampering_wrong_verifier_and_stale_proof_fail_with_privacy_safe_typed_errors() {
        let sealed = seal(evidence_value());
        let mut tampered = sealed.clone();
        tampered["sessionId"] = json!("private-attacker-session");
        let tampered: RuntimeTerminalEvidence = serde_json::from_value(tampered).unwrap();
        let error = verify_runtime_terminal_evidence(
            tampered,
            &binding(),
            &FixtureVerifier {
                key_id: "key:runtime-instance-1",
                stale: false,
            },
        )
        .unwrap_err();
        assert_eq!(
            error.code(),
            RuntimeTerminalEvidenceErrorCode::IntegrityInvalid
        );
        assert!(!format!("{error:#}").contains("private-attacker-session"));

        let evidence: RuntimeTerminalEvidence = serde_json::from_value(sealed.clone()).unwrap();
        let error = verify_runtime_terminal_evidence(
            evidence,
            &binding(),
            &FixtureVerifier {
                key_id: "key:wrong",
                stale: false,
            },
        )
        .unwrap_err();
        assert_eq!(
            error.code(),
            RuntimeTerminalEvidenceErrorCode::AuthenticationInvalid
        );
        assert!(!format!("{error:#}").contains("key:runtime-instance-1"));

        let evidence: RuntimeTerminalEvidence = serde_json::from_value(sealed).unwrap();
        let error = verify_runtime_terminal_evidence(
            evidence,
            &binding(),
            &FixtureVerifier {
                key_id: "key:runtime-instance-1",
                stale: true,
            },
        )
        .unwrap_err();
        assert_eq!(
            error.code(),
            RuntimeTerminalEvidenceErrorCode::AuthenticationStale
        );
        assert!(!format!("{error:#}").contains("proof:runtime-instance-1"));
    }
}
