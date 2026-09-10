use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub use super::assurance::{AssuranceClass, StepUpAuthorizationEnrollment};
use super::MOBILE_PROTOCOL_VERSION;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileEnvelope<T> {
    pub ok: bool,
    pub protocol_version: u16,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<MobileError>,
}

impl<T> MobileEnvelope<T> {
    pub fn success(request_id: String, data: T) -> Self {
        Self {
            ok: true,
            protocol_version: MOBILE_PROTOCOL_VERSION,
            request_id,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(request_id: String, code: MobileErrorCode) -> Self {
        Self {
            ok: false,
            protocol_version: MOBILE_PROTOCOL_VERSION,
            request_id,
            data: None,
            error: Some(MobileError {
                code,
                retryable: code.is_retryable(),
            }),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileError {
    pub code: MobileErrorCode,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileErrorCode {
    InvalidRequest,
    PairingExpired,
    PairingConsumed,
    PairingConfirmationRequired,
    PairingPhraseMismatch,
    DeviceUnknown,
    DeviceRevoked,
    SignatureInvalid,
    RequestExpired,
    RequestReplayed,
    RateLimited,
    AssuranceRequired,
    ProtocolUnsupported,
    CapabilityUnavailable,
    MemoryNotFound,
    MemoryContentTooLarge,
    MemoryContentInvalid,
    MemoryContentUnavailable,
    DaemonUnavailable,
    ResponseInvalid,
    GatewayDisabled,
}

impl MobileErrorCode {
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::PairingConfirmationRequired
                | Self::RequestExpired
                | Self::RequestReplayed
                | Self::RateLimited
                | Self::AssuranceRequired
                | Self::MemoryContentUnavailable
                | Self::DaemonUnavailable
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileCapabilities {
    pub minimum_protocol_version: u16,
    pub current_protocol_version: u16,
    pub maximum_protocol_version: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileMemorySource {
    pub kind: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileMemoryPrivacySummary {
    pub classification: Option<String>,
    pub reveal_required: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MobileVerificationState {
    Verified,
    NeedsReview,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileMemoryVerificationSummary {
    pub state: MobileVerificationState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileMemorySummary {
    pub id: Uuid,
    pub familiar_id: String,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    pub relative_updated_at: String,
    pub excerpt: String,
    pub source: MobileMemorySource,
    pub privacy: MobileMemoryPrivacySummary,
    pub verification: MobileMemoryVerificationSummary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileOverview {
    pub generated_at: DateTime<Utc>,
    pub totals: MobileOverviewTotals,
    pub last_updated_at: Option<DateTime<Utc>>,
    pub capabilities: MobileMemoryCapabilities,
    pub verification: MobileOverviewVerification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileOverviewTotals {
    pub entries: usize,
    pub familiars: usize,
    pub verified: usize,
    pub needs_review: usize,
    pub unknown: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileMemoryCapabilities {
    pub detail: bool,
    pub verification: bool,
    pub attestation_metadata: bool,
    pub supersession_history: bool,
    pub mutations: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileOverviewVerification {
    pub state: MobileVerificationState,
    pub checked_at: DateTime<Utc>,
    pub manifest: String,
    pub index: String,
    pub issues: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileMemoryPrivacyDetail {
    pub classification: Option<String>,
    pub reveal_required: Option<bool>,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileMemoryVerificationDetail {
    pub state: MobileVerificationState,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileAttestationMetadata {
    pub field_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileSupersession {
    pub supersedes: Option<Uuid>,
    pub superseded_by: Option<Uuid>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileMemoryDetail {
    pub id: Uuid,
    pub familiar_id: String,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    pub source: MobileMemorySource,
    pub content: String,
    pub content_format: MobileContentFormat,
    pub privacy: MobileMemoryPrivacyDetail,
    pub verification: MobileMemoryVerificationDetail,
    pub attestation_metadata: Option<MobileAttestationMetadata>,
    pub supersession: MobileSupersession,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MobileContentFormat {
    Markdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MobileProtocolRange {
    pub minimum: u16,
    pub maximum: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MobilePairingRequest {
    pub protocol_version: u16,
    pub pairing_nonce: String,
    pub device_name: String,
    pub device_public_key: String,
    pub app_version: String,
    pub supported_protocol: MobileProtocolRange,
    pub step_up_authorization: Option<StepUpAuthorizationEnrollment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePendingPairing {
    pub pairing_id: Uuid,
    pub phrase: Vec<String>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobileAssuranceChallenge {
    pub challenge: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MobilePairingConfirmation {
    pub phrase: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobileDeviceScope {
    MemoryRead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePairedDevice {
    pub id: Uuid,
    pub display_name: String,
    pub paired_at: DateTime<Utc>,
    pub scopes: Vec<MobileDeviceScope>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_mobile_detail() -> MobileMemoryDetail {
        MobileMemoryDetail {
            id: Uuid::nil(),
            familiar_id: "synthetic".to_owned(),
            title: "Synthetic note".to_owned(),
            updated_at: DateTime::from_timestamp(1_785_326_400, 0).unwrap(),
            source: MobileMemorySource {
                kind: "coven-origin".to_owned(),
                label: "Coven origin".to_owned(),
            },
            content: "# Synthetic note".to_owned(),
            content_format: MobileContentFormat::Markdown,
            privacy: MobileMemoryPrivacyDetail {
                classification: Some("public".to_owned()),
                reveal_required: Some(false),
                reason: "Synthetic fixture is public.".to_owned(),
            },
            verification: MobileMemoryVerificationDetail {
                state: MobileVerificationState::Verified,
                reason: "Synthetic fixture verification passed.".to_owned(),
            },
            attestation_metadata: Some(MobileAttestationMetadata { field_count: 2 }),
            supersession: MobileSupersession {
                supersedes: None,
                superseded_by: None,
            },
        }
    }

    #[test]
    fn mobile_detail_omits_paths_and_attestation_values() {
        let encoded = serde_json::to_value(sample_mobile_detail()).unwrap();
        assert!(encoded.get("path").is_none());
        assert!(encoded.get("attestation").is_none());
        assert_eq!(encoded["attestationMetadata"]["fieldCount"], 2);
    }

    #[test]
    fn mobile_error_serializes_without_internal_prose() {
        let encoded = serde_json::to_value(MobileEnvelope::<()>::error(
            "01J00000000000000000000000".to_owned(),
            MobileErrorCode::DeviceRevoked,
        ))
        .unwrap();
        assert_eq!(encoded["error"]["code"], "device_revoked");
        assert!(encoded["error"].get("message").is_none());
    }

    #[test]
    fn pairing_step_up_enrollment_is_closed_and_requires_all_fields() {
        let valid = serde_json::json!({
            "protocolVersion": 2,
            "pairingNonce": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "deviceName": "Synthetic phone",
            "devicePublicKey": "synthetic",
            "appVersion": "1.0.0",
            "supportedProtocol": { "minimum": 1, "maximum": 2 },
            "stepUpAuthorization": {
                "publicKey": "synthetic-step-up",
                "assuranceClass": "biometric_only",
                "enrollmentSignature": "synthetic-signature"
            }
        });
        let parsed: MobilePairingRequest = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(
            parsed.step_up_authorization.unwrap().assurance_class,
            AssuranceClass::BiometricOnly
        );

        for invalid in [
            {
                let mut value = valid.clone();
                value["stepUpAuthorization"]
                    .as_object_mut()
                    .unwrap()
                    .remove("enrollmentSignature");
                value
            },
            {
                let mut value = valid.clone();
                value["stepUpAuthorization"]["assuranceClass"] =
                    serde_json::json!("recent_user_verification");
                value
            },
            {
                let mut value = valid;
                value["stepUpAuthorization"]["unexpected"] = serde_json::json!(true);
                value
            },
        ] {
            assert!(serde_json::from_value::<MobilePairingRequest>(invalid).is_err());
        }
    }
}
