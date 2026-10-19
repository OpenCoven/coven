//! Bounded private control framing, separate from subject stdout/stderr.
use crate::Failure;

pub const MAX_CONTROL_BYTES: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    #[serde(rename = "launch_invalid")]
    InvalidLaunch,
    ContainmentUnavailable,
    ControllerLost,
    DeadlineExceeded,
    IoRefused,
    OutputLimitExceeded,
    CleanupUnproven,
}

#[derive(serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Payload {
    Ready,
    Exited {
        #[serde(rename = "exitCode")]
        exit_code: u32,
    },
    Refused {
        failure: Refusal,
    },
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Frame<'a> {
    schema_version: &'static str,
    nonce: &'a str,
    #[serde(flatten)]
    payload: Payload,
}

fn encode(nonce: &str, payload: Payload) -> Result<Vec<u8>, Failure> {
    if nonce.len() != 32
        || !nonce
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(Failure::InvalidLaunch);
    }
    // All materialized fields are fixed enums, bounded integers or this exact
    // nonce. No subject path, argument, payload or native error is accepted.
    let mut bytes = serde_json::to_vec(&Frame {
        schema_version: crate::ABI,
        nonce,
        payload,
    })
    .map_err(|_| Failure::InvalidLaunch)?;
    if bytes.len() >= MAX_CONTROL_BYTES {
        return Err(Failure::InvalidLaunch);
    }
    bytes.push(b'\n');
    Ok(bytes)
}

pub fn ready(nonce: &str) -> Result<Vec<u8>, Failure> {
    encode(nonce, Payload::Ready)
}

pub fn exited(nonce: &str, code: u32) -> Result<Vec<u8>, Failure> {
    encode(nonce, Payload::Exited { exit_code: code })
}

pub fn refused(nonce: &str, reason: Refusal) -> Result<Vec<u8>, Failure> {
    encode(nonce, Payload::Refused { failure: reason })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    const NONCE: &str = "0123456789abcdef0123456789abcdef";

    fn decoded(bytes: Vec<u8>) -> Value {
        assert!(bytes.len() <= MAX_CONTROL_BYTES);
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(bytes.iter().filter(|&&b| b == b'\n').count(), 1);
        serde_json::from_slice(&bytes[..bytes.len() - 1]).unwrap()
    }

    #[test]
    fn ready_has_only_version_kind_and_exact_nonce() {
        assert_eq!(
            decoded(ready(NONCE).unwrap()),
            json!({
                "schemaVersion":crate::ABI, "kind":"ready", "nonce":NONCE
            })
        );
    }

    #[test]
    fn exited_preserves_the_entire_unsigned_native_exit_code() {
        for code in [0, 1, i32::MAX as u32, i32::MAX as u32 + 1, u32::MAX] {
            assert_eq!(
                decoded(exited(NONCE, code).unwrap()),
                json!({
                    "schemaVersion":crate::ABI, "kind":"exited", "nonce":NONCE, "exitCode":code
                })
            );
        }
    }

    #[test]
    fn refusals_are_closed_fixed_codes_without_subject_or_controller_data() {
        for (reason, code) in [
            (Refusal::InvalidLaunch, "launch_invalid"),
            (Refusal::ContainmentUnavailable, "containment_unavailable"),
            (Refusal::ControllerLost, "controller_lost"),
            (Refusal::DeadlineExceeded, "deadline_exceeded"),
            (Refusal::IoRefused, "io_refused"),
            (Refusal::OutputLimitExceeded, "output_limit_exceeded"),
            (Refusal::CleanupUnproven, "cleanup_unproven"),
        ] {
            assert_eq!(
                decoded(refused(NONCE, reason).unwrap()),
                json!({
                    "schemaVersion":crate::ABI, "kind":"refused", "nonce":NONCE, "failure":code
                })
            );
        }
    }

    #[test]
    fn invalid_nonce_is_never_reflected_by_any_encoder() {
        for nonce in [
            "",
            "PRIVATE_CONTROLLER",
            "0123456789ABCDEF0123456789ABCDEF",
            "0123456789abcdef0123456789abcde\n",
            "0123456789abcdef0123456789abcdef0",
        ] {
            for result in [
                ready(nonce),
                exited(nonce, 0),
                refused(nonce, Refusal::CleanupUnproven),
            ] {
                let error = result.unwrap_err();
                assert_eq!(error, Failure::InvalidLaunch);
                assert_eq!(error.to_string(), "launch_invalid");
            }
        }
    }
}
