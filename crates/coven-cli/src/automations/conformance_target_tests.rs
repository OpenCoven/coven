use serde_json::{json, Value};

use super::conformance_target::{
    capability, evaluate, evaluate_run_terminal_monotonicity_case_counts, TargetSuiteStatus,
    CALENDAR_SCHEDULE_RESOLUTION_SUITE, CAPABILITY_NEGOTIATION_SUITE,
    MISFIRE_LATEST_PLANNING_SUITE, OCCURRENCE_LEASE_RECOVERY_SUITE, OVERLAP_FORBID_CLAIMING_SUITE,
    RUN_TERMINAL_MONOTONICITY_SUITE,
};

const ATTEMPT_TERMINAL_IMMUTABILITY_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/attempt-terminal-immutability.vectors.json"
);
const COMMAND_ADOPTION_IDEMPOTENCY_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/command-adoption-idempotency.vectors.json"
);
const COMMAND_ENVELOPE_OUTCOMES_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/command-envelope-outcomes.vectors.json"
);
const MISSION_COMMANDS_VECTORS: &str =
    include_str!("../../../../conformance/automations/runner/mission-commands.vectors.json");
const CALENDAR_SCHEDULE_RESOLUTION_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/calendar-schedule-resolution.vectors.json"
);
const CANCELLATION_TIMEOUT_ARBITRATION_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/cancellation-timeout-arbitration.vectors.json"
);
const DEFINITION_LIFECYCLE_TRANSITIONS_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/definition-lifecycle-transitions.vectors.json"
);
const DEFINITION_VALIDATION_VECTORS: &str =
    include_str!("../../../../conformance/automations/runner/definition-validation.vectors.json");
const EVENT_REDUCER_DETERMINISM_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/event-reducer-determinism.vectors.json"
);
const MISFIRE_LATEST_PLANNING_VECTORS: &str =
    include_str!("../../../../conformance/automations/runner/misfire-latest-planning.vectors.json");
const OCCURRENCE_LEASE_RECOVERY_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/occurrence-lease-recovery.vectors.json"
);
const OVERLAP_FORBID_CLAIMING_VECTORS: &str =
    include_str!("../../../../conformance/automations/runner/overlap-forbid-claiming.vectors.json");
const RETRY_BACKOFF_TIMING_VECTORS: &str =
    include_str!("../../../../conformance/automations/runner/retry-backoff-timing.vectors.json");
const RETRY_QUARANTINE_RECOVERY_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/retry-quarantine-recovery.vectors.json"
);
const SCHEDULER_LEADERSHIP_FENCING_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/scheduler-leadership-fencing.vectors.json"
);
const STARTUP_RECONCILIATION_WAKE_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/startup-reconciliation-wake.vectors.json"
);
const OCCURRENCE_FENCE_UNIQUENESS_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/occurrence-fence-uniqueness.vectors.json"
);
const RECEIPT_INTEGRITY_VALIDATION_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/receipt-integrity-validation.vectors.json"
);
const RRULE_VOCABULARY_VECTORS: &str =
    include_str!("../../../../conformance/automations/runner/rrule-vocabulary.vectors.json");
const RUNTIME_AUTHORITY_TERMINAL_RECOVERY_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/runtime-authority-terminal-recovery.vectors.json"
);

fn request_for_profile(profile: &str, suite_id: &str, vector: Value) -> Value {
    json!({
        "schemaVersion": "coven.automations.conformance-suite-request.v1",
        "profile": profile,
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

fn request_for(suite_id: &str, vector: Value) -> Value {
    request_for_profile("structural", suite_id, vector)
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
            "profiles": [
                {
                    "profile": "structural",
                    "suites": [
                        "attempt-terminal-immutability",
                        CAPABILITY_NEGOTIATION_SUITE,
                        "command-adoption-idempotency",
                        "command-envelope-outcomes",
                        "definition-lifecycle-transitions",
                        "definition-validation",
                        "event-reducer-determinism",
                        "mission-commands",
                        "occurrence-fence-uniqueness",
                        "receipt-integrity-validation",
                        "rrule-vocabulary",
                        RUN_TERMINAL_MONOTONICITY_SUITE
                    ]
                },
                {
                    "profile": "scheduler_reliability",
                    "suites": [
                        CALENDAR_SCHEDULE_RESOLUTION_SUITE,
                        "cancellation-timeout-arbitration",
                        "command-storage-lock-recovery",
                        "command-storage-readonly-recovery",
                        "command-storage-corruption-recovery",
                        "command-transaction-rollback",
                        "legacy-import-source-preservation",
                        "command-storage-capacity-recovery",
                        "command-process-crash-recovery",
                        MISFIRE_LATEST_PLANNING_SUITE,
                        OCCURRENCE_LEASE_RECOVERY_SUITE,
                        OVERLAP_FORBID_CLAIMING_SUITE,
                        "retry-backoff-timing",
                        "retry-quarantine-recovery",
                        "scheduler-leadership-fencing",
                        "startup-reconciliation-wake",
                        "result-delivery-recovery",
                        "result-delivery-process-crash-recovery",
                        "run-settlement-process-crash-recovery",
                        "run-launch-process-crash-recovery"
                    ]
                },
                {
                    "profile": "runtime_authority",
                    "suites": ["runtime-authority-terminal-recovery"]
                }
            ]
        })
    );
}

fn runtime_terminal_recovery_request() -> Value {
    request_for_profile(
        "runtime_authority",
        "runtime-authority-terminal-recovery",
        serde_json::from_str(RUNTIME_AUTHORITY_TERMINAL_RECOVERY_VECTORS).unwrap(),
    )
}

#[test]
fn runtime_authority_terminal_recovery_executes_checked_in_vectors() {
    let response = evaluate(&runtime_terminal_recovery_request()).unwrap();
    assert_eq!(response.status, TargetSuiteStatus::Passed);
    let evidence = response.evidence.unwrap();
    assert_eq!(evidence["executedCases"], 5);
    assert_eq!(evidence["passedCases"], 5);
    assert_eq!(evidence.as_object().unwrap().len(), 3);
    assert!(evidence["vectorDigest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
}

#[test]
fn runtime_authority_terminal_recovery_expectations_drive_every_assertion() {
    let baseline = runtime_terminal_recovery_request();
    for (key, value) in baseline["vector"]["cases"][0]["expected"]
        .as_object()
        .unwrap()
    {
        let mut request = baseline.clone();
        request["vector"]["cases"][0]["expected"][key] = match value {
            Value::Bool(value) => json!(!value),
            Value::Number(value) => json!(value.as_u64().unwrap() + 1),
            Value::String(_) => json!("failed"),
            _ => panic!("unexpected expectation shape: {key}"),
        };
        let response = evaluate(&request).unwrap();
        assert_eq!(response.status, TargetSuiteStatus::Failed, "{key}");
        assert_eq!(response.evidence, None, "{key}");
    }
}

#[test]
fn runtime_authority_terminal_recovery_rejects_invalid_shapes_and_inputs() {
    let invalid_mutations: &[fn(&mut Value)] = &[
        |v| v["schemaVersion"] = json!("unsupported"),
        |v| v["unexpected"] = json!(true),
        |v| v["cases"] = json!([]),
        |v| v["cases"].as_array_mut().unwrap().truncate(4),
        |v| v["cases"] = json!(vec![v["cases"][0].clone(); 129]),
        |v| v["cases"][0]["caseId"] = json!(""),
        |v| v["cases"][0]["caseId"] = json!("-bad"),
        |v| v["cases"][0]["caseId"] = json!("a".repeat(129)),
        |v| v["cases"][1]["caseId"] = v["cases"][0]["caseId"].clone(),
        |v| v["cases"][1]["scenario"] = v["cases"][0]["scenario"].clone(),
        |v| v["cases"][0]["scenario"] = json!("unknown"),
        |v| v["cases"][0]["startedAt"] = json!("not-a-time"),
        |v| v["cases"][0]["startedAt"] = json!("2026-09-03T12:00:01.000Z"),
        |v| v["cases"][0]["terminalAt"] = json!("2026-09-03T12:00:06.000Z"),
        |v| v["cases"][0]["reconcileAt"] = json!([]),
        |v| v["cases"][0]["reconcileAt"][2] = json!("2026-09-03T12:00:07.000Z"),
        |v| v["cases"][0]["sessionStatus"] = json!("failed"),
        |v| v["cases"][0]["exitCode"] = json!(1),
        |v| {
            v["cases"][2]
                .as_object_mut()
                .unwrap()
                .remove("exitCode")
                .map(|_| ())
                .unwrap()
        },
        |v| v["cases"][0]["evidenceShapedOutput"] = json!(false),
        |v| v["cases"][0]["unexpected"] = json!(true),
        |v| v["cases"][0]["expected"]["unexpected"] = json!(true),
        |v| v["cases"][0]["expected"]["runState"] = json!("unknown"),
        |v| v["cases"][0]["expected"]["runRows"] = json!(-1),
        |v| v["cases"][0]["expected"]["runUnresolved"] = json!("true"),
        |v| {
            v["cases"][0]["expected"]
                .as_object_mut()
                .unwrap()
                .remove("bindingPreserved");
        },
    ];
    for (index, mutate) in invalid_mutations.iter().enumerate() {
        let mut request = runtime_terminal_recovery_request();
        mutate(&mut request["vector"]);
        assert_eq!(
            evaluate(&request).unwrap_err(),
            "conformance vector is invalid",
            "{index}"
        );
    }
}

#[test]
fn runtime_authority_terminal_recovery_requires_runtime_authority_profile() {
    for profile in ["structural", "scheduler_reliability"] {
        let mut request = runtime_terminal_recovery_request();
        request["profile"] = json!(profile);
        assert_eq!(
            evaluate(&request).unwrap_err(),
            "conformance suite is unsupported"
        );
    }
}

#[test]
fn cancellation_timeout_arbitration_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(CANCELLATION_TIMEOUT_ARBITRATION_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        "cancellation-timeout-arbitration",
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 2);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 2);
}

