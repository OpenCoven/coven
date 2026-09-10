//! Portable Coven Automations v1 conformance-result verification.
//!
//! This module verifies result envelopes produced elsewhere. It does not run
//! conformance suites, certify Coven, or provide a production signer or trust
//! root.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, Duration, Utc};
use p256::ecdsa::signature::Verifier;
use p256::ecdsa::{Signature, VerifyingKey};
use serde::ser::Serializer;
use serde::{Deserialize, Deserializer, Serialize};

use super::canonical_json::{canonicalize, sha256_hex, MAX_SAFE_INTEGER};
use super::types::{deserialize_non_null_option, DigestValue, Sha256Digest};

const MAX_PROFILE_RESULTS: usize = 7;
const MAX_SUITES_PER_PROFILE: usize = 128;
const MAX_TRUST_POLICY_AGE: Duration = Duration::days(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConformanceResultError {
    TrustPolicyInvalid,
    SchemaInvalid,
    StatementDigestMismatch,
    SourceRepositoryMismatch,
    SourceCommitMismatch,
    BundleSchemaVersionMismatch,
    BundleSourceCommitMismatch,
    BundleDigestMismatch,
    ContractContentDigestMismatch,
    FileCountMismatch,
    RunnerNameMismatch,
    RunnerVersionMismatch,
    RunnerArtifactDigestMismatch,
    VectorSetDigestMismatch,
    ProfileDuplicate,
    SuiteDuplicate,
    SuiteSetMismatch,
    PassedSuiteEvidenceMissing,
    ProfileStatusInconsistent,
    OverallStatusInconsistent,
    FullProfileIncomplete,
    ReleasePolicyMissing,
    PolicyBindingMismatch,
    EvidenceExpirationRequired,
    EvidenceChronologyInvalid,
    EvidenceNotYetValid,
    EvidenceStale,
    EvidenceExpired,
    RequiredProfileMissing,
    RequiredProfileNotPassed,
    RequiredSuiteSetMismatch,
    EnvironmentNotAllowed,
    AuthenticationRequired,
    AuthenticationKeyUntrusted,
    AuthenticationEncodingInvalid,
    AuthenticationInvalid,
}

impl fmt::Display for ConformanceResultError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::TrustPolicyInvalid => "conformance trust policy is invalid",
            Self::SchemaInvalid => "conformance result schema is invalid",
            Self::StatementDigestMismatch => "conformance statement digest is invalid",
            Self::SourceRepositoryMismatch => "conformance source repository does not match",
            Self::SourceCommitMismatch => "conformance source commit does not match",
            Self::BundleSchemaVersionMismatch => "conformance bundle schema version does not match",
            Self::BundleSourceCommitMismatch => "conformance bundle source commit does not match",
            Self::BundleDigestMismatch => "conformance bundle digest does not match",
            Self::ContractContentDigestMismatch => {
                "conformance contract content digest does not match"
            }
            Self::FileCountMismatch => "conformance artifact file count does not match",
            Self::RunnerNameMismatch => "conformance runner name does not match",
            Self::RunnerVersionMismatch => "conformance runner version does not match",
            Self::RunnerArtifactDigestMismatch => {
                "conformance runner artifact digest does not match"
            }
            Self::VectorSetDigestMismatch => "conformance vector set digest does not match",
            Self::ProfileDuplicate => "conformance profile results contain a duplicate",
            Self::SuiteDuplicate => "conformance suite results contain a duplicate",
            Self::SuiteSetMismatch => "conformance required suite results do not match",
            Self::PassedSuiteEvidenceMissing => "passed conformance suite is missing evidence",
            Self::ProfileStatusInconsistent => "conformance profile status is inconsistent",
            Self::OverallStatusInconsistent => "conformance overall status is inconsistent",
            Self::FullProfileIncomplete => "full conformance profile is missing a passed component",
            Self::ReleasePolicyMissing => "release conformance policy is unavailable",
            Self::PolicyBindingMismatch => "release conformance policy does not match",
            Self::EvidenceExpirationRequired => "release conformance expiration is required",
            Self::EvidenceChronologyInvalid => "release conformance chronology is invalid",
            Self::EvidenceNotYetValid => "release conformance evidence is not yet valid",
            Self::EvidenceStale => "release conformance evidence is stale",
            Self::EvidenceExpired => "release conformance evidence is expired",
            Self::RequiredProfileMissing => "required conformance profile is missing",
            Self::RequiredProfileNotPassed => "required conformance profile did not pass",
            Self::RequiredSuiteSetMismatch => "required conformance suite inventory does not match",
            Self::EnvironmentNotAllowed => "conformance environment is not allowed",
            Self::AuthenticationRequired => "release conformance authentication is required",
            Self::AuthenticationKeyUntrusted => "conformance authentication key is not trusted",
            Self::AuthenticationEncodingInvalid => "conformance authentication encoding is invalid",
            Self::AuthenticationInvalid => "conformance authentication is invalid",
        })
    }
}

