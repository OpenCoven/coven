//! Frozen error envelope and status mapping for Automations v1.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::types::{
    deserialize_non_null_option, AdoptionKey, ErrorMessage, PositiveInteger, StringConstraintError,
};

/// Every error code frozen by `error-envelope.schema.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    #[serde(rename = "SCHEMA_VERSION_UNSUPPORTED")]
    SchemaVersionUnsupported,
    #[serde(rename = "VALIDATION_FAILED")]
    ValidationFailed,
    #[serde(rename = "ADOPTION_REPLAY_MISMATCH")]
    AdoptionReplayMismatch,
    #[serde(rename = "REVISION_CONFLICT")]
    RevisionConflict,
    #[serde(rename = "NOT_FOUND")]
    NotFound,
    #[serde(rename = "GONE_TOMBSTONED")]
    GoneTombstoned,
    #[serde(rename = "CAPABILITY_UNSUPPORTED")]
    CapabilityUnsupported,
    #[serde(rename = "ILLEGAL_TRANSITION")]
    IllegalTransition,
    #[serde(rename = "AUTHORITY_REQUIRED")]
    AuthorityRequired,
    #[serde(rename = "APPROVAL_REQUIRED")]
    ApprovalRequired,
    #[serde(rename = "CANCEL_PENDING")]
    CancelPending,
    #[serde(rename = "OVERLAP_FORBIDDEN")]
    OverlapForbidden,
    #[serde(rename = "RETRY_DISPOSITION_INVALID")]
    RetryDispositionInvalid,
    #[serde(rename = "AMBIGUOUS_RETRY_FORBIDDEN")]
    AmbiguousRetryForbidden,
    #[serde(rename = "CURSOR_EXPIRED")]
    CursorExpired,
    #[serde(rename = "STREAM_OUT_OF_ORDER")]
    StreamOutOfOrder,
    #[serde(rename = "PAYLOAD_TOO_LARGE")]
    PayloadTooLarge,
    #[serde(rename = "DEADLINE_EXCEEDED")]
    DeadlineExceeded,
    #[serde(rename = "CONCURRENCY_LIMIT")]
    ConcurrencyLimit,
    #[serde(rename = "INTERNAL")]
    Internal,
}

impl ErrorCode {
    /// The HTTP status pinned for this code by the published contract.
    #[must_use]
    pub const fn http_status(self) -> u16 {
        match self {
            Self::SchemaVersionUnsupported | Self::ValidationFailed => 400,
            Self::AdoptionReplayMismatch
            | Self::RevisionConflict
            | Self::CancelPending
            | Self::OverlapForbidden
            | Self::StreamOutOfOrder => 409,
            Self::NotFound => 404,
            Self::GoneTombstoned | Self::CursorExpired => 410,
            Self::CapabilityUnsupported
            | Self::IllegalTransition
            | Self::RetryDispositionInvalid
            | Self::AmbiguousRetryForbidden => 422,
            Self::AuthorityRequired | Self::ApprovalRequired => 403,
            Self::PayloadTooLarge => 413,
            Self::DeadlineExceeded => 504,
            Self::ConcurrencyLimit => 429,
            Self::Internal => 500,
        }
    }

    /// The complete frozen v1 error vocabulary.
    pub const ALL: [Self; 20] = [
        Self::SchemaVersionUnsupported,
        Self::ValidationFailed,
        Self::AdoptionReplayMismatch,
        Self::RevisionConflict,
        Self::NotFound,
        Self::GoneTombstoned,
        Self::CapabilityUnsupported,
        Self::IllegalTransition,
        Self::AuthorityRequired,
        Self::ApprovalRequired,
        Self::CancelPending,
        Self::OverlapForbidden,
        Self::RetryDispositionInvalid,
        Self::AmbiguousRetryForbidden,
        Self::CursorExpired,
        Self::StreamOutOfOrder,
        Self::PayloadTooLarge,
        Self::DeadlineExceeded,
        Self::ConcurrencyLimit,
        Self::Internal,
    ];
}

/// A typed error cannot be constructed with a status that differs from its code.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorEnvelope {
    code: ErrorCode,
    http_status: u16,
    pub message: ErrorMessage,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<BTreeMap<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub adoption: Option<ErrorAdoption>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_revision: Option<PositiveInteger>,
}

impl ErrorEnvelope {
    #[must_use]
    pub fn new(code: ErrorCode, message: ErrorMessage, retryable: bool) -> Self {
        Self {
            code,
            http_status: code.http_status(),
            message,
            retryable,
            details: None,
            adoption: None,
            current_revision: None,
        }
    }

    pub fn try_new(
        code: ErrorCode,
        message: impl Into<String>,
        retryable: bool,
    ) -> Result<Self, StringConstraintError> {
        Ok(Self::new(
            code,
            ErrorMessage::new(message.into())?,
            retryable,
        ))
    }

    #[must_use]
    pub const fn code(&self) -> ErrorCode {
        self.code
    }

    #[must_use]
    pub const fn http_status(&self) -> u16 {
        self.http_status
    }

    #[must_use]
    pub fn with_details(mut self, details: BTreeMap<String, Value>) -> Self {
        self.details = Some(details);
        self
    }

    #[must_use]
    pub fn with_adoption(mut self, adoption: ErrorAdoption) -> Self {
        self.adoption = Some(adoption);
        self
    }

    #[must_use]
    pub fn with_current_revision(mut self, current_revision: PositiveInteger) -> Self {
        self.current_revision = Some(current_revision);
        self
    }
}

impl<'de> Deserialize<'de> for ErrorEnvelope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct Raw {
            code: ErrorCode,
            http_status: u16,
            message: ErrorMessage,
            retryable: bool,
            #[serde(default, deserialize_with = "deserialize_non_null_option")]
            details: Option<BTreeMap<String, Value>>,
            #[serde(default, deserialize_with = "deserialize_non_null_option")]
            adoption: Option<ErrorAdoption>,
            #[serde(default, deserialize_with = "deserialize_non_null_option")]
            current_revision: Option<PositiveInteger>,
        }

        let raw = Raw::deserialize(deserializer)?;
        if raw.http_status != raw.code.http_status() {
            let code = serde_json::to_string(&raw.code).unwrap_or_else(|_| "error code".to_owned());
            return Err(serde::de::Error::custom(format!(
                "{} requires HTTP status {}",
                code.trim_matches('"'),
                raw.code.http_status()
            )));
        }
        Ok(Self {
            code: raw.code,
            http_status: raw.http_status,
            message: raw.message,
            retryable: raw.retryable,
            details: raw.details,
            adoption: raw.adoption,
            current_revision: raw.current_revision,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorAdoption {
    pub key: AdoptionKey,
    #[serde(
        default,
        deserialize_with = "deserialize_non_null_option",
        skip_serializing_if = "Option::is_none"
    )]
    pub conflict_outcome: Option<AdoptionConflictOutcome>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptionConflictOutcome {
    Committed,
    Rejected,
}