#[test]
fn cancellation_timeout_arbitration_suite_rejects_a_weakened_stop_count() {
    let mut vectors: Value =
        serde_json::from_str(CANCELLATION_TIMEOUT_ARBITRATION_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["runtimeStopCount"] = json!(2);

    assert_eq!(
        evaluate(&request_for_profile(
            "scheduler_reliability",
            "cancellation-timeout-arbitration",
            vectors,
        ))
        .unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn cancellation_timeout_arbitration_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 14] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["cancellationAt"] = json!("not-a-time"),
        |vectors| vectors["cases"][0]["timeoutAt"] = vectors["cases"][0]["cancellationAt"].clone(),
        |vectors| {
            vectors["cases"][0]["timeoutObservedAt"] = vectors["cases"][0]["timeoutAt"].clone()
        },
        |vectors| vectors["cases"][0]["expected"]["cancellationOutcome"] = json!("cancel_pending"),
        |vectors| vectors["cases"][1]["expected"]["competingOutcome"] = json!("deferred"),
        |vectors| {
            vectors["cases"][1]["expected"]["preCompetingCancellationState"] = json!("stopping")
        },
        |vectors| vectors["cases"][1]["expected"]["preCompetingStopFenceOwner"] = json!("timeout"),
        |vectors| vectors["cases"][0]["expected"]["competingCandidateObserved"] = json!(false),
        |vectors| vectors["cases"][0]["expected"]["runtimeStopCount"] = json!(0),
        |vectors| vectors["cases"][1]["unexpected"] = json!(true),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value =
            serde_json::from_str(CANCELLATION_TIMEOUT_ARBITRATION_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                "cancellation-timeout-arbitration",
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn cancellation_timeout_arbitration_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(CANCELLATION_TIMEOUT_ARBITRATION_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for("cancellation-timeout-arbitration", vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn calendar_schedule_resolution_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(CALENDAR_SCHEDULE_RESOLUTION_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        CALENDAR_SCHEDULE_RESOLUTION_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 11);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 11);
}

#[test]
fn calendar_schedule_resolution_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(CALENDAR_SCHEDULE_RESOLUTION_VECTORS).unwrap();
    vectors["cases"][5]["expected"]["nextDueAt"] = json!("2026-03-08T07:00:00.000Z");

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        CALENDAR_SCHEDULE_RESOLUTION_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn calendar_schedule_resolution_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 7] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["from"] = json!("not-a-time"),
        |vectors| vectors["cases"][5]["timezone"] = json!("utc"),
        |vectors| {
            vectors["cases"][10]["expected"] = json!({
                "outcome": "scheduled",
                "nextDueAt": "2026-08-28T09:00:00.000Z"
            })
        },
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value =
            serde_json::from_str(CALENDAR_SCHEDULE_RESOLUTION_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                CALENDAR_SCHEDULE_RESOLUTION_SUITE,
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn calendar_schedule_resolution_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(CALENDAR_SCHEDULE_RESOLUTION_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for(CALENDAR_SCHEDULE_RESOLUTION_SUITE, vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn occurrence_lease_recovery_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(OCCURRENCE_LEASE_RECOVERY_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        OCCURRENCE_LEASE_RECOVERY_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 5);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 5);
}

#[test]
fn occurrence_lease_recovery_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_LEASE_RECOVERY_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["recoveredCount"] = json!(0);

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        OCCURRENCE_LEASE_RECOVERY_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn occurrence_lease_recovery_suite_accepts_portable_case_ids() {
    let mut vectors: Value = serde_json::from_str(OCCURRENCE_LEASE_RECOVERY_VECTORS).unwrap();
    vectors["cases"][0]["caseId"] = json!(format!("case:{}", "a".repeat(120)));

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        OCCURRENCE_LEASE_RECOVERY_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
}

#[test]
fn occurrence_lease_recovery_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 8] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["now"] = json!("not-a-time"),
        |vectors| vectors["cases"][0]["initial"]["leaseExpiresAt"] = json!("not-a-time"),
        |vectors| vectors["cases"][0]["initial"]["runningRun"] = json!(true),
        |vectors| vectors["cases"][4]["expected"]["state"] = json!("failed"),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(OCCURRENCE_LEASE_RECOVERY_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                OCCURRENCE_LEASE_RECOVERY_SUITE,
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn occurrence_lease_recovery_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(OCCURRENCE_LEASE_RECOVERY_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for(OCCURRENCE_LEASE_RECOVERY_SUITE, vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn overlap_forbid_claiming_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(OVERLAP_FORBID_CLAIMING_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        OVERLAP_FORBID_CLAIMING_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 7);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 7);
}

#[test]
fn overlap_forbid_claiming_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(OVERLAP_FORBID_CLAIMING_VECTORS).unwrap();
    vectors["cases"][1]["expected"] = json!({
        "claimed": true,
        "targetState": "claimed",
        "targetAttempt": 1,
        "leaseOwner": "daemon-a",
        "leaseExpiresAt": "2026-09-01T11:00:00.000Z"
    });

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        OVERLAP_FORBID_CLAIMING_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn overlap_forbid_claiming_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 8] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["now"] = json!("not-a-time"),
        |vectors| vectors["cases"][2]["blocker"] = json!("claimed_occurrence"),
        |vectors| vectors["cases"][0]["expected"]["leaseExpiresAt"] = Value::Null,
        |vectors| vectors["cases"][1]["expected"]["targetAttempt"] = json!(1),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(OVERLAP_FORBID_CLAIMING_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                OVERLAP_FORBID_CLAIMING_SUITE,
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn overlap_forbid_claiming_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(OVERLAP_FORBID_CLAIMING_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for(OVERLAP_FORBID_CLAIMING_SUITE, vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn retry_backoff_timing_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(RETRY_BACKOFF_TIMING_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        "retry-backoff-timing",
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 5);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 5);
}

#[test]
fn retry_backoff_timing_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(RETRY_BACKOFF_TIMING_VECTORS).unwrap();
    vectors["cases"][2]["expected"]["delaySeconds"] = json!(1);

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        "retry-backoff-timing",
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn retry_backoff_timing_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 9] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["observedAt"] = json!("not-a-time"),
        |vectors| vectors["cases"][0]["nextAttemptNumber"] = json!(1),
        |vectors| vectors["cases"][1]["policy"]["backoffSeconds"] = Value::Null,
        |vectors| vectors["cases"][2]["policy"]["backoffPolicy"] = json!("fixed"),
        |vectors| vectors["cases"][4]["expected"]["minimumDelaySeconds"] = json!(90000),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(RETRY_BACKOFF_TIMING_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                "retry-backoff-timing",
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn retry_backoff_timing_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(RETRY_BACKOFF_TIMING_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for("retry-backoff-timing", vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn retry_quarantine_recovery_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(RETRY_QUARANTINE_RECOVERY_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        "retry-quarantine-recovery",
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 4);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 4);
}

#[test]
fn retry_quarantine_recovery_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(RETRY_QUARANTINE_RECOVERY_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["reason"] = json!("different failure");

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        "retry-quarantine-recovery",
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn retry_quarantine_recovery_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 13] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["exhaustions"][0]["at"] = json!("not-a-time"),
        |vectors| vectors["cases"][0]["exhaustions"][0]["failureClass"] = json!("unknown"),
        |vectors| vectors["cases"][0]["exhaustions"][0]["reason"] = json!(""),
        |vectors| {
            vectors["cases"][1]["exhaustions"][1]["failureClass"] =
                vectors["cases"][1]["exhaustions"][0]["failureClass"].clone()
        },
        |vectors| {
            vectors["cases"][1]["exhaustions"][1]["reason"] =
                vectors["cases"][1]["exhaustions"][0]["reason"].clone()
        },
        |vectors| vectors["cases"][0]["releaseAt"] = json!("2026-09-02T08:45:00.000Z"),
        |vectors| {
            vectors["cases"][2]["expected"]["quarantinedAt"] = json!("2026-09-02T08:30:00.000Z")
        },
        |vectors| {
            vectors["cases"][3]["observeAt"] = json!("2026-09-01T08:50:00.000Z");
            vectors["cases"][3]["expected"]["plannedCount"] = json!(0);
            vectors["cases"][3]["expected"]["claimedCount"] = json!(0);
        },
        |vectors| {
            vectors["cases"][3]["observeAt"] = json!("2026-09-02T08:50:00.000Z");
            vectors["cases"][3]["expected"]["plannedCount"] = json!(0);
            vectors["cases"][3]["expected"]["claimedCount"] = json!(0);
        },
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(RETRY_QUARANTINE_RECOVERY_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                "retry-quarantine-recovery",
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn retry_quarantine_recovery_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(RETRY_QUARANTINE_RECOVERY_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for("retry-quarantine-recovery", vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn scheduler_leadership_fencing_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(SCHEDULER_LEADERSHIP_FENCING_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        "scheduler-leadership-fencing",
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 3);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 3);
}

#[test]
fn scheduler_leadership_fencing_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(SCHEDULER_LEADERSHIP_FENCING_VECTORS).unwrap();
    vectors["cases"][1]["expected"]["secondGeneration"] = json!(3);

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        "scheduler-leadership-fencing",
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn scheduler_leadership_fencing_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 7] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["firstAcquiredAt"] = json!("not-a-time"),
        |vectors| vectors["cases"][1]["secondAttemptAt"] = json!("2026-09-03T12:00:00.000Z"),
        |vectors| vectors["cases"][2]["expected"]["outcome"] = json!("generation_advanced"),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value =
            serde_json::from_str(SCHEDULER_LEADERSHIP_FENCING_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                "scheduler-leadership-fencing",
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn scheduler_leadership_fencing_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(SCHEDULER_LEADERSHIP_FENCING_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for("scheduler-leadership-fencing", vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

// Run environment-sensitive error paths in a child so no parallel test sees
// the deliberately invalid temporary directory configuration.
#[test]
fn startup_reconciliation_trace_failure_child() {
    if std::env::var("COVEN_STARTUP_TRACE_CHILD").as_deref() != Ok("temp-home") {
        return;
    }
    let vectors: Value = serde_json::from_str(STARTUP_RECONCILIATION_WAKE_VECTORS).unwrap();
    let result = evaluate(&request_for_profile(
        "scheduler_reliability",
        "startup-reconciliation-wake",
        vectors,
    ));
    assert_eq!(result.unwrap_err(), "conformance suite execution failed");
    println!("startup trace child confirmed the existing execution error");
}

#[test]
fn startup_reconciliation_trace_identifies_temp_home_failure_without_private_path() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    struct OwnedChild {
        child: std::process::Child,
        home: Option<tempfile::TempDir>,
    }
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.child.kill();
            if self.child.wait().is_err() {
                if let Some(home) = self.home.take() {
                    let _ = home.keep();
                }
                eprintln!("conformance diagnostic child cleanup incomplete; owned home retained");
            }
        }
    }

    let temp = tempfile::Builder::new()
        .prefix("coven-conformance-error-child-")
        .tempdir()
        .unwrap();
    let invalid_root = temp.path().join("private-conformance-path");
    std::fs::write(&invalid_root, b"not a directory").unwrap();
    let stdout = temp.path().join("child.stdout");
    let stderr = temp.path().join("child.stderr");
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "automations::conformance_target_tests::startup_reconciliation_trace_failure_child",
            "--nocapture",
        ])
        .env("COVEN_STARTUP_TRACE_CHILD", "temp-home")
        .env("TMPDIR", &invalid_root)
        .env("TMP", &invalid_root)
        .env("TEMP", &invalid_root)
        .stdout(Stdio::from(std::fs::File::create(&stdout).unwrap()))
        .stderr(Stdio::from(std::fs::File::create(&stderr).unwrap()))
        .spawn()
        .unwrap();
    // This guard also kills/reaps on polling errors or assertion unwinding.
    let mut owned = OwnedChild {
        child,
        home: Some(temp),
    };
    // This guard catches a wedged owned test process; it asserts no runtime
    // promptness or production lifecycle deadline.
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = owned.child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            panic!("startup trace child exceeded its 60-second hang guard");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = format!(
        "{}{}",
        std::fs::read_to_string(stdout).unwrap(),
        std::fs::read_to_string(stderr).unwrap()
    );
    assert!(status.success(), "child failed: {output}");
    assert!(
        output.contains("startup trace child confirmed the existing execution error"),
        "child did not exercise the intended failure: {output}"
    );
    assert!(
        output.contains("phase=temp-home outcome=error"),
        "missing owning-operation diagnostic: {output}"
    );
    assert!(
        !output.contains("private-conformance-path"),
        "diagnostic leaked the private path"
    );
}

#[test]
fn startup_reconciliation_wake_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(STARTUP_RECONCILIATION_WAKE_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        "startup-reconciliation-wake",
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 2);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 2);
}