impl std::error::Error for ConformanceResultError {}

macro_rules! validated_text {
    ($name:ident, $validator:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, ConformanceResultError> {
                let value = value.into();
                if $validator(&value) {
                    Ok(Self(value))
                } else {
                    Err(ConformanceResultError::SchemaInvalid)
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

fn matches_opaque_identifier(value: &str, maximum: usize) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= maximum
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b':' | b'@' | b'-')
        })
}

fn matches_result_id(value: &str) -> bool {
    matches_opaque_identifier(value, 160)
}

fn matches_policy_id(value: &str) -> bool {
    matches_opaque_identifier(value, 160)
}

fn matches_key_id(value: &str) -> bool {
    matches_opaque_identifier(value, 160)
}

fn matches_suite_id(value: &str) -> bool {
    matches_opaque_identifier(value, 128)
}

fn matches_repository(value: &str) -> bool {
    let Some((scheme, location)) = value.split_once("://") else {
        return false;
    };
    !scheme.is_empty()
        && scheme.as_bytes()[0].is_ascii_alphabetic()
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
        && !location.is_empty()
        && value.len() <= 512
        && location
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'()*+,./:;=?@_~-".contains(&byte))
}

fn matches_source_commit(value: &str) -> bool {
    value.len() == 40
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn matches_version(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 96
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b':' | b'-')
        })
}

fn matches_environment_fact(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 128
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..].iter().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b':' | b'-')
        })
}

fn matches_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    let punctuation_matches = match bytes.len() {
        20 => {
            bytes[4] == b'-'
                && bytes[7] == b'-'
                && bytes[10] == b'T'
                && bytes[13] == b':'
                && bytes[16] == b':'
                && bytes[19] == b'Z'
        }
        24 => {
            bytes[4] == b'-'
                && bytes[7] == b'-'
                && bytes[10] == b'T'
                && bytes[13] == b':'
                && bytes[16] == b':'
                && bytes[19] == b'.'
                && bytes[23] == b'Z'
        }
        _ => false,
    };
    punctuation_matches
        && bytes.iter().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19 | 23) || byte.is_ascii_digit()
        })
        && DateTime::parse_from_rfc3339(value).is_ok()
}

