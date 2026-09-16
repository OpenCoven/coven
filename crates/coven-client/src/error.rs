use std::io;

use serde_json::Value;
use thiserror::Error;

pub(crate) const UNIX_CONNECT_OPERATION: &str = "failed to connect to Coven daemon socket";
pub(crate) const UNIX_CONFIGURE_WRITES_OPERATION: &str =
    "failed to configure nonblocking Coven daemon socket writes";
#[doc(hidden)]
pub const WINDOWS_CONNECT_OPERATION: &str = "failed to connect to Coven daemon pipe";
pub(crate) const WINDOWS_CONFIGURE_WRITES_OPERATION: &str =
    "failed to configure nonblocking Coven daemon pipe writes";
#[doc(hidden)]
pub const EMPTY_RESPONSE_TIMEOUT_MESSAGE: &str = "no response bytes arrived before the deadline";
/// Prefix of the Unix transport's response-deadline diagnostic. The rest of
/// that message carries counts and monotonic durations, so it is never stable
/// enough to compare whole.
#[doc(hidden)]
pub const RESPONSE_READ_TIMEOUT_PREFIX: &str = "timed out reading Coven daemon response";

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

impl ClientError {
    /// Return whether the transport proved that no request bytes were sent.
    pub fn request_was_definitely_not_sent(&self) -> bool {
        matches!(
            self,
            Self::Io {
                operation: UNIX_CONNECT_OPERATION
                    | UNIX_CONFIGURE_WRITES_OPERATION
                    | WINDOWS_CONNECT_OPERATION
                    | WINDOWS_CONFIGURE_WRITES_OPERATION,
                ..
            } | Self::DaemonInstanceChanged
        )
    }
}

impl std::fmt::Display for DaemonError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn io_error(operation: &'static str) -> ClientError {
        ClientError::Io {
            operation,
            source: io::Error::from(io::ErrorKind::ConnectionReset),
        }
    }

    #[test]
    fn request_delivery_classification_is_exact() {
        for operation in [
            UNIX_CONNECT_OPERATION,
            UNIX_CONFIGURE_WRITES_OPERATION,
            WINDOWS_CONNECT_OPERATION,
            WINDOWS_CONFIGURE_WRITES_OPERATION,
        ] {
            assert!(io_error(operation).request_was_definitely_not_sent());
        }

        assert!(ClientError::DaemonInstanceChanged.request_was_definitely_not_sent());
        assert!(!io_error("failed to write Coven daemon request").request_was_definitely_not_sent());
        assert!(!io_error("failed to connect to Coven daemon socket later")
            .request_was_definitely_not_sent());
    }
}

/// Whether `error` is a response-deadline timeout, in any of the spellings the
/// transports produce.
///
/// The two platforms report the same condition differently: Windows emits the
/// bare [`EMPTY_RESPONSE_TIMEOUT_MESSAGE`], while the Unix transport emits a
/// diagnostic carrying phase, byte counts, and elapsed micros. Both land in
/// [`ClientError::InvalidHttpResponse`], whose payload is a string, so callers
/// that need to tell "the deadline elapsed" from "the response was malformed"
/// were left comparing messages by hand — and a caller that only knew the
/// Windows spelling silently stopped matching on Unix once the diagnostics
/// grew.
///
/// Keeping the predicate next to the code that builds those messages is the
/// point: it cannot drift out of sync with them.
pub fn is_response_deadline_timeout(error: &ClientError) -> bool {
    match error {
        ClientError::Io { source, .. } => source.kind() == io::ErrorKind::TimedOut,
        ClientError::InvalidHttpResponse(message) => {
            message == EMPTY_RESPONSE_TIMEOUT_MESSAGE
                || message.starts_with(RESPONSE_READ_TIMEOUT_PREFIX)
        }
        _ => false,
    }
}

#[cfg(test)]
mod response_deadline_tests {
    use super::*;

    #[test]
    fn recognizes_both_transport_spellings() {
        assert!(is_response_deadline_timeout(
            &ClientError::InvalidHttpResponse(EMPTY_RESPONSE_TIMEOUT_MESSAGE.to_owned())
        ));
        // The Unix diagnostic, verbatim from a real CI failure.
        assert!(is_response_deadline_timeout(
            &ClientError::InvalidHttpResponse(
                "timed out reading Coven daemon response (request_kind=lifecycle-health; \
             phase=headers; received_bytes=0; expected_body_bytes=None; \
             request_budget_us=249999; request_elapsed_us=250031; read_elapsed_us=82767; \
             deadline_overrun_us=32)"
                    .to_owned()
            )
        ));
        assert!(is_response_deadline_timeout(&ClientError::Io {
            operation: "read",
            source: io::Error::from(io::ErrorKind::TimedOut),
        }));
    }

    #[test]
    fn does_not_swallow_other_invalid_responses() {
        // A malformed response must stay fatal; only the deadline is retryable.
        for message in [
            "missing status line",
            "unsupported transfer encoding",
            "response exceeded the declared body length",
        ] {
            assert!(
                !is_response_deadline_timeout(&ClientError::InvalidHttpResponse(
                    message.to_owned()
                )),
                "{message:?} must not read as a timeout"
            );
        }
        assert!(!is_response_deadline_timeout(&ClientError::HealthNotReady));
        assert!(!is_response_deadline_timeout(&ClientError::Io {
            operation: "connect",
            source: io::Error::from(io::ErrorKind::ConnectionRefused),
        }));
    }
}