#[test]
fn startup_reconciliation_wake_suite_uses_a_vector_bound_definition_change_time() {
    let mut vectors: Value = serde_json::from_str(STARTUP_RECONCILIATION_WAKE_VECTORS).unwrap();
    vectors["cases"][1]["changedAt"] = json!("2099-09-02T08:30:00.000Z");

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        "startup-reconciliation-wake",
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 2);
}

#[test]
fn automation_action_time_override_is_persisted_by_the_native_api_route() {
    let vectors: Value = serde_json::from_str(STARTUP_RECONCILIATION_WAKE_VECTORS).unwrap();
    let case = &vectors["cases"][1];
    let temp = tempfile::tempdir().unwrap();
    crate::daemon::ensure_private_coven_home(temp.path()).unwrap();
    let store_path = temp.path().join("coven.sqlite3");
    let conn = crate::store::open_store(&store_path).unwrap();
    let definition = super::definition::RoutineDefinition::from_json(&case["definition"]).unwrap();
    super::store::insert_definition(&conn, &definition).unwrap();
    conn.execute(
        "UPDATE automation_definitions
         SET created_at = ?1, updated_at = ?1
         WHERE id = ?2",
        rusqlite::params![case["createdAt"].as_str().unwrap(), definition.id],
    )
    .unwrap();
    drop(conn);

    let body = json!({
        "action": "coven.automations.definition.revise.v1",
        "adoptionKey": "adopt:revise:definition-wake:timestamp",
        "expectedRevision": 1,
        "definition": case["revisedDefinition"],
    })
    .to_string();
    let changed_at = case["changedAt"].as_str().unwrap();
    let response = crate::api::handle_request_with_body_at(
        "POST",
        "/api/v1/actions",
        temp.path(),
        None,
        Some(&body),
        changed_at,
    )
    .unwrap();

    assert_eq!(response.status, 200);
    let conn = crate::store::open_store(&store_path).unwrap();
    let record = super::store::get_definition(&conn, &definition.id)
        .unwrap()
        .unwrap();
    assert_eq!(record.updated_at, changed_at);
}