fn matches_signature(value: &str) -> bool {
    (1..=128).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

validated_text!(ConformanceResultId, matches_result_id);
validated_text!(ConformancePolicyId, matches_policy_id);
validated_text!(ConformanceKeyId, matches_key_id);
validated_text!(ConformanceSuiteId, matches_suite_id);
validated_text!(ConformanceRepository, matches_repository);
validated_text!(ConformanceSourceCommit, matches_source_commit);
validated_text!(ConformanceVersion, matches_version);
validated_text!(ConformanceEnvironmentFact, matches_environment_fact);
validated_text!(ConformanceTimestamp, matches_timestamp);
validated_text!(ConformanceSignature, matches_signature);

impl ConformanceTimestamp {
    fn as_datetime(&self) -> Result<DateTime<Utc>, ConformanceResultError> {
        DateTime::parse_from_rfc3339(self.as_str())
            .map(|value| value.with_timezone(&Utc))
            .map_err(|_| ConformanceResultError::SchemaInvalid)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConformanceResultSchemaVersion {
    #[serde(rename = "coven.automations.conformance-result.v1")]
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConformanceContractProfile {
    #[serde(rename = "coven.automations.v1")]
    V1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConformanceProfile {
    Structural,
    SchedulerReliability,
    RuntimeAuthority,
    Continuity,
    Privacy,
    Interoperability,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConformanceStatus {
    Passed,
    Failed,
    Incomplete,
    NotApplicable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConformancePolicyBinding {
    pub policy_id: ConformancePolicyId,
    pub policy_version: ConformanceVersion,
    pub digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConformanceDecisionScope {
    AuditOnly,
    ReleaseEligibility {
        #[serde(rename = "policyBinding")]
        policy_binding: ConformancePolicyBinding,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConformanceSourceBinding {
    pub repository: ConformanceRepository,
    pub commit: ConformanceSourceCommit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConformanceFileCount(u64);

impl ConformanceFileCount {
    pub fn new(value: u64) -> Result<Self, ConformanceResultError> {
        if (1..=MAX_SAFE_INTEGER).contains(&value) {
            Ok(Self(value))
        } else {
            Err(ConformanceResultError::SchemaInvalid)
        }
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl Serialize for ConformanceFileCount {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(self.0)
    }
}

impl<'de> Deserialize<'de> for ConformanceFileCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let number = serde_json::Number::deserialize(deserializer)?;
        parse_exact_positive_integer(&number.to_string())
            .and_then(Self::new)
            .map_err(serde::de::Error::custom)
    }
}

fn parse_exact_positive_integer(value: &str) -> Result<u64, ConformanceResultError> {
    if value.starts_with('-') {
        return Err(ConformanceResultError::SchemaInvalid);
    }
    let (mantissa, exponent) =
        value
            .split_once(['e', 'E'])
            .map_or((value, 0_i32), |(mantissa, exponent)| {
                exponent
                    .parse::<i32>()
                    .map(|exponent| (mantissa, exponent))
                    .unwrap_or((mantissa, i32::MIN))
            });
    if exponent == i32::MIN {
        return Err(ConformanceResultError::SchemaInvalid);
    }
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let mut digits = String::with_capacity(whole.len() + fraction.len());
    digits.push_str(whole);
    digits.push_str(fraction);
    let first_nonzero = digits.find(|character| character != '0');
    let Some(first_nonzero) = first_nonzero else {
        return Err(ConformanceResultError::SchemaInvalid);
    };
    digits.drain(..first_nonzero);

    let decimal_places =
        i64::try_from(fraction.len()).map_err(|_| ConformanceResultError::SchemaInvalid)?;
    let scale = decimal_places - i64::from(exponent);
    if scale > 0 {
        let scale = usize::try_from(scale).map_err(|_| ConformanceResultError::SchemaInvalid)?;
        if scale >= digits.len()
            || !digits[digits.len() - scale..]
                .bytes()
                .all(|byte| byte == b'0')
        {
            return Err(ConformanceResultError::SchemaInvalid);
        }
        digits.truncate(digits.len() - scale);
    } else if scale < 0 {
        let zeros = usize::try_from(-scale).map_err(|_| ConformanceResultError::SchemaInvalid)?;
        if digits.len().saturating_add(zeros) > MAX_SAFE_INTEGER.to_string().len() {
            return Err(ConformanceResultError::SchemaInvalid);
        }
        digits.extend(std::iter::repeat_n('0', zeros));
    }

    digits
        .parse::<u64>()
        .map_err(|_| ConformanceResultError::SchemaInvalid)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConformanceProtocolArtifactBinding {
    pub bundle_schema_version: ConformanceVersion,
    pub source_commit: ConformanceSourceCommit,
    pub bundle_sha256: Sha256Digest,
    pub contract_content_sha256: Sha256Digest,
    pub file_count: ConformanceFileCount,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConformanceRunnerBinding {
    pub name: ConformanceResultId,
    pub version: ConformanceVersion,
    pub artifact_sha256: Sha256Digest,
    pub vector_set_sha256: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConformanceEnvironment {
    pub os: ConformanceEnvironmentFact,
    pub arch: ConformanceEnvironmentFact,
    pub runtime: ConformanceEnvironmentFact,
}

impl ConformanceEnvironment {
    pub fn new(
        os: impl Into<String>,
        arch: impl Into<String>,
        runtime: impl Into<String>,
    ) -> Result<Self, ConformanceResultError> {
        Ok(Self {
            os: ConformanceEnvironmentFact::new(os)?,
            arch: ConformanceEnvironmentFact::new(arch)?,
            runtime: ConformanceEnvironmentFact::new(runtime)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConformanceSuiteResult {
    pub suite_id: ConformanceSuiteId,
    pub status: ConformanceStatus,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub evidence_digest: Option<DigestValue>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConformanceProfileResult {
    pub profile: ConformanceProfile,
    pub status: ConformanceStatus,
    pub required_suites: Vec<ConformanceSuiteId>,
    pub suite_results: Vec<ConformanceSuiteResult>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConformanceResultStatement {
    pub contract_profile: ConformanceContractProfile,
    pub result_id: ConformanceResultId,
    pub decision_scope: ConformanceDecisionScope,
    pub source: ConformanceSourceBinding,
    pub protocol_artifact: ConformanceProtocolArtifactBinding,
    pub runner: ConformanceRunnerBinding,
    pub environment: ConformanceEnvironment,
    pub observed_at: ConformanceTimestamp,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub expires_at: Option<ConformanceTimestamp>,
    pub profile_results: Vec<ConformanceProfileResult>,
    pub overall_status: ConformanceStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConformanceAuthenticationMethod {
    #[serde(rename = "p256-sha256")]
    P256Sha256,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConformanceAuthentication {
    pub method: ConformanceAuthenticationMethod,
    pub key_id: ConformanceKeyId,
    pub signature: ConformanceSignature,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConformanceResult {
    pub schema_version: ConformanceResultSchemaVersion,
    pub statement: ConformanceResultStatement,
    pub statement_digest: DigestValue,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub authentication: Option<ConformanceAuthentication>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedSourceBinding {
    repository: ConformanceRepository,
    commit: ConformanceSourceCommit,
}

impl ExpectedSourceBinding {
    pub fn new(
        repository: impl Into<String>,
        commit: impl Into<String>,
    ) -> Result<Self, ConformanceResultError> {
        Ok(Self {
            repository: ConformanceRepository::new(repository)?,
            commit: ConformanceSourceCommit::new(commit)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedProtocolArtifactBinding {
    bundle_schema_version: ConformanceVersion,
    source_commit: ConformanceSourceCommit,
    bundle_sha256: Sha256Digest,
    contract_content_sha256: Sha256Digest,
    file_count: ConformanceFileCount,
}

impl ExpectedProtocolArtifactBinding {
    pub fn new(
        bundle_schema_version: impl Into<String>,
        source_commit: impl Into<String>,
        bundle_sha256: impl Into<String>,
        contract_content_sha256: impl Into<String>,
        file_count: u64,
    ) -> Result<Self, ConformanceResultError> {
        Ok(Self {
            bundle_schema_version: ConformanceVersion::new(bundle_schema_version)?,
            source_commit: ConformanceSourceCommit::new(source_commit)?,
            bundle_sha256: Sha256Digest::new(bundle_sha256.into())
                .map_err(|_| ConformanceResultError::SchemaInvalid)?,
            contract_content_sha256: Sha256Digest::new(contract_content_sha256.into())
                .map_err(|_| ConformanceResultError::SchemaInvalid)?,
            file_count: ConformanceFileCount::new(file_count)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedRunnerBinding {
    name: ConformanceResultId,
    version: ConformanceVersion,
    artifact_sha256: Sha256Digest,
    vector_set_sha256: Sha256Digest,
}

impl ExpectedRunnerBinding {
    pub fn new(
        name: impl Into<String>,
        version: impl Into<String>,
        artifact_sha256: impl Into<String>,
        vector_set_sha256: impl Into<String>,
    ) -> Result<Self, ConformanceResultError> {
        Ok(Self {
            name: ConformanceResultId::new(name)?,
            version: ConformanceVersion::new(version)?,
            artifact_sha256: Sha256Digest::new(artifact_sha256.into())
                .map_err(|_| ConformanceResultError::SchemaInvalid)?,
            vector_set_sha256: Sha256Digest::new(vector_set_sha256.into())
                .map_err(|_| ConformanceResultError::SchemaInvalid)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedArtifactBinding {
    source: ExpectedSourceBinding,
    protocol_artifact: ExpectedProtocolArtifactBinding,
    runner: ExpectedRunnerBinding,
}

impl ExpectedArtifactBinding {
    #[must_use]
    pub const fn new(
        source: ExpectedSourceBinding,
        protocol_artifact: ExpectedProtocolArtifactBinding,
        runner: ExpectedRunnerBinding,
    ) -> Self {
        Self {
            source,
            protocol_artifact,
            runner,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpectedPolicyBinding {
    policy_id: ConformancePolicyId,
    policy_version: ConformanceVersion,
    digest: Sha256Digest,
}

impl ExpectedPolicyBinding {
    pub fn new(
        policy_id: impl Into<String>,
        policy_version: impl Into<String>,
        digest: impl Into<String>,
    ) -> Result<Self, ConformanceResultError> {
        Ok(Self {
            policy_id: ConformancePolicyId::new(policy_id)?,
            policy_version: ConformanceVersion::new(policy_version)?,
            digest: Sha256Digest::new(digest.into())
                .map_err(|_| ConformanceResultError::SchemaInvalid)?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConformanceProfileRequirement {
    profile: ConformanceProfile,
    required_suites: BTreeSet<ConformanceSuiteId>,
}

impl ConformanceProfileRequirement {
    pub fn new<I, S>(
        profile: ConformanceProfile,
        required_suites: I,
    ) -> Result<Self, ConformanceResultError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut suites = BTreeSet::new();
        for suite_id in required_suites {
            let suite_id = ConformanceSuiteId::new(suite_id)
                .map_err(|_| ConformanceResultError::TrustPolicyInvalid)?;
            if !suites.insert(suite_id) || suites.len() > MAX_SUITES_PER_PROFILE {
                return Err(ConformanceResultError::TrustPolicyInvalid);
            }
        }
        if suites.is_empty() {
            return Err(ConformanceResultError::TrustPolicyInvalid);
        }
        Ok(Self {
            profile,
            required_suites: suites,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ConformanceTrustPolicy {
    release_policy: Option<ExpectedPolicyBinding>,
    required_profiles: BTreeMap<ConformanceProfile, BTreeSet<ConformanceSuiteId>>,
    allowed_environments: BTreeSet<ConformanceEnvironment>,
    max_evidence_age: Duration,
    trusted_keys: BTreeMap<ConformanceKeyId, VerifyingKey>,
}

impl ConformanceTrustPolicy {
    pub fn new<I, E>(
        release_policy: Option<ExpectedPolicyBinding>,
        required_profiles: I,
        allowed_environments: E,
        max_evidence_age: Duration,
    ) -> Result<Self, ConformanceResultError>
    where
        I: IntoIterator<Item = ConformanceProfileRequirement>,
        E: IntoIterator<Item = ConformanceEnvironment>,
    {
        if max_evidence_age <= Duration::zero() || max_evidence_age > MAX_TRUST_POLICY_AGE {
            return Err(ConformanceResultError::TrustPolicyInvalid);
        }
        let mut required = BTreeMap::new();
        for requirement in required_profiles {
            if required
                .insert(requirement.profile, requirement.required_suites)
                .is_some()
            {
                return Err(ConformanceResultError::TrustPolicyInvalid);
            }
        }
        if required.len() > MAX_PROFILE_RESULTS {
            return Err(ConformanceResultError::TrustPolicyInvalid);
        }
        if required.contains_key(&ConformanceProfile::Full)
            && [
                ConformanceProfile::Structural,
                ConformanceProfile::SchedulerReliability,
                ConformanceProfile::RuntimeAuthority,
                ConformanceProfile::Continuity,
                ConformanceProfile::Privacy,
                ConformanceProfile::Interoperability,
            ]
            .iter()
            .any(|component| !required.contains_key(component))
        {
            return Err(ConformanceResultError::TrustPolicyInvalid);
        }

        let mut environments = BTreeSet::new();
        for environment in allowed_environments {
            if !environments.insert(environment) {
                return Err(ConformanceResultError::TrustPolicyInvalid);
            }
        }

        let release_inventory_missing = required.is_empty() || environments.is_empty();
        let audit_inventory_present = !required.is_empty() || !environments.is_empty();
        if (release_policy.is_some() && release_inventory_missing)
            || (release_policy.is_none() && audit_inventory_present)
        {
            return Err(ConformanceResultError::TrustPolicyInvalid);
        }

        Ok(Self {
            release_policy,
            required_profiles: required,
            allowed_environments: environments,
            max_evidence_age,
            trusted_keys: BTreeMap::new(),
        })
    }

    pub fn trust_key(
        &mut self,
        key_id: impl Into<String>,
        verifying_key: VerifyingKey,
    ) -> Result<(), ConformanceResultError> {
        let key_id = ConformanceKeyId::new(key_id)?;
        if self.trusted_keys.contains_key(&key_id) {
            return Err(ConformanceResultError::TrustPolicyInvalid);
        }
        self.trusted_keys.insert(key_id, verifying_key);
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConformanceVerificationClass {
    AuditOnly,
    ReleaseEligibility,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedConformanceResult {
    result: ConformanceResult,
    classification: ConformanceVerificationClass,
    authentication_verified: bool,
}

impl VerifiedConformanceResult {
    #[must_use]
    pub const fn classification(&self) -> ConformanceVerificationClass {
        self.classification
    }

    #[must_use]
    pub const fn authentication_verified(&self) -> bool {
        self.authentication_verified
    }

    #[must_use]
    pub const fn result(&self) -> &ConformanceResult {
        &self.result
    }
}

pub fn verify_conformance_result(
    bytes: &[u8],
    expected: &ExpectedArtifactBinding,
    now: DateTime<Utc>,
    trust_policy: &ConformanceTrustPolicy,
) -> Result<VerifiedConformanceResult, ConformanceResultError> {
    let result: ConformanceResult =
        serde_json::from_slice(bytes).map_err(|_| ConformanceResultError::SchemaInvalid)?;
    let canonical_statement =
        canonicalize(&result.statement).map_err(|_| ConformanceResultError::SchemaInvalid)?;
    if sha256_hex(&canonical_statement) != result.statement_digest.value.as_str() {
        return Err(ConformanceResultError::StatementDigestMismatch);
    }

    verify_exact_binding(&result.statement, expected)?;
    let profile_statuses = validate_profile_semantics(&result.statement)?;

    let (classification, authentication_verified) = match &result.statement.decision_scope {
        ConformanceDecisionScope::AuditOnly => {
            let verified = if let Some(authentication) = &result.authentication {
                verify_authentication(authentication, &canonical_statement, trust_policy)?;
                true
            } else {
                false
            };
            (ConformanceVerificationClass::AuditOnly, verified)
        }
        ConformanceDecisionScope::ReleaseEligibility { policy_binding } => {
            let expected_policy = trust_policy
                .release_policy
                .as_ref()
                .ok_or(ConformanceResultError::ReleasePolicyMissing)?;
            if policy_binding.policy_id != expected_policy.policy_id
                || policy_binding.policy_version != expected_policy.policy_version
                || policy_binding.digest != expected_policy.digest
            {
                return Err(ConformanceResultError::PolicyBindingMismatch);
            }

            if !trust_policy
                .allowed_environments
                .contains(&result.statement.environment)
            {
                return Err(ConformanceResultError::EnvironmentNotAllowed);
            }

            verify_release_timing(&result.statement, now, trust_policy.max_evidence_age)?;
            for (required, required_suites) in &trust_policy.required_profiles {
                let status = profile_statuses
                    .get(required)
                    .ok_or(ConformanceResultError::RequiredProfileMissing)?;
                if *status != ConformanceStatus::Passed {
                    return Err(ConformanceResultError::RequiredProfileNotPassed);
                }
                let profile_result = result
                    .statement
                    .profile_results
                    .iter()
                    .find(|profile_result| profile_result.profile == *required)
                    .ok_or(ConformanceResultError::RequiredProfileMissing)?;
                if profile_result.required_suites.len() != required_suites.len()
                    || profile_result
                        .required_suites
                        .iter()
                        .any(|suite_id| !required_suites.contains(suite_id))
                {
                    return Err(ConformanceResultError::RequiredSuiteSetMismatch);
                }
            }

            let authentication = result
                .authentication
                .as_ref()
                .ok_or(ConformanceResultError::AuthenticationRequired)?;
            verify_authentication(authentication, &canonical_statement, trust_policy)?;
            (ConformanceVerificationClass::ReleaseEligibility, true)
        }
    };

    Ok(VerifiedConformanceResult {
        result,
        classification,
        authentication_verified,
    })
}

fn verify_exact_binding(
    statement: &ConformanceResultStatement,
    expected: &ExpectedArtifactBinding,
) -> Result<(), ConformanceResultError> {
    if statement.source.repository != expected.source.repository {
        return Err(ConformanceResultError::SourceRepositoryMismatch);
    }
    if statement.source.commit != expected.source.commit {
        return Err(ConformanceResultError::SourceCommitMismatch);
    }
    if statement.protocol_artifact.bundle_schema_version
        != expected.protocol_artifact.bundle_schema_version
    {
        return Err(ConformanceResultError::BundleSchemaVersionMismatch);
    }
    if statement.protocol_artifact.source_commit != statement.source.commit
        || statement.protocol_artifact.source_commit != expected.protocol_artifact.source_commit
    {
        return Err(ConformanceResultError::BundleSourceCommitMismatch);
    }
    if statement.protocol_artifact.bundle_sha256 != expected.protocol_artifact.bundle_sha256 {
        return Err(ConformanceResultError::BundleDigestMismatch);
    }
    if statement.protocol_artifact.contract_content_sha256
        != expected.protocol_artifact.contract_content_sha256
    {
        return Err(ConformanceResultError::ContractContentDigestMismatch);
    }
    if statement.protocol_artifact.file_count != expected.protocol_artifact.file_count {
        return Err(ConformanceResultError::FileCountMismatch);
    }
    if statement.runner.name != expected.runner.name {
        return Err(ConformanceResultError::RunnerNameMismatch);
    }
    if statement.runner.version != expected.runner.version {
        return Err(ConformanceResultError::RunnerVersionMismatch);
    }
    if statement.runner.artifact_sha256 != expected.runner.artifact_sha256 {
        return Err(ConformanceResultError::RunnerArtifactDigestMismatch);
    }
    if statement.runner.vector_set_sha256 != expected.runner.vector_set_sha256 {
        return Err(ConformanceResultError::VectorSetDigestMismatch);
    }
    Ok(())
}

fn validate_profile_semantics(
    statement: &ConformanceResultStatement,
) -> Result<BTreeMap<ConformanceProfile, ConformanceStatus>, ConformanceResultError> {
    if statement.profile_results.is_empty() || statement.profile_results.len() > MAX_PROFILE_RESULTS
    {
        return Err(ConformanceResultError::SchemaInvalid);
    }

    let mut statuses = BTreeMap::new();
    for profile_result in &statement.profile_results {
        if statuses
            .insert(profile_result.profile, profile_result.status)
            .is_some()
        {
            return Err(ConformanceResultError::ProfileDuplicate);
        }
        validate_profile_result(profile_result)?;
    }

    if derive_overall_status(statuses.values().copied()) != statement.overall_status {
        return Err(ConformanceResultError::OverallStatusInconsistent);
    }

    if statuses.get(&ConformanceProfile::Full) == Some(&ConformanceStatus::Passed) {
        for component in [
            ConformanceProfile::Structural,
            ConformanceProfile::SchedulerReliability,
            ConformanceProfile::RuntimeAuthority,
            ConformanceProfile::Continuity,
            ConformanceProfile::Privacy,
            ConformanceProfile::Interoperability,
        ] {
            if statuses.get(&component) != Some(&ConformanceStatus::Passed) {
                return Err(ConformanceResultError::FullProfileIncomplete);
            }
        }
    }

    Ok(statuses)
}

fn validate_profile_result(
    profile: &ConformanceProfileResult,
) -> Result<(), ConformanceResultError> {
    if profile.required_suites.is_empty()
        || profile.required_suites.len() > MAX_SUITES_PER_PROFILE
        || profile.suite_results.len() > MAX_SUITES_PER_PROFILE
    {
        return Err(ConformanceResultError::SchemaInvalid);
    }

    let mut required = BTreeSet::new();
    for suite_id in &profile.required_suites {
        if !required.insert(suite_id) {
            return Err(ConformanceResultError::SuiteDuplicate);
        }
    }

    let mut observed = BTreeSet::new();
    for suite in &profile.suite_results {
        if !observed.insert(&suite.suite_id) {
            return Err(ConformanceResultError::SuiteDuplicate);
        }
        if suite.status == ConformanceStatus::Passed && suite.evidence_digest.is_none() {
            return Err(ConformanceResultError::PassedSuiteEvidenceMissing);
        }
    }
    if required != observed {
        return Err(ConformanceResultError::SuiteSetMismatch);
    }

    let statuses = profile
        .suite_results
        .iter()
        .map(|suite| suite.status)
        .collect::<Vec<_>>();
    let consistent = match profile.status {
        ConformanceStatus::Passed => statuses
            .iter()
            .all(|status| *status == ConformanceStatus::Passed),
        ConformanceStatus::Failed => statuses.contains(&ConformanceStatus::Failed),
        ConformanceStatus::Incomplete => {
            !statuses.contains(&ConformanceStatus::Failed)
                && statuses
                    .iter()
                    .any(|status| *status != ConformanceStatus::Passed)
                && statuses
                    .iter()
                    .any(|status| *status != ConformanceStatus::NotApplicable)
        }
        ConformanceStatus::NotApplicable => statuses
            .iter()
            .all(|status| *status == ConformanceStatus::NotApplicable),
    };
    if !consistent {
        return Err(ConformanceResultError::ProfileStatusInconsistent);
    }
    Ok(())
}

fn derive_overall_status(
    statuses: impl IntoIterator<Item = ConformanceStatus>,
) -> ConformanceStatus {
    let statuses = statuses.into_iter().collect::<Vec<_>>();
    if statuses.contains(&ConformanceStatus::Failed) {
        ConformanceStatus::Failed
    } else if statuses
        .iter()
        .all(|status| *status == ConformanceStatus::Passed)
    {
        ConformanceStatus::Passed
    } else if statuses
        .iter()
        .all(|status| *status == ConformanceStatus::NotApplicable)
    {
        ConformanceStatus::NotApplicable
    } else {
        ConformanceStatus::Incomplete
    }
}

fn verify_release_timing(
    statement: &ConformanceResultStatement,
    now: DateTime<Utc>,
    max_age: Duration,
) -> Result<(), ConformanceResultError> {
    let observed_at = statement.observed_at.as_datetime()?;
    let expires_at = statement
        .expires_at
        .as_ref()
        .ok_or(ConformanceResultError::EvidenceExpirationRequired)?
        .as_datetime()?;
    if expires_at <= observed_at {
        return Err(ConformanceResultError::EvidenceChronologyInvalid);
    }
    if observed_at > now {
        return Err(ConformanceResultError::EvidenceNotYetValid);
    }
    if now >= expires_at {
        return Err(ConformanceResultError::EvidenceExpired);
    }
    if now.signed_duration_since(observed_at) > max_age {
        return Err(ConformanceResultError::EvidenceStale);
    }
    Ok(())
}

fn verify_authentication(
    authentication: &ConformanceAuthentication,
    canonical_statement: &[u8],
    trust_policy: &ConformanceTrustPolicy,
) -> Result<(), ConformanceResultError> {
    let verifying_key = trust_policy
        .trusted_keys
        .get(&authentication.key_id)
        .ok_or(ConformanceResultError::AuthenticationKeyUntrusted)?;
    let signature_bytes = URL_SAFE_NO_PAD
        .decode(authentication.signature.as_str())
        .map_err(|_| ConformanceResultError::AuthenticationEncodingInvalid)?;
    if URL_SAFE_NO_PAD.encode(&signature_bytes) != authentication.signature.as_str() {
        return Err(ConformanceResultError::AuthenticationEncodingInvalid);
    }
    let signature = Signature::from_der(&signature_bytes)
        .map_err(|_| ConformanceResultError::AuthenticationEncodingInvalid)?;
    if signature.to_der().as_bytes() != signature_bytes {
        return Err(ConformanceResultError::AuthenticationEncodingInvalid);
    }
    verifying_key
        .verify(canonical_statement, &signature)
        .map_err(|_| ConformanceResultError::AuthenticationInvalid)
}
