//! Typed projection and fail-closed validation seam for
//! `coven.automations.authority.v1`.

use std::collections::BTreeSet;
use std::fmt;

use chrono::DateTime;
use serde::ser::Serializer;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use super::canonical_json::{canonicalize, sha256_hex};
use super::types::{
    AdoptionKey, AttemptId, AutomationId, DigestValue, OccurrenceId, OccurrenceKey,
    PositiveInteger, PrincipalId, ReceiptId, RunId, RuntimeId,
};
use super::types::{BoundedVec, ExtensionBag};

pub const AUTHORITY_PROFILE: &str = "coven.automations.authority.v1";
pub const AUTHORITY_EXTENSION_KEY: &str = AUTHORITY_PROFILE;
pub const BASE_PROFILE: &str = "coven.automations.v1";
pub const RUNTIME_AUTHORITY_CAPABILITY: &str = "automations.runtime-authority.v1";

const BINDING_DOMAIN: &[u8] = b"opencoven:coven-automations-authority-binding:v1";
const RECEIPT_DOMAIN: &[u8] = b"opencoven:coven-automations-authority-receipt-evidence:v1";
const AUTHORITY_EXTENSION_FIELDS: [&str; 4] =
    ["profile", "kind", "executionBinding", "receiptEvidence"];
