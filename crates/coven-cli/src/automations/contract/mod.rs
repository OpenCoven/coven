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
        canonicalize, canonicalize_without_integrity, sha256_digest, sha256_hex, MAX_SAFE_INTEGER,
    };
    use super::error::{ErrorCode, ErrorEnvelope};
    use super::types::{
        AutomationAttempt, AutomationDefinition, AutomationOccurrence, AutomationReceipt,
        AutomationRun, CommandRequest, CommandResponse, EventEnvelope, PositiveInteger,
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
            "common.schema.json",
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
                "common.schema.json" => serde_json::from_str(include_str!(
                    "../../../../../spec/coven-automations/v1/common.schema.json"
                )),
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

        let nested_integrity_a = json!({
            "integrity": {"ignored": "top-level"},
            "extensions": {"x-example": {"integrity": "nested-a"}}
        });
        let nested_integrity_b = json!({
            "integrity": {"ignored": "top-level"},
            "extensions": {"x-example": {"integrity": "nested-b"}}
        });
        let digest_a = sha256_hex(
            &canonicalize_without_integrity(&nested_integrity_a)
                .expect("canonicalize nested integrity collision"),
        );
        let digest_b = sha256_hex(
            &canonicalize_without_integrity(&nested_integrity_b)
                .expect("canonicalize changed nested integrity collision"),
        );
        assert_ne!(digest_a, digest_b);
    }

    #[test]
    fn jcs_refuses_non_finite_numbers() {
        assert!(canonicalize(&f64::NAN).is_err());
        assert!(canonicalize(&f64::INFINITY).is_err());
        assert!(canonicalize(&f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn jcs_rejects_integers_outside_the_safe_ieee_754_domain() {
        assert!(canonicalize(&MAX_SAFE_INTEGER).is_ok());
        assert!(PositiveInteger::new(MAX_SAFE_INTEGER).is_ok());

        let unsafe_integer = MAX_SAFE_INTEGER + 1;
        assert!(PositiveInteger::new(unsafe_integer).is_err());
        assert!(canonicalize(&unsafe_integer).is_err());
        assert!(canonicalize(&u64::MAX).is_err());
        assert!(sha256_digest(&unsafe_integer).is_err());
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

    #[test]
    fn command_payloads_and_expected_revisions_are_command_correlated() {
        let mut wrong_payload = fixture("command.create.golden");
        wrong_payload["command"] = json!("run.cancel.v1");
        assert!(serde_json::from_value::<CommandRequest>(wrong_payload).is_err());

        let mut missing_revision = fixture("command.create.golden");
        missing_revision["command"] = json!("definition.revise.v1");
        assert!(serde_json::from_value::<CommandRequest>(missing_revision).is_err());

        let mut forbidden_revision = fixture("command.create.golden");
        forbidden_revision["expectedRevision"] = json!(1);
        assert!(serde_json::from_value::<CommandRequest>(forbidden_revision).is_err());

        let mut zero_revision = fixture("command.create.golden");
        zero_revision["command"] = json!("definition.revise.v1");
        zero_revision["expectedRevision"] = json!(0);
        assert!(serde_json::from_value::<CommandRequest>(zero_revision).is_err());

        let mut unknown_payload_field = fixture("command.create.golden");
        unknown_payload_field["payload"]["unexpected"] = json!(true);
        assert!(serde_json::from_value::<CommandRequest>(unknown_payload_field).is_err());
    }

    #[test]
    fn receipt_optional_fields_remain_optional() {
        let mut receipt = fixture("receipt.golden");
        for field in [
            "definitionDigest",
            "occurrenceFenceGeneration",
            "attemptNumber",
            "runtime",
        ] {
            receipt
                .as_object_mut()
                .expect("receipt fixture is an object")
                .remove(field);
        }
        assert_round_trip::<AutomationReceipt>(receipt);
    }

    #[test]
    fn common_constraints_and_definition_conditionals_fail_closed() {
        let mut invalid_id = fixture("definition.golden");
        invalid_id["automationId"] = json!("-daily-notes");
        assert!(serde_json::from_value::<AutomationDefinition>(invalid_id).is_err());

        let mut invalid_timestamp = fixture("occurrence.golden");
        invalid_timestamp["scheduledFor"] = json!("2026-08-30T09:00:00+00:00");
        assert!(serde_json::from_value::<AutomationOccurrence>(invalid_timestamp).is_err());

        let mut invalid_extension = fixture("definition.golden");
        invalid_extension["extensions"] = json!({"not-namespaced": true});
        assert!(serde_json::from_value::<AutomationDefinition>(invalid_extension).is_err());

        let mut duplicate_capability = fixture("definition.golden");
        duplicate_capability["runtimeRequirements"]["capabilities"] =
            json!(["sessions.launch", "sessions.launch"]);
        assert!(serde_json::from_value::<AutomationDefinition>(duplicate_capability).is_err());

        let mut oversized_capability = fixture("definition.golden");
        oversized_capability["runtimeRequirements"]["capabilities"] =
            json!([format!("x{}", "a".repeat(96))]);
        assert!(serde_json::from_value::<AutomationDefinition>(oversized_capability).is_err());

        let mut missing_runtime = fixture("definition.golden");
        missing_runtime
            .as_object_mut()
            .expect("definition fixture is an object")
            .remove("runtimeRequirements");
        assert!(serde_json::from_value::<AutomationDefinition>(missing_runtime).is_err());

        let mut missing_familiar = fixture("definition.golden");
        missing_familiar["binding"]
            .as_object_mut()
            .expect("binding fixture is an object")
            .remove("familiarId");
        assert!(serde_json::from_value::<AutomationDefinition>(missing_familiar).is_err());

        let mut fixed_backoff_without_seconds = fixture("definition.golden");
        fixed_backoff_without_seconds["policies"]["retry"]["backoffPolicy"] = json!("fixed");
        fixed_backoff_without_seconds["policies"]["retry"]
            .as_object_mut()
            .expect("retry fixture is an object")
            .remove("backoffSeconds");
        assert!(
            serde_json::from_value::<AutomationDefinition>(fixed_backoff_without_seconds).is_err()
        );

        let mut invalid_digest = fixture("definition.golden");
        invalid_digest["integrity"]["value"] = json!("not-a-sha256");
        assert!(serde_json::from_value::<AutomationDefinition>(invalid_digest).is_err());

        let mut duplicate_exercised_capability = fixture("receipt.golden");
        duplicate_exercised_capability["exercisedCapabilities"] =
            json!(["sessions.launch", "sessions.launch"]);
        assert!(
            serde_json::from_value::<AutomationReceipt>(duplicate_exercised_capability).is_err()
        );
    }

    #[test]
    fn definition_display_and_event_envelope_bounds_fail_closed() {
        let mut empty_name = fixture("definition.golden");
        empty_name["display"]["name"] = json!("");
        assert!(serde_json::from_value::<AutomationDefinition>(empty_name).is_err());

        let mut oversized_description = fixture("definition.golden");
        oversized_description["display"]["description"] = json!("a".repeat(2_001));
        assert!(serde_json::from_value::<AutomationDefinition>(oversized_description).is_err());

        let mut duplicate_tags = fixture("definition.golden");
        duplicate_tags["display"]["tags"] = json!(["notes", "notes"]);
        assert!(serde_json::from_value::<AutomationDefinition>(duplicate_tags).is_err());

        let mut oversized_tag = fixture("definition.golden");
        oversized_tag["display"]["tags"] = json!([format!("x{}", "a".repeat(64))]);
        assert!(serde_json::from_value::<AutomationDefinition>(oversized_tag).is_err());

        let mut oversized_tags = fixture("definition.golden");
        oversized_tags["display"]["tags"] = json!(vec!["tag"; 65]);
        assert!(serde_json::from_value::<AutomationDefinition>(oversized_tags).is_err());

        let mut short_event_id = event_fixture();
        short_event_id["eventId"] = json!("short");
        assert!(serde_json::from_value::<EventEnvelope>(short_event_id).is_err());

        let mut invalid_event_id = event_fixture();
        invalid_event_id["eventId"] = json!("event-id-with-hyphen-00001");
        assert!(serde_json::from_value::<EventEnvelope>(invalid_event_id).is_err());

        let mut oversized_stream_id = event_fixture();
        oversized_stream_id["stream"]["id"] = json!("a".repeat(321));
        assert!(serde_json::from_value::<EventEnvelope>(oversized_stream_id).is_err());

        let mut empty_summary = event_fixture();
        empty_summary["summary"] = json!("");
        assert!(serde_json::from_value::<EventEnvelope>(empty_summary).is_err());

        let mut empty_rrule = fixture("definition.golden");
        empty_rrule["trigger"]["schedule"]["rrule"] = json!("");
        assert!(serde_json::from_value::<AutomationDefinition>(empty_rrule).is_err());

        let mut empty_prompt = fixture("definition.golden");
        empty_prompt["action"]["prompt"] = json!("");
        assert!(serde_json::from_value::<AutomationDefinition>(empty_prompt).is_err());

        let mut oversized_cwd = fixture("definition.golden");
        oversized_cwd["action"]["cwd"] = json!("a".repeat(1_025));
        assert!(serde_json::from_value::<AutomationDefinition>(oversized_cwd).is_err());

        let mut invalid_occurrence_key = fixture("occurrence.golden");
        invalid_occurrence_key["occurrenceKey"] = json!("not a key");
        assert!(serde_json::from_value::<AutomationOccurrence>(invalid_occurrence_key).is_err());

        let mut empty_state_reason = fixture("occurrence.golden");
        empty_state_reason["stateReason"] = json!("");
        assert!(serde_json::from_value::<AutomationOccurrence>(empty_state_reason).is_err());

        let mut oversized_command_reason = fixture("command.create.golden");
        oversized_command_reason["command"] = json!("definition.pause.v1");
        oversized_command_reason["expectedRevision"] = json!(1);
        oversized_command_reason["payload"] =
            json!({"automationId": "daily-notes", "reason": "a".repeat(501)});
        assert!(serde_json::from_value::<CommandRequest>(oversized_command_reason).is_err());
    }

    #[test]
    fn run_and_command_response_outcome_conditionals_fail_closed() {
        let mut no_finished_at = fixture("run.golden");
        no_finished_at
            .as_object_mut()
            .expect("run fixture is an object")
            .remove("finishedAt");
        assert!(serde_json::from_value::<AutomationRun>(no_finished_at).is_err());

        let mut no_terminal_disposition = fixture("run.golden");
        no_terminal_disposition
            .as_object_mut()
            .expect("run fixture is an object")
            .remove("terminalDisposition");
        assert!(serde_json::from_value::<AutomationRun>(no_terminal_disposition).is_err());

        let mut running = fixture("run.golden");
        running["state"] = json!("running");
        running
            .as_object_mut()
            .expect("run fixture is an object")
            .remove("finishedAt");
        running
            .as_object_mut()
            .expect("run fixture is an object")
            .remove("terminalDisposition");
        assert_round_trip::<AutomationRun>(running);

        let mut running_with_finished_at = fixture("run.golden");
        running_with_finished_at["state"] = json!("running");
        running_with_finished_at
            .as_object_mut()
            .expect("run fixture is an object")
            .remove("terminalDisposition");
        assert!(serde_json::from_value::<AutomationRun>(running_with_finished_at).is_err());

        let mut accepted_with_terminal_disposition = fixture("run.golden");
        accepted_with_terminal_disposition["state"] = json!("accepted");
        accepted_with_terminal_disposition
            .as_object_mut()
            .expect("run fixture is an object")
            .remove("finishedAt");
        assert!(
            serde_json::from_value::<AutomationRun>(accepted_with_terminal_disposition).is_err()
        );

        let mut retry_without_prior_disposition = fixture("attempt.golden");
        retry_without_prior_disposition["attemptNumber"] = json!(2);
        assert!(
            serde_json::from_value::<AutomationAttempt>(retry_without_prior_disposition).is_err()
        );

        let base = json!({
            "schemaVersion": "coven.automations.v1",
            "command": "definition.get.v1",
            "adoptionKey": "adopt:get-0001"
        });
        let mut committed_without_result = base.clone();
        committed_without_result["outcome"] = json!("committed");
        assert!(serde_json::from_value::<CommandResponse>(committed_without_result).is_err());

        let mut committed_with_error = base.clone();
        committed_with_error["outcome"] = json!("committed");
        committed_with_error["result"] = json!({});
        committed_with_error["error"] = json!({
            "code": "NOT_FOUND",
            "httpStatus": 404,
            "message": "No such automation.",
            "retryable": false
        });
        assert!(serde_json::from_value::<CommandResponse>(committed_with_error).is_err());

        let mut replayed_without_replay = base.clone();
        replayed_without_replay["outcome"] = json!("replayed");
        replayed_without_replay["result"] = json!({});
        assert!(serde_json::from_value::<CommandResponse>(replayed_without_replay).is_err());

        let mut rejected_with_result = base;
        rejected_with_result["outcome"] = json!("rejected");
        rejected_with_result["result"] = json!({});
        rejected_with_result["error"] = json!({
            "code": "NOT_FOUND",
            "httpStatus": 404,
            "message": "No such automation.",
            "retryable": false
        });
        assert!(serde_json::from_value::<CommandResponse>(rejected_with_result).is_err());
    }
}