#[test]
fn startup_reconciliation_wake_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(STARTUP_RECONCILIATION_WAKE_VECTORS).unwrap();
    vectors["cases"][1]["expected"]["lastPassTrigger"] = json!("deadline");

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        "startup-reconciliation-wake",
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn startup_reconciliation_wake_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 16] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["createdAt"] = json!("not-a-time"),
        |vectors| {
            vectors["cases"][1]
                .as_object_mut()
                .unwrap()
                .remove("changedAt")
                .map(|_| ())
                .unwrap()
        },
        |vectors| vectors["cases"][1]["changedAt"] = json!("not-a-time"),
        |vectors| vectors["cases"][1]["changedAt"] = vectors["cases"][1]["observedAt"].clone(),
        |vectors| vectors["cases"][0]["changedAt"] = json!("2099-09-02T08:00:00.000Z"),
        |vectors| vectors["cases"][0]["definition"]["status"] = json!("PAUSED"),
        |vectors| vectors["cases"][1]["revisedDefinition"]["status"] = json!("PAUSED"),
        |vectors| vectors["cases"][1]["expected"]["outcome"] = json!("startup_reconciled"),
        |vectors| vectors["cases"][0]["expected"]["launchCount"] = json!(0),
        |vectors| vectors["cases"][1]["expected"]["wakeObserved"] = json!(false),
        |vectors| vectors["cases"][0]["expected"]["productionStartPath"] = json!(false),
        |vectors| vectors["cases"][1]["expected"]["systemWaitObserved"] = json!(false),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(STARTUP_RECONCILIATION_WAKE_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                "startup-reconciliation-wake",
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn startup_reconciliation_wake_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(STARTUP_RECONCILIATION_WAKE_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for("startup-reconciliation-wake", vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn misfire_latest_planning_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(MISFIRE_LATEST_PLANNING_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        MISFIRE_LATEST_PLANNING_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 4);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 4);
}

#[test]
fn misfire_latest_planning_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(MISFIRE_LATEST_PLANNING_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["scheduledSlots"] = json!(["2026-09-03T09:00:00.000Z"]);

    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        MISFIRE_LATEST_PLANNING_SUITE,
        vectors,
    ))
    .unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn misfire_latest_planning_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 8] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][0]["createdAt"] = json!("not-a-time"),
        |vectors| vectors["cases"][0]["definition"]["status"] = json!("PAUSED"),
        |vectors| vectors["cases"][1]["existingScheduledFor"] = Value::Null,
        |vectors| vectors["cases"][2]["expected"]["firstOutcome"] = json!("planned"),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(MISFIRE_LATEST_PLANNING_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                MISFIRE_LATEST_PLANNING_SUITE,
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn misfire_latest_planning_suite_rejects_vacuous_timing_scenarios() {
    let invalid_mutations: [fn(&mut Value); 5] = [
        |vectors| vectors["cases"][0]["createdAt"] = json!("2026-09-03T10:00:00.000Z"),
        |vectors| vectors["cases"][1]["observedAt"] = json!("2026-09-04T08:00:00.000Z"),
        |vectors| vectors["cases"][2]["createdAt"] = json!("2026-09-03T13:00:00.000Z"),
        |vectors| {
            vectors["cases"][2]["createdAt"] = json!("2026-09-03T08:00:00.000Z");
            vectors["cases"][2]["observedAt"] = json!("2026-09-03T08:30:00.000Z");
            vectors["cases"][2]["existingScheduledFor"] = json!("2026-09-03T09:00:00.000Z");
            vectors["cases"][2]["expected"]["scheduledSlots"] = json!(["2026-09-03T09:00:00.000Z"]);
        },
        |vectors| vectors["cases"][3]["observedAt"] = json!("2026-09-01T08:30:00.000Z"),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(MISFIRE_LATEST_PLANNING_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                MISFIRE_LATEST_PLANNING_SUITE,
                vectors,
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn misfire_latest_planning_suite_requires_scheduler_profile() {
    let vectors: Value = serde_json::from_str(MISFIRE_LATEST_PLANNING_VECTORS).unwrap();
    assert_eq!(
        evaluate(&request_for(MISFIRE_LATEST_PLANNING_SUITE, vectors)).unwrap_err(),
        "conformance suite is unsupported"
    );
}

#[test]
fn definition_lifecycle_transitions_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(DEFINITION_LIFECYCLE_TRANSITIONS_VECTORS).unwrap();
    let response = evaluate(&request_for("definition-lifecycle-transitions", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 17);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 17);
}

#[test]
fn definition_lifecycle_transitions_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value =
        serde_json::from_str(DEFINITION_LIFECYCLE_TRANSITIONS_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["finalState"] = json!("active");

    let response = evaluate(&request_for("definition-lifecycle-transitions", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn definition_lifecycle_transitions_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 6] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone(),
        |vectors| vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone(),
        |vectors| vectors["cases"][3]["initialState"] = json!("active"),
        |vectors| vectors["cases"][3]["operation"] = json!("revise_paused"),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value =
            serde_json::from_str(DEFINITION_LIFECYCLE_TRANSITIONS_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for("definition-lifecycle-transitions", vectors)).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn rrule_vocabulary_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(RRULE_VOCABULARY_VECTORS).unwrap();
    let response = evaluate(&request_for("rrule-vocabulary", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 22);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 22);
}

#[test]
fn rrule_vocabulary_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(RRULE_VOCABULARY_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["byHour"] = json!([10]);

    let response = evaluate(&request_for("rrule-vocabulary", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn rrule_vocabulary_suite_reports_outcome_mismatches() {
    let mismatches: [fn(&mut Value); 2] = [
        |vectors| vectors["cases"][0]["expected"] = json!({"outcome": "rejected"}),
        |vectors| {
            vectors["cases"][4]["expected"] = json!({
                "outcome": "accepted",
                "frequency": "daily",
                "byHour": [9],
                "byDay": []
            })
        },
    ];

    for mismatch in mismatches {
        let mut vectors: Value = serde_json::from_str(RRULE_VOCABULARY_VECTORS).unwrap();
        mismatch(&mut vectors);
        let response = evaluate(&request_for("rrule-vocabulary", vectors)).unwrap();
        assert_eq!(response.status, TargetSuiteStatus::Failed);
        assert_eq!(response.evidence, None);
    }
}

#[test]
fn rrule_vocabulary_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 10] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][0]["rrule"] = json!(""),
        |vectors| vectors["cases"][0]["rrule"] = json!("x".repeat(1025)),
        |vectors| {
            vectors["cases"][2]["rrule"] = json!("FREQ=DAILY;BYHOUR=10");
            vectors["cases"][2]["expected"] = json!({
                "outcome": "accepted",
                "frequency": "daily",
                "byHour": [10],
                "byDay": []
            });
        },
        |vectors| vectors["cases"][4]["rrule"] = json!("FREQ=DAILY;COUNT=4"),
        |vectors| vectors["cases"][0]["expected"]["byHour"] = json!([17, 9]),
        |vectors| vectors["cases"][2]["expected"]["byDay"] = json!([]),
        |vectors| {
            vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone();
        },
        |vectors| {
            vectors["cases"][1]["scenario"] = vectors["cases"][0]["scenario"].clone();
        },
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(RRULE_VOCABULARY_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for("rrule-vocabulary", vectors)).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn command_adoption_idempotency_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(COMMAND_ADOPTION_IDEMPOTENCY_VECTORS).unwrap();
    let response = evaluate(&request_for("command-adoption-idempotency", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 1);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 1);
}

#[test]
fn command_adoption_idempotency_suite_rejects_impossible_expectations() {
    let mut vectors: Value = serde_json::from_str(COMMAND_ADOPTION_IDEMPOTENCY_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["eventRows"] = json!(2);

    assert_eq!(
        evaluate(&request_for("command-adoption-idempotency", vectors)).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn command_adoption_idempotency_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 8] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| vectors["cases"][0]["adoptionKey"] = json!("bad key"),
        |vectors| vectors["cases"][0]["definition"] = json!("not-an-object"),
        |vectors| {
            vectors["cases"][0]["conflictingDefinition"]["id"] = json!("different-automation")
        },
        |vectors| {
            vectors["cases"][0]["conflictingDefinition"] = vectors["cases"][0]["definition"].clone()
        },
        |vectors| {
            let mut duplicate = vectors["cases"][0].clone();
            duplicate["adoptionKey"] = json!("adopt:create:conformance:0002");
            vectors["cases"].as_array_mut().unwrap().push(duplicate);
        },
        |vectors| {
            let mut duplicate = vectors["cases"][0].clone();
            duplicate["caseId"] = json!("second-case");
            vectors["cases"].as_array_mut().unwrap().push(duplicate);
        },
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value =
            serde_json::from_str(COMMAND_ADOPTION_IDEMPOTENCY_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for("command-adoption-idempotency", vectors)).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn command_envelope_outcomes_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(COMMAND_ENVELOPE_OUTCOMES_VECTORS).unwrap();
    let response = evaluate(&request_for("command-envelope-outcomes", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 6);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 6);
}

#[test]
fn command_envelope_outcomes_suite_fails_when_the_router_disagrees() {
    let wrong_expectations: [fn(&mut Value); 10] = [
        // The refused retry would have to commit.
        |vectors| {
            vectors["cases"][1]["steps"][0]["expect"] = json!({ "outcome": "committed" });
        },
        // The retained NOT_FOUND retry refusal would have to leave no adoption;
        // the refused delivery variant would also have to leave no adoption.
        |vectors| vectors["cases"][1]["steps"][0]["expect"]["writes"] = json!("nothing"),
        |vectors| vectors["cases"][1]["steps"][2]["expect"]["writes"] = json!("nothing"),
        // The stale revise would have to be refused for a different reason.
        |vectors| {
            vectors["cases"][2]["steps"][2]["expect"]["errorCode"] = json!("NOT_FOUND");
        },
        // The duplicate create would have to add a revision.
        |vectors| vectors["cases"][3]["steps"][1]["expect"]["revision"] = json!(2),
        // The replayed revise would have to return the create's result.
        |vectors| {
            vectors["cases"][3]["steps"][3]["envelope"] =
                vectors["cases"][3]["steps"][0]["envelope"].clone();
            vectors["cases"][3]["steps"][3]["expect"] =
                json!({ "outcome": "replayed", "revision": 2, "replays": 0 });
        },
        // Adoption would have to be forgotten across the restart.
        |vectors| {
            vectors["cases"][4]["steps"][2]["expect"] =
                json!({ "outcome": "committed", "revision": 1 });
        },
        // A missing cancellation target still retains its refusal adoption.
        |vectors| vectors["cases"][5]["steps"][0]["expect"]["writes"] = json!("nothing"),
        // Repeating that request must not create a second adoption.
        |vectors| vectors["cases"][5]["steps"][1]["expect"]["writes"] = json!("adoption"),
        // The same key cannot silently switch to another cancellation target.
        |vectors| vectors["cases"][5]["steps"][2]["expect"]["errorCode"] = json!("NOT_FOUND"),
    ];

    for mutate in wrong_expectations {
        let mut vectors: Value = serde_json::from_str(COMMAND_ENVELOPE_OUTCOMES_VECTORS).unwrap();
        mutate(&mut vectors);
        let response = evaluate(&request_for("command-envelope-outcomes", vectors)).unwrap();
        assert_eq!(response.status, TargetSuiteStatus::Failed);
    }
}

#[test]
fn mission_commands_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(MISSION_COMMANDS_VECTORS).unwrap();
    let response = evaluate(&request_for("mission-commands", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 10);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 10);
}

#[test]
fn mission_commands_suite_fails_when_the_executors_disagree() {
    let wrong_expectations: [fn(&mut Value); 8] = [
        // A lost acknowledgement would have to commit a second time.
        |vectors| {
            vectors["cases"][1]["steps"][1]["expect"] =
                json!({ "outcome": "committed", "state": "queued" });
        },
        // The changed request under a used key would have to be adopted.
        |vectors| vectors["cases"][2]["steps"][1]["expect"]["writes"] = json!("adoption"),
        // The stale revision would have to name itself as current.
        |vectors| {
            vectors["cases"][3]["steps"][0]["expect"]["details"] =
                json!({ "currentRevisionId": "{{staleRevisionId}}" });
        },
        // An unenforced envelope would have to be accepted.
        |vectors| {
            vectors["cases"][4]["steps"][0]["expect"] =
                json!({ "outcome": "committed", "state": "queued" });
        },
        // Without Runtime Authority the mission would have to stay queued.
        |vectors| vectors["cases"][5]["steps"][1]["expect"]["state"] = json!("queued"),
        // A settled mission would have to be cancelled.
        |vectors| vectors["cases"][6]["steps"][1]["expect"]["state"] = json!("cancelled"),
        // Another caller would have to learn the mission exists.
        |vectors| {
            vectors["cases"][7]["steps"][1]["expect"] =
                json!({ "outcome": "committed", "state": "unavailable" });
        },
        // A 61-minute timeout would have to pass.
        |vectors| {
            vectors["cases"][8]["steps"][1]["expect"] =
                json!({ "outcome": "committed", "state": "queued" });
        },
    ];

    for mutate in wrong_expectations {
        let mut vectors: Value = serde_json::from_str(MISSION_COMMANDS_VECTORS).unwrap();
        mutate(&mut vectors);
        let response = evaluate(&request_for("mission-commands", vectors)).unwrap();
        assert_eq!(response.status, TargetSuiteStatus::Failed);
    }
}

#[test]
fn mission_commands_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 7] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        // A get takes no adoption key, and a submit needs a payload.
        |vectors| vectors["cases"][0]["steps"][0]["adoptionKey"] = json!("adopt:mission:get"),
        |vectors| {
            vectors["cases"][0]["steps"][1]
                .as_object_mut()
                .unwrap()
                .remove("payload");
        },
        // A replay must name an earlier, identical commit.
        |vectors| vectors["cases"][1]["steps"][1]["adoptionKey"] = json!("adopt:mission:other"),
        // A refusal must say what it writes.
        |vectors| {
            vectors["cases"][3]["steps"][0]["expect"]
                .as_object_mut()
                .unwrap()
                .remove("writes");
        },
        // Every token must be one the suite knows.
        |vectors| vectors["cases"][1]["steps"][0]["payload"]["project"]["cwd"] = json!("{{home}}"),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(MISSION_COMMANDS_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for("mission-commands", vectors)).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn command_envelope_outcomes_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 11] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| {
            let duplicate = vectors["cases"][0].clone();
            vectors["cases"].as_array_mut().unwrap().push(duplicate);
        },
        |vectors| vectors["cases"][0]["steps"] = json!([]),
        |vectors| vectors["cases"][4]["steps"][1] = json!({ "kind": "reboot" }),
        |vectors| vectors["cases"][0]["steps"][0]["envelope"]["command"] = json!("made.up.v1"),
        |vectors| vectors["cases"][0]["steps"][0]["envelope"]["adoptionKey"] = json!("bad key"),
        // A rejection must name its code and what it writes, only a
        // rejection says what it writes, and a replay names an earlier
        // committed send of the same envelope.
        |vectors| {
            vectors["cases"][1]["steps"][0]["expect"]
                .as_object_mut()
                .unwrap()
                .remove("errorCode");
        },
        |vectors| {
            vectors["cases"][1]["steps"][2]["expect"]
                .as_object_mut()
                .unwrap()
                .remove("writes");
        },
        |vectors| vectors["cases"][0]["steps"][0]["expect"]["writes"] = json!("nothing"),
        |vectors| vectors["cases"][3]["steps"][1]["expect"]["replays"] = json!(2),
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value = serde_json::from_str(COMMAND_ENVELOPE_OUTCOMES_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for("command-envelope-outcomes", vectors)).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn receipt_integrity_validation_suite_executes_the_checked_in_vectors() {
    let vectors: Value = serde_json::from_str(RECEIPT_INTEGRITY_VALIDATION_VECTORS).unwrap();
    let response = evaluate(&request_for("receipt-integrity-validation", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Passed);
    assert_eq!(response.evidence.as_ref().unwrap()["executedCases"], 2);
    assert_eq!(response.evidence.as_ref().unwrap()["passedCases"], 2);
}

#[test]
fn receipt_integrity_validation_suite_fails_closed_on_an_expectation_mismatch() {
    let mut vectors: Value = serde_json::from_str(RECEIPT_INTEGRITY_VALIDATION_VECTORS).unwrap();
    vectors["cases"][0]["expected"]["normalizedDigest"] =
        json!("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

    let response = evaluate(&request_for("receipt-integrity-validation", vectors)).unwrap();

    assert_eq!(response.status, TargetSuiteStatus::Failed);
    assert_eq!(response.evidence, None);
}

#[test]
fn receipt_integrity_validation_suite_rejects_invalid_vector_shapes() {
    let invalid_mutations: [fn(&mut Value); 9] = [
        |vectors| vectors["schemaVersion"] = json!("unsupported"),
        |vectors| vectors["cases"][0]["caseId"] = json!("-bad-case-id"),
        |vectors| {
            vectors["cases"][1]["caseId"] = vectors["cases"][0]["caseId"].clone();
        },
        |vectors| vectors["cases"][0]["receipt"] = json!("not-an-object"),
        |vectors| {
            vectors["cases"][1]["receipt"] = json!({ "schemaVersion": "coven.automations.v1" })
        },
        |vectors| {
            vectors["cases"][1]["receipt"]["integrity"]["value"] = json!("not-a-sha256-digest")
        },
        |vectors| {
            vectors["cases"][0]["expected"]["normalizedDigest"] = json!("sha256:not-a-digest");
        },
        |vectors| vectors["cases"][0]["expected"] = json!({"outcome": "rejected"}),
        |vectors| {
            vectors["cases"][1]["expected"] = json!({
                "outcome": "accepted",
                "normalizedDigest":
                    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            });
        },
    ];

    for mutate in invalid_mutations {
        let mut vectors: Value =
            serde_json::from_str(RECEIPT_INTEGRITY_VALIDATION_VECTORS).unwrap();
        mutate(&mut vectors);
        assert_eq!(
            evaluate(&request_for("receipt-integrity-validation", vectors)).unwrap_err(),
            "conformance vector is invalid"
        );
    }
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

fn storage_lock_request() -> Value {
    request_for_profile(
        "scheduler_reliability",
        "command-storage-lock-recovery",
        serde_json::from_str(include_str!(
            "../../../../conformance/automations/runner/command-storage-lock-recovery.vectors.json"
        ))
        .unwrap(),
    )
}

#[test]
fn storage_lock_suite_executes_real_lock_and_unlocked_control() {
    let response = evaluate(&storage_lock_request()).unwrap();
    assert_eq!(response.status, TargetSuiteStatus::Passed);
}

#[test]
fn storage_lock_suite_rejects_missing_control_and_duplicate_cases() {
    let mut missing = storage_lock_request();
    missing["vector"]["cases"].as_array_mut().unwrap().pop();
    assert_eq!(
        evaluate(&missing).unwrap_err(),
        "conformance vector is invalid"
    );
    let mut duplicate = storage_lock_request();
    duplicate["vector"]["cases"][1] = duplicate["vector"]["cases"][0].clone();
    assert_eq!(
        evaluate(&duplicate).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn storage_lock_suite_rejects_weakened_invariant_expectations() {
    for case_index in 0..2 {
        for (field, wrong) in [
            ("initialRows", json!(3)),
            ("finalRows", json!(2)),
            ("replayPreserved", json!(false)),
            (
                "initialOutcome",
                json!(if case_index == 0 { "committed" } else { "busy" }),
            ),
        ] {
            let mut request = storage_lock_request();
            request["vector"]["cases"][case_index]["expected"][field] = wrong;
            assert_eq!(
                evaluate(&request).unwrap_err(),
                "conformance vector is invalid",
                "case {case_index}: {field}"
            );
        }
    }
}

#[test]
fn storage_lock_suite_rejects_malformed_vectors() {
    let original = storage_lock_request();
    let mut invalid = Vec::new();
    let mut request = original.clone();
    request["vector"]["schemaVersion"] = json!("unknown");
    invalid.push(request);
    let mut request = original.clone();
    request["vector"]["cases"][0]["unexpected"] = json!(true);
    invalid.push(request);
    let mut request = original.clone();
    request["vector"]["cases"][0]["adoptionKey"] = json!("");
    invalid.push(request);
    let mut request = original.clone();
    request["vector"]["cases"][0]["definition"] = json!({});
    invalid.push(request);
    let mut request = original;
    request["vector"]["cases"][0]["expected"]["initialOutcome"] = json!("any_error");
    invalid.push(request);
    for request in invalid {
        assert_eq!(
            evaluate(&request).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

const COMMAND_TRANSACTION_ROLLBACK_VECTORS: &str = include_str!(
    "../../../../conformance/automations/runner/command-transaction-rollback.vectors.json"
);

#[test]
fn transaction_rollback_suite_executes_checked_in_faults_and_control() {
    let vector = serde_json::from_str(COMMAND_TRANSACTION_ROLLBACK_VECTORS).unwrap();
    let response = evaluate(&request_for_profile(
        "scheduler_reliability",
        "command-transaction-rollback",
        vector,
    ))
    .unwrap();
    assert_eq!(response.status, TargetSuiteStatus::Passed);
}

#[test]
fn transaction_rollback_suite_rejects_weakened_invariants() {
    for index in 0..3 {
        for (field, value) in [
            ("initialRows", json!(7)),
            ("finalRows", json!(0)),
            ("replayPreserved", json!(false)),
        ] {
            let mut vector: Value =
                serde_json::from_str(COMMAND_TRANSACTION_ROLLBACK_VECTORS).unwrap();
            vector["cases"][index]["expected"][field] = value;
            assert_eq!(
                evaluate(&request_for_profile(
                    "scheduler_reliability",
                    "command-transaction-rollback",
                    vector
                ))
                .unwrap_err(),
                "conformance vector is invalid"
            );
        }
    }
}

#[test]
fn transaction_rollback_suite_requires_both_faults_and_control() {
    for index in 0..3 {
        let mut vector: Value = serde_json::from_str(COMMAND_TRANSACTION_ROLLBACK_VECTORS).unwrap();
        vector["cases"].as_array_mut().unwrap().remove(index);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                "command-transaction-rollback",
                vector
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn transaction_rollback_suite_rejects_invalid_inputs_before_execution() {
    let mutations: [fn(&mut Value); 6] = [
        |vector| vector["cases"][1]["fault"] = json!("event_insert"),
        |vector| vector["cases"][1]["caseId"] = vector["cases"][0]["caseId"].clone(),
        |vector| vector["cases"][0]["unexpected"] = json!("PRIVATE-ROLLBACK-MARKER"),
        |vector| vector["cases"][0]["definition"]["integrity"]["value"] = json!("0".repeat(64)),
        |vector| vector["cases"][0]["adoptionKey"] = json!(""),
        |vector| vector["cases"][0]["fault"] = json!("PRIVATE-ROLLBACK-MARKER"),
    ];
    for mutate in mutations {
        let mut vector: Value = serde_json::from_str(COMMAND_TRANSACTION_ROLLBACK_VECTORS).unwrap();
        mutate(&mut vector);
        assert_eq!(
            evaluate(&request_for_profile(
                "scheduler_reliability",
                "command-transaction-rollback",
                vector
            ))
            .unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn storage_readonly_suite_executes_real_refusal_recovery_and_writable_control() {
    let vector: Value = serde_json::from_str(include_str!(
        "../../../../conformance/automations/runner/command-storage-readonly-recovery.vectors.json"
    ))
    .unwrap();
    let request = request_for_profile(
        "scheduler_reliability",
        "command-storage-readonly-recovery",
        vector,
    );
    let response = evaluate(&request).unwrap();
    assert_eq!(
        response.status,
        TargetSuiteStatus::Passed,
        "real read-only recovery suite must execute"
    );
    assert!(response.evidence.is_some());
}

#[test]
fn storage_readonly_suite_rejects_missing_controls_and_weakened_expectations() {
    let original: Value = serde_json::from_str(include_str!(
        "../../../../conformance/automations/runner/command-storage-readonly-recovery.vectors.json"
    ))
    .unwrap();
    for (pointer, value) in [
        ("/schemaVersion", json!("unknown")),
        ("/cases/0/fault", json!("none")),
        ("/cases/1/fault", json!("sqlite_open_read_only")),
        ("/cases/0/expected/initialOutcome", json!("committed")),
        ("/cases/0/expected/baselinePreserved", json!(false)),
        ("/cases/0/expected/initialRows", json!(1)),
        ("/cases/0/expected/finalRows", json!(0)),
        ("/cases/0/expected/replayPreserved", json!(false)),
        ("/cases/0/adoptionKey", json!("")),
        ("/cases/0/adoptionKey", json!("adopt:readonly:baseline")),
        ("/cases/0/definition/id", json!("readonly-preexisting")),
        ("/cases/0/definition/status", json!("ACTIVE")),
        ("/cases/1/caseId", json!("read-only-refuses-then-recovers")),
        ("/cases/0/caseId", json!("../unsafe")),
    ] {
        let mut vector = original.clone();
        *vector.pointer_mut(pointer).unwrap() = value;
        let request = request_for_profile(
            "scheduler_reliability",
            "command-storage-readonly-recovery",
            vector,
        );
        assert_eq!(
            evaluate(&request).unwrap_err(),
            "conformance vector is invalid",
            "accepted weakened vector at {pointer}"
        );
    }
    let mut vector = original;
    vector["cases"].as_array_mut().unwrap().pop();
    let request = request_for_profile(
        "scheduler_reliability",
        "command-storage-readonly-recovery",
        vector,
    );
    assert_eq!(
        evaluate(&request).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn storage_readonly_suite_rejects_unknown_fixture_fields() {
    let original: Value = serde_json::from_str(include_str!(
        "../../../../conformance/automations/runner/command-storage-readonly-recovery.vectors.json"
    ))
    .unwrap();
    for pointer in ["", "/cases/0", "/cases/0/expected"] {
        let mut vector = original.clone();
        vector.pointer_mut(pointer).unwrap()["unknown"] = json!(true);
        let request = request_for_profile(
            "scheduler_reliability",
            "command-storage-readonly-recovery",
            vector,
        );
        assert_eq!(
            evaluate(&request).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

fn storage_capacity_request() -> Value {
    request_for_profile(
        "scheduler_reliability",
        "command-storage-capacity-recovery",
        serde_json::from_str(include_str!(
            "../../../../conformance/automations/runner/command-storage-capacity-recovery.vectors.json"
        ))
        .unwrap(),
    )
}

#[test]
fn storage_capacity_suite_executes_real_quota_and_unbounded_control() {
    let response = evaluate(&storage_capacity_request()).unwrap();
    assert_eq!(response.status, TargetSuiteStatus::Passed);
}

#[test]
fn storage_capacity_suite_rejects_missing_control_and_duplicate_cases() {
    let mut missing = storage_capacity_request();
    missing["vector"]["cases"].as_array_mut().unwrap().pop();
    assert_eq!(
        evaluate(&missing).unwrap_err(),
        "conformance vector is invalid"
    );
    let mut duplicate = storage_capacity_request();
    duplicate["vector"]["cases"][1] = duplicate["vector"]["cases"][0].clone();
    assert_eq!(
        evaluate(&duplicate).unwrap_err(),
        "conformance vector is invalid"
    );
}

#[test]
fn storage_capacity_suite_rejects_weakened_invariant_expectations() {
    for case_index in 0..2 {
        for (field, wrong) in [
            ("initialRows", json!(3)),
            ("finalRows", json!(2)),
            ("replayPreserved", json!(false)),
            ("baselinePreserved", json!(false)),
            (
                "initialOutcome",
                json!(if case_index == 0 {
                    "committed"
                } else {
                    "disk_full"
                }),
            ),
        ] {
            let mut request = storage_capacity_request();
            request["vector"]["cases"][case_index]["expected"][field] = wrong;
            assert_eq!(
                evaluate(&request).unwrap_err(),
                "conformance vector is invalid",
                "case {case_index}: {field}"
            );
        }
    }
}

fn storage_corruption_request() -> Value {
    request_for_profile(
        "scheduler_reliability",
        "command-storage-corruption-recovery",
        serde_json::from_str(include_str!(
            "../../../../conformance/automations/runner/command-storage-corruption-recovery.vectors.json"
        ))
        .unwrap(),
    )
}

fn storage_corruption_observations() -> Vec<Value> {
    let result = evaluate(&storage_corruption_request()).unwrap();
    assert_eq!(result.status, TargetSuiteStatus::Passed);
    result.evidence.unwrap()["observations"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn storage_corruption_suite_executes_real_faults_and_control() {
    let observations = storage_corruption_observations();
    assert_eq!(observations.len(), 4);
    for (observation, (fault, outcome, code, initial)) in observations.iter().zip([
        ("sqlite_header", "not_a_database", json!(26), 0),
        ("definition_btree", "database_corrupt", json!(11), 0),
        ("none", "committed", Value::Null, 1),
        ("adoption_btree", "database_corrupt", json!(11), 0),
    ]) {
        assert_eq!(observation["faultSource"], fault, "{observation}");
        assert_eq!(observation["initialOutcome"], outcome, "{observation}");
        assert_eq!(observation["sqliteErrorCode"], code, "{observation}");
        assert_eq!(observation["initialAddedDefinitions"], initial);
        assert_eq!(observation["finalAddedDefinitions"], 1);
        // The probe path is fresh by construction; this shows the observed
        // production initializer actually ran on it.
        assert_eq!(observation["coldInitializationEntered"], true);
        for field in [
            "baselinePreserved",
            "storeNotReset",
            "oneByteRestorePreservedOtherBytes",
            "replayPreserved",
        ] {
            assert_eq!(observation[field], true, "{field}: {observation}");
        }
    }
    // The header and definition faults are refused by the cold initializer
    // itself, before the definition command is reachable, and the refusal
    // leaves the corrupted main file byte-for-byte as written. The control is
    // never refused.
    for observation in &observations[..2] {
        assert_eq!(observation["refusalStage"], "cold_open", "{observation}");
        assert_eq!(observation["refusalLeftMainFileUnchanged"], true);
        assert_eq!(observation["coldInitializationCompleted"], false);
        assert_eq!(observation["initialTransactionClosed"], Value::Null);
        assert_eq!(observation["refusedAfterEarlierWrites"], Value::Null);
    }
    let control = &observations[2];
    assert_eq!(control["refusalStage"], Value::Null, "{control}");
    assert_eq!(control["refusalLeftMainFileUnchanged"], Value::Null);
    assert_eq!(control["coldInitializationCompleted"], true);
    assert_eq!(control["initialTransactionClosed"], true);
    assert_eq!(control["refusedAfterEarlierWrites"], Value::Null);
    // The cold initializer never reads the adoption table's root page, so the
    // command reaches the fault itself, after completing earlier writes. None
    // of them persisted: the connection is left outside a transaction, the
    // corrupted main file is unchanged and no WAL remains.
    let adoption = &observations[3];
    assert_eq!(adoption["refusalStage"], "command", "{adoption}");
    assert_eq!(adoption["coldInitializationCompleted"], true);
    assert_eq!(adoption["refusedAfterEarlierWrites"], true);
    assert_eq!(adoption["initialTransactionClosed"], true);
    assert_eq!(adoption["refusalLeftMainFileUnchanged"], true);
    assert_eq!(adoption["walObservedAfterInitial"], false);
}

#[test]
fn storage_corruption_suite_proves_populated_baselines_in_every_observed_table() {
    for observation in storage_corruption_observations() {
        let counts = observation["baselineTableRows"]
            .as_array()
            .expect("each case must prove populated preexisting state");
        assert_eq!(counts.len(), 11);
        assert!(
            counts.iter().all(|count| count.as_u64().unwrap() > 0),
            "{counts:?}"
        );
    }
}

#[test]
fn storage_corruption_evidence_carries_no_locations_or_contents() {
    let result = evaluate(&storage_corruption_request()).unwrap();
    let text = serde_json::to_string(&result.evidence).unwrap();
    for forbidden in [
        "/",
        "\\",
        ".sqlite",
        "probe",
        "offset",
        "Synthetic rich baseline",
        "native adoption conformance probe",
        "corruption-preexisting",
        "adopt:",
    ] {
        assert!(!text.contains(forbidden), "{forbidden} leaked: {text}");
    }
}

#[test]
fn storage_corruption_suite_rejects_missing_controls_and_weakened_expectations() {
    for (pointer, value) in [
        ("/schemaVersion", json!("unknown")),
        (
            "/schemaVersion",
            json!("coven.automations.command-storage-corruption-recovery-vectors.v2"),
        ),
        ("/cases/0/fault", json!("none")),
        ("/cases/1/fault", json!("sqlite_header")),
        ("/cases/2/fault", json!("definition_btree")),
        ("/cases/3/fault", json!("definition_btree")),
        ("/cases/3/fault", json!("none")),
        (
            "/cases/0/expected/initialOutcome",
            json!("database_corrupt"),
        ),
        ("/cases/1/expected/initialOutcome", json!("not_a_database")),
        ("/cases/2/expected/initialOutcome", json!("not_a_database")),
        ("/cases/3/expected/initialOutcome", json!("committed")),
        ("/cases/3/expected/initialRows", json!(1)),
        ("/cases/0/expected/refusalStage", json!("command")),
        ("/cases/1/expected/refusalStage", Value::Null),
        ("/cases/2/expected/refusalStage", json!("command")),
        ("/cases/3/expected/refusalStage", json!("cold_open")),
        ("/cases/0/expected/initialRows", json!(1)),
        ("/cases/2/expected/initialRows", json!(0)),
        ("/cases/1/expected/finalRows", json!(0)),
        ("/cases/1/expected/baselinePreserved", json!(false)),
        ("/cases/1/expected/storeNotReset", json!(false)),
        ("/cases/2/expected/replayPreserved", json!(false)),
        ("/cases/0/adoptionKey", json!("")),
        ("/cases/0/adoptionKey", json!("adopt:corruption:baseline")),
        (
            "/cases/1/adoptionKey",
            json!("adopt:corruption:pending-result"),
        ),
        ("/cases/2/adoptionKey", json!("adopt:corruption:attempt-1")),
        ("/cases/0/definition/id", json!("corruption-preexisting")),
        (
            "/cases/2/definition/id",
            json!("corruption-preexisting-rich"),
        ),
        ("/cases/0/definition/status", json!("ACTIVE")),
        (
            "/cases/1/caseId",
            json!("corrupt-header-refuses-then-recovers"),
        ),
        ("/cases/0/caseId", json!("../unsafe")),
    ] {
        let mut request = storage_corruption_request();
        *request["vector"].pointer_mut(pointer).unwrap() = value;
        assert_eq!(
            evaluate(&request).unwrap_err(),
            "conformance vector is invalid",
            "accepted weakened vector at {pointer}"
        );
    }
}

#[test]
fn storage_corruption_suite_rejects_missing_duplicate_and_caller_chosen_fields() {
    let mut missing = storage_corruption_request();
    missing["vector"]["cases"].as_array_mut().unwrap().pop();
    let mut duplicate = storage_corruption_request();
    duplicate["vector"]["cases"][2] = duplicate["vector"]["cases"][0].clone();
    let mut requests = vec![missing, duplicate];
    for (pointer, field) in [
        ("", "unknown"),
        ("/cases/0", "path"),
        ("/cases/0", "offset"),
        ("/cases/1", "bytes"),
        ("/cases/0/expected", "unknown"),
    ] {
        let mut request = storage_corruption_request();
        request["vector"].pointer_mut(pointer).unwrap()[field] = json!(0);
        requests.push(request);
    }
    for request in requests {
        assert_eq!(
            evaluate(&request).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn storage_corruption_suite_is_scheduler_reliability_only() {
    let mut request = storage_corruption_request();
    request["profile"] = json!("structural");
    assert_eq!(
        evaluate(&request).unwrap_err(),
        "conformance suite is unsupported"
    );
}

fn result_delivery_request() -> Value {
    request_for_profile(
        "scheduler_reliability",
        "result-delivery-recovery",
        serde_json::from_str(include_str!(
            "../../../../conformance/automations/runner/result-delivery-recovery.vectors.json"
        ))
        .unwrap(),
    )
}

fn result_delivery_observations() -> Vec<Value> {
    let result = evaluate(&result_delivery_request()).unwrap();
    assert_eq!(result.status, TargetSuiteStatus::Passed);
    result.evidence.unwrap()["observations"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn result_delivery_suite_executes_every_fault_through_production_discharge() {
    let observations = result_delivery_observations();
    assert_eq!(observations.len(), 6);
    for (observation, (fault, first, after_first, final_state, notices)) in
        observations.iter().zip([
            ("none", "delivered", "delivered", "delivered", 1),
            ("gate_held", "deferred", "pending", "delivered", 1),
            ("inbox_full", "deferred", "pending", "delivered", 1),
            (
                "discharge_rollback",
                "pass_failed",
                "pending",
                "delivered",
                1,
            ),
            ("gate_unavailable", "failed", "failed", "failed", 0),
            ("unreadable_result", "failed", "failed", "failed", 0),
        ])
    {
        assert_eq!(observation["faultSource"], fault, "{observation}");
        assert_eq!(observation["obligationPendingBeforeFault"], true);
        assert_eq!(observation["firstPass"], first, "{observation}");
        assert_eq!(observation["stateAfterFirstPass"], after_first);
        assert_eq!(observation["finalState"], final_state);
        assert_eq!(observation["noticesForRun"], notices);
        assert_eq!(observation["replayIsNoop"], true);
        assert_eq!(observation["settledRecordImmutable"], true);
        // Only a delivered first pass may leave a notice behind it.
        assert_eq!(
            observation["noticesAfterFirstPass"],
            u64::from(first == "delivered")
        );
        let delivered = final_state == "delivered";
        assert_eq!(
            observation["recordNamesNotice"],
            if delivered { json!(true) } else { Value::Null }
        );
    }
    // The rollback is the fixture's own abort, after the notice insert; the
    // notice rolled back with the record, so the run stayed pending.
    assert_eq!(observations[3]["passFailureFromFixture"], true);
    assert_eq!(observations[2]["waitingNoticesPreserved"], true);
    assert_eq!(observations[4]["failureReason"], "gate_unavailable");
    assert_eq!(observations[5]["failureReason"], "result_unreadable");
    // The unreadable run fails alone: the run settled after it still posts.
    assert_eq!(observations[5]["healthyRunDelivered"], true);
    for (index, field) in [
        (3, "passFailureFromFixture"),
        (2, "waitingNoticesPreserved"),
        (5, "healthyRunDelivered"),
    ] {
        for (other, observation) in observations.iter().enumerate() {
            if other != index {
                assert_eq!(observation[field], Value::Null, "{field}: {observation}");
            }
        }
    }
}

#[test]
fn result_delivery_evidence_carries_no_identifiers_or_contents() {
    let result = evaluate(&result_delivery_request()).unwrap();
    let text = serde_json::to_string(&result.evidence).unwrap();
    for forbidden in [
        "/",
        "\\",
        "delivery-conformance",
        "conformance-delivery",
        "conformance-handoff",
        "Conformance handoff",
        "native delivery conformance probe",
        "instance:main",
        "main-session-locks",
        "notice-",
    ] {
        assert!(!text.contains(forbidden), "{forbidden} leaked: {text}");
    }
}

#[test]
fn result_delivery_suite_rejects_missing_controls_and_weakened_expectations() {
    for (pointer, value) in [
        (
            "/schemaVersion",
            json!("coven.automations.result-delivery-recovery-vectors.v2"),
        ),
        ("/cases/0/fault", json!("gate_held")),
        ("/cases/5/fault", json!("none")),
        ("/cases/0/expected/firstPass", json!("deferred")),
        ("/cases/1/expected/firstPass", json!("delivered")),
        ("/cases/3/expected/firstPass", json!("deferred")),
        ("/cases/4/expected/firstPass", json!("deferred")),
        (
            "/cases/4/expected/failureReason",
            json!("result_unreadable"),
        ),
        ("/cases/5/expected/failureReason", Value::Null),
        ("/cases/2/expected/finalState", json!("pending")),
        ("/cases/4/expected/finalState", json!("delivered")),
        ("/cases/3/expected/finalNotices", json!(2)),
        ("/cases/4/expected/finalNotices", json!(1)),
        ("/cases/0/expected/replayIsNoop", json!(false)),
        ("/cases/4/expected/settledRecordImmutable", json!(false)),
        ("/cases/0/definition/outputTarget", Value::Null),
        ("/cases/0/definition/outputTarget", json!({"kind": "notes"})),
        ("/cases/0/definition/status", json!("ACTIVE")),
        ("/cases/0/definition/cwd", json!("/work/project")),
        ("/cases/1/caseId", json!("uninterrupted-delivery-control")),
        ("/cases/0/caseId", json!("../unsafe")),
    ] {
        let mut request = result_delivery_request();
        let vector = &mut request["vector"];
        match vector.pointer_mut(pointer) {
            Some(slot) => *slot = value,
            None => {
                let (parent, field) = pointer.rsplit_once('/').unwrap();
                vector.pointer_mut(parent).unwrap()[field] = value;
            }
        }
        assert_eq!(
            evaluate(&request).unwrap_err(),
            "conformance vector is invalid",
            "accepted weakened vector at {pointer}"
        );
    }
}

#[test]
fn result_delivery_suite_rejects_missing_duplicate_and_caller_chosen_fields() {
    let mut missing = result_delivery_request();
    missing["vector"]["cases"].as_array_mut().unwrap().pop();
    let mut duplicate = result_delivery_request();
    duplicate["vector"]["cases"][5] = duplicate["vector"]["cases"][0].clone();
    // A second control under its own case id, replacing a fault.
    let mut duplicate_fault = result_delivery_request();
    let mut control = duplicate_fault["vector"]["cases"][0].clone();
    control["caseId"] = json!("second-uninterrupted-delivery-control");
    duplicate_fault["vector"]["cases"][5] = control;
    let mut requests = vec![missing, duplicate, duplicate_fault];
    for (pointer, field) in [
        ("", "unknown"),
        ("/cases/0", "runId"),
        ("/cases/1", "path"),
        ("/cases/2", "noticeText"),
        ("/cases/0/expected", "unknown"),
    ] {
        let mut request = result_delivery_request();
        request["vector"].pointer_mut(pointer).unwrap()[field] = json!(0);
        requests.push(request);
    }
    for request in requests {
        assert_eq!(
            evaluate(&request).unwrap_err(),
            "conformance vector is invalid"
        );
    }
}

#[test]
fn result_delivery_suite_is_scheduler_reliability_only() {
    let mut request = result_delivery_request();
    request["profile"] = json!("structural");
    assert_eq!(
        evaluate(&request).unwrap_err(),
        "conformance suite is unsupported"
    );
}
