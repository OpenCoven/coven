use serde_json::{json, Value};

use super::conformance_target::{
    capability, evaluate, evaluate_run_terminal_monotonicity_case_counts, TargetSuiteStatus,
    CAPABILITY_NEGOTIATION_SUITE, RUN_TERMINAL_MONOTONICITY_SUITE,
};

const ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/attempt-terminal-immutability.vectors.json"
);
const DEFINITION_VALIDATION_VECTORS: &str =
    include_str!("../../../../conformance/automations/runner/definition-validation.vectors.json");
const EVENT_REDUCER_DETERMINISM_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/event-reducer-determinism.vectors.json"
);
const OCCURRENCE_FENCE_UNIQUENESS_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/occurrence-fence-uniqueness.vectors.json"
);

fn request_for(suite_id: &str, vector: Value) -> Value {
    json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": "structural",
        "suiteId": suite_id,
        "protocolArtifact": {
            "bundleSchemaVersion": "coven.automations.bundle.v1",
            "sourceCommit": "1111111111111111111111111111111111111111",
            "bundleSha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "contractContentSha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            "fileCount": 19
        },
        "subjectArtifact": {
            "artifactId": "coven-cli",
            "artifactVersion": "0.1.0",
            "platform": {
                "os": "linux",
                "arch": "x86_64"
            },
            "sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
        },
        "vector": vector
    })
}

fn request(vector: Value) -> Value {
    request_for(CAPABILITY_NEGOTIATION_SUITE, vector)
}

#[test]
fn capability_advertises_the_native_structural_suites() {
    assert_eq!(
        serde_json::to_value(capability()).unwrap(),
        json!({
            "schemaVersion": "coven.automations.conformance-target-capability.v1",
            "profiles": [{
                "profile": "structural",
                "suites": [
                    "attempt-terminal-immutability",
                    CAPABILITY_NEGOTIATION_SUITE,
                    "definition-validation",
                    "event-reducer-determinism",
                    "occurrence-fence-uniqueness",
                    RUN_TERMINAL_MONOTONICITY_SUITE
                ]
            }]
        })
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    let response = evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 3);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 3);
}