const EXECUTION_BINDING_FIELDS: [&str; 19] = [
    "profile",
    "kind",
    "bindingId",
    "base",
    "principal",
    "authorization",
    "familiar",
    "contextProjection",
    "threads",
    "capabilities",
    "approval",
    "risk",
    "runtime",
    "versions",
    "decisionTimestamp",
    "producer",
    "privacy",
    "integrity",
    "authentication",
];
const RECEIPT_EVIDENCE_FIELDS: [&str; 26] = [
    "profile",
    "kind",
    "receiptId",
    "automationId",
    "automationRevision",
    "definitionDigest",
    "occurrenceId",
    "occurrenceFenceGeneration",
    "runId",
    "attemptId",
    "attemptNumber",
    "baseReceiptDigest",
    "bindingId",
    "bindingDigest",
    "principalId",
    "familiar",
    "authorization",
    "capabilities",
    "approval",
    "risk",
    "runtime",
    "decisionTimestamp",
    "producer",
    "privacy",
    "integrity",
    "authentication",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityConsumerClass {
    GenericBaseV1,
    RuntimeAuthorityV1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityValidationPhase {
    PreDispatch,
    Terminal,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorityProfileDisposition {
    PreservedOpaque(ExtensionBag),
    Validated(Box<AutomationAuthorityExtension>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityProfileErrorCode {
    AdapterMissing,
    AdoptionReplayed,
    ApprovalExpired,
    ApprovalExhausted,
    ApprovalRequired,
    ApprovalReused,
    ApprovalRevoked,
    ApprovalScopeMismatch,
    AuthenticationInvalid,
    AuthenticationUnverifiable,
    BindingMismatch,
    CapabilityEscalation,
    ChronologyInvalid,
    DefinitionMismatch,
    DispatchConsumptionMismatch,
    DispatchConsumptionMissing,
    EvidenceProjectionForbidden,
    FamiliarMismatch,
    FamiliarStale,
    FamiliarStatusInvalid,
    FamiliarTimeInvalid,
    FenceStale,
    IntegrityInvalid,
    IjsonInvalid,
    NonceReplayed,
    PolicyStale,
    PrincipalMismatch,
    ProfileMissing,
    ProfileRequired,
    ProfileUnknown,
    ReceiptBindingMismatch,
    ReceiptCorrelationMismatch,
    ReceiptEvidenceRequired,
    Replayed,
    RuntimeDowngrade,
    SchemaInvalid,
    SchemaUnknownField,
    Stale,
    TrustedStateUnavailable,
}

impl AuthorityProfileErrorCode {
    pub const ALL: [Self; 39] = [
        Self::AdapterMissing,
        Self::AdoptionReplayed,
        Self::ApprovalExpired,
        Self::ApprovalExhausted,
        Self::ApprovalRequired,
        Self::ApprovalReused,
        Self::ApprovalRevoked,
        Self::ApprovalScopeMismatch,
        Self::AuthenticationInvalid,
        Self::AuthenticationUnverifiable,
        Self::BindingMismatch,
        Self::CapabilityEscalation,
        Self::ChronologyInvalid,
        Self::DefinitionMismatch,
        Self::DispatchConsumptionMismatch,
        Self::DispatchConsumptionMissing,
        Self::EvidenceProjectionForbidden,
        Self::FamiliarMismatch,
        Self::FamiliarStale,
        Self::FamiliarStatusInvalid,
        Self::FamiliarTimeInvalid,
        Self::FenceStale,
        Self::IntegrityInvalid,
        Self::IjsonInvalid,
        Self::NonceReplayed,
        Self::PolicyStale,
        Self::PrincipalMismatch,
        Self::ProfileMissing,
        Self::ProfileRequired,
        Self::ProfileUnknown,
        Self::ReceiptBindingMismatch,
        Self::ReceiptCorrelationMismatch,
        Self::ReceiptEvidenceRequired,
        Self::Replayed,
        Self::RuntimeDowngrade,
        Self::SchemaInvalid,
        Self::SchemaUnknownField,
        Self::Stale,
        Self::TrustedStateUnavailable,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AdapterMissing => "AUTHORITY_ADAPTER_MISSING",
            Self::AdoptionReplayed => "AUTHORITY_ADOPTION_REPLAYED",
            Self::ApprovalExpired => "AUTHORITY_APPROVAL_EXPIRED",
            Self::ApprovalExhausted => "AUTHORITY_APPROVAL_EXHAUSTED",
            Self::ApprovalRequired => "AUTHORITY_APPROVAL_REQUIRED",
            Self::ApprovalReused => "AUTHORITY_APPROVAL_REUSED",
            Self::ApprovalRevoked => "AUTHORITY_APPROVAL_REVOKED",
            Self::ApprovalScopeMismatch => "AUTHORITY_APPROVAL_SCOPE_MISMATCH",
            Self::AuthenticationInvalid => "AUTHORITY_AUTHENTICATION_INVALID",
            Self::AuthenticationUnverifiable => "AUTHORITY_AUTHENTICATION_UNVERIFIABLE",
            Self::BindingMismatch => "AUTHORITY_BINDING_MISMATCH",
            Self::CapabilityEscalation => "AUTHORITY_CAPABILITY_ESCALATION",
            Self::ChronologyInvalid => "AUTHORITY_CHRONOLOGY_INVALID",
            Self::DefinitionMismatch => "AUTHORITY_DEFINITION_MISMATCH",
            Self::DispatchConsumptionMismatch => "AUTHORITY_DISPATCH_CONSUMPTION_MISMATCH",
            Self::DispatchConsumptionMissing => "AUTHORITY_DISPATCH_CONSUMPTION_MISSING",
            Self::EvidenceProjectionForbidden => "AUTHORITY_EVIDENCE_PROJECTION_FORBIDDEN",
            Self::FamiliarMismatch => "AUTHORITY_FAMILIAR_MISMATCH",
            Self::FamiliarStale => "AUTHORITY_FAMILIAR_STALE",
            Self::FamiliarStatusInvalid => "AUTHORITY_FAMILIAR_STATUS_INVALID",
            Self::FamiliarTimeInvalid => "AUTHORITY_FAMILIAR_TIME_INVALID",
            Self::FenceStale => "AUTHORITY_FENCE_STALE",
            Self::IntegrityInvalid => "AUTHORITY_INTEGRITY_INVALID",
            Self::IjsonInvalid => "AUTHORITY_IJSON_INVALID",
            Self::NonceReplayed => "AUTHORITY_NONCE_REPLAYED",
            Self::PolicyStale => "AUTHORITY_POLICY_STALE",
            Self::PrincipalMismatch => "AUTHORITY_PRINCIPAL_MISMATCH",
            Self::ProfileMissing => "AUTHORITY_PROFILE_MISSING",
            Self::ProfileRequired => "AUTHORITY_PROFILE_REQUIRED",
            Self::ProfileUnknown => "AUTHORITY_PROFILE_UNKNOWN",
            Self::ReceiptBindingMismatch => "AUTHORITY_RECEIPT_BINDING_MISMATCH",
            Self::ReceiptCorrelationMismatch => "AUTHORITY_RECEIPT_CORRELATION_MISMATCH",
            Self::ReceiptEvidenceRequired => "AUTHORITY_RECEIPT_EVIDENCE_REQUIRED",
            Self::Replayed => "AUTHORITY_REPLAYED",
            Self::RuntimeDowngrade => "AUTHORITY_RUNTIME_DOWNGRADE",
            Self::SchemaInvalid => "AUTHORITY_SCHEMA_INVALID",
            Self::SchemaUnknownField => "AUTHORITY_SCHEMA_UNKNOWN_FIELD",
            Self::Stale => "AUTHORITY_STALE",
            Self::TrustedStateUnavailable => "AUTHORITY_TRUSTED_STATE_UNAVAILABLE",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityProfileError {
    code: AuthorityProfileErrorCode,
    message: String,
}

impl AuthorityProfileError {
    #[must_use]
    pub fn new(code: AuthorityProfileErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    #[must_use]
    pub const fn code(&self) -> AuthorityProfileErrorCode {
        self.code
    }
}

impl fmt::Display for AuthorityProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code.as_str(), self.message)
    }
}

impl std::error::Error for AuthorityProfileError {}

pub trait AuthorityEvidenceVerifier {
    fn verify(
        &self,
        extension: &AutomationAuthorityExtension,
        phase: AuthorityValidationPhase,
    ) -> Result<(), AuthorityProfileError>;
}

pub fn validate_authority_profile(
    extensions: &ExtensionBag,
    consumer: AuthorityConsumerClass,
    advertised_profiles: &[&str],
    advertised_capabilities: &[&str],
    phase: AuthorityValidationPhase,
    verifier: Option<&dyn AuthorityEvidenceVerifier>,
) -> Result<AuthorityProfileDisposition, AuthorityProfileError> {
    if consumer == AuthorityConsumerClass::GenericBaseV1 {
        return Ok(AuthorityProfileDisposition::PreservedOpaque(
            extensions.clone(),
        ));
    }
    if !advertised_profiles.contains(&BASE_PROFILE)
        || !advertised_profiles.contains(&AUTHORITY_PROFILE)
        || !advertised_capabilities.contains(&RUNTIME_AUTHORITY_CAPABILITY)
    {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::ProfileRequired,
            "Runtime Authority requires explicit base, companion, and capability advertisement",
        ));
    }
    let value = extensions.get(AUTHORITY_EXTENSION_KEY).ok_or_else(|| {
        AuthorityProfileError::new(
            AuthorityProfileErrorCode::ProfileMissing,
            "authority extension is missing",
        )
    })?;
    let object = value.as_object().ok_or_else(|| {
        AuthorityProfileError::new(
            if value.is_null() {
                AuthorityProfileErrorCode::ProfileMissing
            } else {
                AuthorityProfileErrorCode::SchemaInvalid
            },
            "authority extension must be a non-null object",
        )
    })?;
    match object.get("profile") {
        None | Some(Value::Null) => {
            return Err(AuthorityProfileError::new(
                AuthorityProfileErrorCode::ProfileMissing,
                "authority profile is missing",
            ));
        }
        Some(Value::String(profile)) if profile != AUTHORITY_PROFILE => {
            return Err(AuthorityProfileError::new(
                AuthorityProfileErrorCode::ProfileUnknown,
                format!("unsupported authority profile {profile}"),
            ));
        }
        Some(Value::String(_)) => {}
        Some(_) => {
            return Err(AuthorityProfileError::new(
                AuthorityProfileErrorCode::SchemaInvalid,
                "authority profile must be a string",
            ));
        }
    }
    if let Some(field) = unknown_top_level_authority_field(value) {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::SchemaUnknownField,
            format!("unknown top-level authority field {field}"),
        ));
    }
    let extension: AutomationAuthorityExtension =
        serde_json::from_value(value.clone()).map_err(|error| {
            AuthorityProfileError::new(AuthorityProfileErrorCode::SchemaInvalid, error.to_string())
        })?;
    extension.validate_structure(phase)?;
    let verifier = verifier.ok_or_else(|| {
        AuthorityProfileError::new(
            AuthorityProfileErrorCode::AdapterMissing,
            "Runtime Authority verification adapter is unavailable",
        )
    })?;
    verifier.verify(&extension, phase)?;
    Ok(AuthorityProfileDisposition::Validated(Box::new(extension)))
}

fn unknown_top_level_authority_field(value: &Value) -> Option<String> {
    let extension = value.as_object()?;
    if let Some(field) = extension
        .keys()
        .find(|field| !AUTHORITY_EXTENSION_FIELDS.contains(&field.as_str()))
    {
        return Some(field.clone());
    }
    for (member, fields) in [
        ("executionBinding", EXECUTION_BINDING_FIELDS.as_slice()),
        ("receiptEvidence", RECEIPT_EVIDENCE_FIELDS.as_slice()),
    ] {
        let Some(object) = extension.get(member).and_then(Value::as_object) else {
            continue;
        };
        if let Some(field) = object
            .keys()
            .find(|field| !fields.contains(&field.as_str()))
        {
            return Some(format!("{member}.{field}"));
        }
    }
    None
}

macro_rules! validated_text {
    ($name:ident, $validator:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
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
                if $validator(&value) {
                    Ok(Self(value))
                } else {
                    Err(serde::de::Error::custom(stringify!($name)))
                }
            }
        }
    };
}

