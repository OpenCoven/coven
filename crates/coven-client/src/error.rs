use std::io;

use serde_json::Value;
use thiserror::Error;

#[derive(Clone, Debug, PartialEq)]
pub struct DaemonError {
    pub code: String,
    pub message: String,
    pub details: Value,
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("failed to discover owner-local Coven endpoint: {0}")]
    Discovery(String),
    #[error("{operation}: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("Coven daemon response exceeded the {max_bytes}-byte body limit")]
    ResponseTooLarge { max_bytes: usize },
    #[error(
        "Coven daemon request body of {actual_bytes} bytes exceeded the {max_bytes}-byte limit"
    )]
    RequestTooLarge {
        max_bytes: usize,
        actual_bytes: usize,
    },
    #[error("invalid Coven daemon HTTP response: {0}")]
    InvalidHttpResponse(String),
    #[error("Coven daemon response was not valid UTF-8")]
    InvalidUtf8(#[source] std::string::FromUtf8Error),
    #[error("failed to parse Coven daemon response: {0}")]
    InvalidJson(#[source] serde_json::Error),
    #[error("Coven daemon API mismatch: expected {expected}, got {actual}")]
    ProtocolVersion {
        expected: &'static str,
        actual: String,
    },
    #[error("Coven daemon does not advertise capabilities.structuredErrors")]
    StructuredErrorsUnavailable,
    #[error("Coven daemon does not advertise required capabilities.{capability}")]
    CapabilityUnavailable { capability: &'static str },
    #[error("Coven daemon health reported not ready")]
    HealthNotReady,
    #[error("Coven daemon instance changed; health negotiation is required")]
    DaemonInstanceChanged,
    #[error("Coven daemon rejected request with HTTP {status}: {error}")]
    Daemon { status: u16, error: DaemonError },
    #[error("Coven daemon rejected request with HTTP {0}")]
    HttpStatus(u16),
    #[error("invalid Coven API route parameter: {0}")]
    InvalidRouteParameter(&'static str),
    #[error(
        "cannot safely stop a legacy BASE Coven daemon on {platform}: identity-bound process \
         signaling is unavailable; upgrade Coven and retry, or restart the daemon manually"
    )]
    LegacyShutdownUpgradeRequired { platform: &'static str },
    #[error("Coven daemon client is not implemented on this platform")]
    UnsupportedPlatform,
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}