#[test]
fn occurrence_fence_uniqueness_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["secondCommitted"] = json!(true);

    let response = evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn occurrence_fence_uniqueness_suite_requires_all_scenarios() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"].as_array_mut().unwrap().pop();

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_rejects_duplicate_case_ids() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone();

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_rejects_reused_occurrence_ids() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][1]["first"]["occurrenceId"] =
        vectors["cases"][0]["first"]["occurrenceId"].clone();

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_rejects_mismatched_scenarios() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][0]["second"]["scheduledFor"] = json!("2026-08-31T09:00:00.000Z");

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_checks_first_row_preservation() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["firstPreserved"] = json!(false);

    let response = evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn occurrence_fence_uniqueness_suite_rejects_noncanonical_timestamps() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][0]["first"]["scheduledFor"] = json!("2026-08-30T09:00:00Z");

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_rejects_contract_invalid_ids() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][0]["first"]["automationId"] = json!("daily:notes");

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn occurrence_fence_uniqueness_suite_rejects_impossible_zero_row_expectations() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_FENCE_UNIQUENESS_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["rowCount"] = json!(0);

    assert_eq!(
        evaluate(&request_for("occurrence-fence-uniqueness", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn event_reducer_determinism_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    let response = evaluate(&request_for("event-reducer-determinism", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 1);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 1);
}

#[test]
fn event_reducer_determinism_suite_fails_closed_on_a_digest_mismatch() {
    let mut vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    vectors["cases"][0]["expectedStateDigest"] = json!(format!("sha256:{}", "0".repeat(64)));

    let response = evaluate(&request_for("event-reducer-determinism", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn event_reducer_determinism_suite_rejects_an_invalid_duplicate_index() {
    let mut vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    vectors["cases"][0]["duplicateIndex"] = json!(3);

    assert_eq!(
        evaluate(&request_for("event-reducer-determinism", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn event_reducer_determinism_suite_rejects_duplicate_case_ids() {
    let mut vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    let duplicate = vectors["cases"][0].clone();
    vectors["cases"].as_array_mut().unwrap().push(duplicate);

    assert_eq!(
        evaluate(&request_for("event-reducer-determinism", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn event_reducer_determinism_suite_rejects_duplicate_canonical_event_ids() {
    let mut vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    let duplicate = vectors["cases"][0]["events"][1].clone();
    vectors["cases"][0]["events"]
        .as_array_mut()
        .unwrap()
        .push(duplicate);

    assert_eq!(
        evaluate(&request_for("event-reducer-determinism", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn event_reducer_determinism_suite_rejects_malformed_expected_digests() {
    let mut vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    vectors["cases"][0]["expectedStateDigest"] = json!("sha256:not-a-digest");

    assert_eq!(
        evaluate(&request_for("event-reducer-determinism", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn event_reducer_determinism_suite_fails_closed_on_out_of_order_events() {
    let mut vectors: Value = serde_json::from_str(EVENT_REDUCER_DETERMINISM_VECTORS).unwrap();
    vectors["cases"][0]["events"]
        .as_array_mut()
        .unwrap()
        .swap(0, 1);

    let response = evaluate(&request_for("event-reducer-determinism", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn attempt_terminal_immutability_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS).unwrap();
    let response = evaluate(&request_for("attempt-terminal-immutability", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 5);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 5);
}

#[test]
fn attempt_terminal_immutability_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["updateCommitted"] = json!(true);

    let response = evaluate(&request_for("attempt-terminal-immutability", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn attempt_terminal_immutability_suite_rejects_duplicate_case_ids() {
    let mut vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS).unwrap();
    vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone();

    assert_eq!(
        evaluate(&request_for("attempt-terminal-immutability", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn attempt_terminal_immutability_suite_rejects_nonterminal_start_states() {
    let mut vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS).unwrap();
    vectors["cases"][0]["firstState"] = json!("observing");

    assert_eq!(
        evaluate(&request_for("attempt-terminal-immutability", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn attempt_terminal_immutability_suite_rejects_noop_updates() {
    let mut vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS).unwrap();
    vectors["cases"][0]["attemptedState"] = vectors["cases"][0]["firstState"].clone();

    assert_eq!(
        evaluate(&request_for("attempt-terminal-immutability", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn attempt_terminal_immutability_suite_requires_every_terminal_state() {
    let mut vectors: Value = serde_json::from_str(ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS).unwrap();
    vectors["cases"].as_array_mut().unwrap().pop();

    assert_eq!(
        evaluate(&request_for("attempt-terminal-immutability", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn definition_validation_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(DEFINITION_VALIDATION_VECTORS).unwrap();
    let response = evaluate(&request_for("definition-validation", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 2);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 2);
}

#[test]
fn definition_validation_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(DEFINITION_VALIDATION_VECTORS).unwrap();
    vectors["cases"][0]["expected"] = json!({"outcome": "rejected"});

    let response = evaluate(&request_for("definition-validation", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn definition_validation_suite_rejects_malformed_expected_digests() {
    let mut vectors: Value = serde_json::from_str(DEFINITION_VALIDATION_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["normalizedDigest"] = json!("sha256:not-a-digest");

    assert_eq!(
        evaluate(&request_for("definition-validation", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn definition_validation_suite_rejects_duplicate_case_ids() {
    let mut vectors: Value = serde_json::from_str(DEFINITION_VALIDATION_VECTORS).unwrap();
    vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone();

    assert_eq!(
        evaluate(&request_for("definition-validation", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn capability_negotiation_suite_executes_supported_and_refused_cases() {
    let response = evaluate(&request(json!({
        "schemaVersion": "coven.automations.capability-negotiation-vectors.v1",
        "cases": [
            {
                "caseId": "supported-minimal",
                "definition": {
                    "schemaVersion": 1,
                    "id": "native-supported",
                    "name": "Native supported",
                    "status": "PAUSED",
                    "rrule": "FREQ=DAILY",
                    "timezone": "local",
                    "prompt": "Run the native conformance probe",
                    "misfire": "latest",
                    "overlap": "forbid",
                    "timeoutMinutes": 30,
                    "runtime": "coven-code"
                },
                "expected": {
                    "outcome": "supported"
                }
            },
            {
                "caseId": "refused-trigger",
                "definition": {
                    "schemaVersion": 1,
                    "id": "native-refused",
                    "name": "Native refused",
                    "status": "PAUSED",
                    "rrule": "FREQ=DAILY",
                    "timezone": "local",
                    "prompt": "Run the native conformance probe",
                    "misfire": "latest",
                    "overlap": "forbid",
                    "timeoutMinutes": 30,
                    "runtime": "coven-code",
                    "trigger": {
                        "variant": "webhook",
                        "version": 1,
                        "webhook": {}
                    }
                },
                "expected": {
                    "outcome": "unsupported",
                    "variant": "trigger.webhook"
                }
            }
        ]
    })))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(
        response.evidence,
        Some(json!({
            "executedCases": 2,
            "passedCases": 2,
            "vectorDigest": "sha256:fc716602d581388f683fbf9c81b588a7510b4eed308999dd89ea13f76ad5304b"
        }))
    );
}

#[test]
fn capability_negotiation_suite_fails_closed_on_an_expectation_mismatch() {
    let response = evaluate(&request(json!({
        "schemaVersion": "coven.automations.capability-negotiation-vectors.v1",
        "cases": [{
            "caseId": "wrong-expectation",
            "definition": {
                "schemaVersion": 1,
                "id": "native-supported",
                "name": "Native supported",
                "status": "PAUSED",
                "rrule": "FREQ=DAILY",
                "timezone": "local",
                "prompt": "Run the native conformance probe",
                "misfire": "latest",
                "overlap": "forbid",
                "timeoutMinutes": 30,
                "runtime": "coven-code"
            },
            "expected": {
                "outcome": "unsupported",
                "variant": "webhook"
            }
        }]
    })))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn run_terminal_monotonicity_suite_executes_the_real_run_ledger() {
    let response = evaluate(&request_for(
        RUN_TERMINAL_MONOTONICITY_SUITE,
        json!({
            "schemaVersion": "coven.automations.run-terminal-monotonicity-vectors.v1",
            "cases": [
                {
                    "caseId": "succeeded-cannot-be-rewritten",
                    "firstStatus": "succeeded",
                    "replayStatus": "failed",
                    "expected": {
                        "firstCommitted": true,
                        "replayCommitted": false,
                        "finalStatus": "succeeded"
                    }
                },
                {
                    "caseId": "failed-cannot-be-rewritten",
                    "firstStatus": "failed",
                    "replayStatus": "succeeded",
                    "expected": {
                        "firstCommitted": true,
                        "replayCommitted": false,
                        "finalStatus": "failed"
                    }
                }
            ]
        }),
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 2);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 2);
}

#[test]
fn run_terminal_monotonicity_suite_fails_closed_on_an_expectation_mismatch() {
    let response = evaluate(&request_for(
        RUN_TERMINAL_MONOTONICITY_SUITE,
        json!({
            "schemaVersion": "coven.automations.run-terminal-monotonicity-vectors.v1",
            "cases": [{
                "caseId": "rewrite-wrongly-expected",
                "firstStatus": "failed",
                "replayStatus": "succeeded",
                "expected": {
                    "firstCommitted": true,
                    "replayCommitted": true,
                    "finalStatus": "succeeded"
                }
            }]
        }),
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn run_terminal_monotonicity_suite_rejects_non_conflicting_replay_vectors() {
    assert_eq!(
        evaluate(&request_for(
            RUN_TERMINAL_MONOTONICITY_SUITE,
            json!({
                "schemaVersion": "coven.automations.run-terminal-monotonicity-vectors.v1",
                "cases": [{
                    "caseId": "duplicate-succeeded-replay",
                    "firstStatus": "succeeded",
                    "replayStatus": "succeeded",
                    "expected": {
                        "firstCommitted": true,
                        "replayCommitted": false,
                        "finalStatus": "succeeded"
                    }
                }]
            }),
        ))
        .unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn run_terminal_monotonicity_suite_continues_executing_after_a_mismatch() {
    let vector = json!({
        "schemaVersion": "coven.automations.run-terminal-monotonicity-vectors.v1",
        "cases": [
            {
                "caseId": "rewrite-wrongly-expected",
                "firstStatus": "failed",
                "replayStatus": "succeeded",
                "expected": {
                    "firstCommitted": true,
                    "replayCommitted": true,
                    "finalStatus": "succeeded"
                }
            },
            {
                "caseId": "cancelled-cannot-be-rewritten",
                "firstStatus": "cancelled",
                "replayStatus": "timed_out",
                "expected": {
                    "firstCommitted": true,
                    "replayCommitted": false,
                    "finalStatus": "cancelled"
                }
            }
        ]
    });

    assert_eq!(
        evaluate_run_terminal_monotonicity_case_counts(&vector).unwrap(),
        (2, 1)
    );
}

#[test]
fn evaluator_rejects_unadvertised_suites_with_a_static_error() {
    let mut request = request(json!({
        "schemaVersion": "coven.automations.capability-negotiation-vectors.v1",
        "cases": []
    }));
    request["suiteId"] = json!("scheduler-secret-output");

    assert_eq!(
        evaluate(&request).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn evaluator_rejects_malformed_vectors_with_a_static_error() {
    assert_eq!(
        evaluate(&request(json!({
            "schemaVersion": "coven.automations.capability-negotiation-vectors.v1",
            "cases": [{
                "caseId": "contains-secret",
                "definition": "SECRET-DEFINITION",
                "expected": {"outcome": "supported"}
            }]
        })))
        .unwrap_err(),
        "conformance vector is invalid"
    );
}