fn matches_opaque_identifier(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 256
        && bytes[0].is_ascii_alphabetic()
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'@' | b'-')
        })
}

fn matches_capability(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 96
        && bytes[0].is_ascii_lowercase()
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'-')
        })
}

fn matches_version(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'+' | b'_' | b'-'))
}

fn matches_signature(value: &str) -> bool {
    value.len() == 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn matches_occurrence_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    (8..=160).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'@' | b':' | b'-')
        })
}

fn matches_timestamp(value: &str) -> bool {
    let shape = (value.len() == 20 || value.len() == 24)
        && value.ends_with('Z')
        && (value.len() != 24 || value.as_bytes().get(19) == Some(&b'.'));
    shape && DateTime::parse_from_rfc3339(value).is_ok()
}

validated_text!(AuthorityOpaqueIdentifier, matches_opaque_identifier);
validated_text!(AuthorityCapability, matches_capability);
validated_text!(AuthorityVersion, matches_version);
validated_text!(AuthoritySignature, matches_signature);
validated_text!(AuthorityTimestamp, matches_timestamp);
validated_text!(AuthorityOccurrencePrefix, matches_occurrence_prefix);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityCapabilitySet(Vec<AuthorityCapability>);

impl Serialize for AuthorityCapabilitySet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl AuthorityCapabilitySet {
    #[must_use]
    pub fn as_slice(&self) -> &[AuthorityCapability] {
        &self.0
    }

    fn is_subset_of(&self, other: &Self) -> bool {
        self.0.iter().all(|capability| other.0.contains(capability))
    }

    fn has_same_members(&self, other: &Self) -> bool {
        self.0.len() == other.0.len() && self.is_subset_of(other)
    }
}

impl<'de> Deserialize<'de> for AuthorityCapabilitySet {
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
                "authority capability set must be unique and contain at most 128 entries",
            ))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityIdentifierSet(Vec<AuthorityOpaqueIdentifier>);

impl Serialize for AuthorityIdentifierSet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl AuthorityIdentifierSet {
    #[must_use]
    pub fn as_slice(&self) -> &[AuthorityOpaqueIdentifier] {
        &self.0
    }
}

impl<'de> Deserialize<'de> for AuthorityIdentifierSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = Vec::<AuthorityOpaqueIdentifier>::deserialize(deserializer)?;
        let unique = values.iter().collect::<BTreeSet<_>>().len() == values.len();
        if values.len() <= 128 && unique {
            Ok(Self(values))
        } else {
            Err(serde::de::Error::custom(
                "authority identifier set must be unique and contain at most 128 entries",
            ))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NullValue;

impl Serialize for NullValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_none()
    }
}

