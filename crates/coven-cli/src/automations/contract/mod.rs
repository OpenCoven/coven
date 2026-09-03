//! Typed projections of the published Coven Automations v1 protocol.

// The protocol surface lands before the command router that consumes it.
#[allow(dead_code)]
pub mod canonical_json;
// The typed envelope is part of the public contract before routing lands.
#[allow(dead_code)]
pub mod error;
// These projections are intentionally available to the following protocol slices.
#[allow(dead_code)]
pub mod types;

// This binary crate has no external consumer yet; these define the intended
// contract surface for the later routing and SDK integration slices.
#[allow(unused_imports)]
pub use canonical_json::{canonicalize, canonicalize_without_integrity, sha256_digest, sha256_hex};
#[allow(unused_imports)]
pub use error::{ErrorCode, ErrorEnvelope};
#[allow(unused_imports)]
pub use types::{
    AutomationAttempt, AutomationDefinition, AutomationOccurrence, AutomationReceipt,
    AutomationRun, CommandRequest, CommandResponse, EventEnvelope,
};

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;
    use serde::Serialize;
    use serde_json::{json, Value};

    use super::canonical_json::{
        canonicalize, canonicalize_without_integrity, sha256_digest, sha256_hex,
    };
    use super::error::{ErrorCode, ErrorEnvelope};
    use super::types::{
        AutomationAttempt, AutomationDefinition, AutomationOccurrence, AutomationReceipt,
        AutomationRun, CommandRequest, CommandResponse, EventEnvelope,
    };

    const VECTORS: &str =
        include_str!("../../../../../spec/coven-automations/v1/test-vectors.json");

    fn vectors() -> Value {
        serde_json::from_str(VECTORS).expect("checked-in test vectors must be JSON")
    }

    fn fixture(name: &str) -> Value {
        vectors()["fixtures"][name].clone()
    }

    fn event_fixture() -> Value {
        vectors()["cases"]
            .as_array()
            .expect("cases is an array")
            .iter()
            .find(|case| case["name"] == "event-golden-valid")
            .expect("event golden fixture is present")["object"]
            .clone()
    }

    fn assert_round_trip<T>(value: Value)
    where
        T: DeserializeOwned + Serialize,
    {
        let projection: T = serde_json::from_value(value.clone()).expect("valid projection");
        assert_eq!(
            serde_json::to_value(projection).expect("serialize projection"),
            value
        );
    }

    #[test]
    fn checked_in_schema_artifacts_and_representative_vectors_load() {
        for artifact in [
            "automation-definition.schema.json",
            "automation-occurrence.schema.json",
            "automation-run.schema.json",
            "automation-attempt.schema.json",
            "automation-receipt.schema.json",
            "command-envelope.schema.json",
            "error-envelope.schema.json",
            "event-envelope.schema.json",
        ] {
            let path = format!("../../../../../spec/coven-automations/v1/{artifact}");
            let schema: Value = match artifact {
                "automation-definition.schema.json" => serde_json::from_str(include_str!(
                    "../../../../../spec/coven-automations/v1/automation-definition.schema.json"
                )),
                "automation-occurrence.schema.json" => serde_json::from_str(include_str!(
                    "../../../../../spec/coven-automations/v1/automation-occurrence.schema.json"
                )),
                "automation-run.schema.json" => serde_json::from_str(include_str!(
                    "../../../../../spec/coven-automations/v1/automation-run.schema.json"
                )),
                "automation-attempt.schema.json" => serde_json::from_str(include_str!(
                    "../../../../../spec/coven-automations/v1/automation-attempt.schema.json"
                )),
                "automation-receipt.schema.json" => serde_json::from_str(include_str!(
                    "../../../../../spec/coven-automations/v1/automation-receipt.schema.json"
                )),
                "command-envelope.schema.json" => serde_json::from_str(include_str!(
                    "../../../../../spec/coven-automations/v1/command-envelope.schema.json"
                )),
                "error-envelope.schema.json" => serde_json::from_str(include_str!(
                    "../../../../../spec/coven-automations/v1/error-envelope.schema.json"
                )),
                "event-envelope.schema.json" => serde_json::from_str(include_str!(
                    "../../../../../spec/coven-automations/v1/event-envelope.schema.json"
                )),
                _ => unreachable!("artifact list is closed"),
            }
            .unwrap_or_else(|error| panic!("{path} must be JSON: {error}"));
            assert!(schema.get("title").is_some() || schema.get("$ref").is_some());
        }

        assert_round_trip::<AutomationDefinition>(fixture("definition.golden"));
        assert_round_trip::<AutomationOccurrence>(fixture("occurrence.golden"));
        assert_round_trip::<AutomationRun>(fixture("run.golden"));
        assert_round_trip::<AutomationAttempt>(fixture("attempt.golden"));
        assert_round_trip::<AutomationReceipt>(fixture("receipt.golden"));
        assert_round_trip::<CommandRequest>(fixture("command.create.golden"));
        assert_round_trip::<EventEnvelope>(event_fixture());
    }

    #[test]
    fn closed_projections_reject_unknown_fields() {
        let mut definition = fixture("definition.golden");
        definition["unknown"] = json!(true);
        assert!(serde_json::from_value::<AutomationDefinition>(definition).is_err());

        let mut nested_definition = fixture("definition.golden");
        nested_definition["display"]["unknown"] = json!(true);
        assert!(serde_json::from_value::<AutomationDefinition>(nested_definition).is_err());

        let mut occurrence = fixture("occurrence.golden");
        occurrence["unknown"] = json!(true);
        assert!(serde_json::from_value::<AutomationOccurrence>(occurrence).is_err());

        let mut run = fixture("run.golden");
        run["unknown"] = json!(true);
        assert!(serde_json::from_value::<AutomationRun>(run).is_err());

        let mut attempt = fixture("attempt.golden");
        attempt["unknown"] = json!(true);
        assert!(serde_json::from_value::<AutomationAttempt>(attempt).is_err());

        let mut receipt = fixture("receipt.golden");
        receipt["unknown"] = json!(true);
        assert!(serde_json::from_value::<AutomationReceipt>(receipt).is_err());

        let mut command = fixture("command.create.golden");
        command["unknown"] = json!(true);
        assert!(serde_json::from_value::<CommandRequest>(command).is_err());

        let mut event = event_fixture();
        event["unknown"] = json!(true);
        assert!(serde_json::from_value::<EventEnvelope>(event).is_err());

        let error = json!({
            "code": "NOT_FOUND",
            "httpStatus": 404,
            "message": "No such automation.",
            "retryable": false,
            "unknown": true
        });
        assert!(serde_json::from_value::<ErrorEnvelope>(error).is_err());

        let nested_error = json!({
            "code": "NOT_FOUND",
            "httpStatus": 404,
            "message": "No such automation.",
            "retryable": false,
            "adoption": {"key": "adopt:get-0001", "unknown": true}
        });
        assert!(serde_json::from_value::<ErrorEnvelope>(nested_error).is_err());
    }

    #[test]
    fn every_error_code_has_its_frozen_http_status() {
        let expected = [
            (ErrorCode::SchemaVersionUnsupported, 400),
            (ErrorCode::ValidationFailed, 400),
            (ErrorCode::AdoptionReplayMismatch, 409),
            (ErrorCode::RevisionConflict, 409),
            (ErrorCode::NotFound, 404),
            (ErrorCode::GoneTombstoned, 410),
            (ErrorCode::CapabilityUnsupported, 422),
            (ErrorCode::IllegalTransition, 422),
            (ErrorCode::AuthorityRequired, 403),
            (ErrorCode::ApprovalRequired, 403),
            (ErrorCode::CancelPending, 409),
            (ErrorCode::OverlapForbidden, 409),
            (ErrorCode::RetryDispositionInvalid, 422),
            (ErrorCode::AmbiguousRetryForbidden, 422),
            (ErrorCode::CursorExpired, 410),
            (ErrorCode::StreamOutOfOrder, 409),
            (ErrorCode::PayloadTooLarge, 413),
            (ErrorCode::DeadlineExceeded, 504),
            (ErrorCode::ConcurrencyLimit, 429),
            (ErrorCode::Internal, 500),
        ];
        assert_eq!(ErrorCode::ALL.len(), expected.len());

        for (code, status) in expected {
            let envelope = ErrorEnvelope::new(code, "safe message", false);
            assert_eq!(code.http_status(), status);
            assert_eq!(envelope.http_status(), status);

            let mut mismatched = serde_json::to_value(envelope).expect("serialize error");
            mismatched["httpStatus"] = json!(status + 1);
            assert!(serde_json::from_value::<ErrorEnvelope>(mismatched).is_err());
        }
    }

    #[test]
    fn pinned_jcs_sha256_vectors_match_and_key_order_is_ignored() {
        for name in ["definition.golden", "receipt.golden"] {
            let value = fixture(name);
            let expected = value["integrity"]["value"]
                .as_str()
                .expect("fixture integrity digest")
                .to_owned();
            let canonical =
                canonicalize_without_integrity(&value).expect("canonicalize digest preimage");
            assert_eq!(sha256_hex(&canonical), expected, "{name}");
        }

        let first = serde_json::from_str::<Value>(r#"{"z":1,"a":{"y":2,"x":3}}"#)
            .expect("first JSON object");
        let second = serde_json::from_str::<Value>(r#"{"a":{"x":3,"y":2},"z":1}"#)
            .expect("second JSON object");
        assert_eq!(
            sha256_digest(&first).expect("digest first"),
            sha256_digest(&second).expect("digest second")
        );
    }

    #[test]
    fn jcs_refuses_non_finite_numbers() {
        assert!(canonicalize(&f64::NAN).is_err());
        assert!(canonicalize(&f64::INFINITY).is_err());
        assert!(canonicalize(&f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn command_response_round_trips_a_typed_error() {
        let response = json!({
            "schemaVersion": "coven.automations.v1",
            "command": "definition.get.v1",
            "adoptionKey": "adopt:get-0001",
            "outcome": "rejected",
            "error": {
                "code": "NOT_FOUND",
                "httpStatus": 404,
                "message": "No such automation.",
                "retryable": false,
                "details": {"automationId": "missing"}
            }
        });
        assert_round_trip::<CommandResponse>(response);
        assert_round_trip::<ErrorEnvelope>(json!({
            "code": "NOT_FOUND",
            "httpStatus": 404,
            "message": "No such automation.",
            "retryable": false,
            "details": {"automationId": "missing"}
        }));
    }
}