impl<'de> Deserialize<'de> for NullValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        <()>::deserialize(deserializer)?;
        Ok(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthorityProfile {
    #[serde(rename = "coven.automations.authority.v1")]
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthorityExtensionKind {
    AutomationAuthorityExtension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExecutionBindingKind {
    AutomationExecutionBinding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReceiptEvidenceKind {
    AutomationReceiptAuthorityEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationAuthorityExtension {
    pub profile: AuthorityProfile,
    pub kind: AuthorityExtensionKind,
    pub execution_binding: AutomationExecutionBinding,
    pub receipt_evidence: ReceiptEvidenceState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptEvidenceState(pub Option<Box<AutomationReceiptAuthorityEvidence>>);

impl Serialize for ReceiptEvidenceState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ReceiptEvidenceState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Option::<Box<AutomationReceiptAuthorityEvidence>>::deserialize(deserializer).map(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationExecutionBinding {
    pub profile: AuthorityProfile,
    pub kind: ExecutionBindingKind,
    pub binding_id: AuthorityOpaqueIdentifier,
    pub base: AuthorityBaseBinding,
    pub principal: AuthorityPrincipalBinding,
    pub authorization: AuthorityAuthorizationBinding,
    pub familiar: AuthorityFamiliarBinding,
    pub context_projection: AuthorityContextProjection,
    pub threads: AuthorityThreadsBinding,
    pub capabilities: AuthorityCapabilities,
    pub approval: AuthorityApprovalBinding,
    pub risk: AuthorityRisk,
    pub runtime: AuthorityRuntimeBinding,
    pub versions: AuthorityVersions,
    pub decision_timestamp: AuthorityTimestamp,
    pub producer: AuthorityProducer,
    pub privacy: AuthorityPrivacy,
    pub integrity: DigestValue,
    pub authentication: AuthorityAuthentication,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityBaseBinding {
    pub automation_id: AutomationId,
    pub automation_revision: PositiveInteger,
    pub definition_digest: DigestValue,
    pub occurrence_id: OccurrenceId,
    pub occurrence_key: OccurrenceKey,
    pub occurrence_fence_generation: PositiveInteger,
    pub run_id: RunId,
    pub attempt_id: AttemptId,
    pub attempt_number: PositiveInteger,
    pub adoption_key: AdoptionKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationState {
    Authenticated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityPrincipalBinding {
    pub principal_id: PrincipalId,
    pub authorization_proof_ref: AuthorityOpaqueIdentifier,
    pub authentication_state: AuthenticationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityReplayState {
    Fresh,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityOutcome {
    Permit,
    RequiresApproval,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityAuthorizationBinding {
    pub operation: AuthorityOpaqueIdentifier,
    pub request_id: AuthorityOpaqueIdentifier,
    pub request_digest: DigestValue,
    pub decision_id: AuthorityOpaqueIdentifier,
    pub decision_digest: DigestValue,
    pub nonce: AuthorityOpaqueIdentifier,
    pub issued_at: AuthorityTimestamp,
    pub valid_from: AuthorityTimestamp,
    pub valid_until: AuthorityTimestamp,
    pub replay_state: AuthorityReplayState,
    pub consumption_snapshot_id: AuthorityOpaqueIdentifier,
    pub consumption_snapshot_digest: DigestValue,
    pub consumption_store_revision: PositiveInteger,
    pub outcome: AuthorityOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FamiliarStatusAtDecision {
    Active,
    Revoked,
    Retired,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityFamiliarBinding {
    pub familiar_root_id: AuthorityOpaqueIdentifier,
    pub identity_revision_id: AuthorityOpaqueIdentifier,
    pub declaration_digest: DigestValue,
    pub embodiment_binding_id: AuthorityOpaqueIdentifier,
    pub embodiment_digest: DigestValue,
    pub status_at_decision: FamiliarStatusAtDecision,
    pub verified_at: AuthorityTimestamp,
    pub freshness_policy_version: AuthorityOpaqueIdentifier,
    pub freshness_bound_seconds: u16,
    pub valid_time: FamiliarValidTime,
    pub revocation: FamiliarRevocation,
    pub retirement: FamiliarRetirement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FamiliarValidTime {
    pub not_before: AuthorityTimestamp,
    pub not_after: AuthorityTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FamiliarRevocationState {
    NotRevoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FamiliarRevocation {
    pub state: FamiliarRevocationState,
    pub checked_at: AuthorityTimestamp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FamiliarRetirementState {
    NotRetired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FamiliarRetirement {
    pub state: FamiliarRetirementState,
    pub checked_at: AuthorityTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityContextProjection {
    pub project_id: AuthorityOpaqueIdentifier,
    pub workspace_id: AuthorityOpaqueIdentifier,
    pub context_projection_ids: AuthorityIdentifierSet,
    pub memory_projection_ids: AuthorityIdentifierSet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityThreadsBinding {
    pub decision_id: AuthorityOpaqueIdentifier,
    pub decision_digest: DigestValue,
    pub protected_surface_manifest_id: AuthorityOpaqueIdentifier,
    pub protected_surface_manifest_digest: DigestValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeniedCapability {
    pub capability: AuthorityCapability,
    pub reason_code: AuthorityOpaqueIdentifier,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityCapabilities {
    pub requested: AuthorityCapabilitySet,
    pub granted: AuthorityCapabilitySet,
    pub denied: BoundedVec<DeniedCapability, 128>,
    pub degraded: AuthorityCapabilitySet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "requirement",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AuthorityApprovalBinding {
    NotRequired {
        evidence: NullValue,
        scope_digest: NullValue,
        expires_at: NullValue,
        #[serde(rename = "use")]
        use_kind: NullValue,
        consumption: ApprovalNotRequiredConsumption,
    },
    HumanPerRun {
        evidence: ApprovalEvidence,
        scope_digest: DigestValue,
        expires_at: AuthorityTimestamp,
        #[serde(rename = "use")]
        use_kind: SingleUseApproval,
        consumption: ApprovalConsumption,
    },
    ProtectedOwnerPerRun {
        evidence: ApprovalEvidence,
        scope_digest: DigestValue,
        expires_at: AuthorityTimestamp,
        #[serde(rename = "use")]
        use_kind: SingleUseApproval,
        consumption: ApprovalConsumption,
    },
    BoundedRecurring {
        evidence: ApprovalEvidence,
        scope_digest: DigestValue,
        expires_at: AuthorityTimestamp,
        #[serde(rename = "use")]
        use_kind: RecurringApprovalUse,
        consumption: RecurringApprovalConsumption,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalEvidenceState {
    Approved,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalEvidence {
    pub approval_id: AuthorityOpaqueIdentifier,
    pub approval_digest: DigestValue,
    pub state: ApprovalEvidenceState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotRequiredState {
    NotRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalNotRequiredConsumption {
    pub state: NotRequiredState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SingleUseKind {
    SingleUse,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SingleUseApproval {
    pub kind: SingleUseKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurringUseKind {
    Recurring,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecurringApprovalUse {
    pub kind: RecurringUseKind,
    pub grant_id: AuthorityOpaqueIdentifier,
    pub max_uses: u16,
    pub occurrence_prefix: AuthorityOccurrencePrefix,
    pub prior_uses: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsumedForDispatchState {
    ConsumedForDispatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApprovalConsumption {
    pub state: ConsumedForDispatchState,
    pub event_id: AuthorityOpaqueIdentifier,
    pub event_digest: DigestValue,
    pub request_digest: DigestValue,
    pub decision_digest: DigestValue,
    pub occurrence_id: OccurrenceId,
    pub run_id: RunId,
    pub attempt_number: PositiveInteger,
    pub fence_generation: PositiveInteger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecurringApprovalConsumption {
    pub state: ConsumedForDispatchState,
    pub event_id: AuthorityOpaqueIdentifier,
    pub event_digest: DigestValue,
    pub request_digest: DigestValue,
    pub decision_digest: DigestValue,
    pub occurrence_id: OccurrenceId,
    pub run_id: RunId,
    pub attempt_number: PositiveInteger,
    pub fence_generation: PositiveInteger,
    pub usage_number: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskClass {
    R0,
    R1,
    R2,
    R3,
    R4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthoritySideEffectClass {
    None,
    LocalRead,
    LocalWrite,
    ExternalRead,
    ExternalMutation,
    IrreversibleExternalMutation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityRisk {
    pub risk_class: RiskClass,
    pub side_effect_class: AuthoritySideEffectClass,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeSelectionRationale {
    ExactRequirementMatch,
    PolicyPreferred,
    OnlyConformantRuntime,
    OperatorPinned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityRuntimeBinding {
    pub runtime_id: RuntimeId,
    pub descriptor_version: AuthorityVersion,
    pub descriptor_digest: DigestValue,
    pub capabilities: AuthorityCapabilitySet,
    pub selection_rationale: RuntimeSelectionRationale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BaseProfileVersion {
    #[serde(rename = "coven.automations.v1")]
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FamiliarProfileVersion {
    #[serde(rename = "familiar.embodiment_binding.v1")]
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThreadsProfileVersion {
    #[serde(rename = "automation-authority/1.0.0")]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityVersions {
    pub base_profile: BaseProfileVersion,
    pub authority_profile: AuthorityProfile,
    pub familiar_profile: FamiliarProfileVersion,
    pub threads_profile: ThreadsProfileVersion,
    pub policy_version: AuthorityOpaqueIdentifier,
    pub policy_digest: DigestValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityProducer {
    pub component: AuthorityOpaqueIdentifier,
    pub instance_id: AuthorityOpaqueIdentifier,
    pub implementation_version: AuthorityVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthorityAuthenticationMethod {
    #[serde(rename = "ed25519")]
    Ed25519,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityAuthentication {
    pub method: AuthorityAuthenticationMethod,
    pub key_id: AuthorityOpaqueIdentifier,
    pub proof_ref: AuthorityOpaqueIdentifier,
    pub signed_digest: super::types::Sha256Digest,
    pub signature: AuthoritySignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityPrivacyClassification {
    Operational,
    Sensitive,
    Restricted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthorityRetention {
    #[serde(rename = "ephemeral_24h")]
    Ephemeral24h,
    #[serde(rename = "authority_evidence_90d")]
    AuthorityEvidence90d,
    #[serde(rename = "authority_evidence_1y")]
    AuthorityEvidence1y,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityRedactionStatus {
    NotRequired,
    Redacted,
    Tombstoned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FalseValue;

impl Serialize for FalseValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_bool(false)
    }
}

impl<'de> Deserialize<'de> for FalseValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        if bool::deserialize(deserializer)? {
            Err(serde::de::Error::custom("expected false"))
        } else {
            Ok(Self)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityPrivacy {
    pub classification: AuthorityPrivacyClassification,
    pub retention: AuthorityRetention,
    pub redaction_status: AuthorityRedactionStatus,
    pub sensitive_material_included: FalseValue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationReceiptAuthorityEvidence {
    pub profile: AuthorityProfile,
    pub kind: ReceiptEvidenceKind,
    pub receipt_id: ReceiptId,
    pub automation_id: AutomationId,
    pub automation_revision: PositiveInteger,
    pub definition_digest: DigestValue,
    pub occurrence_id: OccurrenceId,
    pub occurrence_fence_generation: PositiveInteger,
    pub run_id: RunId,
    pub attempt_id: AttemptId,
    pub attempt_number: PositiveInteger,
    pub base_receipt_digest: DigestValue,
    pub binding_id: AuthorityOpaqueIdentifier,
    pub binding_digest: DigestValue,
    pub principal_id: PrincipalId,
    pub familiar: AuthorityReceiptFamiliar,
    pub authorization: AuthorityReceiptAuthorization,
    pub capabilities: AuthorityReceiptCapabilities,
    pub approval: AuthorityApprovalBinding,
    pub risk: AuthorityRisk,
    pub runtime: AuthorityReceiptRuntime,
    pub decision_timestamp: AuthorityTimestamp,
    pub producer: AuthorityProducer,
    pub privacy: AuthorityPrivacy,
    pub integrity: DigestValue,
    pub authentication: AuthorityAuthentication,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityReceiptFamiliar {
    pub familiar_root_id: AuthorityOpaqueIdentifier,
    pub identity_revision_id: AuthorityOpaqueIdentifier,
    pub declaration_digest: DigestValue,
    pub status_at_decision: FamiliarStatusAtDecision,
    pub verified_at: AuthorityTimestamp,
    pub freshness_policy_version: AuthorityOpaqueIdentifier,
    pub freshness_bound_seconds: u16,
    pub valid_time: FamiliarValidTime,
    pub revocation: FamiliarRevocation,
    pub retirement: FamiliarRetirement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityReceiptAuthorization {
    pub operation: AuthorityOpaqueIdentifier,
    pub request_id: AuthorityOpaqueIdentifier,
    pub request_digest: DigestValue,
    pub decision_id: AuthorityOpaqueIdentifier,
    pub decision_digest: DigestValue,
    pub consumption_snapshot_digest: DigestValue,
    pub outcome: AuthorityOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityReceiptCapabilities {
    pub requested: AuthorityCapabilitySet,
    pub granted: AuthorityCapabilitySet,
    pub denied: BoundedVec<DeniedCapability, 128>,
    pub degraded: AuthorityCapabilitySet,
    pub exercised: AuthorityCapabilitySet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorityReceiptRuntime {
    pub runtime_id: RuntimeId,
    pub descriptor_version: AuthorityVersion,
    pub descriptor_digest: DigestValue,
    pub capabilities: AuthorityCapabilitySet,
}

impl AutomationAuthorityExtension {
    fn validate_structure(
        &self,
        phase: AuthorityValidationPhase,
    ) -> Result<(), AuthorityProfileError> {
        match (phase, &self.receipt_evidence.0) {
            (AuthorityValidationPhase::PreDispatch, Some(_)) => {
                return Err(AuthorityProfileError::new(
                    AuthorityProfileErrorCode::SchemaInvalid,
                    "pre-dispatch authority must not contain receipt evidence",
                ));
            }
            (AuthorityValidationPhase::Terminal, None) => {
                return Err(AuthorityProfileError::new(
                    AuthorityProfileErrorCode::ReceiptEvidenceRequired,
                    "terminal authority requires receipt evidence",
                ));
            }
            _ => {}
        }
        self.execution_binding.validate_structure()?;
        if let Some(receipt) = &self.receipt_evidence.0 {
            receipt.validate_structure()?;
            self.validate_receipt_correlation(receipt)?;
        }
        Ok(())
    }

    fn validate_receipt_correlation(
        &self,
        receipt: &AutomationReceiptAuthorityEvidence,
    ) -> Result<(), AuthorityProfileError> {
        let binding = &self.execution_binding;
        if receipt.binding_id != binding.binding_id
            || receipt.binding_digest != binding.integrity
            || receipt.automation_id != binding.base.automation_id
            || receipt.automation_revision != binding.base.automation_revision
            || receipt.definition_digest != binding.base.definition_digest
            || receipt.occurrence_id != binding.base.occurrence_id
            || receipt.occurrence_fence_generation != binding.base.occurrence_fence_generation
            || receipt.run_id != binding.base.run_id
            || receipt.attempt_id != binding.base.attempt_id
            || receipt.attempt_number != binding.base.attempt_number
            || receipt.principal_id != binding.principal.principal_id
            || receipt.authorization.operation != binding.authorization.operation
            || receipt.authorization.request_id != binding.authorization.request_id
            || receipt.authorization.request_digest != binding.authorization.request_digest
            || receipt.authorization.decision_id != binding.authorization.decision_id
            || receipt.authorization.decision_digest != binding.authorization.decision_digest
            || receipt.authorization.consumption_snapshot_digest
                != binding.authorization.consumption_snapshot_digest
            || receipt.authorization.outcome != binding.authorization.outcome
            || receipt.approval != binding.approval
            || receipt.risk != binding.risk
            || receipt.decision_timestamp != binding.decision_timestamp
            || receipt.producer != binding.producer
            || receipt.privacy != binding.privacy
        {
            return Err(AuthorityProfileError::new(
                AuthorityProfileErrorCode::ReceiptBindingMismatch,
                "receipt authority evidence does not exact-match its binding",
            ));
        }
        if receipt.familiar.familiar_root_id != binding.familiar.familiar_root_id
            || receipt.familiar.identity_revision_id != binding.familiar.identity_revision_id
            || receipt.familiar.declaration_digest != binding.familiar.declaration_digest
            || receipt.familiar.status_at_decision != binding.familiar.status_at_decision
            || receipt.familiar.verified_at != binding.familiar.verified_at
            || receipt.familiar.freshness_policy_version
                != binding.familiar.freshness_policy_version
            || receipt.familiar.freshness_bound_seconds != binding.familiar.freshness_bound_seconds
            || receipt.familiar.valid_time != binding.familiar.valid_time
            || receipt.familiar.revocation != binding.familiar.revocation
            || receipt.familiar.retirement != binding.familiar.retirement
            || !receipt
                .capabilities
                .requested
                .has_same_members(&binding.capabilities.requested)
            || !receipt
                .capabilities
                .granted
                .has_same_members(&binding.capabilities.granted)
            || receipt.capabilities.denied != binding.capabilities.denied
            || !receipt
                .capabilities
                .degraded
                .has_same_members(&binding.capabilities.degraded)
            || receipt.runtime.runtime_id != binding.runtime.runtime_id
            || receipt.runtime.descriptor_version != binding.runtime.descriptor_version
            || receipt.runtime.descriptor_digest != binding.runtime.descriptor_digest
            || !receipt
                .runtime
                .capabilities
                .has_same_members(&binding.runtime.capabilities)
        {
            return Err(AuthorityProfileError::new(
                AuthorityProfileErrorCode::ReceiptBindingMismatch,
                "receipt authority projection was spliced",
            ));
        }
        Ok(())
    }
}

impl AutomationExecutionBinding {
    fn validate_structure(&self) -> Result<(), AuthorityProfileError> {
        if self.familiar.freshness_bound_seconds > 300 {
            return Err(schema_error("familiar freshness bound exceeds 300 seconds"));
        }
        validate_familiar_status(self.familiar.status_at_decision)?;
        validate_authorization_chronology(
            &self.authorization,
            &self.decision_timestamp,
            &self.familiar,
        )?;
        validate_capability_grant(
            &self.capabilities.requested,
            &self.capabilities.granted,
            &self.runtime.capabilities,
        )?;
        validate_outcome_approval(self.authorization.outcome, &self.approval)?;
        validate_approval(
            &self.approval,
            ApprovalCorrelation {
                request_digest: &self.authorization.request_digest,
                decision_digest: &self.authorization.decision_digest,
                occurrence_id: &self.base.occurrence_id,
                run_id: &self.base.run_id,
                attempt_number: self.base.attempt_number,
                fence_generation: self.base.occurrence_fence_generation,
            },
        )?;
        validate_integrity(self, &self.integrity, &self.authentication, BINDING_DOMAIN)
    }
}

impl AutomationReceiptAuthorityEvidence {
    fn validate_structure(&self) -> Result<(), AuthorityProfileError> {
        if self.familiar.freshness_bound_seconds > 300 {
            return Err(schema_error("familiar freshness bound exceeds 300 seconds"));
        }
        validate_familiar_status(self.familiar.status_at_decision)?;
        validate_familiar_times(&self.familiar, &self.decision_timestamp)?;
        validate_capability_grant(
            &self.capabilities.requested,
            &self.capabilities.granted,
            &self.runtime.capabilities,
        )?;
        validate_outcome_approval(self.authorization.outcome, &self.approval)?;
        if !self
            .capabilities
            .exercised
            .is_subset_of(&self.capabilities.granted)
        {
            return Err(AuthorityProfileError::new(
                AuthorityProfileErrorCode::CapabilityEscalation,
                "receipt exercises a capability that was not granted",
            ));
        }
        validate_approval(
            &self.approval,
            ApprovalCorrelation {
                request_digest: &self.authorization.request_digest,
                decision_digest: &self.authorization.decision_digest,
                occurrence_id: &self.occurrence_id,
                run_id: &self.run_id,
                attempt_number: self.attempt_number,
                fence_generation: self.occurrence_fence_generation,
            },
        )?;
        validate_integrity(self, &self.integrity, &self.authentication, RECEIPT_DOMAIN)
    }
}

fn validate_capability_grant(
    requested: &AuthorityCapabilitySet,
    granted: &AuthorityCapabilitySet,
    runtime: &AuthorityCapabilitySet,
) -> Result<(), AuthorityProfileError> {
    if !granted.is_subset_of(requested) || !granted.is_subset_of(runtime) {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::CapabilityEscalation,
            "granted capability exceeds the request or runtime descriptor",
        ));
    }
    Ok(())
}

fn validate_outcome_approval(
    outcome: AuthorityOutcome,
    approval: &AuthorityApprovalBinding,
) -> Result<(), AuthorityProfileError> {
    let approval_required = !matches!(approval, AuthorityApprovalBinding::NotRequired { .. });
    if matches!(outcome, AuthorityOutcome::Permit) == approval_required {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::ApprovalRequired,
            "authorization outcome and approval requirement do not match",
        ));
    }
    Ok(())
}

fn validate_authorization_chronology(
    authorization: &AuthorityAuthorizationBinding,
    decision: &AuthorityTimestamp,
    familiar: &AuthorityFamiliarBinding,
) -> Result<(), AuthorityProfileError> {
    let issued = timestamp_millis(&authorization.issued_at)?;
    let valid_from = timestamp_millis(&authorization.valid_from)?;
    let valid_until = timestamp_millis(&authorization.valid_until)?;
    let decision_millis = timestamp_millis(decision)?;
    if issued > valid_from
        || valid_from > decision_millis
        || decision_millis >= valid_until
        || valid_from >= valid_until
    {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::ChronologyInvalid,
            "authorization chronology is invalid",
        ));
    }
    validate_familiar_times(familiar, decision)
}

fn validate_familiar_status(status: FamiliarStatusAtDecision) -> Result<(), AuthorityProfileError> {
    if status != FamiliarStatusAtDecision::Active {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::FamiliarStatusInvalid,
            "familiar is not active at the authority decision",
        ));
    }
    Ok(())
}

trait FamiliarTimes {
    fn verified_at(&self) -> &AuthorityTimestamp;
    fn freshness_bound_seconds(&self) -> u16;
    fn valid_time(&self) -> &FamiliarValidTime;
    fn revocation(&self) -> &FamiliarRevocation;
    fn retirement(&self) -> &FamiliarRetirement;
}

impl FamiliarTimes for AuthorityFamiliarBinding {
    fn verified_at(&self) -> &AuthorityTimestamp {
        &self.verified_at
    }
    fn freshness_bound_seconds(&self) -> u16 {
        self.freshness_bound_seconds
    }
    fn valid_time(&self) -> &FamiliarValidTime {
        &self.valid_time
    }
    fn revocation(&self) -> &FamiliarRevocation {
        &self.revocation
    }
    fn retirement(&self) -> &FamiliarRetirement {
        &self.retirement
    }
}

impl FamiliarTimes for AuthorityReceiptFamiliar {
    fn verified_at(&self) -> &AuthorityTimestamp {
        &self.verified_at
    }
    fn freshness_bound_seconds(&self) -> u16 {
        self.freshness_bound_seconds
    }
    fn valid_time(&self) -> &FamiliarValidTime {
        &self.valid_time
    }
    fn revocation(&self) -> &FamiliarRevocation {
        &self.revocation
    }
    fn retirement(&self) -> &FamiliarRetirement {
        &self.retirement
    }
}

fn validate_familiar_times(
    familiar: &impl FamiliarTimes,
    decision: &AuthorityTimestamp,
) -> Result<(), AuthorityProfileError> {
    let verified = timestamp_millis(familiar.verified_at())?;
    let valid_from = timestamp_millis(&familiar.valid_time().not_before)?;
    let valid_until = timestamp_millis(&familiar.valid_time().not_after)?;
    let revocation_checked = timestamp_millis(&familiar.revocation().checked_at)?;
    let retirement_checked = timestamp_millis(&familiar.retirement().checked_at)?;
    let decision = timestamp_millis(decision)?;
    if valid_from > decision || decision >= valid_until || valid_from >= valid_until {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::FamiliarStale,
            "familiar validity does not cover the decision",
        ));
    }
    if verified > decision || revocation_checked > decision || retirement_checked > decision {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::FamiliarTimeInvalid,
            "familiar verification cannot follow the decision",
        ));
    }
    if decision.saturating_sub(verified) > i64::from(familiar.freshness_bound_seconds()) * 1_000 {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::FamiliarStale,
            "familiar verification exceeds its freshness bound at the decision",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ApprovalCorrelation<'a> {
    request_digest: &'a DigestValue,
    decision_digest: &'a DigestValue,
    occurrence_id: &'a OccurrenceId,
    run_id: &'a RunId,
    attempt_number: PositiveInteger,
    fence_generation: PositiveInteger,
}

fn validate_approval_consumption(
    consumption: ApprovalCorrelation<'_>,
    expected: ApprovalCorrelation<'_>,
) -> Result<(), AuthorityProfileError> {
    if consumption != expected {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::ApprovalRequired,
            "approval consumption does not match the authorized dispatch",
        ));
    }
    Ok(())
}

fn validate_approval(
    approval: &AuthorityApprovalBinding,
    correlation: ApprovalCorrelation<'_>,
) -> Result<(), AuthorityProfileError> {
    match approval {
        AuthorityApprovalBinding::HumanPerRun {
            evidence,
            consumption,
            ..
        }
        | AuthorityApprovalBinding::ProtectedOwnerPerRun {
            evidence,
            consumption,
            ..
        } => {
            validate_approval_evidence(evidence)?;
            validate_approval_consumption(
                ApprovalCorrelation {
                    request_digest: &consumption.request_digest,
                    decision_digest: &consumption.decision_digest,
                    occurrence_id: &consumption.occurrence_id,
                    run_id: &consumption.run_id,
                    attempt_number: consumption.attempt_number,
                    fence_generation: consumption.fence_generation,
                },
                correlation,
            )?;
        }
        AuthorityApprovalBinding::BoundedRecurring {
            evidence,
            use_kind,
            consumption,
            ..
        } => {
            validate_approval_evidence(evidence)?;
            if !correlation
                .occurrence_id
                .as_str()
                .starts_with(use_kind.occurrence_prefix.as_str())
            {
                return Err(AuthorityProfileError::new(
                    AuthorityProfileErrorCode::ApprovalScopeMismatch,
                    "bounded recurring approval does not cover the occurrence",
                ));
            }
            validate_approval_consumption(
                ApprovalCorrelation {
                    request_digest: &consumption.request_digest,
                    decision_digest: &consumption.decision_digest,
                    occurrence_id: &consumption.occurrence_id,
                    run_id: &consumption.run_id,
                    attempt_number: consumption.attempt_number,
                    fence_generation: consumption.fence_generation,
                },
                correlation,
            )?;
            if use_kind.max_uses == 0
                || use_kind.max_uses > 366
                || use_kind.prior_uses > 365
                || use_kind.prior_uses >= use_kind.max_uses
                || consumption.usage_number != use_kind.prior_uses + 1
                || consumption.usage_number > 366
            {
                return Err(AuthorityProfileError::new(
                    AuthorityProfileErrorCode::ApprovalExhausted,
                    "bounded recurring approval usage is invalid",
                ));
            }
        }
        AuthorityApprovalBinding::NotRequired { .. } => {}
    }
    Ok(())
}

fn validate_approval_evidence(evidence: &ApprovalEvidence) -> Result<(), AuthorityProfileError> {
    if evidence.state != ApprovalEvidenceState::Approved {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::ApprovalRevoked,
            "approval evidence is revoked",
        ));
    }
    Ok(())
}

fn validate_integrity<T: Serialize>(
    value: &T,
    integrity: &DigestValue,
    authentication: &AuthorityAuthentication,
    domain: &[u8],
) -> Result<(), AuthorityProfileError> {
    let mut body = serde_json::to_value(value).map_err(|error| schema_error(error.to_string()))?;
    let Value::Object(object) = &mut body else {
        return Err(schema_error("authority value must be an object"));
    };
    object.remove("integrity");
    object.remove("authentication");
    let canonical = canonicalize(&body).map_err(|error| {
        AuthorityProfileError::new(AuthorityProfileErrorCode::IjsonInvalid, error.to_string())
    })?;
    let mut preimage = Vec::with_capacity(domain.len() + canonical.len() + 1);
    preimage.extend_from_slice(domain);
    preimage.push(0);
    preimage.extend_from_slice(&canonical);
    let digest = sha256_hex(&preimage);
    if integrity.value.as_str() != digest {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::IntegrityInvalid,
            "authority integrity digest does not match",
        ));
    }
    if authentication.signed_digest.as_str() != digest {
        return Err(AuthorityProfileError::new(
            AuthorityProfileErrorCode::AuthenticationInvalid,
            "authority authentication does not cover the integrity digest",
        ));
    }
    Ok(())
}

fn timestamp_millis(value: &AuthorityTimestamp) -> Result<i64, AuthorityProfileError> {
    DateTime::parse_from_rfc3339(value.as_str())
        .map(|value| value.timestamp_millis())
        .map_err(|_| schema_error("authority timestamp is invalid"))
}

fn schema_error(message: impl Into<String>) -> AuthorityProfileError {
    AuthorityProfileError::new(AuthorityProfileErrorCode::SchemaInvalid, message)
}

#[cfg(test)]
mod structural_tests {
    use serde_json::{json, Value};

    use super::*;

    const VECTORS: &str =
        include_str!("../../../../../spec/coven-automations/authority/v1/test-vectors.json");

    fn fixture(name: &str) -> Value {
        serde_json::from_str::<Value>(VECTORS)
            .expect("authority vectors")
            .pointer(&format!("/fixtures/{name}"))
            .expect("authority fixture")
            .clone()
    }

    fn resign(value: &mut Value, domain: &[u8]) -> String {
        let mut body = value.clone();
        let object = body.as_object_mut().expect("authority object");
        object.remove("integrity");
        object.remove("authentication");
        let canonical = canonicalize(&body).expect("canonical authority value");
        let mut preimage = Vec::with_capacity(domain.len() + canonical.len() + 1);
        preimage.extend_from_slice(domain);
        preimage.push(0);
        preimage.extend_from_slice(&canonical);
        let digest = sha256_hex(&preimage);
        value["integrity"]["value"] = json!(digest);
        value["authentication"]["signedDigest"] = json!(digest);
        digest
    }

    fn extension(binding: Value, receipt: Value) -> AutomationAuthorityExtension {
        serde_json::from_value(json!({
            "profile": AUTHORITY_PROFILE,
            "kind": "AutomationAuthorityExtension",
            "executionBinding": binding,
            "receiptEvidence": receipt
        }))
        .expect("authority extension")
    }

    #[test]
    fn receipt_binding_capability_sets_correlate_without_array_order() {
        for (label, binding_path, receipt_path) in [
            (
                "requested",
                "/capabilities/requested",
                "/capabilities/requested",
            ),
            ("granted", "/capabilities/granted", "/capabilities/granted"),
            (
                "degraded",
                "/capabilities/degraded",
                "/capabilities/degraded",
            ),
            ("runtime", "/runtime/capabilities", "/runtime/capabilities"),
        ] {
            let mut binding = fixture("binding");
            let mut receipt = fixture("receiptEvidence");
            *binding
                .pointer_mut(binding_path)
                .expect("binding capability set") = json!(["analysis.read", "artifact.write"]);
            *receipt
                .pointer_mut(receipt_path)
                .expect("receipt capability set") = json!(["artifact.write", "analysis.read"]);
            let binding_digest = resign(&mut binding, BINDING_DOMAIN);
            receipt["bindingDigest"]["value"] = json!(binding_digest);
            resign(&mut receipt, RECEIPT_DOMAIN);

            extension(binding, receipt)
                .validate_structure(AuthorityValidationPhase::Terminal)
                .unwrap_or_else(|error| {
                    panic!("{label} capability set order must not affect correlation: {error}")
                });
        }
    }

    #[test]
    fn binding_rejects_familiar_age_beyond_signed_bound() {
        let mut binding = fixture("binding");
        binding["familiar"]["verifiedAt"] = json!("2026-09-03T11:54:58.999Z");
        resign(&mut binding, BINDING_DOMAIN);
        let binding: AutomationExecutionBinding =
            serde_json::from_value(binding).expect("execution binding");

        let error = binding
            .validate_structure()
            .expect_err("stale familiar verification must fail closed");
        assert_eq!(error.code(), AuthorityProfileErrorCode::FamiliarStale);
    }

    #[test]
    fn receipt_rejects_familiar_age_beyond_signed_bound() {
        let mut receipt = fixture("receiptEvidence");
        receipt["familiar"]["verifiedAt"] = json!("2026-09-03T11:54:58.999Z");
        resign(&mut receipt, RECEIPT_DOMAIN);
        let receipt: AutomationReceiptAuthorityEvidence =
            serde_json::from_value(receipt).expect("receipt authority evidence");

        let error = receipt
            .validate_structure()
            .expect_err("stale receipt familiar verification must fail closed");
        assert_eq!(error.code(), AuthorityProfileErrorCode::FamiliarStale);
    }

    #[test]
    fn binding_accepts_familiar_age_equal_to_signed_bound() {
        let mut binding = fixture("binding");
        binding["familiar"]["verifiedAt"] = json!("2026-09-03T11:54:59.000Z");
        resign(&mut binding, BINDING_DOMAIN);
        let binding: AutomationExecutionBinding =
            serde_json::from_value(binding).expect("execution binding");

        binding
            .validate_structure()
            .expect("familiar age equal to the bound remains valid");
    }

    #[test]
    fn binding_rejects_approval_consumption_anchor_mismatches() {
        for (path, replacement) in [
            (
                "/approval/consumption/requestDigest/value",
                json!("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"),
            ),
            (
                "/approval/consumption/decisionDigest/value",
                json!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
            ),
            (
                "/approval/consumption/occurrenceId",
                json!("occurrence.other-20260903"),
            ),
            ("/approval/consumption/runId", json!("run.other-1")),
            ("/approval/consumption/attemptNumber", json!(2)),
            ("/approval/consumption/fenceGeneration", json!(8)),
        ] {
            let mut binding = fixture("binding");
            *binding
                .pointer_mut(path)
                .expect("approval consumption field") = replacement;
            resign(&mut binding, BINDING_DOMAIN);
            let binding: AutomationExecutionBinding =
                serde_json::from_value(binding).expect("execution binding");

            let error = match binding.validate_structure() {
                Ok(()) => panic!("{path} mismatch must fail closed"),
                Err(error) => error,
            };
            assert_eq!(
                error.code(),
                AuthorityProfileErrorCode::ApprovalRequired,
                "{path}"
            );
        }
    }

    #[test]
    fn receipt_rejects_approval_consumption_anchor_mismatches() {
        for (path, replacement) in [
            (
                "/approval/consumption/requestDigest/value",
                json!("dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"),
            ),
            (
                "/approval/consumption/decisionDigest/value",
                json!("eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"),
            ),
            (
                "/approval/consumption/occurrenceId",
                json!("occurrence.other-20260903"),
            ),
            ("/approval/consumption/runId", json!("run.other-1")),
            ("/approval/consumption/attemptNumber", json!(2)),
            ("/approval/consumption/fenceGeneration", json!(8)),
        ] {
            let mut receipt = fixture("receiptEvidence");
            *receipt
                .pointer_mut(path)
                .expect("approval consumption field") = replacement;
            resign(&mut receipt, RECEIPT_DOMAIN);
            let receipt: AutomationReceiptAuthorityEvidence =
                serde_json::from_value(receipt).expect("receipt authority evidence");

            let error = match receipt.validate_structure() {
                Ok(()) => panic!("{path} mismatch must fail closed"),
                Err(error) => error,
            };
            assert_eq!(
                error.code(),
                AuthorityProfileErrorCode::ApprovalRequired,
                "{path}"
            );
        }
    }

    #[test]
    fn binding_rejects_recurring_occurrence_outside_prefix() {
        let mut binding = fixture("binding");
        binding["base"]["occurrenceId"] = json!("occurrence.other-20260903");
        binding["approval"]["consumption"]["occurrenceId"] = json!("occurrence.other-20260903");
        resign(&mut binding, BINDING_DOMAIN);
        let binding: AutomationExecutionBinding =
            serde_json::from_value(binding).expect("execution binding");

        let error = binding
            .validate_structure()
            .expect_err("recurring occurrence outside its prefix must fail closed");
        assert_eq!(
            error.code(),
            AuthorityProfileErrorCode::ApprovalScopeMismatch
        );
    }

    #[test]
    fn receipt_rejects_recurring_occurrence_outside_prefix() {
        let mut receipt = fixture("receiptEvidence");
        receipt["occurrenceId"] = json!("occurrence.other-20260903");
        receipt["approval"]["consumption"]["occurrenceId"] = json!("occurrence.other-20260903");
        resign(&mut receipt, RECEIPT_DOMAIN);
        let receipt: AutomationReceiptAuthorityEvidence =
            serde_json::from_value(receipt).expect("receipt authority evidence");

        let error = receipt
            .validate_structure()
            .expect_err("receipt recurring occurrence outside its prefix must fail closed");
        assert_eq!(
            error.code(),
            AuthorityProfileErrorCode::ApprovalScopeMismatch
        );
    }
}
